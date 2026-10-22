// SPDX-License-Identifier: AGPL-3.0-or-later
//! Applying confirmed own writes to the record store (`sync.md` §5.8).
//!
//! When the PDS confirms a write:
//! - each created or updated record is admitted with the exact value that was sent, at level
//!   `pds`, source `own`, if the CID of its DAG-CBOR encoding equals the returned CID; otherwise
//!   it is not stored and the uri needs a P0 point fetch ([`ApplyReport::mismatched`]);
//! - each deleted record gets a tombstone;
//! - the order key is `(commit.rev, sentAt)`; a write without `commit` (a no-op `putRecord`)
//!   uses the head from a `getLatestCommit` the caller made;
//! - `repo.headSeen` is updated (`repo.rev` and `repo.listed` are not), and a write into a
//!   collection not in `repo.colls` sets `repo.collsDirty`.
//!
//! An outbox built with a [`Syncer`] ([`super::Outbox::with_syncer`]) applies through the sync
//! module's helper ([`apply_with_syncer`]: [`Syncer::apply_own_writes`]), which also takes the
//! in-process repo lock, marks the own repo dirty at P1 and queues P0 point fetches of the
//! mismatched uris. Without one (a standalone outbox, tests) [`apply_own_write`] writes the store
//! directly and the [`super::OutboxListener`] is told what is left to schedule.

use std::sync::Arc;

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::xrpc::CommitMeta;
use serde_json::Value;

use crate::engine::EngineError;
use crate::host::{Host, KvStore};
use crate::store::admit::{self, LexiconValidator, Observation};
use crate::store::records::{Delete, DeleteEvidence, OrderKey, RecordStore, StoreError};
use crate::store::repo::RepoRow;
use crate::sync::queue::{DirtyItem, Reason, ready_time};
use crate::sync::{OwnWrite, Priority, Stats, Syncer};

/// One record of a confirmed write.
#[derive(Clone, Debug, PartialEq)]
pub enum AppliedRecord {
    /// Created or updated with `value`; the PDS returned `cid`.
    Put {
        /// The record uri.
        uri: String,
        /// The CID the PDS returned.
        cid: String,
        /// The exact value sent.
        value: Value,
    },
    /// Deleted.
    Delete {
        /// The record uri.
        uri: String,
    },
}

impl AppliedRecord {
    /// The uri.
    pub fn uri(&self) -> &str {
        match self {
            Self::Put { uri, .. } | Self::Delete { uri } => uri,
        }
    }
}

/// A write request the PDS confirmed.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedWrite {
    /// The account (repo) DID.
    pub account: Did,
    /// The PDS it was written to.
    pub pds: String,
    /// The commit rev (or the head rev read afterwards when the answer had no commit).
    pub rev: String,
    /// The commit CID, when known.
    pub commit: Option<String>,
    /// `rev` is this write's own commit rev. `false` when it is the rev of a head read
    /// afterwards (a no-op write, a record found by a re-read): only a lower bound, since
    /// another device may have changed the record in between, so it is stored as not exact.
    pub exact: bool,
    /// When the write request was sent (ms).
    pub sent_at: i64,
    /// The records.
    pub records: Vec<AppliedRecord>,
}

/// What [`apply_own_write`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Uris whose stored state changed.
    pub changed: Vec<String>,
    /// Uris whose sent value did not hash to the returned CID: not stored; they need a P0
    /// point fetch.
    pub mismatched: Vec<String>,
}

