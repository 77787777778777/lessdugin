// SPDX-License-Identifier: AGPL-3.0-or-later
//! Range listing and the cursor-range probe (`sync.md` §5.3, ADR 0004), with the normative fetch
//! caps (§5.4) and the listing safety caps (§5.10, `limits.md` §10.1).
//!
//! ```text
//! list_range(did, c, fk):            N = 100,000 (opinion) or 20,000; P = fk + "-"; B = fk + "."
//!   host.rangeCursor = No, set < 7 days ago → FULL
//!   RANGE  cursor = B, limit 100; every page is checked:
//!            first page HTTP 400 → No, FULL
//!            a key ≥ the request cursor, keys not strictly descending, a first key ≥ the previous
//!            page's last, a cursor that is not the last key → No, FULL (rows discarded)
//!          keep keys starting with P, stop at the first that does not (complete), at the N-th
//!          (complete, capped), at an empty page, a missing cursor or a short page (complete);
//!          stop past RANGE_MAX bytes (incomplete, byte-capped)
//!   FULL   whole collection, limit 100, prefix filter; a repeated cursor, > 5,000 pages or
//!          > 200 MiB → incomplete; two entries with one rkey → error; sort descending, first N
//! pages: > 4 MiB → the same cursor again with limit 10, then 1; still too large → incomplete
//! ```
//!
//! Every key counts toward N, including keys of records that fail admission, so the cap boundary
//! depends only on the repo's key set. A listing returns every key it saw within the cap (the
//! tombstone step must not delete records that exist but were dropped at admission).

use std::collections::BTreeSet;

use lessdugin_atp::syntax::{AtIdentifier, Did, Nsid};
use lessdugin_atp::xrpc::ListRecords;

use crate::host::Host;
use crate::limits;
use crate::store::admit::{self, AdmitError, AdmittedRecord, Expect, ListPage, RecordValidator};
use crate::store::repo::HostRow;
use crate::store::schema;
use crate::sync::net::{self, NetHost};
use crate::xrpc::{CallOpts, Service, Xrpc, XrpcCallError};

/// `limit` of every `listRecords` call (`LIST_RECORDS_LIMIT`).
pub const LIST_RECORDS_LIMIT: u32 = 100;
/// Smaller `limit`s tried after an oversized page (`LIST_PAGE_RETRY_LIMITS`).
pub const LIST_PAGE_RETRY_LIMITS: [u32; 2] = [10, 1];
/// Bytes of one range listing in `RANGE` mode (`RANGE_MAX_BYTES`).
pub const RANGE_MAX_BYTES: u64 = 50 * 1024 * 1024;
/// The same for `opinion` (`RANGE_MAX_BYTES_OPINION`).
pub const RANGE_MAX_BYTES_OPINION: u64 = 100 * 1024 * 1024;
/// Bytes of one full-collection listing (`FULL_LIST_MAX_BYTES`).
pub const FULL_LIST_MAX_BYTES: u64 = 200 * 1024 * 1024;
/// Pages of one full-collection listing (`FULL_LIST_MAX_PAGES`).
pub const FULL_LIST_MAX_PAGES: u32 = 5_000;

/// Listing parameters. [`ListTuning::default`] is the normative configuration; the fields exist
/// so that tests can exercise caps with small repos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListTuning {
    /// Replaces the normative fetch cap N of every collection (tests only).
    pub fetch_cap: Option<u32>,
    /// `limit` of the first request for each page.
    pub limit: u32,
    /// Range byte cap (`None`: [`RANGE_MAX_BYTES`] / [`RANGE_MAX_BYTES_OPINION`]).
    pub range_max_bytes: Option<u64>,
    /// Full-listing byte cap.
    pub full_max_bytes: u64,
    /// Full-listing page cap.
    pub full_max_pages: u32,
    /// Page byte cap.
    pub page_max_bytes: u64,
}

