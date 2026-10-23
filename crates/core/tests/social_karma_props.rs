// SPDX-License-Identifier: AGPL-3.0-or-later
//! Property tests of `karma/1` (`social.md` §6.3, §7) against the full fold
//! ([`fold_view`]), on random forums whose every record is valid under this build's lexicon.
//!
//! | Property | Checked here |
//! |---|---|
//! | P4 layering | stripping the `reacts` and `tags` entries of opinions, and adding wikiRevisions with opinions on them, changes no K1, `seedUp`, E, KP or K |
//! | P5 corollary | removing every opinion of a voter u ∉ S (none on an absent subject) changes no K1, `seedUp` or E, and moves each `KP[r]` by at most `pairCap × strong(max(K1[u], 0))` |
//! | P6 seed persistence | S ⊆ E; E ⊆ S ∪ (P ∖ (B ∪ N)); approval mode: E = S |
//! | P7 sybil bound | records by outsiders (new DIDs: memberships, posts, opinions on any item) change no K1, `seedUp`, E, KP, K, nor the score and agreement of any item of the input |
//! | §7 consistency | without a binding pair cap or absent subjects, `K[a] = Σ (score − self)` over a's posts and comments |
//!
//! `social_props.rs` checks every property of §6.3 (P1–P8, with P5 per stage) on a richer
//! generator (forum and author moderation, threads, edits, wiki, sequences); `social_parity.rs`
//! is P9 (native equals wasm); `social_perf.rs` is the performance budget and the linear growth
//! of P3.
//!
//! `cargo test -p lessdugin-core --test social_karma_props` (any namespace build).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::tid::Tid;
use lessdugin_core::social::full::karma::{Karma, small, strong};
use lessdugin_core::social::full::{ForumView, Query, fold_view};
use lessdugin_core::social::input::FoldInput;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// A genuine CIDv1 (dag-cbor, sha256): the fold treats CIDs as opaque.
const CID: &str = "bafyreibox5tmitcjfmeurxwythqcq5cazsbifbkr56hu4752nd7erb6vdu";
/// 2026-01-01T00:00:00Z (µs).
const T0: u64 = 1_767_225_600_000_000;
/// DIDs: 0 is the owner, 1 an admin, 2 a moderator, 3.. plain users.
const USERS: usize = 10;

/// A did:plc (24 base32 characters) named by `tag` and `i < 26`.
fn plc(tag: char, i: usize) -> String {
    let letter = char::from(b'a' + u8::try_from(i).unwrap());
    format!("did:plc:{tag}{letter}{}", "2".repeat(22))
}

fn did(i: usize) -> String {
    plc('u', i)
}

fn outsider(i: usize) -> String {
    plc('x', i)
}

fn at(minute: usize) -> String {
    format!("2026-01-01T{:02}:{:02}:00.000Z", minute / 60, minute % 60)
}

fn tid(n: usize) -> String {
    let t = T0 + u64::try_from(n).unwrap() * 60_000_000;
    Tid::from_parts(t, 0).unwrap().as_str().to_owned()
}

/// A user's standing: membership, the owner's `#access`, the owner's `#restrict`.
#[derive(Clone, Debug)]
struct User {
    member: bool,
    /// 0 none, 1 member, 2 banned.
    access: u8,
    /// 0 none, 1 nullifyVotes, 2 `vote`, 3 `react`, 4 `tag`.
    restrict: u8,
}

/// An opinion: voter, subject index (posts, then comments, then deleted posts), axes.
#[derive(Clone, Debug)]
struct Op {
    voter: usize,
    subject: usize,
    karma: Option<u8>,
    agreement: Option<u8>,
    react: Option<(u8, bool)>,
    tag: Option<u8>,
}

#[derive(Clone, Debug)]
struct Scenario {
    open: bool,
    pair_cap: i64,
    establish: i64,
    establish_seeds: i64,
    voting: u8,
    users: Vec<User>,
    posts: Vec<usize>,
    comments: Vec<(usize, usize)>,
    deleted: Vec<usize>,
    ops: Vec<Op>,
}

const KARMA: [&str; 5] = ["up", "down", "strongUp", "strongDown", "superUp"];
const AGREEMENT: [&str; 4] = ["agree", "disagree", "strongAgree", "strongDisagree"];
const VOTING: [&str; 3] = ["karmaOnly", "twoAxis", "reacts"];
const REACTS: [&str; 3] = ["insightful", "crux", "not-a-palette-id"];

