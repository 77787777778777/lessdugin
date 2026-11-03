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
//!
//! A relay entry names a repo with some membership record, not one of this forum (`sync.md`
//! §4.4): a relay-only candidate becomes a disc row only once confirmed. Its rejection is a
//! `("disc.relay", fk, did)` row instead, which keeps the back-off and is no discovery row, so
//! the relay's whole membership directory never becomes repos that are followed or put in a
//! Jetstream filter. A forum keeps at most [`DISC_ROWS_MAX`] rows of each kind.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::Forum;
use serde::{Deserialize, Serialize};

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::roster::{self, Roster, RosterRecord};
use crate::dto::Completeness;
use crate::host::{Dir, KeyRange, KvOp, KvStore};
use crate::store::key::{self, Part};
use crate::store::records::{RecordStore, below_cap_boundary, eligible_in_forum, fold_rows_capped};
use crate::store::repo::{RepoRow, RepoStatus};
use crate::store::row::{self, ROW_VERSION};
use crate::store::{StoreError, schema};

/// The key tag of discovery rows.
pub const DISC: &str = "disc";

/// The key tag of relay-only candidates checked and rejected (`sync.md` §4.4, §10.4).
pub const DISC_RELAY: &str = "disc.relay";

/// Rows of each kind kept per forum: past it, the oldest rows that are not confirmed are
/// dropped (`sync.md` §4.4, §6.4, *tunable*).
pub const DISC_ROWS_MAX: usize = 10_000;

/// A discovery candidate's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscState {
    /// Found, not yet confirmed.
    Candidate,
    /// A valid membership record was seen (§4.2.1).
    Confirmed,
    /// The membership record is absent or invalid. A background pass re-checks it only when a
    /// new hint names the DID or after [`REJECTED_RECHECK_MS`]; a user-initiated sync re-checks
    /// up to [`REJECTED_RECHECK_BATCH`] rejected candidates (`sync.md` §4.2.1).
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
    /// Last state change (ms); for a rejected candidate, its last check.
    pub at: i64,
}

/// A background pass confirms a rejected candidate again after this long since its last check
/// without a new hint (the daily reconcile period, `sync.md` §6.8; ms, *tunable*).
pub const REJECTED_RECHECK_MS: i64 = 86_400_000;

/// Rejected candidates a user-initiated `sync_forum` confirms again before their
/// [`REJECTED_RECHECK_MS`] back-off ends, cheapest first (`sync.md` §4.2.1, *tunable*).
pub const REJECTED_RECHECK_BATCH: usize = 50;

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

    /// Records a check that found the membership absent or invalid: the back-off restarts now.
    pub fn reject(&mut self, now_ms: i64) {
        self.state = DiscState::Rejected;
        self.at = now_ms;
    }

    /// `true` if a rejected candidate should be skipped now (no new hint).
    pub fn skip_rejected(&self, now_ms: i64) -> bool {
        self.state == DiscState::Rejected && now_ms.saturating_sub(self.at) < REJECTED_RECHECK_MS
    }
}

/// The key tag of a completed discovery pass.
pub const DISC_COMPLETE: &str = "discComplete";

#[derive(Serialize, Deserialize)]
struct DiscoveryComplete {
    v: u32,
    at_ms: i64,
}

fn disc_complete_key(fk: &str) -> Vec<u8> {
    schema::prefix(DISC_COMPLETE, &[fk])
}

/// Records that a full scoped discovery pass reached its completion calculation.
pub async fn mark_discovery_complete<K: KvStore>(
    kv: &K,
    fk: &str,
    now_ms: i64,
) -> Result<(), StoreError> {
    let row = DiscoveryComplete {
        v: ROW_VERSION,
        at_ms: now_ms,
    };
    kv.write(vec![KvOp::Put(disc_complete_key(fk), row::encode(&row)?)])
        .await?;
    Ok(())
}

/// Whether this cache has completed at least one scoped discovery pass for `fk`.
pub async fn discovery_complete<K: KvStore>(kv: &K, fk: &str) -> Result<bool, StoreError> {
    Ok(kv
        .get_many(vec![disc_complete_key(fk)])
        .await?
        .pop()
        .flatten()
        .and_then(|v| row::decode::<DiscoveryComplete>(&v).ok())
        .is_some_and(|r| r.v == ROW_VERSION && r.at_ms >= 0))
}

/// The key `("disc", fk, did)`.
pub fn disc_key(fk: &str, did: &str) -> Vec<u8> {
    schema::prefix(DISC, &[fk, did])
}

/// The key `("disc.relay", fk, did)`.
pub fn relay_key(fk: &str, did: &str) -> Vec<u8> {
    schema::prefix(DISC_RELAY, &[fk, did])
}

/// Every discovery row of forum `fk`.
pub async fn disc_rows<K: KvStore>(
    kv: &K,
    fk: &str,
) -> Result<BTreeMap<String, DiscRow>, StoreError> {
    scan_rows(kv, DISC, fk).await
}

/// Every relay-only candidate of forum `fk` checked and rejected, by DID.
pub async fn relay_rows<K: KvStore>(
    kv: &K,
    fk: &str,
) -> Result<BTreeMap<String, DiscRow>, StoreError> {
    scan_rows(kv, DISC_RELAY, fk).await
}

