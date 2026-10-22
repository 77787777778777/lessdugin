// SPDX-License-Identifier: MIT OR Apache-2.0
//! Record validity (`data-model.md` §7.2–§7.3): at least one positive and one negative case per
//! rule ID. Entry rules are exercised through the `validity::entry` helpers (the record stays
//! valid either way); the fold-only rules are checked to be left alone by `check`.

mod common;

// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

use common::{DAG_CID, RAW_CID, nsid};
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::{
    Forum, LockAction, Opinion, Post, RestrictAction, Sequence, WikiRevision,
};
use lessdugin_lexicon::validity::{Evaluation, RuleId, Validity, check, check_uri, entry};
use lessdugin_lexicon::values::{Capability, Role, Stance, VoteValue};
use serde_json::{Value, json};

const OWNER: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
const ALICE: &str = "did:plc:abcdefghijklmnopqrstuvwx";
const BOB: &str = "did:plc:7iza6de2dwap2sbkpav7c6c6";
const T1: &str = "3l2s5xxv2ze2c";
const T2: &str = "3l2s6aaaaaaa2";
const T3: &str = "3l2s7bbbbbbb2";
const T4: &str = "3lz7rb6tpt22a";
const NOW: &str = "2026-10-16T12:00:00.000Z";

fn at(did: &str, coll: &str, rkey: &str) -> String {
    format!("at://{did}/{}/{rkey}", nsid(coll))
}

fn forum_uri() -> String {
    at(OWNER, "forum", "rationality")
}

fn other_forum_uri() -> String {
    at(OWNER, "forum", "meta")
}

fn fk() -> String {
    keys::fk(&forum_uri()).unwrap()
}

fn post_uri() -> String {
    at(ALICE, "post", &format!("{}-{T1}", fk()))
}

fn comment_uri() -> String {
    at(BOB, "comment", &format!("{}-{T1}-{T2}", fk()))
}

fn wiki_uri() -> String {
    at(ALICE, "wikiRevision", &format!("{}-{T3}", fk()))
}

fn md(text: &str) -> Value {
    json!({"$type": nsid("markdown"), "text": text})
}

fn md_blob() -> Value {
    json!({"$type": nsid("markdown"), "textBlob": blob("text/markdown", 70000)})
}

fn blob(mime: &str, size: u64) -> Value {
    json!({"$type": "blob", "ref": {"$link": RAW_CID}, "mimeType": mime, "size": size})
}

fn sref(uri: &str) -> Value {
    json!({"uri": uri, "cid": DAG_CID})
}

/// A record under test: collection, at-uri and value.
#[derive(Clone)]
struct R {
    coll: Collection,
    uri: String,
    v: Value,
}

impl R {
    fn check(&self) -> Validity {
        let uri = AtUri::parse(&self.uri).unwrap_or_else(|e| panic!("{}: {e}", self.uri));
        let r = check(self.coll.nsid(), &uri, &self.v);
        assert_eq!(r, check_uri(&uri, &self.v));
        r
    }

    #[track_caller]
    fn valid(&self) -> Validity {
        let r = self.check();
        assert!(r.valid, "expected valid: {:#?}\n{:#}", r.violations, self.v);
        r
    }

    #[track_caller]
    fn fails(&self, rule: RuleId) -> Validity {
        let r = self.check();
        assert!(!r.valid, "expected {rule}: valid\n{:#}", self.v);
        assert!(r.fails(rule), "expected {rule}, got {:#?}", r.violations);
        r
    }

    #[track_caller]
    fn passes(&self, rule: RuleId) -> Validity {
        let r = self.check();
        assert!(!r.fails(rule), "{rule} failed: {:#?}", r.violations);
        r
    }

    fn edit(&self, path: &[&str], value: Option<Value>) -> Self {
        let mut c = self.clone();
        let mut cur = &mut c.v;
        for k in &path[..path.len() - 1] {
            cur = match k.parse::<usize>() {
                Ok(i) => &mut cur[i],
                Err(_) => &mut cur[*k],
            };
        }
        let last = path[path.len() - 1];
        match value {
            Some(v) => cur[last] = v,
            None => {
                cur.as_object_mut().unwrap().remove(last);
            }
        }
        c
    }

    fn with(&self, path: &[&str], value: Value) -> Self {
        self.edit(path, Some(value))
    }

    fn without(&self, path: &[&str]) -> Self {
        self.edit(path, None)
    }

    fn at_rkey(&self, rkey: &str) -> Self {
        let mut c = self.clone();
        let (base, _) = c.uri.rsplit_once('/').unwrap();
        c.uri = format!("{base}/{rkey}");
        c
    }

    fn at_did(&self, did: &str) -> Self {
        let mut c = self.clone();
        let rest = c.uri.strip_prefix("at://").unwrap();
        let (_, path) = rest.split_once('/').unwrap();
        c.uri = format!("at://{did}/{path}");
        c
    }
}

fn forum() -> R {
    R {
        coll: Collection::Forum,
        uri: forum_uri(),
        v: json!({"$type": nsid("forum"), "name": "Rationality", "membership": "approval", "createdAt": NOW}),
    }
}

fn membership() -> R {
    R {
        coll: Collection::Membership,
        uri: at(ALICE, "membership", &fk()),
        v: json!({"$type": nsid("membership"), "forum": forum_uri(), "createdAt": NOW}),
    }
}

fn post() -> R {
    R {
        coll: Collection::Post,
        uri: post_uri(),
        v: json!({"$type": nsid("post"), "forum": forum_uri(), "title": "Hello", "body": md("Hi"), "createdAt": NOW}),
    }
}

fn comment() -> R {
    R {
        coll: Collection::Comment,
        uri: comment_uri(),
        v: json!({"$type": nsid("comment"), "root": sref(&post_uri()), "forum": forum_uri(), "body": md("Yes"), "createdAt": NOW}),
    }
}

/// A revision of `post()` in Alice's repo, with the correct snapshot CID.
fn revision() -> R {
    let snapshot = post().v;
    let cid = lessdugin_atp::datamodel::record_cid(&snapshot).unwrap();
    R {
        coll: Collection::Revision,
        uri: at(ALICE, "revision", &format!("{}-{T1}-{T3}", fk())),
        v: json!({"$type": nsid("revision"), "subject": {"uri": post_uri(), "cid": cid.to_string()},
                  "op": "update", "record": snapshot, "createdAt": NOW}),
    }
}

fn opinion_on(subject: &str) -> R {
    R {
        coll: Collection::Opinion,
        uri: at(
            BOB,
            "opinion",
            &keys::opinion_rkey(NS, &forum_uri(), subject).unwrap(),
        ),
        v: json!({"$type": nsid("opinion"), "subject": sref(subject), "forum": forum_uri(), "karma": "up", "createdAt": NOW}),
    }
}

fn opinion() -> R {
    opinion_on(&post_uri())
}

fn action(name: &str, fields: Value) -> Value {
    let mut a = json!({"$type": format!("{}#{name}", nsid("modAction"))});
    for (k, v) in fields.as_object().unwrap() {
        a[k] = v.clone();
    }
    a
}

/// A modAction by the owner with its derived key.
fn mod_action(scope: Option<&str>, subject: &str, act: Value) -> R {
    let rkey =
        keys::mod_action_rkey(NS, &forum_uri(), scope, subject, act.as_object().unwrap()).unwrap();
    let mut v = json!({"$type": nsid("modAction"), "forum": forum_uri(), "subject": subject, "action": act, "createdAt": NOW});
    if let Some(s) = scope {
        v["scope"] = json!(s);
    }
    R {
        coll: Collection::ModAction,
        uri: at(OWNER, "modAction", &rkey),
        v,
    }
}

fn hide() -> R {
    mod_action(None, &post_uri(), action("hide", json!({})))
}

fn wiki() -> R {
    R {
        coll: Collection::WikiRevision,
        uri: wiki_uri(),
        v: json!({"$type": nsid("wikiRevision"), "forum": forum_uri(), "slug": "decision-theory", "name": "Decision theory", "createdAt": NOW}),
    }
}

