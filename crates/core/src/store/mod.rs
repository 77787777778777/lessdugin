// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `cache` store (`sync.md` §10): key encoding, row encoding, the record store with admission,
//! tombstones and order keys, repo state and host capabilities, over [`crate::host::KvStore`].
//!
//! | Module | Contents |
//! |---|---|
//! | [`key`] | order-preserving tuple key encoding (§10.3) |
//! | [`row`] | canonical DAG-CBOR rows under the shared [`row::ROW_LIMITS`] (§10.5) |
//! | [`schema`] | the key schema, the record scope rule, the cache schema version (§10.4) |
//! | [`strict_json`] | the strict raw-JSON scan run before serde parsing (§5.5) |
//! | [`admit`] | admission checks: shape, JSON → DAG-CBOR → CID, validity (§5.5) |
//! | [`records`] | [`records::RecordStore`], [`records::Batch`]: versions, tombstones, `absent`, `obs` (§5.6) |
//! | [`repo`] | [`repo::RepoRow`] (`repoStatus`, marks, backoff) and [`repo::HostRow`] (§5.3, §5.9) |
//! | [`blob`] | blob metadata and reference counts, cursors, forum access rows |
//! | [`mem`] | [`mem::MemKv`], the in-memory [`crate::host::KvStore`] |

pub mod admit;
pub mod blob;
pub mod key;
pub mod mem;
pub mod records;
pub mod repo;
pub mod row;
pub mod schema;
pub mod strict_json;

pub use admit::{AdmitError, AdmittedRecord, Observation, RecordValidator, Validity};
pub use records::{
    Admission, Batch, Delete, DeleteEvidence, Level, OrderKey, RecordRow, RecordStore, Slot,
    Source, StoreError, StoredRecord, TombRow, Version,
};
pub use repo::{HostRow, RepoRow, RepoStatus};
pub use schema::RecKey;
