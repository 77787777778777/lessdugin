// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`FakeNet`]: an in-memory atproto network — several PDS hosts with real repositories, a PLC
//! directory, relays and DNS-over-HTTPS — with fault injection, served on [`MemHost`]s (as
//! routes) and, with the `http` feature, over real HTTP on `127.0.0.1` for CLI tests.
//!
//! ```text
//! let host = MemHost::new();
//! let net  = FakeNet::on(&host);                      // clock = host's fake clock; routes installed
//! let alice = net.add_account("alice.example.com");   // did:plc + k256 key + DID doc + empty repo
//! alice.put("online.7778777.lessdugin.post", "aaaa-3l2s5xxv2zaa2", &json!({...}));
//! // the engine on `host` now resolves alice (PLC + DoH) and reads/writes her repo
//! ```
//!
//! **Repositories** are real: records are DAG-CBOR blocks, the MST is `lessdugin_atp::mst::build`,
//! commits are signed v3 commits (per-account k256 key, whose Multikey is in the DID document),
//! and revs are TIDs from the clock (strictly increasing per account). Helper writes
//! ([`AccountHandle::put`], [`AccountHandle::batch`]) are committed lazily: the next request
//! about the repo (or [`AccountHandle::head`]) signs one commit for everything pending. XRPC
//! writes commit immediately.
//!
//! **Endpoints** (`<pds>/xrpc/…`): `com.atproto.repo.{getRecord, listRecords, describeRepo,
//! createRecord, putRecord, deleteRecord, applyWrites, uploadBlob}`, `com.atproto.sync.
//! {getLatestCommit, getRecord, getRepo, getRepoStatus, getBlob, listBlobs}`,
//! `com.atproto.identity.resolveHandle` (any handle on the network), `com.atproto.server.
//! {describeServer, createSession, refreshSession, getSession, deleteSession}`, `_health` and
//! `/.well-known/oauth-protected-resource`. Relays answer
//! `com.atproto.sync.listReposByCollection`; the PLC directory serves `/<did>`; DoH endpoints
//! answer `_atproto.<handle>` TXT queries. Behaviour follows the reference PDS as measured in
//! ADR 0004 (listing) and ADR 0005 (writes, blobs):
//!
//! - `listRecords`: descending by rkey by default (`reverse=true` ascending), `limit` 1–100
//!   (default 50, otherwise `400`), the cursor is an exclusive rkey bound in both directions
//!   (any string; empty = none), a non-empty page always carries `cursor` = its last key (even
//!   when short), an empty page none. [`CursorMode`] switches to opaque or broken cursors.
//! - `createRecord` on an existing key, and `applyWrites` create-existing / update-missing /
//!   delete-missing: `500 InternalServerError`, nothing written. `applyWrites` takes ≤ 200 ops
//!   and is atomic; later ops see earlier ones.
//! - `putRecord`: `swapRecord: null` means create-only (`400 InvalidSwap "Record was at <cid>"`
//!   when present), a CID must match (`"Record was at null"` when absent); swaps are checked
//!   before the no-op test; an identical value makes no commit and the answer has no `commit`.
//! - `deleteRecord` of an absent key: `200 {}` (no swap checked). `swapCommit` mismatches:
//!   `400 InvalidSwap "Commit was at <cid>"`.
//! - Records: `$type` is injected when missing, a mismatch is a `400`, floats are a `400`,
//!   `validate: true` is a `400` (no lexicons are known), `validate: false` drops
//!   `validationStatus`; blob refs must match a stored blob (`BlobNotFound`, `InvalidMimeType`,
//!   `InvalidSize`).
//! - Blobs: `uploadBlob` sniffs PNG/JPEG/GIF/WebP (other types keep the declared type without
//!   parameters) and stores a temporary blob. `getBlob` serves only blobs a record references
//!   (else `400 InvalidRequest "Blob not found"`); a blob whose last reference goes is collected
//!   at once; temporary blobs wait for [`FakeNet::gc_blobs`].
//! - Auth: write endpoints, `uploadBlob` and `getSession` need `Authorization: Bearer <token>`
//!   from `createSession` (app passwords or the main password) at the same PDS, or a
//!   `userpds://` request (the host authenticates; see [`MemHost`]). Errors: `401 AuthMissing`,
//!   `400 InvalidToken`, `400 ExpiredToken`; the body's `repo` must be the session's DID.
//! - Inactive repos: reads answer `400 RepoDeactivated | RepoTakendown | RepoSuspended`, deleted
//!   or unknown ones `400 RepoNotFound`; `getRepoStatus` reports `{active: false, status}`.
//!
//! **Faults** ([`FaultRule`]: per host, repo, method and caller; counted or permanent): latency,
//! fixed statuses and XRPC errors, 429 with `Retry-After`, truncated bodies, dropped connections,
//! canned responses. Per host: [`CursorMode`], a page-size cap, a `ratelimit-*` budget
//! ([`RateLimit`]). Per repo: account status, PDS moves, key rotation, DID-document overrides,
//! equivocation ([`AccountHandle::equivocate`]: some callers see a different signed commit).
//!
//! **Logs**: every request ([`FakeNet::requests`]) and a firehose-like event log of commits,
//! identity and account changes ([`FakeNet::events`]).
//!
//! **Callers**: [`FakeNet::install_as`] labels the requests of one `MemHost` (one simulated
//! device); over HTTP the `x-fake-caller` header sets the label (default `http`).