impl Default for ListTuning {
    fn default() -> Self {
        Self {
            fetch_cap: None,
            limit: LIST_RECORDS_LIMIT,
            range_max_bytes: None,
            full_max_bytes: FULL_LIST_MAX_BYTES,
            full_max_pages: FULL_LIST_MAX_PAGES,
            page_max_bytes: limits::LIST_PAGE_MAX_BYTES,
        }
    }
}

impl ListTuning {
    /// N for `coll` (§5.4).
    pub fn cap(&self, coll: &str) -> usize {
        self.fetch_cap.unwrap_or_else(|| schema::fetch_cap(coll)) as usize
    }

    /// The range byte cap for `coll`.
    pub fn range_max(&self, coll: &str) -> u64 {
        self.range_max_bytes.unwrap_or_else(|| {
            if schema::fetch_cap(coll) == limits::FETCH_CAP_OPINION {
                RANGE_MAX_BYTES_OPINION
            } else {
                RANGE_MAX_BYTES
            }
        })
    }
}

/// How a listing was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    /// Cursor-bounded range listing.
    Range,
    /// The whole collection, filtered by prefix.
    Full,
}

/// What a listing taught about the host's cursors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeVerdict {
    /// The range trick worked (`set Yes`).
    Yes,
    /// A violation or a 400 on the probe (`set No`).
    No,
}

/// Applies listing verdicts to a host row: any `No` wins; `Yes` only from `Unknown` (§5.3).
pub fn apply_verdicts(host: &mut HostRow, verdicts: &[RangeVerdict], now_ms: i64) {
    if verdicts.contains(&RangeVerdict::No) {
        host.set_range_no(now_ms);
    } else if verdicts.contains(&RangeVerdict::Yes) {
        host.set_range_yes(now_ms);
    }
}

/// One listed entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    /// The record key.
    pub rkey: String,
    /// The admission result (a failure is a strike against the host; the key still exists).
    pub record: Result<AdmittedRecord, AdmitError>,
    /// The send time of the page's request (ms).
    pub sent_at: i64,
}

/// The result of one listing.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeListing {
    /// In-range entries within the fetch cap, in descending rkey order.
    pub entries: Vec<Listed>,
    /// The listing reached the end of the range (or the cap).
    pub complete: bool,
    /// N keys were reached: the range is capped at the smallest listed key.
    pub capped: bool,
    /// A byte or page safety cap stopped the listing (no completeness claim).
    pub byte_capped: bool,
    /// How the listing was made.
    pub mode: ListMode,
    /// The send time of the first page (ms): the key of tombstones of a complete pass.
    pub first_sent_at: Option<i64>,
    /// What the listing taught about the host's cursors.
    pub verdict: Option<RangeVerdict>,
    /// Entries that failed admission, and pages that broke a listing rule (strikes, `limits.md`
    /// R4).
    pub strikes: u32,
    /// Why an incomplete listing stopped (a safety cap, a cursor that did not advance).
    pub stop: Option<String>,
}

impl RangeListing {
    fn new(mode: ListMode) -> Self {
        Self {
            entries: Vec::new(),
            complete: false,
            capped: false,
            byte_capped: false,
            mode,
            first_sent_at: None,
            verdict: None,
            strikes: 0,
            stop: None,
        }
    }

    fn stopped(&mut self, why: String) {
        self.byte_capped = true;
        self.stop = Some(why);
    }

    /// Every listed rkey.
    pub fn keys(&self) -> BTreeSet<String> {
        self.entries.iter().map(|e| e.rkey.clone()).collect()
    }

    /// The cap boundary (the smallest listed rkey) of a capped listing.
    pub fn boundary(&self) -> Option<&str> {
        if self.capped {
            self.entries.last().map(|e| e.rkey.as_str())
        } else {
            None
        }
    }

    /// The entries that passed admission.
    pub fn admitted(&self) -> impl Iterator<Item = (&AdmittedRecord, i64)> {
        self.entries
            .iter()
            .filter_map(|e| e.record.as_ref().ok().map(|r| (r, e.sent_at)))
    }
}