fn op() -> impl Strategy<Value = Op> {
    (
        0..USERS,
        0usize..64,
        proptest::option::of(0u8..5),
        proptest::option::of(0u8..4),
        proptest::option::of((0u8..3, any::<bool>())),
        proptest::option::of(0u8..5),
    )
        .prop_map(|(voter, subject, karma, agreement, react, tag)| Op {
            voter,
            subject,
            karma,
            agreement,
            react,
            tag,
        })
}

fn scenario(pair_caps: &'static [i64]) -> impl Strategy<Value = Scenario> {
    let user = (
        any::<bool>(),
        0u8..3,
        prop_oneof![4 => Just(0u8), 1 => 1u8..5],
    )
        .prop_map(|(member, access, restrict)| User {
            member,
            access,
            restrict,
        });
    (
        any::<bool>(),
        proptest::sample::select(pair_caps),
        0i64..3,
        1i64..4,
        0u8..3,
        proptest::collection::vec(user, USERS),
        proptest::collection::vec(0..USERS, 1..10),
        proptest::collection::vec((0..USERS, 0usize..16), 0..6),
        proptest::collection::vec(0..USERS, 0..3),
        proptest::collection::vec(op(), 0..60),
    )
        .prop_map(
            |(
                open,
                pair_cap,
                establish,
                establish_seeds,
                voting,
                users,
                posts,
                comments,
                deleted,
                ops,
            )| {
                let comments = comments
                    .into_iter()
                    .map(|(a, p)| (a, p % posts.len()))
                    .collect();
                Scenario {
                    open,
                    pair_cap,
                    establish,
                    establish_seeds,
                    voting,
                    users,
                    posts,
                    comments,
                    deleted,
                    ops,
                }
            },
        )
}

/// The input builder: the vector JSON form of `FoldInput`, every record valid.
struct Builder {
    forum: String,
    fk: String,
    records: BTreeMap<String, Value>,
    absent: BTreeSet<String>,
    dids: BTreeSet<String>,
    next: usize,
}

impl Builder {
    fn new() -> Self {
        let forum = format!("at://{}/{NS}.forum/test", did(0));
        let fk = keys::fk(&forum).unwrap();
        Self {
            forum,
            fk,
            records: BTreeMap::new(),
            absent: BTreeSet::new(),
            dids: (0..3).map(did).collect(),
            next: 1,
        }
    }

    fn put(&mut self, author: &str, coll: &str, rkey: &str, value: Value) -> String {
        let uri = format!("at://{author}/{NS}.{coll}/{rkey}");
        self.dids.insert(author.to_owned());
        self.records.insert(uri.clone(), value);
        uri
    }

    fn fresh_tid(&mut self) -> String {
        self.next += 1;
        tid(self.next)
    }

    fn membership(&mut self, d: &str) {
        let v =
            json!({"$type": format!("{NS}.membership"), "forum": self.forum, "createdAt": at(0)});
        let fk = self.fk.clone();
        self.put(d, "membership", &fk, v);
    }

    fn mod_action(&mut self, subject: &str, action: Value) {
        let Value::Object(a) = &action else {
            unreachable!()
        };
        let rkey = keys::mod_action_rkey(NS, &self.forum, None, subject, a).unwrap();
        let v = json!({
            "$type": format!("{NS}.modAction"),
            "forum": self.forum,
            "subject": subject,
            "action": action,
            "createdAt": at(0),
        });
        self.put(&did(0), "modAction", &rkey, v);
    }

    fn post(&mut self, author: &str) -> String {
        let t = self.fresh_tid();
        let rkey = format!("{}-{t}", self.fk);
        let v = json!({"$type": format!("{NS}.post"), "forum": self.forum, "title": "A post", "createdAt": at(self.next)});
        self.put(author, "post", &rkey, v)
    }

    /// A post uri that is not in `records` (its absence confirmed once an opinion names it).
    fn deleted_post(&mut self, author: &str) -> String {
        let t = self.fresh_tid();
        format!("at://{author}/{NS}.post/{}-{t}", self.fk)
    }