mod auth;
mod blob;
mod faults;
mod handles;
#[cfg(feature = "http")]
mod http_server;
mod log;
mod repo;
mod server;
mod state;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{HostError, HttpRequest, HttpResponse};

pub use blob::{sniff_mime, tiny_png};
pub use faults::{CursorMode, Fault, FaultId, FaultRule, ForkSelector, RateLimit};
pub use handles::{AccountHandle, PdsHandle, RepoWriter};
pub use log::{LoggedRequest, RepoEvent, RepoEventKind, ServiceKind};
pub use repo::{OpAction, RepoOp};
pub use server::APPLY_WRITES_MAX;
pub use state::AccountStatus;

use crate::memhost::{MemHost, target_url};
use faults::ActiveFault;
use server::{Abort, Incoming, Outgoing};
use state::{NetState, PdsState};

/// The PDS [`FakeNet::add_account`] uses when none was added.
pub const DEFAULT_PDS: &str = "https://pds.example.com";

/// Time for the fake network (commit revs, token expiry, rate-limit windows, the logs).
pub trait Clock: Send + Sync + 'static {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
    /// Lets simulated latency pass (fake clocks advance; real clocks ignore it).
    fn advance_ms(&self, _ms: u64) {}
}

impl Clock for MemHost {
    fn now_ms(&self) -> i64 {
        self.now()
    }
    fn advance_ms(&self, ms: u64) {
        MemHost::advance_ms(self, i64::try_from(ms).unwrap_or(i64::MAX));
    }
}

/// A clock moved by hand.
#[derive(Debug)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// A clock at `ms`.
    pub fn new(ms: i64) -> Self {
        Self(AtomicI64::new(ms))
    }
    /// Sets the time.
    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn advance_ms(&self, ms: u64) {
        self.0
            .fetch_add(i64::try_from(ms).unwrap_or(i64::MAX), Ordering::SeqCst);
    }
}

impl Clock for Arc<ManualClock> {
    fn now_ms(&self) -> i64 {
        self.as_ref().now_ms()
    }
    fn advance_ms(&self, ms: u64) {
        self.as_ref().advance_ms(ms);
    }
}

/// The wall clock (for HTTP serving to real clients).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)] // testkit is native-only; real clients need real time
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

/// Network-wide endpoints. In HTTP mode they are replaced by local listeners (and DoH is off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetConfig {
    /// PLC directory base URL.
    pub plc: String,
    /// Relay base URLs (each answers `listReposByCollection` for the whole network).
    pub relays: Vec<String>,
    /// DoH JSON endpoints answering `_atproto.<handle>` TXT queries.
    pub doh: Vec<String>,
}

