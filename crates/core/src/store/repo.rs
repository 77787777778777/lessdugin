// SPDX-License-Identifier: AGPL-3.0-or-later
//! Repo state (`("repo", did)` → [`RepoRow`], `sync.md` §5.9, §10.5) and host capabilities
//! (`("host", origin)` → [`HostRow`], §5.3, §9).
//!
//! **Only the author's PDS advances repo state** (§1.2): `rev`, `commit`, `listed` and `headSeen`
//! change only from responses of `PDS(did)`. Third-party proofs and hints never touch this row.

use std::collections::BTreeMap;

use lessdugin_atp::Cid;
use serde::{Deserialize, Serialize};

use crate::limits;
use crate::store::row::ROW_VERSION;

/// `No` is re-probed after this long (`sync.md` §5.3, *tunable*).
pub const RANGE_CURSOR_RECHECK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Repo backoff base (`sync.md` §9.4, *tunable*).
pub const REPO_BACKOFF_BASE_MS: i64 = 30_000;
/// Repo backoff cap (`sync.md` §9.4, *tunable*).
pub const REPO_BACKOFF_MAX_MS: i64 = 24 * 60 * 60 * 1000;
/// `hot` repos changed at most this long ago (`sync.md` §5.9, *tunable*).
pub const HOT_MS: i64 = 24 * 60 * 60 * 1000;
/// `warm` repos changed at most this long ago (`sync.md` §5.9, *tunable*).
pub const WARM_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The fold's `repoStatus` (`social.md` §1.1, `sync.md` §5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoStatus {
    /// Completely listed at the latest known head.
    Fresh,
    /// Completely listed at an earlier rev.
    Stale,
    /// Never completely listed.
    Unavailable,
}

/// Account status (`sync.md` §3.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    /// Active (the default until told otherwise).
    #[default]
    Active,
    /// Deactivated, taken down, suspended, deleted: excluded from every fold input (I8).
    Inactive,
    /// `desynchronized`, `throttled`: still included.
    Temporary,
}

/// Polling class (`sync.md` §5.9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoClass {
    /// An authority of a forum open in a view.
    Authority,
    /// Changed within a day.
    Hot,
    /// Changed within 30 days.
    Warm,
    /// Otherwise.
    #[default]
    Cold,
}

/// A head observation (`getLatestCommit` from the author's PDS).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadSeen {
    /// The head rev.
    pub rev: String,
    /// The head commit CID.
    pub cid: Cid,
    /// When the request was sent (ms).
    pub sent_at: i64,
}

/// `RepoRow` (`sync.md` §10.5). Scopes are fk strings, `"global"` or `"own"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRow {
    /// Row version.
    pub v: u32,
    /// The PDS endpoint the state was synced from.
    pub pds: String,
    /// The rev of the last stable pass (`""`: none).
    pub rev: String,
    /// The commit CID of that pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<Cid>,
    /// The latest head observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_seen: Option<HeadSeen>,
    /// `describeRepo` collections ∩ (OURS ∪ {app.bsky.actor.profile}).
    #[serde(default)]
    pub colls: Vec<String>,
    /// When `colls` was fetched (ms).
    #[serde(default)]
    pub colls_at: i64,
    /// The head rev `colls` was fetched at (`""`: unknown): a user-initiated pass trusts
    /// `colls` only at this rev (`sync.md` §5.2).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub colls_rev: String,
    /// A hint or own write named a collection not in `colls`.
    #[serde(default)]
    pub colls_dirty: bool,
    /// Per scope: the rev at which every range of the scope was completely listed.
    #[serde(default)]
    pub listed: BTreeMap<String, String>,
    /// Per scope: the cap boundary (smallest listed rkey) of each capped collection.
    #[serde(default)]
    pub capped: BTreeMap<String, BTreeMap<String, String>>,
    /// Per scope: the last listing hit a byte cap.
    #[serde(default)]
    pub byte_capped: BTreeMap<String, bool>,
    /// Dirty marks: uri → rev (§6.5), at most [`limits::REPO_MARKS_MAX`].
    #[serde(default)]
    pub marks: BTreeMap<String, String>,
    /// A mark was dropped because of the cap (a full sync is due).
    #[serde(default)]
    pub marks_overflow: bool,
    /// Last sync attempt (ms).
    #[serde(default)]
    pub checked_at: i64,
    /// Last successful sync (ms).
    #[serde(default)]
    pub last_ok: i64,
    /// Next check not before (ms; repo backoff).
    #[serde(default)]
    pub next_check: i64,
    /// Consecutive failed syncs.
    #[serde(default)]
    pub fails: u32,
    /// Consecutive unstable passes.
    #[serde(default)]
    pub unstable: u32,
    /// Polling class.
    #[serde(default)]
    pub class: RepoClass,
    /// Account status.
    #[serde(default)]
    pub status: AccountStatus,
    /// The raw status from the PDS, if not active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_detail: Option<String>,
    /// When the status was checked (ms).
    #[serde(default)]
    pub status_at: i64,
    /// Since when the head is below `rev` (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_since: Option<i64>,
    /// Last time a sync changed a stored row of this repo (ms).
    #[serde(default)]
    pub last_change_at: i64,
    /// Last reconcile (ms).
    #[serde(default)]
    pub last_reconcile: i64,
}

