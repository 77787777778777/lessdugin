// SPDX-License-Identifier: AGPL-3.0-or-later
//! The per-account write budget and write rate limits (`data-model.md` §8.2, `sync.md` §7.3).
//!
//! - **Budget.** Sliding windows of 5,000 points per hour and 35,000 points per day; a create
//!   costs 3, an update 2 and a delete 1, counting every op of an `applyWrites`. Spent points are
//!   kept per minute bucket in `("out.budget", account)`; a bucket counts in a window while any
//!   part of its minute does, so the estimate never under-counts.
//! - **Rate limits.** A `429` (or `RateLimit-Remaining: 0` on a write response) blocks the
//!   account's writes until `max(Retry-After, RateLimit-Reset)`.
//!
//! A write whose next request would not fit waits in `throttled` until [`BudgetRow::next_fit`].

use serde::{Deserialize, Serialize};

use crate::host::HttpResponse;
use crate::limits::{WRITE_BUDGET_POINTS_PER_DAY, WRITE_BUDGET_POINTS_PER_HOUR};
use crate::outbox::model::OUTBOX_ROW_VERSION;

/// Width of a budget bucket (ms).
pub const BUCKET_MS: i64 = 60_000;
/// The hourly window (ms).
pub const HOUR_MS: i64 = 60 * 60 * 1000;
/// The daily window (ms).
pub const DAY_MS: i64 = 24 * HOUR_MS;
/// The wait after a `429` without `Retry-After` or `RateLimit-Reset` (ms, *tunable*).
pub const DEFAULT_RATE_LIMIT_WAIT_MS: i64 = 60_000;

/// `("out.budget", account)`: spent write points and the rate-limit block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetRow {
    /// Row version.
    pub v: u32,
    /// Spent points per minute: `(bucket start ms, points)`, ascending, at most a day old.
    #[serde(default)]
    pub spent: Vec<(i64, u32)>,
    /// No write is sent before this time (ms): a `429`, or a write response that reported its
    /// `RateLimit-Remaining` as 0.
    #[serde(default)]
    pub blocked_until: i64,
}

impl Default for BudgetRow {
    fn default() -> Self {
        Self {
            v: OUTBOX_ROW_VERSION,
            spent: Vec::new(),
            blocked_until: 0,
        }
    }
}

impl BudgetRow {
    /// Points counted at `at` in the window of `window_ms` ending at `at`.
    pub fn window_points(&self, at: i64, window_ms: i64) -> u64 {
        self.spent
            .iter()
            .filter(|(b, _)| *b <= at && b + BUCKET_MS > at - window_ms)
            .map(|(_, p)| u64::from(*p))
            .sum()
    }

    /// `true` if a request of `cost` points may be sent at `at`.
    pub fn fits(&self, at: i64, cost: u32) -> bool {
        let cost = u64::from(cost);
        at >= self.blocked_until
            && self.window_points(at, HOUR_MS) + cost <= u64::from(WRITE_BUDGET_POINTS_PER_HOUR)
            && self.window_points(at, DAY_MS) + cost <= u64::from(WRITE_BUDGET_POINTS_PER_DAY)
    }

    /// The earliest time `≥ now` at which a request of `cost` points fits. A cost above the
    /// hourly budget never fits; it is reported as a day from now (callers cap their requests).
    pub fn next_fit(&self, now: i64, cost: u32) -> i64 {
        if self.fits(now, cost) {
            return now;
        }
        let mut candidates: Vec<i64> = vec![self.blocked_until];
        for (b, _) in &self.spent {
            candidates.push(b + BUCKET_MS + HOUR_MS);
            candidates.push(b + BUCKET_MS + DAY_MS);
        }
        candidates.retain(|t| *t > now);
        candidates.sort_unstable();
        candidates.dedup();
        candidates
            .into_iter()
            .find(|t| self.fits(*t, cost))
            .unwrap_or(now + DAY_MS)
    }

    /// Records `cost` points spent at `now` and forgets buckets older than a day.
    pub fn charge(&mut self, now: i64, cost: u32) {
        if cost == 0 {
            return;
        }
        let bucket = now - now.rem_euclid(BUCKET_MS);
        match self.spent.iter_mut().find(|(b, _)| *b == bucket) {
            Some((_, p)) => *p = p.saturating_add(cost),
            None => {
                self.spent.push((bucket, cost));
                self.spent.sort_unstable();
            }
        }
        self.spent.retain(|(b, _)| b + BUCKET_MS + DAY_MS > now);
    }

