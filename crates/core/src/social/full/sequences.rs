// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: the sequence library (`karma/1`, `social.md` §14). **STUB**: [`library`] is replaced
//! when sequences are built (M5; no vector kind yet, §19).
//!
//! ```text
//! eligible(s) ⇔ s is a valid sequence in F ∧ s not Hidden by V1–V5
//! listed(s)   ⇔ eligible(s) ∧ (passes(author(s), sequence) ∨ effective(s, pin[forum]))
//! order       = (forum-pinned desc, score desc, t_rank desc, uri asc)
//! entries(s)  = concat(chapters[i].posts)
//! ```
//!
//! Inputs: `cx` (sequence values), `l1` (pins), `v1_v5`, `scores`, `gates` (sequence),
//! `times` (`t_rank`).

use std::collections::{BTreeMap, BTreeSet};

use crate::social::full::cx::Cx;
use crate::social::full::gates::Gates;
use crate::social::full::moderation::Moderation;
use crate::social::full::scores::Score;
use crate::social::full::view::{ItemTime, Pending, Stage, State};

/// The pending marker of [`library`].
pub const PENDING: Pending = Pending {
    stub: "sequences::library",
    layer: "L5",
    section: "§14",
};

/// L5 sequence results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    /// `eligible(s)`.
    pub eligible: BTreeSet<String>,
    /// The listed sequences, in library order.
    pub listed: Vec<String>,
    /// `entries(s)` of every eligible sequence (before the displayable filter).
    pub entries: BTreeMap<String, Vec<String>>,
}

/// **STUB (L5, `social.md` §14).** The sequence library.
pub fn library(
    _cx: &Cx<'_>,
    _l1: &Moderation,
    _v1_v5: &BTreeMap<String, Option<State>>,
    scores: &Stage<BTreeMap<String, Score>>,
    gates: &Stage<Gates>,
    _times: &BTreeMap<String, ItemTime>,
) -> Stage<Library> {
    let _scores = scores.as_ref().map_err(|p| *p)?;
    let _gates = gates.as_ref().map_err(|p| *p)?;
    Err(PENDING)
}
