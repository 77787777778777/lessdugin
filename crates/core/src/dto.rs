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
        #[serde(rename_all = "camelCase")]
        pub struct $name { $($body)* }
    };
    ($(#[$m:meta])* pub enum $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "tsify", derive(Tsify))]
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
        /// The handle the DID document claims, present only while it is `unverified`
        /// (`sync.md` §3.1: shown struck through next to `shortDid`). Untrusted text.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub claimed_handle: Option<String>,
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
        /// An own item with an outbox write that is not `done` (`sync.md` §1.6).
        #[serde(default, skip_serializing_if = "is_false")]
        pub pending: bool,
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
    /// The viewer's own opinion on an item, as stored in the cache (their public opinion
    /// record).
    #[derive(Default)]
    pub struct MyOpinion {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub karma: Option<KarmaValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub agreement: Option<AgreementValue>,
        /// A write of the viewer's opinion on this item is in the outbox and not done (held
        /// for the undo window, or not sent yet): the values above are the stored ones.
        #[serde(default, skip_serializing_if = "is_false")]
        pub pending: bool,
        /// The viewer's vote is recorded but carries no weight (`social.md` §5–§7: the viewer
        /// is not an eligible voter yet, or the opinion does not count): "recorded; will count
        /// once you're established".
        #[serde(default, skip_serializing_if = "is_false")]
        pub not_yet_counted: bool,
    }
}

/// `u32` fields omitted when zero.
fn is_zero(n: &u32) -> bool {
    *n == 0
}

dto! {
    /// Score of an item (`social.md` §7). Hidden and tombstoned items show the zero score to
    /// everyone but the forum's authorities (their scores are in the raw view).
    #[derive(Default)]
    pub struct ScoreView {
        pub score: i64,
        /// Absent when the forum's `policy.voting` is `karmaOnly` (`social.md` §7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub agreement: Option<i64>,
        /// "n votes not yet counted": counting karma votes by voters outside E.
        pub unweighted: u32,
        /// N of "N of M votes were cast on an earlier version".
        pub earlier_version_votes: u32,
        /// M of "N of M": the weighted karma votes (omitted when 0).
        #[serde(default, skip_serializing_if = "is_zero")]
        pub weighted_votes: u32,
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
    /// Aggregated reacts of one kind and quote on an item (`social.md` §9.2): the creator, the
    /// supporters (`for`, the creator included: everyone else "seconded" it) and the opposers
    /// (`against`). Sorted by palette order, then by the quote key. The viewer's own react that
    /// forms no valid tally is listed too, with `uncounted` and no creator.
    pub struct ReactTally {
        pub react: String,
        pub label: String,
        pub glyph: String,
        #[serde(rename = "for")]
        pub for_count: u32,
        pub against: u32,
        /// The creator's quote (`None`: the react applies to the whole item).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub quote: Option<TextQuote>,
        /// The quote resolved against the item's current text (absent while the body is not
        /// rendered or not fetched).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub anchor: Option<Anchor>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub creator: Option<String>,
        /// The viewer's stance, as their opinion record states it.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mine: Option<Stance>,
        /// The viewer's own react, which forms no valid react (no creator passes `reactCreate`,
        /// or the viewer's opinion does not count).
        #[serde(default, skip_serializing_if = "is_false")]
        pub uncounted: bool,
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
        /// The forum's banners (`newerRules`, `moderationUnavailable`; `social.md` §1.6, §4.11).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub banners: Vec<Banner>,
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
        /// The thread's root post: the thread the engine placed the comment in (not the
        /// record's own, author-controlled `root` field).
        pub root: String,
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
        /// The forum's banners (as [`PostList::banners`]).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub banners: Vec<Banner>,
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
        /// The forum's banners (as [`PostList::banners`]).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub banners: Vec<Banner>,
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
        /// The declaration's `reportContact` as written (`mailto:`, `https:` or `at://`;
        /// `data-model.md` §6.1): untrusted, shown only through `safe_href` in `Field` context
        /// (`lfm-1.md` §5).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub report_contact: Option<String>,
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
    /// What the viewer (the active account) may do in a forum, from the fold (`social.md` §4.9,
    /// §5, §8): clients enable or disable actions with it. Writers check gates as UX only; the
    /// fold decides what counts.
    #[derive(Default)]
    pub struct ViewerCaps {
        /// Karma votes count (O4: not banned or nullified; not restricted from `vote`). A
        /// counting vote carries weight only when `weighted`.
        pub vote: bool,
        /// Strong votes: the same as `vote` under `karma/1` (their power grows with KP).
        pub strong_vote: bool,
        /// Agreement votes count: `vote` and `policy.voting` ≠ `karmaOnly`.
        pub agree: bool,
        /// The viewer is an eligible voter (E): their counting votes carry weight; otherwise
        /// they are "recorded; will count once you're established".
        pub weighted: bool,
        /// Reacts count: `policy.voting = reacts`, not restricted from `react`, O4, and the
        /// viewer is in E (reacts by voters outside E are never counted).
        pub react: bool,
        /// `passes(reactCreate)`: a new react by the viewer is valid (with `react`).
        pub react_create: bool,
        /// `passes(reactSecond)`: the viewer seconds a react (with `react`).
        pub react_second: bool,
        /// `passes(reactAnti)`: the viewer's against-react counts (with `react`).
        pub react_anti: bool,
        /// The viewer's posts are listed (V2–V5, V8): a participant (or, in open mode, anyone
        /// whose first content carries a membership), not banned, not restricted from `post`,
        /// and in S when the view is degraded.
        pub post: bool,
        /// The same for comments (`comment` capability).
        pub comment: bool,
        /// Open mode: the viewer is a newcomer (P ∖ (S ∪ E), or not a participant yet), so their
        /// content is collapsed or hidden per `display.newUsers` (V8).
        pub newcomer: bool,
        /// Approval mode: the viewer has a membership record and no grant (an applicant).
        pub applicant: bool,
        /// Forum-tier moderation (the viewer is an authority).
        pub moderate: bool,
        /// The viewer's authority rank (1 moderator, 2 admin, 3 owner).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub rank: Option<u8>,
        /// Curation (`#tier{curated}`): an admin or the owner.
        pub curate: bool,
        /// Author moderation (`social.md` §4.9) on the viewer's own `personal` posts:
        /// `policy.authorModeration`, the viewer in E, and `passes(authorModPersonal)`.
        pub author_mod_personal: bool,
        /// The same on the viewer's own `frontpage` and `curated` posts
        /// (`passes(authorModFrontpage)`).
        pub author_mod_frontpage: bool,
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
        /// What the viewer may do (absent without an active account).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub viewer: Option<ViewerCaps>,
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
    /// A member of a forum: a DID with a valid membership record (`social.md` §3.1).
    pub struct MemberView {
        pub actor: ActorView,
        /// owner, admin, moderator; member (a seed: granted or an authority); established (in
        /// E, not a seed); newcomer (a participant outside S ∪ E, banned and nullified ones
        /// included); none (not a participant: an applicant, or a member of an approval forum
        /// without a grant).
        pub status: ForumRole,
        pub granted: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub membership_note: Option<String>,
        pub joined_at_ms: i64,
        /// `K` (omitted when 0).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub karma: Option<i64>,
        /// Approval mode: a membership without a grant, waiting for a decision.
        #[serde(default, skip_serializing_if = "is_false")]
        pub applicant: bool,
        /// `access = banned` (B).
        #[serde(default, skip_serializing_if = "is_false")]
        pub banned: bool,
        /// `nullifyVotes` (N).
        #[serde(default, skip_serializing_if = "is_false")]
        pub nullified: bool,
        /// The capabilities the member is restricted from (`#restrict`), sorted.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub restricted: Vec<String>,
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
    #[allow(clippy::large_enum_variant)]
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
        Reply, Mention, Quote, Pingback, NewComment, NewPost, Reacted, ModAction, KarmaChange,
    }
}

dto! {
    /// A notification (`social.md` §16). `atMs` is when the item was first seen (never its
    /// `createdAt`), or when a change (reacts, moderation, karma) was noticed; `forum` is the
    /// forum it belongs to.
    pub struct NotificationView {
        pub id: String,
        pub reason: NotificationReason,
        pub at_ms: i64,
        pub subject: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub forum: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub actor: Option<ActorView>,
        pub read: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub preview: Option<String>,
    }
}

dto! {
    /// Notifications, newest first; `unread` counts every unread notification (the badge).
    pub struct NotificationList {
        pub dto: DtoVersion,
        pub items: Vec<NotificationView>,
        pub unread: u32,
    }
}

dto! {
    /// A mod-queue item (`social.md` §4.12). `kind`: `applicant` (subject: the DID),
    /// `conflict` (subject: `tag:slug`), `stale` (subject: the edited record; `uri`: the stale
    /// modAction) or `newcomer` (subject: a newcomer's collapsed or hidden item).
    pub struct ModQueueItem {
        #[serde(rename = "kind")]
        pub item_kind: String,
        pub subject: String,
        pub reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub detail: Option<String>,
        /// The modAction a `stale` item is about.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub uri: Option<String>,
        /// The applicant, or the newcomer item's author.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub actor: Option<ActorView>,
        /// When the applicant joined, or the newcomer item's `t_rank`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub at_ms: Option<i64>,
    }
}

dto! {
    /// The mod queue.
    pub struct ModQueue {
        pub dto: DtoVersion,
        pub items: Vec<ModQueueItem>,
    }
}

string_enum! {
    /// The status of a mod-log entry (`social.md` §4.12).
    pub enum ModLogStatus {
        Effective, Overridden, Lapsed, Expired, Stale, Unauthorised, ReservedUnknown, Edited,
        Removed,
    }
}

dto! {
    /// A mod-log entry (`social.md` §4.12): a modAction, or a revision that edited (`action:
    /// "edit"`) or removed (`action: "remove"`) one. `action` is the modAction's action
    /// (`hide`, `access`, `restrict`, `tier`, `pin`, `lock`, `tag`, or `unknown`), `value` its
    /// asserted values (`hidden`, `visible`, `spam`, `banned`, `member`, `none`, `curated`,
    /// `pinned`, …, comma-separated). `reason` of a `#hide` with `showReason: false` is shown
    /// to the forum's authorities only.
    pub struct ModLogEntry {
        /// The modAction or revision uri.
        pub uri: String,
        pub actor: ActorView,
        pub action: String,
        pub subject: String,
        pub value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub reason: Option<String>,
        /// `updatedAt ?? createdAt` (0: a time before 1970).
        pub at_ms: i64,
        pub status: ModLogStatus,
        /// `status = lapsed`: by a former moderator (or a DID that never was one).
        pub lapsed: bool,
        /// The winning rank of an `overridden` entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub rank: Option<u8>,
        /// An author-scope assertion (`social.md` §4.9).
        #[serde(default, skip_serializing_if = "is_false")]
        pub author_scope: bool,
        /// `adopts`: the record this one re-issues (`mod adopt`).
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
        /// For a comment: the root post of the thread the engine placed it in (absent when the
        /// comment is not placed in a thread of a loaded forum).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub root: Option<String>,
        pub level: TrustLevel,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub record: Option<serde_json::Value>,
        pub validity: Vec<String>,
        /// Every modAction in the forum naming the uri (valid or not), as `{uri, cid, valid,
        /// rule?, status?, record}` by uri; `status` is the mod-log status (§4.12) of a listed
        /// one.
        pub assertions: Vec<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub state: Option<ItemState>,
        /// The fold's score of an item (hidden items included, `social.md` §7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub score: Option<ScoreView>,
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

string_enum! {
    /// How an account is authenticated.
    pub enum AuthKind { Oauth, AppPassword }
}

dto! {
    /// A logged-in account (`engine-api.md` §3.1 `accounts()`). The session itself lives in the
    /// host's secret store; this is only the local record of it.
    pub struct AccountView {
        pub did: String,
        /// The handle, if it was verified in both directions at login.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub handle: Option<String>,
        /// The PDS bound at login (`sync.md` §3.6).
        pub pds: String,
        pub auth_kind: AuthKind,
        pub active: bool,
    }
}

dto! {
    /// What `parse_ref` found (`engine-api.md` §3.1): a forum, post, comment or sequence uri,
    /// another record's uri, or an actor's DID.
    pub enum Ref {
        Forum { uri: String },
        Post { uri: String },
        Comment { uri: String },
        Sequence { uri: String },
        Record { uri: String },
        Actor { did: String },
    }
}

dto! {
    /// The context of `parse_ref`.
    #[derive(Default)]
    pub struct RefContext {
        /// The current forum, for `handle/rkey` post references.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub forum: Option<String>,
    }
}

// ---------------------------------------------------------------------------------------------
// §3.5 Writes

dto! {
    /// The state of a write (`engine-api.md` §3.5): `pending → sending → done | failed | undone`.
    /// `pending` may say why it waits (held for the undo window, throttled by the write budget,
    /// a rate limit or a back-off, or `needsLogin`: the account's session was refused and the
    /// write waits until the account logs in again), until when, and the `error` that caused
    /// the wait; a conflict is `failed` with code `conflict`.
    pub enum WriteState {
        #[serde(rename_all = "camelCase")]
        Pending {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            not_before_ms: Option<i64>,
            #[serde(default, skip_serializing_if = "is_false")]
            held: bool,
            #[serde(default, skip_serializing_if = "is_false")]
            throttled: bool,
            #[serde(default, skip_serializing_if = "is_false")]
            needs_login: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            error: Option<crate::engine::EngineError>,
        },
        Sending,
        /// `cid` is absent for deletes.
        Done {
            uri: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            cid: Option<String>,
        },
        Failed { error: crate::engine::EngineError },
        Undone,
    }
}

dto! {
    /// What a write operation returns once the write is persisted (`engine-api.md` §3.5).
    pub struct WriteTicket {
        pub id: String,
        pub state: WriteState,
    }
}

// ---------------------------------------------------------------------------------------------
// §3 Operation inputs (request types; shapes adopted from `packages/engine/src/types.ts`)

dto! {
    /// A moderator entry of a [`ForumDraft`].
    pub struct ModeratorDraft {
        pub did: String,
        pub role: ModRole,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
    }
}

string_enum! {
    /// `forum.defaultTier` values a declaration may write.
    pub enum DefaultTier { Personal, Frontpage }
}

dto! {
    /// `forum_create` / `forum_update` input (`data-model.md` §6.1).
    pub struct ForumDraft {
        /// The forum rkey (`forum_create` only; fixed afterwards).
        pub slug: String,
        pub name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub about: Option<BodyDraft>,
        pub membership: MembershipMode,
        pub default_tier: DefaultTier,
        #[serde(default)]
        pub featured_tags: Vec<String>,
        #[serde(default)]
        pub moderators: Vec<ModeratorDraft>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub report_contact: Option<String>,
    }
}

impl ForumDraft {
    /// A draft with the defaults `forum create` writes (`approval`, `personal`).
    pub fn new(slug: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            name: name.into(),
            description: None,
            about: None,
            membership: MembershipMode::Approval,
            default_tier: DefaultTier::Personal,
            featured_tags: Vec::new(),
            moderators: Vec::new(),
            report_contact: None,
        }
    }
}

dto! {
    /// `members` filter.
    #[derive(Default)]
    pub struct MembersQuery {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub status: Option<ForumRole>,
        /// Applicants only (approval mode: a membership record and no grant).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub pending: Option<bool>,
    }
}

dto! {
    /// `sync_forum` / `sync_thread` options (`engine-api.md` §3.3).
    #[derive(Default)]
    pub struct SyncOpts {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub budget_ms: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_requests: Option<u32>,
        #[serde(default)]
        pub full: bool,
    }
}

dto! {
    /// `list_posts` input (`social.md` §12).
    pub struct ListPostsQuery {
        pub forum: String,
        pub sort: PostSort,
        pub filter: PostFilter,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tag: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub kind: Option<PostKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub author: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cursor: Option<String>,
        pub limit: u32,
    }
}

impl ListPostsQuery {
    /// The default view of `forum`: `magic` over `frontpage`, one page.
    pub fn new(forum: impl Into<String>) -> Self {
        Self {
            forum: forum.into(),
            sort: PostSort::Magic,
            filter: PostFilter::Frontpage,
            tag: None,
            kind: None,
            author: None,
            cursor: None,
            limit: crate::limits::LIST_PAGE_DEFAULT,
        }
    }
}

dto! {
    /// `get_thread` options.
    pub struct ThreadOpts {
        pub sort: CommentSort,
        #[serde(default)]
        pub unread_only: bool,
    }
}

impl Default for ThreadOpts {
    fn default() -> Self {
        Self {
            sort: CommentSort::Magic,
            unread_only: false,
        }
    }
}

dto! {
    /// A local, not yet uploaded blob (`prepare_media`, M4).
    pub struct MediaRef {
        pub id: String,
        pub cid: String,
        pub mime: String,
        pub size_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub alt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub aspect_ratio: Option<AspectRatio>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
    }
}

dto! {
    /// A file handed to `prepare_media` (`engine-api.md` §3.6, M4): the bytes, the declared type
    /// (checked against the sniffed one, `data-model.md` §8.8) and the display hints.
    pub struct MediaInput {
        /// The declared `mimeType`. When it disagrees with the sniffed type the sniffed one wins
        /// (and a type outside the §8.8 table is refused).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub declared_mime: Option<String>,
        /// The accessible description; writers warn when it is missing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub alt: Option<String>,
        /// Opt-in, display-only, basename only (`data-model.md` §8.7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        /// A layout hint (display clamps it to between 1:20 and 20:1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub aspect_ratio: Option<AspectRatio>,
    }
}

dto! {
    /// A markdown body to write.
    #[derive(Default)]
    pub struct BodyDraft {
        pub markdown: String,
        #[serde(default)]
        pub media: Vec<MediaRef>,
    }
}

impl BodyDraft {
    /// A markdown-only body.
    pub fn text(markdown: impl Into<String>) -> Self {
        Self {
            markdown: markdown.into(),
            media: Vec::new(),
        }
    }
}

dto! {
    /// A link post's card.
    pub struct ExternalLinkDraft {
        pub uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
    }
}

dto! {
    /// `publish` input.
    pub struct PostDraft {
        pub forum: String,
        pub kind: PostKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        pub body: BodyDraft,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub link: Option<ExternalLinkDraft>,
        #[serde(default)]
        pub tags: Vec<String>,
        #[serde(default)]
        pub labels: Vec<String>,
    }
}

impl PostDraft {
    /// An article.
    pub fn article(
        forum: impl Into<String>,
        title: impl Into<String>,
        markdown: impl Into<String>,
    ) -> Self {
        Self {
            forum: forum.into(),
            kind: PostKind::Article,
            title: Some(title.into()),
            body: BodyDraft::text(markdown),
            link: None,
            tags: Vec::new(),
            labels: Vec::new(),
        }
    }
}

dto! {
    /// `comment` input.
    pub struct CommentDraft {
        /// The thread's post.
        pub root: String,
        /// The comment replied to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub parent: Option<String>,
        pub body: BodyDraft,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub quote: Option<TextQuote>,
    }
}

impl CommentDraft {
    /// A reply to `parent` (or a top-level comment) in the thread of `root`.
    pub fn new(
        root: impl Into<String>,
        parent: Option<String>,
        markdown: impl Into<String>,
    ) -> Self {
        Self {
            root: root.into(),
            parent,
            body: BodyDraft::text(markdown),
            quote: None,
        }
    }
}

dto! {
    /// A draft for `edit` and for the local draft store, tagged by what is drafted.
    pub enum Draft {
        Post {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            id: Option<String>,
            post: PostDraft,
        },
        Comment {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            id: Option<String>,
            comment: CommentDraft,
        },
    }
}

dto! {
    /// `edit` options (`engine-api.md` §3.5).
    pub struct EditOpts {
        /// Snapshot the old version in a revision (`false`: `--no-history`).
        pub history: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub summary: Option<String>,
    }
}

impl Default for EditOpts {
    fn default() -> Self {
        Self {
            history: true,
            summary: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// §3.5 Social writes, §3.6 local state (M3): opinions, moderation, notifications, local lists

string_enum! {
    /// Restrictable capabilities (`data-model.md` §6.7 `#restrict`).
    pub enum Capability { Post, Comment, Vote, React, Tag, Wiki, Sequence }
}

string_enum! {
    /// A modAction's scope (`data-model.md` §6.7): the forum tier, or the post author's tier
    /// (`social.md` §4.9).
    pub enum ModScope { Forum, Author }
}

string_enum! {
    /// `#access.level` (`data-model.md` §6.7).
    pub enum AccessLevel { Banned, Member }
}

string_enum! {
    /// `#pin.scope`: a post or sequence in the forum's lists, or a comment in its thread.
    pub enum PinScope { Forum, Thread }
}

string_enum! {
    /// Public actions with a first-use interstitial (`social.md` §17, `threat-model.md`
    /// PRIV-9): vote or agree, react, tag vote, join or apply, mod action.
    pub enum PublicActionKind { Vote, React, TagVote, Join, ModAction }
}

dto! {
    /// `opinion(subject, OpinionIntent)` (`engine-api.md` §3.5, `data-model.md` §8.5): one
    /// change of the viewer's opinion on an item. An absent `value` clears the axis or tag vote.
    /// `React.quote` anchors the react to a passage (only `exact` is needed: the engine derives
    /// `prefix`, `suffix` and `start` from the item's current text, W8).
    pub enum OpinionIntent {
        Karma {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            value: Option<KarmaValue>,
        },
        Agreement {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            value: Option<AgreementValue>,
        },
        React {
            id: String,
            stance: Stance,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            quote: Option<TextQuote>,
            #[serde(default)]
            remove: bool,
        },
        Tag {
            slug: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            value: Option<KarmaValue>,
        },
    }
}

impl OpinionIntent {
    /// The public action whose first-use interstitial covers this intent (`social.md` §17).
    pub fn public_kind(&self) -> PublicActionKind {
        match self {
            Self::Karma { .. } | Self::Agreement { .. } => PublicActionKind::Vote,
            Self::React { .. } => PublicActionKind::React,
            Self::Tag { .. } => PublicActionKind::TagVote,
        }
    }
}

/// The default of `#hide.showReason`.
fn yes() -> bool {
    true
}

dto! {
    /// A modAction `action` as a client asks for it (`data-model.md` §6.7). A record states the
    /// complete desired state of its action key (`social.md` §4.2): a `restrict` lists every
    /// capability, a `hide` says whether it is spam. `lock.allowed` is computed by the engine
    /// from its view of the thread (`data-model.md` §8.6); `tag.head` is a wikiRevision uri
    /// (its CID is the cached version's).
    pub enum ModActionValue {
        #[serde(rename_all = "camelCase")]
        Hide {
            hidden: bool,
            #[serde(default)]
            spam: bool,
            #[serde(default = "yes")]
            show_reason: bool,
        },
        Access { level: AccessLevel },
        #[serde(rename_all = "camelCase")]
        Restrict {
            capabilities: Vec<Capability>,
            #[serde(default)]
            nullify_votes: bool,
        },
        Tier {
            tier: Tier,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            notice: Option<BodyDraft>,
        },
        Pin { scope: PinScope, pinned: bool },
        Lock { locked: bool },
        Tag {
            removed: bool,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            head: Option<String>,
        },
    }
}

impl ModActionValue {
    /// `actionKey` (`data-model.md` §3.1): the definition name, or `"pin:" + scope`.
    pub fn action_key(&self) -> &'static str {
        match self {
            Self::Hide { .. } => "hide",
            Self::Access { .. } => "access",
            Self::Restrict { .. } => "restrict",
            Self::Tier { .. } => "tier",
            Self::Pin {
                scope: PinScope::Forum,
                ..
            } => "pin:forum",
            Self::Pin {
                scope: PinScope::Thread,
                ..
            } => "pin:thread",
            Self::Lock { .. } => "lock",
            Self::Tag { .. } => "tag",
        }
    }
}

dto! {
    /// `mod_action(ModActionIntent)` (`data-model.md` §6.7, §8.4, §8.6). `assert` creates the
    /// acting account's record for (scope, subject, action key), or updates it through the edit
    /// protocol; `remove` deletes it through the removal protocol (unhide, unban, unpin,
    /// unlock, unrestrict, …). `subject` is a DID (or handle), an at-uri or `tag:<slug>`.
    pub enum ModActionIntent {
        #[serde(rename_all = "camelCase")]
        Assert {
            forum: String,
            scope: ModScope,
            subject: String,
            action: ModActionValue,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            reason: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            until_ms: Option<i64>,
        },
        #[serde(rename_all = "camelCase")]
        Remove {
            forum: String,
            scope: ModScope,
            subject: String,
            action_key: String,
        },
    }
}

dto! {
    /// `mods_set(forum, ModsChange)` (`data-model.md` §8.4, §8.6): add (or change the role of)
    /// a moderator, or remove one. A removal runs `mod adopt` first and edits the declaration
    /// only once the adoption is done; `skipAdopt` is the opt-out for a rogue moderator, which
    /// clients offer only after a confirmation listing every assertion that will lapse.
    pub enum ModsChange {
        Add {
            did: String,
            role: ModRole,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            title: Option<String>,
        },
        #[serde(rename_all = "camelCase")]
        Remove {
            did: String,
            #[serde(default)]
            skip_adopt: bool,
        },
    }
}

string_enum! {
    /// What a notice on an action's result is about.
    pub enum NoticeCode { NotEstablished, Retroactive, AdoptedFirst }
}

dto! {
    /// A notice clients show with an action's result, e.g. "recorded; will count once you're
    /// established" (`code` is stable; `message` is English text).
    pub struct ActionNotice {
        pub code: NoticeCode,
        pub message: String,
    }
}

dto! {
    /// What `opinion`, `mod_action` and `mods_set` return: the write's ticket (as
    /// [`WriteTicket`]) and the notices to show with it.
    pub struct ActionTicket {
        pub id: String,
        pub state: WriteState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub notices: Vec<ActionNotice>,
    }
}

impl ActionTicket {
    /// The ticket without notices.
    pub fn new(t: WriteTicket) -> Self {
        Self {
            id: t.id,
            state: t.state,
            notices: Vec::new(),
        }
    }

    /// Adds a notice.
    pub fn with_notice(mut self, code: NoticeCode, message: impl Into<String>) -> Self {
        self.notices.push(ActionNotice {
            code,
            message: message.into(),
        });
        self
    }

    /// The plain [`WriteTicket`].
    pub fn ticket(&self) -> WriteTicket {
        WriteTicket {
            id: self.id.clone(),
            state: self.state.clone(),
        }
    }
}

dto! {
    /// `notifications(NotificationsQuery)` (`engine-api.md` §3.4).
    pub struct NotificationsQuery {
        /// `false`: unread only; `true`: everything.
        #[serde(default)]
        pub all: bool,
        pub limit: u32,
    }
}

impl Default for NotificationsQuery {
    fn default() -> Self {
        Self {
            all: false,
            limit: 50,
        }
    }
}

string_enum! {
    /// How often karma changes are notified (`social.md` §16; default daily).
    pub enum KarmaCadence { Realtime, Daily, Weekly }
}

dto! {
    /// A local subscription (`social.md` §16): new posts in a forum, by a user or with a tag;
    /// new comments on a post; new entries of a sequence.
    #[derive(Eq, PartialOrd, Ord)]
    pub enum Subscription {
        Forum { uri: String },
        Post { uri: String },
        User { did: String },
        Tag { forum: String, slug: String },
        Sequence { uri: String },
    }
}

dto! {
    /// A subscription and since when (ms).
    pub struct SubscriptionEntry {
        pub subscription: Subscription,
        pub since_ms: i64,
    }
}

dto! {
    /// A local bookmark, with the post's summary when its forum is loaded.
    pub struct BookmarkEntry {
        pub uri: String,
        pub saved_at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub post: Option<PostSummary>,
    }
}

dto! {
    /// Unread state of one followed forum (`social.md` §16): listed posts and comments first
    /// seen since the last visit (never by `createdAt`), and unread notifications.
    pub struct ForumUnread {
        pub forum: String,
        pub posts: u32,
        pub comments: u32,
        pub notifications: u32,
    }
}

dto! {
    /// `unread_counts()`: the notification badge and the unread state of every followed forum
    /// (sorted by forum uri).
    pub struct UnreadCounts {
        pub dto: DtoVersion,
        pub notifications: u32,
        pub forums: Vec<ForumUnread>,
    }
}

// ---------------------------------------------------------------------------------------------
// §3.8 Events

dto! {
    /// What changed (`engine-api.md` §3.8): UIs re-query the views of a topic.
    #[derive(Eq, PartialOrd, Ord)]
    pub enum Topic {
        Forum { uri: String },
        Thread { uri: String },
        User { did: String },
        Notifications,
        Outbox,
        Drafts,
        Accounts,
    }
}

dto! {
    /// The state of an account's session.
    pub enum AuthState {
        Ok,
        #[serde(rename_all = "camelCase")]
        Expiring { in_ms: i64 },
        Expired,
    }
}

string_enum! {
    /// Warning kinds.
    pub enum WarningKind { Equivocation, IndexerContradiction, Degraded, RenderFailure, Other }
}

dto! {
    /// An engine event (`engine-api.md` §3.8), delivered in order and never re-entrantly.
    pub enum Event {
        Changed { topics: Vec<Topic> },
        SyncProgress { forum: String, done: u32, total: u32 },
        Write { id: String, state: WriteState },
        Auth { did: String, state: AuthState },
        /// `Warning { kind, source?, message }`: the warning's own kind travels as `warning`.
        Warning {
            warning: WarningKind,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            source: Option<String>,
            message: String,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_ticket_shapes() {
        let t = WriteTicket {
            id: "did:plc:x/1".into(),
            state: WriteState::Pending {
                not_before_ms: None,
                held: false,
                throttled: false,
                needs_login: false,
                error: None,
            },
        };
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"id":"did:plc:x/1","state":{"kind":"pending"}}"#
        );
        let held = WriteState::Pending {
            not_before_ms: Some(5),
            held: true,
            throttled: false,
            needs_login: false,
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&held).unwrap(),
            r#"{"kind":"pending","notBeforeMs":5,"held":true}"#
        );
        let login = WriteState::Pending {
            not_before_ms: None,
            held: false,
            throttled: false,
            needs_login: true,
            error: Some(crate::engine::EngineError::new(
                crate::engine::ErrorCode::AuthRequired,
                "expired",
            )),
        };
        let json = serde_json::to_string(&login).unwrap();
        assert!(
            json.starts_with(
                r#"{"kind":"pending","needsLogin":true,"error":{"code":"auth_required""#
            ),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<WriteState>(&json).unwrap(), login);
        let done = WriteState::Done {
            uri: "at://x/y/z".into(),
            cid: None,
        };
        assert_eq!(
            serde_json::to_string(&done).unwrap(),
            r#"{"kind":"done","uri":"at://x/y/z"}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteState::Undone).unwrap(),
            r#"{"kind":"undone"}"#
        );
    }

    #[test]
    fn item_state_shapes() {
        let s = ItemState::Hidden {
            reason: HiddenReason::Removed {
                rank: 1,
                actor: None,
                spam: false,
                reason: None,
            },
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"kind":"hidden","reason":{"kind":"removed","rank":1,"spam":false}}"#
        );
        let c = ItemState::Collapsed {
            reason: CollapseReason::NewUser,
        };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"kind":"collapsed","reason":"newUser"}"#
        );
    }

    #[test]
    fn badges_omit_false() {
        let b = Badges {
            edited: true,
            ..Default::default()
        };
        assert_eq!(serde_json::to_string(&b).unwrap(), r#"{"edited":true}"#);
    }

    #[test]
    fn account_view_shape() {
        let a = AccountView {
            did: "did:plc:ewvi7nxzyoun6zhxrhs64oiz".into(),
            handle: None,
            pds: "https://pds.example.com".into(),
            auth_kind: AuthKind::AppPassword,
            active: true,
        };
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"did":"did:plc:ewvi7nxzyoun6zhxrhs64oiz","pds":"https://pds.example.com","authKind":"appPassword","active":true}"#
        );
        assert_eq!(
            serde_json::to_string(&AuthKind::Oauth).unwrap(),
            r#""oauth""#
        );
    }

    #[test]
    fn event_shapes() {
        let e = Event::Changed {
            topics: vec![
                Topic::Forum {
                    uri: "at://x/y/z".into(),
                },
                Topic::Outbox,
            ],
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"kind":"changed","topics":[{"kind":"forum","uri":"at://x/y/z"},{"kind":"outbox"}]}"#
        );
        let p = Event::SyncProgress {
            forum: "f".into(),
            done: 1,
            total: 2,
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"kind":"syncProgress","forum":"f","done":1,"total":2}"#
        );
        let w = Event::Warning {
            warning: WarningKind::Degraded,
            source: None,
            message: "m".into(),
        };
        assert_eq!(
            serde_json::to_string(&w).unwrap(),
            r#"{"kind":"warning","warning":"degraded","message":"m"}"#
        );
        let a = Event::Auth {
            did: "d".into(),
            state: AuthState::Expiring { in_ms: 5 },
        };
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"kind":"auth","did":"d","state":{"kind":"expiring","inMs":5}}"#
        );
    }

    #[test]
    fn request_shapes() {
        let q: ListPostsQuery =
            serde_json::from_str(r#"{"forum":"at://f","sort":"new","filter":"all","limit":5}"#)
                .unwrap();
        assert_eq!(q.sort, PostSort::New);
        let d: Draft = serde_json::from_str(
            r#"{"kind":"comment","comment":{"root":"at://p","body":{"markdown":"hi"}}}"#,
        )
        .unwrap();
        assert!(matches!(d, Draft::Comment { id: None, .. }));
        let f = ForumDraft::new("meta", "Meta");
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"slug":"meta","name":"Meta","membership":"approval","defaultTier":"personal","featuredTags":[],"moderators":[]}"#
        );
    }

    #[test]
    fn dto_version_is_one() {
        assert_eq!(serde_json::to_string(&DtoVersion::default()).unwrap(), "1");
    }
}
