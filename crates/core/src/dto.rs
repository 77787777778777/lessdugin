// SPDX-License-Identifier: AGPL-3.0-or-later
//! View DTOs (`dto/1`, `docs/spec/engine-api.md` §4).
//!
//! These types are the CLI's `--json` output and the SPA's data model (TypeScript types are
//! generated from them with `tsify` when the `tsify` feature is enabled). Field names and shapes
//! are normative: change them only together with `engine-api.md`, and only compatibly (new
//! optional fields, new enum variants) within `dto/1`.

use serde::{Deserialize, Serialize};
#[cfg(feature = "tsify")]
use tsify::Tsify;

/// The DTO schema version carried by every top-level response.
pub const DTO_VERSION: u32 = 1;

/// Serde helper: a constant `dto` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "tsify", derive(Tsify))]
#[serde(transparent)]
pub struct DtoVersion(pub u32);

impl Default for DtoVersion {
    fn default() -> Self {
        Self(DTO_VERSION)
    }
}

macro_rules! dto {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "tsify", derive(Tsify))]
        #[cfg_attr(feature = "tsify", tsify(into_wasm_abi, from_wasm_abi))]
        #[serde(rename_all = "camelCase")]
        pub struct $name { $($body)* }
    };
    ($(#[$m:meta])* pub enum $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "tsify", derive(Tsify))]
        #[cfg_attr(feature = "tsify", tsify(into_wasm_abi, from_wasm_abi))]
        #[serde(tag = "kind", rename_all = "camelCase")]
        pub enum $name { $($body)* }
    };
}

