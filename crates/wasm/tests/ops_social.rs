// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 operations through the wasm dispatcher (`ops::call`), over the fake network
//! (`MemHost` + `FakePds`): every argument is the JSON `@lessdugin/engine` sends for its
//! `EngineApi` method (optional request fields left out, as a TypeScript caller may), and every
//! result is read back as the dto/1 JSON the SPA receives.
//!
//! alice owns an approval-mode forum, approves bob and carol, and posts; bob comments; carol
//! acknowledges the public-action interstitial, then votes, agrees and reacts; alice hides
//! bob's comment and pins her post (the social subset of the parity scenario, `docs/plan.md`
//! §4.2), then reads the mod log and queue, adds carol as a moderator and updates the forum.

use futures::executor::block_on;
use lessdugin_core::engine::{Engine, EngineConfig, EngineError, ErrorCode};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost};
use lessdugin_wasm::ops;
use serde_json::{Value, json};

const MINUTE: i64 = 60_000;

struct Client {
    host: MemHost,
    engine: Engine<MemHost>,
    did: String,
}

impl Client {
    fn new(host: MemHost, acct: &AccountHandle) -> Self {
        let engine = Engine::new(host.clone(), EngineConfig::default());
        let did = acct.did().to_string();
        let c = Self { host, engine, did };
        let view = c.call(
            "addAccount",
            json!([{ "did": c.did, "pds": acct.pds_base(), "auth": "appPassword" }]),
        );
        assert_eq!(view["active"], true, "{view}");
        c
    }

    fn try_call(&self, op: &str, args: Value) -> Result<Value, EngineError> {
        let text = block_on(ops::call(&self.engine, op, &args.to_string()))?;
        Ok(serde_json::from_str(&text).expect("dto/1 JSON"))
    }

    fn call(&self, op: &str, args: Value) -> Value {
        self.try_call(op, args)
            .unwrap_or_else(|e| panic!("{op}: {e:?}"))
    }

    fn err(&self, op: &str, args: Value) -> EngineError {
        self.try_call(op, args).expect_err(op)
    }

    /// Sends every pending write (sleeping through the undo window); all must be done.
    fn flush(&self) {
        let r = self.call("outboxFlush", json!([]));
        assert_eq!(
            (r["failed"].as_u64(), r["conflicts"].as_u64()),
            (Some(0), Some(0)),
            "{r}"
        );
    }

    /// The uri a done write created.
    fn done(&self, ticket: &Value) -> String {
        let t = self.call("writeStatus", json!([ticket["id"]]));
        assert_eq!(t["state"]["kind"], "done", "{t}");
        t["state"]["uri"].as_str().expect("uri").to_owned()
    }

    fn sync(&self, forum: &str) {
        let r = self.call("syncForum", json!([forum, {}]));
        assert_eq!(r["partial"], false, "{r}");
    }
}

