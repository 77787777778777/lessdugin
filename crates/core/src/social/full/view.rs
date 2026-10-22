// SPDX-License-Identifier: AGPL-3.0-or-later
//! `ForumView` (`social.md` §1.1): every layer's results for one forum, plus the shared types
//! of the layers (dimensions, resolutions, statuses, visibility states).
//!
//! ```text
//! ForumView
//!   per forum : policy, banners, degraded, inputDigest, mod log, mod queue, raw annotations,
//!               threads (placement), orderings (per Query)
//!   per user  : authorities A and ranks (L0); members, P, G, B, N, S, applicants (L2);
//!               newcomers; K1, seedUp, E, KP, K (L3); gate passes (L4)
//!   per item  : resolved moderation dimensions with winners (L1), tier; t_rank and flags;
//!               V1–V7 states; score, agreement (L3.5); reacts, tags (L5); V1–V10 states and
//!               the thread pass (L6)
//! ```
//!
//! **Stubs.** A layer this build does not compute yet is a [`Stage`] holding [`Pending`], which
//! names the stub function and the spec section it implements. Nothing reads a pending layer's
//! values: every layer that depends on a pending one is pending too, so no output is ever a
//! silent default. Layers L0–L2, time, threads, V1–V5 and V7, the mod log of forum-scope
//! records, the mod queue and `inputDigest` are implemented.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::values::{AccessLevel, Capability, MembershipMode, Tier};

use crate::social::full::author::AuthorTier;
use crate::social::full::gates::Gates;
use crate::social::full::karma::Karma;
use crate::social::full::moderation::Moderation;
use crate::social::full::query::Query;
use crate::social::full::ranking::Orderings;
use crate::social::full::reacts::ReactTally;
use crate::social::full::scores::Score;
use crate::social::full::sequences::Library;
use crate::social::full::visibility::Visibility;
use crate::social::full::wiki::Wiki;
use crate::social::policy::EffectivePolicy;
use crate::social::threads::Placement;

/// A layer this build does not compute yet: the stub that would compute it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pending {
    /// The stub function (`module::function`).
    pub stub: &'static str,
    /// The layer (§1.4).
    pub layer: &'static str,
    /// The spec sections it implements (`social.md`).
    pub section: &'static str,
}

impl core::fmt::Display for Pending {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} ({}, {})", self.stub, self.layer, self.section)
    }
}

/// A layer result, or the stub it waits for.
pub type Stage<T> = Result<T, Pending>;

/// A forum-tier moderation dimension (§4.2; the vector names of `testdata/vectors/README.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dim {
    /// `hidden` (`#hide`).
    Hidden,
    /// `spam` = hidden ∧ spam (`#hide`).
    Spam,
    /// `access` (`#access`).
    Access,
    /// `restrict`, a capability set (`#restrict`).
    Restrict,
    /// `nullify` (`#restrict.nullifyVotes`).
    Nullify,
    /// `tier` (`#tier`).
    Tier,
    /// `pin[forum]` (`#pin{scope: forum}`).
    PinForum,
    /// `pin[thread]` (`#pin{scope: thread}`).
    PinThread,
    /// `locked` (`#lock`).
    Locked,
    /// `tag.removed` (`#tag`).
    TagRemoved,
    /// `tag.head` (`#tag`, only when `head` is present).
    TagHead,
}

impl Dim {
    /// Every dimension.
    pub const ALL: [Dim; 11] = [
        Dim::Hidden,
        Dim::Spam,
        Dim::Access,
        Dim::Restrict,
        Dim::Nullify,
        Dim::Tier,
        Dim::PinForum,
        Dim::PinThread,
        Dim::Locked,
        Dim::TagRemoved,
        Dim::TagHead,
    ];

