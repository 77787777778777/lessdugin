// SPDX-License-Identifier: AGPL-3.0-or-later
//! L1: forum-tier moderation (`mod/1`, `social.md` §4.2–§4.8, §4.10): assertions, authorisation,
//! CID binding, resolution, and the forum-scope mod-log statuses (§4.12).
//!
//! ```text
//! listed(m)    ⇔ m is a valid modAction with m.forum = F                        (mod log, raw view)
//! no effect    : reserved key ^FK-TID$ (MA2), unknown scope / action / value (MA7), #lock of a
//!                forum uri (MA9)                                                → reserved/unknown
//! dimensions   : #hide → hidden (default true), spam = hidden ∧ spam; #access → access;
//!                #restrict → restrict (known caps), nullify; #tier → tier; #pin → pin[scope];
//!                #lock → locked (+ allowed); #tag → tag.removed, tag.head (only if present)
//! positive     : hidden = false, tier ∈ {frontpage, curated}, pinned = true    (whole record)
//! counts(m)    ⇔ scope_eff = forum ∧ X ∈ A                                       else lapsed
//!                ∧ (X = owner ∨ X = subjectAuthor(T) ∨ rank(X) > subjectRank(T))
//!                ∧ role allows it (a moderator never curates)                   else unauthorised
//!                ∧ (until absent ∨ µs(until) > now)                             else expired
//!                ∧ (positive ⇒ T ∈ records ∧ subjectCid = cid(T))               else stale
//!                  (tag.head counts only if records[head.uri] has CID head.cid; else the record
//!                   still asserts tag.removed, with status stale)
//! resolve(T,d) : r = max rank; v* = safest value at r (restrict: union; tag.head: all equal, else
//!                a conflict and Default); winner = smallest uri at r with v* (restrict: at r)
//! allowed(p)   = ⋂ MA9-filtered allow-lists of the rank-r records with locked = true
//! status       : effective ⇔ some asserted d ≠ spam resolves at the record's rank to its value
//!                (restrict: any value); else overridden with the highest resolution rank
//! ```

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::keys::{self, ModActionKey};
use lessdugin_lexicon::values::{
    AccessLevel, ActionKind, Capability, DefaultTier, PinScope, Tier,
};
use serde_json::Value;

use crate::social::full::authority::{rank, subject_author, subject_rank};
use crate::social::full::cx::Cx;
use crate::social::full::view::{Dim, DimValue, ModStatus, Resolution};
use crate::social::input::{Skel, split_uri};
use crate::social::time;

/// `scope_eff` of a modAction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModScope {
    /// `forum` (also when `scope` is absent).
    Forum,
    /// `author` (§4.9).
    Author,
    /// Any other value (MA7: no effect).
    Unknown,
}

/// Why a valid modAction has no effect in v1 (status `reserved/unknown`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoEffect {
    /// MA2: the reserved key form `^FK-TID$`.
    ReservedKey,
    /// MA7: an unknown `scope`.
    UnknownScope,
    /// MA7: an unknown action `$type`.
    UnknownAction,
    /// MA7: an unknown `#access.level`, `#tier.tier` or `#pin.scope`.
    UnknownValue,
    /// MA9: a `#lock` of a forum uri.
    ForumLock,
}

/// A listed modAction (valid, `forum = F`), read from its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModRecord {
    /// The uri.
    pub uri: String,
    /// The CID.
    pub cid: String,
    /// The actor X = `did(m.uri)`.
    pub actor: String,
    /// `scope_eff`.
    pub scope: ModScope,
    /// `subject`.
    pub subject: String,
    /// `subjectCid`.
    pub subject_cid: Option<String>,
    /// The action, if known.
    pub action: Option<ActionKind>,
    /// Why the record has no effect, if it has none.
    pub no_effect: Option<NoEffect>,
    /// The asserted dimensions and values (§4.2), in table order.
    pub asserts: Vec<(Dim, DimValue)>,
    /// The record is positive (CID-bound, §4.5).
    pub positive: bool,
    /// `#tag.head` as `(uri, cid)`.
    pub head: Option<(String, String)>,
    /// `reason`.
    pub reason: Option<String>,
    /// `#hide.showReason` (default `true`).
    pub show_reason: bool,
    /// `until`: `None` when absent; `Some(None)` for a datetime without µs (before 1970).
    pub until: Option<Option<i64>>,
    /// `#lock.allowed` as written (before the MA9 filter).
    pub allowed: Vec<String>,
    /// `µs(updatedAt) ?? µs(createdAt)` (the mod-log time).
    pub time: Option<i64>,
}