/// An untagged enum (variants are told apart by their fields).
macro_rules! untagged {
    ($(#[$m:meta])* pub enum $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "tsify", derive(Tsify))]
        #[cfg_attr(feature = "tsify", tsify(into_wasm_abi, from_wasm_abi))]
        #[serde(untagged, rename_all = "camelCase")]
        pub enum $name { $($body)* }
    };
}

/// A simple string enum (serialised as its camelCase name).
macro_rules! string_enum {
    ($(#[$m:meta])* pub enum $name:ident { $($(#[$vm:meta])* $v:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "tsify", derive(Tsify))]
        #[serde(rename_all = "camelCase")]
        pub enum $name { $($(#[$vm])* $v),* }
    };
}

// ---------------------------------------------------------------------------------------------
// §4.1 Common types

string_enum! {
    /// Handle verification state.
    pub enum HandleState { Verified, Unverified, Invalid }
}

dto! {
    /// An actor (user) as displayed.
    pub struct ActorView {
        pub did: String,
        /// Present only if verified in both directions.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub handle: Option<String>,
        pub handle_state: HandleState,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub display_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub avatar: Option<MediaView>,
        pub short_did: String,
    }
}

string_enum! {
    /// Why a listed item is collapsed.
    pub enum CollapseReason { NewUser, LowScore, Muted }
}

dto! {
    /// Why an item is hidden (`social.md` §10.1).
    pub enum HiddenReason {
        NotParticipant,
        ModerationUnavailable,
        Removed {
            rank: u8,
            #[serde(skip_serializing_if = "Option::is_none")]
            actor: Option<String>,
            spam: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            reason: Option<String>,
        },
        Banned,
        Restricted,
        AuthorModerated,
        Locked,
        NewUser,
    }
}

dto! {
    /// The visibility state of an item after the fold and the lens.
    pub enum ItemState {
        Visible,
        Collapsed { reason: CollapseReason },
        Hidden { reason: HiddenReason },
        Tombstone { reason: HiddenReason },
    }
}

/// `Option<bool>` fields that are only ever `true` when present.
fn is_false(b: &bool) -> bool {
    !*b
}

dto! {
    /// Trust and status badges (fixed badge column).
    #[derive(Default)]
    pub struct Badges {
        #[serde(default, skip_serializing_if = "is_false")]
        pub verified: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        pub edited: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        pub future_dated: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        pub identity_unverified: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        pub stale: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        pub hint: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        pub inconsistent_timestamps: bool,
    }
}

string_enum! {
    /// Karma-axis opinion values.
    pub enum KarmaValue { Up, Down, StrongUp, StrongDown }
}

string_enum! {
    /// Agreement-axis opinion values.
    pub enum AgreementValue { Agree, Disagree, StrongAgree, StrongDisagree }
}

dto! {
    /// The viewer's own opinion on an item.
    #[derive(Default)]
    pub struct MyOpinion {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub karma: Option<KarmaValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub agreement: Option<AgreementValue>,
    }
}

dto! {
    /// Score of an item (`social.md` §7).
    #[derive(Default)]
    pub struct ScoreView {
        pub score: i64,
        pub agreement: i64,
        pub unweighted: u32,
        pub earlier_version_votes: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mine: Option<MyOpinion>,
    }
}

string_enum! {
    /// React stance.
    pub enum Stance { For, Against }
}

dto! {
    /// A W3C-style text quote selector (`data-model.md` `defs#textQuote`).
    pub struct TextQuote {
        pub exact: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub prefix: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub suffix: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub start: Option<u32>,
    }
}

untagged! {
    /// A resolved quote anchor: code-point offsets into the plain-text projection P.
    pub enum Anchor {
        Range { start: u32, end: u32 },
        Orphan { orphan: bool },
    }
}

dto! {
    /// Aggregated reacts of one kind (and anchor) on an item.
    pub struct ReactTally {
        pub react: String,
        pub label: String,
        pub glyph: String,
        #[serde(rename = "for")]
        pub for_count: u32,
        pub against: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub quote: Option<TextQuote>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub anchor: Option<Anchor>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub creator: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mine: Option<Stance>,
    }
}

dto! {
    /// A table-of-contents entry.
    pub struct TocEntry {
        pub level: u8,
        pub id: String,
        pub text: String,
    }
}

dto! {
    /// Width and height.
    pub struct AspectRatio {
        pub width: u32,
        pub height: u32,
    }
}

dto! {
    /// A media item referenced by a body.
    pub struct MediaView {
        pub index: u16,
        pub cid: String,
        pub mime: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub alt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub aspect_ratio: Option<AspectRatio>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub duration_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        pub author: String,
        pub pds: String,
        pub autoload: bool,
        pub size_bytes: u64,
    }
}

dto! {
    /// A rendered markdown body.
    pub struct RenderedBody {
        pub html: String,
        pub text: String,
        pub toc: Vec<TocEntry>,
        pub word_count: u32,
        pub read_minutes: u32,
        pub media: Vec<MediaView>,
        pub excerpt: String,
        pub notices: Vec<String>,
        pub renderer_version: String,
    }
}

dto! {
    /// Completeness of the data behind a view.
    #[derive(Default)]
    pub struct Completeness {
        pub partial: bool,
        pub repos_complete: u32,
        pub repos_total: u32,
        pub degraded: bool,
    }
}

// ---------------------------------------------------------------------------------------------
// §4.2 Posts and threads

string_enum! {
    /// Post kinds.
    pub enum PostKind { Article, Link, Question, QuickTake }
}

string_enum! {
    /// Resolved tier.
    pub enum Tier { Personal, Frontpage, Curated }
}

string_enum! {
    /// Post sorts (`social.md` §12.2).
    pub enum PostSort { Magic, Top, New, Old, RecentComments }
}

string_enum! {
    /// Post list filters (`social.md` §12.3).
    pub enum PostFilter { Frontpage, All, Curated }
}

string_enum! {
    /// Comment sorts (`social.md` §12.4).
    pub enum CommentSort { Magic, Top, New, Old, LatestReply }
}

dto! {
    /// A link post's external link.
    pub struct ExternalLinkView {
        pub uri: String,
        pub host: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
    }
}

dto! {
    /// A post in a list.
    pub struct PostSummary {
        pub uri: String,
        pub cid: String,
        pub forum: String,
        pub author: ActorView,
        #[serde(rename = "kind")]
        pub post_kind: PostKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        pub excerpt: String,
        pub tags: Vec<String>,
        pub tier: Tier,
        pub pinned: bool,
        pub locked: bool,
        pub created_at_ms: i64,
        pub t_rank_ms: i64,
        pub comment_count: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_comment_at_ms: Option<i64>,
        pub score: ScoreView,
        pub state: ItemState,
        pub badges: Badges,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub unread_comments: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub link: Option<ExternalLinkView>,
        pub labels: Vec<String>,
    }
}

dto! {
    /// A page of posts.
    pub struct PostList {
        pub dto: DtoVersion,
        pub forum: String,
        pub sort: PostSort,
        pub filter: PostFilter,
        pub items: Vec<PostSummary>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cursor: Option<String>,
        pub completeness: Completeness,
    }
}

dto! {
    /// Revision summary of an item.
    #[derive(Default)]
    pub struct HistorySummary {
        pub revisions: u32,
        pub redacted: bool,
    }
}

dto! {
    /// Previous/next navigation within a sequence.
    pub struct SequenceNav {
        pub sequence: String,
        pub title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub prev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub next: Option<String>,
    }
}

dto! {
    /// A post with its rendered body.
    pub struct PostView {
        pub dto: DtoVersion,
        pub post: PostSummary,
        pub body: RenderedBody,
        pub reacts: Vec<ReactTally>,
        pub history: HistorySummary,
        pub sequences: Vec<SequenceNav>,
    }
}

untagged! {
    /// Which version of its target a comment refers to.
    pub enum VersionRef {
        Revision { revision: u32 },
        Unavailable { unavailable: bool },
    }
}

dto! {
    /// A comment in a thread (pre-order with depth).
    pub struct CommentNode {
        pub uri: String,
        pub cid: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub parent: Option<String>,
        pub depth: u32,
        pub author: ActorView,
        pub body: RenderedBody,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub quote: Option<TextQuote>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub quote_anchor: Option<Anchor>,
        pub created_at_ms: i64,
        pub t_rank_ms: i64,
        pub score: ScoreView,
        pub reacts: Vec<ReactTally>,
        pub state: ItemState,
        pub badges: Badges,
        pub retracted: bool,
        pub moderator_hat: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub version: Option<VersionRef>,
        pub orphaned: bool,
        pub missing: bool,
        pub unread: bool,
        pub pinned: bool,
    }
}

untagged! {
    /// The root of a thread: the post, or a placeholder when it is not available.
    pub enum ThreadRoot {
        Post(Box<PostView>),
        Missing { missing: String },
    }
}

dto! {
    /// A thread.
    pub struct ThreadView {
        pub dto: DtoVersion,
        pub post: ThreadRoot,
        pub comments: Vec<CommentNode>,
        pub sort: CommentSort,
        pub completeness: Completeness,
    }
}

dto! {
    /// One Recent Discussion item.
    pub struct DiscussionItem {
        pub post: PostSummary,
        pub comments: Vec<CommentNode>,
    }
}

dto! {
    /// Recent Discussion.
    pub struct DiscussionList {
        pub dto: DtoVersion,
        pub items: Vec<DiscussionItem>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cursor: Option<String>,
        pub completeness: Completeness,
    }
}

dto! {
    /// One version in an item's history.
    pub struct VersionEntry {
        pub n: u32,
        pub cid: String,
        pub created_at_ms: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub summary: Option<String>,
        pub available: bool,
    }
}

dto! {
    /// An item's version history.
    pub struct HistoryView {
        pub dto: DtoVersion,
        pub uri: String,
        pub versions: Vec<VersionEntry>,
    }
}

// ---------------------------------------------------------------------------------------------
// §4.3 Forums, users, organisation

string_enum! {
    /// Membership mode.
    pub enum MembershipMode { Open, Approval }
}

string_enum! {
    /// Moderator roles.
    pub enum ModRole { Admin, Moderator }
}

string_enum! {
    /// The viewer's (or a member's) role/status in a forum.
    pub enum ForumRole { Owner, Admin, Moderator, Member, Established, Newcomer, None }
}

dto! {
    /// A forum in a list.
    pub struct ForumSummary {
        pub uri: String,
        pub owner: ActorView,
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub membership: MembershipMode,
        pub followed: bool,
        pub member: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub alias: Option<String>,
    }
}

dto! {
    /// A moderator entry.
    pub struct ModeratorView {
        pub actor: ActorView,
        pub role: ModRole,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        /// Shown as a moderator only if they also hold a membership record.
        pub shown: bool,
    }
}

dto! {
    /// A banner shown above a forum view.
    pub struct Banner {
        pub code: String,
        pub message: String,
    }
}

dto! {
    /// The effective policy of a forum (`social.md` §1.6), as name/value pairs in `PARAMS` order.
    pub struct EffectivePolicy {
        pub moderation: String,
        pub karma: String,
        pub ranking: String,
        pub react_palette: String,
        pub params: Vec<(String, String)>,
    }
}

dto! {
    /// A forum header.
    pub struct ForumView {
        pub dto: DtoVersion,
        pub forum: ForumSummary,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub about: Option<RenderedBody>,
        pub moderators: Vec<ModeratorView>,
        pub policy: EffectivePolicy,
        pub my_role: ForumRole,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub my_karma: Option<i64>,
        pub banners: Vec<Banner>,
        pub completeness: Completeness,
    }
}

dto! {
    /// Karma stages of a user (`social.md` §6).
    pub struct KarmaView {
        pub k1: i64,
        pub kp: i64,
        pub k: i64,
    }
}

dto! {
    /// A link on a profile.
    pub struct LinkView {
        pub uri: String,
        pub host: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub label: Option<String>,
    }
}

dto! {
    /// A user page.
    pub struct UserView {
        pub dto: DtoVersion,
        pub actor: ActorView,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub forum: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub karma: Option<KarmaView>,
        pub status: ForumRole,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub bio: Option<RenderedBody>,
        pub links: Vec<LinkView>,
        pub pinned: Vec<PostSummary>,
        pub recent_posts: Vec<PostSummary>,
        pub recent_comments: Vec<CommentNode>,
    }
}

dto! {
    /// A member of a forum.
    pub struct MemberView {
        pub actor: ActorView,
        pub status: ForumRole,
        pub granted: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub membership_note: Option<String>,
        pub joined_at_ms: i64,
    }
}

dto! {
    /// A tag in a list.
    pub struct TagSummary {
        pub slug: String,
        pub name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub post_count: u32,
        pub unmerged_edits: u32,
    }
}

dto! {
    /// A tag page.
    pub struct TagView {
        pub dto: DtoVersion,
        pub tag: TagSummary,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub page: Option<RenderedBody>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub head: Option<String>,
        pub posts: Vec<PostSummary>,
    }
}

dto! {
    /// A sequence in a list.
    pub struct SequenceSummary {
        pub uri: String,
        pub author: ActorView,
        pub title: String,
        pub chapters: u32,
        pub posts: u32,
        pub score: ScoreView,
        pub pinned: bool,
    }
}

dto! {
    /// A chapter of a sequence.
    pub struct ChapterView {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub body: Option<RenderedBody>,
        pub posts: Vec<PostSummary>,
    }
}

dto! {
    /// A sequence page.
    pub struct SequenceView {
        pub dto: DtoVersion,
        pub sequence: SequenceSummary,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub body: Option<RenderedBody>,
        pub chapters: Vec<ChapterView>,
    }
}

untagged! {
    /// A search hit's item.
    pub enum SearchItem {
        Post(PostSummary),
        Comment(Box<CommentNode>),
    }
}

dto! {
    /// A search hit.
    pub struct SearchHit {
        pub item: SearchItem,
        pub snippet: String,
        pub highlights: Vec<(u32, u32)>,
    }
}

dto! {
    /// Search results.
    pub struct SearchResults {
        pub dto: DtoVersion,
        pub hits: Vec<SearchHit>,
    }
}

string_enum! {
    /// Notification reasons (`social.md` §16).
    pub enum NotificationReason {
        Reply, Mention, Quote, NewComment, NewPost, Reacted, ModAction, KarmaChange,
    }
}

dto! {
    /// A notification.
    pub struct NotificationView {
        pub id: String,
        pub reason: NotificationReason,
        pub at_ms: i64,
        pub subject: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub actor: Option<ActorView>,
        pub read: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub preview: Option<String>,
    }
}

dto! {
    /// Notifications.
    pub struct NotificationList {
        pub dto: DtoVersion,
        pub items: Vec<NotificationView>,
    }
}

dto! {
    /// A mod-queue item.
    pub struct ModQueueItem {
        #[serde(rename = "kind")]
        pub item_kind: String,
        pub subject: String,
        pub reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub detail: Option<String>,
    }
}

dto! {
    /// The mod queue.
    pub struct ModQueue {
        pub dto: DtoVersion,
        pub items: Vec<ModQueueItem>,
    }
}

dto! {
    /// A mod-log entry.
    pub struct ModLogEntry {
        pub actor: ActorView,
        pub action: String,
        pub subject: String,
        pub value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub reason: Option<String>,
        pub at_ms: i64,
        pub lapsed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub adopted_from: Option<String>,
    }
}

dto! {
    /// The mod log.
    pub struct ModLog {
        pub dto: DtoVersion,
        pub entries: Vec<ModLogEntry>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cursor: Option<String>,
    }
}

string_enum! {
    /// Content trust level (`sync.md` §1.2).
    pub enum TrustLevel { Pds, Proven, Hint }
}

dto! {
    /// A raw view of one record, with all annotations.
    pub struct RawView {
        pub dto: DtoVersion,
        pub uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cid: Option<String>,
        pub level: TrustLevel,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub record: Option<serde_json::Value>,
        pub validity: Vec<String>,
        pub assertions: Vec<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub state: Option<ItemState>,
    }
}

string_enum! {
    /// Outcome of `verify`.
    pub enum VerifyOutcome { Proven, Absent, Failed }
}

dto! {
    /// One check performed by `verify`.
    pub struct VerifyCheck {
        pub name: String,
        pub ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub detail: Option<String>,
    }
}

dto! {
    /// Result of `verify`.
    pub struct VerifyReport {
        pub dto: DtoVersion,
        pub uri: String,
        pub result: VerifyOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub rev: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub commit_cid: Option<String>,
        pub key_source: String,
        pub checks: Vec<VerifyCheck>,
    }
}

dto! {
    /// An issue encountered during sync.
    pub struct SyncIssue {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub did: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub host: Option<String>,
        pub code: String,
        pub message: String,
    }
}

dto! {
    /// Result of `sync_forum`.
    pub struct SyncReport {
        pub dto: DtoVersion,
        pub forum: String,
        pub repos_total: u32,
        pub repos_complete: u32,
        pub repos_stale: u32,
        pub repos_unavailable: u32,
        pub requests: u32,
        pub bytes: u64,
        pub changed: u32,
        pub partial: bool,
        pub degraded: bool,
        pub errors: Vec<SyncIssue>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_state_shapes() {
        let s = ItemState::Hidden {
            reason: HiddenReason::Removed { rank: 1, actor: None, spam: false, reason: None },
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"kind":"hidden","reason":{"kind":"removed","rank":1,"spam":false}}"#
        );
        let c = ItemState::Collapsed { reason: CollapseReason::NewUser };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"kind":"collapsed","reason":"newUser"}"#
        );
    }

    #[test]
    fn badges_omit_false() {
        let b = Badges { edited: true, ..Default::default() };
        assert_eq!(serde_json::to_string(&b).unwrap(), r#"{"edited":true}"#);
    }

    #[test]
    fn dto_version_is_one() {
        assert_eq!(serde_json::to_string(&DtoVersion::default()).unwrap(), "1");
    }
}
