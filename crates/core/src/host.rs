// SPDX-License-Identifier: AGPL-3.0-or-later
//! The [`Host`] and [`KvStore`] traits: the only way the core touches the network, storage,
//! time or randomness (`docs/spec/sync.md`, "Host abstraction").
//!
//! The core never spawns tasks, reads clocks or sleeps by itself. Long-lived futures returned by
//! the engine (live tail, outbox pump, background verification) are handed to the host, which
//! runs them with `tokio::spawn` natively or `spawn_local` in the browser.
//!
//! Futures are `Send` on native targets and not `Send` in wasm, expressed with [`MaybeSend`].

use std::future::Future;
use std::ops::Bound;

use lessdugin_atp::syntax::Did;

/// `Send` on native targets; no bound on wasm32.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Send` on native targets; no bound on wasm32.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

/// `Send + Sync` on native targets; no bound on wasm32.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}

/// `Send + Sync` on native targets; no bound on wasm32.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

/// Where an HTTP request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An anonymous request to an absolute URL (any PDS, PLC, DoH, relay, indexer…).
    /// Hosts never attach credentials to these, and never follow redirects to private networks.
    Anon(url::Url),
    /// An authenticated request to the PDS bound to `account` at login. The host adds
    /// authentication (DPoP / bearer) and never follows redirects.
    UserPds {
        /// The logged-in account.
        account: Did,
        /// Path and query, e.g. `/xrpc/com.atproto.repo.putRecord`.
        path_and_query: String,
    },
}

/// An HTTP request.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    /// Method (GET or POST in practice).
    pub method: http::Method,
    /// Destination.
    pub target: Target,
    /// Extra headers. Browser hosts must not add custom headers to anonymous cross-origin GETs
    /// (to avoid CORS preflights); the core only sets `content-type` on bodies.
    pub headers: Vec<(String, String)>,
    /// Request body.
    pub body: Option<Vec<u8>>,
    /// Hard cap on the response body, counted while streaming *after* decompression.
    /// Exceeding it fails the request with [`HostError::TooLarge`].
    pub max_bytes: u64,
    /// Total timeout in milliseconds.
    pub timeout_ms: u32,
}

impl HttpRequest {
    /// A GET with default caps (4 MB, 20 s).
    pub fn get(target: Target) -> Self {
        Self {
            method: http::Method::GET,
            target,
            headers: Vec::new(),
            body: None,
            max_bytes: crate::limits::DEFAULT_RESPONSE_MAX_BYTES,
            timeout_ms: crate::limits::REQUEST_TIMEOUT_MS,
        }
    }

    /// A POST with a JSON body and default caps.
    pub fn post_json(target: Target, body: Vec<u8>) -> Self {
        Self {
            method: http::Method::POST,
            target,
            headers: vec![("content-type".into(), "application/json".into())],
            body: Some(body),
            max_bytes: crate::limits::DEFAULT_RESPONSE_MAX_BYTES,
            timeout_ms: crate::limits::REQUEST_TIMEOUT_MS,
        }
    }
}

/// An HTTP response (the body is fully buffered and already capped).
#[derive(Clone, Debug)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Response headers (names lowercased).
    pub headers: Vec<(String, String)>,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// The first header with this (lowercase) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Errors a host can report.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// Connection, DNS or TLS failure.
    #[error("network error: {0}")]
    Network(String),
    /// The request did not finish in time.
    #[error("timed out")]
    Timeout,
    /// The response exceeded `max_bytes`.
    #[error("response too large")]
    TooLarge,
    /// The request was refused by the host's policy (SSRF guard, forbidden scheme, port…).
    #[error("blocked by policy: {0}")]
    Blocked(String),
    /// A browser refused a cross-origin response (CORS).
    #[error("cross-origin request refused")]
    Cors,
    /// No usable session for this account (login required).
    #[error("authentication required for {0}")]
    AuthRequired(String),
    /// Storage failure.
    #[error("storage error: {0}")]
    Storage(String),
    /// The operation is not supported by this host (e.g. system DNS in a browser).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

/// A WebSocket message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    /// A text frame.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
}

/// An open WebSocket.
pub trait WebSocket: MaybeSend {
    /// The next message; `None` when the socket closed.
    fn next(&mut self) -> impl Future<Output = Option<Result<WsMessage, HostError>>> + MaybeSend;
    /// Send a text frame.
    fn send_text(&mut self, text: String) -> impl Future<Output = Result<(), HostError>> + MaybeSend;
    /// Close the socket.
    fn close(&mut self) -> impl Future<Output = ()> + MaybeSend;
}

/// A verified blob held by the host (a file natively, a `Blob`/object URL in the browser).
/// The core only ever sees this handle, never the bytes of large media (`sync.md`, blobs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobHandle {
    /// The blob's CID (raw codec), already verified by the host while streaming.
    pub cid: String,
    /// Size in bytes.
    pub size: u64,
    /// Host-specific locator (file path, object URL, cache key).
    pub locator: String,
}

