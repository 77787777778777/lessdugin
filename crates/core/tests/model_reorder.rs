// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forum model answers every sort, filter and thread from its cached layers
//! ([`ForumModel::orderings`], `full::reorder`) without folding again. Over every fold vector
//! input (`testdata/vectors/`), the orderings it gives for each post sort × filter and each
//! comment sort of each thread must equal those of a full fold with that query (`social.md`
//! §1.1, §12).
//!
//! The vectors are in the final namespace, so this test exists only in a `final-ns` build:
//! `cargo test -p lessdugin-core --features final-ns --test model_reorder`.

#![cfg(feature = "final-ns")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::model::ForumModel;
use lessdugin_core::social::full::query::{
    CommentQuery, CommentSort, PostFilter, PostQuery, PostSort,
};
use lessdugin_core::social::full::{Query, fold_view, lexicon_validity};
use lessdugin_core::social::input::FoldInput;
use lessdugin_core::store::records::Level;
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/vectors")
}

const POST_SORTS: [PostSort; 5] = [
    PostSort::Magic,
    PostSort::Top,
    PostSort::New,
    PostSort::Old,
    PostSort::RecentComments,
];
const FILTERS: [PostFilter; 3] = [PostFilter::Frontpage, PostFilter::All, PostFilter::Curated];
const COMMENT_SORTS: [CommentSort; 5] = [
    CommentSort::Magic,
    CommentSort::Top,
    CommentSort::New,
    CommentSort::Old,
    CommentSort::LatestReply,
];

#[test]
fn reordering_the_cached_layers_equals_a_refold() {
    let (mut inputs, mut queries) = (0, 0);
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
                let fold = &v["input"]["fold"];
                let fi = FoldInput::from_vector_json(fold).unwrap();
                let decl = AtUri::parse(fold["decl"]["uri"].as_str().unwrap()).unwrap();
                let Ok(key) = ForumKey::of_uri(&decl) else {
                    continue;
                };
                // an input the fold rejects has no model (the engine keeps its last good one)
                let Ok(model) = ForumModel::from_contract(
                    key,
                    &fi,
                    &lexicon_validity,
                    Level::Pds,
                    BTreeMap::new(),
                    BTreeSet::new(),
                ) else {
                    assert!(fold_view(&fi, &Query::default()).is_err());
                    continue;
                };
                inputs += 1;
                let name = format!("{path:?} {}", v["name"]);
                // the base orderings are the model's own
                assert_eq!(
                    model.orderings(&Query::default()).ok(),
                    model.view.orderings.clone().ok(),
                    "{name}"
                );
                let mut threads: Vec<String> = model.view.threads.keys().cloned().collect();
                threads.truncate(4);
                for sort in POST_SORTS {
                    for filter in FILTERS {
                        let q = Query {
                            posts: Some(PostQuery {
                                sort,
                                filter,
                                ..PostQuery::default()
                            }),
                            ..Query::default()
                        };
                        let full = fold_view(&fi, &q).unwrap().orderings;
                        assert_eq!(model.orderings(&q), full, "{name} {sort:?} {filter:?}");
                        queries += 1;
                    }
                }
                for t in &threads {
                    for sort in COMMENT_SORTS {
                        let q = Query {
                            thread: Some(t.clone()),
                            posts: None,
                            comments: Some(CommentQuery {
                                thread: t.clone(),
                                sort,
                            }),
                        };
                        let full = fold_view(&fi, &q).unwrap().orderings;
                        assert_eq!(model.orderings(&q), full, "{name} {t} {sort:?}");
                        queries += 1;
                    }
                }
            }
        }
    }
    assert!(inputs > 50, "only {inputs} fold vector inputs");
    eprintln!("{inputs} inputs, {queries} queries");
}
