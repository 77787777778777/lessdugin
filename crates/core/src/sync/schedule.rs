// SPDX-License-Identifier: AGPL-3.0-or-later
//! Polling (`sync.md` §6.7) and reconcile (§6.8) schedules: pure functions of repo state, the
//! repo's role and `now` (randomness is an argument).
//!
//! | Repo class (§5.9) | Poll interval, ±20 % jitter |
//! |---|---|
//! | `authority` | 2 min |
//! | `hot` | 10 min |
//! | `warm` | 1 h |
//! | `cold` | 12 h |
//!
//! A poll is one `getLatestCommit`; a head that differs from `(repo.rev, repo.commit)` queues the
//! repo at P1 (P2 for `cold`). Opening a view head-checks, at P0, every author on screen whose
//! last head observation is more than 5 minutes old.
//!
//! Reconcile: every participant of a followed forum once per 24 h (`lastReconcile + 24 h ·
//! uniform(0.9, 1.1)`, P3, `describeRepo` refreshed); every authority hourly at P1; account
//! status daily.

use crate::store::repo::{RepoClass, RepoRow};
use crate::sync::net::Priority;
use crate::sync::queue::uniform;

/// `POLL_AUTHORITY_MS`.
pub const POLL_AUTHORITY_MS: i64 = 120_000;
/// `POLL_HOT_MS`.
pub const POLL_HOT_MS: i64 = 600_000;
/// `POLL_WARM_MS`.
pub const POLL_WARM_MS: i64 = 3_600_000;
/// `POLL_COLD_MS`.
pub const POLL_COLD_MS: i64 = 43_200_000;
/// A view-open head check is due for authors whose head was seen longer ago than this.
pub const VIEW_HEAD_MAX_AGE_MS: i64 = 300_000;
/// Participants are reconciled once per this period (`· uniform(0.9, 1.1)`).
pub const RECONCILE_PARTICIPANT_MS: i64 = 86_400_000;
/// Authorities of followed forums are reconciled this often.
pub const RECONCILE_AUTHORITY_MS: i64 = 3_600_000;
/// Account status is re-checked this often (§3.4).
pub const ACCOUNT_STATUS_MS: i64 = 86_400_000;

/// The poll interval of a class.
pub fn poll_interval(class: RepoClass) -> i64 {
    match class {
        RepoClass::Authority => POLL_AUTHORITY_MS,
        RepoClass::Hot => POLL_HOT_MS,
        RepoClass::Warm => POLL_WARM_MS,
        RepoClass::Cold => POLL_COLD_MS,
    }
}

/// `interval · uniform(0.8, 1.2)` (±20 %), in permille steps.
pub fn jittered(interval: i64, random: u64) -> i64 {
    interval / 1000 * uniform(800, 1200, random)
}

/// The last time the repo's head was looked at: a head observation or a sync attempt.
pub fn last_checked(repo: &RepoRow) -> i64 {
    repo.head_seen
        .as_ref()
        .map_or(0, |h| h.sent_at)
        .max(repo.checked_at)
}

/// When the next poll of a repo is due (`authority`: an authority of a forum open in a view).
pub fn next_poll_at(repo: &RepoRow, authority: bool, now_ms: i64, random: u64) -> i64 {
    let class = repo.class_at(now_ms, authority);
    let at = last_checked(repo).saturating_add(jittered(poll_interval(class), random));
    // the repo backoff (§9.4) also holds polls back
    at.max(repo.next_check)
}

/// A due poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollDue {
    /// The repo.
    pub did: String,
    /// Its class at `now`.
    pub class: RepoClass,
    /// When it was due.
    pub due_at: i64,
}

/// The polls due at `now`, by due time then DID. `repos` is `(did, row, authority)`;
/// `random(did)` gives the jitter of each repo (stable per DID and period, for example from a
/// hash of the DID and its last check, so repeated calls agree).
pub fn due_polls<'a>(
    repos: impl IntoIterator<Item = (&'a str, &'a RepoRow, bool)>,
    now_ms: i64,
    random: impl Fn(&str) -> u64,
) -> Vec<PollDue> {
    let mut out: Vec<PollDue> = repos
        .into_iter()
        .filter_map(|(did, row, authority)| {
            let due_at = next_poll_at(row, authority, now_ms, random(did));
            (due_at <= now_ms).then(|| PollDue {
                did: did.to_owned(),
                class: row.class_at(now_ms, authority),
                due_at,
            })
        })
        .collect();
    out.sort_by(|a, b| (a.due_at, &a.did).cmp(&(b.due_at, &b.did)));
    out
}

/// What a poll's head observation asks for: `None` if the head is the synced one, else the
/// priority of the sync (P1, or P2 for `cold`).
pub fn poll_followup(repo: &RepoRow, class: RepoClass) -> Option<Priority> {
    let head = repo.head_seen.as_ref()?;
    let same = head.rev == repo.rev && repo.commit.as_ref() == Some(&head.cid);
    if same {
        None
    } else if class == RepoClass::Cold {
        Some(Priority::P2)
    } else {
        Some(Priority::P1)
    }
}

