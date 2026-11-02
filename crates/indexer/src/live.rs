// SPDX-License-Identifier: AGPL-3.0-or-later
//! Two concurrent wildcard Jetstream readers feed one sequential applier, which owns the core
//! relevance filter and the shared 10,000-entry deduplicator (sync.md §15.1). A frame's cursor
//! is persisted only after its point fetch, proof and projection completed, or after its
//! failure was handed to the durable per-repo retry path. No single event can hold a cursor:
//! a failed fetch, a missing proof or an absent declaration is recorded, not replayed forever.
//! Because one applier handles every frame in arrival order, a duplicate from the other
//! endpoint advances that endpoint's cursor only after the original was applied.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::host::{Host, HostError, WebSocket, WsMessage};
use lessdugin_core::live::events::{Action, Context, EventFilter};
use lessdugin_core::live::source::{DEDUP_CAPACITY, MAX_MESSAGE_BYTES};
use lessdugin_core::live::wire::{self, Event, EventKind, Wire};
use lessdugin_core::sync::repo::MarkHint;
use lessdugin_core::sync::{Priority, Stats};
use lessdugin_lexicon::collection::nsid;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use url::Url;

use crate::store::{LiveChange, Store};
use crate::{Indexer, Metrics, NS, ingest, proofs};

const REWIND_US: i64 = 10_000_000;
/// With no frame for this long a socket is presumed half-open (a NAT or balancer dropped the
/// flow without a FIN) and is replaced (sync.md §6.2 reconnect and failover).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Frames read ahead of the applier, per indexer.
const QUEUE: usize = 1_024;
/// Frames applied before one projection pass.
const BATCH: usize = 32;
/// An ordinary forum sync requested by a membership or declaration event waits this long, so
/// a burst of newcomers costs one sync.
pub const ROSTER_SYNC_DEBOUNCE_MS: i64 = 2_000;

/// What a reader hands to the applier.
#[derive(Debug)]
pub(crate) enum Item {
    Event {
        source: String,
        origin: String,
        event: Event,
    },
    /// Two consecutive connections failed on an oversized message at one position: resume one
    /// second past it and reconcile (sync.md §6.2).
    Skip { source: String, time_us: i64 },
}

/// Starts both readers and the applier; the handles stop them when aborted.
pub fn spawn(indexer: Indexer) -> Vec<tokio::task::JoinHandle<()>> {
    if indexer.config.jetstream.is_empty() {
        return Vec::new();
    }
    let (tx, rx) = mpsc::channel(QUEUE);
    let mut tasks = vec![tokio::spawn(applier(indexer.clone(), rx))];
    for endpoint in indexer.config.jetstream.iter().take(2).cloned() {
        let host = indexer.engine.host().clone();
        let env = ReaderEnv::of(&indexer);
        let tx = tx.clone();
        tasks.push(tokio::spawn(async move {
            endpoint_loop(env, endpoint, tx, IDLE_TIMEOUT, move |url| {
                let host = host.clone();
                async move { host.websocket(url, MAX_MESSAGE_BYTES).await }
            })
            .await;
        }));
    }
    tasks
}

fn source_name(endpoint: &Url) -> String {
    format!("jetstream:{}", endpoint.origin().ascii_serialization())
}

fn subscribe(endpoint: &Url, cursor: Option<i64>, now_us: i64) -> Url {
    subscribe_at(endpoint, cursor.unwrap_or(now_us).saturating_sub(REWIND_US))
}

/// The subscription at exactly `cursor_us` (no rewind).
fn subscribe_at(endpoint: &Url, cursor_us: i64) -> Url {
    let mut url = endpoint.clone();
    url.set_path("/xrpc/network.bsky.jetstream.subscribeEvents");
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut()
        .append_pair("collections", &format!("{NS}.*"))
        .append_pair("cursor", &cursor_us.to_string());
    url
}

/// What one reader needs from the indexer (separate so the reader can be driven by a scripted
/// socket in tests).
pub(crate) struct ReaderEnv {
    pub(crate) store: Store,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl ReaderEnv {
    fn of(indexer: &Indexer) -> Self {
        let host = indexer.engine.host().clone();
        Self {
            store: indexer.store.clone(),
            metrics: Arc::clone(&indexer.metrics),
            now_ms: Arc::new(move || host.now_ms()),
        }
    }
}

/// Why a connection ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// The server closed it.
    Closed,
    /// No frame within the idle bound.
    Idle,
    /// A message over the cap, after the frame at this position (µs).
    Oversize(i64),
    /// A socket error.
    Failed(String),
    /// The applier stopped.
    Stopped,
}

