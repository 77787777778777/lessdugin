// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure Jetstream relevance filtering and the shared bounded LRU deduplicator.

use std::collections::BTreeMap;

use lessdugin_lexicon::collection::{Collection, nsid};

use crate::sync::Priority;

use super::source::{Cursor, DEDUP_CAPACITY};
use super::wire::{Event, EventKind, InfoKind, Wire};

/// Commit operation (part of the deduplication key).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    /// New record.
    Create,
    /// Replacement.
    Update,
    /// Removal.
    Delete,
}

/// Why a full reconcile is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    /// `OutdatedCursor` or a persisted cursor older than 36 hours.
    OutdatedCursor,
    /// `FutureCursor`.
    FutureCursor,
    /// Two consecutive oversized messages at one cursor.
    OversizedMessage,
}

/// Work for an integrator. No variant carries Jetstream record text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Fetch this URI at the author's PDS; `first_seen_us` is only an observation hint.
    PointFetch {
        /// Repo.
        did: String,
        /// Canonical record URI.
        uri: String,
        /// Claimed repo revision.
        rev: String,
        /// Claimed CID, absent for a delete.
        cid: Option<String>,
        /// Queue priority.
        priority: Priority,
        /// `obs` hint.
        first_seen_us: i64,
    },
    /// A previously unknown DID wrote an exact membership key in `F_new`.
    Candidate {
        /// Candidate repo.
        did: String,
        /// Forum key.
        fk: String,
        /// Membership URI (still only a hint).
        uri: String,
        /// Claimed revision.
        rev: String,
        /// Claimed CID, absent for a delete.
        cid: Option<String>,
        /// `obs` hint.
        first_seen_us: i64,
    },
    /// Drop DID/handle caches. Authorities also need a P1 refresh.
    InvalidateIdentity {
        /// Repo.
        did: String,
        /// Whether this DID is an authority of a followed/open forum.
        authority: bool,
    },
    /// Refresh `getRepoStatus` at P1.
    AccountStatus {
        /// Repo.
        did: String,
        /// Hint only; the PDS response remains authoritative.
        active: bool,
        /// Optional upstream status hint.
        status: Option<String>,
    },
    /// Full re-list of one repo.
    Resync {
        /// Repo.
        did: String,
    },
    /// P2 catch-up for a DID added to the browser filter.
    CatchUp {
        /// Repo.
        did: String,
    },
    /// Reconcile every relevant repo and restart the cursor near now.
    Reconcile(Gap),
}

/// Cheap, synchronously supplied filter state. The integrator can populate these sets from its
/// existing model/cache snapshot without resolving identities or doing other async work.
#[derive(Clone, Debug)]
pub struct Context {
    /// DIDs with an identity row.
    pub known_dids: std::collections::BTreeSet<String>,
    /// Logged-in account DIDs.
    pub own_dids: std::collections::BTreeSet<String>,
    /// Confirmed newcomer/candidate DIDs already being synced.
    pub newcomer_dids: std::collections::BTreeSet<String>,
    /// `fk` of followed/open forums.
    pub forums: std::collections::BTreeSet<String>,
    /// Open-mode forums, plus forums where the viewer is an authority.
    pub newcomer_forums: std::collections::BTreeSet<String>,
    /// Exact followed forum declarations `(owner DID, declaration rkey)`.
    pub declarations: std::collections::BTreeSet<(String, String)>,
    /// DIDs that are authorities (identity changes are refreshed at P1).
    pub authorities: std::collections::BTreeSet<String>,
    /// Exact collections accepted as `OURS`.
    pub ours: std::collections::BTreeSet<String>,
    /// Browser DID-filter input (own + authorities + participants + candidates).
    pub filter_dids: Vec<String>,
    /// True only after every followed/open forum completed a discovery pass.
    pub participants_known: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            known_dids: Default::default(),
            own_dids: Default::default(),
            newcomer_dids: Default::default(),
            forums: Default::default(),
            newcomer_forums: Default::default(),
            declarations: Default::default(),
            authorities: Default::default(),
            ours: Collection::ALL
                .into_iter()
                .map(|c| c.nsid().to_owned())
                .collect(),
            filter_dids: Default::default(),
            participants_known: false,
        }
    }
}

impl Context {
    fn known(&self, did: &str) -> bool {
        self.known_dids.contains(did)
            || self.own_dids.contains(did)
            || self.newcomer_dids.contains(did)
    }

    fn relevant(&self, did: &str, collection: &str, rkey: &str) -> bool {
        self.own_dids.contains(did)
            || self
                .forums
                .iter()
                .any(|fk| rkey == fk || rkey.strip_prefix(fk).is_some_and(|s| s.starts_with('-')))
            || (collection == nsid::FORUM
                && self
                    .declarations
                    .contains(&(did.to_owned(), rkey.to_owned())))
            || (collection == nsid::PROFILE && self.known(did))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DedupKey {
    Commit(String, String, String, String, Operation),
    Other(String, &'static str, u64),
}

/// One 10,000-entry LRU shared by v1 and v2. Replays across either wire cannot duplicate work.
#[derive(Clone, Debug)]
pub struct Dedup {
    capacity: usize,
    tick: u64,
    entries: BTreeMap<DedupKey, u64>,
}

impl Default for Dedup {
    fn default() -> Self {
        Self::new(DEDUP_CAPACITY)
    }
}

impl Dedup {
    /// An LRU holding at most `capacity` keys (a zero capacity is treated as one).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            tick: 0,
            entries: BTreeMap::new(),
        }
    }

    /// Number of retained keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Is the cache empty?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns false for a duplicate; a hit is promoted.
    fn insert(&mut self, key: DedupKey) -> bool {
        self.tick = self.tick.wrapping_add(1);
        if let Some(t) = self.entries.get_mut(&key) {
            *t = self.tick;
            return false;
        }
        while self.entries.len() >= self.capacity {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, t)| **t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, self.tick);
        true
    }
}

