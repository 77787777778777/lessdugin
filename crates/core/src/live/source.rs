// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-driven Jetstream connections, subscription planning, cursors and recovery.
//!
//! Actionable frames remain unacknowledged until the integrator has applied them. The native
//! host negotiates and verifies `xrpc.v1.json`; the browser companion owns browser sockets.
//!
//! [`LiveSource::next`] is cancellation-safe: the leader-lock request, the connect and each
//! socket's receive in flight are kept in the source across calls, so a caller may drop `next`
//! at any await (`live_tail` races it with its timers). The frame such a receive gets is
//! returned by the next call; no host socket handle is ever asked for a frame twice.

use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use futures::lock::Mutex as AsyncMutex;
use lessdugin_atp::syntax::Nsid;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::host::{Host, HostError, HostFuture, KvOp, KvStore, WebSocket, WsMessage};
use crate::store::blob::{CursorRow, Wire as RowWire};
use crate::store::{row, schema};

use super::events::{Action, Context, EventFilter, Gap};
use super::wire::{self, EventKind, InfoKind, Wire};

/// `("cur", "jetstream")`.
pub const CURSOR_KEY: &str = "jetstream";
/// Shared v1/v2 LRU size (`sync.md` §6.3).
pub const DEDUP_CAPACITY: usize = 10_000;
/// Maximum decompressed WebSocket message.
pub const MAX_MESSAGE_BYTES: u32 = 1_200_000;
const PERSIST_MS: i64 = 5_000;
const PERSIST_EVENTS: u32 = 1_000;
const CURSOR_REWIND_US: i64 = 5_000_000;
const OUTDATED_US: i64 = 36 * 60 * 60 * 1_000_000;
const HEALTHY_FALLBACK_MS: i64 = 120_000;
const FILTER_RECONNECT_MS: i64 = 600_000;
const MAX_BACKOFF_MS: i64 = 60_000;
/// A gap streak (`#info` frames, `sync.md` §6.3) ends after this long without a gap while
/// frames arrive (*tunable*; longer than [`MAX_BACKOFF_MS`], so an endpoint that answers every
/// connection with a gap never ends its streak).
const GAP_QUIET_MS: i64 = 120_000;
/// At most one gap-triggered reconcile per endpoint in this long (ms, *tunable*; the hourly
/// authority reconcile period of `sync.md` §6.8), whatever its gap streaks.
const GAP_RECONCILE_MS: i64 = 3_600_000;
/// How long [`LiveSource::stop`] waits for a connect or lock request in flight, to close or
/// release what it returns (ms).
const STOP_WAIT_MS: u64 = 1_000;

/// Persisted Jetstream position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Wire that produced the cursor.
    pub wire: Wire,
    /// Endpoint instance (sequence numbers are only meaningful within one).
    pub instance: String,
    /// Inclusive v2 sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Last witnessed event time.
    pub time_us: i64,
}

impl Cursor {
    /// The `cur/jetstream` row (`sync.md` §10.5 `CursorRow`), stamped `now_ms`.
    fn to_row(&self, now_ms: i64) -> CursorRow {
        let mut r = CursorRow::new(now_ms);
        r.wire = Some(match self.wire {
            Wire::V1 => RowWire::V1,
            Wire::V2 => RowWire::V2,
        });
        r.instance = (!self.instance.is_empty()).then(|| self.instance.clone());
        r.seq = self.seq.and_then(|s| i64::try_from(s).ok());
        r.time_us = Some(self.time_us);
        r
    }

    /// The cursor of a stored row; `None` for another version or a row without a wire or time.
    fn from_row(r: CursorRow) -> Option<Self> {
        if r.v != row::ROW_VERSION {
            return None;
        }
        Some(Self {
            wire: match r.wire? {
                RowWire::V1 => Wire::V1,
                RowWire::V2 => Wire::V2,
            },
            instance: r.instance.unwrap_or_default(),
            seq: r.seq.and_then(|s| u64::try_from(s).ok()),
            time_us: r.time_us?,
        })
    }

    /// Query cursor for a connection. Same-instance v2 uses inclusive `seq`; everything else
    /// rewinds five seconds. Returns also whether the persisted cursor is too old to trust.
    pub fn resume(&self, instance: &str, wire: Wire, now_us: i64) -> (String, bool) {
        let outdated = now_us.saturating_sub(self.time_us) > OUTDATED_US;
        if outdated {
            return (now_us.saturating_sub(CURSOR_REWIND_US).to_string(), true);
        }
        if wire == Wire::V2
            && self.wire == Wire::V2
            && self.instance == instance
            && let Some(seq) = self.seq
        {
            return (seq.to_string(), false);
        }
        (
            self.time_us.saturating_sub(CURSOR_REWIND_US).to_string(),
            false,
        )
    }