/// Where the next connection of one endpoint starts, and the oversize rule's state.
#[derive(Debug, Default)]
pub(crate) struct ReaderState {
    /// Start exactly here (no rewind) instead of at the durable cursor.
    pub(crate) resume_us: Option<i64>,
    /// `(position, consecutive oversize failures there)`.
    pub(crate) oversized: Option<(i64, u8)>,
}

impl ReaderState {
    /// An oversized message after `position`. The first failure retries at exactly that
    /// position; the second skips one second past it. Returns the skip target.
    pub(crate) fn on_oversize(&mut self, position: i64) -> Option<i64> {
        let count = match self.oversized {
            Some((at, n)) if at == position => n.saturating_add(1),
            _ => 1,
        };
        if count >= 2 {
            let skip = position.saturating_add(1_000_000);
            self.oversized = None;
            self.resume_us = Some(skip);
            Some(skip)
        } else {
            self.oversized = Some((position, count));
            self.resume_us = Some(position);
            None
        }
    }

    /// A frame at `time_us` arrived: only progress past the failure position resets the rule
    /// (a replay of earlier frames does not).
    fn on_frame(&mut self, time_us: i64) {
        if self.oversized.is_some_and(|(at, _)| time_us > at) {
            self.oversized = None;
        }
    }
}