    /// Blocks writes until `until` (keeps a later block).
    pub fn block_until(&mut self, until: i64) {
        self.blocked_until = self.blocked_until.max(until);
    }
}

/// `RateLimit-Reset` as an absolute time (ms): epoch seconds above 10^9, otherwise seconds from
/// `now` (`sync.md` §9.2).
pub fn rate_limit_reset_at(resp: &HttpResponse, now: i64) -> Option<i64> {
    let t: i64 = resp.header("ratelimit-reset")?.trim().parse().ok()?;
    if t < 0 {
        return None;
    }
    Some(if t > 1_000_000_000 {
        t.saturating_mul(1000)
    } else {
        now.saturating_add(t.saturating_mul(1000))
    })
}

/// When a `429` lets writes resume: `max(Retry-After, RateLimit-Reset)`, or
/// [`DEFAULT_RATE_LIMIT_WAIT_MS`] from now without either header.
pub fn rate_limited_until(resp: &HttpResponse, now: i64) -> i64 {
    let retry_after = resp
        .header("retry-after")
        .and_then(|v| crate::xrpc::parse_retry_after(v, now))
        .map(|ms| now.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX)));
    let reset = rate_limit_reset_at(resp, now);
    match (retry_after, reset) {
        (None, None) => now + DEFAULT_RATE_LIMIT_WAIT_MS,
        (a, b) => a.max(b).unwrap_or(now),
    }
}

/// After a successful write response: the block its `RateLimit-*` headers ask for, if the
/// remaining budget is exhausted.
pub fn exhausted_until(resp: &HttpResponse, now: i64) -> Option<i64> {
    let remaining: i64 = resp.header("ratelimit-remaining")?.trim().parse().ok()?;
    if remaining > 0 {
        return None;
    }
    Some(rate_limit_reset_at(resp, now).unwrap_or(now + DEFAULT_RATE_LIMIT_WAIT_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status: 429,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            body: Vec::new(),
        }
    }

    #[test]
    fn sliding_windows() {
        let t0 = 1_760_000_040_000; // a bucket boundary
        let mut b = BudgetRow::default();
        assert!(b.fits(t0, 5_000));
        assert!(!b.fits(t0, 5_001));
        b.charge(t0, 4_998);
        assert!(b.fits(t0, 2));
        assert!(!b.fits(t0, 3));
        // the bucket leaves the hourly window one bucket after an hour
        let at = b.next_fit(t0, 3);
        assert!(at > t0 + HOUR_MS && at <= t0 + HOUR_MS + BUCKET_MS);
        assert!(b.fits(at, 3));
        assert!(!b.fits(at - 1, 3));
        // the daily window
        let mut d = BudgetRow::default();
        for h in 0..7 {
            d.charge(t0 + h * HOUR_MS + 5 * BUCKET_MS, 5_000);
        }
        let now = t0 + 7 * HOUR_MS + 6 * BUCKET_MS;
        assert!(!d.fits(now, 1), "35,000 points spent within a day");
        let at = d.next_fit(now, 1);
        assert_eq!(at, t0 + 6 * BUCKET_MS + DAY_MS);
        // pruning
        d.charge(t0 + 3 * DAY_MS, 1);
        assert_eq!(d.spent.len(), 1);
    }

    #[test]
    fn rate_limit_headers() {
        let now = 1_000_000_000_000;
        assert_eq!(
            rate_limited_until(&resp(&[("retry-after", "30")]), now),
            now + 30_000
        );
        assert_eq!(
            rate_limited_until(
                &resp(&[("retry-after", "30"), ("ratelimit-reset", "90")]),
                now
            ),
            now + 90_000
        );
        assert_eq!(
            rate_limited_until(&resp(&[("ratelimit-reset", "1000000120")]), now),
            1_000_000_120_000
        );
        assert_eq!(
            rate_limited_until(&resp(&[]), now),
            now + DEFAULT_RATE_LIMIT_WAIT_MS
        );
        assert_eq!(
            exhausted_until(&resp(&[("ratelimit-remaining", "3")]), now),
            None
        );
        assert_eq!(
            exhausted_until(
                &resp(&[("ratelimit-remaining", "0"), ("ratelimit-reset", "10")]),
                now
            ),
            Some(now + 10_000)
        );
        let mut b = BudgetRow::default();
        b.block_until(now + 5);
        b.block_until(now + 1);
        assert_eq!(b.blocked_until, now + 5);
        assert!(!b.fits(now, 1));
        assert_eq!(b.next_fit(now, 1), now + 5);
    }
}