impl ModRecord {
    /// Reads the listed modAction `x` (value `v`) under the namespace of `cx`.
    pub fn parse(cx: &Cx<'_>, x: &Skel, v: &Value) -> Self {
        let a = v.get("action");
        let field = |k: &str| a.and_then(|a| a.get(k));
        let flag = |k: &str, d: bool| field(k).and_then(Value::as_bool).unwrap_or(d);
        let text = |k: &str| field(k).and_then(Value::as_str);
        let subject = v
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let scope = match v.get("scope") {
            None => ModScope::Forum,
            Some(s) => match s.as_str() {
                Some("forum") => ModScope::Forum,
                Some("author") => ModScope::Author,
                _ => ModScope::Unknown,
            },
        };
        let action = text("$type").and_then(|t| ActionKind::from_type_in(cx.ns(), t));
        let mut no_effect = match keys::split_mod_action_key(&x.rkey) {
            Some(ModActionKey::Reserved { .. }) => Some(NoEffect::ReservedKey),
            _ => None,
        };
        if scope == ModScope::Unknown {
            no_effect = no_effect.or(Some(NoEffect::UnknownScope));
        }
        let mut asserts = Vec::new();
        let mut positive = false;
        let mut head = None;
        let mut allowed = Vec::new();
        let mut show_reason = true;
        match action {
            None => no_effect = no_effect.or(Some(NoEffect::UnknownAction)),
            Some(ActionKind::Hide) => {
                let hidden = flag("hidden", true);
                asserts.push((Dim::Hidden, DimValue::Bool(hidden)));
                asserts.push((
                    Dim::Spam,
                    DimValue::Bool(hidden && flag("spam", false)),
                ));
                positive = !hidden;
                show_reason = flag("showReason", true);
            }
            Some(ActionKind::Access) => match text("level").and_then(AccessLevel::parse) {
                Some(l) => asserts.push((Dim::Access, DimValue::Access(l))),
                None => no_effect = no_effect.or(Some(NoEffect::UnknownValue)),
            },
            Some(ActionKind::Restrict) => {
                let caps: BTreeSet<Capability> = field("capabilities")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| Capability::parse(c.as_str()?))
                    .collect();
                asserts.push((Dim::Restrict, DimValue::Caps(caps)));
                asserts.push((
                    Dim::Nullify,
                    DimValue::Bool(flag("nullifyVotes", false)),
                ));
            }
            Some(ActionKind::Tier) => match text("tier").and_then(Tier::parse) {
                Some(t) => {
                    asserts.push((Dim::Tier, DimValue::Tier(t)));
                    positive = t >= Tier::Frontpage;
                }
                None => no_effect = no_effect.or(Some(NoEffect::UnknownValue)),
            },
            Some(ActionKind::Pin) => {
                let pinned = flag("pinned", true);
                match text("scope").and_then(PinScope::parse) {
                    Some(PinScope::Forum) => asserts.push((Dim::PinForum, DimValue::Bool(pinned))),
                    Some(PinScope::Thread) => {
                        asserts.push((Dim::PinThread, DimValue::Bool(pinned)));
                    }
                    None => no_effect = no_effect.or(Some(NoEffect::UnknownValue)),
                }
                positive = pinned;
            }
            Some(ActionKind::Lock) => {
                if subject.starts_with("at://") && cx.coll_of(&subject) == Some(Collection::Forum)
                {
                    no_effect = no_effect.or(Some(NoEffect::ForumLock));
                }
                asserts.push((Dim::Locked, DimValue::Bool(flag("locked", true))));
                allowed = field("allowed")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.as_str().map(str::to_owned))
                    .collect();
            }
            Some(ActionKind::Tag) => {
                asserts.push((Dim::TagRemoved, DimValue::Bool(flag("removed", false))));
                if let Some(h) = field("head") {
                    let uri = h.get("uri").and_then(Value::as_str).unwrap_or_default();
                    let cid = h.get("cid").and_then(Value::as_str).unwrap_or_default();
                    head = Some((uri.to_owned(), cid.to_owned()));
                    asserts.push((
                        Dim::TagHead,
                        DimValue::Head {
                            uri: uri.to_owned(),
                            cid: cid.to_owned(),
                        },
                    ));
                }
            }
        }
        Self {
            uri: x.uri.clone(),
            cid: x.cid.clone(),
            actor: x.did.clone(),
            scope,
            subject,
            subject_cid: v
                .get("subjectCid")
                .and_then(Value::as_str)
                .map(str::to_owned),
            action,
            no_effect,
            asserts,
            positive,
            head,
            reason: v.get("reason").and_then(Value::as_str).map(str::to_owned),
            show_reason,
            until: v
                .get("until")
                .and_then(Value::as_str)
                .map(|u| time::us(u)),
            allowed,
            time: x
                .updated_at
                .as_deref()
                .and_then(time::us)
                .or_else(|| x.created_at.as_deref().and_then(time::us)),
        }
    }

    /// The value the record asserts for `d`.
    pub fn value_of(&self, d: Dim) -> Option<&DimValue> {
        self.asserts.iter().find(|(e, _)| *e == d).map(|(_, v)| v)
    }

    /// Condition 5 fails: `until` is present and `µs(until) > now` does not hold (a datetime
    /// before 1970 has no µs, so it has expired).
    pub fn expired(&self, now: i64) -> bool {
        self.until.is_some_and(|u| !u.is_some_and(|t| t > now))
    }

    /// The CID check of §4.5 for a positive record (`true` for a negative one).
    pub fn cid_ok(&self, cx: &Cx<'_>) -> bool {
        !self.positive
            || cx
                .skel(&self.subject)
                .is_some_and(|s| self.subject_cid.as_deref() == Some(s.cid.as_str()))
    }

    /// The `#tag.head` check of §4.5 (`true` without a head).
    pub fn head_ok(&self, cx: &Cx<'_>) -> bool {
        self.head
            .as_ref()
            .is_none_or(|(u, c)| cx.skel(u).is_some_and(|s| &s.cid == c))
    }

    /// The edited subject of a stale record, when it is in `records` (a mod-queue item, §4.12):
    /// the subject of a positive record, or `head.uri` of a `#tag`.
    pub fn edited_subject(&self, cx: &Cx<'_>) -> bool {
        let positive_edited = self.positive && !self.cid_ok(cx) && cx.skel(&self.subject).is_some();
        let head_edited = !self.head_ok(cx)
            && self
                .head
                .as_ref()
                .is_some_and(|(u, _)| cx.skel(u).is_some());
        positive_edited || head_edited
    }
}