/// Why a listing failed.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ListError {
    /// A request failed (after the transport's retries).
    #[error(transparent)]
    Xrpc(#[from] XrpcCallError),
    /// A page was malformed or named records outside the repo and collection.
    #[error("bad listRecords page: {0}")]
    BadPage(String),
    /// A full listing returned one rkey twice (an inconsistent listing).
    #[error("inconsistent listing: rkey {0:?} returned twice")]
    Duplicate(String),
    /// A listing stopped at a safety cap or at a cursor that did not advance (`sync.md` §5.10):
    /// its range stays unlisted.
    #[error("listing of {collection} stopped: {why}")]
    Stopped {
        /// The collection.
        collection: String,
        /// Why.
        why: String,
    },
}

/// One fetched page.
struct Page {
    page: ListPage,
    rkeys: Vec<String>,
    limit: u32,
    sent_at: i64,
    bytes: u64,
}

enum PageError {
    Xrpc(XrpcCallError),
    /// Even a one-record page is larger than the page cap.
    TooLarge,
    Bad(String),
}

/// The listing context of one repo.
pub struct Lister<'a, H: Host> {
    /// The XRPC client (over the job's [`NetHost`]).
    pub xrpc: &'a Xrpc<'a, NetHost<H>>,
    /// The repo's PDS.
    pub service: &'a Service,
    /// The PDS origin.
    pub origin: &'a str,
    /// The repo.
    pub did: &'a Did,
    /// Record validity.
    pub validator: &'a dyn RecordValidator,
    /// Parameters.
    pub tuning: &'a ListTuning,
}

