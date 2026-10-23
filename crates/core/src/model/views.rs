// SPDX-License-Identifier: AGPL-3.0-or-later
//! DTO assembly (`engine-api.md` §4) from a [`ForumModel`] and its full fold
//! ([`crate::social::full::ForumView`]): post summaries and views, threads (pre-order of the
//! displayed nodes, siblings sorted per `social.md` §12.4), Recent Discussion, history, members,
//! the forum header with the viewer's capabilities, the mod queue and the mod log.
//!
//! ```text
//! state(x)   = the fold's displayed state (V1–V10, then the thread pass §10.2), then the lens
//!              (§10.3): a listed item by a muted author is Collapsed(Muted)
//! Removed{…} = rank, actor and spam always; reason if showReason, or to the forum's authorities
//! body      = rendered only for listed items (hidden content is never rendered: tombstones and
//!              placeholders carry an empty body, title, link, tags and labels)
//! score     = the fold's §7 values for listed items, and for every item to authorities; zero
//!              (with `mine`) otherwise
//! mine      = the viewer's own opinion record (at://me/NS.opinion/opinionRkey(F, subject)), with
//!              `pending` (an outbox write of it is not done) and `notYetCounted` (its karma axis
//!              carries no weight: not counting, or the viewer ∉ E)
//! reacts    = the fold's valid reacts (§9.2) of listed items, by palette order then qkey, with
//!              the creator's quote anchored against P of the item's current body, the viewer's
//!              stance, and the viewer's own reacts that form no valid react (`uncounted`)
//! ```
//!
//! Views are pure functions of the model, the record values that carry the bodies to render
//! (re-read from the store by the caller), the authors' actor views (from the identity cache) and
//! the viewer's local state; the same inputs give byte-identical JSON on every platform
//! (`engine-api.md` §1).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::values::{
    Agreement, Capability, MembershipMode, NewUsers, PostKind as LexPostKind, Role, Stance,
    Tier as LexTier, VoteValue, Voting,
};
use lessdugin_markdown::quote::{QuoteResolution, QuoteTarget, resolve_quote};
use serde_json::Value;

use crate::dto::{
    ActorView, AgreementValue, Anchor, Badges, Banner, CollapseReason, CommentNode, CommentSort,
    Completeness, DtoVersion, ExternalLinkView, ForumRole, ForumSummary, ForumView, HandleState,
    HiddenReason, HistorySummary, HistoryView, ItemState, KarmaValue, MemberView, MembersQuery,
    MembershipMode as DtoMode, ModLogEntry, ModLogStatus, ModQueue, ModQueueItem, ModRole,
    ModeratorView, MyOpinion, PostKind, PostSummary, PostView, ReactTally, RenderedBody, ScoreView,
    Stance as DtoStance, TextQuote, ThreadRoot, ThreadView, Tier, VersionEntry, VersionRef,
    ViewerCaps,
};
use crate::identity::short_did;
use crate::model::build::ForumModel;
use crate::model::palette;
use crate::model::render::{RenderCache, empty_body, render_md};
use crate::social::full::gates::Gate;
use crate::social::full::moderation::{ModRecord, ModScope};
use crate::social::full::query::{self, CommentQuery, Query};
use crate::social::full::reacts::{Quote, entries, palette as palette_ids};
use crate::social::full::timeline::Version;
use crate::social::full::view::{self as fv, Dim, DimValue, ModStatus, QueueItem, State};
use crate::social::input::{Skel, SkelData, split_uri};
use crate::social::threads::Parent;
use crate::social::time::ms;
use crate::store::records::Level;

/// The newest newcomer items the mod queue lists (open mode, `social.md` §4.12).
pub const NEWCOMER_QUEUE_MAX: usize = 100;

/// An actor view with only the DID (no cached identity).
pub fn bare_actor(did: &str) -> ActorView {
    ActorView {
        did: did.to_owned(),
        handle: None,
        handle_state: HandleState::Unverified,
        claimed_handle: None,
        display_name: None,
        avatar: None,
        short_did: short_did(did),
    }
}

/// The DTO post kind of an effective lexicon kind.
pub fn post_kind(k: LexPostKind) -> PostKind {
    match k {
        LexPostKind::Article => PostKind::Article,
        LexPostKind::Link => PostKind::Link,
        LexPostKind::Question => PostKind::Question,
        LexPostKind::QuickTake => PostKind::QuickTake,
    }
}

/// The DTO tier of a lexicon tier.
pub fn tier(t: LexTier) -> Tier {
    match t {
        LexTier::Personal => Tier::Personal,
        LexTier::Frontpage => Tier::Frontpage,
        LexTier::Curated => Tier::Curated,
    }
}

/// The fold's post sort of a DTO sort.
pub fn post_sort(s: crate::dto::PostSort) -> query::PostSort {
    use crate::dto::PostSort as S;
    match s {
        S::Magic => query::PostSort::Magic,
        S::Top => query::PostSort::Top,
        S::New => query::PostSort::New,
        S::Old => query::PostSort::Old,
        S::RecentComments => query::PostSort::RecentComments,
    }
}

/// The fold's list filter of a DTO filter.
pub fn post_filter(f: crate::dto::PostFilter) -> query::PostFilter {
    use crate::dto::PostFilter as F;
    match f {
        F::Frontpage => query::PostFilter::Frontpage,
        F::All => query::PostFilter::All,
        F::Curated => query::PostFilter::Curated,
    }
}

/// The fold's comment sort of a DTO sort.
pub fn comment_sort(s: CommentSort) -> query::CommentSort {
    match s {
        CommentSort::Magic => query::CommentSort::Magic,
        CommentSort::Top => query::CommentSort::Top,
        CommentSort::New => query::CommentSort::New,
        CommentSort::Old => query::CommentSort::Old,
        CommentSort::LatestReply => query::CommentSort::LatestReply,
    }
}

