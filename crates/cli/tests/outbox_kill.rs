// SPDX-License-Identifier: AGPL-3.0-or-later
//! "The outbox survives `kill -9`" (`docs/plan.md` M2): a writing command is killed with
//! SIGKILL while its write is in flight, and the next command completes the write exactly once.
//!
//! 1. Killed after the PDS applied the write but before the answer arrived (a latency fault on
//!    `createRecord`; the fake PDS applies a request before it sleeps): the restarted outbox
//!    finds the record and marks the write done without sending it again.
//! 2. Killed after the write was persisted and picked, before anything was sent (a latency
//!    fault on the fresh `getLatestCommit` an edit's `swapCommit` comes from): the restarted
//!    outbox checks the PDS, finds nothing applied, and sends the edit once.

#![cfg(unix)]
#![allow(clippy::disallowed_methods)] // real processes, a real clock and real waits

#[path = "support/fakenet.rs"]
mod fakenet;

use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use fakenet::{Net, User, s, text};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::Fault;
use serde_json::Value;

/// Waits until the fake network has logged a request for `nsid` (by `did`) after `after`.
fn wait_for(net: &Net, nsid: &str, did: &str, after: u64, child: &mut Child) {
    let start = Instant::now();
    loop {
        let seen = net.net.requests().iter().any(|r| {
            r.seq > after && r.nsid.as_deref() == Some(nsid) && r.did.as_deref() == Some(did)
        });
        if seen {
            return;
        }
        if let Ok(Some(st)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            panic!("the command ended ({st}) before {nsid}: {err}");
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "no {nsid} request"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn last_seq(net: &Net) -> u64 {
    net.net.requests().last().map_or(0, |r| r.seq)
}

fn count(net: &Net, nsid: &str, did: &str, after: u64) -> usize {
    net.net
        .requests()
        .iter()
        .filter(|r| {
            r.seq > after && r.nsid.as_deref() == Some(nsid) && r.did.as_deref() == Some(did)
        })
        .count()
}

/// Starts `lessdugin --json <args>` in the background.
fn spawn(u: &User, args: &[&str]) -> Child {
    u.std_cmd()
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn kill_9(mut child: Child) {
    // `Child::kill` is SIGKILL on unix
    child.kill().unwrap();
    let st = child.wait().unwrap();
    assert!(!st.success());
}

fn writes(u: &User) -> Vec<Value> {
    let v = u.json(&["--offline", "outbox"]);
    v["writes"].as_array().unwrap().clone()
}

#[test]
fn a_write_killed_while_the_pds_answers_is_not_sent_twice() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);
    let did = alice.did();
    let f = alice.file("p.md", "---\ntitle: Exactly once\n---\nBody.\n");

    alice.account.inject(
        Fault::latency(120_000)
            .on_nsid("com.atproto.repo.createRecord")
            .once(),
    );
    let before = last_seq(&net);
    let mut child = spawn(&alice, &["post", s(&f), "--yes"]);
    wait_for(
        &net,
        "com.atproto.repo.createRecord",
        &did,
        before,
        &mut child,
    );
    kill_9(child);

    // the record is at the PDS; the outbox still says the write was being sent
    assert_eq!(alice.account.list(nsid::POST).len(), 1);
    let w = writes(&alice);
    assert_eq!(w.len(), 2, "{w:?}"); // the forum and the post
    assert_eq!(w[1]["state"]["kind"], "sending", "{w:?}");

    // the next command finishes it: checked, not sent again
    let before = last_seq(&net);
    let flushed = alice.json(&["outbox", "flush"]);
    let w = flushed["writes"].as_array().unwrap();
    assert!(w.iter().all(|x| x["state"]["kind"] == "done"), "{flushed}");
    assert_eq!(
        count(&net, "com.atproto.repo.createRecord", &did, before),
        0
    );
    assert_eq!(alice.account.list(nsid::POST).len(), 1);
    let post = w[1]["state"]["uri"].as_str().unwrap();
    let (rkey, _) = &alice.account.list(nsid::POST)[0];
    assert!(post.ends_with(rkey.as_str()), "{post} vs {rkey}");
    // and the post reads back once
    let list = alice.json(&["ls"]);
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
}

#[test]
fn a_write_killed_before_it_was_sent_is_sent_once() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);
    let did = alice.did();
    let f = alice.file("p.md", "---\ntitle: Edit me\n---\nOld body.\n");
    let t = alice.json(&["post", s(&f), "--yes"]);
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    let rkey = post.rsplit('/').next().unwrap().to_owned();
    alice.ok(&["read", &post]);

    alice.account.inject(
        Fault::latency(120_000)
            .on_nsid("com.atproto.sync.getLatestCommit")
            .once(),
    );
    let before = last_seq(&net);
    let mut child = spawn(&alice, &["edit", &post, "-m", "New body."]);
    wait_for(
        &net,
        "com.atproto.sync.getLatestCommit",
        &did,
        before,
        &mut child,
    );
    kill_9(child);

    // nothing was written; the edit is persisted, picked for sending
    assert_eq!(count(&net, "com.atproto.repo.applyWrites", &did, before), 0);
    assert_eq!(
        alice.account.get(nsid::POST, &rkey).unwrap()["body"]["text"],
        "Old body.\n"
    );
    let w = writes(&alice);
    let last = w.last().unwrap();
    assert_eq!(last["state"]["kind"], "sending", "{w:?}");

    let before = last_seq(&net);
    let out = alice.run(&["--json", "outbox", "flush"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(count(&net, "com.atproto.repo.applyWrites", &did, before), 1);
    assert_eq!(
        alice.account.get(nsid::POST, &rkey).unwrap()["body"]["text"],
        "New body.\n"
    );
    let revisions: Vec<String> = alice
        .account
        .list(nsid::REVISION)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with(&format!("{rkey}-")))
        .collect();
    assert_eq!(revisions.len(), 1, "{revisions:?}");
    let w = writes(&alice);
    assert!(w.iter().all(|x| x["state"]["kind"] == "done"), "{w:?}");
    // a second flush has nothing to do
    let before = last_seq(&net);
    alice.ok(&["outbox", "flush"]);
    assert_eq!(count(&net, "com.atproto.repo.applyWrites", &did, before), 0);
}

/// A comment draft killed while its write is in flight: the write is in the outbox, so the
/// draft is gone and cannot be published a second time (each publish would send a new comment
/// with a new rkey).
#[test]
fn a_draft_whose_write_is_queued_is_not_published_twice() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);
    let did = alice.did();
    let f = alice.file("p.md", "---\ntitle: Comment here\n---\nBody.\n");
    let t = alice.json(&["post", s(&f), "--yes"]);
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    alice.ok(&["read", &post]);
    let drafts = alice.devnet_dir().join("drafts");
    std::fs::create_dir_all(&drafts).unwrap();
    std::fs::write(
        drafts.join("kill-1.md"),
        format!("---\ntype: comment\nroot: \"{post}\"\n---\nOnly once.\n"),
    )
    .unwrap();

    alice.account.inject(
        Fault::latency(120_000)
            .on_nsid("com.atproto.repo.createRecord")
            .once(),
    );
    let before = last_seq(&net);
    let mut child = spawn(&alice, &["draft", "publish", "kill-1", "--yes"]);
    wait_for(
        &net,
        "com.atproto.repo.createRecord",
        &did,
        before,
        &mut child,
    );
    kill_9(child);

    // the draft went away once the write was queued
    let d = alice.json(&["draft", "ls"]);
    assert_eq!(d["drafts"], serde_json::json!([]), "{d}");
    let e = alice.fails(4, &["draft", "publish", "kill-1", "--yes"]);
    assert_eq!(e["code"], "not_found");
    // and the outbox finishes the one comment
    alice.ok(&["outbox", "flush"]);
    assert_eq!(alice.account.list(nsid::COMMENT).len(), 1);
}
