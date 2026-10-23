// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forum and content commands (synthesis §6.10, `docs/plan.md` M2) against a FakePds
//! network served over HTTP: every command, its `--json` output (`dto/1`, byte-exact against
//! the DTO types), human output (insta snapshots with run-specific values redacted), refs
//! (`3`, `c2`, `<handle>/<rkey>`, at-uris, links, forum aliases and numbers), drafts and the
//! editor, `--offline`, error paths and exit codes.

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, User, s, text};
use lessdugin_core::dto::{
    ForumView, HistoryView, PostList, PostView, RawView, SyncReport, ThreadView,
};
use lessdugin_lexicon::collection::nsid;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Redacts the values that change from run to run (DIDs are named by their user).
fn redact(users: &[&User], text: &str) -> String {
    let mut t = text.to_owned();
    for u in users {
        let name = u.handle.split('.').next().unwrap_or("user");
        t = t.replace(&u.did(), &format!("<{name}>"));
    }
    t
}

fn settings() -> insta::Settings {
    let mut st = insta::Settings::clone_current();
    st.add_filter(r"bafy[a-z2-7]{20,}", "<cid>");
    st.add_filter(r"-[2-7a-z]{13}\b", "-<tid>");
    st.add_filter(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC", "<time>");
    st.add_filter(r#""(\w+(?:Ms|At))": "?\d[\d\-T:.Z]*"?"#, r#""$1": <time>"#);
    st.add_filter(r"(<\w+>)/\d{6,}", "$1/<seq>");
    st.add_filter(r"127\.0\.0\.1:\d+", "127.0.0.1:<port>");
    st.add_filter(r"/tmp/\.tmp\w+", "<tmp>");
    st.add_filter(r"\b[a-km-z2-9]{6}\b(\.md)", "<draft>$1");
    st
}

/// Checks that `json` is exactly how the DTO type serialises it (pretty), and returns it.
fn exact<T: Serialize + DeserializeOwned>(json: &str) -> T {
    let v: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}: {json}"));
    let again = serde_json::to_string_pretty(&v).unwrap();
    assert_eq!(again.trim_end(), json.trim_end(), "not byte-exact dto/1");
    v
}

fn json_out(u: &User, args: &[&str]) -> String {
    let mut a = vec!["--json"];
    a.extend_from_slice(args);
    u.ok(&a)
}

fn forum_of(u: &User, slug: &str) -> String {
    format!("at://{}/{}/{slug}", u.did(), nsid::FORUM)
}

#[test]
fn a_forum_from_creation_to_deletion() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let users = [&alice];

    // forum create writes the membership mode (approval by default) and becomes current
    let t = alice.json(&[
        "forum",
        "create",
        "meta",
        "--name",
        "Meta",
        "--description",
        "Talk about the forum",
    ]);
    assert_eq!(t["dto"], 1);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let forum = forum_of(&alice, "meta");
    assert_eq!(t["state"]["uri"], forum);
    let decl = alice.account.get(nsid::FORUM, "meta").unwrap();
    assert_eq!(decl["membership"], "approval");
    assert_eq!(decl["name"], "Meta");
    let open = alice.json(&["forum", "create", "lounge", "--mode", "open"]);
    assert_eq!(open["state"]["kind"], "done");
    assert_eq!(
        alice.account.get(nsid::FORUM, "lounge").unwrap()["membership"],
        "open"
    );
    alice.ok(&["forum", "use", "meta"]);

    // forum ls / show
    let fl = alice.json(&["forum", "ls"]);
    assert_eq!(fl["dto"], 1);
    assert_eq!(fl["current"], forum);
    assert_eq!(fl["forums"].as_array().unwrap().len(), 2);
    let show = json_out(&alice, &["forum", "show"]);
    let fv: ForumView = exact(&show);
    assert_eq!(fv.forum.uri, forum);
    assert_eq!(fv.forum.owner.handle.as_deref(), Some("alice.test"));
    settings().bind(|| {
        insta::assert_snapshot!("forum_ls", redact(&users, &alice.ok(&["forum", "ls"])));
        insta::assert_snapshot!(
            "forum_show",
            redact(&users, &alice.ok(&["forum", "show", "alice.test/meta"]))
        );
    });

    // post from a file with frontmatter; `ls` numbers it
    let file = alice.file(
        "hello.md",
        "---\ntitle: \"Hello, world\"\ntags: [intro]\n---\nFirst *post*, see [the docs](https://example.com).\n",
    );
    let t = alice.json(&["post", s(&file), "--yes"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    let second = alice.file("second.md", "Just a quick take.\n");
    let t = alice.json(&["post", s(&second), "--kind", "quick-take", "--yes"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let list = json_out(&alice, &["ls", "--all", "--sort", "new"]);
    let pl: PostList = exact(&list);
    assert_eq!(pl.items.len(), 2);
    assert_eq!(pl.items[1].uri, post, "newest first");
    assert_eq!(pl.items[1].title.as_deref(), Some("Hello, world"));
    assert_eq!(pl.items[1].tags, ["intro"]);
    settings().bind(|| {
        insta::assert_snapshot!(
            "ls",
            redact(&users, &alice.ok(&["ls", "--all", "--sort", "old"]))
        );
    });

    // comments: `1` is the first post of the last `ls --sort old`, `c1` the first comment
    let t = alice.json(&["comment", "1", "-m", "A first comment."]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let reply = alice.file("reply.md", "A reply to the comment.\n");
    alice.ok(&["read", "1"]);
    let t = alice.json(&["comment", "c1", s(&reply)]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let c2 = t["state"]["uri"].as_str().unwrap().to_owned();
    let thread = json_out(&alice, &["read", &post]);
    let tv: ThreadView = exact(&thread);
    assert_eq!(tv.comments.len(), 2);
    assert_eq!(tv.comments[1].uri, c2);
    assert_eq!(
        tv.comments[1].parent.as_deref(),
        Some(tv.comments[0].uri.as_str())
    );
    assert_eq!(tv.comments[1].depth, tv.comments[0].depth + 1);
    settings().bind(|| {
        insta::assert_snapshot!("read", redact(&users, &alice.ok(&["read", "1"])));
    });

    // the same post by handle/rkey, by SPA link, and as a post only
    let rkey = post.rsplit('/').next().unwrap();
    let by_handle = json_out(&alice, &["read", &format!("alice.test/{rkey}")]);
    assert_eq!(by_handle, thread);
    let link = format!("https://lessdugin.example/#/p/alice.test/{rkey}");
    assert_eq!(json_out(&alice, &["read", &link]), thread);
    let pv: PostView = exact(&json_out(&alice, &["read", &post, "--no-comments"]));
    assert_eq!(pv.post.uri, post);
    let raw: RawView = exact(&json_out(&alice, &["read", "c2", "--raw"]));
    assert_eq!(raw.uri, c2);
    assert_eq!(raw.root.as_deref(), Some(post.as_str()));

    // edit with a summary: the old version is kept as a revision
    let t = alice.json(&[
        "edit",
        &post,
        "-m",
        "First post, edited.",
        "--summary",
        "typo",
    ]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let h: HistoryView = exact(&json_out(&alice, &["history", &post]));
    assert_eq!(h.versions.len(), 2, "{h:?}");
    assert!(
        h.versions
            .iter()
            .any(|v| v.summary.as_deref() == Some("typo"))
    );
    let pv: PostView = exact(&json_out(&alice, &["read", &post, "--no-comments"]));
    assert!(pv.post.badges.edited);
    assert_eq!(pv.post.title.as_deref(), Some("Hello, world"), "title kept");
    assert_eq!(pv.body.text.trim(), "First post, edited.");
    let t = alice.json(&["edit", &post, "--title", "Hello again", "--no-history"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let pv: PostView = exact(&json_out(&alice, &["read", &post, "--no-comments"]));
    assert_eq!(pv.post.title.as_deref(), Some("Hello again"));
    assert_eq!(pv.body.text.trim(), "First post, edited.", "body kept");
    settings().bind(|| {
        insta::assert_snapshot!("history", redact(&users, &alice.ok(&["history", &post])));
    });

    // retract the reply, delete the first comment
    alice.ok(&["read", &post]);
    let t = alice.json(&["retract", "c2"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let c1 = tv.comments[0].uri.clone();
    let t = alice.json(&["delete", &c1, "--yes"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let tv: ThreadView = exact(&json_out(&alice, &["read", &post]));
    let reply = tv.comments.iter().find(|c| c.uri == c2).unwrap();
    assert!(reply.retracted);
    let c1rkey = c1.rsplit('/').next().unwrap();
    assert!(alice.account.get(nsid::COMMENT, c1rkey).is_none());

    // delete the post, with its revision
    let t = alice.json(&["delete", &post, "--yes"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    assert!(alice.account.get(nsid::POST, rkey).is_none());
    // …and its revision (the retracted comment keeps its own)
    let post_revisions: Vec<String> = alice
        .account
        .list(nsid::REVISION)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| {
            k.strip_prefix(&format!("{rkey}-"))
                .is_some_and(|rest| !rest.contains('-'))
        })
        .collect();
    assert!(post_revisions.is_empty(), "{post_revisions:?}");
    // its comments stay (they are the commenters' records)
    let left: PostList = exact(&json_out(&alice, &["ls", "--all"]));
    assert!(left.items.iter().all(|p| p.uri != post));

    // sync and leave
    let r: SyncReport = exact(&json_out(&alice, &["sync"]));
    assert_eq!(r.forum, forum);
    assert!(!r.partial);
    let t = alice.json(&["leave"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let mrkey = forum_key_rkey(&alice, "meta");
    assert!(alice.account.get(nsid::MEMBERSHIP, &mrkey).is_none());
}

/// The rkey of `u`'s membership in its forum `slug` (`fk(F)`).
fn forum_key_rkey(u: &User, slug: &str) -> String {
    lessdugin_lexicon::keys::fk(&forum_of(u, slug)).unwrap()
}

#[test]
fn approval_mode_with_three_users_and_anonymous_reading() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let carol = net.user("carol.test", true);
    let anon = net.reader(&alice);
    let users = [&alice, &bob, &carol];

    alice.json(&["forum", "create", "meta", "--name", "Meta"]);
    // bob joins with a note (and an alias); the forum becomes his current one
    let t = bob.json(&[
        "join",
        "alice.test/meta",
        "--note",
        "I would like to post",
        "--alias",
        "m",
    ]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let fl = bob.json(&["forum", "ls"]);
    assert_eq!(fl["forums"][0]["alias"], "m");
    // the owner approves bob and carol (applicants are found through relays and hints, which
    // a devnet does not have, so the owner names them)
    let pending = alice.json(&["members", "--pending"]);
    assert_eq!(pending["dto"], 1);
    let a = alice.json(&["approve", "bob.test", &carol.did()]);
    let w = a["writes"].as_array().unwrap();
    assert_eq!(w.len(), 2);
    assert!(w.iter().all(|t| t["state"]["kind"] == "done"), "{a}");
    // bob is now a member, with his note
    let members = alice.json(&["members"]);
    let m = members["members"].as_array().unwrap();
    let b = m
        .iter()
        .find(|x| x["actor"]["did"] == bob.did().as_str())
        .unwrap_or_else(|| panic!("{members}"));
    assert_eq!(b["granted"], true);
    assert_eq!(b["membershipNote"], "I would like to post");
    settings().bind(|| {
        insta::assert_snapshot!("members", redact(&users, &alice.ok(&["members"])));
    });

    // bob posts (the forum by alias), carol replies after reading it
    let f = bob.file("p.md", "Bob's question body.\n");
    let t = bob.json(&[
        "post",
        s(&f),
        "--forum",
        "m",
        "--title",
        "Bob asks",
        "--kind",
        "question",
        "--yes",
    ]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    // carol: first content in the forum is batched with her membership
    let t = carol.json(&["comment", &post, "-m", "Carol answers."]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let mrkey = forum_key_rkey(&alice, "meta");
    assert!(carol.account.get(nsid::MEMBERSHIP, &mrkey).is_some());

    // anonymous reading: ls with a forum reference, then `read 1`
    let pl: PostList = exact(&json_out(&anon, &["ls", "alice.test/meta", "--all"]));
    assert_eq!(pl.items.len(), 1, "{pl:?}");
    assert_eq!(pl.items[0].uri, post);
    assert_eq!(pl.items[0].author.handle.as_deref(), Some("bob.test"));
    let tv: ThreadView = exact(&json_out(&anon, &["read", "1"]));
    assert_eq!(tv.comments.len(), 1);
    assert_eq!(tv.comments[0].author.handle.as_deref(), Some("carol.test"));
    settings().bind(|| {
        insta::assert_snapshot!("read_anonymous", redact(&users, &anon.ok(&["read", "1"])));
    });
    // three clients agree on the thread (the viewer-independent parts)
    let from_bob: ThreadView = exact(&json_out(&bob, &["read", &post]));
    assert_eq!(from_bob.comments, tv.comments);
    // an anonymous reader cannot write
    let e = anon.fails(3, &["comment", "1", "-m", "hi"]);
    assert_eq!(e["code"], "auth_required");
    // nor can anyone edit someone else's comment
    let e = bob.fails(3, &["edit", &tv.comments[0].uri, "-m", "x"]);
    assert_eq!(e["code"], "auth_required");
}

#[test]
fn references_and_error_paths() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let fresh = net.reader(&alice);

    // nothing listed in this terminal yet
    let e = alice.fails(1, &["read", "1"]);
    assert_eq!(e["code"], "invalid_ref");
    assert!(
        e["message"].as_str().unwrap().contains("lessdugin ls"),
        "{e}"
    );
    let e = alice.fails(1, &["read", "c3"]);
    assert_eq!(e["code"], "invalid_ref");
    // no forum anywhere: a usage error
    let e = alice.fails(2, &["ls"]);
    assert_eq!(e["code"], "usage");
    let e = alice.fails(1, &["read", "not a reference"]);
    assert_eq!(e["code"], "invalid_ref");
    // a bad slug
    let e = alice.fails(1, &["forum", "create", "Bad Slug"]);
    assert_eq!(e["code"], "invalid_input", "{e}");

    alice.json(&["forum", "create", "meta"]);
    let f = alice.file("p.md", "---\ntitle: One\n---\nBody\n");
    let t = alice.json(&["post", s(&f), "--yes"]);
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    alice.ok(&["ls", "--all"]);
    let e = alice.fails(1, &["read", "7"]);
    assert!(
        e["message"].as_str().unwrap().contains("has 1 entries"),
        "{e}"
    );
    // listings are per terminal session
    let mut a2 = alice.cmd();
    a2.env("LESSDUGIN_SESSION", "another-terminal");
    let out = a2.args(["--json", "read", "1"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));

    // a post that does not exist
    let missing = post.replace(post.rsplit('-').next().unwrap(), "2222222222222");
    let e = fresh.fails(4, &["read", &missing]);
    assert_eq!(e["code"], "not_found", "{e}");
    // a forum reference given as a post
    let e = alice.fails(1, &["read", &forum_of(&alice, "meta")]);
    assert_eq!(e["code"], "invalid_ref");
    // not logged in
    let e = fresh.fails(3, &["post", s(&f), "--forum", "alice.test/meta", "--yes"]);
    assert_eq!(e["code"], "auth_required");
    let e = fresh.fails(3, &["join", "alice.test/meta"]);
    assert_eq!(e["code"], "auth_required");
    // destructive commands need --yes without a terminal
    let e = alice.fails(2, &["delete", &post]);
    assert_eq!(e["code"], "usage");
    // empty comments are refused
    let e = alice.fails(2, &["comment", &post, "-m", "  "]);
    assert_eq!(e["code"], "usage");
    // a comment is not retracted… a post is
    let e = alice.fails(1, &["retract", &post]);
    assert_eq!(e["code"], "invalid_input");
    // invalid posts never reach the network
    let bad = alice.file("bad.md", "---\nkind: link\n---\n");
    let e = alice.fails(1, &["post", s(&bad), "--yes"]);
    assert_eq!(e["code"], "invalid_input");
    let msg = e["message"].as_str().unwrap();
    assert!(msg.contains("title") && msg.contains("link"), "{msg}");
    // …and are kept as drafts with the errors as comments
    let id = e["detail"]["draft"].as_str().unwrap();
    let draft =
        std::fs::read_to_string(alice.devnet_dir().join(format!("drafts/{id}.md"))).unwrap();
    assert!(draft.starts_with("---\n# error: "), "{draft}");
    // sync needs the network
    let e = alice.fails(2, &["--offline", "sync"]);
    assert_eq!(e["code"], "usage");
    // a write conflict exits 5: an edit queued offline from a stale copy loses
    let other_dev = net.second_device(&alice);
    other_dev.ok(&["read", &post]);
    let queued = other_dev.json(&["--offline", "edit", &post, "-m", "stale edit"]);
    assert_eq!(queued["state"]["kind"], "pending", "{queued}");
    let t = alice.json(&["edit", &post, "-m", "fresh edit"]);
    assert_eq!(t["state"]["kind"], "done");
    let e = other_dev.fails(5, &["outbox", "flush"]);
    assert_eq!(e["code"], "conflict", "{e}");
}

#[test]
fn drafts_and_the_editor() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);

    // an editor that fills in the title and the body
    let editor = alice.file(
        "editor.sh",
        "#!/bin/sh\nsed -i 's/^title: .*/title: \"From the editor\"/' \"$1\"\nsed -i '/ >8 /i Written in the editor.' \"$1\"\n",
    );
    make_executable(&editor);
    let mut c = alice.cmd();
    c.env("VISUAL", &editor)
        .args(["--json", "post", "--yes"])
        .write_stdin("");
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let t: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let pl: PostList = exact(&json_out(&alice, &["ls", "--all", "--sort", "new"]));
    assert_eq!(pl.items[0].title.as_deref(), Some("From the editor"));
    // the published draft is gone
    let d = alice.json(&["draft", "ls"]);
    assert_eq!(d["drafts"], serde_json::json!([]));

    // --draft keeps it; without a terminal, publishing needs --yes and keeps the draft
    let f = alice.file("d.md", "---\ntitle: Later\n---\nNot yet.\n");
    let saved = alice.json(&["post", s(&f), "--draft"]);
    let id = saved["id"].as_str().unwrap().to_owned();
    let d = alice.json(&["draft", "ls"]);
    assert_eq!(d["drafts"][0]["id"], id);
    assert_eq!(d["drafts"][0]["title"], "Later");
    assert_eq!(d["drafts"][0]["type"], "post");
    settings().bind(|| {
        insta::assert_snapshot!(
            "draft_file",
            std::fs::read_to_string(alice.devnet_dir().join(format!("drafts/{id}.md"))).unwrap()
        );
    });
    let e = alice.fails(2, &["draft", "publish", &id]);
    assert_eq!(e["code"], "usage");
    // draft edit runs the editor on the file
    let mut c = alice.cmd();
    c.env("EDITOR", &editor)
        .args(["draft", "edit", &id])
        .write_stdin("");
    assert!(c.output().unwrap().status.success());
    let t = alice.json(&["draft", "publish", &id, "--yes"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let pl: PostList = exact(&json_out(&alice, &["ls", "--all", "--sort", "new"]));
    assert_eq!(pl.items[0].title.as_deref(), Some("From the editor"));
    assert!(pl.items.len() == 2);
    let e = alice.fails(4, &["draft", "rm", &id]);
    assert_eq!(e["code"], "not_found");
    let e = alice.fails(1, &["draft", "rm", "../x"]);
    assert_eq!(e["code"], "invalid_input");

    // a comment written in the editor
    alice.ok(&["ls", "--all"]);
    let comment_editor = alice.file(
        "c.sh",
        "#!/bin/sh\nsed -i '/ >8 /i From the comment editor.' \"$1\"\n",
    );
    make_executable(&comment_editor);
    let mut c = alice.cmd();
    c.env("VISUAL", &comment_editor)
        .args(["--json", "comment", "1"])
        .write_stdin("");
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let tv: ThreadView = exact(&json_out(&alice, &["read", "1"]));
    assert_eq!(tv.comments[0].body.text.trim(), "From the comment editor.");
    // an editor that writes nothing: nothing is posted
    let noop = alice.file("noop.sh", "#!/bin/sh\nexit 0\n");
    make_executable(&noop);
    let mut c = alice.cmd();
    c.env("VISUAL", &noop)
        .args(["--json", "comment", "1"])
        .write_stdin("");
    assert_eq!(c.output().unwrap().status.code(), Some(2));
    // an edit in the editor (the current text is in the draft)
    let edit_editor = alice.file(
        "e.sh",
        "#!/bin/sh\nsed -i 's/From the comment editor/Edited in the editor/' \"$1\"\n",
    );
    make_executable(&edit_editor);
    let mut c = alice.cmd();
    c.env("VISUAL", &edit_editor)
        .args(["--json", "edit", "c1"])
        .write_stdin("");
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let tv: ThreadView = exact(&json_out(&alice, &["read", "1"]));
    assert_eq!(tv.comments[0].body.text.trim(), "Edited in the editor.");
    assert_eq!(
        alice.json(&["draft", "ls"])["drafts"],
        serde_json::json!([])
    );
}

fn make_executable(p: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn offline_writes_wait_in_the_outbox() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    alice.json(&["forum", "create", "meta"]);
    let f = alice.file("p.md", "---\ntitle: Offline\n---\nWritten on a train.\n");
    alice.ok(&["ls"]);

    // reading offline uses the cache
    let pl: PostList = exact(&json_out(&alice, &["--offline", "ls"]));
    assert!(pl.items.is_empty());
    // a post written offline is queued, not sent
    let t = alice.json(&["--offline", "post", s(&f), "--yes"]);
    assert_eq!(t["state"]["kind"], "pending", "{t}");
    assert!(alice.account.list(nsid::POST).is_empty());
    let q = alice.json(&["outbox"]);
    let w = q["writes"].as_array().unwrap();
    assert!(
        w.iter()
            .any(|x| x["id"] == t["id"] && x["state"]["kind"] == "pending"),
        "{q}"
    );
    let human = alice.ok(&["--offline", "post", s(&f), "--yes"]);
    assert!(human.starts_with("Queued (offline): write "), "{human}");
    // the next writing command sends both, in order
    let flushed = alice.json(&["outbox", "flush"]);
    let w = flushed["writes"].as_array().unwrap();
    assert!(w.iter().all(|x| x["state"]["kind"] == "done"), "{flushed}");
    assert_eq!(alice.account.list(nsid::POST).len(), 2);
    // and nothing is sent twice
    let again = alice.ok(&["outbox", "flush"]);
    assert!(again.starts_with("Sent 0 writes"), "{again}");
    assert_eq!(alice.account.list(nsid::POST).len(), 2);
}

#[test]
fn every_command_documents_itself() {
    let net = Net::new();
    let u = net.user("alice.test", false);
    let help = u.ok(&["--help"]);
    for c in [
        "read",
        "ls",
        "post",
        "comment",
        "edit",
        "delete",
        "retract",
        "history",
        "sync",
        "join",
        "leave",
        "forum",
        "members",
        "approve",
        "draft",
        "outbox",
        "--forum",
        "--offline",
    ] {
        assert!(help.contains(c), "{c} missing from\n{help}");
    }
    let forum = u.ok(&["forum", "--help"]);
    for c in ["ls", "show", "follow", "unfollow", "use", "create"] {
        assert!(forum.contains(c), "{c}: {forum}");
    }
    let create = u.ok(&["forum", "create", "--help"]);
    assert!(
        create.contains("--mode") && create.contains("approval"),
        "{create}"
    );
    let e = u.run(&["ls", "--sort", "sideways"]);
    assert_eq!(e.status.code(), Some(2));
}

/// Asserts that terminal output carries no control sequence from record text: only the
/// printer's own SGR (and OSC 8) forms, no BEL, no C1 control, no bidi override.
fn clean_terminal_output(what: &str, out: &str, color: bool) {
    if let Err(e) = lessdugin_markdown::printer::check_printer_output(out) {
        panic!("{what}: {e}:\n{out:?}");
    }
    if !color {
        assert!(
            !out.contains('\u{1b}'),
            "{what}: an escape without color:\n{out:?}"
        );
    }
    for bad in ['\u{7}', '\u{9b}', '\u{202e}', '\u{2066}'] {
        assert!(
            !out.contains(bad),
            "{what}: {bad:?} reached the terminal:\n{out:?}"
        );
    }
}

#[test]
fn hostile_text_never_reaches_the_terminal() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let anon = net.reader(&alice);
    let evil_name = "Evil \u{1b}]0;pwned\u{7} \u{202e}name\u{2066}";
    alice.json(&[
        "forum", "create", "meta", "--name", evil_name, "--about", evil_name,
    ]);
    let f = alice.file(
        "p.md",
        "---\ntitle: \"Evil \\u001b]0;pwned\\u0007 \\u202etitle\"\n---\n\
         Body \u{1b}[31mred\u{1b}[0m \u{9b}31m and \u{202e}rtl \
         [x](https://evil.example/\u{1b}) [ok](https://example.com/a).\n",
    );
    let t = alice.json(&["post", s(&f), "--yes"]);
    let post = t["state"]["uri"].as_str().unwrap().to_owned();
    alice.json(&[
        "comment",
        &post,
        "-m",
        "\u{1b}[2J\u{1b}]8;;https://x\u{1b}\\cleared\u{7}",
    ]);
    alice.json(&[
        "edit",
        &post,
        "-m",
        "Edited \u{1b}[5m\u{9b}?25l",
        "--summary",
        evil_name,
    ]);
    // the text is stored as written…
    let (rkey, _) = &alice.account.list(nsid::POST)[0];
    let rec = alice.account.get(nsid::POST, rkey).unwrap();
    assert!(rec["title"].as_str().unwrap().contains('\u{1b}'), "{rec}");

    // …and every human view shows it inert, with and without color
    for u in [&alice, &anon] {
        u.ok(&["ls", "alice.test/meta", "--all"]);
        for (color, flag) in [(true, "always"), (false, "never")] {
            for args in [
                vec!["ls", "alice.test/meta", "--all"],
                vec!["read", "1", "--no-pager"],
                vec!["read", "1", "--no-comments", "--no-pager"],
                vec!["read", "c1", "--no-pager"],
                vec!["history", "1"],
                vec!["forum", "show", "alice.test/meta"],
            ] {
                let mut a = vec!["--color", flag];
                a.extend_from_slice(&args);
                let out = u.run(&a);
                assert!(out.status.success(), "{a:?}: {}", text(&out.stderr));
                let stdout = text(&out.stdout);
                assert!(
                    stdout.contains("pwned") || stdout.contains("Edited"),
                    "{a:?}: {stdout}"
                );
                clean_terminal_output(&format!("{a:?}"), &stdout, color);
                clean_terminal_output(&format!("{a:?} (stderr)"), &text(&out.stderr), color);
            }
        }
    }
    // `forum ls` (followed forums) and errors that echo their input
    clean_terminal_output("forum ls", &alice.ok(&["forum", "ls"]), false);
    let out = alice.run(&["read", "\u{1b}]0;pwned\u{7}\u{202e}"]);
    assert_eq!(out.status.code(), Some(1));
    clean_terminal_output("error", &text(&out.stderr), false);
    let out = alice.run(&["--color", "always", "forum", "use", "\u{1b}[31mnope"]);
    assert_ne!(out.status.code(), Some(0));
    clean_terminal_output("error", &text(&out.stderr), true);
    // --raw is the record's JSON: escaped, so still inert
    let raw = alice.ok(&["read", &post, "--raw"]);
    clean_terminal_output("raw", &raw, false);
}

/// Runs `lessdugin --json <args>` with `stdin`; must succeed.
fn json_stdin(u: &User, args: &[&str], stdin: &str) -> Value {
    let out = u
        .cmd()
        .arg("--json")
        .args(args)
        .write_stdin(stdin)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn tags_links_stdin_follows_and_accounts() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", false);
    let carol = net.user("carol.test", true);
    let t = alice.json(&[
        "forum", "create", "lounge", "--mode", "open", "--name", "Lounge",
    ]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    alice.json(&["forum", "create", "meta"]);
    alice.ok(&["forum", "use", "lounge"]);

    // tags from flags, a link post and bodies from standard input
    let f = alice.file("t.md", "---\ntitle: Tagged\n---\nWith tags.\n");
    let t = alice.json(&["post", s(&f), "--tag", "rust", "--tag", "cli", "--yes"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let t = json_stdin(
        &alice,
        &[
            "post",
            "-",
            "--kind",
            "link",
            "--link",
            "https://example.com/x",
            "--title",
            "A link",
            "--yes",
        ],
        "Worth reading.\n",
    );
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let link_post = t["state"]["uri"].as_str().unwrap().to_owned();
    let pl: PostList = exact(&json_out(&alice, &["ls", "--all", "--sort", "old"]));
    assert_eq!(pl.items.len(), 2);
    assert_eq!(pl.items[0].tags, ["rust", "cli"]);
    assert_eq!(pl.items[1].uri, link_post);
    let link = pl.items[1].link.as_ref().unwrap();
    assert_eq!(
        (link.uri.as_str(), link.host.as_str()),
        ("https://example.com/x", "example.com")
    );
    let t = json_stdin(&alice, &["comment", "2", "-"], "From stdin.\n");
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let tv: ThreadView = exact(&json_out(&alice, &["read", "2"]));
    assert_eq!(tv.comments[0].body.text.trim(), "From stdin.");
    // a link post needs its link
    let e = alice.fails(1, &["post", s(&f), "--kind", "link", "--yes"]);
    assert_eq!(e["code"], "invalid_input", "{e}");

    // follow a forum by reference with an alias; `ls <n>` takes a forum of the last `forum ls`
    let fv = carol.json(&["forum", "follow", "alice.test/lounge", "--alias", "l"]);
    assert_eq!(
        fv,
        serde_json::json!({
            "dto": 1, "uri": forum_of(&alice, "lounge"), "followed": true, "alias": "l"
        })
    );
    carol.json(&["forum", "follow", &forum_of(&alice, "meta")]);
    let e = carol.fails(4, &["forum", "follow", "alice.test/nowhere"]);
    assert_eq!(e["code"], "not_found", "{e}");
    let fl = carol.json(&["forum", "ls"]);
    let forums = fl["forums"].as_array().unwrap();
    assert_eq!(forums.len(), 2, "{fl}");
    // the declaration was read when following
    assert_eq!(forums[0]["name"], "Lounge", "{fl}");
    assert_eq!(forums[0]["membership"], "open", "{fl}");
    let n = forums
        .iter()
        .position(|f| f["uri"] == forum_of(&alice, "lounge").as_str())
        .unwrap_or_else(|| panic!("{fl}"))
        + 1;
    let pl: PostList = exact(&json_out(&carol, &["ls", &n.to_string(), "--all"]));
    assert_eq!(pl.forum, forum_of(&alice, "lounge"));
    assert_eq!(pl.items.len(), 2);
    // The tag filter matches only tags whose wiki page exists (social.md §13.3:
    // an unknown tag never applies). The CLI cannot create a wiki page until M5,
    // so `--tag rust` returns nothing here even though the post carries the tag
    // (asserted above); M5 (organisation/discovery) adds the wiki and revisits this.
    let pl: PostList = exact(&json_out(&carol, &["ls", "l", "--all", "--tag", "rust"]));
    assert_eq!(pl.items.len(), 0, "{pl:?}");
    // two followed forums and no current one: `ls` asks which
    let e = carol.fails(2, &["ls"]);
    assert!(
        e["message"].as_str().unwrap().contains("which forum"),
        "{e}"
    );
    let uf = carol.json(&["forum", "unfollow", "l"]);
    assert_eq!(uf["followed"], false);
    let fl = carol.json(&["forum", "ls"]);
    assert_eq!(fl["forums"].as_array().unwrap().len(), 1, "{fl}");
    // with one followed forum, it is the default
    let pl: PostList = exact(&json_out(&carol, &["ls"]));
    assert_eq!(pl.forum, forum_of(&alice, "meta"));

    // two accounts on one device: --account picks the author
    let out = alice
        .cmd()
        .args(["login", "bob.test", "--password-stdin"])
        .write_stdin(format!("{}\n", bob.account.app_password()))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let note = alice.file(
        "n.md",
        "---\ntitle: By bob\n---\nBob, from alice's laptop.\n",
    );
    let t = alice.json(&[
        "--account",
        "bob.test",
        "post",
        s(&note),
        "--forum",
        "lounge",
        "--yes",
    ]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    assert!(t["id"].as_str().unwrap().starts_with(&bob.did()), "{t}");
    assert_eq!(bob.account.list(nsid::POST).len(), 1);
    let t = alice.json(&[
        "--account",
        "alice.test",
        "comment",
        &link_post,
        "-m",
        "As alice.",
    ]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    assert!(t["id"].as_str().unwrap().starts_with(&alice.did()), "{t}");
    let tv: ThreadView = exact(&json_out(&alice, &["read", &link_post]));
    let authors: Vec<_> = tv
        .comments
        .iter()
        .map(|c| c.author.handle.clone())
        .collect();
    assert!(
        authors.iter().all(|h| h.as_deref() == Some("alice.test")),
        "{authors:?}"
    );
    // an account not logged in here
    let e = alice.fails(
        3,
        &["--account", "carol.test", "comment", &link_post, "-m", "x"],
    );
    assert_eq!(e["code"], "auth_required", "{e}");
    // edits go to the record's owner, whichever account is active
    let bobs = bob.account.list(nsid::POST)[0].0.clone();
    let bob_post = format!("at://{}/{}/{bobs}", bob.did(), nsid::POST);
    let t = alice.json(&[
        "--account",
        "alice.test",
        "edit",
        &bob_post,
        "-m",
        "Edited.",
    ]);
    assert!(t["id"].as_str().unwrap().starts_with(&bob.did()), "{t}");
}

#[test]
fn outbox_undo_retry_and_partial_sync() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let users = [&alice, &bob];
    alice.json(&["forum", "create", "lounge"]);
    let f = alice.file("p.md", "---\ntitle: Maybe\n---\nNot sure yet.\n");

    // undo: a queued write is dropped before it is sent
    let t = alice.json(&["--offline", "post", s(&f), "--yes"]);
    assert_eq!(t["state"]["kind"], "pending", "{t}");
    let id = t["id"].as_str().unwrap().to_owned();
    let u = alice.json(&["outbox", "undo", &id]);
    assert_eq!(u["state"]["kind"], "undone", "{u}");
    assert_eq!(
        alice.ok(&["outbox", "flush"]).lines().next(),
        Some("Sent 0 writes")
    );
    assert!(alice.account.list(nsid::POST).is_empty());
    let e = alice.fails(1, &["outbox", "undo", "not-a-write"]);
    assert_eq!(e["code"], "invalid_input", "{e}");

    // retry: a write the PDS refused is failed, and sent again on request
    alice.account.inject(
        lessdugin_testkit::fake_pds::Fault::error(400, "InvalidRequest", "try again later")
            .on_nsid("com.atproto.repo.createRecord")
            .once(),
    );
    let out = alice.run(&["--json", "post", s(&f), "--yes"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let failed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(failed["state"]["kind"], "failed", "{failed}");
    assert!(alice.account.list(nsid::POST).is_empty());
    let id = failed["id"].as_str().unwrap();
    let human = alice.ok(&["outbox"]);
    assert!(human.contains("failed: "), "{human}");
    let t = alice.json(&["outbox", "retry", id]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    assert_eq!(alice.account.list(nsid::POST).len(), 1);

    // sync --full, and a member whose repository cannot be read: partial, exit 6
    let a = alice.json(&["approve", "bob.test"]);
    assert_eq!(a["writes"][0]["state"]["kind"], "done", "{a}");
    let t = bob.json(&["join", "alice.test/lounge"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    let t = bob.json(&["comment", &t_uri(&alice), "-m", "Hi."]);
    assert_eq!(t["state"]["kind"], "done", "{t}");
    // bob's repository fails before alice ever read it
    let fault = bob
        .account
        .inject(lessdugin_testkit::fake_pds::Fault::status(500));
    let out = alice.run(&["--json", "sync", "--full"]);
    assert_eq!(out.status.code(), Some(6), "{}", text(&out.stderr));
    let r: SyncReport = exact(&text(&out.stdout));
    assert!(r.partial, "{r:?}");
    assert!(
        r.errors
            .iter()
            .any(|e| e.did.as_deref() == Some(bob.did().as_str())),
        "{r:?}"
    );
    let out = alice.run(&["sync"]);
    assert_eq!(out.status.code(), Some(6));
    assert!(
        text(&out.stderr).contains("partial"),
        "{}",
        text(&out.stderr)
    );
    net.net.remove_fault(fault);
    let mut st = settings();
    st.add_filter(r"\d+ (requests?|records? changed)", "<n> $1");
    st.bind(|| {
        insta::assert_snapshot!("sync_full", redact(&users, &alice.ok(&["sync", "--full"])));
    });
    let r: SyncReport = exact(&json_out(&alice, &["sync", "--full"]));
    assert!(!r.partial && r.repos_complete == r.repos_total, "{r:?}");
    let tv: ThreadView = exact(&json_out(&alice, &["read", &t_uri(&alice)]));
    assert_eq!(tv.comments.len(), 1);
}

/// The uri of `u`'s only post.
fn t_uri(u: &User) -> String {
    let (rkey, _) = &u.account.list(nsid::POST)[0];
    format!("at://{}/{}/{rkey}", u.did(), nsid::POST)
}
