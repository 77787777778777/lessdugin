// SPDX-License-Identifier: AGPL-3.0-or-later
//! Performance of the full social fold ([`fold_view`]): a synthetic forum of 1,000 posts,
//! 10,000 comments and 50,000 opinions (plus 1,000 memberships, modActions of both scopes,
//! edits, wikiRevisions and sequences, every record valid under its canonical key) folds within
//! a recorded budget, and the fold time grows linearly with the input (`social.md` §6.3 P3:
//! each stage makes one pass over V).
//!
//! Recorded on the development container (62,296 records, one fold, best of three in release):
//!
//! | Build | Full forum | Magic posts + comments of the largest thread | Half forum | Full / half |
//! |---|---|---|---|---|
//! | release | 1.69 s | 1.66 s | 0.77 s | 2.19 |
//! | debug (test profile) | 8.1 s | 8.0 s | 3.8 s | 2.16 |
//!
//! About 0.67 s of the release time is record validity (`data-model.md` §7, recomputed by
//! [`fold_view`] for every record; the engine's store keeps it per row), 0.25 s the skeletons,
//! 0.28 s L3 (50,000 opinions through three stages); every other layer takes under 0.1 s.
//! The budgets ([`BUDGET_RELEASE`], [`BUDGET_DEBUG`]) are about 3× and 7× those times; a run
//! prints its own (`-- --nocapture`). For release numbers:
//! `cargo test -p lessdugin-core --release --test social_perf -- --nocapture`.

mod social_gen;

use std::collections::BTreeMap;
use std::time::Duration;

use lessdugin_core::social::full::query::{CommentQuery, CommentSort, PostQuery, PostSort};
use lessdugin_core::social::full::{ForumView, Query, fold_view};
use lessdugin_core::social::input::FoldInput;
use lessdugin_lexicon::ns::NS;
use serde_json::{Map, json};
use social_gen::{DAY, Forum, Kind, MAX_MINUTE, Rng, T0, at, user};

/// The budget of one fold of the full-size forum in an optimised build (recorded: 1.69 s).
const BUDGET_RELEASE: Duration = Duration::from_secs(5);
/// The budget in an unoptimised (test profile) build (recorded: 8.1 s).
const BUDGET_DEBUG: Duration = Duration::from_secs(60);

/// The sizes of a synthetic forum.
#[derive(Clone, Copy, Debug)]
struct Scale {
    users: usize,
    posts: usize,
    comments: usize,
    opinions: usize,
}

const FULL: Scale = Scale {
    users: 1_000,
    posts: 1_000,
    comments: 10_000,
    opinions: 50_000,
};