/// Directions for range scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Ascending key order.
    Forward,
    /// Descending key order.
    Reverse,
}

/// A key range for scans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRange {
    /// Lower bound.
    pub start: Bound<Vec<u8>>,
    /// Upper bound.
    pub end: Bound<Vec<u8>>,
}

impl KeyRange {
    /// All keys starting with `prefix`.
    pub fn prefix(prefix: &[u8]) -> Self {
        let start = Bound::Included(prefix.to_vec());
        let end = match prefix_successor(prefix) {
            Some(succ) => Bound::Excluded(succ),
            None => Bound::Unbounded,
        };
        Self { start, end }
    }

    /// True if `key` is inside the range.
    pub fn contains(&self, key: &[u8]) -> bool {
        let lower_ok = match &self.start {
            Bound::Included(s) => key >= s.as_slice(),
            Bound::Excluded(s) => key > s.as_slice(),
            Bound::Unbounded => true,
        };
        let upper_ok = match &self.end {
            Bound::Included(e) => key <= e.as_slice(),
            Bound::Excluded(e) => key < e.as_slice(),
            Bound::Unbounded => true,
        };
        lower_ok && upper_ok
    }
}

/// The smallest byte string greater than every string with this prefix, if any.
pub fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut v = prefix.to_vec();
    while let Some(last) = v.pop() {
        if last < 0xff {
            v.push(last + 1);
            return Some(v);
        }
    }
    None
}

/// One operation of an atomic write batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KvOp {
    /// Insert or replace.
    Put(Vec<u8>, Vec<u8>),
    /// Remove one key.
    Delete(Vec<u8>),
    /// Remove every key in a range.
    DeleteRange(KeyRange),
}

/// An ordered key-value store with atomic batches. Compatible with IndexedDB: there are no
/// interactive transactions, so every read-modify-write in the core is idempotent and
/// rev-monotonic (`sync.md`, store invariants).
pub trait KvStore: MaybeSendSync {
    /// Read several keys at once; the result has one entry per key, in order.
    fn get_many(
        &self,
        keys: Vec<Vec<u8>>,
    ) -> impl Future<Output = Result<Vec<Option<Vec<u8>>>, HostError>> + MaybeSend;

    /// Scan a key range in the given direction, returning at most `limit` pairs.
    fn scan(
        &self,
        range: KeyRange,
        dir: Dir,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(Vec<u8>, Vec<u8>)>, HostError>> + MaybeSend;

    /// Apply a batch atomically.
    fn write(&self, batch: Vec<KvOp>) -> impl Future<Output = Result<(), HostError>> + MaybeSend;
}

/// The platform services the engine needs.
pub trait Host: MaybeSendSync + 'static {
    /// The key-value store type.
    type Kv: KvStore;
    /// The WebSocket type.
    type Ws: WebSocket;

    /// The disposable cache store (`sync.md`, invariant I1).
    fn cache(&self) -> &Self::Kv;
    /// The durable state store (outbox, drafts, read marks, settings).
    fn state(&self) -> &Self::Kv;

    /// Perform an HTTP request.
    fn http(
        &self,
        req: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HostError>> + MaybeSend;

    /// Fetch `com.atproto.sync.getBlob` for (`did`, `cid`) from `pds`, streaming it while
    /// hashing, and fail unless sha256 matches the CID. Blob bytes never pass through wasm memory.
    fn fetch_blob(
        &self,
        pds: url::Url,
        did: Did,
        cid: String,
        max_bytes: u64,
    ) -> impl Future<Output = Result<BlobHandle, HostError>> + MaybeSend;

    /// Open a WebSocket (Jetstream). Messages larger than `max_message_bytes` fail the stream.
    fn websocket(
        &self,
        url: url::Url,
        max_message_bytes: u32,
    ) -> impl Future<Output = Result<Self::Ws, HostError>> + MaybeSend;

    /// Resolve TXT records with the system resolver. Browser hosts return
    /// `Err(HostError::Unsupported)`; the core uses DNS-over-HTTPS by default everywhere.
    fn resolve_txt_system(
        &self,
        name: String,
    ) -> impl Future<Output = Result<Option<Vec<String>>, HostError>> + MaybeSend;

    /// Sleep for `ms` milliseconds.
    fn sleep(&self, ms: u64) -> impl Future<Output = ()> + MaybeSend;

    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;

    /// Fill `buf` with cryptographically secure random bytes.
    fn fill_random(&self, buf: &mut [u8]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ranges() {
        let r = KeyRange::prefix(b"ab");
        assert!(r.contains(b"ab"));
        assert!(r.contains(b"ab\xff\xff"));
        assert!(!r.contains(b"ac"));
        assert!(!r.contains(b"aa\xff"));
        assert_eq!(prefix_successor(b"a\xff"), Some(b"b".to_vec()));
        assert_eq!(prefix_successor(b"\xff\xff"), None);
        let all = KeyRange::prefix(b"\xff");
        assert!(all.contains(b"\xff\x00"));
    }
}
