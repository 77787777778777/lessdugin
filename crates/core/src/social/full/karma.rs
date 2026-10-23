// SPDX-License-Identifier: AGPL-3.0-or-later
//! L3: opinion validity (`karma/1`, `social.md` §5), vote power (§6.1) and the karma stages
//! (§6.2).
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
//! power(k, x) = ±small(max(k, 0)) for up/down, agree/disagree; ±strong(max(k, 0)) for the
//!               strong values (LADDER, first threshold ≤ k; else 1)
//! V = { counting v with the karma axis, coll(subject) ∈ {post, comment} }
//! included(v, Elig) ⇔ voter ∈ Elig ∧ (subject ∉ absent ∨ ≥ 2 distinct voters of Elig in V on it)
//! pairsum(basis, Elig): per (voter, recipient) Σ (basis ? power(basis[voter], x) : sign(x)),
//!                       clamped to ±(basis ? pairCap × strong(max(basis[voter], 0)) : pairCap)
//! K1 = pairsum(None, S); seedUp[u] = |{ s ∈ S : Σ sign over included(v, S), s → u > 0 }|
//! E  = S ∪ { u ∈ P ∖ S : u ∉ B ∪ N ∧ K1[u] ≥ establish ∧ seedUp[u] ≥ establishSeeds }
//! KP = pairsum(Some(K1), E); K = pairsum(Some(KP), E)                      (§6.2, saturating)
//! ```
//!
//! Inputs: `cx` (opinions, `absent`, V1 via [`Cx::live`], policy `voting`, `pairCap`,
//! `establish`, `establishSeeds`), `l1` (`spam`, `restrict` via [`Moderation::restricted`]),
//! `l2` (P, S, B, N). Never reads L4+ (§6.3 P4). Opinions are read from their JSON values, so
//! the fold runs under any namespace.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{Agreement, Capability, VoteValue, Voting};
use serde_json::Value;

use crate::social::full::cx::Cx;
use crate::social::full::moderation::Moderation;
use crate::social::full::view::{Participation, Stage};
use crate::social::input::split_uri;

/// `LADDER` (§6.1): `(threshold, power)`, the first entry with `k ≥ threshold` wins.
pub const LADDER: [(i64, i64); 15] = [
    (500_000, 16),
    (250_000, 15),
    (175_000, 14),
    (100_000, 13),
    (75_000, 12),
    (50_000, 11),
    (25_000, 10),
    (10_000, 9),
    (5_000, 8),
    (2_500, 7),
    (1_000, 6),
    (500, 5),
    (250, 4),
    (100, 3),
    (10, 2),
];

/// `small(k)` (§6.1): 2 from 1000 karma, else 1.
pub const fn small(k: i64) -> i64 {
    if k >= 1000 { 2 } else { 1 }
}

/// `strong(k)` (§6.1): the power of the first `LADDER` entry with `k ≥ threshold`, else 1.
pub fn strong(k: i64) -> i64 {
    LADDER.iter().find(|(t, _)| k >= *t).map_or(1, |(_, p)| *p)
}

/// `power(k, x)` of a karma value (§6.1; k < 0 counts as 0).
pub fn vote_power(k: i64, x: VoteValue) -> i64 {
    let k = k.max(0);
    match x {
        VoteValue::Up => small(k),
        VoteValue::Down => -small(k),
        VoteValue::StrongUp => strong(k),
        VoteValue::StrongDown => -strong(k),
    }
}

/// `power(k, x)` of an agreement value (§6.1; k < 0 counts as 0).
pub fn agreement_power(k: i64, x: Agreement) -> i64 {
    let k = k.max(0);
    match x {
        Agreement::Agree => small(k),
        Agreement::Disagree => -small(k),
        Agreement::StrongAgree => strong(k),
        Agreement::StrongDisagree => -strong(k),
    }
}

/// `sign(x)` (§6.1): +1 for `up` and `strongUp`, −1 for `down` and `strongDown`.
pub const fn sign(x: VoteValue) -> i64 {
    match x {
        VoteValue::Up | VoteValue::StrongUp => 1,
        VoteValue::Down | VoteValue::StrongDown => -1,
    }
}