/// A deterministic synthetic forum of `scale`.
fn forum(scale: Scale, seed: u64) -> Forum {
    let mut rng = Rng(seed);
    let decl = json!({
        "name": "A large forum",
        "membership": "open",
        "moderators": [{"did": user(1), "role": "admin"}, {"did": user(2), "role": "moderator"}],
        "policy": {"voting": "reacts"},
    });
    let mut f = Forum::new(decl, T0 + 30 * DAY);
    let users: Vec<String> = (0..scale.users).map(user).collect();
    let owner = users[0].clone();
    let ty = |n: &str| format!("{NS}.modAction#{n}");
    for (i, u) in users.iter().enumerate() {
        f.membership(u);
        if i >= 3 && i % 16 == 0 {
            let a = json!({"$type": ty("access"), "level": "member"});
            f.mod_action(&owner, false, u, None, a, None, None, 1);
        }
        if i >= 3 && i % 199 == 0 {
            let a = json!({"$type": ty("access"), "level": "banned"});
            f.mod_action(&users[1], false, u, None, a, None, None, 2);
        }
        if i >= 3 && i % 211 == 0 {
            let a = json!({"$type": ty("restrict"), "capabilities": ["vote"]});
            f.mod_action(&users[2], false, u, None, a, None, None, 3);
        }
    }
    // posts, some edited
    let mut posts: Vec<(String, String)> = Vec::new();
    for _ in 0..scale.posts {
        let a = &users[rng.below(users.len())];
        let minute = i64::try_from(rng.below(usize::try_from(MAX_MINUTE / 2).unwrap())).unwrap();
        let uri = f.ranged_uri(a, "post", minute);
        let kind = if rng.chance(1, 10) { 1 } else { 0 };
        let tags = u8::try_from(rng.below(4)).unwrap();
        let v = f.post_value(minute, kind, tags, 0, "A post");
        if rng.chance(1, 20) {
            let mut new = f.post_value(minute, kind, tags, 0, "A post, edited");
            new["updatedAt"] = json!(at(minute + 60));
            f.put(&uri, Kind::Post, new, true, Forum::seen(minute));
            f.revision(a, &uri, &v, minute + 60);
        } else {
            f.put(&uri, Kind::Post, v, true, Forum::seen(minute));
        }
        let cid = f.cid_of(&uri).unwrap().to_owned();
        posts.push((uri, cid));
    }
    // comments: 70% replies to an earlier comment of the same thread
    let mut threads: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
    let mut comments: Vec<(String, String)> = Vec::new();
    for _ in 0..scale.comments {
        let p = rng.below(posts.len());
        let a = &users[rng.below(users.len())];
        let minute = i64::try_from(rng.below(usize::try_from(MAX_MINUTE).unwrap())).unwrap();
        let (root, root_cid) = posts[p].clone();
        let uri = f.comment_uri(a, &root, minute);
        let siblings = threads.entry(p).or_default();
        let parent = (!siblings.is_empty() && rng.chance(7, 10))
            .then(|| siblings[rng.below(siblings.len())].clone());
        let pref = parent.as_ref().map(|(u, c)| (u.as_str(), c.as_str()));
        let v = f.comment_value(minute, (&root, &root_cid), pref, "A comment", false);
        f.put(&uri, Kind::Comment, v, true, Forum::seen(minute));
        let cid = f.cid_of(&uri).unwrap().to_owned();
        siblings.push((uri.clone(), cid.clone()));
        comments.push((uri, cid));
    }
    // wikiRevisions: linear chains over five slugs; sequences
    let mut heads: BTreeMap<usize, (String, String)> = BTreeMap::new();
    for i in 0..30 {
        let slug = i % 5;
        let a = &users[3 + rng.below(20)];
        let uri = f.ranged_uri(a, "wikiRevision", 100 + i64::try_from(i).unwrap());
        let mut v = json!({
            "$type": format!("{NS}.wikiRevision"),
            "forum": f.uri,
            "slug": format!("tag{slug}"),
            "name": format!("Tag {slug}"),
            "createdAt": at(100),
        });
        if let Some((pu, pc)) = heads.get(&slug) {
            v["parents"] = json!([{"uri": pu, "cid": pc}]);
        }
        f.put(&uri, Kind::WikiRevision, v, true, Forum::seen(100));
        let cid = f.cid_of(&uri).unwrap().to_owned();
        heads.insert(slug, (uri, cid));
    }
    for i in 0..5 {
        let a = &users[3 + i];
        let uri = f.ranged_uri(a, "sequence", 200);
        let chapter: Vec<&str> = posts
            .iter()
            .skip(i * 7)
            .take(5)
            .map(|p| p.0.as_str())
            .collect();
        let v = json!({
            "$type": format!("{NS}.sequence"),
            "forum": f.uri,
            "title": "A sequence",
            "chapters": [{"posts": chapter}],
            "createdAt": at(200),
        });
        f.put(&uri, Kind::Sequence, v, true, Forum::seen(200));
    }
    // forum-scope moderation: hides, keeps, tiers, pins, locks
    for i in 0..100 {
        let actor = &users[1 + i % 2];
        let (subject, cid) = if i % 2 == 0 {
            comments[rng.below(comments.len())].clone()
        } else {
            posts[rng.below(posts.len())].clone()
        };
        let action = match i % 5 {
            0 => json!({"$type": ty("hide"), "spam": i % 10 == 0}),
            1 if i % 2 == 1 => json!({"$type": ty("tier"), "tier": "frontpage"}),
            2 => json!({"$type": ty("hide"), "hidden": false}),
            3 if i % 2 == 1 => json!({"$type": ty("pin"), "scope": "forum"}),
            4 if i % 2 == 1 => json!({"$type": ty("lock"), "allowed": []}),
            _ => json!({"$type": ty("hide")}),
        };
        f.mod_action(actor, false, &subject, Some(&cid), action, None, None, 5);
    }
    // author-scope moderation by the post authors
    for _ in 0..30 {
        let (c, cid) = comments[rng.below(comments.len())].clone();
        let root = f.get(&c).unwrap().value["root"]["uri"]
            .as_str()
            .unwrap()
            .to_owned();
        let author = root.split('/').nth(2).unwrap().to_owned();
        let a = json!({"$type": ty("hide")});
        f.mod_action(&author, true, &c, Some(&cid), a, None, None, 6);
    }
    // opinions: distinct (voter, subject) pairs
    let karma = [
        "up",
        "up",
        "up",
        "up",
        "up",
        "up",
        "up",
        "strongUp",
        "down",
        "strongDown",
    ];
    let mut n = 0;
    while n < scale.opinions {
        let voter = &users[rng.below(users.len())];
        let (subject, cid) = if rng.chance(3, 10) {
            &posts[rng.below(posts.len())]
        } else {
            &comments[rng.below(comments.len())]
        };
        if subject.split('/').nth(2) == Some(voter.as_str()) {
            continue;
        }
        let mut axes = Map::new();
        axes.insert("karma".into(), json!(karma[rng.below(karma.len())]));
        if rng.chance(1, 5) {
            axes.insert("agreement".into(), json!("agree"));
        }
        if rng.chance(1, 20) {
            axes.insert(
                "reacts".into(),
                json!([{"react": "insightful", "stance": "for"}]),
            );
        }
        let (subject, cid) = (subject.clone(), cid.clone());
        if f.opinion(voter, &subject, &cid, axes, 0).is_some() {
            n += 1;
        }
    }
    f
}

