// SPDX-License-Identifier: AGPL-3.0-or-later
//! L3: opinion validity (`karma/1`, `social.md` §5), vote power (§6.1) and the karma stages
//! (§6.2). **STUB**: the layer agent replaces [`karma`] and [`power`] (keep the signatures, or
//! update `full.rs`, `project.rs` and the runner with them).
//!
//! ```text
//! counts(v) ⇔ O1 v valid ∧ v.forum = F
//!           ∧ O2 (records[v.subject.uri] is a candidate item not Excluded by V1 ∨ v.subject.uri ∈ absent)
//!           ∧ O3 did(v.uri) ≠ did(v.subject.uri)
//!           ∧ O4 did(v.uri) ∉ B ∪ N
//!           ∧ O5 effective(v.subject.uri, spam) = false
//! axes (§5 table), per counting opinion: karma (known, not restricted from vote); agreement
//!   (known, not restricted from vote, voting ≠ karmaOnly); reacts (voting = reacts, not
//!   restricted from react, subject not absent); tags (subject a post, not absent, not
//!   restricted from tag)
//! V = { counting v with the karma axis, coll(subject) ∈ {post, comment} }
//! K1 = pairsum(None, S); seedUp; E = S ∪ { u ∈ P ∖ S : u ∉ B ∪ N ∧ K1[u] ≥ establish
//!                                           ∧ seedUp[u] ≥ establishSeeds }
//! KP = pairsum(Some(K1), E); K = pairsum(Some(KP), E)                      (§6.2, saturating)
//! ```
//!
//! Inputs: `cx` (opinions, `absent`, V1 via [`Cx::live`], policy `voting`, `pairCap`,
//! `establish`, `establishSeeds`), `a` (L0), `l1` (`spam`, `restrict` via
//! [`Moderation::restricted`]), `l2` (P, S, B, N). Never reads L4+ (§6.3 P4).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{Agreement, VoteValue};

use crate::social::full::cx::Cx;
use crate::social::full::moderation::Moderation;
use crate::social::full::view::{Participation, Pending, Stage};

/// The pending marker of [`karma`].
pub const PENDING: Pending = Pending {
    stub: "karma::karma",
    layer: "L3",
    section: "§5, §6.2",
};

/// The pending marker of [`power`].
pub const POWER_PENDING: Pending = Pending {
    stub: "karma::power",
    layer: "L3",
    section: "§6.1",
};

/// A counting opinion (O1–O5) with its axes resolved (§5 table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountingOpinion {
    /// `voter(v) = did(v.uri)`.
    pub voter: String,
    /// `v.subject.uri`.
    pub subject: String,
    /// `v.subject.cid` (the "earlier version" display, §7).
    pub subject_cid: String,
    /// `v.subject.uri ∈ absent` (the deleted-subject rule).
    pub absent: bool,
    /// The karma axis, when present and enabled.
    pub karma: Option<VoteValue>,
    /// The agreement axis, when present and enabled.
    pub agreement: Option<Agreement>,
    /// The reacts axis is enabled.
    pub reacts: bool,
    /// The tags axis is enabled.
    pub tags: bool,
}

/// L3 results (§5, §6.2). Integer maps hold only non-zero entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Karma {
    /// The uris of the opinions that satisfy O1–O5 (absent subjects included).
    pub counting: BTreeSet<String>,
    /// The counting opinions with their axes, by uri.
    pub opinions: BTreeMap<String, CountingOpinion>,
    /// K1 (stage 1, from seeds).
    pub k1: BTreeMap<String, i64>,
    /// `seedUp`.
    pub seed_up: BTreeMap<String, i64>,
    /// E, the eligible voters (S ⊆ E).
    pub e: BTreeSet<String>,
    /// KP (the power basis).
    pub kp: BTreeMap<String, i64>,
    /// K (the displayed karma; gates use it).
    pub k: BTreeMap<String, i64>,
}

impl Karma {
    /// `K[u]` (0 when missing).
    pub fn k_of(&self, did: &str) -> i64 {
        self.k.get(did).copied().unwrap_or(0)
    }

    /// `KP[u]` (0 when missing).
    pub fn kp_of(&self, did: &str) -> i64 {
        self.kp.get(did).copied().unwrap_or(0)
    }
}

/// **STUB (L3, `social.md` §5, §6.2).** Opinion validity, axes, K1, `seedUp`, E, KP and K.
///
/// Exact already when `records` holds no opinion: V = ∅, so every map is empty and E = S
/// (`seedUp[u] = 0 < establishSeeds`, which FO4 keeps ≥ 1).
pub fn karma(
    cx: &Cx<'_>,
    _a: &BTreeMap<String, u8>,
    _l1: &Moderation,
    l2: &Participation,
) -> Stage<Karma> {
    let no_opinions = !cx.records().any(|x| x.coll == Some(Collection::Opinion));
    if no_opinions && cx.policy.establish_seeds >= 1 {
        return Ok(Karma {
            e: l2.s.clone(),
            ..Karma::default()
        });
    }
    Err(PENDING)
}

/// **STUB (L3, `social.md` §6.1).** `power(k, x)` for `x` ∈ {`up`, `down`, `strongUp`,
/// `strongDown`, `agree`, `disagree`, `strongAgree`, `strongDisagree`} (k < 0 counts as 0).
pub fn power(_k: i64, _x: &str) -> Stage<i64> {
    Err(POWER_PENDING)
}