    /// A time-only cursor at `now - 5 s` (a restarted cursor, `sync.md` §6.3).
    fn restart(wire: Wire, now_ms: i64) -> Self {
        Self {
            wire,
            instance: String::new(),
            seq: None,
            time_us: now_ms
                .saturating_mul(1_000)
                .saturating_sub(CURSOR_REWIND_US),
        }
    }
}

// Cursor query values are decimal strings (v2 seq or unix microseconds).
/// Live-source settings.
#[derive(Clone, Debug)]
pub struct LiveConfig {
    /// Jetstream base URLs, in failover order.
    pub endpoints: Vec<Url>,
    /// Build namespace (without `.*`).
    pub namespace: String,
    /// Browser filtering rules; native always uses v2 wildcard.
    pub browser: bool,
}

impl LiveConfig {
    /// Production-shaped configuration over explicit endpoints.
    pub fn new(endpoints: Vec<Url>, browser: bool) -> Self {
        Self {
            endpoints,
            namespace: lessdugin_lexicon::ns::NS.to_owned(),
            browser,
        }
    }
}

/// Wire/filter selection, separately testable from connection state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// v2 namespace wildcard, no participant DID filter.
    Wildcard,
    /// v2 with 1–150 explicit participant DIDs.
    V2Dids,
    /// v1 with `options_update`, for 151–10,000 participant DIDs.
    V1Dids,
    /// The browser's `filtered`-mode side connection while `F_new ≠ ∅`: v2 membership commits
    /// of every DID (`sync.md` §6.2).
    Membership,
}

impl FilterMode {
    /// The main connection's mode: `filtered` only in the browser, once participants are known
    /// and `D` holds 1–10,000 DIDs.
    pub fn select(browser: bool, participants_known: bool, dids: usize) -> Self {
        if !browser || !participants_known || dids == 0 || dids > 10_000 {
            FilterMode::Wildcard
        } else if dids <= 150 {
            FilterMode::V2Dids
        } else {
            FilterMode::V1Dids
        }
    }

    /// A participant DID filter (`filtered` mode).
    pub fn filtered(self) -> bool {
        matches!(self, FilterMode::V2Dids | FilterMode::V1Dids)
    }
}

/// One planned connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    /// Wire parser to use.
    pub wire: Wire,
    /// Selected mode.
    pub mode: FilterMode,
    /// Exact endpoint URL.
    pub url: Url,
    /// Frame to send immediately after connecting (v1 only).
    pub hello: Option<String>,
}

impl Subscription {
    /// Plans native/browser filtering. Empty or unknown participant sets and sets above 10,000
    /// use wildcard. DID filters are participant sets, never a host-wide shared filter.
    pub fn plan(
        base: &Url,
        namespace: &str,
        browser: bool,
        participants_known: bool,
        dids: &[String],
        cursor: Option<&Cursor>,
        now_us: i64,
    ) -> Result<Self, LiveError> {
        Nsid::parse(namespace).map_err(|e| LiveError::Config(e.to_string()))?;
        let instance = instance(base);
        let mode = FilterMode::select(browser, participants_known, dids.len());
        let wire = if mode == FilterMode::V1Dids {
            Wire::V1
        } else {
            Wire::V2
        };
        let mut url = base.clone();
        url.set_path(if wire == Wire::V2 {
            "/xrpc/network.bsky.jetstream.subscribeEvents"
        } else {
            "/subscribe"
        });
        url.set_query(None);
        url.set_fragment(None);
        let resume = cursor.map(|c| c.resume(&instance, wire, now_us).0);
        {
            let mut q = url.query_pairs_mut();
            if wire == Wire::V2 {
                q.append_pair("collections", &format!("{namespace}.*"));
                if mode == FilterMode::V2Dids {
                    for did in dids {
                        q.append_pair("dids", did);
                    }
                    // No kinds=commit filter: identity/account events for these DIDs are
                    // required even when commit traffic is filtered.
                }
            } else {
                q.append_pair("wantedCollections", &format!("{namespace}.*"));
                q.append_pair("requireHello", "true");
            }
            if let Some(c) = resume {
                q.append_pair("cursor", &c.to_string());
            } else {
                q.append_pair(
                    "cursor",
                    &now_us.saturating_sub(CURSOR_REWIND_US).to_string(),
                );
            }
        }
        let hello = (wire == Wire::V1).then(|| {
            serde_json::json!({
                "type": "options_update",
                "payload": {
                    "wantedCollections": [format!("{namespace}.*")],
                    "wantedDids": dids,
                    "maxMessageSizeBytes": MAX_MESSAGE_BYTES,
                }
            })
            .to_string()
        });
        Ok(Self {
            wire,
            mode,
            url,
            hello,
        })
    }