impl<H: Host> Lister<'_, H> {
    /// One page at `cursor`, retrying an oversized page with smaller limits.
    async fn page(&self, coll: &Nsid, cursor: Option<&str>) -> Result<Page, PageError> {
        let mut limits_seq = vec![self.tuning.limit];
        limits_seq.extend(
            LIST_PAGE_RETRY_LIMITS
                .iter()
                .copied()
                .filter(|l| *l < self.tuning.limit),
        );
        for limit in limits_seq {
            let params = ListRecords {
                repo: AtIdentifier::Did(self.did.clone()),
                collection: coll.clone(),
                limit: Some(limit),
                cursor: cursor.map(str::to_owned),
                reverse: None,
            };
            let opts = CallOpts::capped(self.tuning.page_max_bytes);
            let resp = match self.xrpc.query_raw(self.service, &params, &opts).await {
                Ok(r) => r,
                Err(XrpcCallError::TooLarge) => continue,
                Err(e) => return Err(PageError::Xrpc(e)),
            };
            let sent_at = net::sent_at(&resp).unwrap_or_default();
            let expect = Expect {
                did: self.did,
                collection: coll.as_str(),
                rkey: None,
            };
            let page = admit::admit_list_page(&resp.body, &expect, self.validator)
                .map_err(|e| PageError::Bad(e.to_string()))?;
            let mut rkeys = Vec::with_capacity(page.entries.len());
            for e in &page.entries {
                match &e.uri {
                    Some(u) if u.did() == self.did && u.collection() == coll => {
                        rkeys.push(u.rkey().as_str().to_owned());
                    }
                    _ => {
                        return Err(PageError::Bad(format!(
                            "entry uri {:?} is not a canonical record uri of {} in {coll}",
                            e.raw_uri, self.did
                        )));
                    }
                }
            }
            return Ok(Page {
                page,
                rkeys,
                limit,
                sent_at,
                bytes: resp.body.len() as u64,
            });
        }
        Err(PageError::TooLarge)
    }

    /// `list_range(did, coll, fk)` (§5.3). `use_range` is the host's answer to
    /// [`HostRow::use_range`]; `short_page_ends` its short-page rule.
    pub async fn list_range(
        &self,
        coll: &Nsid,
        fk: &str,
        use_range: bool,
        short_page_ends: bool,
    ) -> Result<RangeListing, ListError> {
        if use_range {
            match self.range_mode(coll, fk, short_page_ends).await? {
                RangeOutcome::Done(l) => return Ok(l),
                RangeOutcome::Fallback(strikes) => {
                    let mut l = self.full_mode(coll, Some(fk)).await?;
                    l.verdict = Some(RangeVerdict::No);
                    l.strikes += strikes;
                    return Ok(l);
                }
            }
        }
        self.full_mode(coll, Some(fk)).await
    }

    /// Every record of `coll` (scope `OWN`, §5.1): a full listing without a prefix filter or cap.
    pub async fn list_all(&self, coll: &Nsid) -> Result<RangeListing, ListError> {
        self.full_mode(coll, None).await
    }

    async fn range_mode(
        &self,
        coll: &Nsid,
        fk: &str,
        short_page_ends: bool,
    ) -> Result<RangeOutcome, ListError> {
        let p = format!("{fk}-");
        let b = format!("{fk}.");
        let n = self.tuning.cap(coll.as_str());
        let max_bytes = self.tuning.range_max(coll.as_str());
        let mut out = RangeListing::new(ListMode::Range);
        let mut cursor = b.clone();
        let mut prev: Option<String> = None;
        let mut first = true;
        let mut bytes = 0u64;
        loop {
            let page = match self.page(coll, Some(&cursor)).await {
                Ok(p) => p,
                Err(PageError::Xrpc(e))
                    if first && e.status() == Some(400) && !e.is_repo_status() =>
                {
                    return Ok(RangeOutcome::Fallback(out.strikes));
                }
                Err(PageError::Xrpc(e)) => return Err(e.into()),
                Err(PageError::TooLarge) => {
                    out.stopped(format!(
                        "a one-record page is over {} bytes",
                        self.tuning.page_max_bytes
                    ));
                    return Ok(RangeOutcome::Done(out));
                }
                Err(PageError::Bad(why)) => return Err(ListError::BadPage(why)),
            };
            let ks = &page.rkeys;
            let violation = ks.iter().any(|k| k.as_str() >= cursor.as_str() || *k >= b)
                || ks.windows(2).any(|w| w[0] <= w[1])
                || matches!((&prev, ks.first()), (Some(pv), Some(k0)) if k0 >= pv)
                || matches!((ks.last(), &page.page.cursor), (Some(last), Some(c)) if c != last);
            if violation {
                return Ok(RangeOutcome::Fallback(out.strikes + 1));
            }
            if first {
                out.first_sent_at = Some(page.sent_at);
            }
            first = false;
            for (entry, rkey) in page.page.entries.into_iter().zip(ks.iter()) {
                if !rkey.starts_with(&p) {
                    out.complete = true;
                    out.verdict = Some(RangeVerdict::Yes);
                    return Ok(RangeOutcome::Done(out));
                }
                if entry.result.is_err() {
                    out.strikes += 1;
                }
                out.entries.push(Listed {
                    rkey: rkey.clone(),
                    record: entry.result,
                    sent_at: page.sent_at,
                });
                if out.entries.len() >= n {
                    out.complete = true;
                    out.capped = true;
                    out.verdict = Some(RangeVerdict::Yes);
                    return Ok(RangeOutcome::Done(out));
                }
            }
            bytes = bytes.saturating_add(page.bytes);
            if bytes > max_bytes {
                out.stopped(format!("the range is over {max_bytes} bytes"));
                return Ok(RangeOutcome::Done(out));
            }
            let short = (ks.len() as u32) < page.limit && short_page_ends;
            match (&page.page.cursor, ks.last()) {
                (Some(c), Some(last)) if !short => {
                    prev = Some(last.clone());
                    cursor = c.clone();
                }
                _ => {
                    out.complete = true;
                    out.verdict = Some(RangeVerdict::Yes);
                    return Ok(RangeOutcome::Done(out));
                }
            }
        }
    }

    async fn full_mode(&self, coll: &Nsid, fk: Option<&str>) -> Result<RangeListing, ListError> {
        let prefix = fk.map(|f| format!("{f}-"));
        let n = fk.map_or(usize::MAX, |_| self.tuning.cap(coll.as_str()));
        let mut out = RangeListing::new(ListMode::Full);
        let mut all: Vec<Listed> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0u32;
        let mut bytes = 0u64;
        let mut complete = false;
        loop {
            let page = match self.page(coll, cursor.as_deref()).await {
                Ok(p) => p,
                Err(PageError::Xrpc(e)) => return Err(e.into()),
                Err(PageError::TooLarge) => {
                    out.stopped(format!(
                        "a one-record page is over {} bytes",
                        self.tuning.page_max_bytes
                    ));
                    break;
                }
                Err(PageError::Bad(why)) => return Err(ListError::BadPage(why)),
            };
            if out.first_sent_at.is_none() {
                out.first_sent_at = Some(page.sent_at);
            }
            pages += 1;
            bytes = bytes.saturating_add(page.bytes);
            let empty = page.rkeys.is_empty();
            for (entry, rkey) in page.page.entries.into_iter().zip(page.rkeys.iter()) {
                if prefix
                    .as_ref()
                    .is_some_and(|p| !rkey.starts_with(p.as_str()))
                {
                    continue;
                }
                if entry.result.is_err() {
                    out.strikes += 1;
                }
                all.push(Listed {
                    rkey: rkey.clone(),
                    record: entry.result,
                    sent_at: page.sent_at,
                });
            }
            let next = match page.page.cursor {
                Some(c) if !empty => c,
                _ => {
                    complete = true;
                    break;
                }
            };
            if seen.contains(&next) {
                // cursors must strictly advance (§5.10): a host misbehaviour
                out.strikes += 1;
                out.stopped(format!("the cursor {next:?} repeated"));
                break;
            }
            if pages >= self.tuning.full_max_pages {
                out.stopped(format!("over {} pages", self.tuning.full_max_pages));
                break;
            }
            if bytes > self.tuning.full_max_bytes {
                out.stopped(format!("over {} bytes", self.tuning.full_max_bytes));
                break;
            }
            seen.insert(next.clone());
            cursor = Some(next);
        }
        all.sort_by(|a, b| b.rkey.cmp(&a.rkey));
        if complete && let Some(w) = all.windows(2).find(|w| w[0].rkey == w[1].rkey) {
            return Err(ListError::Duplicate(w[0].rkey.clone()));
        }
        all.dedup_by(|a, b| a.rkey == b.rkey);
        if complete {
            out.complete = true;
            out.capped = all.len() >= n;
        }
        all.truncate(n);
        out.entries = all;
        Ok(out)
    }
}

