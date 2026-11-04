// SPDX-License-Identifier: AGPL-3.0-or-later
//! Long-lived work (`engine-api.md` §3.3): `live_tail`, `background_verify` and the background
//! verification of shown authors' handles.
//!
//! `live_tail` runs rounds ([`Engine::live_step`]) until [`Engine::live_stop`]. M2 is polling
//! mode (`sync.md` §6.7, §6.8); each round:
//!
//! 1. seeds the followed list of accounts that logged in since their last seeding (§4.8);
//! 2. polls the due heads (`getLatestCommit`) of the followed forums' authorities and
//!    participants and of the accounts' own repos; a moved head queues the repo (§6.7);
//! 3. queues the due reconciles (§6.8; a repo never synced is due at once);
//! 4. serves up to [`limits::LIVE_TAIL_BATCH`] ready dirty-queue items (§6.1): point fetches,
//!    and `sync_repo` over the repo's forum scopes (those it is relevant to, and those it was
//!    listed for), plus `OWN` for an account; the models of the forums that changed are dropped
//!    and `Changed` is emitted; an applied sync of an own repo re-seeds the followed list;
//! 5. verifies the handles of up to [`limits::HANDLE_VERIFY_BATCH`] shown authors (§3.3; the
//!    views queue them), emitting `Changed` when a handle state changed;
//! 6. drops expired tombstones, once per [`limits::TOMBSTONE_GC_EVERY_MS`];
//!
//! then sleeps (with the host) until the next queue item or poll is due, at most
//! [`limits::LIVE_TAIL_IDLE_MS`]. Jetstream (§6.4) and indexer `getChanges` (§6.6) feed the same
//! queue from M5 and M6: between rounds `live_tail` reads Jetstream frames (several per round;
//! while frames arrive a round runs at most once per [`LIVE_FRAME_STEP_MS`]) and, after a round,
//! polls the change feed of one due forum the consented indexer serves. A live frame whose
//! application fails is retried with back-off and then left to the dirty queue, so it never
//! holds the stream. Errors are logged and the round is retried; they are never returned.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use lessdugin_atp::syntax::{AtUri, Did};

use crate::discovery::state::disc_rows_over_cap;
use crate::dto::{Event, Topic};
use crate::host::{Dir, Host, KeyRange, KvOp, KvStore};
use crate::limits;
use crate::live::{
    Action as LiveAction, Context as LiveContext, LiveConfig, LiveSource, SourceStatus,
};
use crate::outbox;
use crate::store::RepoRow;
use crate::store::key::{self, Part};
use crate::store::records::RecordStore;
use crate::store::schema::{self, GLOBAL_SCOPE, scope_of};
use crate::sync::queue::ready_time;
use crate::sync::schedule::{self, ReconcileRole};
use crate::sync::{Action, DirtyItem, Priority, Reason, RepoOutcome, Scope, Stats};

use super::events::SubscriptionId;
use super::models::{Runtime, store_error};
use super::{Engine, EngineError, IndexerMode};

/// A live frame whose application fails is tried this many times in all, then acknowledged and
/// left to the dirty queue (`sync.md` §6.4, *tunable*).
const LIVE_APPLY_ATTEMPTS: u32 = 3;
/// The delay before the second try of a failed live frame, doubled for each later one (ms,
/// *tunable*).
const LIVE_RETRY_MS: i64 = 2_000;
/// While Jetstream frames arrive, a `live_step` round runs at most this often (ms).
const LIVE_FRAME_STEP_MS: i64 = 1_000;
/// The live filter context is rebuilt after a change at most this often (ms).
const CONTEXT_MIN_MS: i64 = 1_000;
/// The live filter context is rebuilt at least this often (ms; changes another engine makes to
/// the stores emit no event here).
const CONTEXT_TTL_MS: i64 = 30_000;
/// Indexer `getChanges` cadence per followed forum while its model is loaded (a view of it is
/// open) and otherwise (`sync.md` §6.6, ms, *tunable*).
const CHANGES_OPEN_MS: i64 = 10_000;
const CHANGES_IDLE_MS: i64 = 60_000;
/// A live source is healthy for this long after its last good frame or poll (`sync.md` §6.2).
const HEALTHY_MS: i64 = 120_000;
/// What one `live_tail` round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRound {
    /// Heads polled.
    pub polled: usize,
    /// Reconciles queued.
    pub reconciles: usize,
    /// `sync_repo` passes run.
    pub synced: usize,
    /// Forums whose cached state changed (`Changed` was emitted), by uri.
    pub changed: Vec<String>,
    /// Author handles verified.
    pub verified: usize,
    /// Expired tombstones dropped.
    pub gc: usize,
    /// When the next round is due (ms from now).
    pub next_ms: i64,
}

/// Why a repo is followed by `live_tail`.
#[derive(Clone, Debug, Default)]
struct Relevance {
    /// The forum keys it is synced for.
    fks: BTreeSet<String>,
    /// An authority of a followed forum.
    authority: bool,
    /// A logged-in account.
    own: bool,
}

/// A jitter seed for a repo, stable per DID and check (as `schedule::due_polls` asks).
fn jitter(did: &str, row: Option<&RepoRow>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in did.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h ^ row.map_or(0, |r| r.checked_at as u64)
}

/// The actionable live frame `live_tail` is applying.
#[derive(Default)]
struct LiveFrame {
    actions: Vec<LiveAction>,
    /// Failed tries so far.
    attempts: u32,
    /// The next try is not before this (ms).
    retry_at_ms: i64,
    /// Given up on: left to the dirty queue, only its acknowledgement is pending.
    deferred: bool,
}

/// The live filter context (`sync.md` §6.4), kept between frames: it is rebuilt after a
/// `Changed` event or one of `live_tail`'s own applied frames (at most once per
/// [`CONTEXT_MIN_MS`]), and at least every [`CONTEXT_TTL_MS`].
struct ContextCache {
    rt: Arc<Runtime>,
    sub: SubscriptionId,
    dirty: Arc<AtomicBool>,
    cx: Option<LiveContext>,
    built_ms: i64,
    retry_at_ms: i64,
}

