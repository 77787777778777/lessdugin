// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M5 organisation commands through the real CLI on the FakePds network
//! (`support/fakenet.rs`): wiki edits that name the head they were made against (decision O-2).

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, s, text};
use lessdugin_lexicon::collection::nsid;
use serde_json::Value;

/// The `parents` of a wiki revision as the PDS holds it.
fn parents(u: &fakenet::User, revision: &str) -> Vec<String> {
    let rkey = revision.rsplit('/').next().unwrap();
    let rec = u.account.get(nsid::WIKI_REVISION, rkey).unwrap();
    rec["parents"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| p["uri"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn done_uri(t: &Value) -> String {
    assert_eq!(t["state"]["kind"], "done", "{t}");
    t["state"]["uri"].as_str().unwrap().to_owned()
}

/// Decision O-2 (`engine-api.md` §3.5 `wiki_edit`): `wiki edit` names the head its text was
/// edited from, the page as this device last showed it (or `--base`), and syncs before it
/// writes. An edit made against a head another device has moved past is refused (`conflict`,
/// exit 5) instead of silently replacing the newer revision; the author's text is kept (the
/// file as it is, standard input in a file the error names) and the message says the page
/// changed and how to reload it and redo the edit.
#[test]
fn a_wiki_edit_against_a_moved_head_is_refused_and_the_text_kept() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    let first = alice.file("r1.md", "The first text.\n");
    let r1 = done_uri(&alice.json(&[
        "wiki",
        "edit",
        "rust",
        "--name",
        "Rust",
        "--file",
        s(&first),
    ]));
    assert!(parents(&alice, &r1).is_empty());

    // a second device of alice's loads the page at r1
    let other = net.second_device(&alice);
    other.ok(&["forum", "follow", "alice.test/meta"]);
    other.ok(&["sync", "alice.test/meta"]);
    let shown = other.json(&["tag", "rust", "--forum", "alice.test/meta"]);
    assert_eq!(shown["head"], r1.as_str(), "{shown}");

    // meanwhile the first device publishes r2 on r1: its own cached head
    let second = alice.file("r2.md", "The second text.\n");
    let r2 = done_uri(&alice.json(&[
        "wiki",
        "edit",
        "rust",
        "--name",
        "Rust",
        "--file",
        s(&second),
    ]));
    assert_eq!(parents(&alice, &r2), std::slice::from_ref(&r1));

    // the second device's edit was made against r1: refused, nothing written, the file kept
    let mine = other.file("mine.md", "My edit of the first text.\n");
    let e = other.fails(
        5,
        &[
            "wiki",
            "edit",
            "rust",
            "--name",
            "Rust",
            "--file",
            s(&mine),
            "--forum",
            "alice.test/meta",
        ],
    );
    assert_eq!(e["code"], "conflict", "{e}");
    assert_eq!(e["detail"]["kind"], "wikiHeadMoved", "{e}");
    assert_eq!(e["detail"]["base"], r1.as_str(), "{e}");
    assert_eq!(e["detail"]["head"], r2.as_str(), "{e}");
    let message = e["message"].as_str().unwrap();
    assert!(message.contains("changed since it was loaded"), "{message}");
    assert!(message.contains(s(&mine)), "{message}");
    assert!(message.contains("lessdugin tag rust"), "{message}");
    assert!(message.contains(&format!("--base {r2}")), "{message}");
    assert_eq!(alice.account.list(nsid::WIKI_REVISION).len(), 2);
    assert_eq!(
        std::fs::read_to_string(&mine).unwrap(),
        "My edit of the first text.\n"
    );
    // the text output says the same on stderr
    let out = other.run(&[
        "wiki",
        "edit",
        "rust",
        "--name",
        "Rust",
        "--file",
        s(&mine),
        "--base",
        &r1,
    ]);
    assert_eq!(out.status.code(), Some(5), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("changed since it was loaded"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(alice.account.list(nsid::WIKI_REVISION).len(), 2);

    // redone against the head the error named, the edit is written on it
    let r3 = done_uri(&other.json(&[
        "wiki",
        "edit",
        "rust",
        "--name",
        "Rust",
        "--file",
        s(&mine),
        "--base",
        &r2,
    ]));
    assert_eq!(parents(&alice, &r3), std::slice::from_ref(&r2));

    // standard input: alice's first device, still at r3's parent, writes from a pipe; the
    // refused text is saved and the error names the file
    alice.ok(&["tag", "rust"]);
    let out = alice
        .cmd()
        .args([
            "--json", "wiki", "edit", "rust", "--name", "Rust", "--base", &r2,
        ])
        .write_stdin("Piped text.\n")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    let e: Value =
        serde_json::from_str(err.lines().rev().find(|l| l.starts_with('{')).unwrap()).unwrap();
    assert_eq!(e["detail"]["kind"], "wikiHeadMoved", "{e}");
    let kept = e["detail"]["kept"].as_str().expect("the kept file");
    assert!(e["message"].as_str().unwrap().contains(kept), "{e}");
    assert_eq!(std::fs::read_to_string(kept).unwrap(), "Piped text.\n");
    // it is no draft: `draft ls` does not list it
    assert!(!alice.ok(&["draft", "ls"]).contains("Piped"));
    assert_eq!(alice.account.list(nsid::WIKI_REVISION).len(), 3);
}

/// Publishes a post titled `title` in alice's forum; returns its uri.
fn post(u: &fakenet::User, title: &str) -> String {
    let f = u.file(
        &format!("{title}.md"),
        &format!("---\ntitle: {title}\n---\nAbout {title}.\n"),
    );
    done_uri(&u.json(&["post", s(&f), "--yes"]))
}

/// Creates a one-chapter sequence of `posts`; returns its uri.
fn sequence(u: &fakenet::User, title: &str, posts: &[&str]) -> String {
    let f = u.file(
        &format!("{title}.json"),
        &serde_json::json!({
            "forum": "alice.test/meta",
            "title": title,
            "chapters": [{ "title": "One", "posts": posts }],
        })
        .to_string(),
    );
    done_uri(&u.json(&["sequence", "create", s(&f)]))
}

/// The first `SequenceNav` of a `read --no-comments --json` (a `PostView`).
fn nav(v: &Value) -> (String, Option<String>, Option<String>) {
    let n = &v["sequences"][0];
    (
        n["sequence"].as_str().unwrap_or_default().to_owned(),
        n["prev"].as_str().map(str::to_owned),
        n["next"].as_str().map(str::to_owned),
    )
}

/// `social.md` §14, `engine-api.md` §3.4 `get_post(uri, sequence?)` (review m5-acceptance-8):
/// a post opened from a sequence is read in it. `sequence read` numbers its posts and `read
/// <n>` reads the n-th in that sequence; `read next` and `read prev` follow the sequence the
/// last `read` showed; `--sequence` names one. Next and previous are that sequence's, also for
/// a post in two sequences, and the sequence's continue-reading line shows the post last read
/// from it.
#[test]
fn posts_opened_from_a_sequence_are_read_in_it() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    let (p1, p2, p3) = (
        post(&alice, "One"),
        post(&alice, "Two"),
        post(&alice, "Three"),
    );
    let forward = sequence(&alice, "Forward", &[&p1, &p2, &p3]);
    let backward = sequence(&alice, "Backward", &[&p3, &p2, &p1]);
    let reader = net.reader(&alice);
    reader.ok(&["sync", "alice.test/meta"]);

    // the listing numbers the backward sequence's posts; `read 2` is Two, read in it
    let out = reader.ok(&["sequence", "read", &backward]);
    assert!(out.contains("1  Three"), "{out}");
    assert!(out.contains("2  Two"), "{out}");
    assert!(!out.contains("Continue reading"), "{out}");
    let v = reader.json(&["read", "2", "--no-comments"]);
    assert_eq!(v["post"]["uri"], p2.as_str(), "{v}");
    assert_eq!(
        nav(&v),
        (backward.clone(), Some(p3.clone()), Some(p1.clone())),
        "{v}"
    );
    // `read next` follows it to One, still in the backward sequence: its last post
    let v = reader.json(&["read", "next", "--no-comments"]);
    assert_eq!(v["post"]["uri"], p1.as_str(), "{v}");
    assert_eq!(nav(&v), (backward.clone(), Some(p2.clone()), None), "{v}");
    let e = reader.fails(1, &["read", "next"]);
    assert_eq!(e["code"], "invalid_ref", "{e}");
    assert!(e["message"].as_str().unwrap().contains("last"), "{e}");
    // the sequence continues where it was left
    let sv = reader.json(&["sequence", "read", &backward]);
    assert_eq!(sv["continueReading"]["post"], p1.as_str(), "{sv}");
    let out = reader.ok(&["sequence", "read", &backward]);
    assert!(out.contains("Continue reading: One  (`read 3`)"), "{out}");
    // the text shows the post's place in the sequence and how to step along it
    let out = reader.ok(&["read", "prev", "--no-pager"]);
    assert!(out.contains("sequence: Backward"), "{out}");
    assert!(
        out.contains(&format!("previous: {p3}  (`read prev`)")),
        "{out}"
    );
    assert!(
        out.contains(&format!("next:     {p1}  (`read next`)")),
        "{out}"
    );

    // `--sequence` names the other one: Two between One and Three
    let v = reader.json(&["read", &p2, "--sequence", &forward, "--no-comments"]);
    assert_eq!(
        nav(&v),
        (forward.clone(), Some(p1.clone()), Some(p3.clone())),
        "{v}"
    );
    let sv = reader.json(&["sequence", "read", &forward]);
    assert_eq!(sv["continueReading"]["post"], p2.as_str(), "{sv}");
    // the backward sequence kept its own position
    let sv = reader.json(&["sequence", "read", &backward]);
    assert_eq!(sv["continueReading"]["post"], p2.as_str(), "{sv}");
    // a number of a later `ls` reads in no sequence: the listing's context is gone, so no
    // sequence's position moves
    let ls = reader.json(&["ls", "alice.test/meta", "--all"]);
    let first = ls["items"][0]["uri"].as_str().unwrap().to_owned();
    assert_ne!(first, p2, "{ls}");
    let v = reader.json(&["read", "1", "--no-comments"]);
    assert_eq!(v["post"]["uri"], first.as_str(), "{v}");
    for sequence in [&forward, &backward] {
        let sv = reader.json(&["sequence", "read", sequence]);
        assert_eq!(sv["continueReading"]["post"], p2.as_str(), "{sv}");
    }
    // an unparsable sequence is refused
    let e = reader.fails(1, &["read", &p2, "--sequence", "not a sequence"]);
    assert_eq!(e["code"], "invalid_ref", "{e}");
}
