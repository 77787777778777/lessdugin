// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`MemHost`]: an in-memory [`Host`] for offline tests.
//!
//! - `cache` and `state` are [`MemKv`] stores.
//! - HTTP goes to a programmable router: handlers are registered by (method, URL prefix) and
//!   return canned responses or run closures; the longest matching prefix wins (ties: the latest
//!   registration). Every request is recorded for assertions. An unrouted request gets a 404 and
//!   is marked unmatched. Response caps (`max_bytes`) are enforced like a real host.
//! - The clock is fake: [`MemHost::advance_ms`] moves it, and [`Host::sleep`] advances it by the
//!   requested time and returns immediately (sleeps are recorded).
//! - Randomness is deterministic (splitmix64 from a settable seed).
//! - System DNS answers, blobs and WebSocket scripts are programmable.
//!
//! `Target::UserPds` requests are routed under the pseudo-URL `userpds://<did><path_and_query>`.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{
    BlobHandle, Host, HostError, HttpRequest, HttpResponse, Redirects, Target, WebSocket, WsMessage,
};
use lessdugin_core::store::mem::MemKv;

use crate::resp;

/// A request handler.
pub type Handler = Arc<dyn Fn(&HttpRequest) -> Result<HttpResponse, HostError> + Send + Sync>;

/// The default fake time: 2025-10-09T08:53:20Z.
pub const DEFAULT_NOW_MS: i64 = 1_760_000_000_000;

struct Route {
    method: Option<http::Method>,
    prefix: String,
    handler: Handler,
    seq: u64,
}

/// A request as the host saw it.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    /// Method.
    pub method: http::Method,
    /// The URL (`userpds://<did><path>` for `Target::UserPds`).
    pub url: String,
    /// The target.
    pub target: Target,
    /// Headers.
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Option<Vec<u8>>,
    /// Response cap.
    pub max_bytes: u64,
    /// Timeout.
    pub timeout_ms: u32,
    /// Redirect policy.
    pub redirects: Redirects,
    /// Fake time when sent.
    pub at_ms: i64,
    /// Whether a route handled it.
    pub matched: bool,
}