    /// The vector name.
    pub const fn name(self) -> &'static str {
        match self {
            Dim::Hidden => "hidden",
            Dim::Spam => "spam",
            Dim::Access => "access",
            Dim::Restrict => "restrict",
            Dim::Nullify => "nullify",
            Dim::Tier => "tier",
            Dim::PinForum => "pin.forum",
            Dim::PinThread => "pin.thread",
            Dim::Locked => "locked",
            Dim::TagRemoved => "tag.removed",
            Dim::TagHead => "tag.head",
        }
    }
}

/// The value a record asserts for a dimension.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DimValue {
    /// `hidden`, `spam`, `nullify`, `pin[*]`, `locked`, `tag.removed`.
    Bool(bool),
    /// `access`.
    Access(AccessLevel),
    /// `tier`.
    Tier(Tier),
    /// `restrict`: the known capabilities (MA8).
    Caps(BTreeSet<Capability>),
    /// `tag.head`: the strong reference.
    Head {
        /// `head.uri`.
        uri: String,
        /// `head.cid`.
        cid: String,
    },
}

/// `resolve(T, d) = Resolved{value, rank, winner}` (§4.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The resolved value.
    pub value: DimValue,
    /// The winning rank r.
    pub rank: u8,
    /// The winning record's uri (the smallest uri among the rank-r entries with the value; for
    /// `restrict`, among all rank-r entries).
    pub winner: String,
    /// `allowed(p)` (§4.4): only for `locked` = true; the MA9-filtered allow-lists of the rank-r
    /// locking records, intersected.
    pub allowed: Option<BTreeSet<String>>,
}

/// A mod-log status (§4.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModStatus {
    /// In effect.
    Effective,
    /// Counted but beaten, with the winning rank.
    Overridden {
        /// The highest rank among the resolutions of the record's dimensions (author scope: the
        /// forum-tier value's rank).
        rank: u8,
    },
    /// The actor is not in A (§4.10).
    Lapsed,
    /// `until` has passed.
    Expired,
    /// A positive record's CID check failed (§4.5), or a `#tag.head` check failed.
    Stale,
    /// Rank, role, gate or author condition failed.
    Unauthorised,
    /// No effect in v1 (MA2 reserved key, MA7 unknown scope, action or value, MA9 forum lock).
    ReservedUnknown,
    /// A modAction's `op: update` revision.
    Edited,
    /// A modAction's `op: delete` revision.
    Removed,
}

impl ModStatus {
    /// The vector name.
    pub const fn name(self) -> &'static str {
        match self {
            ModStatus::Effective => "effective",
            ModStatus::Overridden { .. } => "overridden",
            ModStatus::Lapsed => "lapsed",
            ModStatus::Expired => "expired",
            ModStatus::Stale => "stale",
            ModStatus::Unauthorised => "unauthorised",
            ModStatus::ReservedUnknown => "reserved/unknown",
            ModStatus::Edited => "edited",
            ModStatus::Removed => "removed",
        }
    }
}

/// One mod-log entry (§4.12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModLogEntry {
    /// The modAction or revision uri.
    pub uri: String,
    /// Its status.
    pub status: ModStatus,
    /// `µs(updatedAt) ?? µs(createdAt)` (modAction) or `µs(createdAt)` (revision).
    pub time: Option<i64>,
}

/// A mod-queue item the fold provides (§4.12), ordered by (type, key bytes).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueueItem {
    /// An applicant (approval mode, §3.3); the key is the DID.
    Applicant(String),
    /// An unordered `tag.head` conflict at the top rank; the key is the `tag:` subject.
    Conflict(String),
    /// A stale positive assertion (or `#tag.head`) whose subject (or head) was edited; the key
    /// is the modAction uri.
    Stale(String),
}

/// The `Removed{…}` details of V3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removed {
    /// The winning rank of `hidden`.
    pub rank: u8,
    /// The winner's actor.
    pub actor: String,
    /// `effective(x, spam)`.
    pub spam: bool,
    /// The winner's `reason`, if it has one and `showReason` holds.
    pub reason: Option<String>,
}