impl RepoRow {
    /// A fresh row for a repo hosted at `pds`.
    pub fn new(pds: &str) -> Self {
        Self {
            v: ROW_VERSION,
            pds: pds.to_owned(),
            rev: String::new(),
            commit: None,
            head_seen: None,
            colls: Vec::new(),
            colls_at: 0,
            colls_rev: String::new(),
            colls_dirty: false,
            listed: BTreeMap::new(),
            capped: BTreeMap::new(),
            byte_capped: BTreeMap::new(),
            marks: BTreeMap::new(),
            marks_overflow: false,
            checked_at: 0,
            last_ok: 0,
            next_check: 0,
            fails: 0,
            unstable: 0,
            class: RepoClass::Cold,
            status: AccountStatus::Active,
            status_detail: None,
            status_at: 0,
            rollback_since: None,
            last_change_at: 0,
            last_reconcile: 0,
        }
    }

    /// `listed[scope]`, `""` if never completely listed.
    pub fn listed_rev(&self, scope: &str) -> &str {
        self.listed.get(scope).map_or("", String::as_str)
    }

    /// The cap boundary of `(scope, coll)`, if that range was capped.
    pub fn cap_boundary(&self, scope: &str, coll: &str) -> Option<&str> {
        self.capped
            .get(scope)
            .and_then(|m| m.get(coll))
            .map(String::as_str)
    }

    /// `latestKnown(d) = max(headSeen.rev, max marks rev)` (§5.9).
    pub fn latest_known(&self) -> &str {
        let head = self.head_seen.as_ref().map_or("", |h| h.rev.as_str());
        self.marks
            .values()
            .map(String::as_str)
            .fold(head, |a, b| a.max(b))
    }

    /// `repoStatus(d, scope)` (§5.9).
    pub fn repo_status(&self, scope: &str) -> RepoStatus {
        let listed = self.listed_rev(scope);
        if listed.is_empty() {
            RepoStatus::Unavailable
        } else if listed >= self.latest_known()
            && !self.byte_capped.get(scope).copied().unwrap_or(false)
        {
            RepoStatus::Fresh
        } else {
            RepoStatus::Stale
        }
    }

    /// Records a head observation from the author's PDS; the latest by send time wins.
    pub fn observe_head(&mut self, rev: &str, cid: Cid, sent_at: i64) {
        if self.head_seen.as_ref().is_none_or(|h| sent_at >= h.sent_at) {
            self.head_seen = Some(HeadSeen {
                rev: rev.to_owned(),
                cid,
                sent_at,
            });
        }
    }

    /// Adds a dirty mark; beyond [`limits::REPO_MARKS_MAX`] sets `marks_overflow` instead.
    /// Returns `false` on overflow.
    pub fn add_mark(&mut self, uri: &str, rev: &str) -> bool {
        if let Some(r) = self.marks.get_mut(uri) {
            if rev > r.as_str() {
                *r = rev.to_owned();
            }
            return true;
        }
        if self.marks.len() >= limits::REPO_MARKS_MAX {
            self.marks_overflow = true;
            return false;
        }
        self.marks.insert(uri.to_owned(), rev.to_owned());
        true
    }

