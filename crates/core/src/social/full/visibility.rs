// SPDX-License-Identifier: AGPL-3.0-or-later
//! L6: visibility V8–V10 and the thread pass (`mod/1`, `social.md` §10.1, §10.2). V1–V7 are
//! [`crate::social::full::modstate`]'s. **STUB**: the layer agent replaces [`visibility`].
//!
//! ```text
//! state(x) = v1_v7(x) if Some, else
//!            V8  mode = open ∧ author(x) ∉ S ∪ E: newUsers collapse → Collapsed(NewUser),
//!                hide → Hidden(NewUser), show → no match
//!            V9  score(x) ≤ display.collapseBelow → Collapsed(LowScore)
//!            V10 Visible
//! thread pass: a Hidden post or comment with a listed descendant in the placed tree
//!              (through Missing placeholders; Excluded comments are not nodes; judged on the
//!              states before the pass) becomes Tombstone, keeping its reason and Removed{…};
//!              every other state is unchanged; a placeholder is shown iff it has a listed
//!              descendant
//! ```
//!
//! Inputs: `cx` (items, policy `display`), `l2` (mode, S), `v1_v7`, `karma` (E), `scores`,
//! `threads` (placements).

use std::collections::{BTreeMap, BTreeSet};

use crate::social::full::cx::Cx;
use crate::social::full::karma::Karma;
use crate::social::full::scores::Score;
use crate::social::full::view::{Participation, Pending, Stage, State};
use crate::social::threads::Placement;

/// The pending marker of [`visibility`].
pub const PENDING: Pending = Pending {
    stub: "visibility::visibility",
    layer: "L6",
    section: "§10.1 V8–V10, §10.2",
};

/// L6 visibility results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    /// V1–V10 for every candidate item (Excluded ones included), before the thread pass.
    pub states: BTreeMap<String, State>,
    /// The same items after the thread pass (Tombstones).
    pub displayed: BTreeMap<String, State>,
    /// The displayed `Missing` placeholders (with a listed descendant), by thread root.
    pub shown_missing: BTreeMap<String, BTreeSet<String>>,
}

/// **STUB (L6, `social.md` §10.1 V8–V10, §10.2).** Visibility states and the thread pass.
pub fn visibility(
    _cx: &Cx<'_>,
    _l2: &Participation,
    v1_v7: &Stage<BTreeMap<String, Option<State>>>,
    karma: &Stage<Karma>,
    scores: &Stage<BTreeMap<String, Score>>,
    _threads: &BTreeMap<String, Placement>,
) -> Stage<Visibility> {
    let _v1_v7 = v1_v7.as_ref().map_err(|p| *p)?;
    let _karma = karma.as_ref().map_err(|p| *p)?;
    let _scores = scores.as_ref().map_err(|p| *p)?;
    Err(PENDING)
}