/// Why an item is hidden (§10.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HiddenReason {
    /// V2: the author is not a participant.
    NotParticipant,
    /// V2: degraded view and the author is not a seed (§4.11).
    ModerationUnavailable,
    /// V3: forum-tier hide.
    Removed(Removed),
    /// V4: the author is banned.
    Banned,
    /// V5: the author is restricted from the item's capability.
    Restricted,
    /// V6: author-tier moderation (§4.9).
    AuthorModerated,
    /// V7: locked thread.
    Locked,
    /// V8 with `display.newUsers = hide`.
    NewUser,
}

impl HiddenReason {
    /// The vector name.
    pub const fn name(&self) -> &'static str {
        match self {
            HiddenReason::NotParticipant => "NotParticipant",
            HiddenReason::ModerationUnavailable => "ModerationUnavailable",
            HiddenReason::Removed(_) => "Removed",
            HiddenReason::Banned => "Banned",
            HiddenReason::Restricted => "Restricted",
            HiddenReason::AuthorModerated => "AuthorModerated",
            HiddenReason::Locked => "Locked",
            HiddenReason::NewUser => "NewUser",
        }
    }
}

/// Why an item is collapsed (§10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapseReason {
    /// V8 with `display.newUsers = collapse`.
    NewUser,
    /// V9.
    LowScore,
}

impl CollapseReason {
    /// The vector name.
    pub const fn name(self) -> &'static str {
        match self {
            CollapseReason::NewUser => "NewUser",
            CollapseReason::LowScore => "LowScore",
        }
    }
}

/// A visibility state (§10.1, §10.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// V1.
    Excluded,
    /// V10.
    Visible,
    /// V8, V9.
    Collapsed(CollapseReason),
    /// V2–V8.
    Hidden(HiddenReason),
    /// The thread pass: a hidden post or comment with a listed descendant (§10.2).
    Tombstone(HiddenReason),
}

impl State {
    /// Listed: `Visible` or `Collapsed(*)`.
    pub fn listed(&self) -> bool {
        matches!(self, State::Visible | State::Collapsed(_))
    }
}

/// L2 participation (§3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participation {
    /// The effective membership mode.
    pub mode: MembershipMode,
    /// `member(d)`: a valid membership record with rkey fk and `forum = F`.
    pub members: BTreeSet<String>,
    /// P.
    pub p: BTreeSet<String>,
    /// G: `access = member`.
    pub g: BTreeSet<String>,
    /// B: `access = banned`.
    pub b: BTreeSet<String>,
    /// N: `nullify = true`.
    pub n: BTreeSet<String>,
    /// S = (A ∪ G) ∖ (B ∪ N).
    pub s: BTreeSet<String>,
    /// Applicants: `{ d ∉ A : member(d) ∧ access(d) = none }`, approval mode only (§3.3).
    pub applicants: BTreeSet<String>,
}

/// The time values of a candidate item not Excluded by V1 (§2.2, §2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemTime {
    /// `t_rank` (µs).
    pub t_rank: i64,
    /// `[future-dated]`.
    pub future: bool,
    /// "inconsistent timestamps".
    pub inconsistent: bool,
}

/// Raw-view annotations (§4.12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Raw {
    /// The first failing rule of every invalid record in `records`.
    pub invalid: BTreeMap<String, String>,
    /// Every listed modAction (valid, `forum = F`, any scope and status) by subject, sorted by
    /// uri; the statuses are in the mod log.
    pub assertions: BTreeMap<String, Vec<String>>,
}

/// A banner (§1.6, §4.11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    /// A stable code: `newerRules` or `moderationUnavailable`.
    pub code: String,
    /// The text.
    pub message: String,
}