/// Applies a confirmed own write to the record store in `cache` (one atomic batch).
pub async fn apply_own_write<K: KvStore>(
    cache: &K,
    w: &AppliedWrite,
    now_ms: i64,
) -> Result<ApplyReport, StoreError> {
    let mut report = ApplyReport::default();
    if w.rev.is_empty() {
        // No commit and no head read afterwards: there is no order key to admit under, so the
        // uris are left to a point fetch.
        report.mismatched = w.records.iter().map(|r| r.uri().to_owned()).collect();
        return Ok(report);
    }
    let store = RecordStore::new(cache);
    let mut batch = store.batch();
    let mut colls: Vec<String> = Vec::new();
    for r in &w.records {
        let Ok(uri) = AtUri::parse(r.uri()) else {
            report.mismatched.push(r.uri().to_owned());
            continue;
        };
        colls.push(uri.collection().as_str().to_owned());
        match r {
            AppliedRecord::Put { cid, value, .. } => {
                match admit::admit_own_write(&uri, value, cid, &LexiconValidator) {
                    Ok(a) => {
                        let obs = Observation::own_write(&w.rev, w.sent_at, now_ms);
                        let obs = if w.exact { obs } else { obs.inexact() };
                        batch.put_version(a.into_version(obs)).await?;
                    }
                    Err(e) => {
                        tracing::warn!(uri = %uri, error = %e, "own write does not match its CID");
                        report.mismatched.push(uri.as_str().to_owned());
                    }
                }
            }
            AppliedRecord::Delete { .. } => {
                let d = Delete {
                    key: OrderKey::new(w.rev.clone(), w.sent_at),
                    evidence: DeleteEvidence::OwnDelete,
                    at_ms: now_ms,
                };
                batch.tombstone(&uri, d).await?;
            }
        }
    }
    let did = w.account.as_str();
    let mut repo = batch
        .repo(did)
        .await?
        .unwrap_or_else(|| RepoRow::new(&w.pds));
    if let Some(cid) = w
        .commit
        .as_deref()
        .and_then(|c| lessdugin_atp::Cid::try_from(c).ok())
    {
        repo.observe_head(&w.rev, cid, w.sent_at);
    }
    if !repo.colls.is_empty() && colls.iter().any(|c| !repo.colls.contains(c)) {
        repo.colls_dirty = true;
    }
    if !batch.changed().is_empty() {
        repo.last_change_at = now_ms;
    }
    batch.put_repo(did, &repo)?;
    let committed = batch.commit().await?;
    report.changed = committed
        .changed
        .into_iter()
        .filter(|u| w.records.iter().any(|r| r.uri() == u))
        .collect();
    Ok(report)
}

/// Applies a confirmed own write through the sync module (`sync.md` §5.8,
/// [`Syncer::apply_own_writes`]): admission at level `pds`, source `own`, tombstones, `headSeen`,
/// `collsDirty`, the P1 dirty mark of the own repo and P0 point fetches of mismatched uris, all
/// under the repo lock. If that fails (say the own DID document cannot be resolved right now),
/// the write is applied directly under the same lock and the dirty marks are queued here.
pub async fn apply_with_syncer<H: Host>(
    syncer: &Syncer<H>,
    w: &AppliedWrite,
) -> Result<ApplyReport, EngineError> {
    let s = syncer.session(Priority::P0, Arc::new(Stats::new()));
    let commit = w
        .commit
        .as_ref()
        // a rev read afterwards is not passed: the syncer reads the head itself and stores the
        // records as not exact
        .filter(|_| !w.rev.is_empty() && w.exact)
        .map(|cid| CommitMeta {
            cid: cid.clone(),
            rev: w.rev.clone(),
        });
    let writes: Vec<OwnWrite> = w
        .records
        .iter()
        .filter_map(|r| {
            let uri = AtUri::parse(r.uri()).ok()?;
            Some(match r {
                AppliedRecord::Put { cid, value, .. } => OwnWrite {
                    uri,
                    value: Some(value.clone()),
                    cid: Some(cid.clone()).filter(|c| !c.is_empty()),
                },
                AppliedRecord::Delete { .. } => OwnWrite {
                    uri,
                    value: None,
                    cid: None,
                },
            })
        })
        .collect();
    match syncer
        .apply_own_writes(&s, &w.account, w.sent_at, commit.as_ref(), &writes)
        .await
    {
        Ok(r) => {
            let mut changed = r.stored;
            changed.extend(r.tombstoned);
            Ok(ApplyReport {
                changed,
                mismatched: r.refetch,
            })
        }
        Err(e) => {
            tracing::warn!(account = %w.account, error = %e, "own write: sync path failed; applying directly");
            let now = syncer.host().now_ms();
            let report = {
                let _guard = syncer.net().repo_lock(&w.account).await;
                apply_own_write(syncer.host().cache(), w, now)
                    .await
                    .map_err(|e| EngineError::internal(format!("applying an own write: {e}")))?
            };
            let did = w.account.as_str();
            for u in &report.mismatched {
                let at = ready_time(Priority::P0, now, random(syncer.host()), None);
                syncer.enqueue(DirtyItem::point(did, u, Priority::P0, at));
            }
            let at = ready_time(Priority::P1, now, random(syncer.host()), None);
            let mut item = DirtyItem::new(did, Priority::P1, Reason::OwnWrite, at);
            if !w.rev.is_empty() {
                item = item.with_min_rev(w.rev.clone());
            }
            syncer.enqueue(item);
            Ok(report)
        }
    }
}

fn random<H: Host>(host: &H) -> u64 {
    let mut b = [0u8; 8];
    host.fill_random(&mut b);
    u64::from_le_bytes(b)
}