    fn comment(&mut self, author: &str, root: &str) -> String {
        let t = self.fresh_tid();
        let rkey = keys::comment_rkey(NS, &self.forum, root, &t).unwrap();
        let v = json!({
            "$type": format!("{NS}.comment"),
            "forum": self.forum,
            "root": {"uri": root, "cid": CID},
            "body": {"$type": format!("{NS}.markdown"), "text": "A comment"},
            "createdAt": at(self.next),
        });
        self.put(author, "comment", &rkey, v)
    }

    fn wiki_revision(&mut self, author: &str) -> String {
        let t = self.fresh_tid();
        let rkey = format!("{}-{t}", self.fk);
        let v = json!({"$type": format!("{NS}.wikiRevision"), "forum": self.forum, "slug": "t", "name": "T", "createdAt": at(self.next)});
        self.put(author, "wikiRevision", &rkey, v)
    }

    fn opinion(&mut self, voter: &str, subject: &str, axes: Map<String, Value>) {
        let rkey = keys::opinion_rkey(NS, &self.forum, subject).unwrap();
        let mut v = json!({
            "$type": format!("{NS}.opinion"),
            "forum": self.forum,
            "subject": {"uri": subject, "cid": CID},
            "createdAt": at(0),
        });
        v.as_object_mut().unwrap().extend(axes);
        if !self.records.contains_key(subject) {
            self.absent.insert(subject.to_owned());
        }
        self.put(voter, "opinion", &rkey, v);
    }

    fn input(&self, s: &Scenario) -> FoldInput {
        let mut decl = json!({
            "$type": format!("{NS}.forum"),
            "name": "Test forum",
            "createdAt": at(0),
            "membership": if s.open { "open" } else { "approval" },
            "moderators": [{"did": did(1), "role": "admin"}, {"did": did(2), "role": "moderator"}],
            "policy": {
                "voting": VOTING[usize::from(s.voting)],
                "karma": {"pairCap": s.pair_cap, "establish": s.establish, "establishSeeds": s.establish_seeds},
            },
        });
        if s.voting == 2 {
            // the default, written explicitly or not
            decl["policy"].as_object_mut().unwrap().remove("voting");
        }
        let records: Vec<Value> = self
            .records
            .iter()
            .map(|(u, v)| json!({"uri": u, "cid": CID, "value": v}))
            .collect();
        let status: Map<String, Value> = self
            .dids
            .iter()
            .map(|d| (d.clone(), json!("fresh")))
            .collect();
        let v = json!({
            "decl": {"uri": self.forum, "cid": CID, "value": decl},
            "records": records,
            "absent": self.absent,
            "obs": [],
            "repoStatus": status,
            "now": T0 + 30 * 86_400_000_000,
        });
        FoldInput::from_vector_json(&v).unwrap()
    }
}

/// How the input is varied around a scenario.
#[derive(Clone, Copy, Default)]
struct Variant {
    /// Leave out the `reacts` and `tags` entries (P4).
    no_entries: bool,
    /// Add wikiRevisions and opinions on them (P4).
    wiki: bool,
    /// Leave out every opinion of this voter (P5).
    without_voter: Option<usize>,
    /// Add records by outsiders (P7).
    outsiders: bool,
}