/// The fastest of `runs` folds of `input` under `query`, and the last view.
#[allow(clippy::disallowed_methods)] // timing a test; the fold itself never reads clocks
fn time(input: &FoldInput, query: &Query, runs: usize) -> (Duration, ForumView) {
    let mut best = Duration::MAX;
    let mut view = None;
    for _ in 0..runs {
        let t = std::time::Instant::now();
        let v = fold_view(input, query).expect("the fold accepts the input");
        best = best.min(t.elapsed());
        view = Some(v);
    }
    (best, view.unwrap())
}

#[test]
#[allow(clippy::disallowed_methods)] // timing a test
fn large_forum_folds_within_budget_and_linearly() {
    let release = !cfg!(debug_assertions);
    let budget = if release {
        BUDGET_RELEASE
    } else {
        BUDGET_DEBUG
    };
    let runs = if release { 3 } else { 1 };

    let t = std::time::Instant::now();
    let full = forum(FULL, 1);
    let input = full.input();
    let built = t.elapsed();
    let records = input.records.len();
    assert!(records > 61_000, "{records} records");

    let (t_full, view) = time(&input, &Query::default(), runs);
    assert!(
        view.raw.invalid.is_empty(),
        "invalid: {:?}",
        view.raw.invalid.iter().take(3).collect::<Vec<_>>()
    );
    let k = view.karma.as_ref().unwrap();
    let scores = view.scores.as_ref().unwrap();
    assert!(k.counting.len() >= 45_000, "{} counting", k.counting.len());
    assert!(scores.len() >= 11_000, "{} scores", scores.len());
    assert!(
        k.e.len() > view.participation.s.len(),
        "some users are established"
    );

    // a post list and the comments of the largest thread
    let biggest = view
        .threads
        .iter()
        .max_by_key(|(_, p)| p.parent.len())
        .map(|(r, _)| r.clone())
        .unwrap();
    let q = Query {
        thread: Some(biggest.clone()),
        posts: Some(PostQuery {
            sort: PostSort::Magic,
            ..PostQuery::default()
        }),
        comments: Some(CommentQuery {
            thread: biggest,
            sort: CommentSort::Magic,
        }),
    };
    let (t_query, qv) = time(&input, &q, runs);
    assert!(qv.orderings.as_ref().unwrap().post_order.is_some());

    // linear growth: half the forum folds in about half the time
    let half = forum(
        Scale {
            users: FULL.users / 2,
            posts: FULL.posts / 2,
            comments: FULL.comments / 2,
            opinions: FULL.opinions / 2,
        },
        1,
    );
    let half_input = half.input();
    let (t_half, _) = time(&half_input, &Query::default(), runs);
    let ratio = t_full.as_secs_f64() / t_half.as_secs_f64();

    println!(
        "social fold ({} build): {records} records ({} posts, {} comments, {} opinions); input built in {built:.2?}",
        if release { "release" } else { "debug" },
        FULL.posts,
        FULL.comments,
        FULL.opinions,
    );
    println!(
        "  fold: {t_full:.2?} (empty query), {t_query:.2?} (magic posts + magic comments of the largest thread); budget {budget:?}"
    );
    println!(
        "  half size ({} records): {t_half:.2?}; full/half = {ratio:.2}",
        half_input.records.len()
    );
    assert!(t_full <= budget, "the fold took {t_full:?} > {budget:?}");
    assert!(t_query <= budget, "the fold took {t_query:?} > {budget:?}");
    // 2.0 is linear, 4.0 quadratic; the margin absorbs noise from other processes
    assert!(ratio < 3.2, "full/half fold time = {ratio:.2}: not linear");
}