impl RecordedRequest {
    /// The first header with this (lowercase) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

type TxtAnswer = Result<Option<Vec<String>>, HostError>;

struct Inner {
    cache: MemKv,
    state: MemKv,
    routes: Mutex<Vec<Route>>,
    seq: AtomicU64,
    requests: Mutex<Vec<RecordedRequest>>,
    now: AtomicI64,
    sleeps: Mutex<Vec<u64>>,
    rng: Mutex<u64>,
    txt: Mutex<BTreeMap<String, TxtAnswer>>,
    blobs: Mutex<BTreeMap<String, Vec<u8>>>,
    ws: Mutex<Vec<(String, Vec<WsMessage>)>>,
    ws_sent: Arc<Mutex<Vec<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// An in-memory host. Cloning shares everything.
#[derive(Clone)]
pub struct MemHost {
    inner: Arc<Inner>,
}

impl Default for MemHost {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MemHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemHost")
            .field("now_ms", &self.now())
            .field("requests", &lock(&self.inner.requests).len())
            .finish_non_exhaustive()
    }
}

/// The routing URL of a target.
pub fn target_url(target: &Target) -> String {
    match target {
        Target::Anon(u) => u.to_string(),
        Target::UserPds {
            account,
            path_and_query,
        } => format!("userpds://{account}{path_and_query}"),
    }
}

impl MemHost {
    /// A host with empty stores, no routes, the clock at [`DEFAULT_NOW_MS`] and seed 0.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                cache: MemKv::new(),
                state: MemKv::new(),
                routes: Mutex::new(Vec::new()),
                seq: AtomicU64::new(0),
                requests: Mutex::new(Vec::new()),
                now: AtomicI64::new(DEFAULT_NOW_MS),
                sleeps: Mutex::new(Vec::new()),
                rng: Mutex::new(0),
                txt: Mutex::new(BTreeMap::new()),
                blobs: Mutex::new(BTreeMap::new()),
                ws: Mutex::new(Vec::new()),
                ws_sent: Arc::new(Mutex::new(Vec::new())),
            }),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Routing

    /// Routes requests whose URL starts with `prefix` (and whose method matches, if given).
    pub fn route<F>(&self, method: Option<http::Method>, prefix: &str, f: F)
    where
        F: Fn(&HttpRequest) -> Result<HttpResponse, HostError> + Send + Sync + 'static,
    {
        let seq = self.inner.seq.fetch_add(1, Ordering::SeqCst);
        lock(&self.inner.routes).push(Route {
            method,
            prefix: prefix.to_owned(),
            handler: Arc::new(f),
            seq,
        });
    }

    /// Routes GETs under `prefix` to a closure.
    pub fn on_get<F>(&self, prefix: &str, f: F)
    where
        F: Fn(&HttpRequest) -> Result<HttpResponse, HostError> + Send + Sync + 'static,
    {
        self.route(Some(http::Method::GET), prefix, f);
    }

    /// Routes POSTs under `prefix` to a closure.
    pub fn on_post<F>(&self, prefix: &str, f: F)
    where
        F: Fn(&HttpRequest) -> Result<HttpResponse, HostError> + Send + Sync + 'static,
    {
        self.route(Some(http::Method::POST), prefix, f);
    }

    /// Answers GETs under `prefix` with a canned response.
    pub fn respond_get(&self, prefix: &str, response: HttpResponse) {
        self.on_get(prefix, move |_| Ok(response.clone()));
    }

    /// Answers requests (any method) under `prefix` with a canned response.
    pub fn respond(&self, prefix: &str, response: HttpResponse) {
        self.route(None, prefix, move |_| Ok(response.clone()));
    }

    /// Fails requests (any method) under `prefix` with a host error.
    pub fn fail(&self, prefix: &str, error: HostError) {
        self.route(None, prefix, move |_| Err(error.clone()));
    }

    /// Answers successive requests under `prefix` with `answers` in order, repeating the last.
    pub fn respond_seq(&self, prefix: &str, answers: Vec<Result<HttpResponse, HostError>>) {
        assert!(!answers.is_empty(), "respond_seq needs at least one answer");
        let n = Arc::new(AtomicU64::new(0));
        self.route(None, prefix, move |_| {
            let i = n.fetch_add(1, Ordering::SeqCst) as usize;
            answers[i.min(answers.len() - 1)].clone()
        });
    }

    /// Removes every route.
    pub fn clear_routes(&self) {
        lock(&self.inner.routes).clear();
    }

    fn dispatch(&self, req: &HttpRequest) -> Result<HttpResponse, HostError> {
        let url = target_url(&req.target);
        let handler = {
            let routes = lock(&self.inner.routes);
            routes
                .iter()
                .filter(|r| r.method.as_ref().is_none_or(|m| *m == req.method))
                .filter(|r| url.starts_with(&r.prefix))
                .max_by_key(|r| (r.prefix.len(), r.seq))
                .map(|r| Arc::clone(&r.handler))
        };
        let matched = handler.is_some();
        lock(&self.inner.requests).push(RecordedRequest {
            method: req.method.clone(),
            url: url.clone(),
            target: req.target.clone(),
            headers: req.headers.clone(),
            body: req.body.clone(),
            max_bytes: req.max_bytes,
            timeout_ms: req.timeout_ms,
            redirects: req.redirects,
            at_ms: self.now(),
            matched,
        });
        let resp = match handler {
            Some(h) => h(req)?,
            None => resp::xrpc_error(404, "NotFound", &format!("no route for {url}")),
        };
        if resp.body.len() as u64 > req.max_bytes {
            return Err(HostError::TooLarge);
        }
        Ok(resp)
    }

    // -----------------------------------------------------------------------------------------
    // Assertions

    /// Every request so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.inner.requests).clone()
    }

    /// Requests whose URL starts with `prefix`.
    pub fn requests_to(&self, prefix: &str) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.url.starts_with(prefix))
            .collect()
    }

    /// Requests no route handled.
    pub fn unmatched(&self) -> Vec<RecordedRequest> {
        self.requests().into_iter().filter(|r| !r.matched).collect()
    }

    /// Forgets recorded requests and sleeps.
    pub fn clear_requests(&self) {
        lock(&self.inner.requests).clear();
        lock(&self.inner.sleeps).clear();
    }

    /// Every `sleep` so far (ms).
    pub fn sleeps(&self) -> Vec<u64> {
        lock(&self.inner.sleeps).clone()
    }

    // -----------------------------------------------------------------------------------------
    // Clock and randomness

    /// The fake time (ms).
    pub fn now(&self) -> i64 {
        self.inner.now.load(Ordering::SeqCst)
    }

    /// Sets the fake time.
    pub fn set_now_ms(&self, ms: i64) {
        self.inner.now.store(ms, Ordering::SeqCst);
    }

    /// Moves the fake time forward.
    pub fn advance_ms(&self, ms: i64) {
        self.inner.now.fetch_add(ms, Ordering::SeqCst);
    }

    /// Reseeds the random generator.
    pub fn set_seed(&self, seed: u64) {
        *lock(&self.inner.rng) = seed;
    }

    // -----------------------------------------------------------------------------------------
    // Stores, DNS, blobs, sockets

    /// The cache store.
    pub fn cache_kv(&self) -> &MemKv {
        &self.inner.cache
    }

    /// The state store.
    pub fn state_kv(&self) -> &MemKv {
        &self.inner.state
    }

    /// Sets the system resolver's answer for a TXT name.
    pub fn set_system_txt(&self, name: &str, answer: Result<Option<Vec<String>>, HostError>) {
        lock(&self.inner.txt).insert(name.to_owned(), answer);
    }

    /// Stores blob bytes under a CID (as if fetched before).
    pub fn put_blob(&self, cid: &str, bytes: Vec<u8>) {
        lock(&self.inner.blobs).insert(cid.to_owned(), bytes);
    }

    /// The stored bytes of a blob.
    pub fn blob(&self, cid: &str) -> Option<Vec<u8>> {
        lock(&self.inner.blobs).get(cid).cloned()
    }

    /// Scripts the messages of WebSockets opened at URLs starting with `prefix`.
    pub fn script_websocket(&self, prefix: &str, messages: Vec<WsMessage>) {
        lock(&self.inner.ws).push((prefix.to_owned(), messages));
    }

    /// Text frames sent on scripted WebSockets.
    pub fn websocket_sent(&self) -> Vec<String> {
        lock(&self.inner.ws_sent).clone()
    }
}

