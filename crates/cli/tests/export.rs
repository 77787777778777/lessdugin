// SPDX-License-Identifier: AGPL-3.0-or-later
//! `export` through the real CLI on the FakePds network (`support/fakenet.rs`): `--mine` (the
//! default, `threat-model.md` PRIV-8) exports the account's own posts and comments from its
//! repository; a forum export is what the forum shows, without hidden and deleted content, also
//! when a moderator exports it; `--format md` writes markdown files into `--out`; `--local` is
//! this device's local state.

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, s, text};
use serde_json::Value;

fn uri(t: &Value) -> String {
    assert_eq!(t["state"]["kind"], "done", "{t}");
    t["state"]["uri"].as_str().unwrap().to_owned()
}

fn uris(items: &Value) -> Vec<String> {
    let mut v: Vec<String> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["uri"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

fn files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn export_mine_and_a_forum_as_json_and_markdown() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    bob.json(&["join", "alice.test/meta", "--yes"]);
    alice.json(&["approve", "bob.test", "--yes"]);
    let f = alice.file("p.md", "---\ntitle: Hello\n---\nFirst post.\n");
    let hello = uri(&alice.json(&["post", s(&f), "--yes"]));
    bob.ok(&["sync", "alice.test/meta"]);
    let f = bob.file("b.md", "---\ntitle: Bob's post\ntags: [intro]\n---\nBob writes.\n");
    let bobs = uri(&bob.json(&["post", s(&f), "--yes"]));
    let visible = uri(&bob.json(&["comment", &hello, "-m", "A visible reply."]));
    let hidden = uri(&bob.json(&["comment", &hello, "-m", "A hidden reply."]));
    let deleted = uri(&bob.json(&["comment", &hello, "-m", "A deleted reply."]));
    assert_eq!(
        bob.json(&["delete", &deleted, "--yes"])["state"]["kind"],
        "done"
    );
    // the owner hides one comment and bob's post
    alice.ok(&["sync", "alice.test/meta"]);
    alice.json(&["mod", "hide", &hidden, "--reason", "off topic", "--yes"]);
    alice.json(&["mod", "hide", &bobs, "--yes"]);
    alice.ok(&["sync", "alice.test/meta"]);

    // a forum export is what the forum shows, even for the owner, whose views carry the
    // hidden items' content (PRIV-8)
    let out = alice.ok(&["export", "alice.test/meta"]);
    assert!(!out.contains("A hidden reply") && !out.contains("Bob writes"), "{out}");
    assert!(!out.contains("A deleted reply"), "{out}");
    let x: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(x["dto"], 1, "{x}");
    assert_eq!(x["scope"], "forum", "{x}");
    assert_eq!(uris(&x["posts"]), [hello.clone()], "{x}");
    assert_eq!(uris(&x["comments"]), [visible.clone()], "{x}");
    assert_eq!(x["posts"][0]["markdown"], "First post.\n", "{x}");
    assert_eq!(x["comments"][0]["markdown"], "A visible reply.", "{x}");
    assert_eq!(x["comments"][0]["handle"], "bob.test", "{x}");
    assert_eq!(x["posts"][0]["record"]["title"], "Hello", "{x}");

    // as markdown: one file per thread, the post then its comments
    let dir = alice.path("forum-md");
    alice.ok(&["export", "alice.test/meta", "--format", "md", "--out", s(&dir)]);
    let names = files(&dir);
    assert_eq!(names.len(), 1, "{names:?}");
    let md = std::fs::read_to_string(dir.join(&names[0])).unwrap();
    assert!(md.starts_with("---\ntitle: \"Hello\"\n"), "{md}");
    assert!(md.contains("First post.") && md.contains("A visible reply."), "{md}");
    assert!(md.contains("### @bob.test"), "{md}");
    assert!(!md.contains("A hidden reply") && !md.contains("A deleted reply"), "{md}");

    // --mine (the default): bob's own posts and comments, from his repository, hidden or not;
    // the deleted comment is no longer in it
    let x: Value = serde_json::from_str(&bob.ok(&["export"])).unwrap();
    assert_eq!(x["scope"], "mine", "{x}");
    assert_eq!(x["account"], bob.did().as_str(), "{x}");
    assert_eq!(uris(&x["posts"]), [bobs.clone()], "{x}");
    let mut want = vec![visible.clone(), hidden.clone()];
    want.sort();
    assert_eq!(uris(&x["comments"]), want, "{x}");
    assert_eq!(x["posts"][0]["markdown"], "Bob writes.\n", "{x}");
    let m: Value = serde_json::from_str(&bob.ok(&["export", "--mine"])).unwrap();
    assert_eq!((&m["posts"], &m["comments"]), (&x["posts"], &x["comments"]));
    let dir = bob.path("mine-md");
    let j = bob.json(&["export", "--format", "md", "--out", s(&dir)]);
    assert_eq!(j["posts"], 1, "{j}");
    assert_eq!(j["comments"], 2, "{j}");
    assert_eq!(files(&dir), ["comments", "posts"]);
    let posts = files(&dir.join("posts"));
    assert_eq!(posts.len(), 1, "{posts:?}");
    let md = std::fs::read_to_string(dir.join("posts").join(&posts[0])).unwrap();
    assert!(md.contains("title: \"Bob's post\"\n"), "{md}");
    assert!(md.contains("tags: [\"intro\"]\n"), "{md}");
    assert!(md.contains(&format!("uri: \"{bobs}\"\n")), "{md}");
    assert!(md.ends_with("---\nBob writes.\n"), "{md}");
    assert_eq!(files(&dir.join("comments")).len(), 2);
    // and as JSON into a directory
    let dir = bob.path("mine-json");
    bob.ok(&["export", "--out", s(&dir)]);
    let x: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("export.json")).unwrap()).unwrap();
    assert_eq!(uris(&x["posts"]), [bobs]);

    // markdown is files: it needs a directory
    let out = bob.run(&["export", "--format", "md"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    // --local is this device's local state, as before
    let l = bob.json(&["export", "--local"]);
    assert!(l["members"].is_array(), "{l}");
    assert_eq!(l["scope"], "mine", "{l}");
}
