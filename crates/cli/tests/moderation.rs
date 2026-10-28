// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 acceptance checks of `docs/plan.md` §M3 at the CLI level:
//!
//! - a post lock is refused above 5,000 allowed comments (the moderator is told to hide
//!   instead; the engine's own case is `a_lock_keeps_the_visible_comments_and_is_refused_
//!   above_5000` in `crates/core/tests/engine_actions.rs` — this one drives it through
//!   `lessdugin mod lock`);
//! - `mods rm` runs `mod adopt` first: the departing moderator's assertions are re-issued
//!   under the remover (with `adopts` pointing at each original) before the declaration
//!   edits, and the ticket says so (`adoptedFirst` notice); `mods rm --skip-adopt` lists the
//!   assertions that lapse and needs a confirmation;
//! - a backdated item shows its `t_rank`, not its claimed `createdAt` (`social.md` §2.3).
//!
//! All run on the FakePds network (`support/fakenet.rs`) with the real CLI binary.

#![allow(clippy::disallowed_methods)] // a real clock and tid for the seeded repositories

#[path = "support/fakenet.rs"]
mod fakenet;

use fakenet::{Net, s};
use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_core::discovery::ForumKey;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::records::{
    AccessAction, Action, Forum, Markdown, Md, Membership, ModAction, Post,
};
use lessdugin_lexicon::values::{AccessLevel, MembershipMode};
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::repo::tid_at_ms;
use serde_json::{Value, json};

/// Writes records straight into the fake repositories (the `cold_sync.rs` seeding shape).
struct Seed {
    base_ms: i64,
    seq: std::cell::Cell<i64>,
}

impl Seed {
    fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        Self {
            base_ms: i64::try_from(now).unwrap() - 3_600_000,
            seq: std::cell::Cell::new(0),
        }
    }

    fn ms(&self) -> i64 {
        self.seq.set(self.seq.get() + 1);
        self.base_ms + self.seq.get()
    }

    fn dt(&self) -> Datetime {
        Datetime::from_millis(self.ms()).unwrap()
    }

    fn tid(&self) -> String {
        tid_at_ms(self.ms()).to_string()
    }

    fn put(&self, a: &AccountHandle, coll: &str, rkey: &str, v: &Value) -> String {
        let uri = AtUri::parse(&a.uri(coll, rkey)).unwrap();
        let validity = lessdugin_lexicon::validity::check_uri(&uri, v);
        assert!(validity.first_rule().is_none(), "{uri}: {validity:?}");
        a.put(coll, rkey, v);
        uri.to_string()
    }

    fn forum(&self, owner: &AccountHandle, slug: &str) -> ForumKey {
        let key = ForumKey::new(owner.did(), slug).unwrap();
        let f = Forum::create(slug, self.dt(), MembershipMode::Approval);
        self.put(owner, nsid::FORUM, slug, &serde_json::to_value(&f).unwrap());
        let m = Membership::new(key.uri.clone(), self.dt());
        self.put(
            owner,
            nsid::MEMBERSHIP,
            &key.fk,
            &serde_json::to_value(&m).unwrap(),
        );
        key
    }

    fn member(&self, a: &AccountHandle, owner: &AccountHandle, f: &ForumKey) {
        let m = Membership::new(f.uri.clone(), self.dt());
        self.put(
            a,
            nsid::MEMBERSHIP,
            &f.fk,
            &serde_json::to_value(&m).unwrap(),
        );
        let action = Action::Access(AccessAction::new(AccessLevel::Member));
        let ma = ModAction::new(f.uri.clone(), a.did().as_str(), action, self.dt());
        let v = serde_json::to_value(&ma).unwrap();
        let am = v["action"].as_object().unwrap().clone();
        let rkey = keys::mod_action_rkey(NS, f.uri.as_str(), None, a.did().as_str(), &am).unwrap();
        self.put(owner, nsid::MOD_ACTION, &rkey, &v);
    }

    fn post(&self, a: &AccountHandle, f: &ForumKey) -> (String, String) {
        let p = Post::article(f.uri.clone(), "The post", "Body.", self.dt());
        let rkey = keys::post_rkey(f.uri.as_str(), &self.tid()).unwrap();
        let uri = self.put(a, nsid::POST, &rkey, &serde_json::to_value(&p).unwrap());
        let cid = a.cid(nsid::POST, &rkey).unwrap().to_string();
        (uri, cid)
    }

    fn comment(&self, a: &AccountHandle, f: &ForumKey, root: &(String, String), i: usize) {
        let at = Datetime::from_millis(self.base_ms + 10_000 + i as i64).unwrap();
        self.comment_claiming(a, f, root, i, &at);
    }

    /// A comment whose `createdAt` claims `created` (its TID is the real time).
    fn comment_claiming(
        &self,
        a: &AccountHandle,
        f: &ForumKey,
        root: &(String, String),
        i: usize,
        created: &Datetime,
    ) {
        let body = serde_json::to_value(Md::Known(Markdown::inline("A reply."))).unwrap();
        let v = json!({
            "$type": nsid::COMMENT,
            "root": {"uri": root.0, "cid": root.1},
            "forum": f.uri.as_str(),
            "body": body,
            "createdAt": created.as_str(),
        });
        let rkey = keys::comment_rkey(
            NS,
            f.uri.as_str(),
            &root.0,
            tid_at_ms(self.base_ms + 10_000 + i as i64).as_ref(),
        )
        .unwrap();
        self.put(a, nsid::COMMENT, &rkey, &v);
    }
}