#[test]
fn the_social_operations_round_trip_through_the_dispatcher() {
    let ha = MemHost::new();
    let net = FakeNet::on(&ha);
    let _ = net.default_pds();
    let (hb, hc) = (MemHost::new(), MemHost::new());
    hb.set_seed(2);
    hc.set_seed(3);
    net.install_as(&hb, "bob");
    net.install_as(&hc, "carol");
    let alice = Client::new(ha, &net.add_account("alice.example.com"));
    let bob = Client::new(hb, &net.add_account("bob.example.com"));
    let carol = Client::new(hc, &net.add_account("carol.example.com"));
    let all = [&alice, &bob, &carol];
    let tick = |ms: i64| {
        for c in all {
            c.host.advance_ms(ms);
        }
    };

    // alice creates an approval-mode forum (featuredTags and moderators left out)
    let t = alice.call(
        "forumCreate",
        json!([{ "slug": "meta", "name": "Meta", "membership": "approval",
                 "defaultTier": "personal" }]),
    );
    alice.flush();
    let forum = alice.done(&t);

    // alice approves bob and carol (applicants are found by discovery, which the fake network
    // leaves out: she grants them up front) and dismisses someone else on this device
    let tickets = alice.call("approve", json!([forum, [bob.did, carol.did]]));
    assert_eq!(tickets.as_array().map(Vec::len), Some(2), "{tickets}");
    alice.flush();
    const STRANGER: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
    assert_eq!(alice.call("reject", json!([forum, [STRANGER]])), json!([]));
    assert_eq!(
        alice.call("rejectedApplicants", json!([forum])),
        json!([STRANGER])
    );
    tick(MINUTE);
    for c in [&bob, &carol] {
        c.call("forumOpen", json!([forum]));
        // trailing optional arguments arrive as `null` (client.ts `padArgs`)
        c.call("join", json!([forum, null]));
        c.flush();
    }
    tick(MINUTE);
    alice.sync(&forum);
    let members = alice.call("members", json!([forum, null]));
    let granted: Vec<(&str, &Value)> = members
        .as_array()
        .expect("members")
        .iter()
        .filter(|m| m["granted"] == true)
        .map(|m| (m["actor"]["did"].as_str().expect("did"), &m["status"]))
        .collect();
    assert_eq!(granted.len(), 2, "{members}");
    assert_eq!(
        alice.call("members", json!([forum, { "pending": true }])),
        json!([])
    );

    // alice posts (tags, labels and media left out); bob comments
    tick(MINUTE);
    let t = alice.call(
        "publish",
        json!([{ "forum": forum, "kind": "article", "title": "Welcome",
                 "body": { "markdown": "Hello **everyone**." } }]),
    );
    alice.flush();
    let post = alice.done(&t);
    tick(MINUTE);
    bob.sync(&forum);
    let t = bob.call(
        "comment",
        json!([{ "root": post, "body": { "markdown": "Thanks!" } }]),
    );
    bob.flush();
    let reply = bob.done(&t);

    // carol: the "this is public" interstitial first, then a vote, an agreement and a react
    tick(MINUTE);
    carol.sync(&forum);
    assert_eq!(
        carol.call("acknowledgedPublicActions", json!([])),
        json!([])
    );
    assert_eq!(
        carol.call("publicActionAcknowledged", json!(["vote"])),
        json!(false)
    );
    carol.call("acknowledgePublicAction", json!(["vote"]));
    carol.call("acknowledgePublicAction", json!(["react"]));
    assert_eq!(
        carol.call("acknowledgedPublicActions", json!([])),
        json!(["vote", "react"])
    );
    let caps = carol.call("forumOpen", json!([forum]))["viewer"].clone();
    assert_eq!(
        (&caps["vote"], &caps["react"]),
        (&json!(true), &json!(true)),
        "{caps}"
    );
    let t = carol.call("opinion", json!([post, { "kind": "karma", "value": "up" }]));
    assert_eq!(t["state"]["kind"], "pending", "{t}");
    assert_eq!(t["state"]["held"], true, "the undo window: {t}");
    assert!(t.get("notices").is_none(), "carol is a seed: {t}");
    carol.call(
        "opinion",
        json!([post, { "kind": "agreement", "value": "agree" }]),
    );
    carol.call(
        "opinion",
        json!([post, { "kind": "react", "id": "insightful", "stance": "for" }]),
    );
    carol.flush();
    let e = carol.err(
        "opinion",
        json!([post, { "kind": "karma", "value": "sideways" }]),
    );
    assert_eq!(e.code, ErrorCode::InvalidInput, "{e:?}");

    // alice hides bob's comment (spam and showReason left out) and pins her post
    tick(MINUTE);
    alice.sync(&forum);
    let t = alice.call(
        "modAction",
        json!([{ "kind": "assert", "forum": forum, "scope": "forum", "subject": reply,
                 "action": { "kind": "hide", "hidden": true }, "reason": "off topic" }]),
    );
    assert_eq!(t["state"]["kind"], "pending", "{t}");
    alice.call(
        "modAction",
        json!([{ "kind": "assert", "forum": forum, "scope": "forum", "subject": post,
                 "action": { "kind": "pin", "scope": "forum", "pinned": true } }]),
    );
    alice.flush();
    let e = carol.err("modQueue", json!([forum]));
    assert_eq!(e.code, ErrorCode::Blocked, "{e:?}");
    let queue = alice.call("modQueue", json!([forum]));
    assert_eq!(queue["dto"], 1, "{queue}");

    // everyone sees the same thread: the post pinned and scored, the comment hidden
    tick(MINUTE);
    for c in all {
        c.sync(&forum);
    }
    let log = carol.call("modLog", json!([forum, null, 1]));
    assert_eq!(log["entries"].as_array().map(Vec::len), Some(1), "{log}");
    assert!(log["cursor"].is_string(), "a second page: {log}");
    let page2 = carol.call("modLog", json!([forum, log["cursor"], 10]));
    let mut actions: Vec<&str> = log["entries"]
        .as_array()
        .into_iter()
        .chain(page2["entries"].as_array())
        .flatten()
        .map(|e| e["action"].as_str().expect("action"))
        .collect();
    actions.sort_unstable();
    assert!(
        actions.contains(&"hide") && actions.contains(&"pin"),
        "{actions:?}"
    );
    let list = carol.call(
        "listPosts",
        json!([{ "forum": forum, "sort": "magic", "filter": "all", "limit": 10 }]),
    );
    let item = &list["items"][0];
    assert_eq!(item["uri"], post.as_str(), "{list}");
    assert_eq!(item["pinned"], true, "{item}");
    assert_eq!(item["score"]["mine"]["karma"], "up", "{item}");
    assert_eq!(item["score"]["mine"]["agreement"], "agree", "{item}");
    let thread = carol.call("getThread", json!([post, { "sort": "magic" }]));
    let reacts = &thread["post"]["reacts"];
    assert_eq!(reacts[0]["react"], "insightful", "{reacts}");
    assert_eq!(reacts[0]["mine"], "for", "{reacts}");
    let node = thread["comments"]
        .as_array()
        .expect("comments")
        .iter()
        .find(|n| n["uri"] == reply.as_str());
    if let Some(node) = node {
        assert_ne!(node["state"]["kind"], "visible", "{node}");
    }
    let raw = carol.call("raw", json!([reply]));
    assert!(
        raw["assertions"].as_array().is_some_and(|a| !a.is_empty()),
        "{raw}"
    );
    assert_eq!(raw["state"]["kind"], "hidden", "{raw}");
    assert_eq!(raw["state"]["reason"]["kind"], "removed", "{raw}");

    // notifications and unread state (only `limit` given)
    let n = bob.call("notifications", json!([{ "limit": 20 }]));
    assert_eq!(n["dto"], 1, "{n}");
    let unread = n["unread"].as_u64().expect("unread");
    let counts = bob.call("unreadCounts", json!([]));
    assert_eq!(counts["notifications"].as_u64(), Some(unread), "{counts}");
    bob.call("markAllNotificationsRead", json!([]));
    assert_eq!(
        bob.call("notifications", json!([{ "limit": 20 }]))["unread"],
        0
    );
    let ids: Vec<Value> = bob.call("notifications", json!([{ "all": true, "limit": 20 }]))["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    bob.call("markNotificationsRead", json!([ids]));
    bob.call("markRead", json!([post]));
    bob.call("markRead", json!([forum]));

    // local lists
    carol.call("bookmark", json!([post, true]));
    assert_eq!(carol.call("bookmarked", json!([post])), json!(true));
    assert_eq!(carol.call("bookmarks", json!([]))[0]["uri"], post.as_str());
    carol.call(
        "setSubscription",
        json!([{ "kind": "post", "uri": post }, true]),
    );
    assert_eq!(
        carol.call("subscriptions", json!([]))[0]["subscription"],
        json!({ "kind": "post", "uri": post })
    );
    carol.call("mute", json!([bob.did, true]));
    assert_eq!(carol.call("mutes", json!([]))[0]["did"], bob.did.as_str());
    carol.call("mute", json!([bob.did, false]));
    assert_eq!(carol.call("mutes", json!([])), json!([]));
    assert_eq!(carol.call("karmaNotifications", json!([])), json!("daily"));
    carol.call("setKarmaNotifications", json!(["weekly"]));
    assert_eq!(carol.call("karmaNotifications", json!([])), json!("weekly"));

    // alice adds carol as a moderator and renames the forum (moderators left out: kept)
    let t = alice.call(
        "modsSet",
        json!([forum, { "kind": "add", "did": carol.did, "role": "moderator" }]),
    );
    assert!(t["id"].is_string(), "{t}");
    alice.flush();
    alice.sync(&forum);
    let t = alice.call(
        "forumUpdate",
        json!([forum, { "slug": "meta", "name": "Meta (renamed)", "membership": "approval",
                        "defaultTier": "personal" }]),
    );
    alice.flush();
    alice.done(&t);
    tick(MINUTE);
    carol.sync(&forum);
    let f = carol.call("forumOpen", json!([forum]));
    assert_eq!(f["forum"]["name"], "Meta (renamed)", "{f}");
    assert_eq!(f["myRole"], "moderator", "{f}");
    assert_eq!(f["viewer"]["moderate"], true, "{f}");
    assert_eq!(carol.call("outboxExpedite", json!([])), json!(0));
}

#[test]
fn writes_and_acknowledgements_need_an_account() {
    let e = Engine::new(MemHost::new(), EngineConfig::default());
    let forum = format!("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{}/meta", nsid::FORUM);
    let post = format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{}/3l2s5xxv2zaa2",
        nsid::POST
    );
    for (op, args) in [
        ("opinion", json!([post, { "kind": "karma" }])),
        (
            "modAction",
            json!([{ "kind": "remove", "forum": forum, "scope": "forum", "subject": post,
                     "actionKey": "hide" }]),
        ),
        ("modAdopt", json!([forum, "did:plc:abc"])),
        (
            "modsSet",
            json!([forum, { "kind": "remove", "did": "did:plc:abc" }]),
        ),
        (
            "forumUpdate",
            json!([forum, { "slug": "meta", "name": "M", "membership": "open",
                            "defaultTier": "personal" }]),
        ),
        ("acknowledgePublicAction", json!(["modAction"])),
    ] {
        let err = block_on(ops::call(&e, op, &args.to_string())).expect_err(op);
        assert_eq!(err.code, ErrorCode::AuthRequired, "{op}: {err:?}");
    }
    // without an account: nothing to notify, nothing acknowledged
    let call = |op: &str, args: Value| {
        block_on(ops::call(&e, op, &args.to_string())).unwrap_or_else(|e| panic!("{op}: {e:?}"))
    };
    assert_eq!(
        call("notifications", json!([{ "limit": 5 }])),
        r#"{"dto":1,"items":[],"unread":0}"#
    );
    assert_eq!(
        call("unreadCounts", json!([])),
        r#"{"dto":1,"notifications":0,"forums":[]}"#
    );
    assert_eq!(call("acknowledgedPublicActions", json!([])), "[]");
    assert_eq!(call("markAllNotificationsRead", json!([])), "null");
    assert_eq!(call("bookmarks", json!([])), "[]");
    assert_eq!(call("subscriptions", json!([])), "[]");
    assert_eq!(call("mutes", json!([])), "[]");
}

#[test]
fn bad_social_arguments_are_invalid_input() {
    let e = Engine::new(MemHost::new(), EngineConfig::default());
    for (op, args) in [
        ("opinion", json!(["at://x", { "kind": "vote" }])),
        ("modAction", json!([{ "kind": "assert" }])),
        (
            "modsSet",
            json!(["at://x", { "kind": "add", "did": "did:plc:x" }]),
        ),
        ("modLog", json!([])),
        ("markNotificationsRead", json!(["id"])),
        ("publicActionAcknowledged", json!(["wave"])),
        ("setKarmaNotifications", json!(["hourly"])),
        ("setSubscription", json!([{ "kind": "forum" }, true])),
        ("approve", json!(["at://x", "did:plc:x"])),
    ] {
        let err = block_on(ops::call(&e, op, &args.to_string())).expect_err(op);
        assert_eq!(err.code, ErrorCode::InvalidInput, "{op}: {err:?}");
    }
}