/// `power(k, x)` (§6.1) for `x` ∈ {`up`, `down`, `strongUp`, `strongDown`, `agree`,
/// `disagree`, `strongAgree`, `strongDisagree`} (k < 0 counts as 0); `None` for any other `x`.
pub fn power(k: i64, x: &str) -> Option<i64> {
    VoteValue::parse(x)
        .map(|v| vote_power(k, v))
        .or_else(|| Agreement::parse(x).map(|a| agreement_power(k, a)))
}

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

    /// `K1[u]` (0 when missing).
    pub fn k1_of(&self, did: &str) -> i64 {
        self.k1.get(did).copied().unwrap_or(0)
    }
}

fn str_at<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key)?.as_str()
}

/// O1–O5 and the axes of the §5 table, for every opinion in `records`.
fn counting_opinions(
    cx: &Cx<'_>,
    l1: &Moderation,
    l2: &Participation,
) -> BTreeMap<String, CountingOpinion> {
    let voting = cx.policy.voting;
    let mut out = BTreeMap::new();
    for x in cx.records().filter(|x| x.coll == Some(Collection::Opinion)) {
        // O1: valid (G1–G10, OP1, OP2) and v.forum = F
        if !cx.in_forum(x) {
            continue;
        }
        let Some(v) = x.value() else { continue };
        let Some(subject) = v.get("subject") else {
            continue;
        };
        let Some(s) = str_at(subject, "uri") else {
            continue;
        };
        let absent = cx.is_absent(s);
        // O2: a candidate item not Excluded by V1 (CO5 included), or confirmed absent
        if !(cx.live(s) || absent) {
            continue;
        }
        // O3: self-opinions are ignored entirely
        let Some((recipient, _, _)) = split_uri(s) else {
            continue;
        };
        if x.did == recipient {
            continue;
        }
        // O4
        if l2.b.contains(&x.did) || l2.n.contains(&x.did) {
            continue;
        }
        // O5 (reads the uri, so it applies to absent subjects too)
        if l1.spam(s) {
            continue;
        }
        let vote_ok = !l1.restricted(&x.did, Capability::Vote);
        let karma = str_at(v, "karma")
            .and_then(VoteValue::parse)
            .filter(|_| vote_ok);
        let agreement = str_at(v, "agreement")
            .and_then(Agreement::parse)
            .filter(|_| vote_ok && voting != Voting::KarmaOnly);
        let reacts =
            voting == Voting::Reacts && !absent && !l1.restricted(&x.did, Capability::React);
        let tags = cx.coll_of(s) == Some(Collection::Post)
            && !absent
            && !l1.restricted(&x.did, Capability::Tag);
        out.insert(
            x.uri.clone(),
            CountingOpinion {
                voter: x.did.clone(),
                subject: s.to_owned(),
                subject_cid: str_at(subject, "cid").unwrap_or_default().to_owned(),
                absent,
                karma,
                agreement,
                reacts,
                tags,
            },
        );
    }
    out
}

/// One element of V (§6.2).
struct Vote<'k> {
    voter: &'k str,
    recipient: &'k str,
    subject: &'k str,
    absent: bool,
    value: VoteValue,
}

/// V: the counting opinions with an enabled karma axis on a post or comment (absent subjects
/// included, their kind read from `coll(uri)`).
fn votes<'k>(cx: &Cx<'_>, opinions: &'k BTreeMap<String, CountingOpinion>) -> Vec<Vote<'k>> {
    opinions
        .values()
        .filter_map(|o| {
            let value = o.karma?;
            if !matches!(
                cx.coll_of(&o.subject),
                Some(Collection::Post | Collection::Comment)
            ) {
                return None;
            }
            let (recipient, _, _) = split_uri(&o.subject)?;
            Some(Vote {
                voter: &o.voter,
                recipient,
                subject: &o.subject,
                absent: o.absent,
                value,
            })
        })
        .collect()
}

