// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 social commands through the real CLI on the FakePds network (`support/fakenet.rs`):
//! votes, agreement, reacts (with the public interstitial on first use), notifications,
//! Recent Discussion and the read paths of `mod queue` / `mod log`.
//!
//! The fold semantics these ride on are the engine's (tested in `crates/core/tests`); this
//! file asserts the CLI's plumbing: argument handling, the §17 acknowledgement, the write
//! drive, the dto/1 output and the human rendering.

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, s, text};
use lessdugin_core::dto::{NotificationList, PostList, ThreadView};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Deserializes and checks the dto/1 round-trip (the `content.rs` helper).
fn exact<T: Serialize + DeserializeOwned>(json: &str) -> T {
    let v: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}: {json}"));
    let again = serde_json::to_string_pretty(&v).unwrap();
    assert_eq!(again.trim_end(), json.trim_end(), "not byte-exact dto/1");
    v
}

/// A forum with one post by alice, bob and carol approved members.
struct World {
    net: Net,
    alice: fakenet::User,
    bob: fakenet::User,
    carol: fakenet::User,
    post: String,
    forum: String,
}

impl World {
    fn new() -> Self {
        let net = Net::new();
        let alice = net.user("alice.test", true);
        let bob = net.user("bob.test", true);
        let carol = net.user("carol.test", true);
        alice.json(&["forum", "create", "meta", "--frontpage"]);
        bob.json(&["join", "alice.test/meta"]);
        carol.json(&["join", "alice.test/meta"]);
        let a = alice.json(&["approve", "bob.test", "carol.test"]);
        for w in a["writes"].as_array().unwrap() {
            assert_eq!(w["state"]["kind"], "done", "{w}");
        }
        let f = alice.file("p.md", "---\ntitle: Hello\n---\nFirst post.\n");
        let p = alice.json(&["post", s(&f), "--yes"]);
        let post = p["state"]["uri"].as_str().unwrap().to_owned();
        for u in [&bob, &carol] {
            u.ok(&["sync", "alice.test/meta"]);
        }
        Self {
            net,
            alice,
            bob,
            carol,
            post,
            forum: "alice.test/meta".to_owned(),
        }
    }
}

#[test]
fn votes_agreement_and_reacts_move_the_score_and_notify() {
    let w = World::new();

    // the first vote shows the public interstitial (headless: a note on stderr), then records
    let out = w.bob.run(&["vote", &w.post, "up"]);
    assert!(out.status.success(), "{:?}", text(&out.stderr));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("public"), "{stderr}");
    assert!(
        text(&out.stdout).contains("Voted"),
        "{:?}",
        text(&out.stdout)
    );

    // the vote is a record in bob's repository
    let opinions = w
        .bob
        .account
        .list(lessdugin_lexicon::collection::nsid::OPINION);
    assert_eq!(opinions.len(), 1, "{opinions:?}");
    let rec = w
        .bob
        .account
        .get(lessdugin_lexicon::collection::nsid::OPINION, &opinions[0].0)
        .unwrap();
    assert_eq!(rec["karma"], "up", "{rec}");

    // alice sees it: the score is above her self-vote alone
    w.alice.ok(&["sync", &w.forum]);
    let pl: PostList = exact(&w.alice.ok(&["--json", "ls", "--all"]));
    assert_eq!(pl.items.len(), 1);
    assert!(pl.items[0].score.score >= 2, "{}", pl.items[0].score.score);

    // agreement on top
    let a = w.bob.json(&["agree", &w.post, "agree"]);
    assert_eq!(a["state"]["kind"], "done", "{a}");
    w.alice.ok(&["sync", &w.forum]);
    let tv: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    let root = match &tv.post {
        lessdugin_core::dto::ThreadRoot::Post(p) => &p.post,
        _ => panic!("a post thread"),
    };
    assert!(
        root.score.agreement.unwrap_or(0) >= 1,
        "{:?}",
        root.score.agreement
    );

    // a react (the palette lists the ids; `crux` is in default/1)
    let reacts = w.carol.ok(&["reacts"]);
    assert!(reacts.contains("crux"), "{reacts}");
    // Notifications are computed from first-seen times (social.md §16), never from
    // `createdAt`: the first call sets the baseline, so it must come before the react.
    let base: NotificationList = exact(&w.alice.ok(&["--json", "notifications", "--all"]));
    assert_eq!(base.unread, 0, "{base:?}");
    let r = w.carol.json(&["react", &w.post, "crux"]);
    assert_eq!(r["state"]["kind"], "done", "{r}");
    w.alice.ok(&["sync", &w.forum]);
    let tv: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    let root = match &tv.post {
        lessdugin_core::dto::ThreadRoot::Post(p) => p,
        _ => panic!("a post thread"),
    };
    assert!(
        root.reacts
            .iter()
            .any(|t| t.react == "crux" && t.for_count >= 1),
        "{:?}",
        root.reacts
    );

    // clearing a vote removes it (the opinion record goes when the axes are empty)
    let c = w.bob.json(&["vote", &w.post, "clear"]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    let opinions = w
        .bob
        .account
        .list(lessdugin_lexicon::collection::nsid::OPINION);
    // the agreement axis keeps the record alive
    assert_eq!(opinions.len(), 1, "{opinions:?}");
    let rec = w
        .bob
        .account
        .get(lessdugin_lexicon::collection::nsid::OPINION, &opinions[0].0)
        .unwrap();
    assert!(
        rec.get("karma").is_none() || rec["karma"].is_null(),
        "{rec}"
    );

    // alice was notified of the react on her post
    let n: NotificationList = exact(&w.alice.ok(&["--json", "notifications", "--all"]));
    assert!(n.unread >= 1, "{n:?}");
    assert!(
        n.items
            .iter()
            .any(|x| x.reason == lessdugin_core::dto::NotificationReason::Reacted),
        "{n:?}"
    );
    // marking them read clears the badge
    w.alice.json(&["notifications", "--mark-read"]);
    let n2: NotificationList = exact(&w.alice.ok(&["--json", "notifications", "--all"]));
    assert_eq!(n2.unread, 0, "{n2:?}");
}

