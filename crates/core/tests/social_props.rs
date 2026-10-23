// SPDX-License-Identifier: AGPL-3.0-or-later
//! Property tests of the full social fold ([`fold_view`]): `social.md` §6.3 P1–P8, the extra
//! properties of §19 and the determinism rules of §1.3, on random forums whose records carry
//! canonical keys and genuine CIDs (`social_gen`: memberships, forum and author moderation,
//! posts, comments with cycles and missing parents, edits, opinions with every axis, deleted and
//! unsynced subjects, wikiRevisions, sequences).
//!
//! | Property | Statement checked (§6.3 unless noted) |
//! |---|---|
//! | P1 | every output is independent of the order of records: the vector JSON form with `records`, `absent`, `obs` and `repoStatus` shuffled folds to a byte-identical serialised view and `inputDigest`, under every query (so the visibility and ranking outputs too, §19); set-valued entries and object keys inside records, reordered, change no projected output |
//! | P2 | supplying the same `(uri, cid)` twice (and the same `absent` and `obs` entries) changes nothing; duplicated set entries inside records (MA8, OP4, PO7, WR2, §9.2) change no projected output |
//! | P3 | each stage reads only earlier stages: removing opinions leaves L0–L2 and V1–V5 unchanged, removing memberships leaves L0–L1, removing L5-only inputs leaves L0–L4, and moving `obs` (and `now` without `until`) changes only `t_rank` and the orderings (§1.4: time is read by L6 only). "One pass over V" is the linear scaling of `social_perf.rs` |
//! | P4 | adding or removing L5–L7-only inputs (author-scope modActions, the `reacts` and `tags` entries of opinions, wikiRevisions, sequences, opinions on wikiRevisions or sequences) never changes K1, `seedUp`, E, KP or K |
//! | P5 | per stage: removing voter u's opinions on recipient r's items (none on an absent subject) moves `K1[r]` by at most `pairCap` and no other K1; with E and the K1 basis unchanged, `KP[r]` by at most `pairCap × strong(max(K1[u], 0))` and no other KP; with E and the KP basis unchanged, `K[r]` by at most `pairCap × strong(max(KP[u], 0))` and no other K. The corollary: removing all opinions of u ∉ S (none on an absent subject) changes no K1, `seedUp` or E and moves each `KP[r]` by at most `pairCap × strong(max(K1[u], 0))` |
//! | P6 | s ∈ S ⇒ s ∈ E, whatever K1[s] is |
//! | P7 | I' = I ∪ R, R by DIDs ∉ E(I) ∪ A with hypotheses (a) and (b): K1, `seedUp`, E, KP, K, the score and agreement of every item of I, and the V1–V10 state (before the thread pass) of every item of I whose author wrote nothing in R are unchanged. No hypothesis (c): CO5 compares only a valid root (§20.1 item 51), so R may hold a record whose `forum` is not F at the root uri of a comment of I; `p7_holds_for_an_invalid_record_at_a_root_uri` pins the former counterexample (§20.1 item 49) |
//! | P8 | incremental equals batch: a stream of puts, edits, deletes and re-puts, applied by an incremental driver (today: the engine's rebuild-and-refold), ends in the view of the batch fold of the final record set; replaying a suffix (resume) and folding intermediate states change nothing |
//! | §19 | the thread cycle cut does not depend on walk order (and §11 steps 2–4 hold); the lens never changes fold outputs |
//! | §1.3 | folding twice, and folding a shuffled input, gives identical serialised views and `inputDigest`; the fold's sources use no hash maps, clocks, randomness or floats outside `hot` |
//! | §1.1 | the fold is total: records with mangled values (any JSON shape, fields removed or added, deep nesting) still fold, are Excluded and never counted when invalid, and keep P1; a mangled declaration is rejected exactly when it is invalid |
//!
//! P9 (native equals wasm) is `social_parity.rs`; the performance budget is `social_perf.rs`.
//! `generator_coverage_report` (ignored; `--ignored --nocapture`) prints how often the generated
//! forums reach each state, mod-log status and hypothesis.
//! Any namespace build: `cargo test -p lessdugin-core --test social_props` (cases:
//! `PROPTEST_CASES`).

mod social_gen;

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_core::social::full::karma::{Karma, strong};
use lessdugin_core::social::full::visibility::{Lensed, lens};
use lessdugin_core::social::full::{ForumView, Query};
use lessdugin_core::social::input::FoldInput;
use lessdugin_core::social::threads::{Node, Parent, ThreadComment, place};
use lessdugin_lexicon::ns::NS;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRng, TestRunner};
use serde_json::{Value, json};
use social_gen::{
    Forum, Kind, Rec, Rng, SMALL, Spec, USERS, build, check_validity, first_post, fold, queries,
    serialize, spec, user,
};

/// `cases` unless `PROPTEST_CASES` says otherwise.
fn cases(n: u32) -> Config {
    let n = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(n);
    Config::with_cases(n)
}

fn karma(v: &ForumView) -> &Karma {
    v.karma.as_ref().expect("karma is computed")
}

/// The fold of the whole forum, checked for generator slips.
fn fold_all(f: &Forum, q: &Query) -> (FoldInput, ForumView) {
    let input = f.input();
    let view = fold(&input, q);
    check_validity(f, &input, &view);
    assert!(
        view.pending()
            .iter()
            .all(|p| p.stub.starts_with("sequences::"))
    );
    (input, view)
}

/// Every projected field (`testdata/vectors/README.md`) as JSON values (objects compare as
/// maps, arrays in order).
fn projection(v: &ForumView) -> BTreeMap<&'static str, Value> {
    lessdugin_core::social::full::project::FIELDS
        .iter()
        .map(|f| {
            let x = lessdugin_core::social::full::project(v, f)
                .unwrap_or_else(|e| json!(format!("{e:?}")));
            (*f, x)
        })
        .collect()
}

/// The karma stages of §6.2.
fn stages(k: &Karma) -> impl PartialEq + core::fmt::Debug + '_ {
    (&k.k1, &k.seed_up, &k.e, &k.kp, &k.k)
}

fn get(m: &BTreeMap<String, i64>, k: &str) -> i64 {
    m.get(k).copied().unwrap_or(0)
}

/// Rebuilds `f` with `edit` applied to record values; `keep_cid` keeps each record's CID (the
/// fold treats CIDs as opaque, so the input stays well-formed).
fn edit_values(f: &Forum, keep_cid: bool, mut edit: impl FnMut(&Rec, &mut Value)) -> Forum {
    let mut g = f.clone();
    for r in &mut g.recs {
        let mut v = r.value.clone();
        edit(r, &mut v);
        if v != r.value {
            if !keep_cid {
                r.cid = lessdugin_atp::datamodel::record_cid(&v)
                    .unwrap()
                    .to_string();
            }
            r.value = v;
        }
    }
    g
}

/// Reorders every object's keys and the given set-valued arrays of a value, recursively.
fn scramble(v: &mut Value, rng: &mut Rng, sets: &[&str]) {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = std::mem::take(m).into_iter().collect();
            rng.shuffle(&mut entries);
            for (k, mut x) in entries {
                if sets.contains(&k.as_str())
                    && let Value::Array(a) = &mut x
                {
                    rng.shuffle(a);
                }
                scramble(&mut x, rng, sets);
                m.insert(k, x);
            }
        }
        Value::Array(a) => {
            for x in a {
                scramble(x, rng, sets);
            }
        }
        _ => {}
    }
}

