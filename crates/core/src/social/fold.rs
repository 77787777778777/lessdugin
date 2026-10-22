// SPDX-License-Identifier: AGPL-3.0-or-later
//! The fold (`social.md` §1): `ForumView = fold(FoldInput, Query)`, over the skeleton form of the
//! input ([`ForumInput`]).
//!
//! **M2 scope.** This is the walking-skeleton fold. It computes, exactly per the spec:
//! - L0 authorities and ranks (§4.1), the effective policy (§1.6) and fail-closed `degraded`
//!   (§4.11);
//! - L2 participation P in both modes (§3.1), with the forum-tier `#access` resolution and
//!   seeds S of [`crate::discovery::roster`] (`#access` and `nullifyVotes` only);
//! - time `t_rank` (§2), thread placement (§11) and the thread pass (§10.2);
//! - visibility rules V1 (`Excluded`) and V2 (`Hidden(NotParticipant)`, and
//!   `Hidden(ModerationUnavailable)` in a degraded view); every other rule defaults to
//!   `Visible`.
//!
//! Everything else is marked `TODO(M3)` and takes its neutral value: forum-tier moderation
//! (hide, tier, pin, lock, restrict, tags; §4.2–§4.10), author moderation (§4.9), opinions,
//! karma, scores and gates (§5–§8), reacts (§9), V3–V9, wiki heads and sequences (§13, §14).
//! The fold is pure: no I/O, no clock, ordered collections only (§1.3).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::records::Forum;
use lessdugin_lexicon::values::{DefaultTier, MembershipMode, Voting};

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::roster::{Roster, RosterRecord, authorities};
use crate::dto::{HiddenReason, ItemState, Tier};
use crate::social::input::{ForumInput, Skel, SkelData};
use crate::social::policy::EffectivePolicy;
use crate::social::threads::{self, Node, Placement, ThreadComment};
use crate::social::time;
use crate::store::repo::RepoStatus;

/// The visibility state of a candidate item before the thread pass (§10.1).
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// V1: invalid, another forum, or a failed cross-record rule (CO5).
    Excluded,
    /// Listed and expanded.
    Visible,
    /// Listed, collapsed.
    Collapsed(crate::dto::CollapseReason),
    /// Not listed.
    Hidden(HiddenReason),
}

impl State {
    /// `Visible` or `Collapsed(*)`.
    pub fn listed(&self) -> bool {
        matches!(self, Self::Visible | Self::Collapsed(_))
    }

    /// The DTO state (`None` for `Excluded`).
    pub fn item_state(&self) -> Option<ItemState> {
        Some(match self {
            Self::Excluded => return None,
            Self::Visible => ItemState::Visible,
            Self::Collapsed(r) => ItemState::Collapsed { reason: *r },
            Self::Hidden(r) => ItemState::Hidden { reason: r.clone() },
        })
    }
}

/// Score values of an item (§7). TODO(M3): computed from opinions; zero until then.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Score {
    /// `score(i)`.
    pub score: i64,
    /// `agreement(i)`; `None` under `voting = karmaOnly`.
    pub agreement: Option<i64>,
    /// `unweighted(i)`.
    pub unweighted: u32,
    /// `earlier(i)`.
    pub earlier: u32,
    /// The weighted karma-axis votes (M of "N of M").
    pub weighted: u32,
}

/// The fold's per-item results.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemFold {
    /// V1–V10 (before the thread pass).
    pub state: State,
    /// The displayed state after the thread pass (§10.2): `None` when not displayed (Excluded,
    /// or hidden without a listed descendant).
    pub displayed: Option<ItemState>,
    /// `t_rank` (µs); `None` for Excluded items.
    pub t_rank: Option<i64>,
    /// `[future-dated]`.
    pub future: bool,
    /// "inconsistent timestamps".
    pub inconsistent: bool,
    /// The resolved tier (posts). TODO(M3): `#tier`; the declaration's `defaultTier` until then.
    pub tier: Tier,
    /// Forum-pinned (posts) or thread-pinned (comments). TODO(M3).
    pub pinned: bool,
    /// Locked (posts). TODO(M3).
    pub locked: bool,
    /// Score. TODO(M3).
    pub score: Score,
    /// `[edited]`: a valid `op: update` revision of it is in `records`, or `updatedAt` is later
    /// than `createdAt`.
    pub edited: bool,
}

/// One placed thread with the thread pass applied.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadFold {
    /// The placement (§11).
    pub placement: Placement,
    /// The `Missing` placeholders that are displayed (they have a listed descendant).
    pub shown_missing: BTreeSet<String>,
    /// The number of listed comments.
    pub listed: u32,
}