    /// Plans the `filtered`-mode side connection (`sync.md` §6.2): v2
    /// `collections=<NS>.membership&kinds=commit` with no DID filter, resumed from `cursor`.
    pub fn membership(
        base: &Url,
        namespace: &str,
        cursor: Option<&Cursor>,
        now_us: i64,
    ) -> Result<Self, LiveError> {
        Nsid::parse(namespace).map_err(|e| LiveError::Config(e.to_string()))?;
        let resume = cursor.map_or_else(
            || now_us.saturating_sub(CURSOR_REWIND_US).to_string(),
            |c| c.resume(&instance(base), Wire::V2, now_us).0,
        );
        let mut url = base.clone();
        url.set_path("/xrpc/network.bsky.jetstream.subscribeEvents");
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("collections", &format!("{namespace}.membership"))
            .append_pair("kinds", "commit")
            .append_pair("cursor", &resume);
        Ok(Self {
            wire: Wire::V2,
            mode: FilterMode::Membership,
            url,
            hello: None,
        })
    }
}

fn instance(base: &Url) -> String {
    match (base.host_str(), base.port_or_known_default()) {
        (Some(h), Some(p)) => format!("{}://{h}:{p}", base.scheme()),
        _ => base.as_str().to_owned(),
    }
}

/// Errors of durable cursor I/O or invalid local configuration. Remote malformed events and
/// socket failures are logged and recovered, not returned as fatal errors.
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    /// Host/store failure.
    #[error(transparent)]
    Host(#[from] HostError),
    /// Cursor row encoding failure.
    #[error("cursor row: {0}")]
    Row(#[from] row::RowError),
    /// Invalid local endpoint/namespace.
    #[error("live source configuration: {0}")]
    Config(String),
}

/// Result of one driven source step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SourceStatus {
    /// Actions parsed from one frame (or queued by recovery).
    Actions(Vec<Action>),
    /// No action; wait for the next frame/retry and continue other engine work.
    #[default]
    Waiting,
    /// Another engine/tab holds `sync-leader`; no socket was opened.
    LeaderUnavailable,
    /// [`LiveSource::stop`] was called.
    Stopped,
}

#[derive(Clone, Debug)]
struct PendingFrame {
    actions: Vec<Action>,
    cursor: Option<Cursor>,
    restart_cursor: bool,
    /// From the membership side connection (its cursor is the side cursor).
    side: bool,
}

type Received = Option<Result<WsMessage, HostError>>;
type Connecting<W> = HostFuture<'static, (Subscription, String, Result<W, HostError>)>;

/// One open socket. Its receive in flight lives here across [`LiveSource::next`] calls, so a
/// dropped `next` never abandons a host receive (a browser handle would hand that frame to the
/// abandoned receive). The socket is shared with that receive, so it can always be closed.
struct Conn<W> {
    ws: Arc<AsyncMutex<W>>,
    recv: Option<HostFuture<'static, Received>>,
    sub: Subscription,
    instance: String,
}

impl<W: WebSocket + 'static> Conn<W> {
    fn new(ws: W, sub: Subscription, instance: String) -> Self {
        Self {
            ws: Arc::new(AsyncMutex::new(ws)),
            recv: None,
            sub,
            instance,
        }
    }

    /// Polls the receive in flight, starting one if none is.
    fn poll_recv(&mut self, cx: &mut TaskContext<'_>) -> Poll<Received> {
        let ws = &self.ws;
        let recv = self.recv.get_or_insert_with(|| {
            let ws = Arc::clone(ws);
            Box::pin(async move { ws.lock().await.next().await })
        });
        let out = recv.as_mut().poll(cx);
        if out.is_ready() {
            self.recv = None;
        }
        out
    }

    async fn close(mut self) {
        // The receive in flight holds the socket; dropping it frees the socket for the close.
        self.recv = None;
        self.ws.lock().await.close().await;
    }
}

/// Awaits the future in `slot` and clears the slot. Dropped before it completes, it leaves the
/// future in the slot for the next call.
async fn drive<T>(slot: &mut Option<HostFuture<'static, T>>) -> Option<T> {
    let out = futures::future::poll_fn(|cx| match slot.as_mut() {
        Some(f) => f.as_mut().poll(cx).map(Some),
        None => Poll::Ready(None),
    })
    .await;
    *slot = None;
    out
}

enum Io<W> {
    Main(Received),
    Side(Received),
    SideConnected(Subscription, String, Result<W, HostError>),
}

