// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the CLI does when the network refuses it part of the way, and how the next command
//! recovers:
//!
//! 1. A session the PDS refuses (an expired access token whose refresh is refused too): the
//!    write is held in the outbox (`sync.md` §7.3, `needsLogin`), the command exits 3
//!    (`auth_required`), and the write is sent once after `lessdugin login`.
//! 2. A forum sync that fails for a repository is a warning (a JSON line under `--json`), and
//!    it does not count as fresh: the next `read` syncs the forum again instead of skipping it
//!    for `FORUM_FRESH_MS`.

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, s, text};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::Fault;
use serde_json::Value;

#[test]
fn a_refused_session_holds_the_write_until_the_next_login() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);
    let f = alice.file("p.md", "---\ntitle: After login\n---\nHeld.\n");

    alice.account.inject(
        Fault::error(401, "ExpiredToken", "Token has expired")
            .on_nsid("com.atproto.repo.createRecord")
            .once(),
    );
    // (refreshSession names no repository, so this fault is not scoped to the account)
    let refused = net.net.inject(
        Fault::error(400, "ExpiredToken", "Token has been revoked")
            .on_nsid("com.atproto.server.refreshSession"),
    );
    let e = alice.fails(3, &["post", s(&f), "--yes"]);
    assert_eq!(e["code"], "auth_required", "{e}");
    assert_eq!(e["detail"]["kind"], "sessionExpired", "{e}");
    let id = e["detail"]["write"].as_str().unwrap().to_owned();
    assert!(alice.account.list(nsid::POST).is_empty());

    // the write waits for a login: shown as such, and not sent by a flush
    let q = alice.json(&["outbox"]);
    let w = q["writes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id.as_str())
        .unwrap_or_else(|| panic!("{q}"));
    assert_eq!(w["state"]["kind"], "pending", "{q}");
    assert_eq!(w["state"]["needsLogin"], true, "{q}");
    assert_eq!(w["state"]["error"]["code"], "auth_required", "{q}");
    let human = alice.ok(&["outbox"]);
    assert!(human.contains("lessdugin login"), "{human}");
    let out = alice.run(&["--json", "outbox", "flush"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert!(alice.account.list(nsid::POST).is_empty());

    // a new login releases it, and the next flush sends it once
    net.net.remove_fault(refused);
    let out = alice
        .cmd()
        .args(["login", "alice.test", "--password-stdin"])
        .write_stdin(format!("{}\n", alice.account.app_password()))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let flushed = alice.json(&["outbox", "flush"]);
    let done = flushed["writes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == id.as_str() && w["state"]["kind"] == "done");
    assert!(done, "{flushed}");
    assert_eq!(alice.account.list(nsid::POST).len(), 1);
    alice.ok(&["outbox", "flush"]);
    assert_eq!(alice.account.list(nsid::POST).len(), 1);
}

#[test]
fn a_failed_forum_sync_is_a_warning_and_is_tried_again() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let anon = net.reader(&alice);
    alice.json(&["forum", "create", "meta"]);
    let t = bob.json(&["join", "alice.test/meta"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    alice.json(&["approve", "bob.test"]);
    let f = alice.file("p.md", "---\ntitle: Sync me\n---\nBody.\n");
    let t = alice.json(&["post", s(&f), "--yes"]);
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    let t = bob.json(&["comment", &post, "-m", "Bob was here."]);
    assert_eq!(t["state"]["kind"], "done", "{t}");

    // bob's repository fails while the reader lists the forum: a warning, the list is shown
    let fault = bob.account.inject(Fault::status(500));
    let out = anon.run(&["--json", "ls", "alice.test/meta"]);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let pl: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(pl["items"].as_array().unwrap().len(), 1, "{pl}");
    let warning = stderr
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| {
            v["warning"]
                .as_str()
                .is_some_and(|w| w.starts_with("the sync failed for 1 repository"))
        });
    assert!(warning.is_some(), "a JSON warning line: {stderr}");
    assert!(
        stderr
            .lines()
            .all(|l| l.trim().is_empty() || l.starts_with('{')),
        "every stderr line is JSON under --json: {stderr}"
    );

    // the failed sync was not fresh: reading the post syncs the forum again and finds bob's
    // comment (a thread sync alone knows only alice)
    net.net.remove_fault(fault);
    let tv = anon.json(&["read", "1"]);
    let comments = tv["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{tv}");
    assert_eq!(comments[0]["author"]["handle"], "bob.test", "{tv}");
}

/// An editor that fails: a comment draft with nothing written in it is removed, one with text
/// is kept and named, so the text is never lost and no empty draft is left behind.
#[test]
fn a_failed_editor_keeps_only_drafts_with_text() {
    use std::os::unix::fs::PermissionsExt;
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);
    let f = alice.file("p.md", "---\ntitle: Edit me\n---\nBody.\n");
    let t = alice.json(&["post", s(&f), "--yes"]);
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    let editor = |name: &str, script: &str| {
        let p = alice.file(name, script);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    };
    let aborted = editor("aborted.sh", "#!/bin/sh\nexit 1\n");
    let wrote = editor(
        "wrote.sh",
        "#!/bin/sh\nsed -i '/ >8 /i Half a comment.' \"$1\"\nexit 1\n",
    );

    let out = alice
        .cmd()
        .env("EDITOR", &aborted)
        .args(["--json", "comment", &post])
        .output()
        .unwrap();
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(!stderr.contains("draft edit"), "nothing is kept: {stderr}");
    let d = alice.json(&["draft", "ls"]);
    assert_eq!(d["drafts"], serde_json::json!([]), "no empty draft: {d}");

    let out = alice
        .cmd()
        .env("EDITOR", &wrote)
        .args(["comment", &post])
        .output()
        .unwrap();
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    let d = alice.json(&["draft", "ls"]);
    let drafts = d["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(), 1, "{d}");
    let id = drafts[0]["id"].as_str().unwrap();
    assert!(
        stderr.contains(&format!("lessdugin draft edit {id}")),
        "the kept draft is named: {stderr}"
    );
    assert!(alice.account.list(nsid::COMMENT).is_empty());
}
