// SPDX-License-Identifier: AGPL-3.0-or-later
//! The dirty queue (`sync.md` §6.1): one item per DID, fed by every change source (Jetstream,
//! the indexer change feed, polling, reconcile, own writes, discovery, opening a view).
//!
//! **Coalescing.** Merging two items for a DID takes the highest priority (P0 highest), the
//! greatest `minRev`, the union of reasons and uris, and the earliest ready time.
//!
//! **Ready times** (debounce plus jitter, *tunable*): P0 `uniform(0, 5 s)`, P1 `uniform(2 s,
//! 30 s)`, P2 `uniform(30 s, 600 s)`, P3 at its scheduled time.
//!
//! **Dispatch** is by `(priority, ready time, did)`. An item that carries uris and no other
//! reason is served by point fetches (§5.7); any other item runs `sync_repo` for the DID's scopes.
//!
//! The queue is pure state: it never reads a clock (callers pass `now`) and takes randomness as
//! an argument.

use std::collections::{BTreeMap, BTreeSet};

use crate::sync::net::Priority;

/// Why a DID is queued (or synced).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// Point fetches of the item's uris only (§5.7).
    Point,
    /// An authority of an open forum (§4.2).
    Authority,
    /// A participant or confirmed candidate (§4.2.1).
    Participant,
    /// The head moved during a listing (§5.2).
    Unstable,
    /// An own write: a listing confirms it (§5.8).
    OwnWrite,
    /// A poll saw a new head (§6.7).
    Poll,
    /// The daily (participants) or hourly (authorities) reconcile (§6.8): `describeRepo` is
    /// refreshed.
    Reconcile,
    /// A hint (Jetstream, indexer change) named the repo (§6.4).
    Hint,
    /// A Jetstream `#sync` event: a full re-list (§6.4).
    Resync,
    /// A content view was opened (§6.7).
    View,
    /// `sync --full`: cached state is discarded, a rollback is accepted (§5.9).
    Full,
    /// The repo moved to another PDS (§3.3).
    Migration,
}

/// One queued DID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyItem {
    /// The repo.
    pub did: String,
    /// Sync at least up to this rev.
    pub min_rev: Option<String>,
    /// Priority.
    pub priority: Priority,
    /// Why.
    pub reasons: BTreeSet<Reason>,
    /// Uris to point-fetch.
    pub uris: BTreeSet<String>,
    /// Not before (ms).
    pub ready_at: i64,
}

impl DirtyItem {
    /// An item ready at `ready_at`.
    pub fn new(did: impl Into<String>, priority: Priority, reason: Reason, ready_at: i64) -> Self {
        Self {
            did: did.into(),
            min_rev: None,
            priority,
            reasons: BTreeSet::from([reason]),
            uris: BTreeSet::new(),
            ready_at,
        }
    }

    /// A point-fetch item for `uri`.
    pub fn point(did: impl Into<String>, uri: &str, priority: Priority, ready_at: i64) -> Self {
        let mut it = Self::new(did, priority, Reason::Point, ready_at);
        it.uris.insert(uri.to_owned());
        it
    }

    /// Sets `minRev`.
    pub fn with_min_rev(mut self, rev: impl Into<String>) -> Self {
        self.min_rev = Some(rev.into());
        self
    }

    /// Merges `other` into `self` (same DID).
    pub fn merge(&mut self, other: DirtyItem) {
        self.priority = self.priority.min(other.priority);
        self.min_rev = match (self.min_rev.take(), other.min_rev) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.reasons.extend(other.reasons);
        self.uris.extend(other.uris);
        self.ready_at = self.ready_at.min(other.ready_at);
    }

    /// What serving the item takes.
    pub fn action(&self) -> Action {
        if !self.uris.is_empty() && self.reasons.iter().all(|r| *r == Reason::Point) {
            Action::PointFetch(self.uris.iter().cloned().collect())
        } else {
            Action::Sync
        }
    }
}

/// How a queue item is served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Point fetches of these uris (§5.7).
    PointFetch(Vec<String>),
    /// `sync_repo` for the DID's scopes (any uris are covered by the listing).
    Sync,
}

/// The debounce window of a priority (ms, inclusive), `None` for P3 (its scheduled time).
pub fn debounce_window(p: Priority) -> Option<(i64, i64)> {
    match p {
        Priority::P0 => Some((0, 5_000)),
        Priority::P1 => Some((2_000, 30_000)),
        Priority::P2 => Some((30_000, 600_000)),
        Priority::P3 => None,
    }
}

/// `uniform(a, b)` from a random value.
pub fn uniform(a: i64, b: i64, random: u64) -> i64 {
    if b <= a {
        return a;
    }
    let span = (b - a) as u64 + 1;
    a + (random % span) as i64
}