impl Default for NetConfig {
    /// The engine's production defaults: `plc.directory`, the two Bluesky relays, Cloudflare and
    /// Google DoH.
    fn default() -> Self {
        Self {
            plc: crate::identity::PLC.to_owned(),
            relays: vec![
                "https://bsky.network".to_owned(),
                "https://relay1.us-east.bsky.network".to_owned(),
            ],
            doh: vec![
                crate::identity::DOH.to_owned(),
                crate::identity::DOH2.to_owned(),
            ],
        }
    }
}

/// Per-PDS behaviour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdsConfig {
    /// `listRecords` cursor behaviour.
    pub cursor_mode: CursorMode,
    /// Caps every `listRecords` page (a server with a smaller page size than asked).
    pub max_page: Option<u32>,
    /// `uploadBlob` size limit (bytes; larger: `413 PayloadTooLarge`). Default 5 MiB, the
    /// reference PDS default.
    pub blob_upload_limit: u64,
    /// JSON request body limit for procedures (bytes; larger: `413 PayloadTooLarge`).
    pub json_body_limit: usize,
    /// Access token lifetime (ms).
    pub access_ttl_ms: i64,
    /// Refresh token lifetime (ms).
    pub refresh_ttl_ms: i64,
    /// `authorization_servers` of the protected-resource metadata (default: the PDS itself).
    pub authorization_servers: Option<Vec<String>>,
    /// `describeServer.availableUserDomains`.
    pub available_user_domains: Vec<String>,
    /// A read budget with `ratelimit-*` headers.
    pub rate_limit: Option<RateLimit>,
}

impl Default for PdsConfig {
    fn default() -> Self {
        Self {
            cursor_mode: CursorMode::Bound,
            max_page: None,
            blob_upload_limit: 5 * 1024 * 1024,
            json_body_limit: 1024 * 1024,
            access_ttl_ms: 2 * 60 * 60 * 1000,
            refresh_ttl_ms: 90 * 24 * 60 * 60 * 1000,
            authorization_servers: None,
            available_user_domains: vec![".test".to_owned()],
            rate_limit: None,
        }
    }
}

struct Attached {
    host: MemHost,
    caller: String,
}

struct Inner {
    clock: Box<dyn Clock>,
    state: Mutex<NetState>,
    attached: Mutex<Vec<Attached>>,
    #[cfg(feature = "http")]
    http: Option<http_server::Server>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn serve(&self, inc: &Incoming<'_>) -> Outgoing {
        let now = self.clock.now_ms();
        lock(&self.state).dispatch(inc, now)
    }
}

/// Applies latency and aborts the way an in-memory host would.
fn finish_in_memory(
    out: Outgoing,
    timeout_ms: u32,
    advance: impl Fn(u64),
) -> Result<HttpResponse, HostError> {
    if out.delay_ms > 0 {
        if out.delay_ms > u64::from(timeout_ms) {
            advance(u64::from(timeout_ms));
            return Err(HostError::Timeout);
        }
        advance(out.delay_ms);
    }
    match out.abort {
        None => Ok(out.resp),
        Some(Abort::Truncate(n)) => {
            let mut r = out.resp;
            r.body.truncate(n);
            Ok(r)
        }
        Some(Abort::Disconnect) => Err(HostError::Network(
            "connection reset (fake network fault)".to_owned(),
        )),
        Some(Abort::Host(e)) => Err(e),
    }
}

/// An in-memory atproto network. Cloning shares it.
#[derive(Clone)]
pub struct FakeNet {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FakeNet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let st = self.state();
        f.debug_struct("FakeNet")
            .field("pds", &st.pds_order)
            .field("accounts", &st.accounts.len())
            .field("requests", &st.log.len())
            .finish_non_exhaustive()
    }
}

