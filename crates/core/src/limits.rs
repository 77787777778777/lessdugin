// SPDX-License-Identifier: AGPL-3.0-or-later
//! Normative limits (mirrors `docs/spec/limits.md`). Changing a value that feeds the fold
//! (fetch caps) changes behaviour and needs a new behaviour ID.

/// Default cap on an HTTP response body (bytes, after decompression).
pub const DEFAULT_RESPONSE_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Cap on a single record's JSON (bytes).
pub const RECORD_JSON_MAX_BYTES: u64 = 1_000_000;
/// Cap on a `listRecords` page (bytes).
pub const LIST_PAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Cap on a proof CAR (bytes).
pub const PROOF_CAR_MAX_BYTES: u64 = 1_000_000;
/// Cap on an indexer proof bundle (bytes).
pub const INDEXER_BUNDLE_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Images up to this size may load automatically (bytes).
pub const BLOB_AUTOLOAD_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Hard cap on any blob (bytes).
pub const BLOB_HARD_MAX_BYTES: u64 = 300 * 1024 * 1024;
/// Jetstream message cap (bytes).
pub const JETSTREAM_MESSAGE_MAX_BYTES: u32 = 1_200_000;

/// Connect timeout (ms).
pub const CONNECT_TIMEOUT_MS: u32 = 5_000;
/// Request timeout (ms).
pub const REQUEST_TIMEOUT_MS: u32 = 20_000;
/// CAR download timeout (ms).
pub const CAR_TIMEOUT_MS: u32 = 60_000;
/// Blob downloads fail after this long without progress (ms).
pub const BLOB_IDLE_TIMEOUT_MS: u32 = 20_000;
/// Total timeout of a blob download (ms).
pub const BLOB_TOTAL_TIMEOUT_MS: u32 = 300_000;
/// Hard cap on redirects followed by one anonymous request (`threat-model.md` §5.6.2).
pub const MAX_REDIRECTS: u8 = 5;
/// Redirects followed by identity lookups (`/.well-known/atproto-did`, did:web documents).
pub const IDENTITY_REDIRECTS: u8 = 3;

/// Normative fetch cap per (repo, forum, collection): the first N keys in descending rkey order
/// of the `{fk}-` range. Beyond it the view is `[partial]` (`data-model.md` §7.4).
pub const FETCH_CAP_DEFAULT: u32 = 20_000;
/// Normative fetch cap for `opinion` (see [`FETCH_CAP_DEFAULT`]).
pub const FETCH_CAP_OPINION: u32 = 100_000;

/// Maximum future skew accepted for revs and timestamps before items count as future-dated.
pub const FUTURE_SKEW_MS: i64 = 5 * 60 * 1000;

/// Requests in flight (native).
pub const INFLIGHT_NATIVE: usize = 32;
/// Requests in flight (browser).
pub const INFLIGHT_BROWSER: usize = 16;
/// Requests in flight per host.
pub const INFLIGHT_PER_HOST: usize = 6;
/// PLC requests in flight.
pub const INFLIGHT_PLC: usize = 8;
/// DoH requests in flight.
pub const INFLIGHT_DOH: usize = 8;

/// Cap on a DID document (bytes, `sync.md` §3.2).
pub const DID_DOC_MAX_BYTES: u64 = 65_536;
/// Cap on `/.well-known/atproto-did` (bytes, `sync.md` §3.1).
pub const WELLKNOWN_DID_MAX_BYTES: u64 = 2_048;
/// Cap on a DNS-over-HTTPS JSON response (bytes).
pub const DOH_RESPONSE_MAX_BYTES: u64 = 65_536;
/// Cap on `/.well-known/oauth-protected-resource` (bytes, `sync.md` §3.6).
pub const OAUTH_METADATA_MAX_BYTES: u64 = 65_536;
/// Cap on small XRPC JSON responses (`resolveHandle`, `getLatestCommit`, mini-docs, errors).
pub const SMALL_RESPONSE_MAX_BYTES: u64 = 65_536;
/// Cap on a `com.atproto.repo.getRecord` response: one record's JSON plus the envelope.
pub const RECORD_RESPONSE_MAX_BYTES: u64 = RECORD_JSON_MAX_BYTES + 16 * 1024;

/// GET retries on network errors, timeouts, 5xx and 429 (`sync.md` §9.3, *tunable*).
pub const RETRY_MAX: u32 = 3;
/// Base of the full-jitter exponential backoff (ms).
pub const RETRY_BASE_MS: u64 = 250;
/// Cap of the full-jitter exponential backoff (ms).
pub const RETRY_CAP_MS: u64 = 8_000;
/// The transport waits at most this long for a `Retry-After` inside one call; a longer wait is
/// returned to the caller as `RateLimited` (the scheduler owns longer pauses).
pub const RETRY_AFTER_MAX_WAIT_MS: u64 = 60_000;

/// Nesting depth of a record value, in JSON or DAG-CBOR (`limits.md` §6, rule G10). Deeper: the
/// record is stored as an invalid version.
pub const NESTING_MAX_RECORD: usize = 32;
/// Nesting depth of any JSON response document or DAG-CBOR block (`limits.md` §6). Deeper: the
/// response is rejected whole (R1).
pub const NESTING_MAX_DOCUMENT: usize = 36;
/// DAG-CBOR size of a record value (`limits.md` §4, rule G6). Larger: the record is invalid.
pub const RECORD_CBOR_MAX_BYTES: usize = 1_000_000;
/// Tombstones are kept at least this long (`limits.md` §15, I4).
pub const TOMBSTONE_MIN_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Dirty marks kept per repo (`sync.md` §6.5); beyond it `marksOverflow` is set.
pub const REPO_MARKS_MAX: usize = 1_000;

// ---------------------------------------------------------------------------------------------
// Write path (`limits.md` §4, §14; `data-model.md` §5.3, §8)

/// A spillable `md` field whose UTF-8 source is longer is written as `textBlob` (W).
pub const SPILL_THRESHOLD_BYTES: usize = 65_536;
/// The exact JSON request body of `createRecord`, `putRecord` or `applyWrites` (W).
pub const WRITE_BODY_MAX_BYTES: usize = 900_000;
/// Opinion intents are held this long so they can be undone or coalesced (ms).
pub const OPINION_HOLD_MS: i64 = 4_000;
/// Attempts of an edit, opinion write, modAction removal or `mod adopt` batch on `InvalidSwap`.
pub const SWAP_RETRY_MAX: u32 = 3;
/// Lower bound of the delay before such a retry (ms).
pub const SWAP_RETRY_JITTER_MIN_MS: u64 = 200;
/// Upper bound of the delay before such a retry (ms).
pub const SWAP_RETRY_JITTER_MAX_MS: u64 = 800;
/// Ops per `applyWrites` (delete chunks, redaction, `mod adopt`).
pub const APPLYWRITES_MAX_OPS: usize = 200;
/// Write budget per account and hour (points).
pub const WRITE_BUDGET_POINTS_PER_HOUR: u32 = 5_000;
/// Write budget per account and day (points).
pub const WRITE_BUDGET_POINTS_PER_DAY: u32 = 35_000;
/// Points of a create op.
pub const WRITE_COST_CREATE: u32 = 3;
/// Points of an update op.
pub const WRITE_COST_UPDATE: u32 = 2;
/// Points of a delete op.
pub const WRITE_COST_DELETE: u32 = 1;
/// `#lock.allowed` entries; a longer computed allow-list is refused.
pub const LOCK_ALLOWED_MAX: usize = 5_000;
