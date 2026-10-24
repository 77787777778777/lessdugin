// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 acceptance checks of `docs/plan.md` §M3 at the CLI level:
//!
//! - a post lock is refused above 5,000 allowed comments (the moderator is told to hide
//!   instead; the engine's own case is `a_lock_keeps_the_visible_comments_and_is_refused_
//!   above_5000` in `crates/core/tests/engine_actions.rs` — this one drives it through
//!   `lessdugin mod lock`);
//! - `mods rm` runs `mod adopt` first: the departing moderator's assertions are re-issued
//!   under the remover (with `adopts` pointing at each original) before the declaration
//!   edits, and the ticket says so (`adoptedFirst` notice).
//!
//! Both run on the FakePds network (`support/fakenet.rs`) with the real CLI binary.

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
        let body = serde_json::to_value(Md::Known(Markdown::inline("A reply."))).unwrap();
        let v = json!({
            "$type": nsid::COMMENT,
            "root": {"uri": root.0, "cid": root.1},
            "forum": f.uri.as_str(),
            "body": body,
            "createdAt": Datetime::from_millis(self.base_ms + 10_000 + i as i64)
                .unwrap()
                .as_str(),
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
    let e = alice.fails(1, &["mod", "lock", &post.0]);
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
        u.json(&["join", "alice.test/meta"]);
    }
    let a = alice.json(&["approve", "bob.test", "carol.test"]);
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
    let h = bob.json(&["mod", "hide", &post_uri, "--reason", "testing"]);
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
