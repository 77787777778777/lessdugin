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
