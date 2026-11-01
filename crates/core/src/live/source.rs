// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host-driven Jetstream connections, subscription planning, cursors and recovery.
//!
//! Actionable frames remain unacknowledged until the integrator has applied them. The native
//! host negotiates and verifies `xrpc.v1.json`; the browser companion owns browser sockets.

use std::sync::Arc;

use lessdugin_atp::syntax::Nsid;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::host::{Host, HostError, KvOp, KvStore, WebSocket, WsMessage};
use crate::store::{row, schema};

use super::events::{Action, Context, EventFilter, Gap};
use super::wire::{self, Wire};

/// `("cur", "jetstream")`.
pub const CURSOR_KEY: &str = "jetstream";
/// Shared v1/v2 LRU size (`sync.md` §6.3).
pub const DEDUP_CAPACITY: usize = 10_000;
/// Maximum decompressed WebSocket message.
pub const MAX_MESSAGE_BYTES: u32 = 1_200_000;
const CURSOR_ROW_VERSION: u32 = 1;
const PERSIST_MS: i64 = 5_000;
const PERSIST_EVENTS: u32 = 1_000;
const CURSOR_REWIND_US: i64 = 5_000_000;
const OUTDATED_US: i64 = 36 * 60 * 60 * 1_000_000;
const HEALTHY_FALLBACK_MS: i64 = 120_000;
const FILTER_RECONNECT_MS: i64 = 600_000;
const MAX_BACKOFF_MS: i64 = 60_000;

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

#[derive(Serialize, Deserialize)]
struct CursorRow {
    v: u32,
    #[serde(flatten)]
    cursor: Cursor,
}

impl Cursor {
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
        let mode = if !browser || !participants_known || dids.is_empty() || dids.len() > 10_000 {
            FilterMode::Wildcard
        } else if dids.len() <= 150 {
            FilterMode::V2Dids
        } else {
            FilterMode::V1Dids
        };
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
}