/// A scripted WebSocket.
pub struct MemWs {
    queue: VecDeque<WsMessage>,
    max: u32,
    sent: Arc<Mutex<Vec<String>>>,
}

impl WebSocket for MemWs {
    async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
        let msg = self.queue.pop_front()?;
        let len = match &msg {
            WsMessage::Text(t) => t.len(),
            WsMessage::Binary(b) => b.len(),
        };
        if len > self.max as usize {
            self.queue.clear();
            return Some(Err(HostError::TooLarge));
        }
        Some(Ok(msg))
    }

    async fn send_text(&mut self, text: String) -> Result<(), HostError> {
        lock(&self.sent).push(text);
        Ok(())
    }

    async fn close(&mut self) {
        self.queue.clear();
    }
}

impl Host for MemHost {
    type Kv = MemKv;
    type Ws = MemWs;

    fn cache(&self) -> &MemKv {
        &self.inner.cache
    }

    fn state(&self) -> &MemKv {
        &self.inner.state
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        self.dispatch(&req)
    }

    async fn fetch_blob(
        &self,
        pds: url::Url,
        did: Did,
        cid: String,
        max_bytes: u64,
    ) -> Result<BlobHandle, HostError> {
        if let Some(bytes) = self.blob(&cid) {
            return Ok(BlobHandle {
                cid: cid.clone(),
                size: bytes.len() as u64,
                locator: format!("mem:{cid}"),
            });
        }
        let parsed = lessdugin_atp::cid::parse(&cid)
            .ok()
            .filter(|c| {
                c.codec() == lessdugin_atp::cid::RAW
                    && c.hash().code() == lessdugin_atp::cid::SHA2_256
            })
            .ok_or_else(|| HostError::Unsupported(format!("not a raw sha2-256 CID: {cid}")))?;
        let qs = lessdugin_atp::xrpc::query_string(&[("did", did.as_str()), ("cid", cid.as_str())]);
        let url = format!(
            "{}/xrpc/com.atproto.sync.getBlob?{qs}",
            pds.as_str().trim_end_matches('/')
        );
        let url = url::Url::parse(&url).map_err(|e| HostError::Other(e.to_string()))?;
        let req = HttpRequest::get(Target::Anon(url)).with_max_bytes(max_bytes);
        let resp = self.dispatch(&req)?;
        if !(200..300).contains(&resp.status) {
            let name = serde_json::from_slice::<serde_json::Value>(&resp.body)
                .ok()
                .and_then(|v| v.get("error")?.as_str().map(str::to_owned));
            return Err(HostError::HttpStatus(resp.status, name));
        }
        lessdugin_atp::cid::verify(&parsed, &resp.body).map_err(|_| HostError::CidMismatch)?;
        let size = resp.body.len() as u64;
        self.put_blob(&cid, resp.body);
        Ok(BlobHandle {
            cid: cid.clone(),
            size,
            locator: format!("mem:{cid}"),
        })
    }

    async fn websocket(&self, url: url::Url, max_message_bytes: u32) -> Result<MemWs, HostError> {
        let s = url.to_string();
        let script = lock(&self.inner.ws)
            .iter()
            .rev()
            .find(|(p, _)| s.starts_with(p.as_str()))
            .map(|(_, m)| m.clone())
            .ok_or_else(|| HostError::Network(format!("no scripted WebSocket for {s}")))?;
        Ok(MemWs {
            queue: script.into(),
            max: max_message_bytes,
            sent: Arc::clone(&self.inner.ws_sent),
        })
    }

    async fn resolve_txt_system(&self, name: String) -> Result<Option<Vec<String>>, HostError> {
        lock(&self.inner.txt)
            .get(&name)
            .cloned()
            .unwrap_or_else(|| Err(HostError::Unsupported("system DNS not scripted".into())))
    }

    async fn sleep(&self, ms: u64) {
        lock(&self.inner.sleeps).push(ms);
        self.advance_ms(i64::try_from(ms).unwrap_or(i64::MAX));
    }

    fn now_ms(&self) -> i64 {
        self.now()
    }

    fn fill_random(&self, buf: &mut [u8]) {
        let mut state = lock(&self.inner.rng);
        for chunk in buf.chunks_mut(8) {
            // splitmix64
            *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
    }
}
