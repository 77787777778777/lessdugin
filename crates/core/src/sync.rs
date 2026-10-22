// SPDX-License-Identifier: AGPL-3.0-or-later
//! Per-repo sync (L1, `sync.md` §5) and its scheduling (§6.1, §6.7, §6.8, §9).
//!
//! | Module | Contents |
//! |---|---|
//! | [`net`] | [`Net`]: concurrency limits, per-host budgets, breakers, PLC pacing; [`NetHost`] |
//! | [`list`] | `list_range`: the cursor-range probe, `FULL` fallback, fetch caps, safety caps (§5.3–§5.4) |
//! | [`repo`] | `sync_repo` (§5.2), point fetches (§5.7), own writes (§5.8), head checks and polls, account status, serving the queue (`run_ready`) |
//! | [`queue`] | the dirty queue with priorities and coalescing (§6.1) |
//! | [`schedule`] | polling and reconcile schedules (§6.7, §6.8): `due_polls`, `due_reconciles` |
//!
//! A [`Syncer`] holds the state shared by every job of an engine ([`SyncShared`]: the scheduler
//! and the dirty queue). Each job runs in a [`Session`]: a [`NetHost`] at the job's priority with
//! the command's request counters, and an identity resolver over it, so that PLC and DoH
//! requests are scheduled and counted like the rest.
//!
//! Repo locks are in-process ([`Net::repo_lock`]); the cross-process locks and the sync leader of
//! §10.6 belong to the host.
//!
//! Records enter the store only through admission (`crate::store::admit`) and the record store's
//! [`crate::store::Batch`]; repo state, host capabilities and the records of a pass are written
//! in one atomic batch.

pub mod list;
pub mod net;
pub mod queue;
pub mod repo;
pub mod schedule;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

pub use list::{ListMode, ListTuning, RangeListing};
pub use net::{Net, NetHost, NetLimits, Priority, Stats};
pub use queue::{Action, DirtyItem, DirtyQueue, Reason};
pub use repo::{
    MembershipCheck, OwnWrite, OwnWritesApplied, PointOutcome, RepoOutcome, RepoReport, Scope,
    SyncRequest,
};

use crate::engine::{EngineConfig, Platform, Sources};
use crate::host::Host;
use crate::identity::{IdentityConfig, IdentityError, Resolver};
use crate::store::StoreError;
use crate::xrpc::{Xrpc, XrpcCallError};

/// Sync settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncConfig {
    /// Identity resolution.
    pub identity: IdentityConfig,
    /// Concurrency limits.
    pub limits: NetLimits,
    /// Listing parameters (normative by default).
    pub tuning: ListTuning,
    /// `sources = assisted`.
    pub assisted: bool,
}

impl SyncConfig {
    /// The settings of an engine configuration.
    pub fn from_engine(c: &EngineConfig) -> Self {
        Self {
            identity: c.identity_config(),
            limits: match c.platform {
                Platform::Native => NetLimits::NATIVE,
                Platform::Browser => NetLimits::BROWSER,
            },
            tuning: ListTuning::default(),
            assisted: c.sources == Sources::Assisted,
        }
    }
}

/// Why a sync job failed.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SyncError {
    /// The DID could not be resolved to a document.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// The DID document names no usable PDS.
    #[error("{did} has no usable PDS: {why}")]
    NoPds {
        /// The DID.
        did: String,
        /// Why.
        why: String,
    },
    /// A request failed.
    #[error(transparent)]
    Xrpc(#[from] XrpcCallError),
    /// A listing failed.
    #[error(transparent)]
    List(#[from] list::ListError),
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A response failed admission (`sync.md` §5.5).
    #[error("admission: {0}")]
    Admission(String),
    /// The DID's identity is only a hint (`sync.md` §1.5): its PDS can neither confirm nor
    /// reject a membership, so a candidate stays a candidate.
    #[error("the identity of {did} is only a hint: its membership cannot be confirmed")]
    IdentityHint {
        /// The DID.
        did: String,
    },
}

impl SyncError {
    /// A short code for `SyncIssue.code`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Identity(_) => "identity",
            Self::NoPds { .. } => "no_pds",
            Self::Xrpc(e) => xrpc_code(e),
            Self::List(list::ListError::Xrpc(e)) => xrpc_code(e),
            Self::List(_) => "listing",
            Self::Store(_) => "store",
            Self::Admission(_) => "admission",
            Self::IdentityHint { .. } => "identity_hint",
        }
    }

    /// `true` for a request this engine refused to send (`Blocked`: a request cap, a time
    /// budget, an open breaker, a policy): not the repo's fault, so no repo backoff follows.
    pub fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::Xrpc(XrpcCallError::Blocked(_))
                | Self::List(list::ListError::Xrpc(XrpcCallError::Blocked(_)))
                | Self::Identity(IdentityError::Fetch {
                    error: XrpcCallError::Blocked(_),
                    ..
                })
        )
    }

    /// `true` for transient failures (network, timeout, 5xx, rate limits).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Xrpc(e) | Self::List(list::ListError::Xrpc(e)) => e.is_transient(),
            Self::Identity(IdentityError::Fetch { error, .. }) => error.is_transient(),
            _ => false,
        }
    }
}

fn xrpc_code(e: &XrpcCallError) -> &'static str {
    match e {
        XrpcCallError::RateLimited { .. } => "rate_limited",
        XrpcCallError::Blocked(_) => "blocked",
        XrpcCallError::TooLarge => "too_large",
        e if e.is_repo_status() => "repo_status",
        _ => "network",
    }
}

