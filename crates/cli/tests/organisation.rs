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
