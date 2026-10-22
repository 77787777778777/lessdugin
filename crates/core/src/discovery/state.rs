// SPDX-License-Identifier: AGPL-3.0-or-later
//! Discovery state read from the cache: discovery rows (`("disc", fk, did)`, `sync.md` §10.4),
//! the roster and closure over the stored records, the completeness set C(F) and completeness
//! (§4.5).
//!
//! ```text
//! C(F) = approval: A ∪ G                       ∖ rejected candidates
//!        open:     A ∪ S ∪ ConfirmedClosure     ∖ rejected candidates
//! complete(d) ⇔ repoStatus(d, fk) = Fresh (no byte cap in the last listing)
//! degraded    ⇔ some authority is Unavailable
//! ```
//!
//! A candidate whose membership was checked and found absent or invalid (a `rejected` disc row)
//! is left out of C(F): it has nothing to sync for the forum, and counting it would keep the
//! forum `[partial]` for ever. (`sync.md` §4.5 lists G in C(F) without saying what happens to a
//! granted DID that has no membership record; this is the reading taken here.)

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::Forum;
use serde::{Deserialize, Serialize};

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::roster::{Roster, RosterRecord};
use crate::dto::Completeness;
use crate::host::{Dir, KeyRange, KvOp, KvStore};
use crate::store::key::{self, Part};
use crate::store::records::{RecordStore, eligible, fold_rows_capped};
use crate::store::repo::{RepoRow, RepoStatus};
use crate::store::row::{self, ROW_VERSION};
use crate::store::{StoreError, schema};

/// The key tag of discovery rows.
pub const DISC: &str = "disc";

/// A discovery candidate's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscState {
    /// Found, not yet confirmed.
    Candidate,
    /// A valid membership record was seen (§4.2.1).
    Confirmed,
    /// The membership record is absent or invalid; re-checked only when a new hint names the
    /// DID (or after [`REJECTED_RECHECK_MS`]).
    Rejected,
}

/// `DiscRow` (`sync.md` §10.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscRow {
    /// Row version.
    pub v: u32,
    /// Where the candidate came from (`grant`, `closure`, `view`, `hint:<source>`), sorted.
    pub src: Vec<String>,
    /// First seen (ms).
    pub first: i64,
    /// State.
    pub state: DiscState,
    /// Last state change (ms).
    pub at: i64,
}

/// A rejected candidate is confirmed again after this long without a new hint (the daily
/// reconcile period, `sync.md` §6.8).
pub const REJECTED_RECHECK_MS: i64 = 86_400_000;

impl DiscRow {
    /// A new candidate row.
    pub fn new(src: &str, now_ms: i64) -> Self {
        Self {
            v: ROW_VERSION,
            src: vec![src.to_owned()],
            first: now_ms,
            state: DiscState::Candidate,
            at: now_ms,
        }
    }

    /// Adds a source.
    pub fn add_src(&mut self, src: &str) {
        if !self.src.iter().any(|s| s == src) {
            self.src.push(src.to_owned());
            self.src.sort();
        }
    }

    /// Sets the state.
    pub fn set(&mut self, state: DiscState, now_ms: i64) {
        if self.state != state {
            self.state = state;
            self.at = now_ms;
        }
    }

    /// `true` if a rejected candidate should be skipped now (no new hint).
    pub fn skip_rejected(&self, now_ms: i64) -> bool {
        self.state == DiscState::Rejected && now_ms.saturating_sub(self.at) < REJECTED_RECHECK_MS
    }
}

/// The key `("disc", fk, did)`.
pub fn disc_key(fk: &str, did: &str) -> Vec<u8> {
    schema::prefix(DISC, &[fk, did])
}

/// Every discovery row of forum `fk`.
pub async fn disc_rows<K: KvStore>(
    kv: &K,
    fk: &str,
) -> Result<BTreeMap<String, DiscRow>, StoreError> {
    let mut out = BTreeMap::new();
    let prefix = schema::prefix(DISC, &[fk]);
    let mut range = Some(KeyRange::prefix(&prefix));
    while let Some(r) = range.take() {
        let page = kv.scan(r.clone(), Dir::Forward, 1_000).await?;
        let full = page.len() == 1_000;
        for (k, v) in &page {
            let Ok(parts) = key::decode(k) else {
                continue;
            };
            let Some(Part::Bytes(did)) = parts.get(2) else {
                continue;
            };
            let Ok(did) = std::str::from_utf8(did) else {
                continue;
            };
            if let Ok(row) = row::decode::<DiscRow>(v)
                && row.v == ROW_VERSION
            {
                out.insert(did.to_owned(), row);
            }
        }
        if full && let Some((last, _)) = page.last() {
            range = Some(KeyRange {
                start: std::ops::Bound::Excluded(last.clone()),
                end: r.end,
            });
        }
    }
    Ok(out)
}

/// Writes discovery rows of forum `fk` in one atomic write.
pub async fn put_disc_rows<K: KvStore>(
    kv: &K,
    fk: &str,
    rows: &BTreeMap<String, DiscRow>,
) -> Result<(), StoreError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut ops = Vec::with_capacity(rows.len());
    for (did, r) in rows {
        ops.push(KvOp::Put(disc_key(fk, did), row::encode(r)?));
    }
    kv.write(ops).await?;
    Ok(())
}