/// State shared by every sync job of an engine.
pub struct SyncShared {
    config: SyncConfig,
    net: Arc<Net>,
    queue: Mutex<DirtyQueue>,
    /// First-seen times learned from hints, applied when the version is admitted (§5.6.5):
    /// `(uri, cid) → µs`.
    hints: Mutex<BTreeMap<(String, String), i64>>,
}

impl std::fmt::Debug for SyncShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncShared")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Hint first-seen times kept in memory at most (oldest dropped first).
pub const HINT_FIRST_SEEN_MAX: usize = 10_000;

impl SyncShared {
    /// Shared state for `config`.
    pub fn new(config: SyncConfig) -> Self {
        let net = Net::new(
            config.limits,
            Some(&config.identity.plc),
            &config.identity.doh,
        );
        Self {
            config,
            net: Arc::new(net),
            queue: Mutex::new(DirtyQueue::new()),
            hints: Mutex::new(BTreeMap::new()),
        }
    }

    /// The settings.
    pub fn config(&self) -> &SyncConfig {
        &self.config
    }

    /// The scheduler.
    pub fn net(&self) -> &Arc<Net> {
        &self.net
    }

    fn queue(&self) -> MutexGuard<'_, DirtyQueue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hints(&self) -> MutexGuard<'_, BTreeMap<(String, String), i64>> {
        self.hints.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The sync entry point of an engine (cheap to clone).
pub struct Syncer<H: Host> {
    host: Arc<H>,
    shared: Arc<SyncShared>,
}

impl<H: Host> Clone for Syncer<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<H: Host> Syncer<H> {
    /// A syncer with fresh shared state.
    pub fn new(host: Arc<H>, config: SyncConfig) -> Self {
        Self::with_shared(host, Arc::new(SyncShared::new(config)))
    }

    /// A syncer over existing shared state.
    pub fn with_shared(host: Arc<H>, shared: Arc<SyncShared>) -> Self {
        Self { host, shared }
    }

    /// The host.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// The shared state.
    pub fn shared(&self) -> &Arc<SyncShared> {
        &self.shared
    }

    /// The settings.
    pub fn config(&self) -> &SyncConfig {
        &self.shared.config
    }

    /// The scheduler.
    pub fn net(&self) -> &Net {
        &self.shared.net
    }

    /// A job context at `prio`, counting into `stats`.
    pub fn session(&self, prio: Priority, stats: Arc<Stats>) -> Session<H> {
        let nh = Arc::new(NetHost::new(
            Arc::clone(&self.host),
            Arc::clone(&self.shared.net),
            prio,
            stats,
        ));
        let resolver = Resolver::new(Arc::clone(&nh), self.shared.config.identity.clone());
        Session { host: nh, resolver }
    }

    /// Queues a DID (§6.1), coalescing with its queued item.
    pub fn enqueue(&self, item: DirtyItem) {
        self.shared.queue().enqueue(item);
    }

    /// A copy of the dirty queue.
    pub fn queue_snapshot(&self) -> DirtyQueue {
        self.shared.queue().clone()
    }

    /// Removes and returns the next ready queue item.
    pub fn pop_ready(&self, now_ms: i64) -> Option<DirtyItem> {
        self.shared.queue().pop_ready(now_ms)
    }

    /// Records a first-seen time from a hint (Jetstream commit, indexer `#recordRef`), applied
    /// when `(uri, cid)` is admitted (§5.6.5). Only the earliest time is kept.
    pub fn note_hint_first_seen(&self, uri: &str, cid: &str, t_us: i64) {
        let mut h = self.shared.hints();
        let e = h.entry((uri.to_owned(), cid.to_owned())).or_insert(t_us);
        *e = (*e).min(t_us);
        while h.len() > HINT_FIRST_SEEN_MAX {
            let Some(k) = h.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone()) else {
                break;
            };
            h.remove(&k);
        }
    }

    /// The hint first-seen time of `(uri, cid)`, if any.
    pub fn hint_first_seen(&self, uri: &str, cid: &str) -> Option<i64> {
        self.shared
            .hints()
            .get(&(uri.to_owned(), cid.to_owned()))
            .copied()
    }

    fn random(&self) -> u64 {
        let mut b = [0u8; 8];
        self.host.fill_random(&mut b);
        u64::from_le_bytes(b)
    }
}

/// One job's view of the network: a [`NetHost`] at the job's priority and an identity resolver
/// over it.
pub struct Session<H: Host> {
    host: Arc<NetHost<H>>,
    resolver: Resolver<NetHost<H>>,
}

impl<H: Host> Session<H> {
    /// The scheduled host.
    pub fn host(&self) -> &NetHost<H> {
        &self.host
    }

    /// An XRPC client over the scheduled host.
    pub fn xrpc(&self) -> Xrpc<'_, NetHost<H>> {
        Xrpc::new(&self.host)
    }

    /// The identity resolver (its requests are scheduled and counted too).
    pub fn resolver(&self) -> &Resolver<NetHost<H>> {
        &self.resolver
    }

    /// The command's counters.
    pub fn stats(&self) -> &Arc<Stats> {
        self.host.stats()
    }

    /// The priority.
    pub fn priority(&self) -> Priority {
        self.host.priority()
    }
}