/// A wiki page of `n` revisions of one slug by the owner (who passes every gate), revision i
/// naming the revisions i ± 1 to i ± `reach` as parents (at most 8, WR2): with `cyclic`, one
/// strongly connected component that the cycle cut of §13.2 takes apart one revision at a time;
/// without, only the backward edges (an acyclic page of the same size).
fn wiki_page(n: usize, reach: usize, cyclic: bool) -> FoldInput {
    let decl = json!({
        "name": "A wiki",
        "membership": "approval",
        "policy": {"tagCreation": "open"},
    });
    let mut f = Forum::new(decl, T0 + 30 * DAY);
    let owner = user(0);
    let uris: Vec<String> = (0..n)
        .map(|i| f.ranged_uri(&owner, "wikiRevision", i64::try_from(i % 1000).unwrap()))
        .collect();
    let mut cids: Vec<String> = Vec::new();
    for i in 0..n {
        let mut parents = Vec::new();
        for d in 1..=reach {
            if i >= d {
                parents.push(json!({"uri": uris[i - d], "cid": cids[i - d]}));
            }
            if cyclic && i + d < n {
                parents.push(json!({"uri": uris[i + d], "cid": social_gen::FOREIGN_CID}));
            }
        }
        let mut v = json!({
            "$type": format!("{NS}.wikiRevision"),
            "forum": f.uri,
            "slug": "page",
            "name": "Page",
            "createdAt": at(10),
        });
        if !parents.is_empty() {
            v["parents"] = json!(parents);
        }
        f.put(&uris[i], Kind::WikiRevision, v, true, Forum::seen(10));
        cids.push(f.cid_of(&uris[i]).unwrap().to_owned());
    }
    f.input()
}

/// SOC-10 ("bounded walks"): the cycle cut of a hostile 1,000-revision page (parents i ± 1 to
/// i ± 4, one component cut 999 times, each re-decomposition O(component), so ~4·10⁶ edge
/// visits in all) costs a small multiple of an acyclic page of the same size. The round-based
/// cut it replaced recomputed the components of the whole page, keyed by uri strings, once per
/// round: 1.39 s in release against 34 ms for the acyclic page.
///
/// Recorded (release, best of three, whole fold): hostile page 59 ms, acyclic page 25 ms
/// (ratio 2.4).
#[test]
#[allow(clippy::disallowed_methods)] // timing a test
fn wiki_cycle_cut_budget() {
    let release = !cfg!(debug_assertions);
    let runs = if release { 3 } else { 1 };
    let hostile = wiki_page(1000, 4, true);
    let plain = wiki_page(1000, 4, false);
    let (t_hostile, v) = time(&hostile, &Query::default(), runs);
    let (t_plain, w) = time(&plain, &Query::default(), runs);
    assert!(v.raw.invalid.is_empty() && w.raw.invalid.is_empty());
    let page = &v.wiki.as_ref().unwrap().pages["page"];
    assert_eq!(page.candidates.len(), 1000);
    assert!(!page.partial);
    // the cut leaves each revision its edges to the greater uris, so the head is the
    // smallest uri at depth 999
    let depth = page.candidates.values().map(|c| c.depth).max();
    assert_eq!(depth, Some(999));
    let plain_depth = w.wiki.as_ref().unwrap().pages["page"]
        .candidates
        .values()
        .map(|c| c.depth)
        .max();
    assert_eq!(plain_depth, Some(999));
    let ratio = t_hostile.as_secs_f64() / t_plain.as_secs_f64();
    println!(
        "wiki cycle cut ({} build): hostile page {t_hostile:.2?}, acyclic page {t_plain:.2?}, ratio {ratio:.2}",
        if release { "release" } else { "debug" }
    );
    let budget = if release {
        Duration::from_millis(150)
    } else {
        Duration::from_secs(3)
    };
    assert!(t_hostile <= budget, "{t_hostile:?} > {budget:?}");
    assert!(ratio < 4.0, "the cut costs {ratio:.2}× the acyclic page");
}
