// SPDX-License-Identifier: AGPL-3.0-or-later
//! L3.5: scores and agreement (`karma/1`, `social.md` §7). **STUB**: the layer agent replaces
//! [`scores`].
//!
//! ```text
//! for every candidate item i not Excluded by V1 (Hidden ones included), a = author(i):
//! self(i)       = a ∈ E ? (post ? strong(max(KP[a], 0)) : small(max(KP[a], 0))) : 0
//! W(i)          = { counting v on i : voter(v) ∈ E }
//! score(i)      = self(i) + Σ_{v ∈ W(i), karma axis} power(KP[voter], v.karma)       (no pair cap)
//! agreement(i)  = voting = karmaOnly ? None : Σ_{v ∈ W(i), agreement axis} power(KP[voter], v.agreement)
//! unweighted(i) = |{ counting v on i : karma axis, voter ∉ E }|
//! earlier(i)    = |{ v ∈ W(i) : karma axis, v.subject.cid ≠ cid(i) }|;  weighted = |{ v ∈ W(i) : karma axis }|
//! ```
//!
//! Inputs: `cx` (items, V1 via [`Cx::live`], CIDs, `voting`), `karma` (E, KP, the counting
//! opinions and their axes).

use std::collections::BTreeMap;

use crate::social::full::cx::Cx;
use crate::social::full::karma::Karma;
use crate::social::full::view::{Pending, Stage};

/// The pending marker of [`scores`].
pub const PENDING: Pending = Pending {
    stub: "scores::scores",
    layer: "L3.5",
    section: "§7",
};

/// The §7 values of one item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Score {
    /// `score(i)`.
    pub score: i64,
    /// `agreement(i)`; `None` under `voting = karmaOnly`.
    pub agreement: Option<i64>,
    /// `unweighted(i)`: "n votes not yet counted".
    pub unweighted: u32,
    /// `earlier(i)`: N of "N of M votes were cast on an earlier version".
    pub earlier: u32,
    /// M of "N of M": the weighted karma-axis votes.
    pub weighted: u32,
}

/// **STUB (L3.5, `social.md` §7).** The score of every candidate item not Excluded by V1.
pub fn scores(_cx: &Cx<'_>, karma: &Stage<Karma>) -> Stage<BTreeMap<String, Score>> {
    let _karma = karma.as_ref().map_err(|p| *p)?;
    Err(PENDING)
}