/// The forum's stored records of one collection that may enter the fold (`fold_rows_capped`),
/// valid and eligible, by authors that `keep` accepts.
pub async fn forum_records<K: KvStore>(
    kv: &K,
    fk: &str,
    collection: &str,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<RosterRecord>, StoreError> {
    let store = RecordStore::new(kv);
    let rows = store.scan_forum(fk, Some(collection)).collect().await?;
    let rows: Vec<_> = fold_rows_capped(fk, rows)
        .into_iter()
        .filter(|r| r.row.valid && keep(&r.key.did))
        .collect();
    let dids: BTreeSet<&str> = rows.iter().map(|r| r.key.did.as_str()).collect();
    let dids: Vec<&str> = dids.into_iter().collect();
    let repos: BTreeMap<&str, RepoRow> = dids
        .iter()
        .copied()
        .zip(store.repos(&dids).await?)
        .filter_map(|(d, r)| Some((d, r?)))
        .collect();
    Ok(rows
        .iter()
        .filter(|r| eligible(&r.row, repos.get(r.key.did.as_str())))
        .filter_map(|r| {
            Some(RosterRecord {
                did: r.key.did.clone(),
                collection: r.key.collection.clone(),
                rkey: r.key.rkey.clone(),
                value: r.row.value_json().ok()?,
            })
        })
        .collect())
}

/// What the cache says about a forum's participants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForumState {
    /// A, G, B, N, S and the members.
    pub roster: Roster,
    /// The closure (§4.3; computed in open mode only).
    pub closure: BTreeSet<String>,
    /// Discovery rows by DID.
    pub disc: BTreeMap<String, DiscRow>,
}

impl ForumState {
    /// Reads the roster, closure and discovery rows of a forum at `now_us`.
    pub async fn load<K: KvStore>(
        kv: &K,
        key: &ForumKey,
        decl: &Forum,
        now_us: i64,
    ) -> Result<Self, StoreError> {
        let mut recs = forum_records(kv, &key.fk, nsid::MOD_ACTION, |_| true).await?;
        recs.extend(forum_records(kv, &key.fk, nsid::MEMBERSHIP, |_| true).await?);
        let roster = Roster::compute(key, decl, &recs, now_us);
        let closure = if roster.is_open() {
            let seeds = &roster.seeds;
            let mut refs = forum_records(kv, &key.fk, nsid::OPINION, |d| seeds.contains(d)).await?;
            refs.extend(forum_records(kv, &key.fk, nsid::COMMENT, |d| seeds.contains(d)).await?);
            roster.closure(key, &refs)
        } else {
            BTreeSet::new()
        };
        Ok(Self {
            roster,
            closure,
            disc: disc_rows(kv, &key.fk).await?,
        })
    }

    fn rejected(&self, d: &str) -> bool {
        !self.roster.authorities.contains_key(d)
            && self
                .disc
                .get(d)
                .is_some_and(|r| r.state == DiscState::Rejected)
    }

    /// C(F) (§4.5), without rejected candidates.
    pub fn completeness_set(&self) -> BTreeSet<String> {
        let r = &self.roster;
        let mut set: BTreeSet<String> = r.authorities.keys().cloned().collect();
        if r.is_open() {
            set.extend(r.seeds.iter().cloned());
            set.extend(
                self.closure
                    .iter()
                    .filter(|d| {
                        self.disc
                            .get(*d)
                            .is_some_and(|x| x.state == DiscState::Confirmed)
                    })
                    .cloned(),
            );
        } else {
            set.extend(r.granted.iter().cloned());
        }
        set.retain(|d| !self.rejected(d));
        set
    }
}

/// Repo states over a set of DIDs for one forum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusCounts {
    /// `Fresh`.
    pub fresh: BTreeSet<String>,
    /// `Stale`.
    pub stale: BTreeSet<String>,
    /// `Unavailable`.
    pub unavailable: BTreeSet<String>,
}

/// The repo status (§5.9) of each DID in `dids` for scope `fk`.
pub async fn status_counts<K: KvStore>(
    kv: &K,
    fk: &str,
    dids: &BTreeSet<String>,
) -> Result<StatusCounts, StoreError> {
    let list: Vec<&str> = dids.iter().map(String::as_str).collect();
    let repos = RecordStore::new(kv).repos(&list).await?;
    let mut out = StatusCounts::default();
    for (d, r) in list.iter().zip(repos) {
        let st = r.map_or(RepoStatus::Unavailable, |r| r.repo_status(fk));
        let set = match st {
            RepoStatus::Fresh => &mut out.fresh,
            RepoStatus::Stale => &mut out.stale,
            RepoStatus::Unavailable => &mut out.unavailable,
        };
        set.insert((*d).to_owned());
    }
    Ok(out)
}

/// Completeness of forum `key` (§4.5, §4.6) from the cache.
pub async fn completeness<K: KvStore>(
    kv: &K,
    key: &ForumKey,
    state: &ForumState,
) -> Result<(Completeness, StatusCounts), StoreError> {
    let set = state.completeness_set();
    let counts = status_counts(kv, &key.fk, &set).await?;
    let degraded = state
        .roster
        .authorities
        .keys()
        .any(|a| counts.unavailable.contains(a));
    let total = set.len() as u32;
    let complete = counts.fresh.len() as u32;
    Ok((
        Completeness {
            partial: complete < total,
            repos_complete: complete,
            repos_total: total,
            degraded,
        },
        counts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disc_rows_track_state() {
        let mut r = DiscRow::new("closure", 10);
        r.add_src("grant");
        r.add_src("closure");
        assert_eq!(r.src, ["closure", "grant"]);
        r.set(DiscState::Rejected, 20);
        assert!(r.skip_rejected(20 + REJECTED_RECHECK_MS - 1));
        assert!(!r.skip_rejected(20 + REJECTED_RECHECK_MS));
        r.set(DiscState::Rejected, 99);
        assert_eq!(r.at, 20);
        let bytes = row::encode(&r).unwrap();
        assert_eq!(row::decode::<DiscRow>(&bytes).unwrap(), r);
        assert!(disc_key("aaaaaaaaaaaa", "did:x").starts_with(&schema::prefix(DISC, &[])));
    }
}
