// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: the sequence library (`karma/1`, `social.md` §14).
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

use lessdugin_lexicon::Collection;
use serde_json::Value;

use crate::social::full::cx::Cx;
use crate::social::full::gates::{Gate, Gates};
use crate::social::full::moderation::Moderation;
use crate::social::full::scores::Score;
use crate::social::full::view::{ItemTime, Stage, State};

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

/// L5 (`social.md` §14): eligibility and the sequence library order. Sequence scores and
/// times are already computed by the common item layers; no sequence contributes karma.
pub fn library(
    cx: &Cx<'_>,
    l1: &Moderation,
    v1_v5: &BTreeMap<String, Option<State>>,
    scores: &Stage<BTreeMap<String, Score>>,
    gates: &Stage<Gates>,
    times: &BTreeMap<String, ItemTime>,
) -> Stage<Library> {
    let scores = scores.as_ref().map_err(|p| *p)?;
    let gates = gates.as_ref().map_err(|p| *p)?;
    let mut out = Library::default();
    for x in cx.records().filter(|x| {
        x.coll == Some(Collection::Sequence)
            && cx.live(&x.uri)
            && matches!(v1_v5.get(&x.uri), Some(None))
    }) {
        let Some(v) = cx.value(&x.uri) else { continue };
        let entries = v
            .get("chapters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.get("posts")?.as_array())
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        out.entries.insert(x.uri.clone(), entries);
        out.eligible.insert(x.uri.clone());
    }
    out.listed = out
        .eligible
        .iter()
        .filter(|uri| {
            cx.skel(uri)
                .is_some_and(|x| l1.pinned_forum(uri) || gates.pass(&x.did, Gate::Sequence))
        })
        .cloned()
        .collect();
    out.listed.sort_by(|a, b| {
        let pin = |u: &String| l1.pinned_forum(u);
        let score = |u: &String| scores.get(u).map_or(0, |s| s.score);
        let time = |u: &String| times.get(u).map_or(0, |t| t.t_rank);
        pin(b)
            .cmp(&pin(a))
            .then_with(|| score(b).cmp(&score(a)))
            .then_with(|| time(b).cmp(&time(a)))
            .then_with(|| a.cmp(b))
    });
    Ok(out)
}