fn sequence() -> R {
    R {
        coll: Collection::Sequence,
        uri: at(ALICE, "sequence", &format!("{}-{T4}", fk())),
        v: json!({"$type": nsid("sequence"), "forum": forum_uri(), "title": "Intro",
                  "chapters": [{"title": "One", "posts": [post_uri()]}], "createdAt": NOW}),
    }
}

fn profile() -> R {
    R {
        coll: Collection::Profile,
        uri: at(ALICE, "profile", "self"),
        v: json!({"$type": nsid("profile"), "displayName": "Alice"}),
    }
}

fn all_valid() -> Vec<R> {
    vec![
        forum(),
        membership(),
        post(),
        comment(),
        revision(),
        opinion(),
        hide(),
        wiki(),
        sequence(),
        profile(),
    ]
}

#[test]
fn baseline_records_are_valid() {
    for r in all_valid() {
        let v = r.valid();
        assert!(v.no_effect.is_empty(), "{:?}", v.no_effect);
        assert!(v.is_effective());
        assert_eq!(v.first_rule(), None);
    }
}

#[test]
fn rule_ids() {
    assert_eq!(RuleId::ALL.len(), 58);
    for r in RuleId::ALL {
        assert_eq!(RuleId::parse(r.as_str()), Some(*r));
        assert!(!r.text().is_empty());
    }
    assert_eq!(RuleId::G10.as_str(), "G10");
    assert_eq!(RuleId::MA12.to_string(), "MA12");
    assert!(RuleId::G10 < RuleId::FO1);
    assert_eq!(RuleId::CO5.evaluation(), Evaluation::Fold);
    assert_eq!(RuleId::OP5.evaluation(), Evaluation::Fold);
    assert_eq!(RuleId::PO2.evaluation(), Evaluation::Definition);
    assert_eq!(RuleId::FO2.evaluation(), Evaluation::Entry);
    assert_eq!(RuleId::G4.evaluation(), Evaluation::Here);
    assert_eq!(RuleId::parse("G11"), None);
}

#[test]
fn violations_are_ordered_by_rule() {
    // a post with a bad key, a missing title and a non-slug tag
    let r = post()
        .at_rkey(&format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()))
        .without(&["title"])
        .with(&["tags"], json!(["Not A Slug"]));
    let v = r.fails(RuleId::G5);
    assert_eq!(v.first_rule(), Some(RuleId::G5));
    assert_eq!(
        v.rule_ids(),
        vec![RuleId::G5, RuleId::G8, RuleId::G9, RuleId::PO1, RuleId::PO3]
    );
}

// ---------------------------------------------------------------------------------------------
// Generic rules

#[test]
fn g1_type_equals_collection() {
    post().valid();
    post()
        .with(&["$type"], json!(nsid("comment")))
        .fails(RuleId::G1);
    post().without(&["$type"]).fails(RuleId::G1);
    post().with(&["$type"], json!(7)).fails(RuleId::G1);
    // the other namespace prefix is not this build's collection
    let other = if NS == lessdugin_lexicon::ns::NS_TEMP {
        lessdugin_lexicon::ns::NS_FINAL
    } else {
        lessdugin_lexicon::ns::NS_TEMP
    };
    let foreign = format!("{other}.post");
    let uri = AtUri::parse(&format!("at://{ALICE}/{foreign}/x")).unwrap();
    let r = check(&foreign, &uri, &json!({"$type": foreign}));
    assert_eq!(r.first_rule(), Some(RuleId::G1));
    let r = check("app.bsky.feed.post", &uri, &json!({}));
    assert!(r.fails(RuleId::G1));
    // the collection argument must be the uri's
    let uri = AtUri::parse(&post_uri()).unwrap();
    assert!(check(&nsid("comment"), &uri, &comment().v).fails(RuleId::G1));
}

#[test]
fn g2_lexicon_validation() {
    post().valid();
    post()
        .with(&["title"], json!("t".repeat(3001)))
        .fails(RuleId::G2);
    post()
        .with(&["title"], json!("é".repeat(301)))
        .fails(RuleId::G2); // 301 graphemes
    post().without(&["createdAt"]).fails(RuleId::G2);
    post()
        .with(&["createdAt"], json!("yesterday"))
        .fails(RuleId::G2);
    post().with(&["tags"], json!("a")).fails(RuleId::G2);
    post()
        .with(&["link"], json!({"uri": "not a uri"}))
        .fails(RuleId::G2);
    // `uri` follows RFC 3986: case-insensitive schemes with digits and "+"; ASCII only
    for ok in [
        "HTTPS://example.com/a",
        "git+ssh://git@example.com/r.git",
        "h323:alice@example.com",
    ] {
        post()
            .with(&["kind"], json!("link"))
            .with(&["link"], json!({ "uri": ok }))
            .valid();
        forum().with(&["reportContact"], json!(ok)).valid();
    }
    forum()
        .with(&["reportContact"], json!("https://例え.jp/"))
        .fails(RuleId::G2);
    // knownValues are not enforced
    post().with(&["kind"], json!("poll")).valid();
    // unknown fields are ignored by the lexicon, but must still be data model (no floats)
    post().with(&["zz"], json!({"a": [1, "b"]})).valid();
    post().with(&["zz"], json!(1.5)).fails(RuleId::G2);
    // sync.md §5.5, anywhere in the value: no fraction or exponent, within ±(2^53 − 1)
    for (zz, path) in [
        (json!(3.0), "$.zz"),
        (json!({"a": [1, 1e3]}), "$.zz.a[1]"),
        (json!([9_007_199_254_740_992_u64]), "$.zz[0]"),
        (json!({"n": -9_007_199_254_740_992_i64}), "$.zz.n"),
    ] {
        let v = post().with(&["zz"], zz).fails(RuleId::G2);
        assert_eq!(v.violations.len(), 1, "{:?}", v.violations);
        assert_eq!(v.violations[0].path, path);
    }
    post()
        .with(
            &["zz"],
            json!([9_007_199_254_740_991_u64, -9_007_199_254_740_991_i64]),
        )
        .valid();
    // a schema integer is reported once, by the lexicon walk
    let v = forum()
        .with(&["policy"], json!({"karma": {"pairCap": 3.0}}))
        .fails(RuleId::G2);
    assert_eq!(v.violations.len(), 1, "{:?}", v.violations);
    post()
        .with(&["zz"], json!({"$link": "nope"}))
        .fails(RuleId::G2);
    post().with(&["zz"], json!({"$type": ""})).fails(RuleId::G2);
    // a non-object value
    let uri = AtUri::parse(&post_uri()).unwrap();
    assert_eq!(
        check(&nsid("post"), &uri, &json!([1])).first_rule(),
        Some(RuleId::G2)
    );
    // known union members are validated, unknown ones left alone
    post().with(&["body", "text"], json!(1)).fails(RuleId::G2);
    post()
        .with(
            &["body"],
            json!({"$type": "com.example.richtext", "text": 1}),
        )
        .valid();
    // blob accept and maxSize against the declared mimeType and size
    forum()
        .with(&["icon"], blob("image/png", 2_000_000))
        .valid();
    forum()
        .with(&["icon"], blob("video/mp4", 10))
        .fails(RuleId::G2);
    forum()
        .with(&["icon"], blob("image/png", 2_000_001))
        .fails(RuleId::G2);
    // a blob's ref must be a raw-codec CID (data model)
    forum()
        .with(
            &["icon"],
            json!({"$type": "blob", "ref": {"$link": DAG_CID}, "mimeType": "image/png", "size": 1}),
        )
        .fails(RuleId::G2);
}