/// `lessdugin mod lock` is refused once a thread would keep more than 5,000 visible
/// comments (synthesis §4.4 F21): the moderator is told to hide the thread instead.
#[test]
fn a_lock_above_5000_allowed_comments_is_refused_with_the_hide_hint() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let seed = Seed::new();
    let f = seed.forum(&alice.account, "big");
    seed.member(&bob.account, &alice.account, &f);
    let post = seed.post(&alice.account, &f);
    // 5,001 visible comments in bob's repository (the limit is 5,000, `limits.rs`)
    for i in 0..5_001 {
        seed.comment(&bob.account, &f, &post, i);
    }
    let forum = f.uri.to_string();

    alice.ok(&["sync", &forum]);
    let e = alice.fails(1, &["mod", "lock", &post.0, "--yes"]);
    assert_eq!(e["code"], "invalid_input", "{e}");
    let msg = e["message"].as_str().unwrap();
    assert!(msg.contains("hide the thread instead"), "{e}");
    assert_eq!(e["detail"]["hint"], "useHide", "{e}");
    assert_eq!(e["detail"]["allowed"], 5_001, "{e}");

    // hiding is the way out (the hint is actionable)
    let h = alice.json(&["mod", "hide", &post.0, "--reason", "too big to lock"]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
}

/// `lessdugin mods rm` adopts the departing moderator's assertions first: they are
/// re-issued under the remover with `adopts` pointing at each original, the ticket carries
/// the `adoptedFirst` notice, and the declaration edit completes after the adoption.
#[test]
fn mods_rm_runs_mod_adopt_first() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let carol = net.user("carol.test", true);

    alice.json(&["forum", "create", "meta", "--frontpage"]);
    let f = alice.file("p.md", "---\ntitle: From carol\n---\nHello.\n");

    // bob and carol join and are approved; bob becomes a moderator
    for u in [&bob, &carol] {
        u.json(&["join", "alice.test/meta", "--yes"]);
    }
    let a = alice.json(&["approve", "bob.test", "carol.test", "--yes"]);
    for w in a["writes"].as_array().unwrap() {
        assert_eq!(w["state"]["kind"], "done", "{w}");
    }
    let m = alice.json(&["mods", "add", "bob.test"]);
    assert_eq!(m["state"]["kind"], "done", "{m}");

    // carol posts, and bob (a moderator, higher rank than the post's author) hides it
    carol.ok(&["sync", "alice.test/meta"]);
    let post = carol.json(&["post", s(&f), "--yes"]);
    let post_uri = post["state"]["uri"].as_str().unwrap().to_owned();
    bob.ok(&["sync", "alice.test/meta"]);
    let h = bob.json(&["mod", "hide", &post_uri, "--reason", "testing", "--yes"]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
    let bob_hide = bob
        .account
        .list(nsid::MOD_ACTION)
        .into_iter()
        .find(|(rkey, _)| {
            bob.account
                .get(nsid::MOD_ACTION, rkey)
                // `hidden: true` is the default and may be omitted; the $type is enough
                .is_some_and(|v| {
                    v["action"]["$type"]
                        .as_str()
                        .is_some_and(|t| t.ends_with("#hide"))
                        && v["action"]["hidden"] != serde_json::json!(false)
                })
        })
        .map(|(rkey, _)| bob.account.uri(nsid::MOD_ACTION, &rkey))
        .expect("bob's hide assertion");

    let r = alice.json(&["mods", "rm", "bob.test"]);
    assert_eq!(r["state"]["kind"], "done", "{r}");
    let codes: Vec<&str> = r["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"adoptedFirst"), "{r}");

    // alice now holds the adopted assertion, pointing at bob's original
    let adopted: Vec<Value> = alice
        .account
        .list(nsid::MOD_ACTION)
        .iter()
        .filter_map(|(rkey, _)| alice.account.get(nsid::MOD_ACTION, rkey))
        .filter(|v| v["adopts"]["uri"] == json!(bob_hide))
        .collect();
    assert_eq!(adopted.len(), 1, "bob's hide was adopted once");
    assert!(
        adopted[0]["action"]["$type"]
            .as_str()
            .is_some_and(|t| t.ends_with("#hide")),
        "the adopted action is a hide: {}",
        adopted[0]
    );
    assert_ne!(adopted[0]["action"]["hidden"], json!(false));

    // and the declaration no longer lists bob
    let forum_rec = alice.account.get(nsid::FORUM, "meta").unwrap();
    assert!(
        forum_rec.get("moderators").is_none()
            || forum_rec["moderators"].as_array().unwrap().is_empty(),
        "{forum_rec}"
    );

    // the adopted hide still applies: a fresh reader sees carol's post hidden
    let dave = net.reader(&alice);
    dave.ok(&["sync", "alice.test/meta"]);
    let out = dave.ok(&["ls", "alice.test/meta"]);
    assert!(
        !out.contains("From carol"),
        "the hidden post is listed: {out}"
    );
}

/// `ls` and `read` show an item's `t_rank`, never its self-asserted `createdAt`
/// (`social.md` §2.2–§2.3): a post and a comment backdated by years are shown at the forum's
/// creation (the causal lower bound), like the SPA shows them.
#[test]
fn a_backdated_item_shows_its_t_rank_not_its_claimed_time() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let seed = Seed::new();
    let f = seed.forum(&alice.account, "past");
    let declared = alice.account.get(nsid::FORUM, "past").unwrap()["createdAt"]
        .as_str()
        .unwrap()[..10]
        .to_owned();
    let claimed = Datetime::parse("2019-03-04T05:06:07Z").unwrap();
    let p = Post::article(f.uri.clone(), "Old news", "Body.", claimed.clone());
    let rkey = keys::post_rkey(f.uri.as_str(), &seed.tid()).unwrap();
    let uri = seed.put(
        &alice.account,
        nsid::POST,
        &rkey,
        &serde_json::to_value(&p).unwrap(),
    );
    let cid = alice.account.cid(nsid::POST, &rkey).unwrap().to_string();
    seed.comment_claiming(&alice.account, &f, &(uri.clone(), cid), 0, &claimed);
    let forum = f.uri.to_string();

    alice.ok(&["sync", &forum]);
    let ls = alice.ok(&["ls", &forum, "--all"]);
    assert!(ls.contains("Old news"), "{ls}");
    assert!(
        !ls.contains("2019-03-04"),
        "the claimed time is shown: {ls}"
    );
    assert!(ls.contains(&declared), "{ls}");
    let read = alice.ok(&["read", &uri, "--no-pager"]);
    assert!(read.contains("A reply."), "{read}");
    assert!(
        !read.contains("2019-03-04"),
        "the claimed time is shown: {read}"
    );
}