impl FakeNet {
    /// A network with the default endpoints ([`NetConfig::default`]) and no PDS yet.
    pub fn new(clock: impl Clock) -> Self {
        Self::with_config(clock, NetConfig::default())
    }

    /// A network with these endpoints.
    pub fn with_config(clock: impl Clock, config: NetConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                clock: Box::new(clock),
                state: Mutex::new(NetState::new(normalize_config(config))),
                attached: Mutex::new(Vec::new()),
                #[cfg(feature = "http")]
                http: None,
            }),
        }
    }

    /// A network on `host`'s fake clock, installed on `host` (caller label `default`).
    pub fn on(host: &MemHost) -> Self {
        let net = Self::new(host.clone());
        net.install(host);
        net
    }

    /// Serves the network over HTTP on `127.0.0.1` (one listener per PDS, one for the PLC
    /// directory, one per relay; no DoH), on its own Tokio runtime. Base URLs are
    /// `http://127.0.0.1:<port>`; use them with the engine's devnet mode.
    #[cfg(feature = "http")]
    pub fn serve_http(clock: impl Clock) -> Self {
        Self::serve_http_with_relays(clock, 1)
    }

    /// [`FakeNet::serve_http`] with `relays` relay listeners.
    #[cfg(feature = "http")]
    pub fn serve_http_with_relays(clock: impl Clock, relays: usize) -> Self {
        let server = http_server::Server::new();
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let plc = server.bind(weak.clone());
            let relays = (0..relays).map(|_| server.bind(weak.clone())).collect();
            Inner {
                clock: Box::new(clock),
                state: Mutex::new(NetState::new(NetConfig {
                    plc,
                    relays,
                    doh: Vec::new(),
                })),
                attached: Mutex::new(Vec::new()),
                http: Some(server),
            }
        });
        Self { inner }
    }

    fn state(&self) -> MutexGuard<'_, NetState> {
        lock(&self.inner.state)
    }

    fn now(&self) -> i64 {
        self.inner.clock.now_ms()
    }

    /// The fake time (ms).
    pub fn now_ms(&self) -> i64 {
        self.now()
    }

    /// The endpoints.
    pub fn config(&self) -> NetConfig {
        self.state().config.clone()
    }

    /// The PLC directory base URL.
    pub fn plc(&self) -> String {
        self.state().config.plc.clone()
    }

    /// The relay base URLs.
    pub fn relays(&self) -> Vec<String> {
        self.state().config.relays.clone()
    }

    // -----------------------------------------------------------------------------------------
    // Serving

    /// Routes the network's requests on `host` (caller label `default`): every PDS (also those
    /// added later), the PLC directory, relays, DoH endpoints and `userpds://` requests (which
    /// count as authenticated as their account). Routes registered later on `host` with a
    /// longer prefix win over these.
    pub fn install(&self, host: &MemHost) {
        self.install_as(host, "default");
    }

    /// [`FakeNet::install`] with a caller label (for per-caller faults and equivocation).
    pub fn install_as(&self, host: &MemHost, caller: &str) {
        let idx = {
            let mut att = lock(&self.inner.attached);
            att.push(Attached {
                host: host.clone(),
                caller: caller.to_owned(),
            });
            att.len() - 1
        };
        let (pds, config) = {
            let st = self.state();
            (st.pds_order.clone(), st.config.clone())
        };
        let route = |prefix: &str, method: Option<http::Method>| {
            host.route(method, prefix, self.mem_handler(idx, caller));
        };
        route(&format!("{}/", config.plc), None);
        for r in &config.relays {
            route(&format!("{r}/"), None);
        }
        for d in &config.doh {
            route(d, Some(http::Method::GET));
        }
        route("userpds://", None);
        for base in pds {
            route(&format!("{base}/"), None);
        }
    }

    fn mem_handler(
        &self,
        idx: usize,
        caller: &str,
    ) -> impl Fn(&HttpRequest) -> Result<HttpResponse, HostError> + Send + Sync + 'static {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let caller = caller.to_owned();
        move |req: &HttpRequest| {
            let inner = weak
                .upgrade()
                .ok_or_else(|| HostError::Network("the fake network was dropped".to_owned()))?;
            let url = target_url(&req.target);
            let out = inner.serve(&Incoming {
                method: &req.method,
                url: &url,
                headers: &req.headers,
                body: req.body.as_deref(),
                caller: &caller,
            });
            let host = lock(&inner.attached).get(idx).map(|a| a.host.clone());
            finish_in_memory(out, req.timeout_ms, |ms| {
                if let Some(h) = &host {
                    Clock::advance_ms(h, ms);
                }
            })
        }
    }

    /// Answers one request directly (no `MemHost`): latency advances the network's clock.
    pub fn request(&self, req: &HttpRequest, caller: &str) -> Result<HttpResponse, HostError> {
        let url = target_url(&req.target);
        let out = self.inner.serve(&Incoming {
            method: &req.method,
            url: &url,
            headers: &req.headers,
            body: req.body.as_deref(),
            caller,
        });
        finish_in_memory(out, req.timeout_ms, |ms| self.inner.clock.advance_ms(ms))
    }

    // -----------------------------------------------------------------------------------------
    // Hosts and accounts

    /// Adds a PDS. `name` is a base URL (`https://pds2.example.com`) or a host name
    /// (`pds2.example.com` → `https://pds2.example.com`); over HTTP it is only a label and the
    /// base is a fresh `http://127.0.0.1:<port>`.
    pub fn add_pds(&self, name: &str) -> PdsHandle {
        self.add_pds_with(name, PdsConfig::default())
    }

    /// [`FakeNet::add_pds`] with a configuration.
    pub fn add_pds_with(&self, name: &str, cfg: PdsConfig) -> PdsHandle {
        let base = self.allocate_base(name);
        let host_name = base
            .split("://")
            .nth(1)
            .unwrap_or(&base)
            .replace(':', "%3A");
        {
            let now = self.now();
            let mut st = self.state();
            assert!(!st.pds.contains_key(&base), "PDS {base} exists");
            let rate = cfg.rate_limit.map(|r| faults::RateState::new(r, now));
            st.pds.insert(
                base.clone(),
                PdsState {
                    label: name.to_owned(),
                    server_did: format!("did:web:{host_name}"),
                    cfg,
                    tokens: BTreeMap::new(),
                    cursors: BTreeMap::new(),
                    rate,
                },
            );
            st.pds_order.push(base.clone());
        }
        let attached: Vec<(MemHost, String)> = lock(&self.inner.attached)
            .iter()
            .map(|a| (a.host.clone(), a.caller.clone()))
            .collect();
        for (idx, (host, caller)) in attached.into_iter().enumerate() {
            host.route(None, &format!("{base}/"), self.mem_handler(idx, &caller));
        }
        PdsHandle {
            net: self.clone(),
            base,
        }
    }

    fn allocate_base(&self, name: &str) -> String {
        #[cfg(feature = "http")]
        if let Some(server) = &self.inner.http {
            return server.bind(Arc::downgrade(&self.inner));
        }
        let name = name.trim_end_matches('/');
        if name.contains("://") {
            name.to_owned()
        } else {
            format!("https://{name}")
        }
    }

    /// A PDS by base URL.
    pub fn pds(&self, base: &str) -> Option<PdsHandle> {
        let base = base.trim_end_matches('/');
        self.state().pds.contains_key(base).then(|| PdsHandle {
            net: self.clone(),
            base: base.to_owned(),
        })
    }

    /// Every PDS, in the order added.
    pub fn pds_hosts(&self) -> Vec<PdsHandle> {
        let order = self.state().pds_order.clone();
        order
            .into_iter()
            .map(|base| PdsHandle {
                net: self.clone(),
                base,
            })
            .collect()
    }

    /// The first PDS added ([`DEFAULT_PDS`] is added if there is none).
    pub fn default_pds(&self) -> PdsHandle {
        let first = self.state().pds_order.first().cloned();
        match first {
            Some(base) => PdsHandle {
                net: self.clone(),
                base,
            },
            None => self.add_pds(DEFAULT_PDS),
        }
    }

    /// Adds an account on the default PDS (see [`PdsHandle::add_account`]).
    pub fn add_account(&self, handle: &str) -> AccountHandle {
        self.default_pds().add_account(handle)
    }

    /// An account by DID or handle (also deleted ones, by DID).
    pub fn account(&self, ident: &str) -> Option<AccountHandle> {
        let st = self.state();
        let did = if ident.starts_with("did:") {
            st.accounts.get(ident).map(|a| a.did.clone())
        } else {
            st.resolve_ident(ident)
                .and_then(|d| st.accounts.get(&d).map(|a| a.did.clone()))
        }?;
        Some(AccountHandle {
            net: self.clone(),
            did,
        })
    }

    /// Every account, by DID.
    pub fn accounts(&self) -> Vec<AccountHandle> {
        let dids: Vec<Did> = self
            .state()
            .accounts
            .values()
            .map(|a| a.did.clone())
            .collect();
        dids.into_iter()
            .map(|did| AccountHandle {
                net: self.clone(),
                did,
            })
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // Faults, logs, blobs

    /// Injects a fault.
    pub fn inject(&self, rule: FaultRule) -> FaultId {
        let mut st = self.state();
        let id = FaultId(st.next());
        st.faults.push(ActiveFault {
            id,
            rule,
            seen: 0,
            applied: 0,
        });
        id
    }

    /// Removes a fault; true if it was active.
    pub fn remove_fault(&self, id: FaultId) -> bool {
        let mut st = self.state();
        let before = st.faults.len();
        st.faults.retain(|f| f.id != id);
        st.faults.len() != before
    }

    /// Removes every fault.
    pub fn clear_faults(&self) {
        self.state().faults.clear();
    }

    /// How many times a fault was applied (`None` once it expired or was removed).
    pub fn fault_hits(&self, id: FaultId) -> Option<u32> {
        self.state()
            .faults
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.applied)
    }

    /// Every request so far.
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.state().log.clone()
    }

    /// Requests for an XRPC method (or `plc`, `doh`).
    pub fn requests_for(&self, nsid: &str) -> Vec<LoggedRequest> {
        self.state()
            .log
            .iter()
            .filter(|r| r.nsid.as_deref() == Some(nsid))
            .cloned()
            .collect()
    }

    /// The number of requests for an XRPC method (or `plc`, `doh`).
    pub fn count(&self, nsid: &str) -> usize {
        self.state()
            .log
            .iter()
            .filter(|r| r.nsid.as_deref() == Some(nsid))
            .count()
    }

    /// Forgets the request log.
    pub fn clear_requests(&self) {
        self.state().log.clear();
    }

    /// Every repo event so far.
    pub fn events(&self) -> Vec<RepoEvent> {
        self.state().events.clone()
    }

    /// Repo events with `seq > after`.
    pub fn events_since(&self, after: u64) -> Vec<RepoEvent> {
        self.state()
            .events
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect()
    }

    /// Commits every account's pending helper writes (emitting their events).
    pub fn flush(&self) {
        let now = self.now();
        let mut st = self.state();
        let dids: Vec<String> = st.accounts.keys().cloned().collect();
        for d in dids {
            st.ensure_commit(&d, now);
        }
    }

    /// Deletes every temporary (never referenced) blob; returns how many.
    pub fn gc_blobs(&self) -> usize {
        let mut st = self.state();
        let mut n = 0;
        for a in st.accounts.values_mut() {
            let before = a.blobs.len();
            a.blobs.retain(|_, b| b.permanent);
            n += before - a.blobs.len();
        }
        n
    }
}

fn normalize_config(mut c: NetConfig) -> NetConfig {
    c.plc = c.plc.trim_end_matches('/').to_owned();
    for r in &mut c.relays {
        *r = r.trim_end_matches('/').to_owned();
    }
    c
}