#[test]
fn g2_interop_data_model_fixtures() {
    // testdata/interop/data-model: valid values stay valid inside an unknown field; invalid
    // ones (floats, bad $type, malformed blobs, $bytes and $link objects) fail G2
    let load = |name: &str| -> Vec<Value> {
        serde_json::from_str(&common::text(&format!(
            "testdata/interop/data-model/{name}"
        )))
        .unwrap()
    };
    let valid = load("data-model-valid.json");
    let invalid = load("data-model-invalid.json");
    assert!(valid.len() >= 5 && invalid.len() >= 10);
    for f in valid {
        let j = f["json"].clone();
        if f["note"] == "float, but integer-like" {
            // deliberate deviation from upstream: sync.md §5.5 rejects any number with a
            // fraction or exponent (`123.0`), even where atproto reads it as an integer
            post().with(&["zz"], j).fails(RuleId::G2);
            continue;
        }
        post().with(&["zz"], j).valid();
    }
    for f in invalid {
        let j = f["json"].clone();
        if j.is_object() {
            post().with(&["zz"], j).fails(RuleId::G2);
        } else {
            // "top-level not an object": the record value itself
            let mut r = post();
            r.v = j;
            r.fails(RuleId::G2);
        }
    }
}

#[test]
fn g3_typed_blobs() {
    forum()
        .with(&["icon"], blob("image/png", 10))
        .passes(RuleId::G3);
    forum()
        .with(&["icon"], json!({"cid": RAW_CID, "mimeType": "image/png"}))
        .fails(RuleId::G3);
    post()
        .with(
            &["body", "media"],
            json!([{"blob": {"cid": RAW_CID, "mimeType": "image/png"}}]),
        )
        .fails(RuleId::G3);
}

#[test]
fn g4_canonical_references() {
    membership().valid();
    // handle authority, did:key, query, other collection, non-slug forum rkey
    for bad in [
        "at://alice.example.com/".to_owned() + &nsid("forum") + "/rationality",
        format!(
            "at://did:key:zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA/{}/rationality",
            nsid("forum")
        ),
        format!("{}?x=1", forum_uri()),
        at(OWNER, "post", "rationality"),
        at(OWNER, "forum", "Rationality"),
        format!(
            "at://did:web:example.com:8080/{}/rationality",
            nsid("forum")
        ),
    ] {
        membership().with(&["forum"], json!(bad)).fails(RuleId::G4);
    }
    membership()
        .with(
            &["forum"],
            json!(format!(
                "at://did:web:forum.example.com/{}/general",
                nsid("forum")
            )),
        )
        .at_rkey(
            &keys::fk(&format!(
                "at://did:web:forum.example.com/{}/general",
                nsid("forum")
            ))
            .unwrap(),
        )
        .valid();
    // array entries: markdown.links (any collection, canonical), profile.pinned (allowed set)
    post()
        .with(
            &["body", "links"],
            json!([
                post_uri(),
                "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/3l2s5xxv2ze2c"
            ]),
        )
        .valid();
    post()
        .with(
            &["body", "links"],
            json!(["at://alice.example.com/app.bsky.feed.post/x"]),
        )
        .fails(RuleId::G4);
    // a reference into an NS collection must match its grammar
    post()
        .with(
            &["body", "links"],
            json!([at(ALICE, "post", "not-a-post-key")]),
        )
        .fails(RuleId::G4);
    profile()
        .with(&["pinned"], json!([post_uri(), comment_uri()]))
        .valid();
    profile()
        .with(&["pinned"], json!([forum_uri()]))
        .fails(RuleId::G4);
    // the record is invalid whatever the entry position
    let r = profile()
        .with(&["pinned"], json!([post_uri(), "at://x.example/a.b.c/d"]))
        .fails(RuleId::G4);
    assert_eq!(r.violations[0].path, "$.pinned[1]");
}

#[test]
fn g5_canonical_key() {
    post().passes(RuleId::G5);
    post().at_rkey(&format!("{}-{T1}x", fk())).fails(RuleId::G5);
    comment()
        .at_rkey(&format!("{}-{T3}-{T2}", fk()))
        .fails(RuleId::G5);
    revision().at_did(BOB).fails(RuleId::G5);
}

#[test]
fn g6_record_size() {
    // unknown fields are not limited by the lexicon, only by G6
    post()
        .with(&["zz"], json!("x".repeat(990_000)))
        .passes(RuleId::G6);
    post()
        .with(&["zz"], json!("x".repeat(1_000_001)))
        .fails(RuleId::G6);
}

#[test]
fn g7_markdown_body_rules() {
    post()
        .with(&["body"], md_blob())
        .with(&["excerpt"], json!("An excerpt"))
        .valid();
    post()
        .with(&["body", "textBlob"], blob("text/markdown", 10))
        .fails(RuleId::G7);
    post().without(&["body", "text"]).fails(RuleId::G7);
    // an empty text counts as present
    post().with(&["body", "text"], json!("")).valid();
    comment().with(&["body"], md_blob()).fails(RuleId::G7);
    forum().with(&["about"], md_blob()).valid();
    wiki().with(&["body"], md_blob()).valid();
    sequence().with(&["body"], md_blob()).valid();
    // unknown members are not checked
    comment()
        .with(&["body"], json!({"$type": "com.example.md", "textBlob": 1}))
        .valid();
}

#[test]
fn g8_slugs() {
    post()
        .with(&["tags"], json!(["ai", "decision-theory", "2024"]))
        .valid();
    for bad in ["AI", "a--b", "-a", "a b", "", "é"] {
        post().with(&["tags"], json!([bad])).fails(RuleId::G8);
    }
    forum()
        .with(&["featuredTags"], json!(["ok", "Not"]))
        .fails(RuleId::G8);
    wiki()
        .with(&["slug"], json!("Decision Theory"))
        .fails(RuleId::G8);
    wiki()
        .with(&["parentTags"], json!(["x_y"]))
        .fails(RuleId::G8);
    opinion()
        .with(&["reacts"], json!([{"react": "Agree"}]))
        .fails(RuleId::G8);
    opinion()
        .with(
            &["tags"],
            json!([{"tag": "ok", "value": "up"}, {"tag": "No!", "value": "up"}]),
        )
        .fails(RuleId::G8);
    opinion()
        .with(&["reacts"], json!([{"react": "agree"}]))
        .valid();
}

#[test]
fn g9_fk_segment() {
    for r in [
        membership(),
        post(),
        comment(),
        opinion(),
        hide(),
        wiki(),
        sequence(),
    ] {
        r.passes(RuleId::G9);
        // G9 is part of G5: a G9 failure always comes with G5, reported once
        let v = r
            .with(&["forum"], json!(other_forum_uri()))
            .fails(RuleId::G9);
        assert!(v.fails(RuleId::G5), "{:?}", v.violations);
        assert_eq!(
            v.violations
                .iter()
                .filter(|x| x.rule_id == RuleId::G5)
                .count(),
            1
        );
    }
    // fk(forum) undefined (U+001F): G9 and G5
    let v = post()
        .with(&["forum"], json!(format!("{}\u{1f}", forum_uri())))
        .fails(RuleId::G9);
    assert!(v.fails(RuleId::G5), "{:?}", v.violations);
}

#[test]
fn g10_nesting_depth() {
    let nest = |n: usize| {
        let mut v = json!(1);
        for _ in 0..n {
            v = json!([v]);
        }
        v
    };
    // the record object is depth 1: 31 arrays inside it make 32
    post().with(&["zz"], nest(31)).valid();
    let r = post().with(&["zz"], nest(32)).fails(RuleId::G10);
    assert_eq!(r.rule_ids(), vec![RuleId::G10]);
    // far too deep values are rejected without walking them
    post().with(&["zz"], nest(1000)).fails(RuleId::G10);
}

// ---------------------------------------------------------------------------------------------
// forum

#[test]
fn fo1_slug_key() {
    forum().valid();
    forum().at_rkey("2024-ai-safety-camp").valid();
    forum().at_rkey("Rationality").fails(RuleId::FO1);
    forum().at_rkey("a--b").fails(RuleId::FO1);
    forum().at_rkey(&"a".repeat(65)).fails(RuleId::FO1);
}