/// Result of filtering one valid event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterOutput {
    /// Typed work in wire order (currently at most one action per event).
    pub actions: Vec<Action>,
    /// Cursor advanced by this event, including valid but irrelevant network noise.
    pub cursor: Option<Cursor>,
    /// Replace any persisted cursor with `now - 5s` before reconnecting.
    pub restart_cursor: bool,
    /// The event was an actionable duplicate.
    pub duplicate: bool,
}

/// Pure event filter plus shared deduplication state.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    dedup: Dedup,
    pending: Option<(Wire, String, Event, FilterOutput, Context)>,
}

impl EventFilter {
    /// A filter with a custom dedup bound (useful for small tests).
    pub fn with_dedup_capacity(capacity: usize) -> Self {
        Self {
            dedup: Dedup::new(capacity),
            pending: None,
        }
    }

    /// Shared dedup state.
    pub fn dedup(&self) -> &Dedup {
        &self.dedup
    }

    /// Filters one parsed event. Invalid events never reach this function.
    pub fn apply(&mut self, wire: Wire, instance: &str, e: Event, cx: &Context) -> FilterOutput {
        let mut out = FilterOutput {
            cursor: Some(Cursor {
                wire,
                instance: instance.to_owned(),
                seq: e.seq,
                time_us: e.time_us,
            }),
            ..FilterOutput::default()
        };
        if self.pending.is_some() {
            return FilterOutput::default();
        }
        let original = e.clone();
        if let EventKind::Info(info) = e.kind {
            out.cursor = None;
            out.restart_cursor = true;
            if info == InfoKind::OutdatedCursor {
                out.actions.push(Action::Reconcile(Gap::OutdatedCursor));
                self.pending = Some((wire, instance.to_owned(), original, out.clone(), cx.clone()));
            }
            return out;
        }
        let Some(did) = e.did else { return out };
        match e.kind {
            EventKind::Commit {
                collection,
                rkey,
                rev,
                operation,
                cid,
            } => {
                if !cx.ours.contains(&collection) || !cx.relevant(&did, &collection, &rkey) {
                    return out;
                }
                let key = DedupKey::Commit(
                    did.clone(),
                    rev.clone(),
                    collection.clone(),
                    rkey.clone(),
                    operation,
                );
                if !self.dedup.insert(key) {
                    out.duplicate = true;
                    return out;
                }
                let uri = format!("at://{did}/{collection}/{rkey}");
                if !cx.known(&did) {
                    if collection == nsid::MEMBERSHIP && cx.newcomer_forums.contains(&rkey) {
                        out.actions.push(Action::Candidate {
                            did,
                            fk: rkey,
                            uri,
                            rev,
                            cid,
                            first_seen_us: e.time_us,
                        });
                    }
                    return out;
                }
                let priority = if collection == nsid::OPINION {
                    Priority::P2
                } else {
                    Priority::P1
                };
                out.actions.push(Action::PointFetch {
                    did,
                    uri,
                    rev,
                    cid,
                    priority,
                    first_seen_us: e.time_us,
                });
            }
            EventKind::Identity { inner_seq } => {
                if !cx.known(&did) {
                    return out;
                }
                if !self
                    .dedup
                    .insert(DedupKey::Other(did.clone(), "identity", inner_seq))
                {
                    out.duplicate = true;
                    return out;
                }
                out.actions.push(Action::InvalidateIdentity {
                    authority: cx.authorities.contains(&did),
                    did,
                });
            }
            EventKind::Account {
                inner_seq,
                active,
                status,
            } => {
                if !cx.known(&did) {
                    return out;
                }
                if !self
                    .dedup
                    .insert(DedupKey::Other(did.clone(), "account", inner_seq))
                {
                    out.duplicate = true;
                    return out;
                }
                out.actions.push(Action::AccountStatus {
                    did,
                    active,
                    status,
                });
            }
            EventKind::Sync => {
                if !cx.known(&did) {
                    return out;
                }
                if !self
                    .dedup
                    .insert(DedupKey::Other(did.clone(), "sync", e.seq.unwrap_or(0)))
                {
                    out.duplicate = true;
                    return out;
                }
                out.actions.push(Action::Resync { did });
            }
            EventKind::Info(_) => unreachable!("info returned above"),
        }
        if !out.actions.is_empty() {
            self.pending = Some((wire, instance.to_owned(), original, out.clone(), cx.clone()));
        }
        out
    }

    /// Retries the one actionable frame whose effects have not all completed. A duplicate wire
    /// delivery of it is held back until this explicit acknowledgement, never discarded by LRU.
    pub fn retry(&mut self) -> Option<(Wire, String, Event, Context, FilterOutput)> {
        let (wire, instance, event, out, cx) = self.pending.clone()?;
        Some((wire, instance, event, cx, out))
    }

    /// Marks every action of the pending frame applied, only for the consumer that produced it.
    /// Returns false for another endpoint's acknowledgement attempt.
    pub fn acknowledge_for(&mut self, wire: Wire, instance: &str) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|(w, i, ..)| *w == wire && i == instance)
        {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// Marks every action of the pending frame applied, permitting its cursor to advance.
    pub fn acknowledge(&mut self) {
        self.pending = None;
    }

    /// A failed frame must not allow a later frame to leapfrog its durable cursor.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}