/// A single host-driven live source. The optional socket and leader lock are owned by this value;
/// dropping it closes the socket by dropping the host stream and releases the lock.
pub struct LiveSource<H: Host> {
    host: Arc<H>,
    config: LiveConfig,
    filter: EventFilter,
    socket: Option<H::Ws>,
    leader: Option<H::Lock>,
    subscription: Option<Subscription>,
    cursor: Option<Cursor>,
    loaded: bool,
    endpoint: usize,
    failures: u32,
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
            socket: None,
            leader: None,
            subscription: None,
            cursor: None,
            loaded: false,
            endpoint: 0,
            failures: 0,
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
        self.retry_at_ms.saturating_sub(self.host.now_ms()).max(0)
    }

    /// Stops and closes the current socket. There are no background tasks to join.
    pub async fn stop(&mut self) {
        self.stopped = true;
        if let Some(mut socket) = self.socket.take() {
            socket.close().await;
        }
        self.subscription = None;
        self.leader = None;
    }

    async fn load_cursor(&mut self) -> Result<(), LiveError> {
        if self.loaded {
            return Ok(());
        }
        self.loaded = true;
        let key = schema::cur_key(CURSOR_KEY);
        let raw = self.host.cache().get_many(vec![key]).await?.pop().flatten();
        self.cursor = raw.and_then(|bytes| {
            row::decode::<CursorRow>(&bytes)
                .ok()
                .filter(|r| r.v == CURSOR_ROW_VERSION)
                .map(|r| r.cursor)
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
        let bytes = row::encode(&CursorRow {
            v: CURSOR_ROW_VERSION,
            cursor,
        })?;
        self.host
            .cache()
            .write(vec![KvOp::Put(schema::cur_key(CURSOR_KEY), bytes)])
            .await?;
        self.last_saved_ms = now;
        self.events_since_save = 0;
        Ok(())
    }

    async fn commit_frame(&mut self, frame: &PendingFrame) -> Result<(), LiveError> {
        if frame.restart_cursor {
            self.cursor = Some(Cursor {
                wire: self.subscription.as_ref().map_or(Wire::V2, |s| s.wire),
                instance: String::new(),
                seq: None,
                time_us: self
                    .host
                    .now_ms()
                    .saturating_mul(1_000)
                    .saturating_sub(CURSOR_REWIND_US),
            });
            self.save_cursor(true).await?;
            if let Some(mut socket) = self.socket.take() {
                socket.close().await;
            }
            self.disconnected();
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
        });
        Some(SourceStatus::Actions(actions))
    }

    fn disconnected(&mut self) {
        self.socket = None;
        self.subscription = None;
        self.failures = self.failures.saturating_add(1);
        if self.failures.is_multiple_of(3) && !self.config.endpoints.is_empty() {
            self.endpoint = (self.endpoint + 1) % self.config.endpoints.len();
        }
        let shift = self.failures.saturating_sub(1).min(6);
        let base = 1_000i64.saturating_mul(1i64 << shift).min(MAX_BACKOFF_MS);
        let mut random = [0u8; 8];
        self.host.fill_random(&mut random);
        let r = u64::from_le_bytes(random);
        // [0.8, 1.2], integer arithmetic only.
        let factor = 800 + (r % 401) as i64;
        let delay = base.saturating_mul(factor) / 1_000;
        self.retry_at_ms = self.host.now_ms().saturating_add(delay);
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
                wire: self.subscription.as_ref().map_or(Wire::V2, |s| s.wire),
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
            })
        } else {
            self.oversized = Some((at, count));
            None
        }
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
        let filter_changed = self.planned_dids.as_ref().is_some_and(|old| {
            *old != dids || self.planned_participants_known != cx.participants_known
        });
        if self.config.browser
            && self.socket.is_some()
            && filter_changed
            && self.host.now_ms().saturating_sub(self.last_filter_ms) >= FILTER_RECONNECT_MS
        {
            self.save_cursor(true).await?;
            if let Some(mut socket) = self.socket.take() {
                socket.close().await;
            }
            self.subscription = None;
        }
        if self.socket.is_none() {
            if self.host.now_ms() < self.retry_at_ms {
                return Ok(SourceStatus::Waiting);
            }
            if self.leader.is_none() {
                let Some(lock) = self.host.lock("sync-leader", false).await? else {
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
                if let Some(old) = &self.planned_dids {
                    let old: std::collections::BTreeSet<&str> =
                        old.iter().map(String::as_str).collect();
                    for did in dids.iter().filter(|did| !old.contains(did.as_str())) {
                        self.pending.push(Action::CatchUp { did: did.clone() });
                    }
                }
                self.planned_dids = Some(dids.clone());
                self.planned_participants_known = cx.participants_known;
            }
            self.last_filter_ms = self.host.now_ms();
            let plan = Subscription::plan(
                base,
                &self.config.namespace,
                self.config.browser,
                cx.participants_known,
                &cx.filter_dids,
                self.cursor.as_ref(),
                now_us,
            )?;
            match self
                .host
                .websocket(plan.url.clone(), MAX_MESSAGE_BYTES)
                .await
            {
                Ok(mut socket) => {
                    if let Some(hello) = &plan.hello
                        && let Err(e) = socket.send_text(hello.clone()).await
                    {
                        tracing::debug!(error = %e, "Jetstream options_update failed");
                        self.disconnected();
                        return Ok(SourceStatus::Waiting);
                    }
                    self.subscription = Some(plan);
                    self.socket = Some(socket);
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
        let Some(socket) = self.socket.as_mut() else {
            return Ok(SourceStatus::Waiting);
        };
        let message = socket.next().await;
        match message {
            None => {
                self.save_cursor(true).await?;
                self.disconnected();
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
                let text = match message {
                    WsMessage::Text(t) => t,
                    WsMessage::Binary(b) => match String::from_utf8(b) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::debug!(error = %e, "non-UTF8 Jetstream frame ignored");
                            return Ok(SourceStatus::Waiting);
                        }
                    },
                };
                let wire = self.subscription.as_ref().map_or(Wire::V2, |s| s.wire);
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
                self.oversized = None;
                self.failures = 0;
                self.last_healthy_ms = Some(self.host.now_ms());
                let instance = self
                    .config
                    .endpoints
                    .get(self.endpoint)
                    .map(instance)
                    .unwrap_or_default();
                let out = self.filter.apply(wire, &instance, event, cx);
                self.events_since_save = self.events_since_save.saturating_add(1);
                let frame = PendingFrame {
                    actions: out.actions,
                    cursor: out.cursor,
                    restart_cursor: out.restart_cursor,
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
}