/// The order of an ordered dimension's values, safest first (§4.2): 0 is the safest.
fn safety(d: Dim, v: &DimValue) -> u8 {
    match (d, v) {
        (Dim::PinForum | Dim::PinThread, DimValue::Bool(b)) => u8::from(*b),
        (_, DimValue::Bool(b)) => u8::from(!*b),
        (_, DimValue::Access(l)) => match l {
            AccessLevel::Banned => 0,
            AccessLevel::Member => 1,
        },
        (_, DimValue::Tier(t)) => match t {
            Tier::Personal => 0,
            Tier::Frontpage => 1,
            Tier::Curated => 2,
        },
        _ => 0,
    }
}

/// MA9: the `allowed` entries in the thread of post `p` (rkey prefix `{fk}-{threadTid(p)}-`, by
/// key alone).
pub fn filter_allowed(cx: &Cx<'_>, p: &str, entries: &[String]) -> BTreeSet<String> {
    let Ok(t) = keys::thread_tid(cx.ns(), p) else {
        return BTreeSet::new();
    };
    let prefix = format!("{}-{t}-", cx.fk());
    entries
        .iter()
        .filter(|e| split_uri(e).is_some_and(|(_, _, k)| k.starts_with(&prefix)))
        .cloned()
        .collect()
}

