// SPDX-License-Identifier: AGPL-3.0-or-later
//! Time (`magic/1`, `social.md` §2): conversions and the effective time `t_rank`.
//!
//! ```text
//! µs(datetime) = the atproto datetime in µs (any offset, fraction truncated); None if it does
//!                not parse or is before 1970
//! tidTime(tid) = the TID's 64-bit value >> 10
//! t_obs(x)     = min over obs of x.uri (any version)
//! t_eff(x)     = min_some(t_claim(x), t_tid(x))
//! future(x)    ⇔ t_obs(x) ≠ None ∧ t_eff(x) > t_obs(x) + 300_000_000
//! t_eff'(x)    = future(x) ? t_obs(x) : t_eff(x)
//! t_decl       = c = µs(decl.createdAt) or 0, o = t_obs(decl): (o ≠ None ∧ c > o + 300 s) ? o : c
//! t_rank(x)    = max(lower(x), t_eff'(x)),  lower = t_rank(treeParent) for comments, else t_decl
//! inconsistent ⇔ both present ∧ |t_claim − t_tid| > 3_600_000_000
//! ```
//!
//! Pure functions: no clock is read. The tree walk for comments is in
//! [`crate::social::full::timeline`], which visits each thread top-down after placement (§11
//! step 5).

use lessdugin_atp::syntax::Datetime;
use lessdugin_atp::tid::Tid;

/// The future-dating tolerance (µs): 5 minutes (`limits.md` `FUTURE_SKEW_MS`).
pub const FUTURE_SKEW_US: i64 = crate::limits::FUTURE_SKEW_MS * 1000;

/// The inconsistent-timestamps threshold (µs): one hour (§2.3).
pub const INCONSISTENT_US: i64 = 3_600_000_000;

/// `µs(datetime)` (§2.1): `None` if `s` is not an atproto datetime or is before the epoch.
pub fn us(s: &str) -> Option<i64> {
    let t = Datetime::parse(s).ok()?.timestamp_micros();
    (t >= 0).then_some(t)
}

/// `tidTime(tid)` (§2.1): the TID's integer value without its 10-bit clock id; `None` if `tid`
/// is not a TID.
pub fn tid_time(tid: &str) -> Option<i64> {
    let t = Tid::parse(tid).ok()?;
    i64::try_from(t.to_u64() >> 10).ok()
}

/// The TID an item's rkey carries (§2.2 `t_tid`): the last `-`-separated segment (a post,
/// wikiRevision or sequence key `{fk}-{tid}`, a comment key `{fk}-{rootTid}-{tid}`).
pub fn rkey_tid(rkey: &str) -> Option<&str> {
    rkey.rsplit('-').next()
}

/// The timestamps of one item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemTimes {
    /// `t_claim`: µs(createdAt).
    pub claim: Option<i64>,
    /// `t_tid`: the rkey TID's time.
    pub tid: Option<i64>,
    /// `t_obs`: the earliest first-seen time of any version.
    pub obs: Option<i64>,
}

impl ItemTimes {
    /// `t_eff = min_some(t_claim, t_tid)` (`None` only if both are absent, which validity
    /// excludes).
    pub fn eff(&self) -> Option<i64> {
        match (self.claim, self.tid) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// `future(x)`: first-seen known and `t_eff` more than 5 minutes after it.
    pub fn future(&self) -> bool {
        match (self.obs, self.eff()) {
            (Some(o), Some(e)) => e > o.saturating_add(FUTURE_SKEW_US),
            _ => false,
        }
    }

    /// `t_eff'`: the first-seen time for future-dated items, else `t_eff` (`0` if neither time
    /// is present).
    pub fn eff_clamped(&self) -> i64 {
        if self.future() {
            self.obs.unwrap_or(0)
        } else {
            self.eff().unwrap_or(0)
        }
    }

    /// The display-only "inconsistent timestamps" flag (§2.3).
    pub fn inconsistent(&self) -> bool {
        match (self.claim, self.tid) {
            (Some(a), Some(b)) => (a.saturating_sub(b)).saturating_abs() > INCONSISTENT_US,
            _ => false,
        }
    }

    /// `t_rank = max(lower, t_eff')`.
    pub fn rank(&self, lower: i64) -> i64 {
        lower.max(self.eff_clamped())
    }
}

/// `t_decl` (§2.2) from µs(decl.createdAt) and t_obs(decl).
pub fn t_decl(claim: Option<i64>, obs: Option<i64>) -> i64 {
    let c = claim.unwrap_or(0);
    match obs {
        Some(o) if c > o.saturating_add(FUTURE_SKEW_US) => o,
        _ => c,
    }
}

/// Floor division of µs to ms (`engine-api.md` §1: `…Ms` fields).
pub fn ms(us: i64) -> i64 {
    us.div_euclid(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(us("2026-01-01T00:00:00.000Z"), Some(1_767_225_600_000_000));
        assert_eq!(us("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(us("1969-12-31T23:59:59.999999Z"), None);
        assert_eq!(
            us("2026-01-01T00:00:00.1234567Z"),
            Some(1_767_225_600_123_456)
        );
        assert_eq!(us("2026-01-01"), None);
        assert_eq!(tid_time("2222222222222"), Some(0));
        assert_eq!(tid_time("3mbd3542k22zz"), Some(1_767_225_600_000_000));
        assert_eq!(tid_time("bzzzzzzzzzzzz"), Some(9_007_199_254_740_991));
        assert_eq!(tid_time("nope"), None);
        assert_eq!(
            rkey_tid("aaaaaaaaaaaa-3mbd3542k2222-3mbd3542k22zz"),
            Some("3mbd3542k22zz")
        );
        assert_eq!(ms(-1), -1);
        assert_eq!(ms(1999), 1);
    }

    #[test]
    fn future_clamp_and_flags() {
        let o = 1_000_000_000_000;
        let t = |claim: i64| ItemTimes {
            claim: Some(claim),
            tid: Some(claim),
            obs: Some(o),
        };
        assert!(!t(o + FUTURE_SKEW_US).future());
        assert!(t(o + FUTURE_SKEW_US + 1).future());
        assert_eq!(t(o + FUTURE_SKEW_US + 1).rank(0), o);
        assert_eq!(t(o + 5).rank(o + 10), o + 10);
        let i = ItemTimes {
            claim: Some(0),
            tid: Some(INCONSISTENT_US + 1),
            obs: None,
        };
        assert!(i.inconsistent());
        assert!(!i.future());
        assert_eq!(i.eff(), Some(0));
        assert_eq!(t_decl(Some(o + FUTURE_SKEW_US + 1), Some(o)), o);
        assert_eq!(
            t_decl(Some(o + FUTURE_SKEW_US), Some(o)),
            o + FUTURE_SKEW_US
        );
        assert_eq!(t_decl(None, None), 0);
    }
}
