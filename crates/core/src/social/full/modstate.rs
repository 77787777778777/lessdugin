// SPDX-License-Identifier: AGPL-3.0-or-later
//! The moderation rules of visibility, V1–V7 (`mod/1`, `social.md` §10.1), first match wins.
//!
//! ```text
//! V1 invalid, x.forum ≠ F, or CO5                                         → Excluded
//! V2 author(x) ∉ P                                                        → Hidden(NotParticipant)
//!    degraded ∧ author(x) ∉ S                                             → Hidden(ModerationUnavailable)
//! V3 effective(x, hidden)                → Hidden(Removed{rank, actor, spam = effective(x, spam),
//!                                                         reason if the winner's showReason})
//! V4 author(x) ∈ B ∧ keepRank(x) < banRank(author)                        → Hidden(Banned)
//! V5 cap(x) ∈ restrict(author) ∧ keepRank(x) < restrictRank(author)      → Hidden(Restricted)
//! V6 comment ∧ AuthorModerated(x) (§4.9, L5)                              → Hidden(AuthorModerated)
//! V7 comment, p = x.root.uri: effective(p, locked) ∧ x ∉ allowed(p) ∧ author(x) ∉ A
//!    ∧ keepRank(x) < lockRank(p)                                          → Hidden(Locked)
//! ```
//!
//! V1–V5 read only validity and L0–L2, so L5 may use them (§1.4); V6 needs L5; V7 reads L1.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{AccessLevel, Capability};

use crate::social::full::author::AuthorTier;
use crate::social::full::cx::Cx;
use crate::social::full::moderation::Moderation;
use crate::social::full::view::{Dim, HiddenReason, Participation, Removed, Stage, State};
use crate::social::input::SkelData;

/// `cap(x)`: the capability of an item's collection (§10.1).
pub fn cap(coll: Option<Collection>) -> Option<Capability> {
    match coll? {
        Collection::Post => Some(Capability::Post),
        Collection::Comment => Some(Capability::Comment),
        Collection::Sequence => Some(Capability::Sequence),
        Collection::WikiRevision => Some(Capability::Wiki),
        _ => None,
    }
}

/// The first matching rule of V1–V5 for every candidate item (`None`: none matches).
pub fn v1_v5(
    cx: &Cx<'_>,
    l1: &Moderation,
    l2: &Participation,
    degraded: bool,
) -> BTreeMap<String, Option<State>> {
    cx.items()
        .map(|x| {
            let author = x.did.as_str();
            let keep = l1.keep_rank(&x.uri);
            let state = if cx.excluded.contains(&x.uri) {
                Some(State::Excluded)
            } else if !l2.p.contains(author) {
                Some(State::Hidden(HiddenReason::NotParticipant))
            } else if degraded && !l2.s.contains(author) {
                Some(State::Hidden(HiddenReason::ModerationUnavailable))
            } else if l1.hidden(&x.uri) {
                l1.get(&x.uri, Dim::Hidden).map(|w| {
                    let winner = l1.records.get(&w.winner);
                    State::Hidden(HiddenReason::Removed(Removed {
                        rank: w.rank,
                        actor: winner.map(|m| m.actor.clone()).unwrap_or_default(),
                        spam: l1.spam(&x.uri),
                        reason: winner
                            .filter(|m| m.show_reason)
                            .and_then(|m| m.reason.clone()),
                    }))
                })
            } else if matches!(l1.access(author), Some((AccessLevel::Banned, r)) if keep < r) {
                Some(State::Hidden(HiddenReason::Banned))
            } else if let Some((caps, r)) = l1.restrict(author)
                && cap(x.coll).is_some_and(|c| caps.contains(&c))
                && keep < r
            {
                Some(State::Hidden(HiddenReason::Restricted))
            } else {
                None
            };
            (x.uri.clone(), state)
        })
        .collect()
}

/// The comments meeting V7's condition (whatever an earlier rule decides).
pub fn locked_out(cx: &Cx<'_>, a: &BTreeMap<String, u8>, l1: &Moderation) -> BTreeSet<String> {
    cx.items()
        .filter(|x| x.coll == Some(Collection::Comment))
        .filter(|x| {
            let SkelData::Comment {
                root: Some(root), ..
            } = &x.data
            else {
                return false;
            };
            l1.lock(&root.uri).is_some_and(|(lock_rank, allowed)| {
                !allowed.contains(&x.uri)
                    && !a.contains_key(&x.did)
                    && l1.keep_rank(&x.uri) < lock_rank
            })
        })
        .map(|x| x.uri.clone())
        .collect()
}

/// The first matching rule of V1–V7 for every candidate item (`None`: V8–V10 decide).
pub fn v1_v7(
    cx: &Cx<'_>,
    base: &BTreeMap<String, Option<State>>,
    author: &Stage<AuthorTier>,
    locked_out: &BTreeSet<String>,
) -> Stage<BTreeMap<String, Option<State>>> {
    let author = author.as_ref().map_err(|p| *p)?;
    Ok(base
        .iter()
        .map(|(u, st)| {
            let comment = cx.skel(u).is_some_and(|x| x.coll == Some(Collection::Comment));
            let state = st.clone().or_else(|| {
                if comment && author.moderated.contains(u) {
                    Some(State::Hidden(HiddenReason::AuthorModerated))
                } else if locked_out.contains(u) {
                    Some(State::Hidden(HiddenReason::Locked))
                } else {
                    None
                }
            });
            (u.clone(), state)
        })
        .collect())
}