/// A single host-driven live source. The optional sockets and leader lock are owned by this
/// value; [`LiveSource::stop`] closes the sockets and releases the lock.
pub struct LiveSource<H: Host> {
    host: Arc<H>,
    config: LiveConfig,
    filter: EventFilter,
    main: Option<Conn<H::Ws>>,
    connecting: Option<Connecting<H::Ws>>,
    side: Option<Conn<H::Ws>>,
    side_connecting: Option<Connecting<H::Ws>>,
    side_cursor: Option<Cursor>,
    side_failures: u32,
    side_retry_at_ms: i64,
    side_first: bool,
    locking: Option<HostFuture<'static, Result<Option<H::Lock>, HostError>>>,
    leader: Option<H::Lock>,
    cursor: Option<Cursor>,
    loaded: bool,
    endpoint: usize,
    failures: u32,
    gaps: u32,
    last_gap_ms: Option<i64>,
    /// The current gap streak already had an `OutdatedCursor`.
    streak_outdated: bool,
    /// When each endpoint instance's last gap-triggered reconcile was scheduled (ms).
    gap_reconciles: std::collections::BTreeMap<String, i64>,
    retry_at_ms: i64,
    last_saved_ms: i64,
    events_since_save: u32,
    last_healthy_ms: Option<i64>,
    oversized: Option<(Option<Cursor>, u8)>,
    pending: Vec<Action>,
    unacked: Option<PendingFrame>,
    planned_dids: Option<Vec<String>>,
    planned_participants_known: bool,
    last_filter_ms: i64,
    stopped: bool,
}

impl<H: Host> LiveSource<H> {
    /// A stopped-at-start source with no socket and no leader lock.
    pub fn new(host: Arc<H>, config: LiveConfig) -> Self {
        Self {
            host,
            config,
            filter: EventFilter::default(),
            main: None,
            connecting: None,
            side: None,
            side_connecting: None,
            side_cursor: None,
            side_failures: 0,
            side_retry_at_ms: 0,
            side_first: false,
            locking: None,
            leader: None,
            cursor: None,
            loaded: false,
            endpoint: 0,
            failures: 0,
            gaps: 0,
            last_gap_ms: None,
            streak_outdated: false,
            gap_reconciles: std::collections::BTreeMap::new(),
            retry_at_ms: 0,
            last_saved_ms: 0,
            events_since_save: 0,
            last_healthy_ms: None,
            oversized: None,
            pending: Vec::new(),
            unacked: None,
            planned_dids: None,
            planned_participants_known: false,
            last_filter_ms: 0,
            stopped: false,
        }
    }

    /// Shared dedup/filter state (useful for diagnostics).
    pub fn filter(&self) -> &EventFilter {
        &self.filter
    }

    /// The deduplication LRU, which the indexer change feed shares (`sync.md` §6.6).
    pub fn dedup_mut(&mut self) -> &mut super::events::Dedup {
        self.filter.dedup_mut()
    }