/// The ready time of a new item at `priority` (`scheduled` for P3, or to delay any priority).
pub fn ready_time(priority: Priority, now_ms: i64, random: u64, scheduled: Option<i64>) -> i64 {
    match (debounce_window(priority), scheduled) {
        (_, Some(at)) => at,
        (Some((a, b)), None) => now_ms + uniform(a, b, random),
        (None, None) => now_ms,
    }
}

/// The dirty queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirtyQueue {
    items: BTreeMap<String, DirtyItem>,
}

impl DirtyQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an item, coalescing with the DID's queued item.
    pub fn enqueue(&mut self, item: DirtyItem) {
        match self.items.get_mut(&item.did) {
            Some(cur) => cur.merge(item),
            None => {
                self.items.insert(item.did.clone(), item);
            }
        }
    }

    /// Number of queued DIDs.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` if nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The queued item of a DID.
    pub fn get(&self, did: &str) -> Option<&DirtyItem> {
        self.items.get(did)
    }

    /// Every item, in dispatch order.
    pub fn ordered(&self) -> Vec<&DirtyItem> {
        let mut v: Vec<&DirtyItem> = self.items.values().collect();
        v.sort_by(|a, b| (a.priority, a.ready_at, &a.did).cmp(&(b.priority, b.ready_at, &b.did)));
        v
    }

    /// Removes and returns the next ready item: the smallest `(priority, ready time, did)` among
    /// items with `ready_at ≤ now`.
    pub fn pop_ready(&mut self, now_ms: i64) -> Option<DirtyItem> {
        let did = self
            .items
            .values()
            .filter(|i| i.ready_at <= now_ms)
            .min_by(|a, b| (a.priority, a.ready_at, &a.did).cmp(&(b.priority, b.ready_at, &b.did)))?
            .did
            .clone();
        self.items.remove(&did)
    }

    /// The earliest ready time, if anything is queued.
    pub fn next_ready_at(&self) -> Option<i64> {
        self.items.values().map(|i| i.ready_at).min()
    }

    /// Removes a DID's item.
    pub fn remove(&mut self, did: &str) -> Option<DirtyItem> {
        self.items.remove(did)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescing() {
        let mut q = DirtyQueue::new();
        q.enqueue(DirtyItem::new("did:a", Priority::P2, Reason::Hint, 500).with_min_rev("3l2b"));
        q.enqueue(
            DirtyItem::point("did:a", "at://did:a/c/k", Priority::P0, 900).with_min_rev("3l2a"),
        );
        q.enqueue(DirtyItem::new("did:b", Priority::P1, Reason::Poll, 100));
        assert_eq!(q.len(), 2);
        let a = q.get("did:a").unwrap();
        assert_eq!(a.priority, Priority::P0);
        assert_eq!(a.min_rev.as_deref(), Some("3l2b"));
        assert_eq!(a.ready_at, 500);
        assert_eq!(a.reasons, BTreeSet::from([Reason::Point, Reason::Hint]));
        assert_eq!(a.action(), Action::Sync);
        // not ready yet
        assert!(q.pop_ready(99).is_none());
        assert_eq!(q.pop_ready(100).unwrap().did, "did:b");
        assert_eq!(q.next_ready_at(), Some(500));
        assert_eq!(q.pop_ready(1_000).unwrap().did, "did:a");
        assert!(q.is_empty());
    }

    #[test]
    fn dispatch_order_and_actions() {
        let mut q = DirtyQueue::new();
        q.enqueue(DirtyItem::new("did:z", Priority::P1, Reason::Poll, 10));
        q.enqueue(DirtyItem::new("did:y", Priority::P1, Reason::Poll, 10));
        q.enqueue(DirtyItem::new("did:x", Priority::P2, Reason::Poll, 0));
        q.enqueue(DirtyItem::point(
            "did:w",
            "at://did:w/c/k",
            Priority::P0,
            20,
        ));
        let order: Vec<&str> = q.ordered().iter().map(|i| i.did.as_str()).collect();
        assert_eq!(order, ["did:w", "did:y", "did:z", "did:x"]);
        assert_eq!(
            q.get("did:w").unwrap().action(),
            Action::PointFetch(vec!["at://did:w/c/k".into()])
        );
        assert_eq!(q.pop_ready(15).unwrap().did, "did:y");
    }

    #[test]
    fn ready_times() {
        for r in [0u64, 1, 4_999, 5_000, u64::MAX] {
            let t = ready_time(Priority::P0, 1_000, r, None);
            assert!((1_000..=6_000).contains(&t));
            let t = ready_time(Priority::P1, 1_000, r, None);
            assert!((3_000..=31_000).contains(&t));
            let t = ready_time(Priority::P2, 1_000, r, None);
            assert!((31_000..=601_000).contains(&t));
        }
        assert_eq!(
            ready_time(Priority::P3, 1_000, 7, Some(86_400_000)),
            86_400_000
        );
        assert_eq!(ready_time(Priority::P3, 1_000, 7, None), 1_000);
        assert_eq!(uniform(5, 5, 99), 5);
    }
}
