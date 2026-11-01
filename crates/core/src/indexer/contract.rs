// SPDX-License-Identifier: AGPL-3.0-or-later
//! The small, normative `online.7778777.lessdugin.index.*` client contract.
//!
//! These are transport types, not engine views. In particular `PostHint.tally` and
//! `PostHint.state` are never converted to DTOs or admitted to the record store. Keeping them in
//! a separate module makes that trust boundary explicit (data-model.md §11).

use lessdugin_atp::syntax::{AtUri, Did};
use serde::{Deserialize, Serialize};

pub const NSID_PREFIX: &str = "online.7778777.lessdugin.index.";
pub const MAX_PROOF_URIS: usize = 25;
pub const MAX_HEAD_DIDS: usize = 50;
pub const MAX_CHANGES: usize = 1_000;
pub const MAX_POSTS: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsOf {
    pub seq: i64,
    pub time_us: i64,
    pub lag_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerInfo {
    pub did: String,
    pub core_version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default)]
    pub as_of: AsOf,
    #[serde(default)]
    pub indexer: IndexerInfo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumStatus {
    pub uri: String,
    #[serde(default)]
    pub lag_ms: Option<i64>,
    #[serde(default)]
    pub backfilled: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Description {
    pub did: String,
    pub version: String,
    pub core_version: String,
    #[serde(default)]
    pub as_of: AsOf,
    #[serde(default)]
    pub policies: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub forums: Vec<ForumStatus>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordRef {
    pub uri: String,
    #[serde(default)]
    pub cid: Option<String>,
    #[serde(default)]
    pub first_seen: Option<String>,
    /// A hint only. No caller in the client reads this field.
    #[serde(default, skip_serializing)]
    pub value: Option<serde_json::Value>,
}

impl RecordRef {
    pub fn at_uri(&self) -> Option<AtUri> {
        AtUri::parse(&self.uri).ok()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumHint {
    #[serde(rename = "ref")]
    pub record: RecordRef,
    #[serde(default)]
    pub participants: Option<i64>,
    #[serde(default)]
    pub posts: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumsPage {
    #[serde(flatten)]
    pub meta: Metadata,
    #[serde(default)]
    pub forums: Vec<ForumHint>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantHint {
    pub did: String,
    pub status: String,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub last_seen: Option<String>,
}

impl ParticipantHint {
    pub fn parsed_did(&self) -> Option<Did> {
        Did::parse(&self.did).ok()
    }

    pub fn known_status(&self) -> bool {
        matches!(
            self.status.as_str(),
            "owner" | "admin" | "moderator" | "member" | "established" | "newcomer"
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParticipantsPage {
    #[serde(flatten)]
    pub meta: Metadata,
    #[serde(default)]
    pub participants: Vec<ParticipantHint>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoHeadHint {
    pub did: String,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub pds: Option<String>,
    #[serde(default)]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoHeads {
    #[serde(flatten)]
    pub meta: Metadata,
    #[serde(default)]
    pub heads: Vec<RepoHeadHint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub seq: i64,
    pub uri: String,
    #[serde(default)]
    pub cid: Option<String>,
    pub did: String,
    pub rev: String,
    pub op: String,
}

impl Change {
    pub fn known_op(&self) -> bool {
        matches!(self.op.as_str(), "create" | "update" | "delete")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesPage {
    #[serde(flatten)]
    pub meta: Metadata,
    #[serde(default)]
    pub changes: Vec<Change>,
    pub cursor: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TallyHint {
    pub subject: String,
    pub policy: String,
    pub score: i64,
    #[serde(default)]
    pub agreement: Option<i64>,
    #[serde(default)]
    pub unweighted: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostHint {
    #[serde(rename = "ref")]
    pub record: RecordRef,
    #[serde(default)]
    pub tally: Option<TallyHint>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub comments: Option<i64>,
    #[serde(default)]
    pub last_comment_at: Option<String>,
    pub effective_at: String,
    #[serde(default)]
    pub rank: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyIds {
    pub karma: String,
    pub ranking: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostsPage {
    #[serde(flatten)]
    pub meta: Metadata,
    #[serde(default)]
    pub posts: Vec<PostHint>,
    #[serde(default)]
    pub cursor: Option<String>,
    pub policy: PolicyIds,
    pub input_digest: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentHint {
    #[serde(rename = "ref")]
    pub record: RecordRef,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub depth: Option<i64>,
    #[serde(default)]
    pub tally: Option<TallyHint>,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionHint {
    pub post: PostHint,
    #[serde(default)]
    pub comments: Vec<CommentHint>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscussionPage {
    #[serde(flatten)]
    pub meta: Metadata,
    #[serde(default)]
    pub items: Vec<DiscussionHint>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHint {
    #[serde(flatten)]
    pub meta: Metadata,
    pub post: PostHint,
    #[serde(default)]
    pub comments: Vec<CommentHint>,
    #[serde(default)]
    pub truncated: bool,
    pub input_digest: String,
}

/// URI order and an audit digest. This is the only post-list information the view layer needs;
/// it deliberately has no text, score, badge or tally fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderingHint {
    pub uris: Vec<AtUri>,
    pub cursor: Option<String>,
    pub input_digest: Option<String>,
}