    /// Current in-memory cursor.
    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }

    /// True when polling must run because no valid live frame was seen for two minutes.
    pub fn polling_enabled(&self) -> bool {
        self.last_healthy_ms
            .is_none_or(|t| self.host.now_ms().saturating_sub(t) >= HEALTHY_FALLBACK_MS)
    }

    /// Milliseconds before a reconnect may be attempted.
    pub fn retry_in_ms(&self) -> i64 {
        if self.main.is_some() || self.connecting.is_some() {
            return 0;
        }
        self.retry_at_ms.saturating_sub(self.host.now_ms()).max(0)
    }

    /// Stops and closes the sockets. There are no background tasks to join; a connect or lock
    /// request in flight is awaited briefly, so what it returns is closed or released.
    pub async fn stop(&mut self) {
        self.stopped = true;
        for slot in [&mut self.connecting, &mut self.side_connecting] {
            if let Some(f) = slot.take() {
                let sleep = self.host.sleep(STOP_WAIT_MS);
                futures::pin_mut!(sleep);
                if let futures::future::Either::Left(((_, _, Ok(mut ws)), _)) =
                    futures::future::select(f, sleep).await
                {
                    ws.close().await;
                }
            }
        }
        if let Some(f) = self.locking.take() {
            let sleep = self.host.sleep(STOP_WAIT_MS);
            futures::pin_mut!(sleep);
            // a lock that arrives now is dropped, which releases it
            let _ = futures::future::select(f, sleep).await;
        }
        self.close_main().await;
        if let Some(side) = self.side.take() {
            side.close().await;
        }
        self.leader = None;
    }

    async fn close_main(&mut self) {
        self.connecting = None;
        if let Some(main) = self.main.take() {
            main.close().await;
        }
    }

    async fn load_cursor(&mut self) -> Result<(), LiveError> {
        if self.loaded {
            return Ok(());
        }
        let key = schema::cur_key(CURSOR_KEY);
        let raw = self.host.cache().get_many(vec![key]).await?.pop().flatten();
        // Only once the row is read: a caller that dropped the read reads it again.
        self.loaded = true;
        self.cursor = raw.and_then(|bytes| match row::decode::<CursorRow>(&bytes) {
            Ok(r) => Cursor::from_row(r),
            Err(e) => {
                tracing::warn!(error = %e, "unreadable Jetstream cursor row ignored");
                None
            }
        });
        self.last_saved_ms = self.host.now_ms();
        if let Some(c) = &self.cursor {
            let now_us = self.host.now_ms().saturating_mul(1_000);
            if now_us.saturating_sub(c.time_us) > OUTDATED_US {
                self.cursor = None;
                self.pending.push(Action::Reconcile(Gap::OutdatedCursor));
            }
        }
        Ok(())
    }

    async fn save_cursor(&mut self, force: bool) -> Result<(), LiveError> {
        let Some(cursor) = self.cursor.clone() else {
            return Ok(());
        };
        let now = self.host.now_ms();
        if !force
            && self.events_since_save < PERSIST_EVENTS
            && now.saturating_sub(self.last_saved_ms) < PERSIST_MS
        {
            return Ok(());
        }
        let bytes = row::encode(&cursor.to_row(now))?;
        self.host
            .cache()
            .write(vec![KvOp::Put(schema::cur_key(CURSOR_KEY), bytes)])
            .await?;
        self.last_saved_ms = now;
        self.events_since_save = 0;
        Ok(())
    }

    async fn commit_frame(&mut self, frame: &PendingFrame) -> Result<(), LiveError> {
        if frame.side {
            if frame.restart_cursor {
                self.side_cursor = Some(Cursor::restart(Wire::V2, self.host.now_ms()));
            } else if let Some(cursor) = &frame.cursor {
                self.side_cursor = Some(cursor.clone());
            }
            return Ok(());
        }
        if frame.restart_cursor {
            let wire = self.main.as_ref().map_or(Wire::V2, |c| c.sub.wire);
            self.cursor = Some(Cursor::restart(wire, self.host.now_ms()));
            self.close_main().await;
            self.disconnected();
            self.save_cursor(true).await?;
        } else if let Some(cursor) = &frame.cursor {
            self.cursor = Some(cursor.clone());
            self.save_cursor(false).await?;
        }
        Ok(())
    }

    /// Acknowledges actions returned by [`LiveSource::next`] after their cache/queue updates have
    /// succeeded. Only then does the source cursor advance. Failure leaves the frame buffered.
    pub async fn acknowledge(&mut self) -> Result<(), LiveError> {
        let Some(frame) = self.unacked.clone() else {
            return Ok(());
        };
        self.commit_frame(&frame).await?;
        self.filter.acknowledge();
        self.unacked = None;
        Ok(())
    }

    fn take_pending(&mut self) -> Option<SourceStatus> {
        if self.pending.is_empty() {
            return None;
        }
        let actions = std::mem::take(&mut self.pending);
        self.unacked = Some(PendingFrame {
            actions: actions.clone(),
            cursor: None,
            restart_cursor: false,
            side: false,
        });
        Some(SourceStatus::Actions(actions))
    }

    /// `min(60 s, 1 s · 2^(n−1)) · uniform(0.8, 1.2)` (`sync.md` §6.2).
    fn backoff_ms(&self, n: u32) -> i64 {
        let shift = n.saturating_sub(1).min(6);
        let base = 1_000i64.saturating_mul(1i64 << shift).min(MAX_BACKOFF_MS);
        let mut random = [0u8; 8];
        self.host.fill_random(&mut random);
        let r = u64::from_le_bytes(random);
        // [0.8, 1.2], integer arithmetic only.
        let factor = 800 + (r % 401) as i64;
        base.saturating_mul(factor) / 1_000
    }

    /// The main connection ended (it was closed or failed). Within a gap streak `n` counts the
    /// gaps too, so an endpoint that answers every connection with a gap backs off and fails
    /// over like one that fails.
    fn disconnected(&mut self) {
        self.main = None;
        self.connecting = None;
        self.failures = self.failures.saturating_add(1);
        let n = self.failures.max(self.gaps);
        if n.is_multiple_of(3) && !self.config.endpoints.is_empty() {
            self.endpoint = (self.endpoint + 1) % self.config.endpoints.len();
        }
        self.retry_at_ms = self.host.now_ms().saturating_add(self.backoff_ms(n));
    }

    fn side_disconnected(&mut self) {
        self.side = None;
        self.side_connecting = None;
        self.side_failures = self.side_failures.saturating_add(1);
        self.side_retry_at_ms = self
            .host
            .now_ms()
            .saturating_add(self.backoff_ms(self.side_failures));
    }

    fn recover_oversize(&mut self) -> Option<PendingFrame> {
        let at = self.cursor.clone();
        let count = match &self.oversized {
            Some((previous, n)) if *previous == at => n.saturating_add(1),
            _ => 1,
        };
        if count >= 2 {
            let time_us = at.map_or_else(
                || self.host.now_ms().saturating_mul(1_000),
                |cursor| cursor.time_us,
            );
            let cursor = Cursor {
                wire: self.main.as_ref().map_or(Wire::V2, |c| c.sub.wire),
                // Force time-based resume even on the same v2 instance.
                instance: String::new(),
                seq: None,
                time_us: time_us.saturating_add(1_000_000),
            };
            self.oversized = None;
            Some(PendingFrame {
                actions: vec![Action::Reconcile(Gap::OversizedMessage)],
                cursor: Some(cursor),
                restart_cursor: false,
                side: false,
            })
        } else {
            self.oversized = Some((at, count));
            None
        }
    }

    fn connect(&self, plan: Subscription, instance: String) -> Connecting<H::Ws> {
        let host = Arc::clone(&self.host);
        Box::pin(async move {
            let result = async {
                let mut ws = host.websocket(plan.url.clone(), MAX_MESSAGE_BYTES).await?;
                if let Some(hello) = &plan.hello
                    && let Err(e) = ws.send_text(hello.clone()).await
                {
                    ws.close().await;
                    return Err(e);
                }
                Ok(ws)
            }
            .await;
            (plan, instance, result)
        })
    }

    fn poll_io(&mut self, cx: &mut TaskContext<'_>) -> Poll<Io<H::Ws>> {
        if let Some(f) = self.side_connecting.as_mut()
            && let Poll::Ready((plan, instance, result)) = f.as_mut().poll(cx)
        {
            self.side_connecting = None;
            return Poll::Ready(Io::SideConnected(plan, instance, result));
        }
        // Alternate which socket goes first, so a busy one cannot starve the other.
        for side in [self.side_first, !self.side_first] {
            if side {
                if let Some(c) = self.side.as_mut()
                    && let Poll::Ready(m) = c.poll_recv(cx)
                {
                    self.side_first = false;
                    return Poll::Ready(Io::Side(m));
                }
            } else if let Some(c) = self.main.as_mut()
                && let Poll::Ready(m) = c.poll_recv(cx)
            {
                self.side_first = true;
                return Poll::Ready(Io::Main(m));
            }
        }
        Poll::Pending
    }

    /// Drives at most one socket connect or frame. It never spawns work and never sleeps in a
    /// retry loop; callers use [`LiveSource::retry_in_ms`] to schedule the next call.
    pub async fn next(&mut self, cx: &Context) -> Result<SourceStatus, LiveError> {
        if self.stopped {
            return Ok(SourceStatus::Stopped);
        }
        if let Some(frame) = &self.unacked {
            return Ok(SourceStatus::Actions(frame.actions.clone()));
        }
        self.load_cursor().await?;
        if let Some(status) = self.take_pending() {
            return Ok(status);
        }
        let mut dids = cx.filter_dids.clone();
        dids.sort();
        dids.dedup();
        if self.config.browser
            && let Some(main) = &self.main
        {
            let filter_changed = self.planned_dids.as_ref().is_some_and(|old| {
                *old != dids || self.planned_participants_known != cx.participants_known
            });
            // The first switch to `filtered` is made at once; later changes of D at most once
            // per 10 minutes (`sync.md` §6.2).
            let first_filter = main.sub.mode == FilterMode::Wildcard
                && FilterMode::select(true, cx.participants_known, dids.len()).filtered();
            if filter_changed
                && (first_filter
                    || self.host.now_ms().saturating_sub(self.last_filter_ms)
                        >= FILTER_RECONNECT_MS)
            {
                self.close_main().await;
                self.save_cursor(true).await?;
            }
        }
        if self.main.is_none() {
            if self.connecting.is_none() {
                if self.host.now_ms() < self.retry_at_ms {
                    return Ok(SourceStatus::Waiting);
                }
                if self.leader.is_none() {
                    if self.locking.is_none() {
                        let host = Arc::clone(&self.host);
                        self.locking =
                            Some(Box::pin(
                                async move { host.lock("sync-leader", false).await },
                            ));
                    }
                    let Some(lock) = drive(&mut self.locking).await.transpose()?.flatten() else {
                        self.retry_at_ms = self.host.now_ms().saturating_add(1_000);
                        return Ok(SourceStatus::LeaderUnavailable);
                    };
                    self.leader = Some(lock);
                }
                if self.config.endpoints.is_empty() {
                    self.retry_at_ms = self.host.now_ms().saturating_add(1_000);
                    return Ok(SourceStatus::Waiting);
                }
                let base = &self.config.endpoints[self.endpoint % self.config.endpoints.len()];
                let now_us = self.host.now_ms().saturating_mul(1_000);
                if self
                    .cursor
                    .as_ref()
                    .is_some_and(|c| now_us.saturating_sub(c.time_us) > OUTDATED_US)
                {
                    self.cursor = None;
                    self.pending.push(Action::Reconcile(Gap::OutdatedCursor));
                }
                if self.config.browser {
                    // DIDs new in D missed their events only if the last plan filtered DIDs
                    // (a wildcard connection received everything).
                    let was_filtered = self.planned_dids.as_ref().is_some_and(|old| {
                        FilterMode::select(true, self.planned_participants_known, old.len())
                            .filtered()
                    });
                    if let Some(old) = self.planned_dids.as_ref().filter(|_| was_filtered) {
                        let old: std::collections::BTreeSet<&str> =
                            old.iter().map(String::as_str).collect();
                        for did in dids.iter().filter(|did| !old.contains(did.as_str())) {
                            self.pending.push(Action::CatchUp { did: did.clone() });
                        }
                    }
                    self.planned_dids = Some(dids.clone());
                    self.planned_participants_known = cx.participants_known;
                }
                let plan = Subscription::plan(
                    base,
                    &self.config.namespace,
                    self.config.browser,
                    cx.participants_known,
                    &cx.filter_dids,
                    self.cursor.as_ref(),
                    now_us,
                )?;
                self.connecting = Some(self.connect(plan, instance(base)));
            }
            let Some((plan, instance, result)) = drive(&mut self.connecting).await else {
                return Ok(SourceStatus::Waiting);
            };
            match result {
                Ok(socket) => {
                    if plan.mode.filtered() {
                        self.last_filter_ms = self.host.now_ms();
                    }
                    self.main = Some(Conn::new(socket, plan, instance));
                }
                Err(e) => {
                    tracing::debug!(error = %e, "Jetstream connection failed");
                    self.disconnected();
                    return Ok(SourceStatus::Waiting);
                }
            }
        }
        if let Some(status) = self.take_pending() {
            return Ok(status);
        }
        let side_wanted = self.config.browser
            && self.main.as_ref().is_some_and(|c| c.sub.mode.filtered())
            && !cx.newcomer_forums.is_empty();
        if !side_wanted {
            self.side_connecting = None;
            if let Some(side) = self.side.take() {
                side.close().await;
            }
        }
        loop {
            if side_wanted
                && self.side.is_none()
                && self.side_connecting.is_none()
                && self.host.now_ms() >= self.side_retry_at_ms
            {
                let base = &self.config.endpoints[self.endpoint % self.config.endpoints.len()];
                let now_us = self.host.now_ms().saturating_mul(1_000);
                // Its own cursor, else the main one (a restart resumes from the stored row).
                let from = self.side_cursor.as_ref().or(self.cursor.as_ref());
                let plan = Subscription::membership(base, &self.config.namespace, from, now_us)?;
                self.side_connecting = Some(self.connect(plan, instance(base)));
            }
            match futures::future::poll_fn(|tcx| self.poll_io(tcx)).await {
                Io::SideConnected(plan, instance, Ok(socket)) => {
                    self.side = Some(Conn::new(socket, plan, instance));
                }
                Io::SideConnected(_, _, Err(e)) => {
                    tracing::debug!(error = %e, "Jetstream membership connection failed");
                    self.side_disconnected();
                }
                Io::Main(message) => return self.on_main(message, cx).await,
                Io::Side(message) => return Ok(self.on_side(message, cx).await),
            }
        }
    }

    fn text(message: WsMessage) -> Option<String> {
        match message {
            WsMessage::Text(t) => Some(t),
            WsMessage::Binary(b) => match String::from_utf8(b) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::debug!(error = %e, "non-UTF8 Jetstream frame ignored");
                    None
                }
            },
        }
    }

    async fn on_main(
        &mut self,
        message: Received,
        cx: &Context,
    ) -> Result<SourceStatus, LiveError> {
        match message {
            None => {
                self.disconnected();
                self.save_cursor(true).await?;
                Ok(SourceStatus::Waiting)
            }
            Some(Err(HostError::TooLarge)) => {
                let frame = self.recover_oversize();
                self.disconnected();
                if let Some(frame) = frame {
                    let actions = frame.actions.clone();
                    self.unacked = Some(frame);
                    Ok(SourceStatus::Actions(actions))
                } else {
                    Ok(SourceStatus::Waiting)
                }
            }
            Some(Err(e)) => {
                tracing::debug!(error = %e, "Jetstream socket failed");
                self.disconnected();
                Ok(SourceStatus::Waiting)
            }
            Some(Ok(message)) => {
                let Some(text) = Self::text(message) else {
                    return Ok(SourceStatus::Waiting);
                };
                let wire = self.main.as_ref().map_or(Wire::V2, |c| c.sub.wire);
                let event = match wire::parse(wire, &text) {
                    Ok(Some(e)) => e,
                    Ok(None) => {
                        tracing::debug!("unknown Jetstream payload ignored");
                        return Ok(SourceStatus::Waiting);
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "malformed Jetstream frame ignored");
                        return Ok(SourceStatus::Waiting);
                    }
                };
                let now = self.host.now_ms();
                self.oversized = None;
                let instance = self
                    .main
                    .as_ref()
                    .map(|c| c.instance.clone())
                    .unwrap_or_default();
                let gap = matches!(event.kind, EventKind::Info(_));
                let outdated = matches!(event.kind, EventKind::Info(InfoKind::OutdatedCursor));
                // A gap is no sign of health: it neither resets the reconnect back-off nor
                // keeps polling off. Only the first OutdatedCursor of a streak schedules a
                // reconcile, and at most one per endpoint per GAP_RECONCILE_MS.
                let reconcile = outdated
                    && !self.streak_outdated
                    && self
                        .gap_reconciles
                        .get(&instance)
                        .is_none_or(|t| now.saturating_sub(*t) >= GAP_RECONCILE_MS);
                if gap {
                    self.gaps = self.gaps.saturating_add(1);
                    self.last_gap_ms = Some(now);
                    self.streak_outdated |= outdated;
                } else {
                    self.failures = 0;
                    self.last_healthy_ms = Some(now);
                    if self
                        .last_gap_ms
                        .is_none_or(|t| now.saturating_sub(t) >= GAP_QUIET_MS)
                    {
                        self.gaps = 0;
                        self.streak_outdated = false;
                    }
                }
                let mut out =
                    self.filter
                        .apply(wire, &instance, event, cx, now.saturating_mul(1_000));
                if gap && !out.actions.is_empty() {
                    if reconcile {
                        self.gap_reconciles.insert(instance, now);
                    } else {
                        tracing::debug!("Jetstream gap: a reconcile was scheduled recently");
                        self.filter.acknowledge();
                        out.actions.clear();
                    }
                }
                self.events_since_save = self.events_since_save.saturating_add(1);
                let frame = PendingFrame {
                    actions: out.actions,
                    cursor: out.cursor,
                    restart_cursor: out.restart_cursor,
                    side: false,
                };
                if frame.actions.is_empty() {
                    self.commit_frame(&frame).await?;
                    Ok(SourceStatus::Waiting)
                } else {
                    let actions = frame.actions.clone();
                    self.unacked = Some(frame);
                    Ok(SourceStatus::Actions(actions))
                }
            }
        }
    }

    /// A frame of the membership side connection. Its cursor stays in memory; a gap restarts it
    /// near now and schedules no reconcile (the main connection's gaps do that).
    async fn on_side(&mut self, message: Received, cx: &Context) -> SourceStatus {
        let message = match message {
            Some(Ok(m)) => m,
            Some(Err(HostError::TooLarge)) => {
                // Skip past the oversized frame instead of reconnecting into it.
                let at = self
                    .side_cursor
                    .as_ref()
                    .or(self.cursor.as_ref())
                    .map_or_else(|| self.host.now_ms().saturating_mul(1_000), |c| c.time_us);
                self.side_cursor = Some(Cursor {
                    wire: Wire::V2,
                    instance: String::new(),
                    seq: None,
                    time_us: at.saturating_add(1_000_000),
                });
                self.side_disconnected();
                return SourceStatus::Waiting;
            }
            Some(Err(e)) => {
                tracing::debug!(error = %e, "Jetstream membership socket failed");
                self.side_disconnected();
                return SourceStatus::Waiting;
            }
            None => {
                self.side_disconnected();
                return SourceStatus::Waiting;
            }
        };
        let Some(text) = Self::text(message) else {
            return SourceStatus::Waiting;
        };
        let event = match wire::parse(Wire::V2, &text) {
            Ok(Some(e)) => e,
            Ok(None) => return SourceStatus::Waiting,
            Err(e) => {
                tracing::debug!(error = %e, "malformed Jetstream membership frame ignored");
                return SourceStatus::Waiting;
            }
        };
        let now = self.host.now_ms();
        if matches!(event.kind, EventKind::Info(_)) {
            self.side_cursor = Some(Cursor::restart(Wire::V2, now));
            if let Some(side) = self.side.take() {
                side.close().await;
            }
            self.side_disconnected();
            return SourceStatus::Waiting;
        }
        self.side_failures = 0;
        let instance = self
            .side
            .as_ref()
            .map(|c| c.instance.clone())
            .unwrap_or_default();
        let out = self
            .filter
            .apply(Wire::V2, &instance, event, cx, now.saturating_mul(1_000));
        if out.actions.is_empty() {
            if let Some(cursor) = out.cursor {
                self.side_cursor = Some(cursor);
            }
            return SourceStatus::Waiting;
        }
        let actions = out.actions.clone();
        self.unacked = Some(PendingFrame {
            actions: out.actions,
            cursor: out.cursor,
            restart_cursor: false,
            side: true,
        });
        SourceStatus::Actions(actions)
    }
}