#[test]
fn fo2_moderators_entry() {
    let owner = Did::parse(OWNER).unwrap();
    let r = forum().with(
        &["moderators"],
        json!([
            {"did": ALICE, "role": "admin"},
            {"did": OWNER, "role": "admin"},
            {"did": BOB, "role": "overlord"},
            {"did": ALICE, "role": "moderator"},
            {"did": BOB, "role": "moderator"},
            {"did": "did:web:mod.example.com", "role": "moderator"},
        ]),
    );
    r.valid(); // entry rule: the record stays valid
    let f: Forum = serde_json::from_value(r.v).unwrap();
    let mods: Vec<(&str, Role)> = entry::moderators(&f, &owner)
        .into_iter()
        .map(|(m, role)| (m.did.as_str(), role))
        .collect();
    // the owner and the unknown role are ignored; the first entry for a DID wins (BOB's first
    // entry has an unknown role, so BOB has no entry)
    assert_eq!(
        mods,
        vec![
            (ALICE, Role::Admin),
            ("did:web:mod.example.com", Role::Moderator)
        ]
    );
}

#[test]
fn fo3_indexers_entry() {
    let r = forum().with(
        &["discovery"],
        json!({"indexers": [
            {"url": "https://index.example.com"},
            {"url": "http://index.example.com"},
            {"url": "https://index.example.com:8443"},
            {"url": "mailto:x@example.com"},
            {"url": "HTTPS://Index2.example.com/xrpc"},
        ]}),
    );
    // an uppercase scheme is a valid uri (G2) and a usable indexer url (FO3)
    r.valid();
    let f: Forum = serde_json::from_value(r.v).unwrap();
    let urls: Vec<&str> = entry::usable_indexers(&f)
        .iter()
        .map(|e| e.url.as_str())
        .collect();
    assert_eq!(
        urls,
        [
            "https://index.example.com",
            "HTTPS://Index2.example.com/xrpc"
        ]
    );
}

#[test]
fn fo4_policy_values() {
    // unknown kv values take the default (valid record); out-of-range integers fail G2
    let r = forum().with(
        &["policy"],
        json!({"voting": "quadratic", "tagCreation": "anyone"}),
    );
    r.valid();
    let f: Forum = serde_json::from_value(r.v).unwrap();
    let p = f.policy.unwrap();
    assert!(p.voting().fallback);
    assert_eq!(p.voting().value, lessdugin_lexicon::values::Voting::Reacts);
    assert!(p.tag_creation().fallback);
    forum()
        .with(&["policy"], json!({"karma": {"pairCap": 0}}))
        .fails(RuleId::G2);
    forum()
        .with(&["policy"], json!({"ranking": {"cutoffDays": 36501}}))
        .fails(RuleId::G2);
    forum()
        .with(&["policy"], json!({"gates": {"reactSecond": -1000000000}}))
        .valid();
}

// ---------------------------------------------------------------------------------------------
// membership

#[test]
fn me1_key_is_fk() {
    membership().valid();
    membership()
        .at_rkey(&keys::fk(&other_forum_uri()).unwrap())
        .fails(RuleId::ME1);
    membership().at_rkey("self").fails(RuleId::ME1);
}

// ---------------------------------------------------------------------------------------------
// post

#[test]
fn po1_key() {
    post().valid();
    post()
        .at_rkey(&format!("{}-3l2s5xxv2ze2", fk()))
        .fails(RuleId::PO1);
    post().at_rkey(T1).fails(RuleId::PO1);
    post()
        .at_rkey(&format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()))
        .fails(RuleId::PO1);
}

#[test]
fn po2_effective_kind() {
    // an unknown kind reads as article: a title is required
    post().with(&["kind"], json!("poll")).valid();
    post()
        .with(&["kind"], json!("poll"))
        .without(&["title"])
        .fails(RuleId::PO3);
    let p: Post = serde_json::from_value(post().with(&["kind"], json!("poll")).v).unwrap();
    assert_eq!(p.kind().value, lessdugin_lexicon::values::PostKind::Article);
    assert!(p.kind().fallback);
}

#[test]
fn po3_title_required() {
    post().with(&["title"], json!("x")).valid();
    post().without(&["title"]).fails(RuleId::PO3);
    post()
        .with(&["title"], json!(" \t\u{3000}\u{a0}"))
        .fails(RuleId::PO3);
    post()
        .with(&["kind"], json!("question"))
        .without(&["title"])
        .fails(RuleId::PO3);
    post()
        .with(&["kind"], json!("quickTake"))
        .without(&["title"])
        .valid();
}

#[test]
fn po4_quick_take_body() {
    let qt = post().with(&["kind"], json!("quickTake"));
    qt.valid();
    qt.without(&["body"]).fails(RuleId::PO4);
    // an unknown md member counts as present
    qt.with(&["body"], json!({"$type": "com.example.md"}))
        .valid();
    // entry: a quickTake's title is ignored
    let p: Post = serde_json::from_value(qt.v.clone()).unwrap();
    assert_eq!(entry::post_title(&p), None);
    let p: Post = serde_json::from_value(post().v).unwrap();
    assert_eq!(entry::post_title(&p), Some("Hello"));
}

#[test]
fn po5_link_posts() {
    let lp = post().with(&["kind"], json!("link"));
    lp.fails(RuleId::PO5);
    let with_link = lp.with(&["link"], json!({"uri": "https://example.com/article"}));
    with_link.valid();
    let p: Post = serde_json::from_value(with_link.v).unwrap();
    assert!(entry::post_link(&p).is_some());
    // entry: other kinds ignore link
    let art = post().with(&["link"], json!({"uri": "https://example.com/"}));
    art.valid();
    let p: Post = serde_json::from_value(art.v).unwrap();
    assert!(entry::post_link(&p).is_none());
}

#[test]
fn po6_excerpt_when_spilled() {
    post().with(&["body"], md_blob()).fails(RuleId::PO6);
    let ok = post()
        .with(&["body"], md_blob())
        .with(&["excerpt"], json!("Short"));
    ok.valid();
    let p: Post = serde_json::from_value(ok.v).unwrap();
    assert_eq!(entry::post_excerpt(&p), Some("Short"));
    // entry: an excerpt on an inline body is ignored
    let inline = post().with(&["excerpt"], json!("ignored"));
    inline.valid();
    let p: Post = serde_json::from_value(inline.v).unwrap();
    assert_eq!(entry::post_excerpt(&p), None);
}

#[test]
fn po7_tags_are_a_set() {
    let r = post().with(&["tags"], json!(["ai", "x", "ai"]));
    r.valid();
    let p: Post = serde_json::from_value(r.v).unwrap();
    assert_eq!(entry::post_tags(&p), ["ai", "x"]);
    let p: Post = serde_json::from_value(post().v).unwrap();
    assert!(entry::post_tags(&p).is_empty());
}

#[test]
fn po8_self_labels() {
    let r = post().with(
        &["labels"],
        json!({"$type": "com.atproto.label.defs#selfLabels", "values": [{"val": "porn"}, {"val": "porn"}, {"val": "x-new"}]}),
    );
    r.valid();
    let p: Post = serde_json::from_value(r.v).unwrap();
    assert_eq!(entry::self_labels(&p), ["porn", "x-new"]);
    let r = post().with(
        &["labels"],
        json!({"$type": "com.example.labels", "values": [{"val": "porn"}]}),
    );
    r.valid();
    let p: Post = serde_json::from_value(r.v).unwrap();
    assert!(entry::self_labels(&p).is_empty());
}

// ---------------------------------------------------------------------------------------------
// comment

#[test]
fn co1_key() {
    comment().valid();
    comment()
        .at_rkey(&format!("{}-{T1}", fk()))
        .fails(RuleId::CO1);
    let other = keys::fk(&other_forum_uri()).unwrap();
    comment()
        .at_rkey(&format!("{other}-{T1}-{T2}"))
        .fails(RuleId::CO1);
}