/// `mods rm --skip-adopt` lets the moderator's assertions lapse only after a confirmation that
/// lists every one of them (`social.md` §4.10, `data-model.md` §8.6): headless, without `--yes`
/// it is refused and nothing changes; with `--yes` the list is shown and the moderator is
/// removed without adoption.
#[test]
fn mods_rm_skip_adopt_lists_what_lapses_and_needs_a_confirmation() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let carol = net.user("carol.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    for u in [&bob, &carol] {
        u.json(&["join", "alice.test/meta", "--yes"]);
    }
    alice.json(&["approve", "bob.test", "carol.test", "--yes"]);
    alice.json(&["mods", "add", "bob.test"]);
    carol.ok(&["sync", "alice.test/meta"]);
    let f = carol.file("p.md", "---\ntitle: From carol\n---\nHello.\n");
    let post = carol.json(&["post", s(&f), "--yes"]);
    let post_uri = post["state"]["uri"].as_str().unwrap().to_owned();
    bob.ok(&["sync", "alice.test/meta"]);
    let h = bob.json(&["mod", "hide", &post_uri, "--reason", "spam run", "--yes"]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
    // a keep of a post edited since (a stale positive assertion, social.md §4.5): it counts
    // for nothing now, and lapses with the removal all the same
    let kept = stale_keep(&bob, &carol);
    let mods_of = |u: &fakenet::User| -> Vec<Value> {
        let rec = u.account.get(nsid::FORUM, "meta").unwrap();
        rec.get("moderators")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(mods_of(&alice).len(), 1);
    // the human mod log lists it (its separators are not ASCII)
    let log = alice.ok(&["mod", "log"]);
    assert!(log.contains("spam run") && log.contains(" · "), "{log}");

    // headless without --yes: the hide is listed, the removal refused, the declaration kept
    let out = alice.run(&["mods", "rm", "bob.test", "--skip-adopt"]);
    assert_eq!(out.status.code(), Some(2), "{}", fakenet::text(&out.stderr));
    let err = fakenet::text(&out.stderr);
    assert!(err.contains("lapse") && err.contains("--yes"), "{err}");
    assert!(err.contains(&post_uri) && err.contains("spam run"), "{err}");
    assert!(err.contains("2 assertions"), "{err}");
    assert!(
        err.lines()
            .any(|l| l.contains(&kept) && l.contains("visible") && l.contains("stale")),
        "{err}"
    );
    assert_eq!(mods_of(&alice).len(), 1, "the declaration changed");

    // confirmed: listed again, then removed without adopting
    let out = alice.run(&["mods", "rm", "bob.test", "--skip-adopt", "--yes"]);
    assert!(out.status.success(), "{}", fakenet::text(&out.stderr));
    let err = fakenet::text(&out.stderr);
    assert!(err.contains(&post_uri) && err.contains(&kept), "{err}");
    assert!(mods_of(&alice).is_empty());
    let adopted = alice
        .account
        .list(nsid::MOD_ACTION)
        .iter()
        .filter_map(|(rkey, _)| alice.account.get(nsid::MOD_ACTION, rkey))
        .filter(|v| v.get("adopts").is_some())
        .count();
    assert_eq!(adopted, 0, "nothing is adopted");
}

/// `bob` (a moderator) keeps (`#hide` visible) a new post of `carol`, who then edits it: the
/// keep names the old version, so it is `stale` in the mod log (`social.md` §4.5). Returns the
/// post's uri.
fn stale_keep(bob: &fakenet::User, carol: &fakenet::User) -> String {
    let f = carol.file("kept.md", "---\ntitle: Kept\n---\nAs it was.\n");
    let post = carol.json(&["post", s(&f), "--yes"]);
    let uri = post["state"]["uri"].as_str().unwrap().to_owned();
    bob.ok(&["sync", "alice.test/meta"]);
    let k = bob.json(&["mod", "keep", &uri, "--yes"]);
    assert_eq!(k["state"]["kind"], "done", "{k}");
    // the edit suspends the keep, which the author confirms
    let e = carol.json(&["edit", &uri, "-m", "As it is now.", "--yes"]);
    assert_eq!(e["state"]["kind"], "done", "{e}");
    uri
}

/// `mod adopt` and `mods rm` say which of the former moderator's assertions they did not
/// adopt (`data-model.md` §8.6 "cannot adopt … never counted"): those lapse with the role.
#[test]
fn adopting_reports_what_it_did_not_adopt() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let carol = net.user("carol.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    for u in [&bob, &carol] {
        u.json(&["join", "alice.test/meta", "--yes"]);
    }
    alice.json(&["approve", "bob.test", "carol.test", "--yes"]);
    alice.json(&["mods", "add", "bob.test", "--yes"]);
    carol.ok(&["sync", "alice.test/meta"]);
    let f = carol.file("p.md", "---\ntitle: From carol\n---\nHello.\n");
    let post = carol.json(&["post", s(&f), "--yes"]);
    let hidden = post["state"]["uri"].as_str().unwrap().to_owned();
    bob.ok(&["sync", "alice.test/meta"]);
    let h = bob.json(&["mod", "hide", &hidden, "--reason", "spam run", "--yes"]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
    let kept = stale_keep(&bob, &carol);

    // the effective hide is adopted; the stale keep never counted, so it is not
    let out = alice.run(&["mod", "adopt", "bob.test", "--yes"]);
    assert!(out.status.success(), "{}", fakenet::text(&out.stderr));
    let err = fakenet::text(&out.stderr);
    assert!(err.contains("adopted 1 of 2 assertions"), "{err}");
    let line = err
        .lines()
        .find(|l| l.contains(&kept))
        .unwrap_or_else(|| panic!("the keep is not reported: {err}"));
    assert!(line.contains("never counted (stale)"), "{line}");
    assert!(!err.lines().any(|l| l.contains(&hidden)), "{err}");

    // `mods rm` adopts again (nothing new) and says the same before the keep lapses
    let out = alice.run(&["mods", "rm", "bob.test", "--yes"]);
    assert!(out.status.success(), "{}", fakenet::text(&out.stderr));
    let err = fakenet::text(&out.stderr);
    assert!(err.contains("adopted 1 of 2 assertions"), "{err}");
    assert!(
        err.lines()
            .any(|l| l.contains(&kept) && l.contains("never counted (stale)")),
        "{err}"
    );
    let adopted: Vec<Value> = alice
        .account
        .list(nsid::MOD_ACTION)
        .iter()
        .filter_map(|(rkey, _)| alice.account.get(nsid::MOD_ACTION, rkey))
        .filter(|v| v.get("adopts").is_some())
        .collect();
    assert_eq!(adopted.len(), 1, "{adopted:?}");
    assert_eq!(adopted[0]["subject"], hidden.as_str(), "{adopted:?}");
}

/// An assertion in effect that the adoption skips is listed with the reason of the engine's
/// adopt report (`WriteState.done.report`, sec-8), for `mod adopt` and for the adoption `mods
/// rm` runs first: here the owner's own (expired) hide of the same post, which the adoption
/// keeps instead of overwriting it.
#[test]
fn adopting_names_the_engines_reason() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let bob = net.user("bob.test", true);
    let carol = net.user("carol.test", true);
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    for u in [&bob, &carol] {
        u.json(&["join", "alice.test/meta", "--yes"]);
    }
    alice.json(&["approve", "bob.test", "carol.test", "--yes"]);
    alice.json(&["mods", "add", "bob.test", "--yes"]);
    carol.ok(&["sync", "alice.test/meta"]);
    let f = carol.file("p.md", "---\ntitle: From carol\n---\nHello.\n");
    let post = carol.json(&["post", s(&f), "--yes"]);
    let hidden = post["state"]["uri"].as_str().unwrap().to_owned();
    bob.ok(&["sync", "alice.test/meta"]);
    let h = bob.json(&["mod", "hide", &hidden, "--yes"]);
    assert_eq!(h["state"]["kind"], "done", "{h}");
    alice.ok(&["sync", "alice.test/meta"]);
    let h = alice.json(&[
        "mod",
        "hide",
        &hidden,
        "--until",
        "2001-01-01T00:00:00.000Z",
        "--yes",
    ]);
    assert_eq!(h["state"]["kind"], "done", "{h}");

    let check = |args: &[&str]| {
        let out = alice.run(args);
        assert!(out.status.success(), "{}", fakenet::text(&out.stderr));
        let err = fakenet::text(&out.stderr);
        assert!(err.contains("adopted 0 of 1 assertion"), "{err}");
        let line = err
            .lines()
            .find(|l| l.contains(&hidden))
            .unwrap_or_else(|| panic!("the hide is not reported: {err}"));
        assert!(line.contains("kept own assertion"), "{line}");
        assert!(!line.contains("rank or role"), "{line}");
    };
    check(&["mod", "adopt", "bob.test", "--yes"]);
    // the adopt write's report also travels with `mod adopt --json`
    let j = alice.json(&["mod", "adopt", "bob.test", "--yes"]);
    let report = j["state"]["report"].as_array().cloned().unwrap_or_default();
    assert!(
        report.iter().any(|l| l
            .as_str()
            .is_some_and(|l| l.starts_with("kept own assertion for "))),
        "{j}"
    );
    check(&["mods", "rm", "bob.test", "--yes"]);
}

