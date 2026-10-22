// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forum model and its views over an injected fold input (`social.md` §1.1): explicit
//! records, first-seen times and `now`, no network and no store. The DTO JSON is a pure
//! function of that input, which is what cross-client parity relies on.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::datamodel::record_cid;
use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::dto::{CommentSort, Completeness, ForumRole, HiddenReason, ItemState};
use lessdugin_core::model::views::Views;
use lessdugin_core::model::{ForumModel, RenderCache};
use lessdugin_core::social::input::{FoldInput, ForumInput};
use lessdugin_core::store::records::Level;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_testkit::repo::tid_at_ms;
use serde_json::{Value, json};

const OWNER: &str = "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa";
const BOB: &str = "did:plc:bbbbbbbbbbbbbbbbbbbbbbbb";
const EVE: &str = "did:plc:eeeeeeeeeeeeeeeeeeeeeeee";
/// 2025-10-09T08:53:20Z.
const T0: i64 = 1_760_000_000_000;
const HOUR: i64 = 3_600_000;

struct Input {
    forum: String,
    records: Vec<(String, Value)>,
    obs: Vec<(String, i64)>,
}

fn dt(ms: i64) -> String {
    Datetime::from_millis(ms).unwrap().as_str().to_owned()
}

fn md(text: &str) -> Value {
    json!({"$type": nsid::MARKDOWN, "profile": "lfm/1", "text": text})
}

impl Input {
    fn add(&mut self, did: &str, coll: &str, rkey: &str, v: Value, seen_ms: i64) -> String {
        let uri = format!("at://{did}/{coll}/{rkey}");
        let check = lessdugin_lexicon::validity::check_uri(&AtUri::parse(&uri).unwrap(), &v);
        assert!(check.valid, "{uri}: {check:?}");
        self.records.push((uri.clone(), v));
        self.obs.push((uri.clone(), seen_ms * 1000));
        uri
    }

    fn cid(&self, uri: &str) -> String {
        let v = &self.records.iter().find(|(u, _)| u == uri).unwrap().1;
        record_cid(v).unwrap().to_string()
    }

    fn membership(&mut self, did: &str, at: i64) {
        let v = json!({"$type": nsid::MEMBERSHIP, "forum": self.forum, "createdAt": dt(at)});
        let rkey = keys::membership_rkey(&self.forum).unwrap();
        self.add(did, nsid::MEMBERSHIP, &rkey, v, at);
    }

    fn post(&mut self, did: &str, title: &str, text: &str, at: i64) -> String {
        let v = json!({
            "$type": nsid::POST, "forum": self.forum, "title": title, "body": md(text),
            "createdAt": dt(at),
        });
        let rkey = keys::post_rkey(&self.forum, tid_at_ms(at).as_str()).unwrap();
        self.add(did, nsid::POST, &rkey, v, at)
    }

    fn comment(
        &mut self,
        did: &str,
        root: &str,
        parent: Option<(&str, &str)>,
        text: &str,
        at: i64,
    ) -> String {
        let mut v = json!({
            "$type": nsid::COMMENT, "forum": self.forum,
            "root": {"uri": root, "cid": self.cid(root)},
            "body": md(text), "createdAt": dt(at),
        });
        if let Some((uri, cid)) = parent {
            v["parent"] = json!({"uri": uri, "cid": cid});
        }
        let rkey = keys::comment_rkey(NS, &self.forum, root, tid_at_ms(at).as_str()).unwrap();
        self.add(did, nsid::COMMENT, &rkey, v, at)
    }

    fn contract(&self, decl: &Value, now_ms: i64) -> FoldInput {
        let records: Vec<Value> = self
            .records
            .iter()
            .map(|(u, v)| json!({"uri": u, "cid": record_cid(v).unwrap().to_string(), "value": v}))
            .collect();
        let obs: Vec<Value> = self
            .obs
            .iter()
            .map(|(u, t)| json!({"uri": u, "cid": self.cid(u), "us": t}))
            .chain([json!({"uri": self.forum, "cid": record_cid(decl).unwrap().to_string(), "us": T0 * 1000})])
            .collect();
        let status: BTreeMap<&str, &str> = [OWNER, BOB, EVE]
            .into_iter()
            .map(|d| (d, "fresh"))
            .collect();
        FoldInput::from_vector_json(&json!({
            "decl": {"uri": self.forum, "cid": record_cid(decl).unwrap().to_string(), "value": decl},
            "records": records,
            "absent": [],
            "obs": obs,
            "repoStatus": status,
            "now": now_ms * 1000,
        }))
        .unwrap()
    }
}

fn validity(uri: &str, v: &Value) -> (bool, Option<String>) {
    let c = lessdugin_lexicon::validity::check_uri(&AtUri::parse(uri).unwrap(), v);
    (c.valid, c.first_rule().map(|r| r.to_string()))
}