    /// Removes every mark with `rev ≤ upto` (a stable listing pass at `upto`, §5.2).
    pub fn clear_marks_upto(&mut self, upto: &str) {
        self.marks.retain(|_, r| r.as_str() > upto);
    }

    /// A stable pass at head `(rev, commit)`: `rev`/`commit` advance and the marks it covers go.
    /// `complete` lists each scope whose ranges were all complete, with its capped boundaries
    /// (`coll → smallest listed rkey`) and whether a byte cap was hit.
    pub fn apply_stable_pass<'a>(
        &mut self,
        rev: &str,
        commit: Cid,
        complete: impl IntoIterator<Item = (&'a str, BTreeMap<String, String>)>,
    ) {
        self.rev = rev.to_owned();
        self.commit = Some(commit);
        for (scope, capped) in complete {
            self.listed.insert(scope.to_owned(), rev.to_owned());
            if capped.is_empty() {
                self.capped.remove(scope);
            } else {
                self.capped.insert(scope.to_owned(), capped);
            }
            self.byte_capped.remove(scope);
        }
        self.clear_marks_upto(rev);
        // A stable pass covers every change up to `rev`, including the dropped marks' (they were
        // reported at revs the PDS had reached); once no mark is left the overflow is resolved.
        if self.marks.is_empty() {
            self.marks_overflow = false;
        }
        self.unstable = 0;
        self.rollback_since = None;
    }

    /// Marks a scope's last listing as byte-capped (it stays unlisted or `Stale`).
    pub fn set_byte_capped(&mut self, scope: &str, capped: bool) {
        if capped {
            self.byte_capped.insert(scope.to_owned(), true);
        } else {
            self.byte_capped.remove(scope);
        }
    }

    /// Forgets the listing of `scope` (eviction, `sync --full`).
    pub fn reset_scope(&mut self, scope: &str) {
        self.listed.remove(scope);
        self.capped.remove(scope);
        self.byte_capped.remove(scope);
    }

    /// A successful sync at `now`.
    pub fn succeed(&mut self, now_ms: i64) {
        self.fails = 0;
        self.checked_at = now_ms;
        self.last_ok = now_ms;
        self.next_check = now_ms;
    }

    /// A failed sync at `now`: `nextCheck = now + min(30 s · 2^(fails−1), 24 h) · uniform(0.8,
    /// 1.2)` (§9.4); `random` is any value from `Host::fill_random`.
    pub fn fail(&mut self, now_ms: i64, random: u64) {
        self.fails = self.fails.saturating_add(1);
        self.checked_at = now_ms;
        let exp = self.fails.saturating_sub(1).min(32);
        let base = REPO_BACKOFF_BASE_MS
            .saturating_mul(1i64 << exp.min(31))
            .min(REPO_BACKOFF_MAX_MS);
        let permille = 800 + (random % 401) as i64;
        self.next_check = now_ms.saturating_add(base / 1000 * permille);
    }

    /// The polling class at `now` (§5.9).
    pub fn class_at(&self, now_ms: i64, authority: bool) -> RepoClass {
        let age = now_ms.saturating_sub(self.last_change_at);
        if authority {
            RepoClass::Authority
        } else if self.last_change_at > 0 && age <= HOT_MS {
            RepoClass::Hot
        } else if self.last_change_at > 0 && age <= WARM_MS {
            RepoClass::Warm
        } else {
            RepoClass::Cold
        }
    }
}

/// The cursor-range capability of a host (§5.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RangeCursor {
    /// Not probed yet.
    #[default]
    Unknown,
    /// The cursor works as an exclusive rkey bound.
    Yes,
    /// It does not (opaque cursors, 400, other order).
    No,
}

/// Whether a browser may read this host cross-origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cors {
    /// Not observed yet.
    #[default]
    Unknown,
    /// Cross-origin reads succeeded.
    Allowed,
    /// A cross-origin read was refused.
    Blocked,
}

/// Breaker state (§9.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BreakerState {
    /// Requests flow.
    #[default]
    Closed,
    /// Requests are refused until `until`.
    Open,
    /// One probe is allowed.
    HalfOpen,
}