#[test]
fn co2_root_binding() {
    comment().passes(RuleId::CO2);
    // middle segment is not the root post's TID
    comment()
        .at_rkey(&format!("{}-{T3}-{T2}", fk()))
        .fails(RuleId::CO2);
    // root is not a post
    comment()
        .with(&["root"], sref(&wiki_uri()))
        .fails(RuleId::CO2);
    // root post in another forum (fk)
    let other_post = at(
        ALICE,
        "post",
        &format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()),
    );
    comment()
        .with(&["root"], sref(&other_post))
        .fails(RuleId::CO2);
}

#[test]
fn co3_parent_is_a_comment() {
    let other = at(ALICE, "comment", &format!("{}-{T1}-{T3}", fk()));
    comment().with(&["parent"], sref(&other)).valid();
    // a parent in another thread is a placement question, not validity
    let elsewhere = at(ALICE, "comment", &format!("{}-{T4}-{T3}", fk()));
    comment().with(&["parent"], sref(&elsewhere)).valid();
    comment()
        .with(&["parent"], sref(&post_uri()))
        .fails(RuleId::CO3);
}

#[test]
fn co4_inline_body() {
    comment().passes(RuleId::CO4);
    let r = comment().with(&["body"], md_blob()).fails(RuleId::CO4);
    assert!(r.fails(RuleId::G7));
}

#[test]
fn co5_co6_are_left_to_the_fold() {
    // CO5 needs the root post; CO6 needs the forum's authorities: `check` accepts both cases
    assert_eq!(RuleId::CO5.evaluation(), Evaluation::Fold);
    assert_eq!(RuleId::CO6.evaluation(), Evaluation::Fold);
    let r = comment().with(&["moderatorHat"], json!(true));
    let v = r.valid();
    assert!(!v.fails(RuleId::CO5) && !v.fails(RuleId::CO6));
}

// ---------------------------------------------------------------------------------------------
// revision

#[test]
fn rv1_same_repo() {
    revision().valid();
    revision().at_did(BOB).fails(RuleId::RV1);
}

#[test]
fn rv2_subject_collection() {
    revision().passes(RuleId::RV2);
    let r = revision().with(&["subject"], sref(&opinion().uri));
    let v = r.at_did(BOB).fails(RuleId::RV2);
    assert!(v.fails(RuleId::G4));
    revision()
        .with(&["subject"], sref(&membership().uri))
        .fails(RuleId::RV2);
}

#[test]
fn rv3_key() {
    revision().passes(RuleId::RV3);
    revision()
        .at_rkey(&format!("{}-{T3}", fk()))
        .fails(RuleId::RV3);
    revision()
        .at_rkey(&format!("{}-{T1}-notatid", fk()))
        .fails(RuleId::RV3);
    // a forum revision uses P = fk(forum)
    let snap = forum().v;
    let cid = lessdugin_atp::datamodel::record_cid(&snap)
        .unwrap()
        .to_string();
    let fr = R {
        coll: Collection::Revision,
        uri: at(OWNER, "revision", &format!("{}-{T2}", fk())),
        v: json!({"$type": nsid("revision"), "subject": {"uri": forum_uri(), "cid": cid}, "op": "update", "record": snap, "createdAt": NOW}),
    };
    fr.valid();
    fr.at_rkey(&format!("rationality-{T2}")).fails(RuleId::RV3);
}

#[test]
fn rv4_op() {
    revision().passes(RuleId::RV4);
    revision().with(&["op"], json!("patch")).fails(RuleId::RV4);
    revision().with(&["op"], json!("delete")).fails(RuleId::RV4);
    // delete is allowed for a modAction subject
    let h = hide();
    let cid = lessdugin_atp::datamodel::record_cid(&h.v)
        .unwrap()
        .to_string();
    let rkey = h.uri.rsplit('/').next().unwrap();
    R {
        coll: Collection::Revision,
        uri: at(OWNER, "revision", &format!("{rkey}-{T4}")),
        v: json!({"$type": nsid("revision"), "subject": {"uri": h.uri, "cid": cid}, "op": "delete", "record": h.v, "createdAt": NOW}),
    }
    .valid();
}

#[test]
fn rv5_snapshot_integrity() {
    revision().passes(RuleId::RV5);
    // a revision without record is valid ("snapshot not available")
    revision().without(&["record"]).valid();
    revision()
        .with(&["subject", "cid"], json!(DAG_CID))
        .fails(RuleId::RV5);
    revision()
        .with(&["record", "title"], json!("Edited"))
        .fails(RuleId::RV5);
    revision()
        .with(&["record", "$type"], json!(nsid("comment")))
        .fails(RuleId::RV5);
    revision().with(&["record"], json!([1])).fails(RuleId::RV5);
    revision()
        .with(&["record", "x"], json!(0.5))
        .fails(RuleId::RV5);
    // the snapshot is a JSON object, not a $link or $bytes (the field is `unknown`)
    revision()
        .with(&["record"], json!({ "$link": DAG_CID }))
        .fails(RuleId::G2);
    // the snapshot is displayable only if it passes its own lexicon
    assert!(lessdugin_lexicon::validity::snapshot_renderable(&post().v));
    assert!(!lessdugin_lexicon::validity::snapshot_renderable(
        &post().without(&["title"]).with(&["createdAt"], json!(1)).v
    ));
}

/// A revision of `snapshot` (a post value) in Alice's repo, with the snapshot's CID computed with
/// no cap but depth.
fn revision_of(snapshot: Value) -> R {
    let limits = lessdugin_atp::datamodel::Limits::uncapped(64);
    let cid = lessdugin_atp::datamodel::record_cid_with(&snapshot, &limits).unwrap();
    let mut r = revision();
    r.v["subject"]["cid"] = json!(cid.to_string());
    r.v["record"] = snapshot;
    r
}

#[test]
fn rv5_large_valid_snapshots() {
    // a valid post over the S-class memory caps of `Limits::DEFAULT` (131,072 elements per
    // container, 262,144 values): its snapshot has a CID like any other (only G6 bounds size)
    let wide = post().with(&["zz"], json!(vec![0; 131_073]));
    wide.valid();
    revision_of(wide.v.clone()).valid();
    let many = post().with(
        &["zz"],
        json!([vec![0; 100_000], vec![0; 100_000], vec![0; 100_000]]),
    );
    many.valid();
    revision_of(many.v.clone()).valid();
    // and a wrong CID still fails
    let mut bad = revision_of(wide.v);
    bad.v["subject"]["cid"] = json!(DAG_CID);
    bad.fails(RuleId::RV5);
}

// ---------------------------------------------------------------------------------------------
// opinion

#[test]
fn op1_subject_collection() {
    opinion().valid();
    opinion_on(&comment_uri()).valid();
    opinion_on(&wiki_uri()).valid();
    opinion_on(&sequence().uri).valid();
    // a forum subject: OP1 and G4 (and the key is undefined, OP2)
    let f = forum_uri();
    let r = R {
        coll: Collection::Opinion,
        uri: at(
            BOB,
            "opinion",
            &format!("{}-~-{}", fk(), keys::h16("opinion", &[&f]).unwrap()),
        ),
        v: json!({"$type": nsid("opinion"), "subject": sref(&f), "forum": forum_uri(), "createdAt": NOW}),
    };
    let v = r.fails(RuleId::OP1);
    assert!(v.fails(RuleId::G4) && v.fails(RuleId::OP2));
}

#[test]
fn op2_key() {
    opinion().passes(RuleId::OP2);
    // the key of another subject
    opinion()
        .at_rkey(opinion_on(&comment_uri()).uri.rsplit('/').next().unwrap())
        .fails(RuleId::OP2);
    // a subject in another forum, with a self-consistent key
    let other_post = at(
        ALICE,
        "post",
        &format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()),
    );
    opinion_on(&other_post).fails(RuleId::OP2);
}