/// `ForumView` (§1.1): the full fold's result.
#[derive(Clone, Debug, PartialEq)]
pub struct ForumView {
    /// The NSID prefix the fold ran under.
    pub ns: String,
    /// F.
    pub forum: String,
    /// `did(F)`.
    pub owner: String,
    /// `fk(F)`.
    pub fk: String,
    /// The query the orderings answer.
    pub query: Query,
    /// The effective policy (§1.6).
    pub policy: EffectivePolicy,
    /// "this forum uses rules newer than your client" (§1.6: any of the ten knownValue fields
    /// fell back, `moderators[].role` included).
    pub newer_rules_banner: bool,
    /// §4.11: an authority's repo is `Unavailable`.
    pub degraded: bool,
    /// The banners due.
    pub banners: Vec<Banner>,
    /// `inputDigest` (§1.2).
    pub input_digest: String,
    /// The digest preimage D (§1.2).
    pub digest_preimage: Vec<u8>,

    /// L0: A with ranks (§4.1).
    pub authorities: BTreeMap<String, u8>,

    /// L1: forum-tier moderation (§4.2–§4.8, §4.10): parsed records, statuses, resolutions.
    pub moderation: Moderation,
    /// `effective(p, tier)` for every post not Excluded.
    pub tiers: BTreeMap<String, Tier>,
    /// The mod-queue items the fold provides (§4.12), sorted.
    pub mod_queue: Vec<QueueItem>,
    /// Raw-view annotations (§4.12).
    pub raw: Raw,

    /// L2: participation and seeds (§3).
    pub participation: Participation,

    /// `t_decl` (µs, §2.2).
    pub t_decl: i64,
    /// Time values of every candidate item not Excluded (§2.2).
    pub times: BTreeMap<String, ItemTime>,
    /// Placed threads (§11 steps 1–4) by root post uri: every non-Excluded post, and every root
    /// named by a non-Excluded comment.
    pub threads: BTreeMap<String, Placement>,
    /// Valid `op: update` revisions per subject uri, in TID order (§11 step 6).
    pub revisions: BTreeMap<String, Vec<String>>,
    /// Items with the `[edited]` badge (§11 "Display").
    pub edited: BTreeSet<String>,

    /// V1: the Excluded candidate items.
    pub excluded: BTreeSet<String>,
    /// The first matching rule of V1–V5 for every candidate item (`None`: none matches). These
    /// rules read only validity and L0–L2, so L5 may use them (§1.4).
    pub v1_v5: BTreeMap<String, Option<State>>,
    /// Comments meeting V7's condition (forum-tier lock), whatever earlier rule matches.
    pub locked_out: BTreeSet<String>,
    /// The first matching rule of V1–V7 for every candidate item (`None`: V8–V10 decide).
    pub mod_states: Stage<BTreeMap<String, Option<State>>>,
    /// The mod log (§4.12), in order (time desc, uri asc; no time last).
    pub mod_log: Stage<Vec<ModLogEntry>>,

    /// L3: opinion validity and karma (§5, §6).
    pub karma: Stage<Karma>,
    /// newcomers = P ∖ (S ∪ E) (§3.1).
    pub newcomers: Stage<BTreeSet<String>>,
    /// L3.5: scores (§7) of every candidate item not Excluded.
    pub scores: Stage<BTreeMap<String, Score>>,
    /// L4: gate passes (§8).
    pub gates: Stage<Gates>,
    /// L5: author-tier moderation (§4.9).
    pub author: Stage<AuthorTier>,
    /// L5: valid reacts per subject (§9.2), each list sorted by (react, qkey).
    pub reacts: Stage<BTreeMap<String, Vec<ReactTally>>>,
    /// L5: wiki heads, tag relevance and application (§13).
    pub wiki: Stage<Wiki>,
    /// L5: the sequence library (§14).
    pub library: Stage<Library>,
    /// L6: V1–V10 states and the thread pass (§10.1, §10.2).
    pub visibility: Stage<Visibility>,
    /// L6: the orderings the query asks for (§12).
    pub orderings: Stage<Orderings>,
}

impl ForumView {
    /// `true` if `did` is an authority.
    pub fn is_authority(&self, did: &str) -> bool {
        self.authorities.contains_key(did)
    }
}