/// One endpoint, forever: connect at its cursor, read, reconnect with back-off.
pub(crate) async fn endpoint_loop<W, C, F>(
    env: ReaderEnv,
    endpoint: Url,
    tx: mpsc::Sender<Item>,
    idle: Duration,
    connect: C,
) where
    W: WebSocket,
    C: Fn(Url) -> F,
    F: Future<Output = Result<W, HostError>>,
{
    let source = source_name(&endpoint);
    let origin = endpoint.origin().ascii_serialization();
    let mut state = ReaderState::default();
    let mut backoff = Duration::from_millis(250);
    loop {
        let (url, start) = match state.resume_us.take() {
            Some(at) => (subscribe_at(&endpoint, at), at),
            None => {
                let persisted = env
                    .store
                    .cursor(&source)
                    .and_then(|v| v.parse::<i64>().ok());
                let now_us = (env.now_ms)().saturating_mul(1000);
                let url = subscribe(&endpoint, persisted, now_us);
                (url, persisted.unwrap_or(now_us).saturating_sub(REWIND_US))
            }
        };
        let mut socket = match connect(url).await {
            Ok(s) => s,
            Err(e) => {
                warn!(endpoint=%endpoint,error=%e,"Jetstream connect failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        env.metrics.live_connections.fetch_add(1, Ordering::Relaxed);
        let (end, frames) = read_connection(
            &mut socket,
            idle,
            start,
            &mut state,
            &source,
            &origin,
            &env,
            &tx,
        )
        .await;
        env.metrics.live_connections.fetch_sub(1, Ordering::Relaxed);
        socket.close().await;
        match &end {
            End::Stopped => return,
            End::Closed => debug!(endpoint=%endpoint,"Jetstream connection closed"),
            End::Idle => warn!(endpoint=%endpoint,"Jetstream connection idle; reconnecting"),
            End::Oversize(at) => {
                env.metrics.live_oversized.fetch_add(1, Ordering::Relaxed);
                if let Some(skip) = state.on_oversize(*at) {
                    warn!(endpoint=%endpoint,position=at,resume=skip,"oversized Jetstream message skipped; a reconcile is scheduled");
                    if tx
                        .send(Item::Skip {
                            source: source.clone(),
                            time_us: skip,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                } else {
                    warn!(endpoint=%endpoint,position=at,"oversized Jetstream message");
                }
            }
            End::Failed(e) => warn!(endpoint=%endpoint,error=%e,"Jetstream connection failed"),
        }
        // A connection that delivered frames, or ended on the oversize rule, reconnects at once.
        if frames > 0 || matches!(end, End::Oversize(_) | End::Idle | End::Closed) {
            backoff = Duration::from_millis(250);
        }
        tokio::time::sleep(backoff).await;
        if frames == 0 && matches!(end, End::Failed(_)) {
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}

/// Reads one connection until it ends. Returns why, and how many frames it delivered.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn read_connection<W: WebSocket>(
    socket: &mut W,
    idle: Duration,
    start_us: i64,
    state: &mut ReaderState,
    source: &str,
    origin: &str,
    env: &ReaderEnv,
    tx: &mpsc::Sender<Item>,
) -> (End, usize) {
    let mut position = start_us;
    let mut frames = 0;
    loop {
        let message = match tokio::time::timeout(idle, socket.next()).await {
            Err(_) => return (End::Idle, frames),
            Ok(None) => return (End::Closed, frames),
            Ok(Some(message)) => message,
        };
        let text = match message {
            Err(HostError::TooLarge) => return (End::Oversize(position), frames),
            Err(e) => return (End::Failed(e.to_string()), frames),
            Ok(WsMessage::Text(t)) => t,
            Ok(WsMessage::Binary(b)) => match String::from_utf8(b) {
                Ok(t) => t,
                Err(e) => {
                    debug!(error=%e,"non-UTF-8 Jetstream frame ignored");
                    continue;
                }
            },
        };
        let event = match wire::parse(Wire::V2, &text) {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(e) => {
                debug!(error=%e,"ignored malformed Jetstream event");
                continue;
            }
        };
        if !matches!(event.kind, EventKind::Info(_)) {
            position = event.time_us;
            state.on_frame(event.time_us);
        }
        env.metrics.live_events.fetch_add(1, Ordering::Relaxed);
        frames += 1;
        let item = Item::Event {
            source: source.to_owned(),
            origin: origin.to_owned(),
            event,
        };
        if tx.send(item).await.is_err() {
            return (End::Stopped, frames);
        }
    }
}

/// Cursor writes of one batch, in frame order.
enum Ack {
    Advance(String, i64),
    Set(String, i64),
}

/// The effects of one batch: projections to rebuild, repositories to hand off, cursors.
#[derive(Default)]
struct Work {
    /// Forum → (live commits, authors whose head rows to refresh).
    project: BTreeMap<String, (BTreeMap<String, LiveChange>, BTreeSet<String>)>,
    acks: Vec<Ack>,
    reconcile: bool,
    /// A hand-off could not be persisted: the cursors of this batch must not move.
    unsafe_acks: bool,
}

impl Work {
    fn project(&mut self, forum: &str, author: &str, live: Option<(&str, LiveChange)>) {
        let entry = self.project.entry(forum.to_owned()).or_default();
        entry.1.insert(author.to_owned());
        if let Some((uri, change)) = live {
            entry.0.insert(uri.to_owned(), change);
        }
    }

    fn hand_off(&mut self, indexer: &Indexer, did: &str, forum: &str, error: &str) {
        indexer
            .metrics
            .live_failures
            .fetch_add(1, Ordering::Relaxed);
        match indexer
            .store
            .schedule_repo_retry(did, forum, indexer.now_ms(), error)
        {
            Ok(at) => {
                warn!(did=%did,forum=%forum,error=%error,retry_at=at,"live event handed to the repository retry path")
            }
            Err(e) => {
                warn!(did=%did,forum=%forum,error=%e,"live event hand-off failed");
                self.unsafe_acks = true;
            }
        }
    }
}

async fn applier(indexer: Indexer, mut rx: mpsc::Receiver<Item>) {
    let mut filter = EventFilter::with_dedup_capacity(DEDUP_CAPACITY);
    let mut cx: Option<(u64, Context)> = None;
    let mut carry = None;
    loop {
        let first = match carry.take() {
            Some(item) => item,
            None => match rx.recv().await {
                Some(item) => item,
                None => return,
            },
        };
        let mut uris = BTreeSet::new();
        distinct(&first, &mut uris);
        let mut batch = vec![first];
        while batch.len() < BATCH {
            let Ok(item) = rx.try_recv() else { break };
            // Two commits of one record get their own projections, so each is logged with its
            // own rev and CID.
            if !distinct(&item, &mut uris) {
                carry = Some(item);
                break;
            }
            batch.push(item);
        }
        apply_batch(&indexer, &mut filter, &mut cx, batch).await;
    }
}

/// False when `item` is a commit of a record already in this batch.
fn distinct(item: &Item, uris: &mut BTreeSet<String>) -> bool {
    match item {
        Item::Event { event, .. } => match (&event.did, &event.kind) {
            (
                Some(did),
                EventKind::Commit {
                    collection, rkey, ..
                },
            ) => uris.insert(format!("{did}/{collection}/{rkey}")),
            _ => true,
        },
        Item::Skip { .. } => true,
    }
}

async fn apply_batch(
    indexer: &Indexer,
    filter: &mut EventFilter,
    cx: &mut Option<(u64, Context)>,
    batch: Vec<Item>,
) {
    let mut work = Work::default();
    for item in batch {
        match item {
            Item::Skip { source, time_us } => {
                work.acks.push(Ack::Set(source, time_us));
                work.reconcile = true;
            }
            Item::Event {
                source,
                origin,
                event,
            } => {
                let generation = indexer.store.generation();
                if cx.as_ref().is_none_or(|(g, _)| *g != generation) {
                    *cx = Some((generation, context(indexer)));
                }
                let Some((_, context)) = cx.as_ref() else {
                    continue;
                };
                let out = filter.apply(Wire::V2, &origin, event, context);
                if out.duplicate {
                    indexer
                        .metrics
                        .live_duplicates
                        .fetch_add(1, Ordering::Relaxed);
                }
                for action in out.actions {
                    handle(indexer, action, &mut work).await;
                }
                // Every effect is applied or handed off: nothing is left to replay.
                filter.acknowledge();
                if out.restart_cursor {
                    work.acks
                        .push(Ack::Set(source, indexer.now_ms().saturating_mul(1000)));
                } else if let Some(cursor) = out.cursor {
                    work.acks.push(Ack::Advance(source, cursor.time_us));
                }
            }
        }
    }
    for (forum, (live, authors)) in std::mem::take(&mut work.project) {
        let Ok(key) = AtUri::parse(&forum)
            .map_err(anyhow::Error::from)
            .and_then(|u| ForumKey::of_uri(&u).map_err(anyhow::Error::from))
        else {
            continue;
        };
        match ingest::project_forum(indexer, &key, Some(&authors), &live).await {
            Ok(_) => {}
            Err(e)
                if e.downcast_ref::<lessdugin_core::model::ModelError>()
                    .is_some_and(|e| {
                        matches!(e, lessdugin_core::model::ModelError::NoDeclaration(_))
                    }) =>
            {
                // The declaration was deleted: an empty snapshot and a quarantined seed.
                match indexer.store.note_forum_absent(&forum, indexer.now_ms()) {
                    Ok(deletes) => {
                        ingest::lock(&indexer.models).remove(&forum);
                        info!(forum=%forum,deletes,"forum declaration deleted");
                    }
                    Err(e) => {
                        warn!(forum=%forum,error=%e,"recording an absent forum failed");
                        work.unsafe_acks = true;
                    }
                }
            }
            Err(e) => {
                for did in &authors {
                    work.hand_off(indexer, did, &forum, &format!("projection: {e}"));
                }
            }
        }
    }
    if work.reconcile
        && let Err(e) = indexer.store.schedule_next("reconcile", indexer.now_ms())
    {
        warn!(error=%e,"scheduling a reconcile failed");
        work.unsafe_acks = true;
    }
    if work.unsafe_acks {
        warn!("live batch effects could not all be recorded; its cursors are not advanced");
        return;
    }
    for ack in work.acks {
        let result = match &ack {
            Ack::Advance(source, at) => indexer.store.advance_cursor(source, *at),
            Ack::Set(source, at) => indexer.store.set_cursor(source, &at.to_string()),
        };
        if let Err(e) = result {
            warn!(error=%e,"persisting a Jetstream cursor failed");
        }
    }
}

/// The relevance context, rebuilt only when the store's forum set or rosters changed.
fn context(indexer: &Indexer) -> Context {
    let mut cx = Context::default();
    for forum in indexer.store.forums(None) {
        if let Ok(uri) = AtUri::parse(&forum.uri) {
            cx.declarations
                .insert((forum.did.clone(), uri.rkey().to_string()));
        }
        cx.known_dids.insert(forum.did.clone());
        // Only an indexed forum's range is followed; a seed is first synced, and a quarantined
        // seed is watched only through its declaration.
        if forum.synced_ms > 0 && forum.absent_ms == 0 && !forum.fk.is_empty() {
            cx.forums.insert(forum.fk.clone());
            cx.newcomer_forums.insert(forum.fk);
            cx.authorities.insert(forum.did);
            cx.participants_known = true;
        }
    }
    for did in indexer.store.participant_dids() {
        cx.known_dids.insert(did.clone());
        cx.newcomer_dids.insert(did);
    }
    cx
}

/// The forum seeds a record belongs to: its fk range, or the declaration itself.
fn record_forums(indexer: &Indexer, uri: &AtUri) -> Vec<String> {
    let mut out = BTreeSet::new();
    if uri.collection().as_str() == nsid::FORUM && indexer.store.forum_row(uri.as_str()).is_some() {
        out.insert(uri.to_string());
    }
    let rkey = uri.rkey().as_str();
    if let Some(fk) = lessdugin_lexicon::keys::fk_of(rkey)
        && (rkey.len() == fk.len() || rkey[fk.len()..].starts_with('-'))
    {
        out.extend(indexer.store.forums_by_fk(fk));
    }
    out.into_iter().collect()
}

async fn handle(indexer: &Indexer, action: Action, work: &mut Work) {
    let now = indexer.now_ms();
    match action {
        Action::PointFetch {
            did,
            uri,
            rev,
            cid,
            priority,
            ..
        } => point(indexer, &did, &uri, rev, cid, priority, work).await,
        Action::Candidate {
            did,
            fk,
            cid,
            first_seen_us,
            ..
        } => {
            // A newcomer's membership is a durable hint; confirming it is an ordinary forum
            // sync, debounced and run by the scheduler, never inline.
            let Some(cid) = cid else { return };
            for forum in indexer.store.forums_by_fk(&fk) {
                match indexer.store.add_candidate_from(
                    &forum,
                    &did,
                    &cid,
                    first_seen_us.div_euclid(1000),
                    "",
                ) {
                    Ok(true) => {
                        if let Err(e) = indexer
                            .store
                            .schedule_forum_sync(&forum, now + ROSTER_SYNC_DEBOUNCE_MS)
                        {
                            warn!(forum=%forum,error=%e,"scheduling a newcomer sync failed");
                            work.unsafe_acks = true;
                        }
                    }
                    Ok(false) => debug!(forum=%forum,did=%did,"membership hint over quota"),
                    Err(e) => {
                        warn!(forum=%forum,error=%e,"recording a membership hint failed");
                        work.unsafe_acks = true;
                    }
                }
            }
        }
        Action::InvalidateIdentity { did, authority } => {
            if let Err(e) = indexer.engine.invalidate_identity(&did).await {
                debug!(did=%did,error=%e,"identity invalidation failed");
            }
            if authority {
                // Authorities are re-resolved at P1: by the forum's next ordinary sync.
                for forum in indexer.store.did_forums(&did) {
                    if indexer
                        .store
                        .forum_row(&forum)
                        .is_some_and(|f| f.did == did)
                        && indexer.store.schedule_forum_sync(&forum, now).is_err()
                    {
                        work.unsafe_acks = true;
                    }
                }
            }
        }
        Action::AccountStatus { did, .. } => {
            let forums = indexer.store.did_forums(&did);
            let checked = match Did::parse(&did) {
                Ok(parsed) => {
                    let session = indexer
                        .engine
                        .syncer()
                        .session(Priority::P1, Arc::new(Stats::new()));
                    indexer
                        .engine
                        .syncer()
                        .check_account_status(&session, &parsed)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                Err(e) => Err(e.to_string()),
            };
            for forum in forums {
                match &checked {
                    Ok(()) => work.project(&forum, &did, None),
                    Err(e) => work.hand_off(indexer, &did, &forum, e),
                }
            }
        }
        Action::Resync { did } | Action::CatchUp { did } => {
            let Ok(parsed) = Did::parse(&did) else { return };
            for forum in indexer.store.did_forums(&did) {
                let Ok(key) = AtUri::parse(&forum)
                    .map_err(anyhow::Error::from)
                    .and_then(|u| ForumKey::of_uri(&u).map_err(anyhow::Error::from))
                else {
                    continue;
                };
                match ingest::resync_repo(indexer, &parsed, &key).await {
                    Ok(()) => work.project(&forum, &did, None),
                    Err(e) => work.hand_off(indexer, &did, &forum, &e.to_string()),
                }
            }
        }
        Action::Reconcile(gap) => {
            info!(?gap, "Jetstream gap; a reconcile is scheduled");
            work.reconcile = true;
        }
    }
}

async fn point(
    indexer: &Indexer,
    did: &str,
    uri: &str,
    rev: String,
    cid: Option<String>,
    priority: Priority,
    work: &mut Work,
) {
    if indexer.blocked(uri) {
        return;
    }
    let Ok(at) = AtUri::parse(uri) else {
        debug!(uri=%uri,"ignored an unparsable live URI");
        return;
    };
    let forums = record_forums(indexer, &at);
    let hint = MarkHint {
        cid: cid
            .as_deref()
            .and_then(|c| lessdugin_atp::cid::parse(c).ok()),
    };
    let session = indexer
        .engine
        .syncer()
        .session(priority, Arc::new(Stats::new()));
    if let Err(e) = indexer
        .engine
        .syncer()
        .point_fetch(&session, &at, Some(&hint))
        .await
    {
        if forums.is_empty() {
            debug!(uri=%uri,error=%e,"live point fetch failed outside any forum");
            indexer
                .metrics
                .live_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        for forum in &forums {
            work.hand_off(indexer, did, forum, &format!("point fetch: {e}"));
        }
        return;
    }
    if forums.is_empty() {
        return;
    }
    // One proof per relevant commit (sync.md §15.1). A proof that cannot be fetched or verified
    // now is simply not cached: `getRecordProofs` omits it, and the record is still indexed.
    if let Err(e) = proofs::fetch(indexer, at.as_str()).await {
        debug!(uri=%uri,error=%e,"no proof cached for a live commit");
    }
    let roster =
        at.collection().as_str() == nsid::MEMBERSHIP || at.collection().as_str() == nsid::FORUM;
    for forum in forums {
        let indexed = indexer.store.forum(&forum).is_some();
        if indexed {
            work.project(
                &forum,
                did,
                Some((
                    at.as_str(),
                    LiveChange {
                        rev: rev.clone(),
                        cid: cid.clone(),
                    },
                )),
            );
        }
        // A membership or declaration may change who participates: confirm it with an ordinary
        // sync (debounced), as for a newcomer. An unindexed seed is synced first.
        if (roster || !indexed)
            && let Err(e) = indexer
                .store
                .schedule_forum_sync(&forum, indexer.now_ms() + ROSTER_SYNC_DEBOUNCE_MS)
        {
            warn!(forum=%forum,error=%e,"scheduling a roster sync failed");
            work.unsafe_acks = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_restart_rewinds_ten_seconds() {
        let endpoint = Url::parse("wss://jetstream.example.test").unwrap();
        let url = subscribe(&endpoint, Some(50_000_000), 100_000_000);
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["collections"], format!("{NS}.*"));
        assert_eq!(q["cursor"], "40000000");
        let fresh = subscribe(&endpoint, None, 100_000_000);
        assert_eq!(
            fresh.query_pairs().find(|(k, _)| k == "cursor").unwrap().1,
            "90000000"
        );
    }

    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A socket that plays a script, then closes (or, with `hang`, stays silent forever).
    struct Scripted {
        frames: VecDeque<Result<WsMessage, HostError>>,
        hang: bool,
    }

    impl WebSocket for Scripted {
        async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
            match self.frames.pop_front() {
                Some(frame) => Some(frame),
                None if self.hang => std::future::pending().await,
                None => None,
            }
        }
        async fn send_text(&mut self, _text: String) -> Result<(), HostError> {
            Ok(())
        }
        async fn close(&mut self) {}
    }

    fn frame(time_us: i64) -> Result<WsMessage, HostError> {
        let at = lessdugin_atp::syntax::Datetime::from_micros(time_us)
            .unwrap()
            .to_string();
        Ok(WsMessage::Text(
            serde_json::json!({"$type":"message","payload":{
                "$type":"network.bsky.jetstream.subscribeEvents#commit",
                "did":"did:plc:aaaaaaaaaaaaaaaaaaaaaaaa","seq":1,"witnessedAt":at,
                "rev":"3l2s5xxv2zaa2","operation":"create","collection":nsid::POST,
                "rkey":"aaaaaaaaaaaa-3l2s5xxv2zaa2",
                "cid":lessdugin_atp::cid::for_raw(&[1]).to_string(),
            }})
            .to_string(),
        ))
    }

    fn env() -> ReaderEnv {
        ReaderEnv {
            store: Store::in_memory().unwrap(),
            metrics: Arc::new(Metrics::default()),
            now_ms: Arc::new(|| 1_800_000_000_000),
        }
    }

    fn times(items: &[Item]) -> Vec<(char, i64)> {
        items
            .iter()
            .map(|i| match i {
                Item::Event { event, .. } => ('e', event.time_us),
                Item::Skip { time_us, .. } => ('s', *time_us),
            })
            .collect()
    }

    const T: i64 = 1_790_000_000_000_000;

    #[tokio::test]
    async fn an_oversized_message_is_skipped_after_two_failures_at_one_position() {
        // Every connection replays the frame before the oversized message (a rewind), then hits
        // it. The reader retries once at exactly that position, then resumes one second past it
        // and asks for a reconcile; the frame after the oversized message is then delivered.
        let script = Arc::new(Mutex::new(VecDeque::from([
            vec![frame(T), Err(HostError::TooLarge)],
            vec![frame(T), Err(HostError::TooLarge)],
            vec![frame(T + 1_500_000)],
        ])));
        let urls = Arc::new(Mutex::new(Vec::new()));
        let env = env();
        env.store
            .set_cursor(
                "jetstream:wss://js.example.test",
                &(T - 5_000_000).to_string(),
            )
            .unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        let task = {
            let (script, urls) = (Arc::clone(&script), Arc::clone(&urls));
            tokio::spawn(endpoint_loop(
                env,
                Url::parse("wss://js.example.test").unwrap(),
                tx,
                Duration::from_secs(5),
                move |url: Url| {
                    urls.lock().unwrap().push(url);
                    let next = script.lock().unwrap().pop_front();
                    async move {
                        next.map(|frames| Scripted {
                            frames: frames.into(),
                            hang: false,
                        })
                        .ok_or(HostError::Network("script over".into()))
                    }
                },
            ))
        };
        let mut items = Vec::new();
        while items.len() < 4 {
            items.push(
                tokio::time::timeout(Duration::from_secs(10), rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        task.abort();
        assert_eq!(
            times(&items),
            vec![
                ('e', T),
                ('e', T),
                ('s', T + 1_000_000),
                ('e', T + 1_500_000)
            ]
        );
        let cursors = urls
            .lock()
            .unwrap()
            .iter()
            .take(3)
            .map(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "cursor")
                    .unwrap()
                    .1
                    .parse::<i64>()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(cursors, vec![T - 15_000_000, T, T + 1_000_000]);
    }

    #[test]
    fn a_replayed_frame_does_not_reset_the_oversize_rule() {
        let mut state = ReaderState::default();
        assert_eq!(state.on_oversize(T), None);
        state.on_frame(T - 1);
        state.on_frame(T);
        assert_eq!(state.on_oversize(T), Some(T + 1_000_000));
        // Progress past the position starts a new count.
        assert_eq!(state.on_oversize(T + 2), None);
        state.on_frame(T + 3);
        assert_eq!(state.on_oversize(T + 3), None);
    }

    #[tokio::test]
    async fn non_utf8_binary_frames_are_skipped_not_fatal() {
        let env = env();
        let (tx, mut rx) = mpsc::channel(16);
        let mut socket = Scripted {
            frames: VecDeque::from([Ok(WsMessage::Binary(vec![0xff, 0xfe, 0xfd])), frame(T)]),
            hang: false,
        };
        let mut state = ReaderState::default();
        let (end, n) = read_connection(
            &mut socket,
            Duration::from_secs(5),
            T - 1,
            &mut state,
            "s",
            "o",
            &env,
            &tx,
        )
        .await;
        assert_eq!((end, n), (End::Closed, 1));
        assert!(matches!(rx.try_recv(), Ok(Item::Event { event, .. }) if event.time_us == T));
    }

    #[tokio::test]
    async fn a_silent_socket_is_replaced_after_the_idle_bound() {
        let env = env();
        let (tx, _rx) = mpsc::channel(16);
        let mut socket = Scripted {
            frames: VecDeque::from([frame(T)]),
            hang: true,
        };
        let mut state = ReaderState::default();
        let ended = tokio::time::timeout(
            Duration::from_secs(5),
            read_connection(
                &mut socket,
                Duration::from_millis(50),
                T - 1,
                &mut state,
                "s",
                "o",
                &env,
                &tx,
            ),
        )
        .await
        .expect("a half-open socket must not block the reader");
        assert_eq!(ended, (End::Idle, 1));
    }
}