#[test]
fn votes_refuse_what_cannot_count_and_activity_lists_discussion() {
    let w = World::new();

    // no voting on your own post
    let e = w.alice.fails(1, &["vote", &w.post, "up"]);
    assert_eq!(e["code"], "invalid_input", "{e}");

    // an unknown react is refused before anything is written
    let e = w.bob.fails(1, &["react", &w.post, "nope"]);
    assert_eq!(e["code"], "invalid_input", "{e}");

    // Recent Discussion lists the post once it has a comment
    let c = w.carol.json(&["comment", &w.post, "-m", "A reply."]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    w.alice.ok(&["sync", &w.forum]);
    let act = w.alice.json(&["activity"]);
    assert_eq!(act["items"].as_array().unwrap().len(), 1, "{act}");
    assert_eq!(act["items"][0]["post"]["uri"], w.post, "{act}");
    assert_eq!(
        act["items"][0]["comments"].as_array().unwrap().len(),
        1,
        "{act}"
    );

    // the mod log is public: the approvals (access/modAction grants) are already in it,
    // but no hide yet; the queue is for moderators only
    let log = w.carol.json(&["mod", "log"]);
    let entries = log["entries"].as_array().unwrap();
    assert!(entries.iter().all(|e| e["action"] != "hide"), "{log}");
    let e = w.carol.fails(1, &["mod", "queue"]);
    assert_eq!(e["code"], "blocked", "{e}");
    let q = w.alice.json(&["mod", "queue"]);
    assert!(q["items"].is_array(), "{q}");

    // alice hides the comment: it leaves the thread for a fresh reader, and the log shows it
    let comments = w.alice.json(&["read", &w.post]);
    let comment_uri = comments["comments"][0]["uri"].as_str().unwrap().to_owned();
    let h = w
        .alice
        .json(&["mod", "hide", &comment_uri, "--reason", "off topic"]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
    let dave = w.net.reader(&w.alice);
    dave.ok(&["sync", &w.forum]);
    let out = dave.ok(&["read", &w.post]);
    assert!(
        !out.contains("A reply."),
        "the hidden comment is shown: {out}"
    );
    let log = w.alice.json(&["mod", "log"]);
    let hides: Vec<&Value> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "hide")
        .collect();
    assert_eq!(hides.len(), 1, "{log}");
    assert_eq!(hides[0]["status"], "effective", "{log}");
    assert_eq!(hides[0]["reason"], "off topic", "{log}");
}