fn build(s: &Scenario, var: Variant) -> FoldInput {
    let mut b = Builder::new();
    for (i, u) in s.users.iter().enumerate() {
        let d = did(i);
        if u.member {
            b.membership(&d);
        }
        if i == 0 {
            continue;
        }
        match u.access {
            1 => b.mod_action(
                &d,
                json!({"$type": format!("{NS}.modAction#access"), "level": "member"}),
            ),
            2 => b.mod_action(
                &d,
                json!({"$type": format!("{NS}.modAction#access"), "level": "banned"}),
            ),
            _ => {}
        }
        let restrict = format!("{NS}.modAction#restrict");
        match u.restrict {
            1 => b.mod_action(&d, json!({"$type": restrict, "nullifyVotes": true})),
            2 => b.mod_action(&d, json!({"$type": restrict, "capabilities": ["vote"]})),
            3 => b.mod_action(&d, json!({"$type": restrict, "capabilities": ["react"]})),
            4 => b.mod_action(&d, json!({"$type": restrict, "capabilities": ["tag"]})),
            _ => {}
        }
    }
    let posts: Vec<String> = s.posts.iter().map(|a| b.post(&did(*a))).collect();
    let comments: Vec<String> = s
        .comments
        .iter()
        .map(|(a, p)| {
            let root = posts[*p].clone();
            b.comment(&did(*a), &root)
        })
        .collect();
    let deleted: Vec<String> = s.deleted.iter().map(|a| b.deleted_post(&did(*a))).collect();
    let subjects: Vec<String> = posts
        .iter()
        .chain(&comments)
        .chain(&deleted)
        .cloned()
        .collect();
    let mut seen = BTreeSet::new();
    for o in &s.ops {
        let subject = &subjects[o.subject % subjects.len()];
        if var.without_voter == Some(o.voter) || !seen.insert((o.voter, subject.clone())) {
            continue;
        }
        let mut axes = Map::new();
        if let Some(k) = o.karma {
            axes.insert("karma".into(), json!(KARMA[usize::from(k)]));
        }
        if let Some(a) = o.agreement {
            axes.insert("agreement".into(), json!(AGREEMENT[usize::from(a)]));
        }
        if let Some((r, against)) = o.react {
            let stance = if against { "against" } else { "for" };
            axes.insert(
                "reacts".into(),
                json!([{"react": REACTS[usize::from(r)], "stance": stance}]),
            );
        }
        if let Some(t) = o.tag {
            axes.insert(
                "tags".into(),
                json!([{"tag": "t", "value": KARMA[usize::from(t)]}]),
            );
        }
        if axes.is_empty() {
            axes.insert("karma".into(), json!("up"));
        }
        if var.no_entries {
            // an opinion left without content is not written at all (an L5-only input)
            axes.remove("reacts");
            axes.remove("tags");
            if axes.is_empty() {
                continue;
            }
        }
        b.opinion(&did(o.voter), subject, axes);
    }
    if var.wiki {
        let revs: Vec<String> = (0..3).map(|i| b.wiki_revision(&did(3 + i))).collect();
        for (i, r) in revs.iter().enumerate() {
            let mut axes = Map::new();
            axes.insert("karma".into(), json!("strongUp"));
            b.opinion(&did(i), r, axes);
        }
    }
    if var.outsiders {
        let items: Vec<String> = posts.iter().chain(&comments).cloned().collect();
        for i in 0..4 {
            let x = outsider(i);
            b.membership(&x);
            let own = b.post(&x);
            for (j, s) in items.iter().chain([&own]).enumerate() {
                if (i + j) % 2 == 0 {
                    let mut axes = Map::new();
                    axes.insert("karma".into(), json!(KARMA[(i + j) % 4]));
                    axes.insert("agreement".into(), json!(AGREEMENT[j % 4]));
                    b.opinion(&x, s, axes);
                }
            }
            // outsiders vote on each other's posts too
            if i > 0 {
                let other = b.post(&outsider(i - 1));
                let mut axes = Map::new();
                axes.insert("karma".into(), json!("strongUp"));
                b.opinion(&x, &other, axes);
            }
        }
    }
    b.input(s)
}

fn fold(input: &FoldInput) -> ForumView {
    let v = fold_view(input, &Query::default()).expect("the fold accepts the input");
    assert!(
        v.raw.invalid.is_empty(),
        "generated records must be valid: {:?}",
        v.raw.invalid
    );
    v
}

fn karma(v: &ForumView) -> &Karma {
    v.karma.as_ref().expect("karma is computed")
}

