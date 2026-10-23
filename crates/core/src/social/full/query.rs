// SPDX-License-Identifier: AGPL-3.0-or-later
//! `Query` (`social.md` §1.1, §12): the requested thread, post list and comment list. A query
//! never changes any per-item value; it only selects which orderings the fold computes.
//!
//! The JSON form is the vectors' (`testdata/vectors/README.md`, "Social fold vectors"):
//!
//! ```text
//! { thread?:   post uri,                                         // §11 placement to project
//!   posts?:    { sort, filter, tag?, kind?, author? },           // §12.2, §12.3
//!   comments?: { thread, sort } }                                // §12.4
//! ```

use serde::{Deserialize, Serialize};

/// The post sorts of §12.2.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum PostSort {
    /// `magic` (the default): pins, then `hot`, with the cutoff.
    #[default]
    Magic,
    /// `top`: score desc.
    Top,
    /// `new`: `t_rank` desc.
    New,
    /// `old`: `t_rank` asc.
    Old,
    /// `recentComments`: thread activity desc.
    RecentComments,
}

/// The list filters of §12.3.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum PostFilter {
    /// `frontpage` (the default): tier ≥ frontpage, not quick takes.
    #[default]
    Frontpage,
    /// `all`: every tier and kind.
    All,
    /// `curated`: tier = curated, not quick takes.
    Curated,
}

/// The comment sorts of §12.4.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum CommentSort {
    /// `magic` (the default).
    #[default]
    Magic,
    /// `top`.
    Top,
    /// `new`.
    New,
    /// `old`.
    Old,
    /// `latestReply`.
    LatestReply,
}

/// A post list request (§12.2, §12.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostQuery {
    /// The sort.
    #[serde(default)]
    pub sort: PostSort,
    /// The tier filter.
    #[serde(default)]
    pub filter: PostFilter,
    /// Keep the posts with `applies(p, tag)` (§13.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Keep the posts whose effective kind (PO2) is this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Keep the posts by this DID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// A comment list request (§12.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentQuery {
    /// The thread's root post uri.
    pub thread: String,
    /// The sort.
    #[serde(default)]
    pub sort: CommentSort,
}

/// `Query` (§1.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    /// The thread whose placement (§11) is requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// A post list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posts: Option<PostQuery>,
    /// A comment list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<CommentQuery>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_form() {
        let q: Query = serde_json::from_str(
            r#"{"posts": {"sort": "recentComments", "filter": "all", "tag": "x"},
                "comments": {"thread": "at://p", "sort": "latestReply"}}"#,
        )
        .unwrap();
        let p = q.posts.unwrap();
        assert_eq!(p.sort, PostSort::RecentComments);
        assert_eq!(p.filter, PostFilter::All);
        assert_eq!(p.tag.as_deref(), Some("x"));
        assert_eq!(q.comments.unwrap().sort, CommentSort::LatestReply);
        assert_eq!(
            serde_json::from_str::<Query>("{}").unwrap(),
            Query::default()
        );
    }
}