/// `included(v, Elig)` for every element of V, in order.
fn included(v: &[Vote<'_>], elig: &BTreeSet<String>) -> Vec<bool> {
    // the distinct eligible voters of V on each absent subject
    let mut voters: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for x in v.iter().filter(|x| x.absent && elig.contains(x.voter)) {
        voters.entry(x.subject).or_default().insert(x.voter);
    }
    v.iter()
        .map(|x| {
            elig.contains(x.voter)
                && (!x.absent || voters.get(x.subject).is_some_and(|s| s.len() >= 2))
        })
        .collect()
}

fn get(m: &BTreeMap<String, i64>, k: &str) -> i64 {
    m.get(k).copied().unwrap_or(0)
}

/// `clamp(x, lo, hi) = max(lo, min(hi, x))` (§0.4).
fn clamp(x: i64, lo: i64, hi: i64) -> i64 {
    x.min(hi).max(lo)
}

/// `pairsum(basis, Elig)` (§6.2), non-zero entries only.
fn pairsum(
    v: &[Vote<'_>],
    basis: Option<&BTreeMap<String, i64>>,
    elig: &BTreeSet<String>,
    pair_cap: i64,
) -> BTreeMap<String, i64> {
    let mut acc: BTreeMap<(&str, &str), i64> = BTreeMap::new();
    for (x, inc) in v.iter().zip(included(v, elig)) {
        if !inc {
            continue;
        }
        let w = match basis {
            None => sign(x.value),
            Some(b) => vote_power(get(b, x.voter), x.value),
        };
        let e = acc.entry((x.voter, x.recipient)).or_insert(0);
        *e = e.saturating_add(w);
    }
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for ((u, r), x) in acc {
        let cap = match basis {
            None => pair_cap,
            Some(b) => pair_cap.saturating_mul(strong(get(b, u).max(0))),
        };
        let e = out.entry(r.to_owned()).or_insert(0);
        *e = e.saturating_add(clamp(x, cap.saturating_neg(), cap));
    }
    out.retain(|_, x| *x != 0);
    out
}

/// `seedUp` (§6.2): per recipient, the seeds whose sign sum over `included(v, S)` is positive
/// (no pair cap; a seed with net 0 does not count). Non-zero entries only.
fn seed_up(v: &[Vote<'_>], s: &BTreeSet<String>) -> BTreeMap<String, i64> {
    let mut acc: BTreeMap<(&str, &str), i64> = BTreeMap::new();
    for (x, inc) in v.iter().zip(included(v, s)) {
        if inc {
            let e = acc.entry((x.voter, x.recipient)).or_insert(0);
            *e = e.saturating_add(sign(x.value));
        }
    }
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for ((_, r), x) in acc {
        if x > 0 {
            *out.entry(r.to_owned()).or_insert(0) += 1;
        }
    }
    out
}

/// L3 (`social.md` §5, §6.2): opinion validity, axes, K1, `seedUp`, E, KP and K.
///
/// `_a` (L0) is not read: authorities enter through S (L2), and A never changes an opinion's
/// validity.
pub fn karma(
    cx: &Cx<'_>,
    _a: &BTreeMap<String, u8>,
    l1: &Moderation,
    l2: &Participation,
) -> Stage<Karma> {
    let p = &cx.policy;
    let opinions = counting_opinions(cx, l1, l2);
    let v = votes(cx, &opinions);
    let k1 = pairsum(&v, None, &l2.s, p.pair_cap);
    let seed_up = seed_up(&v, &l2.s);
    let mut e = l2.s.clone();
    for u in l2.p.difference(&l2.s) {
        if !l2.b.contains(u)
            && !l2.n.contains(u)
            && get(&k1, u) >= p.establish
            && get(&seed_up, u) >= p.establish_seeds
        {
            e.insert(u.clone());
        }
    }
    let kp = pairsum(&v, Some(&k1), &e, p.pair_cap);
    let k = pairsum(&v, Some(&kp), &e, p.pair_cap);
    Ok(Karma {
        counting: opinions.keys().cloned().collect(),
        opinions,
        k1,
        seed_up,
        e,
        kp,
        k,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_boundaries() {
        assert_eq!(strong(-5), 1);
        assert_eq!(strong(9), 1);
        assert_eq!(strong(10), 2);
        assert_eq!(strong(999), 5);
        assert_eq!(strong(1000), 6);
        assert_eq!(strong(499_999), 15);
        assert_eq!(strong(500_000), 16);
        assert_eq!(strong(i64::MAX), 16);
        assert_eq!(small(999), 1);
        assert_eq!(small(1000), 2);
        assert_eq!(power(-5, "strongUp"), Some(1));
        assert_eq!(power(1000, "down"), Some(-2));
        assert_eq!(power(100, "strongDisagree"), Some(-3));
        assert_eq!(power(100, "superUp"), None);
        assert_eq!(sign(VoteValue::StrongDown), -1);
    }

    #[test]
    fn clamp_is_max_of_min() {
        assert_eq!(clamp(26, -25, 25), 25);
        assert_eq!(clamp(-26, -25, 25), -25);
        assert_eq!(clamp(3, -25, 25), 3);
    }
}
