// SPDX-License-Identifier: MIT OR Apache-2.0
//! `wikiRevision` (§6.8) and `sequence` (§6.9).

use lessdugin_atp::syntax::{AtUri, Datetime};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Md, Media, StrongRef};
use crate::collection::nsid;

fn wiki_revision_type() -> String {
    nsid::WIKI_REVISION.to_owned()
}

fn sequence_type() -> String {
    nsid::SEQUENCE.to_owned()
}

/// `NS.wikiRevision` (rkey `{fk}-{tid}`; immutable by convention).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiRevision {
    /// `NS.wikiRevision` of this build.
    #[serde(rename = "$type", default = "wiki_revision_type")]
    pub r#type: String,
    /// The forum at-uri.
    pub forum: AtUri,
    /// Page slug; the page identity is `(forum, slug)`.
    pub slug: String,
    /// Parent revisions (≤ 8; MVP writers write 0 or 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parents: Option<Vec<StrongRef>>,
    /// Page display name.
    pub name: String,
    /// Plain-text summary for hovers and cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Page body (MAY spill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Md>,
    /// Whether the page can be applied as a post tag (default `true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_tag: Option<bool>,
    /// Parent tag slugs (≤ 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tags: Option<Vec<String>>,
    /// Edit summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Creation time.
    pub created_at: Datetime,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WikiRevision {
    /// `isTag`, default `true`.
    pub fn is_tag(&self) -> bool {
        self.is_tag.unwrap_or(true)
    }
}

/// `NS.sequence` (rkey `{fk}-{tid}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sequence {
    /// `NS.sequence` of this build.
    #[serde(rename = "$type", default = "sequence_type")]
    pub r#type: String,
    /// The forum at-uri.
    pub forum: AtUri,
    /// Title.
    pub title: String,
    /// Introduction (MAY spill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Md>,
    /// Cover (displayed only if `image/*`, SQ3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<Media>,
    /// Chapters, in order (≤ 50).
    pub chapters: Vec<Chapter>,
    /// Creation time.
    pub created_at: Datetime,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `sequence#chapter`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    /// Chapter title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Chapter introduction (inline only, SQ3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Md>,
    /// Post at-uris (≤ 100), always shown at their latest version.
    pub posts: Vec<AtUri>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