/// L1 results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moderation {
    /// Every listed modAction (valid, `forum = F`), of any scope, by uri.
    pub records: BTreeMap<String, ModRecord>,
    /// The mod-log status of every listed forum-scope record that may have an effect (records
    /// with no effect are `reserved/unknown` in either scope; author scope is §4.9's).
    pub forum_status: BTreeMap<String, ModStatus>,
    /// `resolve(T, d)` for every Resolved (subject, dimension).
    pub resolved: BTreeMap<String, BTreeMap<Dim, Resolution>>,
    /// Unordered `tag.head` conflicts at the top rank: subject → that rank (Default, plus a
    /// mod-queue item).
    pub conflicts: BTreeMap<String, u8>,
    /// `decl.defaultTier` (the default of `tier`).
    pub default_tier: Tier,
}

impl Moderation {
    /// `resolve(T, d)` when Resolved.
    pub fn get(&self, subject: &str, d: Dim) -> Option<&Resolution> {
        self.resolved.get(subject)?.get(&d)
    }

    fn flag(&self, subject: &str, d: Dim) -> bool {
        matches!(
            self.get(subject, d).map(|r| &r.value),
            Some(DimValue::Bool(true))
        )
    }

    /// `effective(T, hidden)`.
    pub fn hidden(&self, uri: &str) -> bool {
        self.flag(uri, Dim::Hidden)
    }

    /// `effective(T, spam)`.
    pub fn spam(&self, uri: &str) -> bool {
        self.flag(uri, Dim::Spam)
    }

    /// `effective(d, nullify)`.
    pub fn nullify(&self, did: &str) -> bool {
        self.flag(did, Dim::Nullify)
    }

    /// `effective(d, access)` with its rank (`None`: the default, none).
    pub fn access(&self, did: &str) -> Option<(AccessLevel, u8)> {
        match self.get(did, Dim::Access)? {
            Resolution {
                value: DimValue::Access(l),
                rank,
                ..
            } => Some((*l, *rank)),
            _ => None,
        }
    }

    /// `effective(d, restrict)` with `restrictRank(d)` (`None`: the default, ∅).
    pub fn restrict(&self, did: &str) -> Option<(&BTreeSet<Capability>, u8)> {
        match self.get(did, Dim::Restrict)? {
            Resolution {
                value: DimValue::Caps(c),
                rank,
                ..
            } => Some((c, *rank)),
            _ => None,
        }
    }

    /// `true` if `did` is restricted from `cap` (keeps do not exempt opinions, §5).
    pub fn restricted(&self, did: &str, cap: Capability) -> bool {
        self.restrict(did).is_some_and(|(c, _)| c.contains(&cap))
    }

    /// `effective(p, tier)`.
    pub fn tier(&self, post: &str) -> Tier {
        match self.get(post, Dim::Tier).map(|r| &r.value) {
            Some(DimValue::Tier(t)) => *t,
            _ => self.default_tier,
        }
    }

    /// `effective(T, pin[forum])`.
    pub fn pinned_forum(&self, uri: &str) -> bool {
        self.flag(uri, Dim::PinForum)
    }