async fn scan_rows<K: KvStore>(
    kv: &K,
    tag: &str,
    fk: &str,
) -> Result<BTreeMap<String, DiscRow>, StoreError> {
    let mut out = BTreeMap::new();
    let prefix = schema::prefix(tag, &[fk]);
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

/// The rows to drop so that at most [`DISC_ROWS_MAX`] remain: the oldest rows that are not
/// confirmed (by first-seen time, then DID), never `keep`.
pub fn disc_rows_over_cap(rows: &BTreeMap<String, DiscRow>, keep: &str) -> Vec<String> {
    let Some(excess) = rows.len().checked_sub(DISC_ROWS_MAX).filter(|n| *n > 0) else {
        return Vec::new();
    };
    let mut unconfirmed: Vec<(i64, &String)> = rows
        .iter()
        .filter(|(d, r)| d.as_str() != keep && r.state != DiscState::Confirmed)
        .map(|(d, r)| (r.first, d))
        .collect();
    unconfirmed.sort();
    unconfirmed
        .into_iter()
        .take(excess)
        .map(|(_, d)| d.clone())
        .collect()
}

/// The writes of one discovery pass to forum `fk`, in one atomic write: discovery rows put and
/// dropped, relay rejections put and dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscWrite {
    /// Discovery rows to put.
    pub rows: BTreeMap<String, DiscRow>,
    /// Discovery rows to drop.
    pub dropped: BTreeSet<String>,
    /// Relay rejections to put.
    pub relay: BTreeMap<String, DiscRow>,
    /// Relay rejections to drop.
    pub relay_dropped: BTreeSet<String>,
}

/// Applies `w` to forum `fk` in one atomic write.
pub async fn write_disc<K: KvStore>(kv: &K, fk: &str, w: &DiscWrite) -> Result<(), StoreError> {
    let mut ops = Vec::new();
    for (did, r) in &w.rows {
        ops.push(KvOp::Put(disc_key(fk, did), row::encode(r)?));
    }
    ops.extend(w.dropped.iter().map(|d| KvOp::Delete(disc_key(fk, d))));
    for (did, r) in &w.relay {
        ops.push(KvOp::Put(relay_key(fk, did), row::encode(r)?));
    }
    ops.extend(w.relay_dropped.iter().map(|d| KvOp::Delete(relay_key(fk, d))));
    if !ops.is_empty() {
        kv.write(ops).await?;
    }
    Ok(())
}

/// The forum's stored records of one collection that may enter the fold (`fold_rows_capped`),
/// valid and eligible at `now_ms` (`authorities` for I8's exception, `sync.md` §3.4), by
/// authors that `keep` accepts.
pub async fn forum_records<K: KvStore>(
    kv: &K,
    fk: &str,
    collection: &str,
    keep: impl Fn(&str) -> bool,
    authorities: &BTreeMap<String, u8>,
    now_ms: i64,
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
        .filter(|r| {
            let repo = repos.get(r.key.did.as_str());
            let authority = authorities.contains_key(&r.key.did);
            eligible_in_forum(&r.row, &r.key, repo, authority, now_ms)
                && !below_cap_boundary(fk, &r.key, repo)
        })
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
        let (a, now_ms) = (
            &roster::authorities(&key.owner, decl),
            now_us.div_euclid(1000),
        );
        let all = |_: &str| true;
        let mut recs = forum_records(kv, &key.fk, nsid::MOD_ACTION, all, a, now_ms).await?;
        recs.extend(forum_records(kv, &key.fk, nsid::MEMBERSHIP, all, a, now_ms).await?);
        let roster = Roster::compute(key, decl, &recs, now_us);
        let closure = if roster.is_open() {
            let seeds = |d: &str| roster.seeds.contains(d);
            let mut refs = forum_records(kv, &key.fk, nsid::OPINION, seeds, a, now_ms).await?;
            refs.extend(forum_records(kv, &key.fk, nsid::COMMENT, seeds, a, now_ms).await?);
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

    /// C(F) (§4.5): A ∪ G (approval mode) or A ∪ S plus the confirmed closure (open mode). A
    /// granted DID counts whatever its membership says (`social.md` §3.3: the grant still counts
    /// after its holder leaves), so its repo is synced and counted like an authority's; only
    /// closure members need a confirmed membership.
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
        // a re-check that rejects again restarts the back-off
        r.reject(99);
        assert_eq!((r.state, r.at), (DiscState::Rejected, 99));
        assert!(r.skip_rejected(20 + REJECTED_RECHECK_MS));
        let bytes = row::encode(&r).unwrap();
        assert_eq!(row::decode::<DiscRow>(&bytes).unwrap(), r);
        assert!(disc_key("aaaaaaaaaaaa", "did:x").starts_with(&schema::prefix(DISC, &[])));
        // a relay rejection is no discovery row: a scan of the disc rows never sees it
        let fk = "aaaaaaaaaaaa";
        assert!(!relay_key(fk, "did:x").starts_with(&schema::prefix(DISC, &[fk])));
    }

    #[test]
    fn rows_past_the_cap_drop_the_oldest_unconfirmed() {
        let mut rows = BTreeMap::new();
        for i in 0..DISC_ROWS_MAX {
            let mut r = DiscRow::new("hint:constellation", i as i64);
            if i < 5 {
                r.set(DiscState::Confirmed, 1);
            }
            rows.insert(format!("did:plc:{i:024}"), r);
        }
        assert!(disc_rows_over_cap(&rows, "").is_empty());
        rows.insert("did:plc:new".into(), DiscRow::new("hint:relay", 0));
        rows.insert("did:plc:newer".into(), DiscRow::new("hint:relay", 0));
        // the confirmed rows are never dropped, `keep` neither; then the oldest, then by DID
        let dropped = disc_rows_over_cap(&rows, "did:plc:new");
        assert_eq!(dropped, ["did:plc:newer", &format!("did:plc:{:024}", 5)]);
    }
}
