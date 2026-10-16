// SPDX-License-Identifier: MIT OR Apache-2.0
//! `post` (§6.3) and `comment` (§6.4).

use lessdugin_atp::syntax::{AtUri, Datetime};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{ExternalLink, Labels, Markdown, Md, StrongRef, TextQuote};
use crate::collection::nsid;
use crate::values::{Effective, PostKind};

fn post_type() -> String {
    nsid::POST.to_owned()
}

fn comment_type() -> String {
    nsid::COMMENT.to_owned()
}

/// `NS.post` (rkey `{fk}-{tid}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    /// `NS.post` of this build.
    #[serde(rename = "$type", default = "post_type")]
    pub r#type: String,
    /// The forum at-uri (fixed after creation).
    pub forum: AtUri,
    /// `kv{article, link, question, quickTake}`, default `article`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Title (required unless quickTake, PO3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Body (MAY spill; required for quickTake, PO4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Md>,
    /// Plain-text excerpt (required when `body` is spilled, PO6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    /// Link card (required for kind `link`, PO5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<ExternalLink>,
    /// The author's own tag slugs (set semantics, PO7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Self-labels (PO8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Labels>,
    /// Creation time.
    pub created_at: Datetime,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Post {
    /// An article in `forum` with `title` and an inline body.
    pub fn article(
        forum: AtUri,
        title: impl Into<String>,
        body: impl Into<String>,
        created_at: Datetime,
    ) -> Self {
        Self {
            r#type: post_type(),
            forum,
            kind: None,
            title: Some(title.into()),
            body: Some(Md::Known(Markdown::inline(body))),
            excerpt: None,
            link: None,
            tags: None,
            labels: None,
            created_at,
            updated_at: None,
            extra: Map::new(),
        }
    }

    /// The effective kind (PO2): `kind` if known, otherwise `article`.
    pub fn kind(&self) -> Effective<PostKind> {
        PostKind::effective(self.kind.as_deref())
    }
}

/// `NS.comment` (rkey `{fk}-{rootTid}-{tid}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// `NS.comment` of this build.
    #[serde(rename = "$type", default = "comment_type")]
    pub r#type: String,
    /// The thread's post; `cid` is the version replied to.
    pub root: StrongRef,
    /// The comment replied to; absent means top-level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<StrongRef>,
    /// The forum at-uri (equals the root post's forum, CO5).
    pub forum: AtUri,
    /// Body (inline only, CO4).
    pub body: Md,
    /// Quoted text, anchored in `parent` if present, otherwise in `root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<TextQuote>,
    /// Retracted by the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted: Option<bool>,
    /// Written as a moderator (honoured per CO6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderator_hat: Option<bool>,
    /// Creation time.
    pub created_at: Datetime,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Comment {
    /// A top-level (or, with `parent`, nested) comment with an inline body.
    pub fn new(
        forum: AtUri,
        root: StrongRef,
        parent: Option<StrongRef>,
        body: impl Into<String>,
        created_at: Datetime,
    ) -> Self {
        Self {
            r#type: comment_type(),
            root,
            parent,
            forum,
            body: Md::Known(Markdown::inline(body)),
            quote: None,
            retracted: None,
            moderator_hat: None,
            created_at,
            updated_at: None,
            extra: Map::new(),
        }
    }
}