/// An edit suspends the positive assertions on the item's current version (`social.md` §4.5,
/// `threat-model.md` SOC-3: a keep, a frontpage or curated tier, a pin stop until an authority
/// asserts them again), so the author is told first: off a terminal the edit needs `--yes`
/// (sec-7). An item without one edits as before.
#[test]
fn editing_a_kept_item_says_what_it_suspends_first() {
    let net = Net::new();
    let alice = net.user("alice.test", true);
    let carol = net.user("carol.test", true);
    alice.json(&["forum", "create", "meta"]);
    carol.json(&["join", "alice.test/meta", "--yes"]);
    alice.json(&["approve", "carol.test", "--yes"]);
    carol.ok(&["sync", "alice.test/meta"]);
    let post = |name: &str| {
        let f = carol.file(name, "---\ntitle: From carol\n---\nAs it was.\n");
        let p = carol.json(&["post", s(&f), "--yes"]);
        p["state"]["uri"].as_str().unwrap().to_owned()
    };
    let (kept, plain) = (post("kept.md"), post("plain.md"));
    alice.ok(&["sync", "alice.test/meta"]);
    for verb in ["keep", "curate"] {
        let k = alice.json(&["mod", verb, &kept, "--yes"]);
        assert_eq!(k["state"]["kind"], "done", "{verb}: {k}");
    }
    // carol's view has them (an edit syncs the forum only when it was not synced lately)
    carol.ok(&["sync", "alice.test/meta"]);
    let body = || {
        let rkey = kept.rsplit('/').next().unwrap();
        carol.account.get(nsid::POST, rkey).unwrap()["body"]["text"].clone()
    };

    let e = carol.fails(2, &["edit", &kept, "-m", "As it is now."]);
    let msg = e["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("suspends a moderator's keep and its frontpage or curated tier"),
        "{e}"
    );
    assert!(msg.contains("--yes"), "{e}");
    assert_eq!(body(), json!("As it was.\n"), "nothing was sent");

    let out = carol.run(&["edit", &kept, "-m", "As it is now.", "--yes"]);
    assert!(out.status.success(), "{}", fakenet::text(&out.stderr));
    let err = fakenet::text(&out.stderr);
    assert!(err.contains("suspends a moderator's keep"), "{err}");
    assert_eq!(body(), json!("As it is now.\n"));

    // nothing positive on it: no warning, no --yes
    let out = carol.run(&["edit", &plain, "-m", "Changed."]);
    assert!(out.status.success(), "{}", fakenet::text(&out.stderr));
    assert!(!fakenet::text(&out.stderr).contains("suspends"));
}