/// The set-valued arrays of record values: post `tags` (PO7), opinion `reacts` and `tags`
/// (§9.2, OP4), `#restrict.capabilities` (MA8), `#lock.allowed` (§4.8), wikiRevision `parents`
/// (WR2).
const SETS: [&str; 5] = ["tags", "reacts", "capabilities", "allowed", "parents"];

proptest! {
    #![proptest_config(cases(48))]

    /// P1 (and the §19 visibility and ranking corollary): the order of records, `absent`, `obs`
    /// and `repoStatus` changes no output, under any query.
    #[test]
    fn p1_permutation_invariance(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let (input, _) = fold_all(&f, &Query::default());
        let mut j = f.json(&|_| true);
        let mut rng = Rng(seed);
        for key in ["records", "absent", "obs"] {
            rng.shuffle(j[key].as_array_mut().unwrap());
        }
        let mut status: Vec<(String, Value)> =
            j["repoStatus"].as_object().unwrap().clone().into_iter().collect();
        rng.shuffle(&mut status);
        j["repoStatus"] = Value::Object(status.into_iter().collect());
        let shuffled = FoldInput::from_vector_json(&j).unwrap();
        prop_assert_eq!(&shuffled, &input);
        for q in queries(first_post(&f).as_deref()) {
            let (a, b) = (fold(&input, &q), fold(&shuffled, &q));
            prop_assert_eq!(&a.input_digest, &b.input_digest);
            prop_assert!(serialize(&a) == serialize(&b), "query {:?}", q);
        }
    }

    /// P1 inside records: set-valued entries and object keys, reordered (CIDs kept), change no
    /// projected output.
    #[test]
    fn p1_entry_order_invariance(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let mut rng = Rng(seed);
        let g = edit_values(&f, true, |_, v| scramble(v, &mut rng, &SETS));
        for q in queries(first_post(&f).as_deref()) {
            let a = fold(&f.input(), &q);
            let b = fold(&g.input(), &q);
            prop_assert_eq!(projection(&a), projection(&b), "query {:?}", q);
        }
    }

    /// P2: the same `(uri, cid)` supplied twice (records, `absent` and `obs` entries) changes
    /// nothing.
    #[test]
    fn p2_idempotence(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let (input, view) = fold_all(&f, &Query::default());
        let mut j = f.json(&|_| true);
        let mut rng = Rng(seed);
        for key in ["records", "absent", "obs"] {
            let a = j[key].as_array_mut().unwrap();
            let dups: Vec<Value> = a.iter().filter(|_| rng.chance(1, 2)).cloned().collect();
            for d in dups {
                let at = rng.below(a.len() + 1);
                a.insert(at, d);
            }
        }
        let twice = FoldInput::from_vector_json(&j).unwrap();
        prop_assert_eq!(&twice, &input);
        prop_assert!(serialize(&fold(&twice, &Query::default())) == serialize(&view));
    }

    /// P2 inside records: duplicated set entries (restrict capabilities, tag votes, reacts,
    /// post tags, lock allow-lists, wiki parents) collapse; no projected output changes.
    #[test]
    fn p2_duplicate_entries(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let mut rng = Rng(seed);
        let g = edit_values(&f, true, |_, v| duplicate_entries(v, &mut rng));
        for q in queries(first_post(&f).as_deref()) {
            let a = fold(&f.input(), &q);
            let b = fold(&g.input(), &q);
            prop_assert_eq!(projection(&a), projection(&b), "query {:?}", q);
        }
    }

    /// P3: every stage reads only earlier stages (§1.4).
    #[test]
    fn p3_stages_read_only_earlier_stages(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let (_, a) = fold_all(&f, &Query::default());
        let mut rng = Rng(seed);

        // opinions are L3+ inputs: L0–L2, V1–V5 and the placement are unchanged without them
        let b = fold(&f.input_where(&|r| r.kind != Kind::Opinion), &Query::default());
        prop_assert_eq!(&a.authorities, &b.authorities);
        prop_assert_eq!(&a.moderation, &b.moderation);
        prop_assert_eq!(&a.tiers, &b.tiers);
        prop_assert_eq!(&a.mod_queue, &b.mod_queue);
        prop_assert_eq!(&a.raw.assertions, &b.raw.assertions);
        prop_assert_eq!(&a.participation, &b.participation);
        prop_assert_eq!(&a.v1_v5, &b.v1_v5);
        prop_assert_eq!(&a.locked_out, &b.locked_out);
        prop_assert_eq!(&a.threads, &b.threads);
        prop_assert_eq!(&a.times, &b.times);

        // memberships are L2 inputs: L0 and L1 are unchanged without them
        let c = fold(&f.input_where(&|r| r.kind != Kind::Membership), &Query::default());
        prop_assert_eq!(&a.authorities, &c.authorities);
        prop_assert_eq!(&a.moderation, &c.moderation);
        prop_assert_eq!(&a.tiers, &c.tiers);

        // L5-only inputs: L0–L4 are unchanged without (some of) them
        let drop = l5_subset(&f, &mut rng);
        let g = strip_entries(&f, &drop);
        let d = fold(&g.input_where(&|r| !drop.removed.contains(&r.uri)), &Query::default());
        prop_assert_eq!(&a.authorities, &d.authorities);
        prop_assert_eq!(&a.participation, &d.participation);
        // L1 reads the existence and CIDs of records (positive assertions, `#tag.head`, §4.5):
        // compare it on the subjects that are still there, `tag:` subjects aside
        let kept = |subject: &str| !subject.starts_with("tag:") && !drop.removed.contains(subject);
        let status = |v: &ForumView| v.moderation.forum_status.iter()
            .filter(|(u, _)| f.get(u).and_then(Rec::subject).is_some_and(kept))
            .map(|(u, st)| (u.clone(), *st))
            .collect::<BTreeMap<_, _>>();
        prop_assert_eq!(status(&a), status(&d));
        let resolved = |v: &ForumView| v.moderation.resolved.iter()
            .filter(|(s, _)| kept(s))
            .map(|(s, r)| (s.clone(), r.clone()))
            .collect::<BTreeMap<_, _>>();
        prop_assert_eq!(resolved(&a), resolved(&d));
        prop_assert_eq!(stages(karma(&a)), stages(karma(&d)));
        // scores of posts and comments (a wikiRevision's or sequence's own score reads the
        // opinions on it, which are L3.5 inputs for that item)
        let (sa, sd) = (a.scores.as_ref().unwrap(), d.scores.as_ref().unwrap());
        let thread_item = |u: &str| {
            u.contains(&format!("/{NS}.post/")) || u.contains(&format!("/{NS}.comment/"))
        };
        for (uri, x) in sd.iter().filter(|(u, _)| thread_item(u)) {
            prop_assert_eq!(Some(x), sa.get(uri), "score of {}", uri);
        }
        let (ga, gd) = (a.gates.as_ref().unwrap(), d.gates.as_ref().unwrap());
        for (did, x) in &gd.passes {
            prop_assert_eq!(Some(x), ga.passes.get(did), "gates of {}", did);
        }
        for (uri, x) in &d.v1_v5 {
            prop_assert_eq!(Some(x), a.v1_v5.get(uri), "V1–V5 of {}", uri);
        }

        // time (§2) is read by L6 only: moving first-seen times (and `now`, unless an `until`
        // reads it) changes no layer below the orderings
        let mut h = f.clone();
        for r in &mut h.recs {
            r.obs += (i64::try_from(rng.below(4 * 1440)).unwrap() - 2 * 1440) * social_gen::MINUTE;
        }
        let untimed = !f.recs.iter().any(|r| r.value.get("until").is_some());
        if untimed {
            h.now += i64::try_from(rng.below(200)).unwrap() * social_gen::DAY;
        }
        let e = fold(&h.input(), &Query::default());
        let mut e2 = e.clone();
        e2.times = a.times.clone();
        e2.orderings = a.orderings.clone();
        prop_assert!(serialize(&a) == serialize(&e2), "a layer below L6 read time");
    }

    /// P4: L5–L7-only inputs never change K1, seedUp, E, KP or K.
    #[test]
    fn p4_layering(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let (_, a) = fold_all(&f, &Query::default());
        let drop = l5_subset(&f, &mut Rng(seed));
        let g = strip_entries(&f, &drop);
        let b = fold(&g.input_where(&|r| !drop.removed.contains(&r.uri)), &Query::default());
        prop_assert_eq!(stages(karma(&a)), stages(karma(&b)));
    }

    /// P5, per stage.
    #[test]
    fn p5_stage_pair_bound(s in spec(SMALL), pick_u in 0..USERS, pick_r in 8..USERS) {
        check_p5(&s, pick_u, pick_r)?;
    }

    /// P5's corollary: removing every opinion of a voter u ∉ S (none on an absent subject)
    /// changes no K1, seedUp or E, and moves each KP[r] by at most
    /// `pairCap × strong(max(K1[u], 0))`.
    #[test]
    fn p5_corollary(s in spec(SMALL), pick in 3..USERS) {
        let f = build(&s);
        let (_, a) = fold_all(&f, &Query::default());
        let ka = karma(&a);
        let u = user(pick);
        let theirs: BTreeSet<String> = f.recs.iter()
            .filter(|x| x.kind == Kind::Opinion && x.author == u)
            .map(|x| x.uri.clone())
            .collect();
        let on_absent = theirs.iter().any(|x| ka.opinions.get(x).is_some_and(|o| o.absent));
        if a.participation.s.contains(&u) || on_absent || theirs.is_empty() {
            return Ok(());
        }
        let b = fold(&f.input_where(&|x| !theirs.contains(&x.uri)), &Query::default());
        let kb = karma(&b);
        prop_assert_eq!(&ka.k1, &kb.k1);
        prop_assert_eq!(&ka.seed_up, &kb.seed_up);
        prop_assert_eq!(&ka.e, &kb.e);
        let cap = a.policy.pair_cap * strong(ka.k1_of(&u).max(0));
        for r in ka.kp.keys().chain(kb.kp.keys()) {
            let d = (ka.kp_of(r) - kb.kp_of(r)).abs();
            prop_assert!(d <= cap, "KP[{}] moved by {} > {}", r, d, cap);
        }
    }

    /// P6: seeds stay in E whatever K1 says; E holds only seeds and established participants.
    #[test]
    fn p6_seed_persistence(s in spec(SMALL)) {
        let f = build(&s);
        let (_, v) = fold_all(&f, &Query::default());
        let (k, l2) = (karma(&v), &v.participation);
        prop_assert!(l2.s.is_subset(&k.e), "S ⊄ E: {:?}", l2.s.difference(&k.e).collect::<Vec<_>>());
        for u in k.e.difference(&l2.s) {
            prop_assert!(l2.p.contains(u) && !l2.b.contains(u) && !l2.n.contains(u));
            prop_assert!(k.k1_of(u) >= v.policy.establish);
        }
        if !s.open {
            prop_assert_eq!(&k.e, &l2.s);
        }
    }

    /// P7: records by DIDs outside E(I) ∪ A, under hypotheses (a) and (b), change no karma
    /// stage, no score or agreement of an item of I, and no V1–V10 state of an item of I whose
    /// author wrote nothing in R.
    #[test]
    fn p7_sybil_bound(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let mut rng = Rng(seed);
        let cand: BTreeSet<String> = f.recs.iter()
            .filter(|_| rng.chance(1, 3))
            .map(|r| r.uri.clone())
            .collect();
        let i = f.input_where(&|r| !cand.contains(&r.uri));
        let vi = fold(&i, &Query::default());
        let r = sybil_records(&f, &i, &vi, &cand);
        prop_assume!(!r.is_empty());
        let i2 = f.input_where(&|x| !cand.contains(&x.uri) || r.contains(&x.uri));
        let v2 = fold(&i2, &Query::default());
        check_p7(&f, &i, &vi, &v2, &r)?;
    }

    /// P8: an incremental driver ends in the batch fold of its final record set.
    #[test]
    fn p8_incremental_equals_batch(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let mut rng = Rng(seed);
        let stream = change_stream(&f, &mut rng);
        let mut driver = Refold::new(&f);
        let mut probes = 0;
        for (n, c) in stream.iter().enumerate() {
            driver.apply(c);
            // fold some intermediate states: no fold may leave state behind
            if rng.chance(1, 8) && probes < 3 {
                probes += 1;
                let _ = driver.view(&Query::default());
            }
            if n + 1 == stream.len() {
                // resume from an earlier cursor: the last events are applied again
                let k = rng.below(stream.len().min(12) + 1);
                for c in &stream[stream.len() - k..] {
                    driver.apply(c);
                }
            }
        }
        let batch = driver.batch_input();
        prop_assert_eq!(batch.records.len(), f.recs.len());
        for q in queries(first_post(&f).as_deref()).into_iter().take(3) {
            let inc = driver.view(&q);
            let bat = fold(&batch, &q);
            prop_assert!(serialize(&inc) == serialize(&bat), "query {:?}", q);
        }
        // the same record set folded from the generator's own input: only the first-seen
        // times differ (earlier versions were seen first), so only `t_rank` and the orderings
        let mut inc = driver.view(&Query::default());
        let own = fold(&f.input(), &Query::default());
        inc.times = own.times.clone();
        inc.orderings = own.orderings.clone();
        prop_assert!(serialize(&inc) == serialize(&own));
    }

    /// §19: the lens never changes fold outputs; it only collapses listed items of muted
    /// authors.
    #[test]
    fn lens_never_changes_fold_outputs(s in spec(SMALL), muted in proptest::collection::btree_set(0..USERS, 0..4)) {
        let f = build(&s);
        let (input, v) = fold_all(&f, &Query::default());
        let before = serialize(&v);
        let muted: BTreeSet<String> = muted.into_iter().map(user).collect();
        let vis = v.visibility.as_ref().unwrap();
        let l = lens(&vis.displayed, &muted);
        prop_assert!(serialize(&v) == before);
        prop_assert!(serialize(&fold(&input, &Query::default())) == before);
        prop_assert_eq!(l.len(), vis.displayed.len());
        for (uri, st) in &vis.displayed {
            let author = uri.split('/').nth(2).unwrap();
            let want = if st.listed() && muted.contains(author) {
                Lensed::Muted
            } else {
                Lensed::Fold(st.clone())
            };
            prop_assert_eq!(&l[uri], &want);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// P2 entries

fn duplicate_entries(v: &mut Value, rng: &mut Rng) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if SETS.contains(&k.as_str())
                    && let Value::Array(a) = x
                    && !a.is_empty()
                    && rng.chance(2, 3)
                {
                    let d = a[rng.below(a.len())].clone();
                    let at = rng.below(a.len() + 1);
                    a.insert(at, d);
                }
                duplicate_entries(x, rng);
            }
        }
        Value::Array(a) => {
            for x in a {
                duplicate_entries(x, rng);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// P3, P4: L5-only inputs

/// A random part of the L5–L7-only inputs of §6.3 P4.
struct L5Drop {
    /// Records left out: author-scope modActions, wikiRevisions, sequences, opinions on
    /// wikiRevisions or sequences, and opinions whose only axes were reacts and tags.
    removed: BTreeSet<String>,
    /// Opinions whose `reacts` and `tags` entries are stripped.
    stripped: BTreeSet<String>,
}

fn l5_subset(f: &Forum, rng: &mut Rng) -> L5Drop {
    let mut removed = BTreeSet::new();
    let mut stripped = BTreeSet::new();
    for r in &f.recs {
        let l5_record = matches!(
            r.kind,
            Kind::ModAuthor | Kind::WikiRevision | Kind::Sequence
        ) || r.opinion_on_l5_item();
        if l5_record && rng.chance(1, 2) {
            removed.insert(r.uri.clone());
        } else if r.kind == Kind::Opinion
            && (r.value.get("reacts").is_some() || r.value.get("tags").is_some())
            && rng.chance(1, 2)
        {
            if r.value.get("karma").is_none() && r.value.get("agreement").is_none() {
                removed.insert(r.uri.clone());
            } else {
                stripped.insert(r.uri.clone());
            }
        }
    }
    L5Drop { removed, stripped }
}

fn strip_entries(f: &Forum, drop: &L5Drop) -> Forum {
    edit_values(f, false, |r, v| {
        if drop.stripped.contains(&r.uri) {
            let m = v.as_object_mut().unwrap();
            m.remove("reacts");
            m.remove("tags");
        }
    })
}

// ---------------------------------------------------------------------------------------------
// P5

/// P5 for voter `pick_u` (among the voters on r's items) and recipient `pick_r`: the number of
/// stages whose hypotheses held and were checked (K1 always; KP when E and the K1 basis are
/// unchanged; K when the KP basis is too).
fn check_p5(s: &Spec, pick_u: usize, pick_r: usize) -> Result<usize, TestCaseError> {
    // users 8 and 9 cast no votes, so that their KP is no voter's basis (the K stage)
    let mut s = s.clone();
    for o in &mut s.ops {
        if o.voter >= 8 {
            o.voter -= 4;
        }
    }
    let f = build(&s);
    let (_, a) = fold_all(&f, &Query::default());
    let ka = karma(&a);
    let r = user(pick_r);
    // the voters on r's items, none of whose opinions on them names an absent subject
    let on_r = |x: &Rec| {
        x.kind == Kind::Opinion
            && x.subject()
                .is_some_and(|s| s.starts_with(&format!("at://{r}/")))
    };
    let mut voters: BTreeMap<&str, bool> = BTreeMap::new();
    for x in f.recs.iter().filter(|x| on_r(x)) {
        let absent = ka.opinions.get(&x.uri).is_some_and(|o| o.absent);
        *voters.entry(x.author.as_str()).or_insert(true) &= !absent;
    }
    let voters: Vec<&str> = voters
        .into_iter()
        .filter(|(_, ok)| *ok)
        .map(|(v, _)| v)
        .collect();
    if voters.is_empty() {
        return Ok(0);
    }
    let u = voters[pick_u % voters.len()].to_owned();
    let mine: BTreeSet<String> = f
        .recs
        .iter()
        .filter(|x| on_r(x) && x.author == u)
        .map(|x| x.uri.clone())
        .collect();
    let b = fold(
        &f.input_where(&|x| !mine.contains(&x.uri)),
        &Query::default(),
    );
    let kb = karma(&b);
    let pair_cap = a.policy.pair_cap;
    let dids: BTreeSet<&String> = ka
        .k1
        .keys()
        .chain(kb.k1.keys())
        .chain(ka.kp.keys())
        .chain(kb.kp.keys())
        .chain(ka.k.keys())
        .chain(kb.k.keys())
        .collect();
    let mut checked = 1;
    // K1 stage (basis None, Elig = S)
    for d in &dids {
        let delta = (get(&ka.k1, d) - get(&kb.k1, d)).abs();
        if *d == &r {
            prop_assert!(delta <= pair_cap, "K1[r] moved by {} > {}", delta, pair_cap);
        } else {
            prop_assert_eq!(delta, 0, "K1[{}] moved", d);
        }
    }
    // KP stage (basis K1, Elig = E): the basis of every voter is unchanged
    let votes = |k: &Karma, d: &str| {
        k.opinions
            .values()
            .any(|o| o.voter == d && o.karma.is_some())
    };
    let k1_basis_same = dids.iter().all(|d| get(&ka.k1, d) == get(&kb.k1, d)) || !votes(ka, &r);
    if ka.e == kb.e && k1_basis_same {
        checked += 1;
        let cap = pair_cap * strong(ka.k1_of(&u).max(0));
        for d in &dids {
            let delta = (get(&ka.kp, d) - get(&kb.kp, d)).abs();
            if *d == &r {
                prop_assert!(delta <= cap, "KP[r] moved by {} > {}", delta, cap);
            } else {
                prop_assert_eq!(delta, 0, "KP[{}] moved", d);
            }
        }
        // K stage (basis KP, Elig = E)
        let kp_basis_same = dids
            .iter()
            .filter(|d| **d != &r)
            .all(|d| get(&ka.kp, d) == get(&kb.kp, d))
            && (get(&ka.kp, &r) == get(&kb.kp, &r) || !votes(ka, &r));
        if kp_basis_same {
            checked += 1;
            let cap = pair_cap * strong(ka.kp_of(&u).max(0));
            for d in &dids {
                let delta = (get(&ka.k, d) - get(&kb.k, d)).abs();
                if *d == &r {
                    prop_assert!(delta <= cap, "K[r] moved by {} > {}", delta, cap);
                } else {
                    prop_assert_eq!(delta, 0, "K[{}] moved", d);
                }
            }
        }
    }
    Ok(checked)
}

// ---------------------------------------------------------------------------------------------
// P7

/// R: the candidates whose author is outside E(I) ∪ A, minus (a) items an E(I) voter of I has an
/// opinion on and (b) memberships of DIDs that I already qualifies for E.
///
/// Posts whose `forum` is not F at the root uri of a comment of I stay in R: CO5 compares only a
/// valid root (`social.md` §20.1 item 51), so hypothesis (c) of item 49 is withdrawn
/// ([`p7_holds_for_an_invalid_record_at_a_root_uri`]).
fn sybil_records(
    f: &Forum,
    i: &FoldInput,
    vi: &ForumView,
    cand: &BTreeSet<String>,
) -> BTreeSet<String> {
    let k = karma(vi);
    let voted_by_e: BTreeSet<&str> = i
        .records
        .keys()
        .filter_map(|u| f.get(u))
        .filter(|x| x.kind == Kind::Opinion && k.e.contains(&x.author))
        .filter_map(Rec::subject)
        .collect();
    let p = &vi.policy;
    cand.iter()
        .filter(|u| {
            let x = f.get(u).unwrap();
            !k.e.contains(&x.author)
                && !vi.authorities.contains_key(&x.author)
                && !voted_by_e.contains(u.as_str())
                && !(x.kind == Kind::Membership
                    && k.k1_of(&x.author) >= p.establish
                    && get(&k.seed_up, &x.author) >= p.establish_seeds)
        })
        .cloned()
        .collect()
}

fn check_p7(
    f: &Forum,
    i: &FoldInput,
    vi: &ForumView,
    v2: &ForumView,
    r: &BTreeSet<String>,
) -> Result<(), TestCaseError> {
    let (ka, kb) = (karma(vi), karma(v2));
    prop_assert_eq!(stages(ka), stages(kb), "a karma stage changed");
    let (sa, sb) = (vi.scores.as_ref().unwrap(), v2.scores.as_ref().unwrap());
    for (uri, x) in sa {
        let y = sb.get(uri);
        prop_assert_eq!(
            Some((x.score, x.agreement)),
            y.map(|y| (y.score, y.agreement)),
            "score of {}\n{}",
            uri,
            diagnose(f, uri, r, v2)
        );
    }
    let authors: BTreeSet<&str> = r
        .iter()
        .map(|u| f.get(u).unwrap().author.as_str())
        .collect();
    let (va, vb) = (
        vi.visibility.as_ref().unwrap(),
        v2.visibility.as_ref().unwrap(),
    );
    for uri in i.records.keys() {
        let x = f.get(uri).unwrap();
        if x.kind.is_item() && !authors.contains(x.author.as_str()) {
            prop_assert_eq!(va.states.get(uri), vb.states.get(uri), "state of {}", uri);
        }
    }
    Ok(())
}

/// What R did to `uri` (P7 failure reports).
fn diagnose(f: &Forum, uri: &str, r: &BTreeSet<String>, v2: &ForumView) -> String {
    let x = f.get(uri).unwrap();
    let mut out = format!(
        "item {uri} by {}\n  I' state: {:?}\n",
        x.author,
        v2.visibility.as_ref().unwrap().states.get(uri)
    );
    if let Some(root) = x.value.get("root").and_then(|r| r["uri"].as_str()) {
        let rr = f.get(root);
        out += &format!(
            "  root {root}: in R {}, meant valid {:?}, invalid in I' {:?}, forum {:?}\n",
            r.contains(root),
            rr.map(|r| r.valid),
            v2.raw.invalid.get(root),
            rr.map(|r| r.value["forum"].clone())
        );
    }
    for u in r {
        out += &format!("  R: {u} ({:?})\n", f.get(u).unwrap().kind);
    }
    out
}

/// The former P7 counterexample of `social.md` §20.1 item 49, which item 51 removes. In I, a
/// granted member's comment sits in the thread of a post that is missing (deleted by its author,
/// an outsider), and a moderator's upvote on it gives the member karma. The outsider publishes a
/// record at the root uri whose `forum` names another forum (invalid by G9, still a fold input).
/// I' = I ∪ R meets §6.3's hypotheses (the outsider ∉ E ∪ A; nobody voted on the root; no
/// membership). CO5 compares only a valid root, so the invalid record is a `Missing` root: the
/// comment keeps its state and score, and the vote still counts.
#[test]
fn p7_holds_for_an_invalid_record_at_a_root_uri() {
    let decl = json!({
        "name": "Test forum",
        "membership": "open",
        "moderators": [{"did": user(1), "role": "admin"}, {"did": user(2), "role": "moderator"}],
    });
    let mut f = Forum::new(decl, social_gen::T0 + 30 * social_gen::DAY);
    let (owner, member, outsider) = (user(0), user(3), user(9));
    f.membership(&member);
    let access = json!({"$type": format!("{NS}.modAction#access"), "level": "member"});
    f.mod_action(&owner, false, &member, None, access, None, None, 1);
    // the outsider's post, deleted: not in I
    let root = f.ranged_uri(&outsider, "post", 10);
    let gone = f.post_value(10, 0, 0, 0, "A post");
    f.ghost(&root, &gone, false);
    let root_cid = f.cid_of(&root).unwrap().to_owned();
    let c = f.comment_uri(&member, &root, 20);
    let cv = f.comment_value(20, (&root, &root_cid), None, "A comment", false);
    f.put(&c, Kind::Comment, cv, true, social_gen::Forum::seen(20));
    let c_cid = f.cid_of(&c).unwrap().to_owned();
    let mut up = serde_json::Map::new();
    up.insert("karma".into(), json!("up"));
    f.opinion(&user(2), &c, &c_cid, up, 30);
    // R: an invalid record at the root uri (G9: `forum` names another forum)
    let bad = f.post_value(40, 0, 0, 2, "A post");
    f.put(&root, Kind::Post, bad, false, social_gen::Forum::seen(40));

    let i = f.input_where(&|r| r.uri != root);
    let vi = fold(&i, &Query::default());
    let r: BTreeSet<String> = [root.clone()].into();
    assert!(
        !karma(&vi).e.contains(&outsider) && !vi.authorities.contains_key(&outsider),
        "the hypotheses of §6.3 P7 hold"
    );
    let i2 = f.input();
    let vi2 = fold(&i2, &Query::default());
    assert!(vi2.raw.invalid.contains_key(&root), "R is invalid (G9)");
    assert_eq!(karma(&vi).k_of(&member), 1);
    assert_eq!(
        vi2.visibility.as_ref().unwrap().states.get(&c),
        Some(&lessdugin_core::social::full::State::Visible)
    );
    // the generated property keeps such a record in R, and P7 holds for it
    assert_eq!(sybil_records(&f, &i, &vi, &r), r);
    check_p7(&f, &i, &vi, &vi2, &r).unwrap();
    // the comment stays in the thread, under the Missing root
    let t = &vi2.threads[&root];
    assert!(t.root_missing);
    assert_eq!(t.parent.get(&c), Some(&Parent::Root));
}

// ---------------------------------------------------------------------------------------------
// P8

/// One change the sync layer applies to its record set (`sync.md` §5–§6): a put of a version,
/// or a delete (a tombstone, which confirms the uri's absence).
#[derive(Clone, Debug)]
enum Change {
    Put(Rec),
    Delete(String),
}

/// The incremental driver under test. Today the engine rebuilds its model and re-runs the
/// whole fold after each `Changed` event (`model::build`, `sync.md` §13), which [`Refold`]
/// reproduces; an incremental implementation that re-runs only the affected layers plugs in
/// here, and the property then checks it against the batch fold.
trait Incremental {
    fn apply(&mut self, change: &Change);
    fn view(&mut self, q: &Query) -> ForumView;
}

/// The record set a stream of changes leaves, with first-seen times per version.
struct Refold {
    forum: Forum,
    live: BTreeMap<String, Rec>,
    tombstones: BTreeSet<String>,
    obs: BTreeMap<(String, String), i64>,
    clock: i64,
}

impl Refold {
    fn new(f: &Forum) -> Self {
        let mut forum = f.clone();
        forum.clear();
        Self {
            forum,
            live: BTreeMap::new(),
            tombstones: BTreeSet::new(),
            obs: BTreeMap::new(),
            clock: social_gen::T0,
        }
    }

    /// The contract input of the current record set (§1.1): `absent` = tombstoned uris named
    /// by a live opinion; `obs` = the first-seen time of every version seen, the minimum per
    /// uri being `t_obs`.
    fn batch_input(&self) -> FoldInput {
        let mut f = self.forum.clone();
        f.deleted = self.tombstones.clone();
        for r in self.live.values() {
            f.put(&r.uri, r.kind, r.value.clone(), r.valid, r.obs);
        }
        let mut j = f.json(&|_| true);
        let obs: Vec<Value> = self
            .obs
            .iter()
            .filter(|((u, _), _)| self.live.contains_key(u) || *u == f.uri)
            .map(|((u, c), t)| json!({"uri": u, "cid": c, "us": t}))
            .chain([json!({"uri": f.uri, "cid": f.decl_cid, "us": social_gen::T0})])
            .collect();
        j["obs"] = json!(obs);
        FoldInput::from_vector_json(&j).unwrap()
    }
}

impl Incremental for Refold {
    fn apply(&mut self, change: &Change) {
        self.clock += social_gen::MINUTE;
        match change {
            Change::Put(r) => {
                let t = *self
                    .obs
                    .entry((r.uri.clone(), r.cid.clone()))
                    .or_insert(self.clock);
                let mut r = r.clone();
                r.obs = t;
                self.tombstones.remove(&r.uri);
                self.live.insert(r.uri.clone(), r);
            }
            Change::Delete(u) => {
                self.live.remove(u);
                self.tombstones.insert(u.clone());
            }
        }
    }

    fn view(&mut self, q: &Query) -> ForumView {
        fold(&self.batch_input(), q)
    }
}

/// A random stream that ends in `f`'s record set: earlier versions put first (edits), records
/// deleted and put again, transient records put and deleted, in a random interleaving that keeps
/// each uri's own changes in order.
fn change_stream(f: &Forum, rng: &mut Rng) -> Vec<Change> {
    let mut per_uri: Vec<Vec<Change>> = Vec::new();
    for r in &f.recs {
        let mut ch = Vec::new();
        if rng.chance(1, 6) {
            let mut old = r.clone();
            old.value["createdAt"] = json!(social_gen::at(1));
            old.cid = lessdugin_atp::datamodel::record_cid(&old.value)
                .unwrap()
                .to_string();
            ch.push(Change::Put(old));
        }
        if rng.chance(1, 10) {
            ch.push(Change::Put(r.clone()));
            ch.push(Change::Delete(r.uri.clone()));
        }
        ch.push(Change::Put(r.clone()));
        per_uri.push(ch);
    }
    // deleted items that were once seen (their absence is confirmed by the tombstone)
    for u in &f.deleted {
        let rec = Rec {
            uri: u.clone(),
            cid: f.ghost_cids[u].clone(),
            value: json!({"$type": format!("{NS}.post"), "forum": f.uri, "title": "gone", "createdAt": social_gen::at(0)}),
            author: u.split('/').nth(2).unwrap().to_owned(),
            kind: Kind::Post,
            valid: true,
            obs: 0,
        };
        if rng.chance(1, 2) {
            per_uri.push(vec![Change::Put(rec), Change::Delete(u.clone())]);
        } else {
            per_uri.push(vec![Change::Delete(u.clone())]);
        }
    }
    // interleave
    let mut out = Vec::new();
    let mut heads: Vec<usize> = vec![0; per_uri.len()];
    let mut open: Vec<usize> = (0..per_uri.len()).collect();
    while !open.is_empty() {
        let k = rng.below(open.len());
        let i = open[k];
        out.push(per_uri[i][heads[i]].clone());
        heads[i] += 1;
        if heads[i] == per_uri[i].len() {
            open.swap_remove(k);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// §19: thread placement

fn comment_uri(i: usize) -> String {
    format!("at://{}/{NS}.comment/k{i:04}", user(3 + i % 5))
}

proptest! {
    #![proptest_config(cases(256))]

    /// §11 steps 2–4 and §19 "the thread cycle cut is independent of walk order": any input
    /// order gives the same placement; each cycle loses the parent edge of its greatest uri;
    /// every other comment keeps its pointer; placeholders are the non-node parent uris.
    #[test]
    fn thread_cycle_cut_independent_of_walk_order(
        n in 1usize..40,
        ptrs in proptest::collection::vec((0u8..4, 0usize..64), 40),
        root_present in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let root = format!("at://{}/{NS}.post/p", user(1));
        let mut cs: Vec<ThreadComment> = (0..n)
            .map(|i| {
                let (k, j) = ptrs[i];
                let parent = match k {
                    0 => None,
                    1 => Some(format!("at://{}/{NS}.comment/ghost{}", user(2), j % 3)),
                    _ => Some(comment_uri(j % n)),
                };
                ThreadComment { uri: comment_uri(i), parent }
            })
            .collect();
        let a = place(&root, root_present, &cs);
        let mut rng = Rng(seed);
        rng.shuffle(&mut cs);
        // duplicates are ignored
        if let Some(d) = cs.first().cloned() {
            cs.push(d);
        }
        let b = place(&root, root_present, &cs);
        prop_assert_eq!(&a, &b);

        let ptr: BTreeMap<String, Option<String>> =
            cs.iter().map(|c| (c.uri.clone(), c.parent.clone())).collect();
        // the cycles of the pointer graph over nodes, by their greatest member
        let mut cut: BTreeSet<String> = BTreeSet::new();
        for start in ptr.keys() {
            let mut path: Vec<&String> = vec![start];
            let mut cur = start;
            while let Some(Some(p)) = ptr.get(cur) {
                if !ptr.contains_key(p) {
                    break;
                }
                if let Some(pos) = path.iter().position(|x| *x == p) {
                    cut.insert(path[pos..].iter().max().unwrap().to_string());
                    break;
                }
                path.push(p);
                cur = p;
            }
        }
        prop_assert_eq!(&a.orphaned, &cut);
        let mut missing = BTreeSet::new();
        for (c, p) in &ptr {
            let want = match p {
                _ if cut.contains(c) => Parent::Root,
                None => Parent::Root,
                Some(p) if ptr.contains_key(p) => Parent::Comment(p.clone()),
                Some(p) => {
                    missing.insert(p.clone());
                    Parent::Missing(p.clone())
                }
            };
            prop_assert_eq!(a.parent.get(c), Some(&want), "parent of {}", c);
            let d = match &want {
                Parent::Root => 1,
                Parent::Missing(_) => 2,
                Parent::Comment(p) => a.depth[p] + 1,
            };
            prop_assert_eq!(a.depth.get(c), Some(&d), "depth of {}", c);
        }
        prop_assert_eq!(&a.missing, &missing);
        // every comment appears once, top-down
        let listed: Vec<String> = a.top_down().iter()
            .filter_map(|n| match n { Node::Comment(u) => Some(u.clone()), _ => None })
            .collect();
        prop_assert_eq!(listed.len(), ptr.len());
    }
}

/// §11 step 4 with any input order: in a chain longer than 1,000 the comment at depth 1,001 is
/// re-attached to the root, and its subtree continues from depth 1.
#[test]
fn thread_depth_cap_any_order() {
    let root = format!("at://{}/{NS}.post/p", user(1));
    let mut rng = Rng(7);
    for n in [999usize, 1_000, 1_001, 1_002, 1_337] {
        let mut cs: Vec<ThreadComment> = (0..n)
            .map(|i| ThreadComment {
                uri: comment_uri(i),
                parent: (i > 0).then(|| comment_uri(i - 1)),
            })
            .collect();
        rng.shuffle(&mut cs);
        let p = place(&root, true, &cs);
        let want: BTreeSet<String> = (n > 1_000)
            .then(|| comment_uri(1_000))
            .into_iter()
            .collect();
        assert_eq!(p.orphaned, want, "chain of {n}");
        assert_eq!(p.max_depth(), u32::try_from(n.min(1_000)).unwrap());
        if n > 1_000 {
            assert_eq!(
                p.depth[&comment_uri(n - 1)],
                u32::try_from(n - 1_000).unwrap()
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// §1.3 determinism

/// A spec drawn from a fixed seed.
fn spec_from_seed(seed: u8, size: social_gen::Size) -> Spec {
    use proptest::strategy::ValueTree;
    let rng = TestRng::from_seed(proptest::test_runner::RngAlgorithm::ChaCha, &[seed; 32]);
    let mut runner = TestRunner::new_with_rng(Config::default(), rng);
    spec(size).new_tree(&mut runner).unwrap().current()
}

/// §1.3: the fold twice, and the fold of a shuffled input, give identical serialised views and
/// `inputDigest`s (no hash-map order, clock or randomness reaches the output).
#[test]
fn determinism_fold_twice_and_shuffled() {
    let size = social_gen::Size {
        posts: 20,
        comments: 60,
        wikis: 8,
        sequences: 3,
        mods: 30,
        ops: 300,
    };
    for seed in 0..6u8 {
        let f = build(&spec_from_seed(seed, size));
        let input = f.input();
        let q = Query::default();
        let first = fold(&input, &q);
        check_validity(&f, &input, &first);
        let reference = serialize(&first);
        for round in 0..3u64 {
            let mut j = f.json(&|_| true);
            let mut rng = Rng(round * 31 + u64::from(seed));
            for key in ["records", "absent", "obs"] {
                rng.shuffle(j[key].as_array_mut().unwrap());
            }
            let v = fold(&FoldInput::from_vector_json(&j).unwrap(), &q);
            assert_eq!(v.input_digest, first.input_digest, "seed {seed}");
            assert!(serialize(&v) == reference, "seed {seed} round {round}");
            // a clone in another thread (fresh thread-locals)
            let input2 = input.clone();
            let w = std::thread::spawn(move || serialize(&fold(&input2, &Query::default())))
                .join()
                .unwrap();
            assert!(w == reference, "seed {seed}: another thread");
        }
        assert_eq!(first.input_digest.len(), 64);
    }
}

/// §1.3 rules 1–3 in the fold's sources: ordered collections only, no clock or randomness, and
/// floats only in `hot` (`social/rank.rs`, through `libm`).
#[test]
fn determinism_rules_in_sources() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/social");
    let mut files = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    assert!(files.len() > 20, "the fold's sources: {files:?}");
    let banned = [
        "HashMap",
        "HashSet",
        "SystemTime",
        "Instant",
        "rand::",
        "getrandom",
        "thread_rng",
        "powf",
        "powi",
        "mul_add",
        "partial_cmp",
    ];
    for p in files {
        let text = std::fs::read_to_string(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            for b in banned {
                assert!(
                    !code.contains(b),
                    "{name}:{}: `{b}` (social.md §1.3)",
                    n + 1
                );
            }
            if name != "rank.rs" {
                for b in ["f64", "f32"] {
                    assert!(!code.contains(b), "{name}:{}: `{b}` outside hot()", n + 1);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Generator coverage

/// How often the generated forums reach each kind of outcome (run with `--ignored
/// --nocapture`): a property is only as strong as the cases it sees.
#[test]
#[ignore = "a report, not a check"]
fn generator_coverage_report() {
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut bump = |k: &str, n: usize| *tally.entry(k.to_owned()).or_insert(0) += n;
    let n = 250u8;
    let mut recs = 0;
    for seed in 0..n {
        let spec = spec_from_seed(seed, SMALL);
        let f = build(&spec);
        let input = f.input();
        recs += input.records.len();
        let v = fold(&input, &Query::default());
        check_validity(&f, &input, &v);
        let k = karma(&v);
        let l2 = &v.participation;
        bump("forums", 1);
        bump("forums with E ∖ S ≠ ∅", usize::from(k.e.len() > l2.s.len()));
        bump(
            "seeds with K1 < 0",
            l2.s.iter().filter(|s| k.k1_of(s) < 0).count(),
        );
        bump("forums degraded", usize::from(v.degraded));
        bump("absent subjects", input.absent.len());
        bump("counting opinions", k.counting.len());
        bump("invalid records", v.raw.invalid.len());
        let vis = v.visibility.as_ref().unwrap();
        for st in vis.displayed.values() {
            let name = match st {
                lessdugin_core::social::full::State::Hidden(r) => format!("Hidden({})", r.name()),
                lessdugin_core::social::full::State::Tombstone(r) => {
                    format!("Tombstone({})", r.name())
                }
                lessdugin_core::social::full::State::Collapsed(r) => {
                    format!("Collapsed({})", r.name())
                }
                other => format!("{other:?}"),
            };
            bump(&format!("state {name}"), 1);
        }
        let a = v.author.as_ref().unwrap();
        bump("author-moderated comments", a.moderated.len());
        bump("author locks", a.locks.len());
        bump("author thread pins", a.thread_pins.len());
        for e in v.mod_log.as_ref().unwrap() {
            bump(&format!("mod log {}", e.status.name()), 1);
        }
        bump(
            "react tallies",
            v.reacts.as_ref().unwrap().values().map(Vec::len).sum(),
        );
        let w = v.wiki.as_ref().unwrap();
        bump(
            "wiki pages with a head",
            w.pages.values().filter(|p| p.canonical.is_some()).count(),
        );
        bump("applied tags", w.applies.values().map(BTreeSet::len).sum());
        bump(
            "orphaned comments (cycles)",
            v.threads.values().map(|t| t.orphaned.len()).sum(),
        );
        bump(
            "missing placeholders",
            v.threads.values().map(|t| t.missing.len()).sum(),
        );
        bump("mod-queue items", v.mod_queue.len());
        // P7: the size of R
        let mut rng = Rng(u64::from(seed));
        let cand: BTreeSet<String> = f
            .recs
            .iter()
            .filter(|_| rng.chance(1, 3))
            .map(|r| r.uri.clone())
            .collect();
        let i = f.input_where(&|r| !cand.contains(&r.uri));
        let vi = fold(&i, &Query::default());
        let r = sybil_records(&f, &i, &vi, &cand);
        bump("P7 |R|", r.len());
        // the former hypothesis (c) (§20.1 items 49, 51): R records at a comment's root uri
        // whose `forum` is not F
        let roots: BTreeSet<&str> = i
            .records
            .values()
            .filter_map(|x| x.value.get("root")?.get("uri")?.as_str())
            .collect();
        bump(
            "P7 R records at a root uri of I naming another forum",
            r.iter()
                .filter(|u| {
                    roots.contains(u.as_str())
                        && f.get(u).unwrap().value["forum"].as_str() != Some(f.uri.as_str())
                })
                .count(),
        );
        bump(
            "P7 R records by DIDs of I",
            r.iter()
                .filter(|u| {
                    let a = &f.get(u).unwrap().author;
                    i.records
                        .keys()
                        .any(|x| x.starts_with(&format!("at://{a}/")))
                })
                .count(),
        );
    }
    let mut p5 = [0usize; 4];
    for seed in 0..n {
        let spec = spec_from_seed(seed, SMALL);
        for (u, r) in [(0, 8), (1, 9), (3, 8), (5, 9)] {
            p5[check_p5(&spec, u, r).unwrap()] += 1;
        }
    }
    println!("P5 runs by stages checked (0 = no voter on r): {p5:?}");
    // P3: how often moving first-seen times moves some t_rank (else the check is vacuous)
    let mut moved = 0;
    for seed in 0..n {
        let f = build(&spec_from_seed(seed, SMALL));
        let a = fold(&f.input(), &Query::default());
        let mut h = f.clone();
        let mut rng = Rng(u64::from(seed));
        for r in &mut h.recs {
            r.obs += (i64::try_from(rng.below(4 * 1440)).unwrap() - 2 * 1440) * social_gen::MINUTE;
        }
        moved += usize::from(fold(&h.input(), &Query::default()).times != a.times);
    }
    println!("P3 obs shifts that moved some t_rank: {moved} of {n}");
    println!("{recs} records in {n} forums");
    for (k, v) in &tally {
        println!("{v:>8}  {k}");
    }
}

// ---------------------------------------------------------------------------------------------
// Totality

/// A replacement value for a field of a record (any JSON shape, near-miss strings, deep nesting).
fn junk(rng: &mut Rng, f: &Forum) -> Value {
    let deep = (0..40).fold(json!(1), |v, _| json!([v]));
    let choices = [
        Value::Null,
        json!(true),
        json!(0),
        json!(-1),
        json!(i64::MAX),
        json!(1.5),
        json!(""),
        json!("x"),
        json!("2026-13-45T99:99:99Z"),
        json!(format!("at://{}/{NS}.post/not-a-key", user(4))),
        json!(f.uri),
        json!(user(5)),
        json!("tag:"),
        json!([]),
        json!([null, 1, "a"]),
        json!({}),
        json!({"uri": 5, "cid": []}),
        json!({"uri": f.uri, "cid": "not-a-cid"}),
        deep,
    ];
    choices[rng.below(choices.len())].clone()
}

/// Replaces, removes or adds fields at random depths of `v`.
fn mangle(v: &mut Value, rng: &mut Rng, f: &Forum, depth: usize) {
    match v {
        Value::Object(m) => {
            let keys: Vec<String> = m.keys().cloned().collect();
            if keys.is_empty() || rng.chance(1, 6) {
                m.insert("extra".into(), junk(rng, f));
                return;
            }
            let k = &keys[rng.below(keys.len())];
            match rng.below(4) {
                0 => {
                    m.remove(k);
                }
                1 if depth < 4 => mangle(m.get_mut(k).unwrap(), rng, f, depth + 1),
                _ => {
                    m.insert(k.clone(), junk(rng, f));
                }
            }
        }
        Value::Array(a) if !a.is_empty() && depth < 4 => {
            let i = rng.below(a.len());
            mangle(&mut a[i], rng, f, depth + 1);
        }
        _ => *v = junk(rng, f),
    }
}

proptest! {
    #![proptest_config(cases(96))]

    /// §1.1 "invalid records are members": the fold is total. Records whose values are mangled
    /// (fields replaced by any JSON shape, removed or added, at any depth; keys, CIDs and the
    /// declaration kept) still fold, never panic, are Excluded (or keep counting) as their
    /// validity says, and P1 still holds.
    #[test]
    fn fold_is_total_on_mangled_records(s in spec(SMALL), seed in any::<u64>()) {
        let f = build(&s);
        let mut rng = Rng(seed);
        let mut g = f.clone();
        for r in &mut g.recs {
            if rng.chance(1, 4) {
                for _ in 0..1 + rng.below(3) {
                    mangle(&mut r.value, &mut rng, &f, 0);
                }
                if !r.value.is_object() {
                    r.value = json!({});
                }
            }
        }
        // a mangled declaration: the fold either rejects it (an invalid declaration makes the
        // input malformed, §1.1) or folds it; it never panics
        if rng.chance(1, 3) {
            let mut h = g.clone();
            mangle(&mut h.decl, &mut rng, &f, 1);
            let v = lessdugin_core::social::full::fold_view(&h.input(), &Query::default());
            let (ok, _) = lessdugin_core::social::full::lexicon_validity(&h.uri, &h.decl);
            prop_assert_eq!(v.is_ok(), ok, "{}", h.decl);
        }
        let input = g.input();
        let v = lessdugin_core::social::full::fold_view(&input, &Query::default());
        let v = match v {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::fail(format!("the fold rejected a well-formed input: {e}"))),
        };
        // every invalid candidate item is Excluded, and never counted, scored or listed
        let k = karma(&v);
        let vis = v.visibility.as_ref().unwrap();
        for uri in v.raw.invalid.keys() {
            if g.get(uri).is_some_and(|r| r.kind.is_item()) {
                prop_assert_eq!(vis.states.get(uri), Some(&lessdugin_core::social::full::State::Excluded));
                prop_assert!(!v.scores.as_ref().unwrap().contains_key(uri));
            }
            prop_assert!(!k.counting.contains(uri));
        }
        // P1 on such inputs
        let mut j = g.json(&|_| true);
        rng.shuffle(j["records"].as_array_mut().unwrap());
        let w = fold(&FoldInput::from_vector_json(&j).unwrap(), &Query::default());
        prop_assert!(serialize(&v) == serialize(&w));
        for q in queries(first_post(&g).as_deref()).into_iter().skip(1).step_by(2) {
            let _ = fold(&input, &q);
        }
    }
}
