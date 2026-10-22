// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: react aggregation (`karma/1` with the `default/1` palette, `social.md` §9.2). **STUB**:
//! the layer agent replaces [`reacts`].
//!
//! ```text
//! per counting opinion v with the reacts axis enabled (§5):
//!   entries(v) = [ e ∈ v.reacts : e.react ∈ palette ∧ e.stance ∈ {for, against} ]  // default for
//!   qkey(e)    = e.quote ? norm(trim_ws(exact)) (lfm-1.md §11) : ""
//!   group by (react, qkey): both stances → dropped; else one (react, q, stance)
//! per subject s and (react, q):
//!   Fv = voters ∈ E with for; Av = voters ∈ E with against
//!   C  = { u ∈ Fv : passes(u, reactCreate) };  valid ⇔ C ≠ ∅
//!   creator = argmax K over C, ties → smallest DID
//!   supporters = { u ∈ Fv : passes(u, reactSecond) } ∪ {creator};  opposers = { u ∈ Av : passes(u, reactAnti) }
//! ```
//!
//! Inputs: `cx` (opinion values for `reacts`), `a` (L0), `karma` (E, K, counting opinions with
//! the reacts axis), `gates` (passes). The palette is `docs/spec/reacts-default-1.json`; `qkey` is
//! `lessdugin_lexicon::records::qkey`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::social::full::cx::Cx;
use crate::social::full::gates::Gates;
use crate::social::full::karma::Karma;
use crate::social::full::view::{Pending, Stage};

/// The pending marker of [`reacts`].
pub const PENDING: Pending = Pending {
    stub: "reacts::reacts",
    layer: "L5",
    section: "§9.2",
};

/// One valid react on a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactTally {
    /// The palette id.
    pub react: String,
    /// The quote key (`""` for the whole item).
    pub qkey: String,
    /// The creator's DID.
    pub creator: String,
    /// The supporters, the creator included.
    pub supporters: BTreeSet<String>,
    /// The opposers.
    pub opposers: BTreeSet<String>,
    /// The creator's quote (display only): the first entry of the creator's group.
    pub quote: Option<Value>,
}

/// **STUB (L5, `social.md` §9.2).** The valid reacts of every subject with one, each list
/// sorted by (react, qkey) bytes.
pub fn reacts(
    _cx: &Cx<'_>,
    _a: &BTreeMap<String, u8>,
    karma: &Stage<Karma>,
    gates: &Stage<Gates>,
) -> Stage<BTreeMap<String, Vec<ReactTally>>> {
    let _karma = karma.as_ref().map_err(|p| *p)?;
    let _gates = gates.as_ref().map_err(|p| *p)?;
    Err(PENDING)
}
