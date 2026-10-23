// SPDX-License-Identifier: AGPL-3.0-or-later
//! L3.5: scores and agreement (`karma/1`, `social.md` §7).
//!
//! ```text
//! for every candidate item i not Excluded by V1 (Hidden ones included), a = author(i):
//! self(i)       = a ∈ E ? (post ? strong(max(KP[a], 0)) : small(max(KP[a], 0))) : 0
//! W(i)          = { counting v on i : voter(v) ∈ E }
//! score(i)      = self(i) + Σ_{v ∈ W(i), karma axis} power(KP[voter], v.karma)       (no pair cap)
//! agreement(i)  = voting = karmaOnly ? None : Σ_{v ∈ W(i), agreement axis} power(KP[voter], v.agreement)
//! unweighted(i) = |{ counting v on i : karma axis, voter ∉ E }|
//! earlier(i)    = |{ v ∈ W(i) : karma axis, v.subject.cid ≠ cid(i) (canonical forms, §0.4) }|;  weighted = |{ v ∈ W(i) : karma axis }|
//! ```
//!
//! Inputs: `cx` (items, V1, CIDs, `voting`), `karma` (E, KP, the counting opinions and their
//! axes). Opinions on absent subjects have no item, so they reach no score (§5).

use std::collections::BTreeMap;

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::Voting;

use crate::social::full::cids;
use crate::social::full::cx::Cx;
use crate::social::full::karma::{
    CountingOpinion, Karma, agreement_power, small, strong, vote_power,
};
use crate::social::full::view::Stage;

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

/// L3.5 (`social.md` §7): the score of every candidate item not Excluded by V1.
pub fn scores(cx: &Cx<'_>, karma: &Stage<Karma>) -> Stage<BTreeMap<String, Score>> {
    let k = karma.as_ref().map_err(|p| *p)?;
    let mut on: BTreeMap<&str, Vec<&CountingOpinion>> = BTreeMap::new();
    for o in k.opinions.values() {
        on.entry(o.subject.as_str()).or_default().push(o);
    }
    let karma_only = cx.policy.voting == Voting::KarmaOnly;
    let mut out = BTreeMap::new();
    for x in cx.items().filter(|x| !cx.excluded.contains(&x.uri)) {
        let a = x.did.as_str();
        let own = if k.e.contains(a) {
            let kp = k.kp_of(a).max(0);
            if x.coll == Some(Collection::Post) {
                strong(kp)
            } else {
                small(kp)
            }
        } else {
            0
        };
        let mut s = Score {
            score: own,
            agreement: (!karma_only).then_some(0),
            ..Score::default()
        };
        for o in on.get(x.uri.as_str()).into_iter().flatten() {
            if k.e.contains(&o.voter) {
                let kp = k.kp_of(&o.voter);
                if let Some(v) = o.karma {
                    s.score = s.score.saturating_add(vote_power(kp, v));
                    s.weighted = s.weighted.saturating_add(1);
                    if !cids::same(&o.subject_cid, &x.cid) {
                        s.earlier = s.earlier.saturating_add(1);
                    }
                }
                if let (Some(v), Some(sum)) = (o.agreement, s.agreement.as_mut()) {
                    *sum = sum.saturating_add(agreement_power(kp, v));
                }
            } else if o.karma.is_some() {
                s.unweighted = s.unweighted.saturating_add(1);
            }
        }
        out.insert(x.uri.clone(), s);
    }
    Ok(out)
}