#[test]
fn views_of_an_injected_input() {
    let key = ForumKey::new(&OWNER.parse().unwrap(), "meta").unwrap();
    let forum = key.uri.to_string();
    let mut decl = serde_json::to_value(lessdugin_lexicon::records::Forum::create(
        "Meta",
        Datetime::from_millis(T0).unwrap(),
        lessdugin_lexicon::values::MembershipMode::Approval,
    ))
    .unwrap();
    decl["defaultTier"] = json!("frontpage");
    let mut i = Input {
        forum: forum.clone(),
        records: Vec::new(),
        obs: Vec::new(),
    };
    i.membership(OWNER, T0);
    // the owner grants bob; eve joins without a grant (not a participant in approval mode)
    let grant = json!({
        "$type": nsid::MOD_ACTION, "forum": forum, "subject": BOB,
        "action": {"$type": format!("{}#access", nsid::MOD_ACTION), "level": "member"},
        "createdAt": dt(T0 + 1),
    });
    let am = grant["action"].as_object().unwrap().clone();
    let rkey = keys::mod_action_rkey(NS, &forum, None, BOB, &am).unwrap();
    i.add(OWNER, nsid::MOD_ACTION, &rkey, grant, T0 + 1);
    i.membership(BOB, T0 + 2);
    i.membership(EVE, T0 + 3);

    let p1 = i.post(OWNER, "Hello", "The *first* post.", T0 + HOUR);
    let p2 = i.post(BOB, "Second", "Another post.", T0 + 2 * HOUR);
    let p1_cid = i.cid(&p1);
    let c1 = i.comment(BOB, &p1, None, "A reply.", T0 + HOUR + 60_000);
    let c1_cid = i.cid(&c1);
    // eve's reply is hidden (not a participant); bob's answer to it keeps it as a tombstone
    let c2 = i.comment(
        EVE,
        &p1,
        Some((&c1, &c1_cid)),
        "Eve here.",
        T0 + HOUR + 120_000,
    );
    let c2_cid = i.cid(&c2);
    let c3 = i.comment(
        BOB,
        &p1,
        Some((&c2, &c2_cid)),
        "Hi eve.",
        T0 + HOUR + 180_000,
    );
    // a reply to a comment that is not in the input: a Missing placeholder under the root
    let gone = format!(
        "at://{BOB}/{}/{}",
        nsid::COMMENT,
        keys::comment_rkey(NS, &forum, &p1, tid_at_ms(T0 + HOUR + 30_000).as_str()).unwrap()
    );
    let c4 = i.comment(
        OWNER,
        &p1,
        Some((&gone, &p1_cid)),
        "Replying to a lost one.",
        T0 + HOUR + 240_000,
    );

    let now = T0 + 3 * HOUR;
    let contract = i.contract(&decl, now);
    let input = ForumInput::from_contract(&contract, NS, validity).unwrap();
    let model =
        ForumModel::from_input(key, input, Level::Pds, BTreeMap::new(), BTreeSet::new()).unwrap();
    let values: BTreeMap<String, Value> = i.records.iter().cloned().collect();
    let renders = RenderCache::default();
    let v = Views {
        model: &model,
        actors: &BTreeMap::new(),
        pds: &BTreeMap::new(),
        values: &values,
        renders: &renders,
        pending: &BTreeSet::new(),
        completeness: &Completeness::default(),
        now: model.now(),
    };

    let thread = v.thread_view(&p1, CommentSort::Old);
    let shape: Vec<(&str, u32, &ItemState, bool)> = thread
        .comments
        .iter()
        .map(|c| (c.uri.as_str(), c.depth, &c.state, c.missing))
        .collect();
    let tomb = ItemState::Tombstone {
        reason: HiddenReason::NotParticipant,
    };
    assert_eq!(
        shape,
        [
            (c1.as_str(), 1, &ItemState::Visible, false),
            (c2.as_str(), 2, &tomb, false),
            (c3.as_str(), 3, &ItemState::Visible, false),
            (gone.as_str(), 1, &ItemState::Visible, true),
            (c4.as_str(), 2, &ItemState::Visible, false),
        ]
    );
    assert!(
        thread.comments[1].body.html.is_empty(),
        "hidden bodies are never rendered"
    );
    assert_eq!(v.role_of(EVE), ForumRole::None);
    assert_eq!(v.role_of(BOB), ForumRole::Member);
    let posts: Vec<_> = [&p2, &p1]
        .iter()
        .filter_map(|u| v.post_summary(u))
        .collect();
    assert_eq!(posts[1].comment_count, 3);

    insta::assert_json_snapshot!("model_thread", thread);
    insta::assert_json_snapshot!("model_posts", posts);
    // the same input gives the same bytes
    let again = ForumModel::from_input(
        model.key.clone(),
        model.input.clone(),
        Level::Pds,
        BTreeMap::new(),
        BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(again, model);
}

fn build(decl: &Value, i: &Input, now: i64) -> ForumModel {
    let key = ForumKey::new(&OWNER.parse().unwrap(), "meta").unwrap();
    let contract = i.contract(decl, now);
    let input = ForumInput::from_contract(&contract, NS, validity).unwrap();
    ForumModel::from_input(key, input, Level::Pds, BTreeMap::new(), BTreeSet::new()).unwrap()
}

fn base(mode: lessdugin_lexicon::values::MembershipMode) -> (String, Value, Input) {
    let key = ForumKey::new(&OWNER.parse().unwrap(), "meta").unwrap();
    let forum = key.uri.to_string();
    let mut decl = serde_json::to_value(lessdugin_lexicon::records::Forum::create(
        "Meta",
        Datetime::from_millis(T0).unwrap(),
        mode,
    ))
    .unwrap();
    decl["defaultTier"] = json!("frontpage");
    let mut i = Input {
        forum: forum.clone(),
        records: Vec::new(),
        obs: Vec::new(),
    };
    i.membership(OWNER, T0);
    (forum, decl, i)
}

fn views<'a>(
    model: &'a ForumModel,
    values: &'a BTreeMap<String, Value>,
    renders: &'a RenderCache,
    pending: &'a BTreeSet<String>,
    actors: &'a BTreeMap<String, lessdugin_core::dto::ActorView>,
    completeness: &'a Completeness,
) -> Views<'a> {
    Views {
        model,
        actors,
        pds: actors_pds(),
        values,
        renders,
        pending,
        completeness,
        now: model.now(),
    }
}

