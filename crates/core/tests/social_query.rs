// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `Query` contract of `social.md` §1.1 ("it never changes any per-item value") over every
//! fold vector input (`testdata/vectors/`): each input is re-folded with every post sort and
//! comment sort, a post list and the comments of a thread, and the per-item fields must equal
//! those of the fold with the empty query. The same pass checks that the comment pre-order
//! (§12.4) lists each displayed node of the thread exactly once (§10.2: hidden comments that are
//! not displayed have no displayed descendant), including the 1,002-comment chain of
//! `thread/depth.json`.
//!
//! The vectors are in the final namespace, so this test exists only in a `final-ns` build:
//! `cargo test -p lessdugin-core --features final-ns --test social_query`.

#![cfg(feature = "final-ns")]

use std::path::PathBuf;

use lessdugin_core::social::full::query::{
    CommentQuery, CommentSort, PostFilter, PostQuery, PostSort,
};
use lessdugin_core::social::full::{Query, fold_view, project};
use lessdugin_core::social::input::FoldInput;
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/vectors")
}

/// Every fold vector, re-folded with each sort, a post list and the comments of the queried (or
/// first) thread: the query changes no per-item value (§1.1).
#[test]
fn queries_change_no_per_item_value() {
    let per_item = [
        "states",
        "displayed",
        "modStates",
        "modLog",
        "scores",
        "times",
        "resolved",
        "tiers",
        "counting",
        "K",
        "E",
        "newcomers",
        "passes",
        "reacts",
        "wiki",
        "tagRel",
        "applies",
    ];
    let mut n = 0;
    for d in [
        "time",
        "modfold",
        "karma",
        "reacts",
        "rank",
        "visibility",
        "thread",
        "fold",
    ] {
        for e in std::fs::read_dir(root().join(d)).unwrap() {
            let path = e.unwrap().path();
            let f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            if matches!(f["kind"].as_str(), Some("conversion" | "power")) {
                continue;
            }
            for v in f["vectors"].as_array().unwrap() {
                let fi = FoldInput::from_vector_json(&v["input"]["fold"]).unwrap();
                let base = fold_view(&fi, &Query::default()).unwrap();
                let thread = v["input"]["query"]["thread"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| base.threads.keys().next().cloned())
                    .unwrap_or_default();
                for (ps, cs) in [
                    (PostSort::Magic, CommentSort::Magic),
                    (PostSort::Top, CommentSort::Top),
                    (PostSort::New, CommentSort::New),
                    (PostSort::Old, CommentSort::Old),
                    (PostSort::RecentComments, CommentSort::LatestReply),
                ] {
                    let q = Query {
                        thread: Some(thread.clone()),
                        posts: Some(PostQuery {
                            sort: ps,
                            filter: PostFilter::All,
                            ..PostQuery::default()
                        }),
                        comments: Some(CommentQuery {
                            thread: thread.clone(),
                            sort: cs,
                        }),
                    };
                    let view = fold_view(&fi, &q).unwrap();
                    for field in per_item {
                        assert_eq!(
                            project(&view, field).ok(),
                            project(&base, field).ok(),
                            "{path:?} {} {field}",
                            v["name"]
                        );
                    }
                    let order = project(&view, "commentOrder").unwrap();
                    let order = order.as_array().unwrap();
                    let mut uniq: Vec<_> = order.iter().collect();
                    uniq.sort_by_key(|x| x.to_string());
                    uniq.dedup();
                    assert_eq!(uniq.len(), order.len(), "duplicate node in commentOrder");
                    // the pre-order reaches every displayed node (§12.4, NOTES 49)
                    let vis = view.visibility.as_ref().unwrap();
                    let want = view.threads.get(&thread).map_or(0, |pl| {
                        pl.parent.keys().filter(|c| vis.shown(c)).count()
                            + vis.shown_missing.get(&thread).map_or(0, |m| m.len())
                    });
                    assert_eq!(order.len(), want, "{path:?} {} commentOrder", v["name"]);
                    let posts = project(&view, "postOrder").unwrap();
                    assert!(posts.as_array().is_some());
                    n += 1;
                }
            }
        }
    }
    assert!(n > 0, "no fold vectors found");
}