#[test]
fn op3_unknown_axis_values_are_entry_level() {
    let r = opinion()
        .with(&["karma"], json!("superUp"))
        .with(&["agreement"], json!("meh"))
        .with(&["reacts"], json!([{"react": "agree"}, {"react": "crux", "stance": "sideways"}, {"react": "typo", "stance": "against"}]));
    r.valid();
    let op: Opinion = serde_json::from_value(r.v).unwrap();
    assert_eq!(op.karma(), None);
    assert_eq!(op.agreement(), None);
    let reacts: Vec<(&str, Stance)> = entry::opinion_reacts(&op)
        .into_iter()
        .map(|(r, s)| (r.react.as_str(), s))
        .collect();
    assert_eq!(reacts, [("agree", Stance::For), ("typo", Stance::Against)]);
    let op: Opinion = serde_json::from_value(opinion().v).unwrap();
    assert_eq!(op.karma(), Some(VoteValue::Up));
}

#[test]
fn op4_tag_votes() {
    let tags = json!([
        {"tag": "ai", "value": "up"},
        {"tag": "ai", "value": "up"},
        {"tag": "x", "value": "up"},
        {"tag": "x", "value": "down"},
        {"tag": "y", "value": "bogus"},
        {"tag": "y", "value": "strongUp"},
    ]);
    let r = opinion().with(&["tags"], tags.clone());
    r.valid();
    let op: Opinion = serde_json::from_value(r.v).unwrap();
    assert_eq!(
        entry::opinion_tag_votes(&op),
        [("ai", VoteValue::Up), ("y", VoteValue::StrongUp)]
    );
    // tags on a non-post subject are ignored
    let c = opinion_on(&comment_uri()).with(&["tags"], tags);
    c.valid();
    let op: Opinion = serde_json::from_value(c.v).unwrap();
    assert!(entry::opinion_tag_votes(&op).is_empty());
}

#[test]
fn op5_is_left_to_the_fold() {
    assert_eq!(RuleId::OP5.evaluation(), Evaluation::Fold);
    // a self-opinion (voter = author) is valid; whether it counts is the fold's decision
    let mut r = opinion();
    r.uri = r.uri.replace(BOB, ALICE);
    r.valid();
}

// ---------------------------------------------------------------------------------------------
// modAction

#[test]
fn ma1_subject_forms() {
    hide().valid();
    mod_action(None, ALICE, action("access", json!({"level": "banned"}))).valid();
    mod_action(
        None,
        "did:web:example.com",
        action("access", json!({"level": "banned"})),
    )
    .valid();
    mod_action(
        None,
        "tag:decision-theory",
        action("tag", json!({"removed": true})),
    )
    .valid();
    for bad in [
        "alice.example.com",
        "did:key:zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA",
        "did:plc:short",
        "tag:Decision Theory",
        "tag:",
        "https://example.com",
    ] {
        mod_action(None, bad, action("access", json!({"level": "banned"}))).fails(RuleId::MA1);
    }
    let r = mod_action(
        None,
        "at://alice.example.com/x.y.z/k",
        action("hide", json!({})),
    )
    .fails(RuleId::MA1);
    assert!(r.fails(RuleId::G4));
    // an NS subject whose rkey breaks its collection's grammar is not a canonical at-uri either
    let r = mod_action(
        None,
        &at(ALICE, "post", "not-a-post-key"),
        action("hide", json!({})),
    )
    .fails(RuleId::MA1);
    assert!(r.fails(RuleId::G4));
    // a foreign collection is canonical (MA1 passes) but not allowed for #hide (MA4)
    let foreign = "at://did:plc:abcdefghijklmnopqrstuvwx/app.bsky.feed.post/3l2s5xxv2ze2c";
    let r = mod_action(None, foreign, action("hide", json!({}))).fails(RuleId::MA4);
    assert!(!r.fails(RuleId::MA1));
}

#[test]
fn ma2_key() {
    hide().passes(RuleId::MA2);
    // the key of another action
    let other = mod_action(
        None,
        &post_uri(),
        action("tier", json!({"tier": "personal"})),
    );
    hide()
        .at_rkey(other.uri.rsplit('/').next().unwrap())
        .fails(RuleId::MA2);
    hide().at_rkey(&format!("{}-x", fk())).fails(RuleId::MA2);
    // scope is hashed
    hide().with(&["scope"], json!("author")).fails(RuleId::MA2);
    // the reserved form {fk}-{tid}: valid, no effect
    let reserved = hide().at_rkey(&format!("{}-{T1}", fk()));
    let v = reserved.valid();
    assert_eq!(v.no_effect[0].rule_id, RuleId::MA2);
    assert!(!v.is_effective());
    // ... but its fk must still be the forum's (G9, which is part of G5)
    let v = reserved
        .at_rkey(&format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()))
        .fails(RuleId::G9);
    assert_eq!(v.rule_ids(), vec![RuleId::G5, RuleId::G9]);
    assert_eq!(v.first_rule(), Some(RuleId::G5));
}

#[test]
fn ma3_subject_in_forum() {
    hide().passes(RuleId::MA3);
    let other_post = at(
        ALICE,
        "post",
        &format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()),
    );
    mod_action(None, &other_post, action("hide", json!({}))).fails(RuleId::MA3);
}

#[test]
fn ma4_subject_allowed_for_action() {
    for ok in [
        mod_action(None, &post_uri(), action("hide", json!({}))),
        mod_action(None, &comment_uri(), action("hide", json!({}))),
        mod_action(None, &wiki_uri(), action("hide", json!({}))),
        mod_action(
            None,
            ALICE,
            action("restrict", json!({"capabilities": ["vote"]})),
        ),
        mod_action(None, &post_uri(), action("lock", json!({}))),
        mod_action(
            None,
            &sequence().uri,
            action("pin", json!({"scope": "forum"})),
        )
        .with(&["subjectCid"], json!(DAG_CID)),
        mod_action(
            None,
            &comment_uri(),
            action("pin", json!({"scope": "thread"})),
        )
        .with(&["subjectCid"], json!(DAG_CID)),
    ] {
        ok.valid();
    }
    for bad in [
        mod_action(
            None,
            &post_uri(),
            action("access", json!({"level": "banned"})),
        ),
        mod_action(None, ALICE, action("hide", json!({}))),
        mod_action(None, "tag:ai", action("restrict", json!({}))),
        mod_action(None, ALICE, action("tag", json!({}))),
        mod_action(
            None,
            &comment_uri(),
            action("tier", json!({"tier": "personal"})),
        ),
        mod_action(
            None,
            &comment_uri(),
            action("pin", json!({"scope": "forum"})),
        )
        .with(&["subjectCid"], json!(DAG_CID)),
        mod_action(None, &post_uri(), action("pin", json!({"scope": "thread"})))
            .with(&["subjectCid"], json!(DAG_CID)),
        mod_action(None, &comment_uri(), action("lock", json!({}))),
    ] {
        bad.fails(RuleId::MA4);
    }
    // unknown actions take any subject
    mod_action(None, "tag:x", action("feature", json!({}))).valid();
}

#[test]
fn ma5_positive_assertions_need_subject_cid() {
    let keep = mod_action(None, &post_uri(), action("hide", json!({"hidden": false})));
    keep.fails(RuleId::MA5);
    keep.with(&["subjectCid"], json!(DAG_CID)).valid();
    let curated = mod_action(
        None,
        &post_uri(),
        action("tier", json!({"tier": "curated"})),
    );
    curated.fails(RuleId::MA5);
    curated.with(&["subjectCid"], json!(DAG_CID)).valid();
    mod_action(
        None,
        &post_uri(),
        action("tier", json!({"tier": "personal"})),
    )
    .valid();
    // pinned defaults to true
    mod_action(None, &post_uri(), action("pin", json!({"scope": "forum"}))).fails(RuleId::MA5);
    mod_action(
        None,
        &post_uri(),
        action("pin", json!({"scope": "forum", "pinned": false})),
    )
    .valid();
    // hiding is negative
    hide().valid();
}