fn actors_pds() -> &'static BTreeMap<String, String> {
    static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeMap::new)
}

/// M5 of the engine review: a hidden or tombstoned post shows none of its author's content —
/// title, link, tags and labels are blank like the body.
#[test]
fn a_hidden_post_shows_no_title_link_tags_or_labels() {
    let (forum, decl, mut i) = base(lessdugin_lexicon::values::MembershipMode::Approval);
    i.membership(EVE, T0 + 3);
    let v = json!({
        "$type": nsid::POST, "forum": forum, "kind": "link", "title": "BUY CHEAP PILLS",
        "link": {"uri": "https://spam.example/pills"}, "tags": ["spam-tag"],
        "labels": {"$type": "com.atproto.label.defs#selfLabels", "values": [{"val": "spam"}]},
        "body": md("spam body"), "createdAt": dt(T0 + HOUR),
    });
    let rkey = keys::post_rkey(&forum, tid_at_ms(T0 + HOUR).as_str()).unwrap();
    let p = i.add(EVE, nsid::POST, &rkey, v, T0 + HOUR);
    // the owner answers under it, so the post stays in the thread as a tombstone
    i.comment(OWNER, &p, None, "Removed, sorry.", T0 + 2 * HOUR);
    let model = build(&decl, &i, T0 + 3 * HOUR);
    let values: BTreeMap<String, Value> = i.records.iter().cloned().collect();
    let (renders, pending, actors, c) = (
        RenderCache::default(),
        BTreeSet::new(),
        BTreeMap::new(),
        Completeness::default(),
    );
    let v = views(&model, &values, &renders, &pending, &actors, &c);
    let pv = v.post_view(&p).unwrap();
    assert!(
        matches!(
            pv.post.state,
            ItemState::Tombstone { .. } | ItemState::Hidden { .. }
        ),
        "{:?}",
        pv.post.state
    );
    assert_eq!(pv.post.title, None);
    assert_eq!(pv.post.link, None);
    assert!(pv.post.tags.is_empty() && pv.post.labels.is_empty());
    assert!(pv.body.html.is_empty() && pv.post.excerpt.is_empty());
    let thread = serde_json::to_string(&v.thread_view(&p, CommentSort::Old)).unwrap();
    assert!(
        !thread.contains("PILLS") && !thread.contains("spam"),
        "{thread}"
    );
}

/// M3 of the engine review: under `voting = karmaOnly` the score carries no `agreement`
/// (`engine-api.md` §4.1); otherwise it does.
#[test]
fn karma_only_scores_have_no_agreement() {
    for (voting, present) in [(Some("karmaOnly"), false), (None, true)] {
        let (_, mut decl, mut i) = base(lessdugin_lexicon::values::MembershipMode::Open);
        if let Some(v) = voting {
            decl["policy"] = json!({ "voting": v });
        }
        let p = i.post(OWNER, "Hello", "x", T0 + HOUR);
        let model = build(&decl, &i, T0 + 3 * HOUR);
        let values: BTreeMap<String, Value> = i.records.iter().cloned().collect();
        let (renders, pending, actors, c) = (
            RenderCache::default(),
            BTreeSet::new(),
            BTreeMap::new(),
            Completeness::default(),
        );
        let v = views(&model, &values, &renders, &pending, &actors, &c);
        let s = serde_json::to_value(v.post_summary(&p).unwrap()).unwrap();
        assert_eq!(
            s["score"].get("agreement").is_some(),
            present,
            "{voting:?}: {s}"
        );
    }
}