/// The authors on screen whose head should be checked at P0 when a view opens (§6.7): no head
/// observation, or one older than 5 minutes.
pub fn view_head_checks<'a>(
    authors: impl IntoIterator<Item = (&'a str, Option<&'a RepoRow>)>,
    now_ms: i64,
) -> Vec<String> {
    authors
        .into_iter()
        .filter(|(_, r)| {
            r.and_then(|r| r.head_seen.as_ref())
                .is_none_or(|h| now_ms.saturating_sub(h.sent_at) > VIEW_HEAD_MAX_AGE_MS)
        })
        .map(|(d, _)| d.to_owned())
        .collect()
}

/// A repo's role in the reconcile schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileRole {
    /// An authority of a followed forum: hourly, P1.
    Authority,
    /// A participant (confirmed candidate) of a followed forum: daily, P3.
    Participant,
}

/// The next reconcile of a repo: `(ready time, priority)`.
pub fn next_reconcile(repo: &RepoRow, role: ReconcileRole, random: u64) -> (i64, Priority) {
    match role {
        ReconcileRole::Authority => (
            repo.last_reconcile.saturating_add(RECONCILE_AUTHORITY_MS),
            Priority::P1,
        ),
        ReconcileRole::Participant => (
            repo.last_reconcile.saturating_add(
                RECONCILE_PARTICIPANT_MS / 1000 * uniform(900, 1100, random),
            ),
            Priority::P3,
        ),
    }
}

/// `true` if the account status of a repo is due for its daily check (§3.4).
pub fn account_status_due(repo: &RepoRow, now_ms: i64) -> bool {
    now_ms.saturating_sub(repo.status_at) >= ACCOUNT_STATUS_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::repo::HOT_MS;

    fn cid() -> lessdugin_atp::Cid {
        lessdugin_atp::cid::for_dag_cbor(b"\xa0")
    }

    #[test]
    fn intervals_and_jitter() {
        assert_eq!(poll_interval(RepoClass::Authority), 120_000);
        assert_eq!(poll_interval(RepoClass::Cold), 43_200_000);
        assert_eq!(jittered(120_000, 0), 96_000);
        assert_eq!(jittered(120_000, 400), 144_000);
        assert_eq!(jittered(120_000, 200), 120_000);
    }

    #[test]
    fn polls_follow_classes() {
        let now = 10 * 86_400_000;
        let mut hot = RepoRow::new("https://p");
        hot.last_change_at = now - 1_000;
        hot.observe_head("3l2a", cid(), now - 700_000);
        let mut cold = RepoRow::new("https://p");
        cold.observe_head("3l2a", cid(), now - 700_000);
        let mut auth = RepoRow::new("https://p");
        auth.observe_head("3l2a", cid(), now - 130_000);
        let due = due_polls(
            [
                ("did:hot", &hot, false),
                ("did:cold", &cold, false),
                ("did:auth", &auth, true),
            ],
            now,
            |_| 200,
        );
        let dids: Vec<&str> = due.iter().map(|d| d.did.as_str()).collect();
        assert_eq!(dids, ["did:hot", "did:auth"]);
        assert_eq!(due[0].class, RepoClass::Hot);
        assert_eq!(due[1].class, RepoClass::Authority);
        // backoff holds polls back
        hot.next_check = now + 1;
        assert!(due_polls([("did:hot", &hot, false)], now, |_| 200).is_empty());
        assert!(HOT_MS > 0);
    }

    #[test]
    fn poll_followups_and_view_checks() {
        let mut r = RepoRow::new("https://p");
        assert_eq!(poll_followup(&r, RepoClass::Hot), None);
        r.observe_head("3l2b", cid(), 5);
        assert_eq!(poll_followup(&r, RepoClass::Hot), Some(Priority::P1));
        assert_eq!(poll_followup(&r, RepoClass::Cold), Some(Priority::P2));
        r.apply_stable_pass("3l2b", cid(), std::iter::empty());
        assert_eq!(poll_followup(&r, RepoClass::Cold), None);
        let checks = view_head_checks(
            [("did:a", Some(&r)), ("did:b", None)],
            5 + VIEW_HEAD_MAX_AGE_MS,
        );
        assert_eq!(checks, ["did:b"]);
        let checks = view_head_checks([("did:a", Some(&r))], 6 + VIEW_HEAD_MAX_AGE_MS);
        assert_eq!(checks, ["did:a"]);
    }

    #[test]
    fn reconcile_times() {
        let mut r = RepoRow::new("https://p");
        r.last_reconcile = 1_000;
        assert_eq!(
            next_reconcile(&r, ReconcileRole::Authority, 0),
            (1_000 + 3_600_000, Priority::P1)
        );
        let (at, p) = next_reconcile(&r, ReconcileRole::Participant, 0);
        assert_eq!((at, p), (1_000 + 77_760_000, Priority::P3));
        let (at, _) = next_reconcile(&r, ReconcileRole::Participant, 200);
        assert_eq!(at, 1_000 + 95_040_000);
        assert!(account_status_due(&r, ACCOUNT_STATUS_MS));
        r.status_at = 10;
        assert!(!account_status_due(&r, ACCOUNT_STATUS_MS));
    }
}
