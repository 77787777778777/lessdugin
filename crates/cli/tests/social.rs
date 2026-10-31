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
        bob.json(&["join", "alice.test/meta", "--yes"]);
        carol.json(&["join", "alice.test/meta", "--yes"]);
        let a = alice.json(&["approve", "bob.test", "carol.test", "--yes"]);
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

    // the first vote is the public interstitial: headless, it needs --yes, and without it
    // nothing is written (engine-api.md §3.6)
    let out = w.bob.run(&["vote", &w.post, "up"]);
    assert_eq!(out.status.code(), Some(2), "{:?}", text(&out.stderr));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("public") && stderr.contains("--yes"),
        "{stderr}"
    );
    assert!(
        w.bob
            .account
            .list(lessdugin_lexicon::collection::nsid::OPINION)
            .is_empty()
    );
    // with --yes the notice is shown and acknowledged, then the vote records
    let out = w.bob.run(&["vote", &w.post, "up", "--yes"]);
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
    let r = w.carol.json(&["react", &w.post, "crux", "--yes"]);
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
    let e = w.alice.fails(1, &["vote", &w.post, "up", "--yes"]);
    assert_eq!(e["code"], "invalid_input", "{e}");

    // an unknown react is refused before anything is written
    let e = w.bob.fails(1, &["react", &w.post, "nope", "--yes"]);
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

/// Joining (with its note) and granting are public actions too (`social.md` §17): the first
/// `join`, `approve` and `mod approve` (a grant is an `#access` mod action) show the
/// interstitial, which headless use confirms with `--yes`; without it nothing is written. Once
/// acknowledged, the kind asks nothing again.
#[test]
fn join_and_approve_ask_for_the_public_interstitial_once() {
    use lessdugin_lexicon::collection::nsid;
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let carol = net.user("carol.test", true);
    alice.json(&["forum", "create", "meta"]);
    carol.json(&["forum", "create", "lounge"]);

    // bob's first join: refused headless, and no membership is written
    let e = bob.fails(2, &["join", "alice.test/meta", "--note", "hello"]);
    assert_eq!(e["code"], "usage", "{e}");
    let msg = e["message"].as_str().unwrap();
    assert!(msg.contains("membership") && msg.contains("--yes"), "{e}");
    assert!(bob.account.list(nsid::MEMBERSHIP).is_empty());
    let out = bob.run(&["join", "alice.test/meta", "--note", "hello", "--yes"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("public"),
        "{}",
        text(&out.stderr)
    );
    // acknowledged: the next join asks nothing
    let t = bob.json(&["join", "carol.test/lounge"]);
    assert_eq!(t["state"]["kind"], "done", "{t}");

    // alice's first grant: refused headless, no grant written
    let e = alice.fails(2, &["approve", "bob.test"]);
    assert_eq!(e["code"], "usage", "{e}");
    assert!(alice.account.list(nsid::MOD_ACTION).is_empty());
    let a = alice.json(&["approve", "bob.test", "--yes"]);
    assert_eq!(a["writes"][0]["state"]["kind"], "done", "{a}");

    // `mod approve` is the same grant: carol's first one needs --yes too
    let e = carol.fails(
        2,
        &["mod", "approve", "bob.test", "--forum", "carol.test/lounge"],
    );
    assert_eq!(e["code"], "usage", "{e}");
    assert!(carol.account.list(nsid::MOD_ACTION).is_empty());
    let a = carol.json(&[
        "mod",
        "approve",
        "bob.test",
        "--forum",
        "carol.test/lounge",
        "--yes",
    ]);
    assert_eq!(a["writes"][0]["state"]["kind"], "done", "{a}");
    // and her later mod actions ask nothing
    let m = carol.json(&["mods", "add", "bob.test"]);
    assert_eq!(m["state"]["kind"], "done", "{m}");
}

/// Author moderation (`social.md` §4.9) reaches every author-scope action and its undo: the
/// post's author hides a comment in their thread and removes that hide again (the scope is
/// part of the record's key, `data-model.md` §6.7), pins a comment and locks their own post,
/// each with `--author-scope`.
#[test]
fn author_scope_lock_pin_and_removal() {
    use lessdugin_lexicon::collection::nsid;
    let w = World::new();
    let c = w.bob.json(&["comment", &w.post, "-m", "A reply."]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    let comment = c["state"]["uri"].as_str().unwrap().to_owned();
    w.alice.ok(&["sync", &w.forum]);
    // alice's author-scope records, by action
    let author_records = |what: &str| -> Vec<Value> {
        w.alice
            .account
            .list(nsid::MOD_ACTION)
            .iter()
            .filter_map(|(rkey, _)| w.alice.account.get(nsid::MOD_ACTION, rkey))
            .filter(|v| {
                v["scope"] == "author"
                    && v["action"]["$type"]
                        .as_str()
                        .is_some_and(|t| t.ends_with(what))
            })
            .collect()
    };

    let h = w.alice.json(&[
        "mod",
        "hide",
        &comment,
        "--author-scope",
        "--reason",
        "off topic",
        "--yes",
    ]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
    assert_eq!(author_records("#hide").len(), 1);
    // the removal names the author scope (a forum-scope unhide finds nothing to remove)
    let e = w.alice.fails(4, &["mod", "unhide", &comment]);
    assert_eq!(e["code"], "not_found", "{e}");
    let u = w.alice.json(&["mod", "unhide", &comment, "--author-scope"]);
    assert_eq!(u["state"]["kind"], "done", "{u}");
    assert!(author_records("#hide").is_empty());

    let p = w.alice.json(&["mod", "pin", &comment, "--author-scope"]);
    assert_eq!(p["state"]["kind"], "done", "{p}");
    let pins = author_records("#pin");
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0]["action"]["scope"], "thread", "{}", pins[0]);

    let l = w.alice.json(&["mod", "lock", &w.post, "--author-scope"]);
    assert_eq!(l["state"]["kind"], "done", "{l}");
    let locks = author_records("#lock");
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0]["subject"], w.post.as_str(), "{}", locks[0]);

    // a grant is a forum action only
    let e = w
        .alice
        .fails(2, &["mod", "approve", "bob.test", "--author-scope"]);
    assert_eq!(e["code"], "usage", "{e}");
}