/// The circuit breaker of a host (§9.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breaker {
    /// State.
    pub state: BreakerState,
    /// Consecutive failures.
    pub failures: u32,
    /// Open until (ms).
    pub until: i64,
    /// Current open period (ms).
    pub open_ms: i64,
}

/// A host's read budget (§9.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    /// `RateLimit-Limit`.
    pub limit: i64,
    /// Estimated remaining requests.
    pub est: i64,
    /// Window reset (ms).
    pub reset_at: i64,
}

/// `HostRow` (`sync.md` §10.5), keyed by origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRow {
    /// Row version.
    pub v: u32,
    /// Cursor-range capability.
    pub range_cursor: RangeCursor,
    /// When it was last set (ms).
    pub rc_at: i64,
    /// A short page ends a `RANGE` listing (`doctor` may turn it off).
    pub short_page_ends: bool,
    /// `putRecord` with `swapRecord: null` works (ADR 0005); `None` until observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub null_swap: Option<bool>,
    /// Circuit breaker.
    #[serde(default)]
    pub breaker: Breaker,
    /// Read budget, if the host sent `RateLimit-*` headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    /// Admission strikes (CID mismatches, malformed records, §5.5).
    #[serde(default)]
    pub strikes: u32,
    /// Cross-origin readability (browser).
    #[serde(default)]
    pub cors: Cors,
    /// When `cors` was observed (ms).
    #[serde(default)]
    pub cors_at: i64,
}

impl Default for HostRow {
    fn default() -> Self {
        Self {
            v: ROW_VERSION,
            range_cursor: RangeCursor::Unknown,
            rc_at: 0,
            short_page_ends: true,
            null_swap: None,
            breaker: Breaker::default(),
            budget: None,
            strikes: 0,
            cors: Cors::Unknown,
            cors_at: 0,
        }
    }
}

impl HostRow {
    /// `true` if `list_range` should try `RANGE` mode at `now` (§5.3): anything but a `No` set
    /// less than 7 days ago.
    pub fn use_range(&self, now_ms: i64) -> bool {
        !(self.range_cursor == RangeCursor::No
            && now_ms.saturating_sub(self.rc_at) < RANGE_CURSOR_RECHECK_MS)
    }

    /// `set Yes`: only from `Unknown` (§5.3).
    pub fn set_range_yes(&mut self, now_ms: i64) {
        if self.range_cursor == RangeCursor::Unknown {
            self.range_cursor = RangeCursor::Yes;
            self.rc_at = now_ms;
        }
    }

    /// `set No` (§5.3).
    pub fn set_range_no(&mut self, now_ms: i64) {
        self.range_cursor = RangeCursor::No;
        self.rc_at = now_ms;
    }

    /// Counts an admission strike.
    pub fn strike(&mut self) {
        self.strikes = self.strikes.saturating_add(1);
    }