impl ContextCache {
    fn new(rt: &Arc<Runtime>) -> Self {
        let dirty = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&dirty);
        let sub = rt.bus.subscribe(Arc::new(move |e: Event| {
            if matches!(e, Event::Changed { .. }) {
                flag.store(true, Ordering::SeqCst);
            }
        }));
        Self {
            rt: Arc::clone(rt),
            sub,
            dirty,
            cx: None,
            built_ms: 0,
            retry_at_ms: 0,
        }
    }

    fn invalidate(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// The context, rebuilt first if it is due. `None` if it was never built (a failed build
    /// keeps the previous one).
    async fn get<H: Host>(&mut self, engine: &Engine<H>) -> Option<&LiveContext> {
        let now = engine.host.now_ms();
        let age = now.saturating_sub(self.built_ms);
        let due = self.cx.is_none()
            || age >= CONTEXT_TTL_MS
            || (age >= CONTEXT_MIN_MS && self.dirty.load(Ordering::SeqCst));
        if due && now >= self.retry_at_ms {
            // cleared first, so a change made while it is built marks it again
            self.dirty.store(false, Ordering::SeqCst);
            match engine.live_context().await {
                Ok(cx) => {
                    self.cx = Some(cx);
                    self.built_ms = now;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "live tail: building the live context failed");
                    self.dirty.store(true, Ordering::SeqCst);
                    self.retry_at_ms = now.saturating_add(outbox::ERROR_WAIT_MS);
                }
            }
        }
        self.cx.as_ref()
    }
}

impl Drop for ContextCache {
    fn drop(&mut self) {
        self.rt.bus.unsubscribe(self.sub);
    }
}

/// When each followed forum's indexer change feed was polled and is polled next (`sync.md`
/// §6.6).
#[derive(Default)]
struct ChangeFeeds {
    /// Per forum uri: `(last poll, next poll)` (ms).
    polls: BTreeMap<String, (i64, i64)>,
    /// The last successful poll (ms): a healthy live source (§6.2).
    healthy_ms: Option<i64>,
    /// When the followed forums are read again (ms), to find newly followed ones and views
    /// opened since the last poll.
    refresh_at_ms: i64,
}

impl ChangeFeeds {
    fn healthy(&self, now_ms: i64) -> bool {
        self.healthy_ms
            .is_some_and(|t| now_ms.saturating_sub(t) < HEALTHY_MS)
    }

    /// When the next poll is due (ms).
    fn due_at(&self) -> i64 {
        self.polls
            .values()
            .map(|(_, next)| *next)
            .min()
            .unwrap_or(i64::MAX)
            .min(self.refresh_at_ms)
    }

    /// Of the `followed` forums (uris), the one whose poll is due first, if any; its poll is
    /// recorded now. A forum never polled is due at once; one whose view (`open`, its model is
    /// loaded) opened since its last poll is due [`CHANGES_OPEN_MS`] after that poll.
    fn take_due(&mut self, followed: &[String], open: &[String], now: i64) -> Option<String> {
        self.refresh_at_ms = now.saturating_add(CHANGES_OPEN_MS);
        self.polls.retain(|uri, _| followed.contains(uri));
        for uri in followed {
            self.polls
                .entry(uri.clone())
                .or_insert((i64::MIN, i64::MIN));
        }
        for (uri, (last, next)) in self.polls.iter_mut() {
            if open.contains(uri) {
                *next = (*next).min(last.saturating_add(CHANGES_OPEN_MS));
            }
        }
        let uri = self
            .polls
            .iter()
            .filter(|(_, (_, next))| *next <= now)
            .min_by_key(|(_, (_, next))| *next)
            .map(|(uri, _)| uri.clone())?;
        let every = if open.contains(&uri) {
            CHANGES_OPEN_MS
        } else {
            CHANGES_IDLE_MS
        };
        self.polls
            .insert(uri.clone(), (now, now.saturating_add(every)));
        Some(uri)
    }
}