/// The fold's result for one forum.
#[derive(Clone, Debug, PartialEq)]
pub struct FoldResult {
    /// The declaration.
    pub decl: Forum,
    /// The effective policy.
    pub policy: EffectivePolicy,
    /// A, with ranks.
    pub authorities: BTreeMap<String, u8>,
    /// Members, G, B, N, S.
    pub roster: Roster,
    /// P.
    pub participants: BTreeSet<String>,
    /// §4.11.
    pub degraded: bool,
    /// `t_decl` (µs).
    pub t_decl: i64,
    /// Every candidate item (post, comment, wikiRevision, sequence), Excluded ones included.
    pub items: BTreeMap<String, ItemFold>,
    /// Threads by root post uri: every non-Excluded post, and every root named by a
    /// non-Excluded comment.
    pub threads: BTreeMap<String, ThreadFold>,
    /// Valid `op: update` revisions per subject uri, in TID order (§11 step 6).
    pub revisions: BTreeMap<String, Vec<String>>,
    /// Banners: `(code, message)`.
    pub banners: Vec<(String, String)>,
}

impl FoldResult {
    /// The item results of `uri`.
    pub fn item(&self, uri: &str) -> Option<&ItemFold> {
        self.items.get(uri)
    }

    /// `true` if `did` is an authority.
    pub fn is_authority(&self, did: &str) -> bool {
        self.authorities.contains_key(did)
    }
}

/// Why a fold could not run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("fold input: {0}")]
pub struct FoldError(pub String);

fn err(e: impl std::fmt::Display) -> FoldError {
    FoldError(e.to_string())
}

/// V1 (§10.1): invalid, `forum ≠ F`, or CO5 (the root post in `records` names another forum).
fn excluded(x: &Skel, input: &ForumInput) -> bool {
    if !x.valid || x.forum.as_deref() != Some(input.forum.as_str()) {
        return true;
    }
    if let SkelData::Comment {
        root: Some(root), ..
    } = &x.data
        && let Some(r) = input.records.get(&root.uri)
        && r.coll == Some(Collection::Post)
        && r.forum != x.forum
    {
        return true;
    }
    false
}