/// Reading a thread sets its read mark (`social.md` §16, `engine-api.md` §3.6 `mark_read`): a
/// comment that arrives after a `read` shows unread on the next one (and in `ls`), and read on
/// the one after. A thread never read has no read mark, so nothing in it is unread.
#[test]
fn reading_a_thread_marks_it_read() {
    let w = World::new();
    let unread = |tv: &ThreadView| -> Vec<bool> { tv.comments.iter().map(|c| c.unread).collect() };
    let c = w.bob.json(&["comment", &w.post, "-m", "A reply."]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    w.alice.ok(&["sync", &w.forum]);
    let first: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    assert_eq!(unread(&first), [false], "{:?}", first.comments);

    let c = w.carol.json(&["comment", &w.post, "-m", "Another reply."]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    w.alice.ok(&["sync", &w.forum]);
    let pl: PostList = exact(&w.alice.ok(&["--json", "ls", "--all"]));
    assert_eq!(pl.items[0].unread_comments, Some(1), "{:?}", pl.items[0]);
    let second: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    let new: Vec<bool> = second
        .comments
        .iter()
        .filter(|c| c.body.text.contains("Another"))
        .map(|c| c.unread)
        .collect();
    assert_eq!(new, [true], "{:?}", second.comments);
    let third: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    assert_eq!(unread(&third), [false, false], "{:?}", third.comments);
    let pl: PostList = exact(&w.alice.ok(&["--json", "ls", "--all"]));
    assert_eq!(pl.items[0].unread_comments, Some(0), "{:?}", pl.items[0]);
}

/// `#restrict` holds both the capability set and `nullifyVotes` in one record, which states
/// the complete desired state of its key (`social.md` §4.2, §4.7): `mod nullify` keeps the
/// moderator's restriction, and `mod restrict` keeps the nullification. The engine merges the
/// dimension a verb leaves out (`engine-api.md` §3.5); the CLI syncs the forum first, so a
/// record written from another device is the one it merges with (m3r-2).
#[test]
fn restrict_and_nullify_keep_each_other() {
    use lessdugin_lexicon::collection::nsid;
    let w = World::new();
    let restrict_of = |did: &str| -> Value {
        let mut recs: Vec<Value> = w
            .alice
            .account
            .list(nsid::MOD_ACTION)
            .iter()
            .filter_map(|(rkey, _)| w.alice.account.get(nsid::MOD_ACTION, rkey))
            .filter(|v| v["action"]["$type"] == nsid::MOD_ACTION_RESTRICT && v["subject"] == did)
            .collect();
        assert_eq!(recs.len(), 1, "{recs:?}");
        recs.remove(0)["action"].clone()
    };
    let bob = w.bob.did();
    let restrict = || restrict_of(&bob);
    let r = w
        .alice
        .json(&["mod", "restrict", "bob.test", "--cap", "post", "--yes"]);
    assert_eq!(r["state"]["kind"], "done", "{r}");
    let n = w.alice.json(&["mod", "nullify", "bob.test"]);
    assert_eq!(n["state"]["kind"], "done", "{n}");
    let a = restrict();
    assert_eq!(a["capabilities"], serde_json::json!(["post"]), "{a}");
    assert_eq!(a["nullifyVotes"], true, "{a}");

    let r = w
        .alice
        .json(&["mod", "restrict", "bob.test", "--cap", "comment"]);
    assert_eq!(r["state"]["kind"], "done", "{r}");
    let a = restrict();
    assert_eq!(a["capabilities"], serde_json::json!(["comment"]), "{a}");
    assert_eq!(a["nullifyVotes"], true, "{a}");

    // a restriction still queued in the outbox (offline) is the one the next verb keeps
    let carol = w.carol.did();
    let r = w
        .alice
        .json(&["--offline", "mod", "restrict", &carol, "--cap", "vote"]);
    assert_eq!(r["state"]["kind"], "pending", "{r}");
    let n = w.alice.json(&["--offline", "mod", "nullify", &carol]);
    assert_eq!(n["state"]["kind"], "pending", "{n}");
    let f = w.alice.json(&["outbox", "flush"]);
    let sent = f["writes"].as_array().unwrap();
    assert!(sent.iter().all(|x| x["state"]["kind"] == "done"), "{f}");
    let a = restrict_of(&carol);
    assert_eq!(a["capabilities"], serde_json::json!(["vote"]), "{a}");
    assert_eq!(a["nullifyVotes"], true, "{a}");

    // and one written from another device, which this one has not synced since
    let dave = w.net.user("dave.test", true).did();
    let other = w.net.second_device(&w.alice);
    let r = other.json(&["mod", "restrict", &dave, "--cap", "post", "--yes"]);
    assert_eq!(r["state"]["kind"], "done", "{r}");
    let n = w.alice.json(&["mod", "nullify", &dave]);
    assert_eq!(n["state"]["kind"], "done", "{n}");
    let a = restrict_of(&dave);
    assert_eq!(a["capabilities"], serde_json::json!(["post"]), "{a}");
    assert_eq!(a["nullifyVotes"], true, "{a}");
}

/// The CLI reaches the owner's and the reader's remaining M3 operations (`engine-api.md` §3.2,
/// §3.6): `forum edit` (the membership mode among the fields; what is not given is kept),
/// `reject` (an applicant leaves the pending list; approving brings them back), `mute` (the
/// lens collapses the author's items, locally) and the karma-notification cadence.
#[test]
fn forum_edit_reject_mute_and_the_karma_cadence() {
    use lessdugin_lexicon::collection::nsid;
    let w = World::new();

    // forum edit: open mode (with the L7 label), a description; the name stays
    let out = w.alice.run(&[
        "forum",
        "edit",
        "--mode",
        "open",
        "--description",
        "Talk",
        "--report-contact",
        "mailto:mods@example.com",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("Open mode relies on"),
        "{}",
        text(&out.stderr)
    );
    let decl = w.alice.account.get(nsid::FORUM, "meta").unwrap();
    assert_eq!(decl["membership"], "open", "{decl}");
    assert_eq!(decl["description"], "Talk", "{decl}");
    assert_eq!(decl["reportContact"], "mailto:mods@example.com", "{decl}");
    assert_eq!(decl["name"], "meta", "{decl}");
    assert_eq!(decl["defaultTier"], "frontpage", "{decl}");
    // back to approval (archiving), the description removed, the contact kept
    let e = w
        .alice
        .json(&["forum", "edit", "--mode", "approval", "--description", ""]);
    assert_eq!(e["state"]["kind"], "done", "{e}");
    let decl = w.alice.account.get(nsid::FORUM, "meta").unwrap();
    assert_eq!(decl["membership"], "approval", "{decl}");
    assert!(decl.get("description").is_none(), "{decl}");
    assert_eq!(decl["reportContact"], "mailto:mods@example.com", "{decl}");
    // only the owner edits
    let e = w
        .bob
        .fails(1, &["forum", "edit", "alice.test/meta", "--name", "Mine"]);
    assert_eq!(e["code"], "blocked", "{e}");

    // reject: dave applies, and alice dismisses him (on this device: nothing to remove)
    let dave = w.net.user("dave.test", true);
    dave.json(&["join", &w.forum, "--yes"]);
    let grants = || -> usize {
        w.alice
            .account
            .list(nsid::MOD_ACTION)
            .iter()
            .filter_map(|(rkey, _)| w.alice.account.get(nsid::MOD_ACTION, rkey))
            .filter(|v| v["subject"] == dave.did().as_str())
            .count()
    };
    let r = w.alice.json(&["reject", "dave.test"]);
    assert_eq!(r["writes"].as_array().unwrap().len(), 0, "{r}");
    // approving grants him (and forgets the rejection); rejecting him then removes the
    // account's own grant
    let a = w.alice.json(&["approve", "dave.test"]);
    assert_eq!(a["writes"][0]["state"]["kind"], "done", "{a}");
    assert_eq!(grants(), 1);
    let r = w.alice.json(&["reject", "dave.test"]);
    let writes = r["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 1, "{r}");
    assert_eq!(writes[0]["state"]["kind"], "done", "{r}");
    assert_eq!(grants(), 0);

    // mute: bob's comment is collapsed for alice only, until she unmutes him
    let c = w.bob.json(&["comment", &w.post, "-m", "A reply."]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    w.alice.ok(&["sync", &w.forum]);
    let m = w.alice.json(&["mute", "bob.test"]);
    assert_eq!(m["mutes"][0]["did"], w.bob.did(), "{m}");
    let tv: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    assert!(
        matches!(
            tv.comments[0].state,
            lessdugin_core::dto::ItemState::Collapsed {
                reason: lessdugin_core::dto::CollapseReason::Muted
            }
        ),
        "{:?}",
        tv.comments[0].state
    );
    let listed = w.alice.ok(&["mutes"]);
    assert!(listed.contains("bob.test"), "{listed}");
    let m = w.alice.json(&["unmute", "bob.test"]);
    assert!(m["mutes"].as_array().unwrap().is_empty(), "{m}");
    let tv: ThreadView = exact(&w.alice.ok(&["--json", "read", &w.post]));
    assert_eq!(
        tv.comments[0].state,
        lessdugin_core::dto::ItemState::Visible
    );

    // the karma cadence is saved on this device
    let cadence = || -> Option<String> {
        let x = w.alice.json(&["export", "--local"]);
        x["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["parts"] == serde_json::json!(["loc", "karmaCadence"]))
            .map(|m| m["value"].as_str().unwrap().to_owned())
    };
    assert_eq!(cadence(), None);
    let out = w.alice.run(&["notifications", "--karma-cadence", "weekly"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("weekly"),
        "{}",
        text(&out.stderr)
    );
    let weekly = cadence().expect("the cadence is saved");
    w.alice
        .ok(&["notifications", "--karma-cadence", "realtime"]);
    assert_ne!(cadence().unwrap(), weekly);
}

/// The human thread shows what the JSON carries (`social.md` §7 "Display", §9.2, §4.11): the
/// agreement, "N of M votes were cast on an earlier version", the react tallies listed under
/// the item with their creator and quote (or "quoted text no longer present"), and the
/// "moderation data unavailable" banner of a degraded view.
#[test]
fn the_human_thread_shows_agreement_reacts_and_banners() {
    let w = World::new();
    w.bob.json(&["vote", &w.post, "up", "--yes"]);
    w.bob.json(&["agree", &w.post, "strong-agree", "--yes"]);
    w.carol
        .json(&["react", &w.post, "crux", "--quote", "First post", "--yes"]);
    w.bob.json(&["react", &w.post, "crux", "--yes"]);
    w.alice.ok(&["sync", &w.forum]);
    let out = w.alice.ok(&["read", &w.post, "--no-pager"]);
    assert!(out.contains("agreement +"), "{out}");
    // the creators are named by handle when the thread knows it, else by short DID
    let carol_short = lessdugin_core::identity::short_did(&w.carol.did());
    let bob_short = lessdugin_core::identity::short_did(&w.bob.did());
    assert!(
        out.contains(&format!(
            "Crux by {carol_short} on \u{201c}First post\u{201d}"
        )),
        "{out}"
    );
    assert!(out.contains(&format!("Crux by {bob_short}\n")), "{out}");

    // the edit moves the votes to an earlier version and orphans the quote
    let e = w
        .alice
        .json(&["edit", &w.post, "-m", "Rewritten from scratch."]);
    assert_eq!(e["state"]["kind"], "done", "{e}");
    w.alice.ok(&["sync", &w.forum]);
    let out = w.alice.ok(&["read", &w.post, "--no-pager"]);
    assert!(
        out.contains("1 of 1 votes were cast on an earlier version"),
        "{out}"
    );
    assert!(out.contains("(quoted text no longer present)"), "{out}");

    // a moderator's repository is unavailable: the view is degraded, and says so
    w.alice.json(&["mods", "add", "bob.test"]);
    w.bob.account.deactivate();
    let dave = w.net.reader(&w.alice);
    // the sync is partial (exit 6) with a moderator's repository unavailable
    dave.run(&["sync", &w.forum]);
    let out = dave.ok(&["--offline", "read", &w.post, "--no-pager"]);
    assert!(out.contains("moderation data unavailable"), "{out}");
    let out = dave.ok(&["--offline", "ls", &w.forum, "--all"]);
    assert!(out.contains("moderation data unavailable"), "{out}");
}

/// `forum edit` keeps the about it is not asked to change, also a spilled one (`textBlob`,
/// over 64 KiB, `data-model.md` §5.3) and its media: the text is fetched from the owner's PDS
/// and spilled again, the media sent again as references (m4-5). `forum show` shows a spilled
/// about's text, which the engine fetches from the owner's PDS (m4-5, engine part).
#[test]
fn forum_edit_keeps_a_spilled_about_and_its_media() {
    use lessdugin_lexicon::collection::nsid;
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\
        \x00\x00\x00\x0dIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\
        \x00\x00\x00\x0bIDAT\x78\xda\x63\x60\x00\x02\x00\x00\x05\x00\x01\xe9\xfa\xdc\xd8\
        \x00\x00\x00\x00IEND\xae\x42\x60\x82";
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let about = format!("# About\n\n{}\n", "All about it. ".repeat(5_000));
    assert!(about.len() > 65_536);
    let c = alice.json(&["forum", "create", "big", "--about", &about]);
    assert_eq!(c["state"]["kind"], "done", "{c}");
    let mut decl = alice.account.get(nsid::FORUM, "big").unwrap();
    let text_blob = decl["about"]["textBlob"]["ref"].clone();
    assert!(text_blob.is_object(), "spilled: {decl}");
    // another client adds an image to the about
    let png = alice.account.upload_blob(PNG.to_vec(), "image/png");
    decl["about"]["media"] = serde_json::json!([{ "blob": png, "alt": "a dot" }]);
    alice.account.put(nsid::FORUM, "big", &decl);
    alice.ok(&["sync", "alice.test/big"]);

    let out = alice.ok(&["forum", "show", "alice.test/big"]);
    assert!(out.contains("All about it."), "{out}");
    assert!(!out.contains("stored as a blob"), "{out}");

    let e = alice.json(&["forum", "edit", "alice.test/big", "--name", "Big"]);
    assert_eq!(e["state"]["kind"], "done", "{e}");
    let decl = alice.account.get(nsid::FORUM, "big").unwrap();
    assert_eq!(decl["name"], "Big", "{decl}");
    assert_eq!(decl["about"]["textBlob"]["ref"], text_blob, "{decl}");
    assert!(decl["about"].get("text").is_none(), "{decl}");
    let media = decl["about"]["media"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(media.len(), 1, "{decl}");
    assert_eq!(media[0]["blob"]["ref"], png["ref"], "{decl}");
    assert_eq!(media[0]["alt"], "a dot", "{decl}");
}