impl<H: Host> Engine<H> {
    /// `live_tail()` (long-lived, `engine-api.md` §3.3): runs [`Engine::live_step`] rounds,
    /// reading Jetstream frames and polling indexer change feeds between them, until
    /// [`Engine::live_stop`]. Start it once (before or after login); the host drives the
    /// future. A `live_tail` started after a stop runs normally. It never returns an error.
    pub async fn live_tail(&self) -> Result<(), EngineError> {
        let generation = self.rt.live_gen.load(Ordering::SeqCst);
        let mut woken = self.rt.live_waker();
        let running = || self.rt.live_gen.load(Ordering::SeqCst) == generation;
        let mut source: Option<LiveSource<H>> = None;
        let mut proof_batch: Option<crate::indexer::LiveProofBatch> = None;
        let mut frame = LiveFrame::default();
        let mut contexts = ContextCache::new(&self.rt);
        let mut feeds = ChangeFeeds::default();
        let mut step_at = self.host.now_ms();
        let mut last_step_ms = i64::MIN;
        // A deferred frame's reconcile (§6.3), enqueued by the next round that can.
        let mut reconcile_due = false;
        let mut wait: i64 = 1;
        while running() {
            let assisted = match self.sources().await {
                Ok(s) => s == super::Sources::Assisted,
                Err(e) => {
                    tracing::warn!(error = %e, "live tail: reading the sources setting failed");
                    source.is_some()
                }
            };
            if !assisted && let Some(mut old) = source.take() {
                old.stop().await;
            }
            let sleep_ms = u64::try_from(wait.max(1)).unwrap_or(1);
            let mut actions = Vec::new();
            let mut received = false;
            if assisted && frame.actions.is_empty() {
                let live = source.get_or_insert_with(|| {
                    LiveSource::new(
                        self.shared_host(),
                        LiveConfig::new(
                            self.config.jetstream.clone(),
                            self.config.platform == super::Platform::Browser,
                        ),
                    )
                });
                let retry = live.retry_in_ms();
                // A failed context build keeps the last one; without any, frames wait.
                let cx = contexts.get(self).await;
                let next = async {
                    match cx {
                        // Do not let an immediately-ready Waiting result starve the stop waker
                        // while a fake host clock is parked during reconnect backoff.
                        Some(cx) if retry == 0 => Some(live.next(cx).await),
                        _ => futures::future::pending().await,
                    }
                };
                futures::pin_mut!(next);
                let sleep = self.host.sleep(sleep_ms);
                futures::pin_mut!(sleep);
                let wake = woken.next();
                futures::pin_mut!(wake);
                let timeout = futures::future::select(sleep, wake);
                futures::pin_mut!(timeout);
                // LiveSource::next is cancellation-safe: a timer that wins loses no frame.
                if let futures::future::Either::Left((Some(result), _)) =
                    futures::future::select(next, timeout).await
                {
                    received = true;
                    match result {
                        Ok(SourceStatus::Actions(a)) => actions = a,
                        Ok(SourceStatus::Stopped) => break,
                        Ok(SourceStatus::Waiting | SourceStatus::LeaderUnavailable) => {}
                        Err(e) => tracing::warn!(error = %e, "live source round failed"),
                    }
                }
            } else {
                let sleep = self.host.sleep(sleep_ms);
                futures::pin_mut!(sleep);
                futures::future::select(sleep, woken.next()).await;
            }
            if !running() {
                break;
            }
            if !actions.is_empty() {
                frame = LiveFrame {
                    actions,
                    ..LiveFrame::default()
                };
            }
            let mut applied = false;
            let now = self.host.now_ms();
            if !frame.actions.is_empty() && now >= frame.retry_at_ms {
                let has_proofs = !frame.deferred
                    && frame
                        .actions
                        .iter()
                        .any(|a| matches!(a, LiveAction::PointFetch { .. }));
                if has_proofs && proof_batch.as_ref().is_none_or(|b| b.is_empty()) {
                    let mut batch = crate::indexer::LiveProofBatch::new(now, self.live_random());
                    for action in &frame.actions {
                        if let LiveAction::PointFetch { uri, .. } = action
                            && let Ok(uri) = AtUri::parse(uri)
                        {
                            batch.push(uri);
                        }
                    }
                    proof_batch = Some(batch);
                }
                let due = proof_batch
                    .as_ref()
                    .is_none_or(|b| b.is_empty() || b.is_due(now));
                if due {
                    let done = if frame.deferred {
                        true
                    } else {
                        let result = if has_proofs {
                            self.apply_batched_live_actions(
                                frame.actions.clone(),
                                proof_batch.as_mut().expect("proof batch created"),
                            )
                            .await
                        } else {
                            self.apply_live_actions(frame.actions.clone()).await
                        };
                        match result {
                            Ok(()) => true,
                            Err(e) => {
                                frame.attempts = frame.attempts.saturating_add(1);
                                if frame.attempts >= LIVE_APPLY_ATTEMPTS {
                                    // The frame must not hold the stream: its repos go to the
                                    // dirty queue, which has its own retries and breakers.
                                    tracing::warn!(
                                        error = %e,
                                        attempts = frame.attempts,
                                        "live frame failed; left to the dirty queue"
                                    );
                                    if self.defer_live_actions(&frame.actions) {
                                        reconcile_due = true;
                                        step_at = now;
                                    }
                                    frame.deferred = true;
                                    true
                                } else {
                                    // The actionable frame stays unacknowledged. Its old cursor
                                    // and all live actions are replayed after the back-off.
                                    tracing::warn!(
                                        error = %e,
                                        attempts = frame.attempts,
                                        "applying live source actions failed"
                                    );
                                    frame.retry_at_ms = now.saturating_add(
                                        LIVE_RETRY_MS << (frame.attempts - 1).min(8),
                                    );
                                    proof_batch = None;
                                    false
                                }
                            }
                        }
                    };
                    if done {
                        let acknowledged = match &mut source {
                            Some(live) => match live.acknowledge().await {
                                Ok(()) => true,
                                Err(e) => {
                                    tracing::warn!(error = %e, "acknowledging live source actions failed");
                                    false
                                }
                            },
                            None => true,
                        };
                        if acknowledged {
                            frame = LiveFrame::default();
                            proof_batch = None;
                            contexts.invalidate();
                            applied = true;
                        } else {
                            frame.retry_at_ms = now.saturating_add(LIVE_RETRY_MS);
                        }
                    }
                }
            }
            let now = self.host.now_ms();
            if received {
                // Frames are read several per round: one round per second at most while they
                // arrive, so network noise does not run a round per frame.
                step_at = step_at.min(last_step_ms.saturating_add(LIVE_FRAME_STEP_MS));
            }
            if applied && let Some(t) = self.syncer().queue_snapshot().next_ready_at() {
                step_at = step_at.min(t);
            }
            if now >= step_at {
                last_step_ms = now;
                if reconcile_due {
                    match self.enqueue_gap_reconcile().await {
                        Ok(()) => reconcile_due = false,
                        Err(e) => {
                            tracing::warn!(error = %e, "live tail: deferred reconcile failed")
                        }
                    }
                }
                let polling = !assisted
                    || (source.as_ref().is_none_or(LiveSource::polling_enabled)
                        && !feeds.healthy(now));
                step_at = match self.live_step_with_polling(polling).await {
                    Ok(r) => {
                        if r.synced > 0 || !r.changed.is_empty() {
                            contexts.invalidate();
                        }
                        self.host.now_ms().saturating_add(r.next_ms)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "live tail round failed");
                        self.host.now_ms().saturating_add(outbox::ERROR_WAIT_MS)
                    }
                };
            }
            // After the round, so a slow indexer never delays the queue's work, and one forum
            // at a time, so it delays the next Jetstream read by one poll at most.
            if assisted && self.host.now_ms() >= feeds.due_at() {
                self.poll_change_feeds(&mut feeds, source.as_mut().map(LiveSource::dedup_mut))
                    .await;
                if let Some(t) = self.syncer().queue_snapshot().next_ready_at() {
                    step_at = step_at.min(t);
                }
            }
            let now = self.host.now_ms();
            let mut until = step_at;
            if !frame.actions.is_empty() {
                let mut at = frame.retry_at_ms;
                if let Some(batch) = &proof_batch
                    && !batch.is_empty()
                {
                    at = at.max(batch.flush_at_ms());
                }
                until = until.min(at);
            }
            if assisted {
                until = until.min(feeds.due_at());
                if let Some(live) = &source {
                    let retry = live.retry_in_ms();
                    if retry > 0 {
                        until = until.min(now.saturating_add(retry));
                    }
                }
                if contexts.cx.is_none() {
                    until = until.min(contexts.retry_at_ms);
                }
            }
            wait = until.saturating_sub(now).max(1);
        }
        if let Some(mut live) = source {
            live.stop().await;
        }
        Ok(())
    }

    /// Polls `getChanges` for the followed forum whose poll is due first (`sync.md` §6.6), one
    /// per round so Jetstream frames are read between polls; a successful poll makes the change
    /// feed a healthy live source. A `#change` that Jetstream already handled (`dedup`, the
    /// shared LRU) is skipped. Newcomer candidates the feed names are handed to discovery as a
    /// Jetstream candidate is (§6.4, §15.4). Without an indexer it does nothing.
    async fn poll_change_feeds(
        &self,
        feeds: &mut ChangeFeeds,
        dedup: Option<&mut crate::live::events::Dedup>,
    ) {
        let now = self.host.now_ms();
        if self.config.indexer_mode == IndexerMode::None {
            feeds.refresh_at_ms = i64::MAX;
            return;
        }
        let followed = match self.followed_keys().await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "live tail: reading the followed forums failed");
                feeds.refresh_at_ms = now.saturating_add(outbox::ERROR_WAIT_MS);
                return;
            }
        };
        let uris: Vec<String> = followed.iter().map(|k| k.uri.to_string()).collect();
        let Some(uri) = feeds.take_due(&uris, &self.rt.loaded_forums(), now) else {
            return;
        };
        let Some(key) = followed.into_iter().find(|k| k.uri.as_str() == uri) else {
            return;
        };
        match self.poll_indexer_changes_resolved(&key, dedup).await {
            Ok(Some(report)) => {
                feeds.healthy_ms = Some(self.host.now_ms());
                tracing::debug!(forum = %key.uri, changes = report.changes, "indexer changes");
                if !report.candidates.is_empty()
                    && let Err(e) = self.confirm_feed_candidates(&key, report.candidates).await
                {
                    // The rows stay: the next forum sync confirms them (§15.4).
                    tracing::warn!(forum = %key.uri, error = %e, "indexer newcomers not confirmed");
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::debug!(forum = %key.uri, error = %e, "indexer changes unavailable");
            }
        }
    }

    /// The newcomer candidates of a change feed poll (`sync.md` §6.6, §15.4), newest first: as
    /// for a Jetstream candidate (§6.4), each gets its discovery row (`hint:indexer`, at most
    /// `DISC_ROWS_MAX` rows for the forum, the oldest unconfirmed dropped first), and one scoped
    /// background discovery pass confirms them at their PDSes, within the newcomer budget. The
    /// feed was just polled and nothing was fetched for them, so the pass does not poll it again.
    async fn confirm_feed_candidates(
        &self,
        key: &crate::discovery::ForumKey,
        candidates: Vec<String>,
    ) -> Result<(), EngineError> {
        const SRC: &str = "hint:indexer";
        let fk = key.fk.as_str();
        let mut rows = crate::discovery::state::disc_rows(self.host.cache(), fk)
            .await
            .map_err(store_error)?;
        let now = self.host.now_ms();
        let mut dropped = BTreeSet::new();
        for did in &candidates {
            rows.entry(did.clone())
                .or_insert_with(|| crate::discovery::DiscRow::new(SRC, now))
                .add_src(SRC);
            for d in disc_rows_over_cap(&rows, did) {
                rows.remove(&d);
                dropped.insert(d);
            }
        }
        let mut ops = Vec::new();
        for did in &candidates {
            if let Some(row) = rows.get(did) {
                ops.push(KvOp::Put(
                    crate::discovery::state::disc_key(fk, did),
                    crate::store::row::encode(row).map_err(|e| store_error(e.into()))?,
                ));
            }
        }
        ops.extend(
            dropped
                .iter()
                .map(|d| KvOp::Delete(crate::discovery::state::disc_key(fk, d))),
        );
        self.host.cache().write(ops).await?;
        let opts = crate::discovery::SyncOpts {
            budget_ms: Some(30_000),
            max_requests: Some(64),
            newcomers: candidates
                .into_iter()
                .filter(|d| rows.contains_key(d))
                .map(|d| (d, "indexer".into()))
                .collect(),
            background: true,
            ..crate::discovery::SyncOpts::default()
        };
        self.sync_forum_resolved(key, opts).await?;
        Ok(())
    }

    /// A live frame given up on: every repo it names goes to the dirty queue for a sync of its
    /// scopes (the reconcile path, with its own retries and breakers). Returns whether it held
    /// a [`LiveAction::Reconcile`], which names every relevant repo: the caller keeps it due.
    fn defer_live_actions(&self, actions: &[LiveAction]) -> bool {
        let sy = self.syncer();
        let dids: BTreeSet<&str> = actions
            .iter()
            .filter_map(|a| match a {
                LiveAction::PointFetch { did, .. }
                | LiveAction::Candidate { did, .. }
                | LiveAction::InvalidateIdentity { did, .. }
                | LiveAction::AccountStatus { did, .. }
                | LiveAction::Resync { did }
                | LiveAction::CatchUp { did } => Some(did.as_str()),
                LiveAction::Reconcile(_) => None,
            })
            .collect();
        for did in dids {
            sy.enqueue(self.live_item(did, Priority::P1, Reason::Reconcile));
        }
        actions
            .iter()
            .any(|a| matches!(a, LiveAction::Reconcile(_)))
    }

    /// A gap's full reconcile (`sync.md` §6.3): every authority at P1, every other relevant repo
    /// but the own ones at P3, ready at once.
    async fn enqueue_gap_reconcile(&self) -> Result<(), EngineError> {
        let sy = self.syncer();
        for (did, relevance) in self.relevant_repos().await? {
            if relevance.authority {
                sy.enqueue(self.live_item(&did, Priority::P1, Reason::Reconcile));
            } else if !relevance.own {
                let mut item = self.live_item(&did, Priority::P3, Reason::Reconcile);
                item.ready_at = self.host.now_ms();
                sy.enqueue(item);
            }
        }
        Ok(())
    }

    /// Stops the running `live_tail`s (they return after their current round).
    pub fn live_stop(&self) {
        self.rt.live_stop();
    }

    /// `background_verify()` (`engine-api.md` §3.3): `verify = Proof` is Reserved (P2,
    /// `sync.md` §2.3), so in v1 it has nothing to do and returns at once.
    pub async fn background_verify(&self) -> Result<(), EngineError> {
        Ok(())
    }

    /// The repos `live_tail` keeps fresh: the authorities, participants and grants of the
    /// followed forums, their confirmed discovery candidates, and the accounts' own repos. A
    /// candidate that is not confirmed is not: hints reach a repo only through `confirm`, under
    /// its back-off and budget (`sync.md` §4.2.1, §4.4, §6.2).
    async fn relevant_repos(&self) -> Result<BTreeMap<String, Relevance>, EngineError> {
        let mut rel: BTreeMap<String, Relevance> = BTreeMap::new();
        for key in self.followed_keys().await? {
            for (did, row) in crate::discovery::state::disc_rows(self.host.cache(), &key.fk)
                .await
                .map_err(store_error)?
            {
                if row.state == crate::discovery::state::DiscState::Confirmed {
                    rel.entry(did).or_default().fks.insert(key.fk.clone());
                }
            }
            let l = match self.loaded(&key).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::debug!(forum = %key.uri, error = %e, "live tail: forum not loaded");
                    continue;
                }
            };
            let f = &l.model.view;
            for a in f.authorities.keys() {
                let r = rel.entry(a.clone()).or_default();
                r.fks.insert(key.fk.clone());
                r.authority = true;
            }
            for p in f.participation.p.iter().chain(&f.participation.g) {
                rel.entry(p.clone()).or_default().fks.insert(key.fk.clone());
            }
        }
        for a in self.accounts().await? {
            rel.entry(a.did).or_default().own = true;
        }
        Ok(rel)
    }

    /// Every actual DID-document cache row, including repos unrelated to the current fold.
    async fn known_identity_dids(&self) -> Result<BTreeSet<String>, EngineError> {
        let mut out = BTreeSet::new();
        let prefix = schema::prefix("id", &["did"]);
        let mut range = Some(KeyRange::prefix(&prefix));
        while let Some(r) = range.take() {
            let page = self
                .host
                .cache()
                .scan(r.clone(), Dir::Forward, 1_000)
                .await?;
            let full = page.len() == 1_000;
            for (k, _) in &page {
                let Ok(parts) = key::decode(k) else { continue };
                let Some(Part::Bytes(did)) = parts.get(2) else {
                    continue;
                };
                let Ok(did) = std::str::from_utf8(did) else {
                    continue;
                };
                if Did::parse(did).is_ok() {
                    out.insert(did.to_owned());
                }
            }
            if full && let Some((last, _)) = page.last() {
                range = Some(KeyRange {
                    start: std::ops::Bound::Excluded(last.clone()),
                    end: r.end,
                });
            }
        }
        Ok(out)
    }

    /// Builds live filtering state from all identity rows and the relevant repos. Fold
    /// completeness is not a substitute for having finished discovery: an incomplete fold may
    /// still have a known D set. D (the browser's DID filter) and the newcomers already synced
    /// are the relevant repos: rejected and unconfirmed hints are in neither (`sync.md` §6.2).
    async fn live_context(&self) -> Result<LiveContext, EngineError> {
        let rel = self.relevant_repos().await?;
        let followed = self.followed_keys().await?;
        let mut cx = LiveContext {
            known_dids: self.known_identity_dids().await?,
            ..LiveContext::default()
        };
        cx.known_dids.extend(rel.keys().cloned());
        cx.filter_dids.extend(rel.keys().cloned());
        for (did, relevance) in &rel {
            if relevance.own {
                cx.own_dids.insert(did.clone());
            } else if !relevance.authority {
                cx.newcomer_dids.insert(did.clone());
            }
            if relevance.authority {
                cx.authorities.insert(did.clone());
            }
            cx.forums.extend(relevance.fks.iter().cloned());
        }
        let mut complete = !followed.is_empty();
        for key in followed {
            cx.declarations
                .insert((key.owner.to_string(), key.slug.clone()));
            complete &= crate::discovery::state::discovery_complete(self.host.cache(), &key.fk)
                .await
                .map_err(store_error)?;
            if let Ok(loaded) = self.loaded(&key).await {
                let viewer_authority = cx
                    .own_dids
                    .iter()
                    .any(|did| loaded.model.view.authorities.contains_key(did));
                if loaded.model.view.policy.membership
                    == lessdugin_lexicon::values::MembershipMode::Open
                    || viewer_authority
                {
                    cx.newcomer_forums.insert(key.fk.clone());
                }
            }
        }
        cx.filter_dids.sort();
        cx.filter_dids.dedup();
        cx.participants_known = complete;
        Ok(cx)
    }

    fn live_random(&self) -> u64 {
        let mut b = [0u8; 8];
        self.host.fill_random(&mut b);
        u64::from_le_bytes(b)
    }

    fn live_item(&self, did: &str, priority: Priority, reason: Reason) -> DirtyItem {
        let now = self.host.now_ms();
        DirtyItem::new(
            did,
            priority,
            reason,
            ready_time(priority, now, self.live_random(), None),
        )
    }

    /// Applies one actionable frame after its live proof delay. Proof-backed point actions are
    /// grouped by forum and hydrated in <=25-URI requests. If no consented indexer advertises
    /// proofs, the exact original actions go through the ordinary PDS dirty queue. Any failed
    /// hydration or PDS fallback returns an error, so the caller cannot acknowledge the frame.
    async fn apply_batched_live_actions(
        &self,
        actions: Vec<LiveAction>,
        batch: &mut crate::indexer::LiveProofBatch,
    ) -> Result<(), EngineError> {
        let now = self.host.now_ms();
        while !batch.is_empty() {
            let _ = batch.take(now);
        }

        let mut ordinary = Vec::new();
        let mut grouped: BTreeMap<String, Vec<LiveAction>> = BTreeMap::new();
        for action in actions {
            let LiveAction::PointFetch { uri, .. } = &action else {
                ordinary.push(action);
                continue;
            };
            let parsed = AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
            let fk = scope_of(parsed.collection().as_str(), parsed.rkey().as_str());
            if let Some(forum) = self.rt.forum_of_fk(fk) {
                grouped.entry(forum).or_default().push(action);
            } else {
                ordinary.push(action);
            }
        }

        for (forum, actions) in grouped {
            let uris = actions
                .iter()
                .filter_map(|a| match a {
                    LiveAction::PointFetch { uri, .. } => AtUri::parse(uri).ok(),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            match self.hydrate_indexer_records(&forum, &uris).await? {
                Some(report)
                    if report.items.len() == uris.len()
                        && report.items.iter().all(|i| {
                            matches!(
                                i.status,
                                crate::indexer::ProofStatus::Admitted
                                    | crate::indexer::ProofStatus::Absent
                            ) || i.point_fetched
                        }) =>
                {
                    self.rt.bus.changed([Topic::Forum { uri: forum.clone() }]);
                }
                Some(report) => {
                    return Err(EngineError::internal(format!(
                        "live proof hydration and PDS fallback did not apply every action: {report:?}"
                    )));
                }
                None => ordinary.extend(actions),
            }
        }
        self.apply_live_actions(ordinary).await
    }

    /// Applies live hints to the existing dirty queue and identity/account APIs. Candidate rows
    /// remain candidates until the normal forum sync confirms their membership at the PDS.
    async fn apply_live_actions(&self, actions: Vec<LiveAction>) -> Result<(), EngineError> {
        if actions.is_empty() {
            return Ok(());
        }
        let sy = self.syncer();
        for action in actions {
            match action {
                LiveAction::PointFetch {
                    did,
                    uri,
                    rev,
                    cid,
                    priority,
                    first_seen_us,
                } => {
                    let parsed =
                        AtUri::parse(&uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
                    sy.note_observed_change(crate::sync::ObservedChange {
                        fk: crate::store::schema::scope_of(
                            parsed.collection().as_str(),
                            parsed.rkey().as_str(),
                        )
                        .to_owned(),
                        uri: uri.clone(),
                        did: did.clone(),
                        rev: rev.clone(),
                        cid: cid.clone(),
                        observed_us: self.host.now_ms().saturating_mul(1000),
                    });
                    if let Some(cid) = &cid {
                        sy.note_hint_first_seen(&uri, cid, first_seen_us);
                    }
                    let stats = Arc::new(Stats::capped(Some(8)));
                    let session = sy.session(priority, stats);
                    sy.mark_hint(&session, &parsed, &rev).await?;
                    let at = ready_time(priority, self.host.now_ms(), self.live_random(), None);
                    sy.enqueue(
                        DirtyItem::point(did, &uri, priority, at)
                            .with_min_rev(rev)
                            .with_mark_hint(&uri, cid),
                    );
                }
                LiveAction::Candidate {
                    did,
                    fk,
                    uri,
                    rev,
                    cid,
                    first_seen_us,
                } => {
                    sy.note_observed_change(crate::sync::ObservedChange {
                        fk: fk.clone(),
                        uri: uri.clone(),
                        did: did.clone(),
                        rev: rev.clone(),
                        cid: cid.clone(),
                        observed_us: self.host.now_ms().saturating_mul(1000),
                    });
                    let mut rows = crate::discovery::state::disc_rows(self.host.cache(), &fk)
                        .await
                        .map_err(store_error)?;
                    let now = self.host.now_ms();
                    let row = rows
                        .entry(did.clone())
                        .or_insert_with(|| crate::discovery::DiscRow::new("hint:jetstream", now));
                    row.add_src("hint:jetstream");
                    let mut ops = vec![KvOp::Put(
                        crate::discovery::state::disc_key(&fk, &did),
                        crate::store::row::encode(&*row).map_err(|e| store_error(e.into()))?,
                    )];
                    ops.extend(
                        disc_rows_over_cap(&rows, &did)
                            .into_iter()
                            .map(|d| KvOp::Delete(crate::discovery::state::disc_key(&fk, &d))),
                    );
                    self.host.cache().write(ops).await?;
                    if let Some(cid) = &cid {
                        sy.note_hint_first_seen(&uri, cid, first_seen_us);
                    }
                    let parsed =
                        AtUri::parse(&uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
                    let stats = Arc::new(Stats::capped(Some(8)));
                    let session = sy.session(Priority::P1, stats);
                    sy.mark_hint(&session, &parsed, &rev).await?;
                    let at = ready_time(Priority::P1, now, self.live_random(), None);
                    sy.enqueue(
                        DirtyItem::point(did.clone(), &uri, Priority::P1, at)
                            .with_min_rev(rev)
                            .with_mark_hint(&uri, cid),
                    );
                    // A point fetch admits the membership record but does not confirm the
                    // candidate. Only this scoped discovery pass may change its DiscState.
                    if let Some(forum) = self.rt.forum_of_fk(&fk) {
                        let opts = crate::discovery::SyncOpts {
                            budget_ms: Some(30_000),
                            max_requests: Some(64),
                            newcomers: vec![(did, "jetstream".into())],
                            background: true,
                            ..crate::discovery::SyncOpts::default()
                        };
                        self.sync_forum(&forum, &opts).await?;
                    }
                }
                LiveAction::InvalidateIdentity { did, authority } => {
                    self.invalidate_identity(&did).await?;
                    if authority {
                        sy.enqueue(self.live_item(&did, Priority::P1, Reason::Hint));
                    }
                }
                LiveAction::AccountStatus { did, .. } => {
                    let Ok(did) = Did::parse(&did) else { continue };
                    let stats = Arc::new(Stats::capped(None));
                    let session = sy.session(Priority::P1, stats);
                    sy.check_account_status(&session, &did).await?;
                }
                LiveAction::Resync { did } => {
                    sy.enqueue(self.live_item(&did, Priority::P1, Reason::Resync));
                }
                LiveAction::CatchUp { did } => {
                    sy.enqueue(self.live_item(&did, Priority::P2, Reason::Hint));
                }
                LiveAction::Reconcile(gap) => {
                    tracing::warn!(?gap, "Jetstream cursor gap: scheduling reconcile");
                    self.enqueue_gap_reconcile().await?;
                }
            }
        }
        Ok(())
    }

    /// One `live_tail` round (see the module docs); also for one-shot use (`tail --once`).
    pub async fn live_step(&self) -> Result<LiveRound, EngineError> {
        self.live_step_with_polling(true).await
    }

    async fn live_step_with_polling(&self, polling: bool) -> Result<LiveRound, EngineError> {
        let mut out = LiveRound::default();
        self.seed_pending().await?;
        let rel = self.relevant_repos().await?;
        let sy = self.syncer();
        let store = RecordStore::new(self.host.cache());
        let now = self.host.now_ms();

        // the rows of the relevant and the queued repos
        let queued = sy.queue_snapshot();
        let mut dids: BTreeSet<&str> = rel.keys().map(String::as_str).collect();
        dids.extend(queued.ordered().iter().map(|i| i.did.as_str()));
        let dids: Vec<&str> = dids.into_iter().collect();
        let rows: BTreeMap<&str, RepoRow> = dids
            .iter()
            .copied()
            .zip(store.repos(&dids).await.map_err(store_error)?)
            .filter_map(|(d, r)| Some((d, r?)))
            .collect();

        // polls and reconciles
        let stats = Arc::new(Stats::capped(None));
        let s = sy.session(Priority::P1, Arc::clone(&stats));
        let polls = if polling {
            schedule::due_polls(
                rel.iter()
                    .filter_map(|(d, r)| Some((d.as_str(), rows.get(d.as_str())?, r.authority))),
                now,
                |d| jitter(d, rows.get(d)),
            )
        } else {
            Vec::new()
        };
        out.polled = polls.len();
        for (did, res) in sy.run_polls(&s, &polls).await {
            if let Err(e) = res {
                tracing::debug!(did = %did, error = %e, "live tail: poll failed");
            }
        }
        let reconciles = schedule::due_reconciles(
            rel.iter().map(|(d, r)| {
                let role = if r.authority {
                    ReconcileRole::Authority
                } else {
                    ReconcileRole::Participant
                };
                (d.as_str(), rows.get(d.as_str()), role)
            }),
            now,
            |d| jitter(d, rows.get(d)),
        );
        out.reconciles = reconciles.len();
        for item in reconciles {
            sy.enqueue(item);
        }

        // the dirty queue: each repo's scopes, and the forums its point fetches touch
        let queued = sy.queue_snapshot();
        let mut scopes: BTreeMap<String, Vec<Scope>> = BTreeMap::new();
        let mut point_fks: BTreeSet<String> = BTreeSet::new();
        for item in queued.ordered() {
            if item.ready_at > now {
                continue;
            }
            if let Action::PointFetch(uris) = item.action() {
                for u in uris {
                    if let Ok(u) = AtUri::parse(&u) {
                        let fk = scope_of(u.collection().as_str(), u.rkey().as_str());
                        if fk != GLOBAL_SCOPE {
                            point_fks.insert(fk.to_owned());
                        }
                    }
                }
                continue;
            }
            let r = rel.get(&item.did);
            let mut fks: BTreeSet<String> = r.map(|r| r.fks.clone()).unwrap_or_default();
            // the scopes of the pass that queued it (a forum open in a view, not followed)
            fks.extend(item.fks.iter().cloned());
            if let Some(row) = rows.get(item.did.as_str()) {
                fks.extend(
                    row.listed
                        .keys()
                        .filter(|k| *k != "global" && *k != "own")
                        .cloned(),
                );
            }
            let mut sc: Vec<Scope> = fks.into_iter().map(Scope::Forum).collect();
            if r.is_some_and(|r| r.own) {
                sc.push(Scope::Own);
            }
            scopes.insert(item.did.clone(), sc);
        }
        let reports = sy
            .run_ready(&s, limits::LIVE_TAIL_BATCH, |d| {
                scopes.get(d).cloned().unwrap_or_default()
            })
            .await;
        out.synced = reports.len();
        let mut changed_fks = point_fks;
        let mut topics: Vec<Topic> = Vec::new();
        for r in &reports {
            if let Some(e) = &r.error {
                tracing::debug!(did = %r.did, error = %e, "live tail: sync failed");
            }
            if r.changed > 0 {
                topics.push(Topic::User { did: r.did.clone() });
                for sc in scopes.get(&r.did).into_iter().flatten() {
                    if let Scope::Forum(fk) = sc {
                        changed_fks.insert(fk.clone());
                    }
                }
            }
            let own = rel.get(&r.did).is_some_and(|x| x.own);
            if own
                && r.outcome == RepoOutcome::Applied
                && let Ok(did) = Did::parse(&r.did)
                && let Err(e) = self.seed_account(&did).await
            {
                tracing::warn!(account = %r.did, error = %e, "re-seeding followed forums");
            }
        }
        for fk in &changed_fks {
            if let Some(f) = self.rt.forum_of_fk(fk) {
                self.rt.invalidate(&f);
                out.changed.push(f.clone());
                topics.push(Topic::Forum { uri: f });
            }
        }
        self.rt.bus.changed(topics);

        // spilled bodies views left pending (lfm-1.md §1.3): one that arrives is announced, so a
        // client shows it without being asked again
        let arrived = self.retry_spilled_bodies(&self.rt.renders).await;
        if !arrived.is_empty() {
            self.rt.bus.changed(arrived);
        }

        // handles of shown authors
        out.verified = self.verify_handles(limits::HANDLE_VERIFY_BATCH).await;

        // expired tombstones, daily
        let gc_due = super::models::lock(&self.rt.gc_at)
            .is_none_or(|t| now.saturating_sub(t) >= limits::TOMBSTONE_GC_EVERY_MS);
        if gc_due {
            *super::models::lock(&self.rt.gc_at) = Some(now);
            match store
                .gc_tombstones(now, limits::TOMBSTONE_MIN_RETENTION_MS)
                .await
            {
                Ok(n) => out.gc = n,
                Err(e) => tracing::warn!(error = %e, "tombstone GC failed"),
            }
        }

        // the next round: the next ready item or due poll (from the rows as they are now), at
        // most the idle time
        let now = self.host.now_ms();
        let mut next = now.saturating_add(limits::LIVE_TAIL_IDLE_MS);
        if let Some(t) = sy.queue_snapshot().next_ready_at() {
            next = next.min(t);
        }
        let rel_dids: Vec<&str> = rel.keys().map(String::as_str).collect();
        let rows = store.repos(&rel_dids).await.map_err(store_error)?;
        for ((d, r), row) in rel.iter().zip(rows) {
            if polling && let Some(row) = row {
                let at = schedule::next_poll_at(&row, r.authority, now, jitter(d, Some(&row)));
                // a poll that is still due (it failed) waits a little, not a whole idle time
                next = next.min(at.max(now.saturating_add(1_000)));
            }
        }
        // a pending spilled body is asked for again when its retry time comes
        for (_, p) in self.rt.renders.spilled_pending() {
            next = next.min(p.at_ms.max(now.saturating_add(1_000)));
        }
        out.next_ms = next.saturating_sub(now).max(1);
        Ok(out)
    }

    /// Verifies the handles of up to `n` queued authors (`sync.md` §3.3; a cached state younger
    /// than its TTL is kept) and emits `Changed` for them and every cached forum if any handle
    /// state changed. Returns how many were checked.
    pub async fn verify_handles(&self, n: usize) -> usize {
        let dids = self.rt.take_verify(n);
        let mut changed = Vec::new();
        for d in &dids {
            let Ok(did) = Did::parse(d) else { continue };
            let before = self.resolver.cached(&did).await.map(|i| i.actor_view());
            match self.resolver.resolve_did(&did).await {
                Ok(id) => {
                    if before.as_ref() != Some(&id.actor_view()) {
                        changed.push(Topic::User { did: d.clone() });
                    }
                }
                Err(e) => tracing::debug!(did = %d, error = %e, "handle verification failed"),
            }
        }
        if !changed.is_empty() {
            changed.extend(
                self.rt
                    .loaded_forums()
                    .into_iter()
                    .map(|uri| Topic::Forum { uri }),
            );
            self.rt.bus.changed(changed);
        }
        dids.len()
    }
}

#[cfg(test)]
mod context_tests {
    use serde::Serialize;

    use super::*;
    use crate::discovery::state::DISC_ROWS_MAX;
    use crate::discovery::{DiscRow, ForumKey};
    use crate::engine::EngineConfig;
    use crate::host::{
        BlobHandle, HostError, HttpRequest, HttpResponse, KvOp, WebSocket, WsMessage,
    };
    use crate::store::{mem::MemKv, row};

    #[derive(Clone, Default)]
    struct TestHost {
        cache: MemKv,
        state: MemKv,
    }

    struct TestWs;

    impl WebSocket for TestWs {
        async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
            None
        }

        async fn send_text(&mut self, _text: String) -> Result<(), HostError> {
            Ok(())
        }

        async fn close(&mut self) {}
    }

    impl Host for TestHost {
        type Kv = MemKv;
        type Ws = TestWs;
        type Lock = ();

        fn cache(&self) -> &MemKv {
            &self.cache
        }

        fn state(&self) -> &MemKv {
            &self.state
        }

        async fn http(&self, _req: HttpRequest) -> Result<HttpResponse, HostError> {
            Err(HostError::Unsupported("test".into()))
        }

        async fn fetch_blob(
            &self,
            _pds: url::Url,
            _did: Did,
            _cid: String,
            _max_bytes: u64,
        ) -> Result<BlobHandle, HostError> {
            Err(HostError::Unsupported("test".into()))
        }

        async fn websocket(&self, _url: url::Url, _max: u32) -> Result<TestWs, HostError> {
            Err(HostError::Unsupported("test".into()))
        }

        async fn resolve_txt_system(
            &self,
            _name: String,
        ) -> Result<Option<Vec<String>>, HostError> {
            Ok(None)
        }

        async fn lock(&self, _name: &str, _wait: bool) -> Result<Option<()>, HostError> {
            Ok(Some(()))
        }

        async fn sleep(&self, _ms: u64) {}

        fn now_ms(&self) -> i64 {
            1
        }

        fn fill_random(&self, buf: &mut [u8]) {
            buf.fill(0);
        }
    }

    #[derive(Serialize)]
    struct FollowRow {
        v: u32,
        at: i64,
        seeded: bool,
    }

    #[test]
    fn change_feeds_poll_one_due_forum_at_a_time_and_follow_open_views() {
        let (a, b) = ("at://a/forum/x".to_owned(), "at://b/forum/y".to_owned());
        let followed = [a.clone(), b.clone()];
        let mut feeds = ChangeFeeds::default();
        // never polled: due at once, one per call
        assert_eq!(feeds.take_due(&followed, &[], 0), Some(a.clone()));
        assert!(feeds.due_at() <= 0, "the other forum is still due");
        assert_eq!(feeds.take_due(&followed, &[], 0), Some(b.clone()));
        assert_eq!(feeds.take_due(&followed, &[], 0), None);
        // idle forums: every 60 s
        assert_eq!(
            feeds.due_at(),
            CHANGES_OPEN_MS,
            "the followed list is read again"
        );
        assert_eq!(feeds.take_due(&followed, &[], CHANGES_OPEN_MS), None);
        // a view of `a` opens after its idle poll: due 10 s after that poll, not 60 s
        let open = [a.clone()];
        assert_eq!(
            feeds.take_due(&followed, &open, CHANGES_OPEN_MS),
            Some(a.clone())
        );
        assert_eq!(
            feeds.take_due(&followed, &open, 2 * CHANGES_OPEN_MS - 1),
            None
        );
        assert_eq!(
            feeds.take_due(&followed, &open, 2 * CHANGES_OPEN_MS),
            Some(a.clone())
        );
        // the most overdue first
        assert_eq!(
            feeds.take_due(&followed, &open, CHANGES_IDLE_MS),
            Some(a.clone())
        );
        assert_eq!(
            feeds.take_due(&followed, &open, CHANGES_IDLE_MS),
            Some(b.clone())
        );
        // an unfollowed forum is forgotten
        assert_eq!(
            feeds.take_due(std::slice::from_ref(&b), &open, CHANGES_IDLE_MS),
            None
        );
        assert_eq!(feeds.polls.len(), 1);
    }

    #[test]
    fn stored_discovery_rows_are_capped_dropping_the_oldest_unconfirmed() {
        let mut rows = BTreeMap::new();
        for i in 0..DISC_ROWS_MAX {
            let mut r = DiscRow::new("hint:test", 1_000 + i as i64);
            if i < 5 {
                // the oldest rows are confirmed members: kept
                r.set(crate::discovery::state::DiscState::Confirmed, 1);
            } else if i % 2 == 0 {
                r.reject(2);
            }
            rows.insert(format!("did:plc:{i:024}"), r);
        }
        assert!(
            disc_rows_over_cap(&rows, "").is_empty(),
            "at the cap nothing is dropped"
        );
        rows.insert("did:plc:new".into(), DiscRow::new("hint:jetstream", 0));
        rows.insert("did:plc:newer".into(), DiscRow::new("hint:jetstream", 1));
        // the new row (first seen earliest) is kept; the oldest unconfirmed ones go
        let dropped = disc_rows_over_cap(&rows, "did:plc:new");
        assert_eq!(
            dropped,
            vec!["did:plc:newer".to_owned(), format!("did:plc:{:024}", 5)]
        );
    }

    #[test]
    fn context_uses_all_identity_rows_and_discovery_not_fold_completeness() {
        futures::executor::block_on(async {
            let host = TestHost::default();
            let engine = Engine::new(host.clone(), EngineConfig::default());
            let owner = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
            let key = ForumKey::new(&owner, "meta").unwrap();
            let follow = FollowRow {
                v: 1,
                at: 1,
                seeded: false,
            };
            host.state()
                .write(vec![KvOp::Put(
                    crate::key!("loc", "follow", key.uri.as_str()),
                    row::encode(&follow).unwrap(),
                )])
                .await
                .unwrap();
            // sync.md §6.2: D holds confirmed participants, never hints that are not.
            let candidate = "did:plc:candidate".to_owned();
            let unconfirmed = "did:plc:unconfirmed".to_owned();
            let rejected = "did:plc:rejected".to_owned();
            let mut confirmed_row = DiscRow::new("hint:test", host.now_ms());
            confirmed_row.set(crate::discovery::state::DiscState::Confirmed, host.now_ms());
            let mut rejected_row = DiscRow::new("hint:test", host.now_ms());
            rejected_row.reject(host.now_ms());
            let rows = BTreeMap::from([
                (candidate.clone(), confirmed_row),
                (
                    unconfirmed.clone(),
                    DiscRow::new("hint:test", host.now_ms()),
                ),
                (rejected.clone(), rejected_row),
            ]);
            crate::discovery::state::put_disc_rows(host.cache(), &key.fk, &rows)
                .await
                .unwrap();
            let unrelated = "did:plc:unrelated";
            host.cache()
                .write(vec![KvOp::Put(
                    schema::prefix("id", &["did", unrelated]),
                    vec![1],
                )])
                .await
                .unwrap();

            let cx = engine.live_context().await.unwrap();
            assert!(cx.known_dids.contains(unrelated));
            assert!(cx.filter_dids.contains(&candidate));
            assert!(cx.newcomer_dids.contains(&candidate));
            for hint in [&unconfirmed, &rejected] {
                assert!(!cx.filter_dids.contains(hint), "{hint} is in D");
                assert!(
                    !cx.newcomer_dids.contains(hint),
                    "{hint} is a known newcomer"
                );
            }
            assert!(
                !engine
                    .relevant_repos()
                    .await
                    .unwrap()
                    .contains_key(&rejected)
            );
            assert!(!cx.participants_known);

            crate::discovery::state::mark_discovery_complete(host.cache(), &key.fk, host.now_ms())
                .await
                .unwrap();
            let cx = engine.live_context().await.unwrap();
            assert!(cx.participants_known, "no complete fold was loaded");
            assert!(cx.filter_dids.contains(&candidate));
        });
    }
}