    /// Records a cross-origin observation.
    pub fn set_cors(&mut self, allowed: bool, now_ms: i64) {
        self.cors = if allowed {
            Cors::Allowed
        } else {
            Cors::Blocked
        };
        self.cors_at = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::row;

    fn cid() -> Cid {
        lessdugin_atp::cid::for_dag_cbor(b"\xa0")
    }

    #[test]
    fn repo_status_rules() {
        let mut r = RepoRow::new("https://pds.example");
        assert_eq!(r.repo_status("fk"), RepoStatus::Unavailable);
        r.observe_head("3l2a", cid(), 10);
        r.apply_stable_pass("3l2a", cid(), [("fk", BTreeMap::new())]);
        assert_eq!(r.repo_status("fk"), RepoStatus::Fresh);
        assert_eq!(r.repo_status("other"), RepoStatus::Unavailable);
        // a newer head makes it stale
        r.observe_head("3l2b", cid(), 20);
        assert_eq!(r.repo_status("fk"), RepoStatus::Stale);
        // an older observation (by send time) does not replace the head
        r.observe_head("3l29", cid(), 15);
        assert_eq!(r.head_seen.as_ref().unwrap().rev, "3l2b");
        r.apply_stable_pass("3l2b", cid(), [("fk", BTreeMap::new())]);
        assert_eq!(r.repo_status("fk"), RepoStatus::Fresh);
        // a mark above the listed rev holds it below Fresh
        assert!(r.add_mark("at://x/y/z", "3l2c"));
        assert_eq!(r.latest_known(), "3l2c");
        assert_eq!(r.repo_status("fk"), RepoStatus::Stale);
        r.apply_stable_pass("3l2c", cid(), [("fk", BTreeMap::new())]);
        assert!(r.marks.is_empty());
        assert_eq!(r.repo_status("fk"), RepoStatus::Fresh);
        // byte cap
        r.set_byte_capped("fk", true);
        assert_eq!(r.repo_status("fk"), RepoStatus::Stale);
        r.reset_scope("fk");
        assert_eq!(r.repo_status("fk"), RepoStatus::Unavailable);
    }

    #[test]
    fn marks_cap() {
        let mut r = RepoRow::new("https://pds.example");
        for i in 0..limits::REPO_MARKS_MAX {
            assert!(r.add_mark(&format!("at://u/{i}"), "3l2a"));
        }
        assert!(r.add_mark("at://u/0", "3l2b")); // existing uri: raised, no overflow
        assert_eq!(r.marks["at://u/0"], "3l2b");
        assert!(!r.add_mark("at://u/new", "3l2a"));
        assert!(r.marks_overflow);
        r.clear_marks_upto("3l2a");
        assert_eq!(r.marks.len(), 1);
    }

    #[test]
    fn backoff_and_class() {
        let mut r = RepoRow::new("https://pds.example");
        r.fail(1_000, 0);
        assert_eq!(r.next_check, 1_000 + 30_000 / 1000 * 800);
        r.fail(1_000, 400);
        assert_eq!(r.next_check, 1_000 + 60_000 / 1000 * 1200);
        for _ in 0..40 {
            r.fail(0, 200);
        }
        assert_eq!(r.next_check, REPO_BACKOFF_MAX_MS);
        r.succeed(5);
        assert_eq!(r.fails, 0);
        assert_eq!(r.class_at(10, true), RepoClass::Authority);
        assert_eq!(r.class_at(10, false), RepoClass::Cold);
        r.last_change_at = 10;
        assert_eq!(r.class_at(10 + HOT_MS, false), RepoClass::Hot);
        assert_eq!(r.class_at(11 + HOT_MS, false), RepoClass::Warm);
        assert_eq!(r.class_at(11 + WARM_MS, false), RepoClass::Cold);
    }

    #[test]
    fn host_range_cursor() {
        let mut h = HostRow::default();
        assert!(h.use_range(0));
        h.set_range_yes(5);
        assert_eq!((h.range_cursor, h.rc_at), (RangeCursor::Yes, 5));
        h.set_range_yes(9); // only from Unknown
        assert_eq!(h.rc_at, 5);
        h.set_range_no(100);
        assert!(!h.use_range(100 + RANGE_CURSOR_RECHECK_MS - 1));
        assert!(h.use_range(100 + RANGE_CURSOR_RECHECK_MS));
        h.set_range_yes(200); // No stays No until re-probed and a violation-free probe sets…
        assert_eq!(h.range_cursor, RangeCursor::No);
        h.set_cors(false, 7);
        assert_eq!(h.cors, Cors::Blocked);
    }

    #[test]
    fn rows_roundtrip() {
        let mut r = RepoRow::new("https://pds.example");
        r.observe_head("3l2a", cid(), 10);
        r.capped.insert(
            "fk".into(),
            BTreeMap::from([("c".to_owned(), "fk-a".to_owned())]),
        );
        r.add_mark("at://a/b/c", "3l2a");
        r.rollback_since = Some(4);
        let bytes = row::encode(&r).unwrap();
        assert_eq!(row::decode::<RepoRow>(&bytes).unwrap(), r);
        let h = HostRow {
            budget: Some(Budget {
                limit: 3000,
                est: 10,
                reset_at: 5,
            }),
            null_swap: Some(true),
            ..HostRow::default()
        };
        assert_eq!(
            row::decode::<HostRow>(&row::encode(&h).unwrap()).unwrap(),
            h
        );
    }
}
