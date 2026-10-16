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
//! Marking the own repo dirty at P1 and scheduling the P0 fetches belongs to the sync module:
//! the outbox reports every [`AppliedWrite`] to its [`super::OutboxListener`].
// TODO(E3): when `sync` has its own-write helper (dirty queue, P0 point fetches), the engine
// connects it through `OutboxListener::on_applied`, or replaces `apply_own_write` with it.

use lessdugin_atp::syntax::{AtUri, Did};
use serde_json::Value;

use crate::host::KvStore;
use crate::store::admit::{self, LexiconValidator, Observation};
use crate::store::records::{Delete, DeleteEvidence, OrderKey, RecordStore, StoreError};
use crate::store::repo::RepoRow;

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
    let store = RecordStore::new(cache);
    let mut batch = store.batch();
    let mut report = ApplyReport::default();
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