/// The host of a URL (for link cards).
fn host_of(uri: &str) -> String {
    url::Url::parse(uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn text_quote(v: &Value) -> Option<TextQuote> {
    Some(TextQuote {
        exact: v.get("exact")?.as_str()?.to_owned(),
        prefix: v.get("prefix").and_then(Value::as_str).map(str::to_owned),
        suffix: v.get("suffix").and_then(Value::as_str).map(str::to_owned),
        start: v
            .get("start")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
    })
}

fn quote_dto(q: &Quote) -> TextQuote {
    TextQuote {
        exact: q.exact.clone(),
        prefix: q.prefix.clone(),
        suffix: q.suffix.clone(),
        start: q.start.and_then(|n| u32::try_from(n).ok()),
    }
}

/// A quote resolved against P (`lfm-1.md` §11): `None` while the text is not available.
fn anchor_of(q: &TextQuote, text: Option<&str>) -> Option<Anchor> {
    let t = text?;
    let tq = lessdugin_markdown::quote::TextQuote {
        exact: &q.exact,
        prefix: q.prefix.as_deref(),
        suffix: q.suffix.as_deref(),
        start: q.start.map(u64::from),
    };
    match resolve_quote(&tq, QuoteTarget::Text(t)) {
        QuoteResolution::Anchored { start, end } => Some(Anchor::Range {
            start: u32::try_from(start).ok()?,
            end: u32::try_from(end).ok()?,
        }),
        QuoteResolution::Orphaned => Some(Anchor::Orphan { orphan: true }),
        QuoteResolution::Pending => None,
    }
}

fn collapse(r: fv::CollapseReason) -> CollapseReason {
    match r {
        fv::CollapseReason::NewUser => CollapseReason::NewUser,
        fv::CollapseReason::LowScore => CollapseReason::LowScore,
    }
}

fn status_dto(s: ModStatus) -> ModLogStatus {
    match s {
        ModStatus::Effective => ModLogStatus::Effective,
        ModStatus::Overridden { .. } => ModLogStatus::Overridden,
        ModStatus::Lapsed => ModLogStatus::Lapsed,
        ModStatus::Expired => ModLogStatus::Expired,
        ModStatus::Stale => ModLogStatus::Stale,
        ModStatus::Unauthorised => ModLogStatus::Unauthorised,
        ModStatus::ReservedUnknown => ModLogStatus::ReservedUnknown,
        ModStatus::Edited => ModLogStatus::Edited,
        ModStatus::Removed => ModLogStatus::Removed,
    }
}

/// The asserted values of a modAction as mod-log text (`hidden`, `visible`, `spam`, …).
fn assert_values(m: &ModRecord) -> String {
    let flag = |b: bool, yes: &str, no: &str| if b { yes } else { no }.to_owned();
    m.asserts
        .iter()
        .filter_map(|(d, v)| {
            Some(match (d, v) {
                (Dim::Hidden, DimValue::Bool(b)) => flag(*b, "hidden", "visible"),
                (Dim::Spam, DimValue::Bool(true)) => "spam".to_owned(),
                (Dim::Spam, _) => return None,
                (Dim::Access, DimValue::Access(l)) => l.as_str().to_owned(),
                (Dim::Restrict, DimValue::Caps(c)) => {
                    let caps: Vec<&str> = c.iter().map(|c| c.as_str()).collect();
                    format!("restrict:{}", caps.join("+"))
                }
                (Dim::Nullify, DimValue::Bool(b)) => flag(*b, "nullify", "unnullify"),
                (Dim::Tier, DimValue::Tier(t)) => t.as_str().to_owned(),
                (Dim::PinForum | Dim::PinThread, DimValue::Bool(b)) => {
                    flag(*b, "pinned", "unpinned")
                }
                (Dim::Locked, DimValue::Bool(b)) => flag(*b, "locked", "unlocked"),
                (Dim::TagRemoved, DimValue::Bool(b)) => flag(*b, "removed", "restored"),
                (Dim::TagHead, DimValue::Head { uri, .. }) => format!("head:{uri}"),
                _ => return None,
            })
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// "edited since <action>" for a stale positive assertion.
fn stale_action(m: &ModRecord) -> &'static str {
    for (d, v) in &m.asserts {
        match (d, v) {
            (Dim::Tier, DimValue::Tier(LexTier::Curated)) => return "curated",
            (Dim::Tier, DimValue::Tier(_)) => return "frontpaged",
            (Dim::PinForum | Dim::PinThread, DimValue::Bool(true)) => return "pinned",
            (Dim::Hidden, DimValue::Bool(false)) => return "kept",
            (Dim::TagHead, _) => return "set as the tag's head",
            _ => {}
        }
    }
    "asserted"
}

/// The viewer of a view: the active account and its local state (`social.md` §10.3, §16).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    /// The active account's DID (`None`: anonymous).
    pub did: Option<String>,
    /// Muted authors (the lens: their listed items are `Collapsed(Muted)`).
    pub muted: BTreeSet<String>,
    /// `lastVisited[post]` (µs, on the `t_obs` scale) of the posts the view shows: a comment is
    /// unread iff `t_obs(c) > lastVisited[root]` (§16). Posts without a mark have no unread
    /// state.
    pub read: BTreeMap<String, i64>,
}

/// Everything a view needs besides the model.
pub struct Views<'a> {
    /// The forum model.
    pub model: &'a ForumModel,
    /// Actor views by DID (missing: [`bare_actor`]).
    pub actors: &'a BTreeMap<String, ActorView>,
    /// PDS origins by DID (media attribution).
    pub pds: &'a BTreeMap<String, String>,
    /// Record values of the items whose bodies are shown.
    pub values: &'a BTreeMap<String, Value>,
    /// The render cache.
    pub renders: &'a RenderCache,
    /// Own uris with an outbox write that is not done.
    pub pending: &'a BTreeSet<String>,
    /// Completeness of the forum.
    pub completeness: &'a Completeness,
    /// The viewer.
    pub viewer: &'a Viewer,
}

impl Views<'_> {
    /// The viewer's DID.
    fn me(&self) -> Option<&str> {
        self.viewer.did.as_deref()
    }

    /// The viewer is one of the forum's authorities.
    pub fn viewer_is_authority(&self) -> bool {
        self.me().is_some_and(|d| self.model.is_authority(d))
    }

    /// The actor view of `did`.
    pub fn actor(&self, did: &str) -> ActorView {
        self.actors
            .get(did)
            .cloned()
            .unwrap_or_else(|| bare_actor(did))
    }

    /// The forum's banners (`newerRules`, `moderationUnavailable`).
    pub fn banners(&self) -> Vec<Banner> {
        self.model
            .view
            .banners
            .iter()
            .map(|b| Banner {
                code: b.code.clone(),
                message: b.message.clone(),
            })
            .collect()
    }

    /// The completeness of the forum, `degraded` also when the fold is (§4.11).
    pub fn completeness(&self) -> Completeness {
        let mut c = self.completeness.clone();
        c.degraded |= self.model.view.degraded;
        c
    }

    fn hidden_reason(&self, uri: &str, r: &fv::HiddenReason) -> HiddenReason {
        match r {
            fv::HiddenReason::NotParticipant => HiddenReason::NotParticipant,
            fv::HiddenReason::ModerationUnavailable => HiddenReason::ModerationUnavailable,
            fv::HiddenReason::Removed(rm) => {
                // the reason text is shown if showReason, and always to the forum's authorities
                let reason = if self.viewer_is_authority() {
                    self.model
                        .view
                        .moderation
                        .get(uri, Dim::Hidden)
                        .and_then(|w| self.model.view.moderation.records.get(&w.winner))
                        .and_then(|m| m.reason.clone())
                        .or_else(|| rm.reason.clone())
                } else {
                    rm.reason.clone()
                };
                HiddenReason::Removed {
                    rank: rm.rank,
                    actor: Some(rm.actor.clone()),
                    spam: rm.spam,
                    reason,
                }
            }
            fv::HiddenReason::Banned => HiddenReason::Banned,
            fv::HiddenReason::Restricted => HiddenReason::Restricted,
            fv::HiddenReason::AuthorModerated => HiddenReason::AuthorModerated,
            fv::HiddenReason::Locked => HiddenReason::Locked,
            fv::HiddenReason::NewUser => HiddenReason::NewUser,
        }
    }

    /// The displayed state of a candidate item after the lens (`None`: Excluded or unknown).
    pub fn item_state(&self, uri: &str) -> Option<ItemState> {
        let st = self.model.displayed(uri)?;
        let author = split_uri(uri).map(|(d, _, _)| d);
        if st.listed() && author.is_some_and(|d| self.viewer.muted.contains(d)) {
            return Some(ItemState::Collapsed {
                reason: CollapseReason::Muted,
            });
        }
        Some(match st {
            State::Excluded => return None,
            State::Visible => ItemState::Visible,
            State::Collapsed(r) => ItemState::Collapsed {
                reason: collapse(*r),
            },
            State::Hidden(r) => ItemState::Hidden {
                reason: self.hidden_reason(uri, r),
            },
            State::Tombstone(r) => ItemState::Tombstone {
                reason: self.hidden_reason(uri, r),
            },
        })
    }

    /// The fold lists `uri` (`Visible` or `Collapsed`, before the lens).
    pub fn listed(&self, uri: &str) -> bool {
        self.model.displayed(uri).is_some_and(State::listed)
    }

    fn badges(&self, x: &Skel) -> Badges {
        let t = self.model.view.times.get(&x.uri);
        Badges {
            verified: self
                .model
                .rows
                .get(&x.uri)
                .is_some_and(|r| r.level == Level::Proven),
            edited: self.model.view.edited.contains(&x.uri),
            future_dated: t.is_some_and(|t| t.future),
            identity_unverified: false,
            stale: self.model.stale(&x.did),
            hint: false,
            inconsistent_timestamps: t.is_some_and(|t| t.inconsistent),
            pending: self.pending.contains(&x.uri),
        }
    }

    /// The uri of the viewer's opinion on `subject` (deterministic key, `data-model.md` §3.2).
    fn my_opinion_uri(&self, subject: &str) -> Option<String> {
        let me = self.me()?;
        let rkey =
            lessdugin_lexicon::keys::opinion_rkey(NS, self.model.key.uri.as_str(), subject).ok()?;
        Some(format!("at://{me}/{}/{rkey}", nsid::OPINION))
    }

    /// The viewer's valid opinion record on `subject`.
    fn my_opinion(&self, subject: &str) -> Option<(String, &Value)> {
        let uri = self.my_opinion_uri(subject)?;
        let x = self.model.skel(&uri).filter(|x| x.valid)?;
        let v = x.value()?;
        Some((uri, v))
    }

    /// The viewer's own opinion on `subject` (`None` without one, and without a pending write).
    pub fn mine(&self, subject: &str) -> Option<MyOpinion> {
        let uri = self.my_opinion_uri(subject)?;
        let pending = self.pending.contains(&uri);
        let value = self.my_opinion(subject).map(|(_, v)| v);
        let text = |k: &str| value.and_then(|v| v.get(k)).and_then(Value::as_str);
        let karma = text("karma").and_then(VoteValue::parse).map(|v| match v {
            VoteValue::Up => KarmaValue::Up,
            VoteValue::Down => KarmaValue::Down,
            VoteValue::StrongUp => KarmaValue::StrongUp,
            VoteValue::StrongDown => KarmaValue::StrongDown,
        });
        let agreement = text("agreement")
            .and_then(Agreement::parse)
            .map(|a| match a {
                Agreement::Agree => AgreementValue::Agree,
                Agreement::Disagree => AgreementValue::Disagree,
                Agreement::StrongAgree => AgreementValue::StrongAgree,
                Agreement::StrongDisagree => AgreementValue::StrongDisagree,
            });
        if karma.is_none() && agreement.is_none() && !pending {
            return None;
        }
        let weighted = self.model.view.karma.as_ref().is_ok_and(|k| {
            k.opinions
                .get(&uri)
                .is_some_and(|o| o.karma.is_some() && k.e.contains(&o.voter))
        });
        Some(MyOpinion {
            karma,
            agreement,
            pending,
            not_yet_counted: karma.is_some() && !weighted,
        })
    }

    /// The score of item `uri` (`listed`: its fold state is listed).
    fn score(&self, uri: &str, listed: bool) -> ScoreView {
        let karma_only = self.model.view.policy.voting == Voting::KarmaOnly;
        let s = self
            .model
            .view
            .scores
            .as_ref()
            .ok()
            .and_then(|m| m.get(uri))
            .filter(|_| listed || self.viewer_is_authority());
        let mut out = match s {
            Some(s) => ScoreView {
                score: s.score,
                agreement: s.agreement,
                unweighted: s.unweighted,
                earlier_version_votes: s.earlier,
                weighted_votes: s.weighted,
                mine: None,
            },
            None => ScoreView {
                agreement: (!karma_only).then_some(0),
                ..ScoreView::default()
            },
        };
        out.mine = self.mine(uri);
        out
    }

    /// The fold's score of `uri` as the raw view shows it (hidden items included).
    pub fn raw_score(&self, uri: &str) -> Option<ScoreView> {
        let s = self.model.view.scores.as_ref().ok()?.get(uri)?;
        Some(ScoreView {
            score: s.score,
            agreement: s.agreement,
            unweighted: s.unweighted,
            earlier_version_votes: s.earlier,
            weighted_votes: s.weighted,
            mine: self.mine(uri),
        })
    }

    /// The reacts of item `uri` with P of its current body (`None`: not available).
    fn reacts(&self, uri: &str, text: Option<&str>) -> Vec<ReactTally> {
        let tallies = self
            .model
            .view
            .reacts
            .as_ref()
            .ok()
            .and_then(|m| m.get(uri));
        let ids = palette_ids(self.model.view.policy.react_palette);
        let mut own = self
            .my_opinion(uri)
            .map(|(_, v)| entries(v, ids))
            .unwrap_or_default();
        let stance = |s: Stance| match s {
            Stance::For => DtoStance::For,
            Stance::Against => DtoStance::Against,
        };
        let info = |react: &str| {
            palette::react(react).map_or((usize::MAX, String::new(), String::new()), |(i, r)| {
                (i, r.label.clone(), r.glyph.clone())
            })
        };
        let mut out: Vec<((usize, String), ReactTally)> = Vec::new();
        for t in tallies.into_iter().flatten() {
            let (i, label, glyph) = info(&t.react);
            let quote = t.quote.as_ref().map(quote_dto);
            let anchor = quote.as_ref().and_then(|q| anchor_of(q, text));
            let mine = own
                .remove(&(t.react.clone(), t.qkey.clone()))
                .map(|(s, _)| stance(s));
            out.push((
                (i, t.qkey.clone()),
                ReactTally {
                    react: t.react.clone(),
                    label,
                    glyph,
                    for_count: u32::try_from(t.supporters.len()).unwrap_or(u32::MAX),
                    against: u32::try_from(t.opposers.len()).unwrap_or(u32::MAX),
                    quote,
                    anchor,
                    creator: Some(t.creator.clone()),
                    mine,
                    uncounted: false,
                },
            ));
        }
        for ((react, q), (s, quote)) in own {
            let (i, label, glyph) = info(&react);
            let quote = quote.as_ref().map(quote_dto);
            let anchor = quote.as_ref().and_then(|q| anchor_of(q, text));
            out.push((
                (i, q),
                ReactTally {
                    react,
                    label,
                    glyph,
                    for_count: 0,
                    against: 0,
                    quote,
                    anchor,
                    creator: None,
                    mine: Some(stance(s)),
                    uncounted: true,
                },
            ));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.into_iter().map(|(_, t)| t).collect()
    }

    fn created_ms(x: &Skel) -> i64 {
        x.times.claim.or_else(|| x.times.eff()).map_or(0, ms)
    }

    fn t_rank_ms(&self, uri: &str) -> i64 {
        self.model.view.times.get(uri).map_or(0, |t| ms(t.t_rank))
    }

    /// The rendered body of item `x` (empty unless the item is listed and its value was read).
    pub fn body(&self, x: &Skel, listed: bool) -> RenderedBody {
        if !listed {
            return empty_body();
        }
        let Some(md) = self.values.get(&x.uri).and_then(|v| v.get("body")) else {
            return empty_body();
        };
        let excerpt = match &x.data {
            SkelData::Post { excerpt, .. } => excerpt.as_deref(),
            _ => None,
        };
        let pds = self.pds.get(&x.did).map_or("", String::as_str);
        self.renders
            .body(&x.cid, md, &self.model.input.ns, excerpt, &x.did, pds)
    }

    /// P of the current body of a listed item whose value was read (`None` for a spilled body
    /// that is not fetched: anchors are pending).
    fn text_of(&self, uri: &str) -> Option<String> {
        let x = self.model.skel(uri)?;
        if !self.listed(uri) {
            return None;
        }
        let body = self.values.get(uri)?.get("body")?;
        if body.get("textBlob").is_some() {
            return None;
        }
        Some(self.body(x, true).text)
    }

    /// The number of listed comments of the thread of `post`, and the newest `t_rank` (ms) of
    /// its `Visible` comments.
    fn thread_stats(&self, post: &str) -> (u32, Option<i64>) {
        let Some(pl) = self.model.view.threads.get(post) else {
            return (0, None);
        };
        let mut listed = 0u32;
        let mut last: Option<i64> = None;
        for c in pl.parent.keys() {
            if self.listed(c) {
                listed = listed.saturating_add(1);
            }
            if matches!(self.model.state(c), Some(State::Visible))
                && let Some(t) = self.model.view.times.get(c)
            {
                last = Some(last.map_or(t.t_rank, |l| l.max(t.t_rank)));
            }
        }
        (listed, last.map(ms))
    }

    /// The unread comments of the thread of `post` (`None` without a read mark).
    fn unread_count(&self, post: &str) -> Option<u32> {
        let mark = *self.viewer.read.get(post)?;
        let pl = self.model.view.threads.get(post)?;
        let n = pl.parent.keys().filter(|c| self.unread(c, mark)).count();
        Some(u32::try_from(n).unwrap_or(u32::MAX))
    }

    fn unread(&self, c: &str, mark: i64) -> bool {
        self.listed(c)
            && self.model.obs(c).is_some_and(|t| t > mark)
            && split_uri(c).map(|(d, _, _)| d) != self.me()
    }

    /// The summary of post `uri` (`None` unless it is a non-Excluded post).
    pub fn post_summary(&self, uri: &str) -> Option<PostSummary> {
        let x = self.model.skel(uri)?;
        let SkelData::Post {
            kind,
            title,
            tags,
            labels,
            link,
            excerpt,
            spilled,
            ..
        } = &x.data
        else {
            return None;
        };
        let state = self.item_state(uri)?;
        let kind = post_kind(LexPostKind::effective(kind.as_deref()).value);
        // a hidden or tombstoned post shows none of its author's content (title, link, tags and
        // labels included), like its body
        let listed = self.listed(uri);
        let excerpt = if !listed {
            String::new()
        } else if *spilled {
            excerpt.clone().unwrap_or_default()
        } else {
            self.body(x, true).excerpt
        };
        let (comment_count, last_comment_at_ms) = self.thread_stats(uri);
        let m = &self.model.view.moderation;
        let author_locked = self
            .model
            .view
            .author
            .as_ref()
            .is_ok_and(|a| a.locks.contains_key(uri));
        Some(PostSummary {
            uri: uri.to_owned(),
            cid: x.cid.clone(),
            forum: self.model.key.uri.to_string(),
            author: self.actor(&x.did),
            post_kind: kind,
            // a quickTake's title is ignored (PO4)
            title: if kind == PostKind::QuickTake || !listed {
                None
            } else {
                title.clone()
            },
            excerpt,
            tags: if listed { tags.clone() } else { Vec::new() },
            tier: tier(m.tier(uri)),
            pinned: m.pinned_forum(uri),
            locked: m.lock(uri).is_some() || author_locked,
            created_at_ms: Self::created_ms(x),
            t_rank_ms: self.t_rank_ms(uri),
            comment_count,
            last_comment_at_ms,
            score: self.score(uri, listed),
            state,
            badges: self.badges(x),
            unread_comments: self.unread_count(uri),
            link: if kind == PostKind::Link && listed {
                link.as_ref().map(|l| ExternalLinkView {
                    uri: l.uri.clone(),
                    host: host_of(&l.uri),
                    title: l.title.clone(),
                    description: l.description.clone(),
                })
            } else {
                None
            },
            labels: if listed { labels.clone() } else { Vec::new() },
        })
    }

    /// Valid `op: update` revisions of `uri`, in TID order.
    fn revisions(&self, uri: &str) -> Vec<&Skel> {
        self.model
            .view
            .revisions
            .get(uri)
            .map(|v| v.iter().filter_map(|r| self.model.skel(r)).collect())
            .unwrap_or_default()
    }

    /// History summary of `uri`. "Redacted" when the item says it was edited but no revision
    /// of it is left.
    pub fn history_summary(&self, uri: &str) -> HistorySummary {
        let revs = self.revisions(uri).len() as u32;
        let edited_later = self.model.skel(uri).is_some_and(Skel::updated_later);
        HistorySummary {
            revisions: revs,
            redacted: revs == 0 && edited_later,
        }
    }

    /// The view of post `uri` with its rendered body and reacts (M5: sequences).
    pub fn post_view(&self, uri: &str) -> Option<PostView> {
        let post = self.post_summary(uri)?;
        let x = self.model.skel(uri)?;
        let listed = self.listed(uri);
        let body = self.body(x, listed);
        let reacts = if listed {
            let text = self.text_of(uri);
            self.reacts(uri, text.as_deref())
        } else {
            Vec::new()
        };
        Some(PostView {
            dto: DtoVersion::default(),
            post,
            body,
            reacts,
            history: self.history_summary(uri),
            sequences: Vec::new(),
        })
    }

    /// The node of comment `uri` in the thread of `root` (`None` unless it is placed there and
    /// not Excluded).
    pub fn comment_node(&self, uri: &str, root: &str) -> Option<CommentNode> {
        let x = self.model.skel(uri)?;
        let pl = self.model.view.threads.get(root)?;
        let SkelData::Comment {
            parent,
            retracted,
            moderator_hat,
            quote,
            ..
        } = &x.data
        else {
            return None;
        };
        let parent_uri = match pl.parent.get(uri)? {
            Parent::Root => None,
            Parent::Comment(p) | Parent::Missing(p) => Some(p.clone()),
        };
        let state = self.item_state(uri)?;
        let listed = self.listed(uri);
        let body = self.body(x, listed);
        let version = self
            .model
            .view
            .versions
            .get(uri)
            .and_then(|v| if parent.is_some() { v.parent } else { v.root })
            .filter(|_| listed)
            .map(|v| match v {
                Version::Revision(n) => VersionRef::Revision { revision: n },
                Version::Unavailable => VersionRef::Unavailable { unavailable: true },
            });
        let quote = quote.as_ref().and_then(text_quote).filter(|_| listed);
        // the quote's target: the parent if present, else the root (§16 "Quote", lfm-1.md §11)
        let quote_anchor = quote.as_ref().and_then(|q| {
            let target = parent.as_ref().map_or(root, |p| p.uri.as_str());
            anchor_of(q, self.text_of(target).as_deref())
        });
        let reacts = if listed {
            self.reacts(uri, Some(&body.text))
        } else {
            Vec::new()
        };
        let m = &self.model.view.moderation;
        let pinned = m
            .pin_thread(uri)
            .or_else(|| {
                self.model
                    .view
                    .author
                    .as_ref()
                    .ok()
                    .and_then(|a| a.thread_pins.get(uri).copied())
            })
            .unwrap_or(false);
        Some(CommentNode {
            uri: uri.to_owned(),
            cid: x.cid.clone(),
            root: root.to_owned(),
            parent: parent_uri,
            depth: pl.depth.get(uri).copied().unwrap_or(1),
            author: self.actor(&x.did),
            score: self.score(uri, listed),
            body,
            quote,
            quote_anchor,
            created_at_ms: Self::created_ms(x),
            t_rank_ms: self.t_rank_ms(uri),
            reacts,
            state,
            badges: self.badges(x),
            retracted: *retracted && listed,
            moderator_hat: *moderator_hat && listed && self.model.is_authority(&x.did),
            version,
            orphaned: pl.orphaned.contains(uri),
            missing: false,
            unread: self
                .viewer
                .read
                .get(root)
                .is_some_and(|mark| self.unread(uri, *mark)),
            pinned,
        })
    }

    fn placeholder_node(&self, uri: &str, root: &str) -> CommentNode {
        let did = split_uri(uri).map_or("", |(d, _, _)| d);
        CommentNode {
            uri: uri.to_owned(),
            cid: String::new(),
            root: root.to_owned(),
            parent: None,
            depth: 1,
            author: self.actor(did),
            body: empty_body(),
            quote: None,
            quote_anchor: None,
            created_at_ms: 0,
            t_rank_ms: 0,
            score: ScoreView::default(),
            reacts: Vec::new(),
            state: ItemState::Visible,
            badges: Badges::default(),
            retracted: false,
            moderator_hat: false,
            version: None,
            orphaned: false,
            missing: true,
            unread: false,
            pinned: false,
        }
    }

    /// The displayed nodes of the thread of `root` in pre-order (`social.md` §12.4: listed
    /// comments, tombstones, and `Missing` placeholders with a listed descendant), siblings
    /// sorted by `sort` (thread pins first; placeholders after the comments, by uri).
    pub fn thread_comments(&self, root: &str, sort: CommentSort) -> Vec<CommentNode> {
        let Some(pl) = self.model.view.threads.get(root) else {
            return Vec::new();
        };
        let q = Query {
            thread: None,
            posts: None,
            comments: Some(CommentQuery {
                thread: root.to_owned(),
                sort: comment_sort(sort),
            }),
        };
        let order = match self.model.orderings(&q) {
            Ok(o) => o.comment_order.unwrap_or_default(),
            Err(p) => {
                tracing::warn!(forum = %self.model.key.uri, pending = %p, "comment order pending");
                return Vec::new();
            }
        };
        order
            .iter()
            .filter_map(|u| {
                if pl.parent.contains_key(u) {
                    self.comment_node(u, root)
                } else {
                    Some(self.placeholder_node(u, root))
                }
            })
            .collect()
    }

    /// The thread of post `root`.
    pub fn thread_view(&self, root: &str, sort: CommentSort) -> ThreadView {
        let post = match self.post_view(root) {
            Some(p) => ThreadRoot::Post(Box::new(p)),
            None => ThreadRoot::Missing {
                missing: root.to_owned(),
            },
        };
        ThreadView {
            dto: DtoVersion::default(),
            post,
            comments: self.thread_comments(root, sort),
            sort,
            completeness: self.completeness(),
            banners: self.banners(),
        }
    }

    /// The version history of `uri` (§11 step 6, `data-model.md` §6.5).
    pub fn history(&self, uri: &str) -> Option<HistoryView> {
        let x = self.model.skel(uri)?;
        let revs = self.revisions(uri);
        let rev_meta = |r: &Skel| match &r.data {
            SkelData::Revision {
                subject,
                summary,
                has_record,
                ..
            } => (
                subject.as_ref().map(|s| s.cid.clone()).unwrap_or_default(),
                summary.clone(),
                *has_record,
            ),
            _ => (String::new(), None, false),
        };
        let mut versions = Vec::with_capacity(revs.len() + 1);
        let mut at = Self::created_ms(x);
        let mut summary: Option<String> = None;
        for (n, r) in revs.iter().enumerate() {
            let (cid, s, available) = rev_meta(r);
            versions.push(VersionEntry {
                n: n as u32 + 1,
                cid,
                created_at_ms: at,
                summary: summary.take(),
                available,
            });
            at = Self::created_ms(r);
            summary = s;
        }
        // an edit without a revision (`--no-history`, a redaction) dates the current version
        // by its later `updatedAt`
        let updated = x
            .updated_at
            .as_deref()
            .and_then(crate::social::time::us)
            .map(ms);
        let at = updated.filter(|u| *u > at).unwrap_or(at);
        versions.push(VersionEntry {
            n: revs.len() as u32 + 1,
            cid: x.cid.clone(),
            created_at_ms: at,
            summary,
            available: true,
        });
        Some(HistoryView {
            dto: DtoVersion::default(),
            uri: uri.to_owned(),
            versions,
        })
    }

    /// The forum role of `did`: owner, admin, moderator (A, by rank); otherwise, with a valid
    /// membership record: member (S: granted, not banned or nullified), established (E ∖ S),
    /// newcomer (P ∖ (S ∪ E)); none.
    pub fn role_of(&self, did: &str) -> ForumRole {
        let p = &self.model.view.participation;
        match self.model.rank(did) {
            3.. => ForumRole::Owner,
            2 => ForumRole::Admin,
            1 => ForumRole::Moderator,
            // a grant without a membership record is not membership yet (the client offers
            // "join")
            0 if !self.model.is_member(did) => ForumRole::None,
            0 if p.s.contains(did) => ForumRole::Member,
            0 if self.model.in_e(did) => ForumRole::Established,
            0 if p.p.contains(did) => ForumRole::Newcomer,
            0 => ForumRole::None,
        }
    }

    /// `members`: every DID with a valid membership record of the forum (`member(d)`), by DID,
    /// filtered by `q` (`pending`: applicants only; `status`: one role).
    pub fn members(&self, q: &MembersQuery) -> Vec<MemberView> {
        let v = &self.model.view;
        let p = &v.participation;
        let mut out = Vec::new();
        for x in self.model.input.records.values() {
            if x.coll != Some(Collection::Membership) || !x.valid || !p.members.contains(&x.did) {
                continue;
            }
            let applicant = p.applicants.contains(&x.did);
            if q.pending == Some(true) && !applicant {
                continue;
            }
            let status = self.role_of(&x.did);
            if q.status.is_some_and(|s| s != status) {
                continue;
            }
            let k = self.model.k(&x.did);
            out.push(MemberView {
                actor: self.actor(&x.did),
                status,
                granted: p.g.contains(&x.did),
                membership_note: x
                    .value()
                    .and_then(|v| v.get("note"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                joined_at_ms: Self::created_ms(x),
                karma: (k != 0).then_some(k),
                applicant,
                banned: p.b.contains(&x.did),
                nullified: p.n.contains(&x.did),
                restricted: v
                    .moderation
                    .restrict(&x.did)
                    .map(|(caps, _)| caps.iter().map(|c| c.as_str().to_owned()).collect())
                    .unwrap_or_default(),
            });
        }
        out.sort_by(|a, b| a.actor.did.cmp(&b.actor.did));
        out
    }

    /// The forum's summary.
    pub fn forum_summary(
        &self,
        me: Option<&str>,
        followed: bool,
        alias: Option<String>,
    ) -> ForumSummary {
        let d = &self.model.decl;
        ForumSummary {
            uri: self.model.key.uri.to_string(),
            owner: self.actor(self.model.key.owner.as_str()),
            name: d.name.clone(),
            description: d.description.clone(),
            membership: match self.model.view.policy.membership {
                MembershipMode::Open => DtoMode::Open,
                MembershipMode::Approval => DtoMode::Approval,
            },
            followed,
            member: me.is_some_and(|m| self.model.is_member(m)),
            alias,
            report_contact: d.report_contact.clone(),
        }
    }

    /// What `me` may do in the forum (`ViewerCaps`).
    pub fn viewer_caps(&self, me: &str) -> ViewerCaps {
        let m = self.model;
        let v = &m.view;
        let p = &v.participation;
        let pol = &v.policy;
        let rank = m.rank(me);
        let authority = rank > 0;
        let excluded = p.b.contains(me) || p.n.contains(me);
        let restricted = |c: Capability| v.moderation.restricted(me, c);
        let vote = !excluded && !restricted(Capability::Vote);
        let react = !excluded && pol.voting == Voting::Reacts && !restricted(Capability::React);
        let e = m.in_e(me);
        let open = pol.membership == MembershipMode::Open;
        // in open mode the first content carries the membership (data-model.md §8)
        let participant = p.p.contains(me) || (open && !p.members.contains(me));
        let newcomer = open && !authority && !p.s.contains(me) && !e;
        let listed = participant
            && !p.b.contains(me)
            && (!v.degraded || p.s.contains(me))
            && !(newcomer && pol.new_users == NewUsers::Hide);
        let author_mod = pol.author_moderation && e;
        ViewerCaps {
            vote,
            strong_vote: vote,
            agree: vote && pol.voting != Voting::KarmaOnly,
            weighted: e,
            react,
            react_create: react && m.passes(me, Gate::ReactCreate),
            react_second: react && m.passes(me, Gate::ReactSecond),
            react_anti: react && m.passes(me, Gate::ReactAnti),
            post: listed && !restricted(Capability::Post),
            comment: listed && !restricted(Capability::Comment),
            newcomer,
            applicant: p.applicants.contains(me),
            moderate: authority,
            rank: authority.then_some(rank),
            curate: rank >= crate::discovery::roster::RANK_ADMIN,
            author_mod_personal: author_mod && m.passes(me, Gate::AuthorModPersonal),
            author_mod_frontpage: author_mod && m.passes(me, Gate::AuthorModFrontpage),
        }
    }

    /// The forum header (`forum_open`).
    pub fn forum_view(
        &self,
        me: Option<&str>,
        followed: bool,
        alias: Option<String>,
        warnings: &[String],
    ) -> ForumView {
        let d = &self.model.decl;
        let about = d.about.as_ref().and_then(|md| {
            let v = serde_json::to_value(md).ok()?;
            Some(render_md(&v, &self.model.input.ns, None))
        });
        let moderators = lessdugin_lexicon::validity::entry::moderators(d, &self.model.key.owner)
            .into_iter()
            .map(|(m, role)| ModeratorView {
                actor: self.actor(m.did.as_str()),
                role: match role {
                    Role::Admin => ModRole::Admin,
                    Role::Moderator => ModRole::Moderator,
                },
                title: m.title.clone(),
                shown: self.model.is_member(m.did.as_str()),
            })
            .collect();
        let mut banners = self.banners();
        banners.extend(warnings.iter().map(|w| Banner {
            code: "warning".into(),
            message: w.clone(),
        }));
        ForumView {
            dto: DtoVersion::default(),
            forum: self.forum_summary(me, followed, alias),
            about,
            moderators,
            policy: self.model.view.policy.dto(),
            my_role: me.map_or(ForumRole::None, |m| self.role_of(m)),
            my_karma: me.map(|m| self.model.k(m)),
            banners,
            completeness: self.completeness(),
            viewer: me.map(|m| self.viewer_caps(m)),
        }
    }

    /// The mod queue (`social.md` §4.12): applicants, top-rank tag-head conflicts and stale
    /// positive assertions (the fold's, by type then key), then, in open mode, the newest
    /// [`NEWCOMER_QUEUE_MAX`] collapsed or hidden newcomer items (`t_rank` desc, uri).
    pub fn mod_queue(&self) -> ModQueue {
        let v = &self.model.view;
        let mut items = Vec::new();
        for q in &v.mod_queue {
            items.push(match q {
                QueueItem::Applicant(d) => {
                    let membership =
                        self.model.input.records.values().find(|x| {
                            x.coll == Some(Collection::Membership) && x.valid && &x.did == d
                        });
                    ModQueueItem {
                        item_kind: "applicant".into(),
                        subject: d.clone(),
                        reason: "applied to join".into(),
                        detail: membership
                            .and_then(|x| x.value()?.get("note")?.as_str().map(str::to_owned)),
                        uri: None,
                        actor: Some(self.actor(d)),
                        at_ms: membership.map(Self::created_ms),
                    }
                }
                QueueItem::Conflict(s) => ModQueueItem {
                    item_kind: "conflict".into(),
                    subject: s.clone(),
                    reason: format!(
                        "conflicting tag heads at rank {}",
                        v.moderation.conflicts.get(s).copied().unwrap_or(0)
                    ),
                    detail: None,
                    uri: None,
                    actor: None,
                    at_ms: None,
                },
                QueueItem::Stale(u) => {
                    let rec = v.moderation.records.get(u);
                    ModQueueItem {
                        item_kind: "stale".into(),
                        subject: rec.map(|r| r.subject.clone()).unwrap_or_default(),
                        reason: format!("edited since {}", rec.map_or("asserted", stale_action)),
                        detail: rec.and_then(|r| r.action).map(|a| a.as_str().to_owned()),
                        uri: Some(u.clone()),
                        actor: rec.map(|r| self.actor(&r.actor)),
                        at_ms: rec.and_then(|r| r.time).map(ms),
                    }
                }
            });
        }
        if v.policy.membership == MembershipMode::Open
            && let Ok(vis) = &v.visibility
        {
            let mut newcomer: Vec<(i64, &str)> = vis
                .states
                .iter()
                .filter(|(_, s)| {
                    matches!(
                        s,
                        State::Collapsed(fv::CollapseReason::NewUser)
                            | State::Hidden(fv::HiddenReason::NewUser)
                    )
                })
                .map(|(u, _)| (v.times.get(u).map_or(0, |t| t.t_rank), u.as_str()))
                .collect();
            newcomer.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            for (t, u) in newcomer.into_iter().take(NEWCOMER_QUEUE_MAX) {
                let hidden = matches!(vis.states.get(u), Some(State::Hidden(_)));
                let did = split_uri(u).map_or("", |(d, _, _)| d);
                let what = match self.model.skel(u).and_then(|x| x.coll) {
                    Some(Collection::Post) => "post",
                    Some(Collection::Comment) => "comment",
                    _ => "item",
                };
                items.push(ModQueueItem {
                    item_kind: "newcomer".into(),
                    subject: u.to_owned(),
                    reason: format!("a new user's {what}"),
                    detail: Some(if hidden { "hidden" } else { "collapsed" }.into()),
                    uri: None,
                    actor: Some(self.actor(did)),
                    at_ms: Some(ms(t)),
                });
            }
        }
        ModQueue {
            dto: DtoVersion::default(),
            items,
        }
    }

    /// The mod log (`social.md` §4.12), in its order (time desc, uri asc; no time last).
    pub fn mod_log_entries(&self) -> Vec<ModLogEntry> {
        let v = &self.model.view;
        let Ok(log) = &v.mod_log else {
            return Vec::new();
        };
        let authority = self.viewer_is_authority();
        log.iter()
            .filter_map(|e| {
                let rank = match e.status {
                    ModStatus::Overridden { rank } => Some(rank),
                    _ => None,
                };
                let status = status_dto(e.status);
                let at_ms = e.time.map_or(0, ms);
                if let Some(m) = v.moderation.records.get(&e.uri) {
                    let hide = m.action == Some(lessdugin_lexicon::values::ActionKind::Hide);
                    let reason = m
                        .reason
                        .clone()
                        .filter(|_| !hide || m.show_reason || authority);
                    let adopted_from = self
                        .model
                        .skel(&e.uri)
                        .and_then(|x| x.value())
                        .and_then(|v| v.get("adopts")?.get("uri")?.as_str())
                        .map(str::to_owned);
                    return Some(ModLogEntry {
                        uri: e.uri.clone(),
                        actor: self.actor(&m.actor),
                        action: m.action.map_or("unknown", |a| a.as_str()).to_owned(),
                        subject: m.subject.clone(),
                        value: assert_values(m),
                        reason,
                        at_ms,
                        status,
                        lapsed: e.status == ModStatus::Lapsed,
                        rank,
                        author_scope: m.scope == ModScope::Author,
                        adopted_from,
                    });
                }
                let x = self.model.skel(&e.uri)?;
                let SkelData::Revision {
                    subject: Some(s), ..
                } = &x.data
                else {
                    return None;
                };
                Some(ModLogEntry {
                    uri: e.uri.clone(),
                    actor: self.actor(&x.did),
                    action: if e.status == ModStatus::Removed {
                        "remove"
                    } else {
                        "edit"
                    }
                    .to_owned(),
                    subject: s.uri.clone(),
                    value: String::new(),
                    reason: None,
                    at_ms,
                    status,
                    lapsed: false,
                    rank,
                    author_scope: false,
                    adopted_from: None,
                })
            })
            .collect()
    }

    /// The DIDs the mod log names (actors), for actor views.
    pub fn mod_log_actors(&self) -> BTreeSet<String> {
        let v = &self.model.view;
        let mut out: BTreeSet<String> = v
            .moderation
            .records
            .values()
            .map(|m| m.actor.clone())
            .collect();
        if let Ok(log) = &v.mod_log {
            out.extend(
                log.iter()
                    .filter_map(|e| split_uri(&e.uri).map(|(d, _, _)| d.to_owned())),
            );
        }
        out
    }

    /// The mod-log status of a listed modAction (`None`: not listed, or no status).
    pub fn mod_status(&self, uri: &str) -> Option<ModLogStatus> {
        let log = self.model.view.mod_log.as_ref().ok()?;
        log.iter()
            .find(|e| e.uri == uri)
            .map(|e| status_dto(e.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors() {
        let q = TextQuote {
            exact: "brown fox".into(),
            prefix: None,
            suffix: None,
            start: None,
        };
        assert_eq!(
            anchor_of(&q, Some("the quick brown fox")),
            Some(Anchor::Range { start: 10, end: 19 })
        );
        assert_eq!(
            anchor_of(&q, Some("nothing here")),
            Some(Anchor::Orphan { orphan: true })
        );
        assert_eq!(anchor_of(&q, None), None);
    }
}