fn stages(k: &Karma) -> impl PartialEq + core::fmt::Debug + '_ {
    (&k.k1, &k.seed_up, &k.e, &k.kp, &k.k)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// P6 and the shape of E (§3.2, §6.2).
    #[test]
    fn p6_seeds_persist(s in scenario(&[1, 2, 3, 25])) {
        let v = fold(&build(&s, Variant::default()));
        let (k, l2) = (karma(&v), &v.participation);
        prop_assert!(l2.s.is_subset(&k.e));
        for u in k.e.difference(&l2.s) {
            prop_assert!(l2.p.contains(u) && !l2.b.contains(u) && !l2.n.contains(u));
        }
        if !s.open {
            prop_assert_eq!(&k.e, &l2.s);
        }
    }

    /// P4: reacts and tags entries, wikiRevisions and opinions on them are L5 inputs only.
    #[test]
    fn p4_layering(s in scenario(&[1, 2, 3, 25])) {
        let a = fold(&build(&s, Variant::default()));
        let b = fold(&build(&s, Variant { no_entries: true, wiki: true, ..Variant::default() }));
        prop_assert_eq!(stages(karma(&a)), stages(karma(&b)));
    }

    /// The corollary of P5: a voter outside S moves each KP[r] by at most their stage cap.
    #[test]
    fn p5_pair_bound(s in scenario(&[1, 2, 3, 25]), pick in 0..USERS) {
        let a = fold(&build(&s, Variant::default()));
        let ka = karma(&a);
        let u = did(pick);
        let on_absent = ka.opinions.values().any(|o| o.voter == u && o.absent);
        prop_assume!(!a.participation.s.contains(&u) && !on_absent);
        let b = fold(&build(&s, Variant { without_voter: Some(pick), ..Variant::default() }));
        let kb = karma(&b);
        prop_assert_eq!(&ka.k1, &kb.k1);
        prop_assert_eq!(&ka.seed_up, &kb.seed_up);
        prop_assert_eq!(&ka.e, &kb.e);
        let cap = s.pair_cap * strong(ka.k1_of(&u).max(0));
        let rs: BTreeSet<&String> = ka.kp.keys().chain(kb.kp.keys()).collect();
        for r in rs {
            let d = (ka.kp_of(r) - kb.kp_of(r)).abs();
            prop_assert!(d <= cap, "KP[{}] moved by {} > {}", r, d, cap);
        }
    }

    /// P7: outsiders change no karma stage, score or agreement of the input's items.
    #[test]
    fn p7_sybil_bound(s in scenario(&[1, 2, 3, 25])) {
        let a = fold(&build(&s, Variant::default()));
        let b = fold(&build(&s, Variant { outsiders: true, ..Variant::default() }));
        prop_assert_eq!(stages(karma(&a)), stages(karma(&b)));
        let (sa, sb) = (a.scores.as_ref().unwrap(), b.scores.as_ref().unwrap());
        for (uri, x) in sa {
            let y = &sb[uri];
            prop_assert_eq!((x.score, x.agreement), (y.score, y.agreement), "{}", uri);
        }
    }

    /// §7 consistency: K[a] = Σ (score − self) over a's posts and comments, when no pair cap
    /// binds and no subject is absent.
    #[test]
    fn scores_consistent_with_k(mut s in scenario(&[1_000_000])) {
        s.deleted.clear();
        let v = fold(&build(&s, Variant::default()));
        let k = karma(&v);
        let scores = v.scores.as_ref().unwrap();
        let mut sum: BTreeMap<String, i64> = BTreeMap::new();
        for (uri, sc) in scores {
            let (author, coll, _) = keys::split_uri(uri).unwrap();
            let (post, comment) = (format!("{NS}.post"), format!("{NS}.comment"));
            let is_post = coll == post;
            prop_assert!(is_post || coll == comment);
            let own = if k.e.contains(author) {
                let kp = k.kp_of(author).max(0);
                if is_post { strong(kp) } else { small(kp) }
            } else {
                0
            };
            *sum.entry(author.to_owned()).or_insert(0) += sc.score - own;
        }
        sum.retain(|_, x| *x != 0);
        prop_assert_eq!(&sum, &k.k);
    }
}

/// A fixed scenario, so that a regression shows without shrinking.
#[test]
fn fixed_scenario_is_deterministic() {
    let s = Scenario {
        open: true,
        pair_cap: 2,
        establish: 1,
        establish_seeds: 2,
        voting: 2,
        users: (0..USERS)
            .map(|i| User {
                member: true,
                access: u8::from(i % 3 == 0),
                restrict: if i == 5 { 2 } else { 0 },
            })
            .collect(),
        posts: (0..USERS).collect(),
        comments: vec![(4, 0), (5, 3)],
        deleted: vec![7],
        ops: (0..USERS * 4)
            .map(|n| Op {
                voter: n % USERS,
                subject: n * 7 % 13,
                karma: Some(u8::try_from(n % 4).unwrap()),
                agreement: None,
                react: None,
                tag: None,
            })
            .collect(),
    };
    let input = build(&s, Variant::default());
    let (a, b) = (fold(&input), fold(&input));
    assert_eq!(a, b);
    let k = karma(&a);
    assert!(a.participation.s.is_subset(&k.e));
    assert!(!k.counting.is_empty() && !k.k1.is_empty() && !k.k.is_empty());
    // the scenario establishes a non-seed: user 5 (restricted from vote, two seeds upvote them)
    let established: Vec<&String> = k.e.difference(&a.participation.s).collect();
    assert_eq!(established, [&did(5)]);
}