    /// `effective(c, pin[thread])` when Resolved (`None`: Default, so the author tier may pin).
    pub fn pin_thread(&self, uri: &str) -> Option<bool> {
        match self.get(uri, Dim::PinThread)?.value {
            DimValue::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// `effective(p, locked) = true`: `(lockRank(p), allowed(p))`.
    pub fn lock(&self, post: &str) -> Option<(u8, &BTreeSet<String>)> {
        let r = self.get(post, Dim::Locked)?;
        match (&r.value, &r.allowed) {
            (DimValue::Bool(true), Some(a)) => Some((r.rank, a)),
            _ => None,
        }
    }

    /// `keepRank(x)`: r if `resolve(x, hidden) = Resolved{false, r, _}`, else 0.
    pub fn keep_rank(&self, uri: &str) -> u8 {
        match self.get(uri, Dim::Hidden) {
            Some(Resolution {
                value: DimValue::Bool(false),
                rank,
                ..
            }) => *rank,
            _ => 0,
        }
    }

    /// `effective("tag:" + slug, tag.removed)`.
    pub fn tag_removed(&self, slug: &str) -> bool {
        self.flag(&format!("tag:{slug}"), Dim::TagRemoved)
    }

    /// `effective("tag:" + slug, tag.head)`: `(uri, cid)` (`None`: none, or a conflict).
    pub fn tag_head(&self, slug: &str) -> Option<(&str, &str)> {
        match &self.get(&format!("tag:{slug}"), Dim::TagHead)?.value {
            DimValue::Head { uri, cid } => Some((uri, cid)),
            _ => None,
        }
    }
}

/// An assertion that counts at forum tier (§4.3).
struct Entry<'m> {
    rank: u8,
    value: &'m DimValue,
    rec: &'m ModRecord,
}

/// §4.3 conditions 2–5 for a forum-scope record with an effect: `Ok` if it may count (the CID
/// check is separate), else its status.
fn authorise(cx: &Cx<'_>, a: &BTreeMap<String, u8>, m: &ModRecord) -> Result<(), ModStatus> {
    let r = rank(a, &m.actor);
    if r == 0 {
        return Err(ModStatus::Lapsed);
    }
    let rank_ok = m.actor == cx.owner()
        || subject_author(&m.subject) == Some(m.actor.as_str())
        || r > subject_rank(a, &m.subject);
    let role_ok = !(matches!(m.value_of(Dim::Tier), Some(DimValue::Tier(Tier::Curated)))
        && r < crate::discovery::roster::RANK_ADMIN);
    if !rank_ok || !role_ok {
        return Err(ModStatus::Unauthorised);
    }
    if m.expired(cx.now()) {
        return Err(ModStatus::Expired);
    }
    Ok(())
}

/// L1: parses the listed modActions, resolves every forum-tier (subject, dimension) and gives
/// each forum-scope record its mod-log status.
pub fn forum_tier(cx: &Cx<'_>, a: &BTreeMap<String, u8>) -> Moderation {
    let records: BTreeMap<String, ModRecord> = cx
        .records()
        .filter(|x| x.coll == Some(Collection::ModAction) && cx.in_forum(x))
        .filter_map(|x| Some((x.uri.clone(), ModRecord::parse(cx, x, cx.value(&x.uri)?))))
        .collect();

    // which forum-scope records count, and with which dimensions
    let mut forum_status: BTreeMap<String, ModStatus> = BTreeMap::new();
    let mut counting: Vec<&ModRecord> = Vec::new();
    let mut groups: BTreeMap<(&str, Dim), Vec<Entry<'_>>> = BTreeMap::new();
    for m in records.values() {
        if m.no_effect.is_some() || m.scope != ModScope::Forum {
            continue;
        }
        if let Err(st) = authorise(cx, a, m) {
            forum_status.insert(m.uri.clone(), st);
            continue;
        }
        if !m.cid_ok(cx) {
            forum_status.insert(m.uri.clone(), ModStatus::Stale);
            continue;
        }
        let head_ok = m.head_ok(cx);
        if !head_ok {
            forum_status.insert(m.uri.clone(), ModStatus::Stale);
        } else {
            counting.push(m);
        }
        for (d, v) in &m.asserts {
            if *d == Dim::TagHead && !head_ok {
                continue;
            }
            groups.entry((&m.subject, *d)).or_default().push(Entry {
                rank: rank(a, &m.actor),
                value: v,
                rec: m,
            });
        }
    }

    // resolve(T, d)
    let mut resolved: BTreeMap<String, BTreeMap<Dim, Resolution>> = BTreeMap::new();
    let mut conflicts: BTreeMap<String, u8> = BTreeMap::new();
    for ((subject, d), xs) in &groups {
        let Some(r) = xs.iter().map(|e| e.rank).max() else {
            continue;
        };
        let top: Vec<&Entry<'_>> = xs.iter().filter(|e| e.rank == r).collect();
        let min_uri = |es: &mut dyn Iterator<Item = &&Entry<'_>>| {
            es.map(|e| e.rec.uri.as_str()).min().unwrap_or_default().to_owned()
        };
        let (value, winner) = match d {
            Dim::Restrict => {
                let caps: BTreeSet<Capability> = top
                    .iter()
                    .filter_map(|e| match e.value {
                        DimValue::Caps(c) => Some(c.iter().copied()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                (DimValue::Caps(caps), min_uri(&mut top.iter()))
            }
            Dim::TagHead => {
                if top.iter().any(|e| e.value != top[0].value) {
                    conflicts.insert((*subject).to_owned(), r);
                    continue;
                }
                (top[0].value.clone(), min_uri(&mut top.iter()))
            }
            _ => {
                let Some(v) = top.iter().map(|e| e.value).min_by_key(|v| safety(*d, v)) else {
                    continue;
                };
                let w = min_uri(&mut top.iter().filter(|e| e.value == v));
                (v.clone(), w)
            }
        };
        let allowed = (*d == Dim::Locked && value == DimValue::Bool(true)).then(|| {
            let mut lists = top
                .iter()
                .filter(|e| *e.value == DimValue::Bool(true))
                .map(|e| filter_allowed(cx, subject, &e.rec.allowed));
            let first = lists.next().unwrap_or_default();
            lists.fold(first, |acc, l| acc.intersection(&l).cloned().collect())
        });
        resolved.entry((*subject).to_owned()).or_default().insert(
            *d,
            Resolution {
                value,
                rank: r,
                winner,
                allowed,
            },
        );
    }

    // effective or overridden (§4.12 steps 6–7)
    for m in counting {
        let own = rank(a, &m.actor);
        let mut effective = false;
        let mut best = 0u8;
        for (d, v) in &m.asserts {
            if *d == Dim::Spam {
                continue;
            }
            let Some(res) = resolved.get(&m.subject).and_then(|x| x.get(d)) else {
                continue;
            };
            best = best.max(res.rank);
            if res.rank == own && (*d == Dim::Restrict || res.value == *v) {
                effective = true;
            }
        }
        let st = if effective {
            ModStatus::Effective
        } else {
            ModStatus::Overridden { rank: best }
        };
        forum_status.insert(m.uri.clone(), st);
    }

    let default_tier = match cx.policy.default_tier {
        DefaultTier::Personal => Tier::Personal,
        DefaultTier::Frontpage => Tier::Frontpage,
    };
    Moderation {
        records,
        forum_status,
        resolved,
        conflicts,
        default_tier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safest_values() {
        assert!(safety(Dim::Hidden, &DimValue::Bool(true)) < safety(Dim::Hidden, &DimValue::Bool(false)));
        assert!(
            safety(Dim::PinForum, &DimValue::Bool(false))
                < safety(Dim::PinForum, &DimValue::Bool(true))
        );
        assert!(
            safety(Dim::Access, &DimValue::Access(AccessLevel::Banned))
                < safety(Dim::Access, &DimValue::Access(AccessLevel::Member))
        );
        assert!(
            safety(Dim::Tier, &DimValue::Tier(Tier::Personal))
                < safety(Dim::Tier, &DimValue::Tier(Tier::Curated))
        );
    }
}