#[test]
fn ma6_author_scope() {
    mod_action(Some("author"), &comment_uri(), action("hide", json!({}))).valid();
    mod_action(Some("author"), &post_uri(), action("lock", json!({}))).valid();
    mod_action(
        Some("author"),
        ALICE,
        action("access", json!({"level": "banned"})),
    )
    .valid();
    mod_action(
        Some("author"),
        &comment_uri(),
        action("pin", json!({"scope": "thread"})),
    )
    .with(&["subjectCid"], json!(DAG_CID))
    .valid();
    for bad in [
        mod_action(Some("author"), &post_uri(), action("hide", json!({}))),
        mod_action(
            Some("author"),
            &comment_uri(),
            action("hide", json!({"hidden": false})),
        )
        .with(&["subjectCid"], json!(DAG_CID)),
        mod_action(
            Some("author"),
            ALICE,
            action("access", json!({"level": "member"})),
        ),
        mod_action(Some("author"), ALICE, action("restrict", json!({}))),
        mod_action(
            Some("author"),
            &post_uri(),
            action("tier", json!({"tier": "personal"})),
        ),
        mod_action(
            Some("author"),
            &post_uri(),
            action("pin", json!({"scope": "forum"})),
        )
        .with(&["subjectCid"], json!(DAG_CID)),
        mod_action(Some("author"), "tag:ai", action("tag", json!({}))),
    ] {
        bad.fails(RuleId::MA6);
    }
}

#[test]
fn ma7_unknown_values_have_no_effect() {
    for (r, path) in [
        (
            mod_action(Some("global"), &post_uri(), action("hide", json!({}))),
            "$.scope",
        ),
        (
            mod_action(None, &post_uri(), action("feature", json!({}))),
            "$.action.$type",
        ),
        (
            mod_action(None, ALICE, action("access", json!({"level": "exiled"}))),
            "$.action.level",
        ),
        (
            mod_action(None, &post_uri(), action("tier", json!({"tier": "gold"}))),
            "$.action.tier",
        ),
        (
            mod_action(None, &post_uri(), action("pin", json!({"scope": "global"})))
                .with(&["subjectCid"], json!(DAG_CID)),
            "$.action.scope",
        ),
        // the other namespace prefix's actions are unknown
        (
            mod_action(None, &post_uri(), json!({"$type": "com.example.mod#hide"})),
            "$.action.$type",
        ),
    ] {
        let v = r.valid();
        assert_eq!(v.no_effect.len(), 1, "{:?}", v.no_effect);
        assert_eq!(v.no_effect[0].rule_id, RuleId::MA7);
        assert_eq!(v.no_effect[0].path, path);
    }
    // unknown access level in author scope: no effect rather than invalid
    mod_action(
        Some("author"),
        ALICE,
        action("access", json!({"level": "exiled"})),
    )
    .valid();
    hide().valid();
}

#[test]
fn ma7_overrides_the_action_rules() {
    // MA7 makes the record valid: MA3–MA6 do not apply, the subject may be in any collection
    let other_post = at(
        ALICE,
        "post",
        &format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()),
    );
    for r in [
        // unknown pin scope, positive (pinned defaults to true), no subjectCid (MA5)
        mod_action(None, &post_uri(), action("pin", json!({"scope": "global"}))),
        // unknown modAction scope with a keep (MA5)
        mod_action(
            Some("global"),
            &post_uri(),
            action("hide", json!({"hidden": false})),
        ),
        // unknown action on another forum's post (MA3; §6.7: "allowed subject: any")
        mod_action(None, &other_post, action("feature", json!({}))),
        // unknown action on a profile uri (MA4/G4)
        mod_action(
            None,
            &at(ALICE, "profile", "self"),
            action("feature", json!({})),
        ),
        // unknown scope with a known action on a subject that action does not take (MA4/G4)
        mod_action(Some("global"), &forum_uri(), action("hide", json!({}))),
        // unknown tier in author scope (MA6) on a comment (MA4)
        mod_action(
            Some("author"),
            &comment_uri(),
            action("tier", json!({"tier": "gold"})),
        ),
        // unknown access level on an at-uri subject (MA4)
        mod_action(
            None,
            &post_uri(),
            action("access", json!({"level": "exiled"})),
        ),
    ] {
        let v = r.valid();
        assert!(
            v.no_effect.iter().any(|n| n.rule_id == RuleId::MA7),
            "{:?}",
            v.no_effect
        );
    }
    // MA1, MA2 and the G rules still apply
    mod_action(None, "nonsense", action("feature", json!({}))).fails(RuleId::MA1);
    mod_action(
        None,
        &format!("at://{ALICE}/{}/{T1}", nsid("post")),
        action("feature", json!({})),
    )
    .fails(RuleId::G4);
    mod_action(None, &post_uri(), action("feature", json!({})))
        .with(&["scope"], json!("global"))
        .fails(RuleId::MA2);
}

#[test]
fn ma8_restrict_capabilities() {
    let r = mod_action(
        None,
        ALICE,
        action(
            "restrict",
            json!({"capabilities": ["vote", "teleport", "vote", "post"]}),
        ),
    );
    r.valid();
    let ra: RestrictAction = serde_json::from_value(r.v["action"].clone()).unwrap();
    assert_eq!(
        entry::restrict_capabilities(&ra)
            .into_iter()
            .collect::<Vec<_>>(),
        [Capability::Post, Capability::Vote]
    );
    let empty: RestrictAction = serde_json::from_value(action("restrict", json!({}))).unwrap();
    assert!(entry::restrict_capabilities(&empty).is_empty());
}

#[test]
fn ma9_forum_lock_and_allowed() {
    let fl = mod_action(None, &forum_uri(), action("lock", json!({})));
    let v = fl.valid();
    assert_eq!(v.no_effect[0].rule_id, RuleId::MA9);
    // allowed entries outside the thread are ignored (entry)
    let inside = comment_uri();
    let outside = at(BOB, "comment", &format!("{}-{T4}-{T2}", fk()));
    let lock = mod_action(
        None,
        &post_uri(),
        action("lock", json!({"allowed": [inside, outside]})),
    );
    lock.valid();
    let la: LockAction = serde_json::from_value(lock.v["action"].clone()).unwrap();
    let kept: Vec<&str> = entry::lock_allowed(&la, &post_uri())
        .iter()
        .map(|u| u.as_str())
        .collect();
    assert_eq!(kept, [comment_uri()]);
    // a non-comment allowed entry fails G4
    mod_action(
        None,
        &post_uri(),
        action("lock", json!({"allowed": [post_uri()]})),
    )
    .fails(RuleId::G4);
}

#[test]
fn ma10_tag_head() {
    let ok = mod_action(
        None,
        "tag:decision-theory",
        action("tag", json!({"head": sref(&wiki_uri())})),
    );
    ok.valid();
    let r = mod_action(
        None,
        "tag:decision-theory",
        action("tag", json!({"head": sref(&post_uri())})),
    )
    .fails(RuleId::MA10);
    assert!(r.fails(RuleId::G4));
    let other_wiki = at(
        ALICE,
        "wikiRevision",
        &format!("{}-{T3}", keys::fk(&other_forum_uri()).unwrap()),
    );
    mod_action(
        None,
        "tag:decision-theory",
        action("tag", json!({"head": sref(&other_wiki)})),
    )
    .fails(RuleId::MA10);
}

#[test]
fn ma11_adopts() {
    let original = mod_action(None, &comment_uri(), action("hide", json!({})));
    hide().with(&["adopts"], sref(&original.uri)).valid();
    let r = hide()
        .with(&["adopts"], sref(&post_uri()))
        .fails(RuleId::MA11);
    assert!(r.fails(RuleId::G4));
}