/// The fold (§1). Fails only if the declaration value cannot be read.
pub fn fold(input: &ForumInput) -> Result<FoldResult, FoldError> {
    let decl: Forum = serde_json::from_value(input.decl.clone()).map_err(err)?;
    let policy = EffectivePolicy::of(&decl);
    let owner = Did::parse(&input.owner).map_err(err)?;
    let a = authorities(&owner, &decl);

    // L2: participation (M2: `#access` and `nullifyVotes` from the discovery roster)
    let key = ForumKey {
        uri: AtUri::parse(&input.forum).map_err(err)?,
        owner: owner.clone(),
        slug: input
            .forum
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned(),
        fk: input.fk.clone(),
    };
    let roster_recs: Vec<RosterRecord> = input
        .records
        .values()
        .filter(|x| x.valid)
        .filter_map(|x| {
            Some(RosterRecord {
                did: x.did.clone(),
                collection: x.collection.clone(),
                rkey: x.rkey.clone(),
                value: x.value()?.clone(),
            })
        })
        .collect();
    let roster = Roster::compute(&key, &decl, &roster_recs, input.now);
    let participants: BTreeSet<String> = match policy.membership {
        MembershipMode::Approval => roster
            .members
            .iter()
            .filter(|d| roster.granted.contains(*d) || roster.banned.contains(*d))
            .chain(a.keys())
            .cloned()
            .collect(),
        MembershipMode::Open => roster.members.iter().chain(a.keys()).cloned().collect(),
    };
    let degraded = a
        .keys()
        .any(|d| input.status(d) == RepoStatus::Unavailable);

    let t_decl = time::t_decl(
        time::us(input.decl.get("createdAt").and_then(|v| v.as_str()).unwrap_or("")),
        input.decl_obs,
    );

    // revisions (§11 step 6)
    let mut revisions: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for x in input.records.values() {
        if let SkelData::Revision {
            subject: Some(s),
            op: Some(op),
            ..
        } = &x.data
            && x.valid
            && op == "update"
        {
            let tid = time::rkey_tid(&x.rkey).unwrap_or_default().to_owned();
            revisions
                .entry(s.uri.clone())
                .or_default()
                .push((tid, x.uri.clone()));
        }
    }
    let revisions: BTreeMap<String, Vec<String>> = revisions
        .into_iter()
        .map(|(s, mut v)| {
            v.sort();
            (s, v.into_iter().map(|(_, u)| u).collect())
        })
        .collect();

    // V1 and the state of every candidate item
    let tier = match policy.default_tier {
        DefaultTier::Personal => Tier::Personal,
        DefaultTier::Frontpage => Tier::Frontpage,
    };
    let agreement = (policy.voting != Voting::KarmaOnly).then_some(0);
    let mut items: BTreeMap<String, ItemFold> = BTreeMap::new();
    for x in input.records.values().filter(|x| x.is_item()) {
        let state = if excluded(x, input) {
            State::Excluded
        } else if !participants.contains(&x.did) {
            State::Hidden(HiddenReason::NotParticipant)
        } else if degraded && !roster.seeds.contains(&x.did) {
            State::Hidden(HiddenReason::ModerationUnavailable)
        } else {
            // TODO(M3): V3 removed, V4 banned, V5 restricted, V6 author-moderated, V7 locked,
            // V8 new user, V9 low score.
            State::Visible
        };
        items.insert(
            x.uri.clone(),
            ItemFold {
                displayed: state.item_state(),
                t_rank: None,
                future: x.times.future(),
                inconsistent: x.times.inconsistent(),
                tier,
                pinned: false,
                locked: false,
                score: Score {
                    agreement,
                    ..Score::default()
                },
                edited: revisions.contains_key(&x.uri) || x.updated_later(),
                state,
            },
        );
    }
    let live = |u: &str| items.get(u).is_some_and(|i| i.state != State::Excluded);

    // t_rank of posts, wikiRevisions and sequences: max(t_decl, t_eff')
    let mut t_rank: BTreeMap<String, i64> = BTreeMap::new();
    for x in input.records.values() {
        if live(&x.uri) && x.coll != Some(Collection::Comment) {
            t_rank.insert(x.uri.clone(), x.times.rank(t_decl));
        }
    }

    // threads (§11): every live post, and every root named by a live comment
    let mut by_root: BTreeMap<String, Vec<ThreadComment>> = BTreeMap::new();
    for x in input.records.values() {
        if x.coll == Some(Collection::Post) && live(&x.uri) {
            by_root.entry(x.uri.clone()).or_default();
        }
        if let SkelData::Comment {
            root: Some(root),
            parent,
            ..
        } = &x.data
            && live(&x.uri)
        {
            by_root
                .entry(root.uri.clone())
                .or_default()
                .push(ThreadComment {
                    uri: x.uri.clone(),
                    parent: parent.as_ref().map(|p| p.uri.clone()),
                });
        }
    }
    let mut threads: BTreeMap<String, ThreadFold> = BTreeMap::new();
    for (root, comments) in by_root {
        let present = input
            .records
            .get(&root)
            .is_some_and(|r| r.coll == Some(Collection::Post))
            && live(&root);
        let placement = threads::place(&root, present, &comments);
        // t_rank top-down (§2.2, §11 step 5)
        let root_rank = if present {
            t_rank.get(&root).copied().unwrap_or(t_decl)
        } else {
            t_decl
        };
        for node in placement.top_down() {
            let Node::Comment(c) = &node else {
                continue;
            };
            let lower = match placement.tree_parent(c) {
                Some(p) => t_rank.get(p).copied().unwrap_or(root_rank),
                None => root_rank,
            };
            if let Some(x) = input.records.get(c) {
                t_rank.insert(c.clone(), x.times.rank(lower));
            }
        }
        threads.insert(
            root,
            ThreadFold {
                placement,
                shown_missing: BTreeSet::new(),
                listed: 0,
            },
        );
    }
    for (u, t) in &t_rank {
        if let Some(i) = items.get_mut(u) {
            i.t_rank = Some(*t);
        }
    }

    // the thread pass (§10.2): hidden nodes with a listed descendant become tombstones
    for (root, th) in threads.iter_mut() {
        let order = th.placement.top_down();
        let mut has_listed: BTreeMap<Node, bool> = BTreeMap::new();
        for node in order.iter().rev() {
            let below = th
                .placement
                .children_of(node)
                .iter()
                .any(|k| has_listed.get(k).copied().unwrap_or(false));
            let me = match node {
                Node::Comment(c) => items.get(c).is_some_and(|i| i.state.listed()),
                _ => false,
            };
            match node {
                Node::Comment(c) => {
                    if let Some(i) = items.get_mut(c) {
                        if me {
                            th.listed += 1;
                        } else if below && let State::Hidden(r) = &i.state {
                            i.displayed = Some(ItemState::Tombstone { reason: r.clone() });
                        } else {
                            i.displayed = None;
                        }
                    }
                }
                Node::Missing(m) if below => {
                    th.shown_missing.insert(m.clone());
                }
                _ => {}
            }
            has_listed.insert(node.clone(), me || below);
        }
        let any_listed = th
            .placement
            .children_of(&Node::Root)
            .iter()
            .any(|k| has_listed.get(k).copied().unwrap_or(false));
        if let Some(i) = items.get_mut(root)
            && !i.state.listed()
        {
            i.displayed = match (&i.state, any_listed) {
                (State::Hidden(r), true) => Some(ItemState::Tombstone { reason: r.clone() }),
                _ => None,
            };
        }
    }
    // hidden items outside any thread (wikiRevisions, sequences) are not displayed
    for i in items.values_mut() {
        if matches!(i.state, State::Hidden(_))
            && !matches!(i.displayed, Some(ItemState::Tombstone { .. }))
        {
            i.displayed = None;
        }
    }

    let mut banners = Vec::new();
    if policy.newer_rules {
        banners.push((
            "newerRules".to_owned(),
            "this forum uses rules newer than your client".to_owned(),
        ));
    }
    if degraded {
        banners.push((
            "moderationUnavailable".to_owned(),
            "moderation data unavailable".to_owned(),
        ));
    }
    Ok(FoldResult {
        decl,
        policy,
        authorities: a,
        roster,
        participants,
        degraded,
        t_decl,
        items,
        threads,
        revisions,
        banners,
    })
}
