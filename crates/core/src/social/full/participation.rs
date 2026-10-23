// SPDX-License-Identifier: AGPL-3.0-or-later
//! L2: participation, membership modes and seeds (`mod/1`, `social.md` §3), and fail-closed
//! `degraded` (§4.11).
//!
//! ```text
//! member(d)  ⇔ a valid membership in d's repo with rkey fk and forum = F
//! access(d)  = effective(d, access) ∈ {member, banned, none}
//! G = {access = member}   B = {access = banned}   N = {effective(d, nullify)}
//! P = approval: A ∪ {member(d) ∧ access(d) ≠ none};   open: A ∪ {member(d)}
//! S = (A ∪ G) ∖ (B ∪ N)
//! applicants = { d ∉ A : member(d) ∧ access(d) = none }   (approval mode; empty in open mode)
//! degraded ⇔ ∃ a ∈ A : repoStatus[a] = Unavailable (a missing DID is Unavailable)
//! ```
//!
//! Newcomers (P ∖ (S ∪ E)) need E, so they are computed after L3.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{AccessLevel, MembershipMode};

use crate::social::full::cx::Cx;
use crate::social::full::moderation::Moderation;
use crate::social::full::view::{Dim, Participation};
use crate::store::repo::RepoStatus;

/// L2 (§3.1).
pub fn participation(cx: &Cx<'_>, a: &BTreeMap<String, u8>, l1: &Moderation) -> Participation {
    let mode = cx.policy.membership;
    let members: BTreeSet<String> = cx
        .records()
        .filter(|x| x.coll == Some(Collection::Membership) && x.rkey == cx.fk() && cx.in_forum(x))
        .map(|x| x.did.clone())
        .collect();
    let mut g = BTreeSet::new();
    let mut b = BTreeSet::new();
    let mut n = BTreeSet::new();
    for (subject, dims) in &l1.resolved {
        if !subject.starts_with("did:") {
            continue;
        }
        match l1.access(subject) {
            Some((AccessLevel::Member, _)) => {
                g.insert(subject.clone());
            }
            Some((AccessLevel::Banned, _)) => {
                b.insert(subject.clone());
            }
            None => {}
        }
        if dims.contains_key(&Dim::Nullify) && l1.nullify(subject) {
            n.insert(subject.clone());
        }
    }
    let p: BTreeSet<String> = match mode {
        MembershipMode::Approval => members
            .iter()
            .filter(|d| g.contains(*d) || b.contains(*d))
            .chain(a.keys())
            .cloned()
            .collect(),
        MembershipMode::Open => members.iter().chain(a.keys()).cloned().collect(),
    };
    let s: BTreeSet<String> = a
        .keys()
        .chain(g.iter())
        .filter(|d| !b.contains(*d) && !n.contains(*d))
        .cloned()
        .collect();
    let applicants = match mode {
        MembershipMode::Approval => members
            .iter()
            .filter(|d| !a.contains_key(*d) && l1.access(d).is_none())
            .cloned()
            .collect(),
        MembershipMode::Open => BTreeSet::new(),
    };
    Participation {
        mode,
        members,
        p,
        g,
        b,
        n,
        s,
        applicants,
    }
}

/// §4.11: some authority's repo is `Unavailable` (missing from `repoStatus` included).
pub fn degraded(cx: &Cx<'_>, a: &BTreeMap<String, u8>) -> bool {
    a.keys().any(|d| cx.status(d) == RepoStatus::Unavailable)
}