#[test]
fn ma12_tier_notice_inline() {
    mod_action(
        None,
        &post_uri(),
        action("tier", json!({"tier": "personal", "notice": md("Moved")})),
    )
    .valid();
    let r = mod_action(
        None,
        &post_uri(),
        action("tier", json!({"tier": "personal", "notice": md_blob()})),
    )
    .fails(RuleId::MA12);
    assert!(r.fails(RuleId::G7));
}

// ---------------------------------------------------------------------------------------------
// wikiRevision, sequence, profile

#[test]
fn wr1_key() {
    wiki().valid();
    wiki()
        .at_rkey(&format!("{}-{T3}-{T1}", fk()))
        .fails(RuleId::WR1);
    wiki()
        .at_rkey(&format!("{}-{T3}", keys::fk(&other_forum_uri()).unwrap()))
        .fails(RuleId::WR1);
}

#[test]
fn wr2_parents() {
    let parent = at(BOB, "wikiRevision", &format!("{}-{T1}", fk()));
    let r = wiki().with(&["parents"], json!([sref(&parent), sref(&parent)]));
    r.valid();
    let w: WikiRevision = serde_json::from_value(r.v).unwrap();
    assert_eq!(entry::wiki_parents(&w).len(), 1);
    wiki()
        .with(&["parents"], json!([sref(&post_uri())]))
        .fails(RuleId::WR2);
    let other = at(
        BOB,
        "wikiRevision",
        &format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()),
    );
    wiki()
        .with(&["parents"], json!([sref(&other)]))
        .fails(RuleId::WR2);
}

#[test]
fn sq1_key() {
    sequence().valid();
    sequence()
        .at_rkey(&format!("{}-{T4}", keys::fk(&other_forum_uri()).unwrap()))
        .fails(RuleId::SQ1);
    sequence().at_rkey("self").fails(RuleId::SQ1);
}

#[test]
fn sq2_chapter_posts() {
    let other_post = at(
        ALICE,
        "post",
        &format!("{}-{T1}", keys::fk(&other_forum_uri()).unwrap()),
    );
    let r = sequence().with(&["chapters"], json!([{"posts": [post_uri(), other_post]}]));
    r.valid();
    let s: Sequence = serde_json::from_value(r.v).unwrap();
    let shown: Vec<&str> = entry::chapter_posts(&s, &s.chapters[0])
        .iter()
        .map(|u| u.as_str())
        .collect();
    assert_eq!(shown, [post_uri()]);
    let v = sequence()
        .with(&["chapters"], json!([{"posts": [comment_uri()]}]))
        .fails(RuleId::SQ2);
    assert!(v.fails(RuleId::G4));
}

#[test]
fn sq3_chapter_body_and_cover() {
    sequence()
        .with(&["chapters", "0", "body"], md("Intro"))
        .valid();
    let v = sequence()
        .with(&["chapters", "0", "body"], md_blob())
        .fails(RuleId::SQ3);
    assert!(v.fails(RuleId::G7));
    let img = sequence().with(&["cover"], json!({"blob": blob("image/webp", 10)}));
    let pdf = sequence().with(&["cover"], json!({"blob": blob("application/pdf", 10)}));
    img.valid();
    pdf.valid();
    let s: Sequence = serde_json::from_value(img.v).unwrap();
    assert!(entry::sequence_cover(&s).is_some());
    let s: Sequence = serde_json::from_value(pdf.v).unwrap();
    assert!(entry::sequence_cover(&s).is_none());
}

#[test]
fn pf1_key() {
    profile().valid();
    profile().at_rkey("main").fails(RuleId::PF1);
}

#[test]
fn pf2_inline_bio() {
    profile().with(&["bio"], md("Hi")).valid();
    let v = profile().with(&["bio"], md_blob()).fails(RuleId::PF2);
    assert!(v.fails(RuleId::G7));
}

#[test]
fn violations_are_capped_per_rule() {
    use lessdugin_lexicon::validity::MAX_VIOLATIONS_PER_RULE;
    // 200,000 non-slug tags (an 800 KB record): every failing rule is listed, a bounded number
    // of times, and the rest only counted
    let tags = vec![json!("A"); 200_000];
    let r = post()
        .with(&["tags"], Value::Array(tags))
        .with(&["body", "links"], json!(vec!["x"; 50]))
        .without(&["title"]);
    let v = r.fails(RuleId::G8);
    assert_eq!(v.first_rule(), Some(RuleId::G2));
    assert_eq!(
        v.rule_ids(),
        vec![RuleId::G2, RuleId::G4, RuleId::G8, RuleId::PO3]
    );
    for rule in [RuleId::G2, RuleId::G4, RuleId::G8] {
        let n = v.violations.iter().filter(|x| x.rule_id == rule).count();
        assert!(n <= MAX_VIOLATIONS_PER_RULE, "{rule}: {n}");
    }
    assert_eq!(
        v.violations
            .iter()
            .filter(|x| x.rule_id == RuleId::G8)
            .count(),
        MAX_VIOLATIONS_PER_RULE
    );
    assert!(v.violations.len() <= 4 * MAX_VIOLATIONS_PER_RULE);
    assert!(
        v.omitted >= 200_000 - MAX_VIOLATIONS_PER_RULE,
        "{}",
        v.omitted
    );
    // listed violations keep their paths
    assert_eq!(
        v.violations
            .iter()
            .find(|x| x.rule_id == RuleId::G8)
            .unwrap()
            .path,
        "$.tags[0]"
    );
    // a valid record omits nothing
    assert_eq!(post().valid().omitted, 0);
}

#[test]
fn every_rule_has_a_test() {
    // keep this list in sync with the tests above (each name starts with the rule id)
    let tested = [
        "g1", "g2", "g3", "g4", "g5", "g6", "g7", "g8", "g9", "g10", "fo1", "fo2", "fo3", "fo4",
        "me1", "po1", "po2", "po3", "po4", "po5", "po6", "po7", "po8", "co1", "co2", "co3", "co4",
        "co5", "co6", "rv1", "rv2", "rv3", "rv4", "rv5", "op1", "op2", "op3", "op4", "op5", "ma1",
        "ma2", "ma3", "ma4", "ma5", "ma6", "ma7", "ma8", "ma9", "ma10", "ma11", "ma12", "wr1",
        "wr2", "sq1", "sq2", "sq3", "pf1", "pf2",
    ];
    for r in RuleId::ALL {
        assert!(tested.contains(&r.as_str().to_lowercase().as_str()), "{r}");
    }
    assert_eq!(tested.len(), RuleId::ALL.len());
}

#[test]
fn typed_records_built_by_constructors_are_valid() {
    use lessdugin_atp::syntax::Datetime;
    use lessdugin_lexicon::records::{Comment, Membership, StrongRef};
    let now = Datetime::parse(NOW).unwrap();
    let f = AtUri::parse(&forum_uri()).unwrap();
    let post = Post::article(f.clone(), "Title", "Body", now.clone());
    let pv = serde_json::to_value(&post).unwrap();
    R {
        coll: Collection::Post,
        uri: post_uri(),
        v: pv.clone(),
    }
    .valid();
    let cid = lessdugin_atp::datamodel::record_cid(&pv)
        .unwrap()
        .to_string();
    let c = Comment::new(
        f.clone(),
        StrongRef::new(AtUri::parse(&post_uri()).unwrap(), cid),
        None,
        "Reply",
        now.clone(),
    );
    R {
        coll: Collection::Comment,
        uri: comment_uri(),
        v: serde_json::to_value(&c).unwrap(),
    }
    .valid();
    let m = Membership::new(f, now.clone());
    R {
        coll: Collection::Membership,
        uri: membership().uri,
        v: serde_json::to_value(&m).unwrap(),
    }
    .valid();
    let fv = serde_json::to_value(Forum::create(
        "Rationality",
        now,
        lessdugin_lexicon::values::MembershipMode::Open,
    ))
    .unwrap();
    R {
        coll: Collection::Forum,
        uri: forum_uri(),
        v: fv,
    }
    .valid();
}