enum RangeOutcome {
    Done(RangeListing),
    /// The host failed a check: redo the listing in `FULL` mode (with these strikes).
    Fallback(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        let mut h = HostRow::default();
        apply_verdicts(&mut h, &[RangeVerdict::Yes, RangeVerdict::Yes], 5);
        assert_eq!(h.range_cursor, crate::store::repo::RangeCursor::Yes);
        apply_verdicts(&mut h, &[RangeVerdict::Yes, RangeVerdict::No], 9);
        assert_eq!(
            (h.range_cursor, h.rc_at),
            (crate::store::repo::RangeCursor::No, 9)
        );
        apply_verdicts(&mut h, &[], 10);
        assert_eq!(h.rc_at, 9);
    }

    #[test]
    fn caps() {
        let t = ListTuning::default();
        let ns = lessdugin_lexicon::ns::NS;
        assert_eq!(t.cap(&format!("{ns}.opinion")), 100_000);
        assert_eq!(t.cap(&format!("{ns}.post")), 20_000);
        assert_eq!(
            t.range_max(&format!("{ns}.opinion")),
            RANGE_MAX_BYTES_OPINION
        );
        assert_eq!(t.range_max(&format!("{ns}.comment")), RANGE_MAX_BYTES);
        let t = ListTuning {
            fetch_cap: Some(3),
            ..ListTuning::default()
        };
        assert_eq!(t.cap(&format!("{ns}.opinion")), 3);
    }
}
