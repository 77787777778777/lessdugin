// SPDX-License-Identifier: AGPL-3.0-or-later
//! `JsHost`: the core [`Host`] over the JS host object of the engine worker
//! (`packages/engine/src/host.ts`, interface `JsHostImports`). See the crate docs for the
//! division of labour; this side only converts values.
//!
//! Every call into JS that can fail is `catch`: a synchronous throw and a rejected promise both
//! become a [`HostError`], from the rejection's `kind` (a `HostError` variant name in camelCase)
//! and `message`; anything else is `Other`.

use std::future::Future;
use std::ops::Bound;

use js_sys::{Array, Promise, Uint8Array};
use lessdugin_core::host::{
    BlobHandle, Dir, Host, HostError, HttpRequest, HttpResponse, KeyRange, KvOp, KvStore,
    Redirects, Target, WebSocket, WsMessage,
};
use lessdugin_core::limits;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    /// The worker's host object (`JsHostImports` in `host.ts`).
    #[wasm_bindgen(typescript_type = "JsHostImports")]
    pub type JsHostImports;

    /// `http(method, url, account, path, headers, body, maxBytes, timeoutMs, redirects)`:
    /// `url` for `Target::Anon`, `account` + `path` for `Target::UserPds`; `headers` is a flat
    /// `[name, value, …]` array; resolves with a `JsHttpResponse`.
    #[wasm_bindgen(method, catch)]
    fn http(
        this: &JsHostImports,
        method: &str,
        url: Option<String>,
        account: Option<String>,
        path: Option<String>,
        headers: Array,
        body: Option<Uint8Array>,
        max_bytes: f64,
        timeout_ms: u32,
        redirects: u8,
    ) -> Result<Promise, JsValue>;

    /// `fetchBlob(pds, did, cid, maxBytes)`: resolves with `{ cid, size, locator }`.
    #[wasm_bindgen(method, catch, js_name = fetchBlob)]
    fn fetch_blob(
        this: &JsHostImports,
        pds: &str,
        did: &str,
        cid: &str,
        max_bytes: f64,
    ) -> Result<Promise, JsValue>;

    /// `lock(name, wait)`: resolves with a `JsLockHandle`, or `null` when busy and `!wait`.
    #[wasm_bindgen(method, catch)]
    fn lock(this: &JsHostImports, name: &str, wait: bool) -> Result<Promise, JsValue>;

    /// `sleep(ms)`: resolves after `ms` milliseconds.
    #[wasm_bindgen(method)]
    fn sleep(this: &JsHostImports, ms: f64) -> Promise;

    /// `now()`: `Date.now()`.
    #[wasm_bindgen(method)]
    fn now(this: &JsHostImports) -> f64;

    /// `random(n)`: `n` bytes from `crypto.getRandomValues`.
    #[wasm_bindgen(method)]
    fn random(this: &JsHostImports, n: u32) -> Uint8Array;

    /// The `cache` store.
    #[wasm_bindgen(method, getter)]
    fn cache(this: &JsHostImports) -> JsKvBackend;

    /// The `state` store.
    #[wasm_bindgen(method, getter)]
    fn state(this: &JsHostImports) -> JsKvBackend;

    /// A `KvBackend` (`idb.ts`).
    #[wasm_bindgen(typescript_type = "KvBackend")]
    pub type JsKvBackend;

    /// `getMany(keys)`: one `Uint8Array | undefined` per key, in order.
    #[wasm_bindgen(method, catch, js_name = getMany)]
    fn get_many(this: &JsKvBackend, keys: Array) -> Result<Promise, JsValue>;

    /// `scan(lower, lowerOpen, upper, upperOpen, reverse, limit)`: a flat `[k, v, k, v, …]`.
    #[wasm_bindgen(method, catch)]
    fn scan(
        this: &JsKvBackend,
        lower: Option<Uint8Array>,
        lower_open: bool,
        upper: Option<Uint8Array>,
        upper_open: bool,
        reverse: bool,
        limit: u32,
    ) -> Result<Promise, JsValue>;

    /// `write(ops)`: `ops` is a flat array of `[tag, …]` entries (see [`JsKv::write`]).
    #[wasm_bindgen(method, catch)]
    fn write(this: &JsKvBackend, ops: Array) -> Result<Promise, JsValue>;

    /// A response of `http`.
    type JsHttpResponse;
    #[wasm_bindgen(method, getter)]
    fn status(this: &JsHttpResponse) -> u16;
    /// Flat `[name, value, …]`, names lowercased.
    #[wasm_bindgen(method, getter)]
    fn headers(this: &JsHttpResponse) -> Array;
    #[wasm_bindgen(method, getter)]
    fn body(this: &JsHttpResponse) -> Uint8Array;

    /// A stored blob of `fetchBlob`.
    type JsBlobHandle;
    #[wasm_bindgen(method, getter)]
    fn cid(this: &JsBlobHandle) -> String;
    #[wasm_bindgen(method, getter)]
    fn size(this: &JsBlobHandle) -> f64;
    #[wasm_bindgen(method, getter)]
    fn locator(this: &JsBlobHandle) -> String;

    /// A held Web Lock.
    #[wasm_bindgen(typescript_type = "LockHandle")]
    pub type JsLockHandle;
    #[wasm_bindgen(method)]
    fn release(this: &JsLockHandle);

    /// A rejection of the host (`{ kind, message, status?, name? }`).
    type JsHostError;
    #[wasm_bindgen(method, getter)]
    fn kind(this: &JsHostError) -> Option<String>;
    #[wasm_bindgen(method, getter)]
    fn message(this: &JsHostError) -> Option<String>;
    #[wasm_bindgen(method, getter)]
    fn status(this: &JsHostError) -> Option<u16>;
    #[wasm_bindgen(method, getter)]
    fn name(this: &JsHostError) -> Option<String>;
}

/// A JS rejection as a [`HostError`].
pub(crate) fn host_error(v: JsValue) -> HostError {
    if !v.is_object() {
        return HostError::Other(v.as_string().unwrap_or_else(|| "host error".into()));
    }
    let e: JsHostError = v.unchecked_into();
    let message = e.message().unwrap_or_default();
    match e.kind().as_deref() {
        Some("network") => HostError::Network(message),
        Some("timeout") => HostError::Timeout,
        Some("tooLarge") => HostError::TooLarge,
        Some("blocked") => HostError::Blocked(message),
        Some("cors") => HostError::Cors,
        Some("authRequired") => HostError::AuthRequired(message),
        Some("storage") => HostError::Storage(message),
        Some("unsupported") => HostError::Unsupported(message),
        Some("cidMismatch") => HostError::CidMismatch,
        Some("httpStatus") => HostError::HttpStatus(e.status().unwrap_or(0), e.name()),
        // a JS exception (TypeError, DOMException…) or an unknown kind
        _ => {
            let name = e.name().unwrap_or_default();
            HostError::Other(if name.is_empty() {
                message
            } else {
                format!("{name}: {message}")
            })
        }
    }
}

async fn await_promise(p: Result<Promise, JsValue>) -> Result<JsValue, HostError> {
    JsFuture::from(p.map_err(host_error)?)
        .await
        .map_err(host_error)
}

fn bytes(v: &[u8]) -> Uint8Array {
    Uint8Array::from(v)
}

/// A `KvStore` over a JS `KvBackend` (IndexedDB in the browser).
pub struct JsKv {
    backend: JsKvBackend,
}

impl JsKv {
    /// Wraps a backend.
    pub fn new(backend: JsKvBackend) -> Self {
        Self { backend }
    }
}

fn bound(b: &Bound<Vec<u8>>) -> (Option<Uint8Array>, bool) {
    match b {
        Bound::Included(k) => (Some(bytes(k)), false),
        Bound::Excluded(k) => (Some(bytes(k)), true),
        Bound::Unbounded => (None, false),
    }
}

/// Write-op tags of the flat `write` array: `0 key value` put, `1 key` delete,
/// `2 lower lowerOpen upper upperOpen` delete range (bounds `undefined` when unbounded).
const OP_PUT: u32 = 0;
const OP_DELETE: u32 = 1;
const OP_DELETE_RANGE: u32 = 2;

fn opt_bytes(b: Option<Uint8Array>) -> JsValue {
    b.map(JsValue::from).unwrap_or(JsValue::UNDEFINED)
}

impl KvStore for JsKv {
    fn get_many(
        &self,
        keys: Vec<Vec<u8>>,
    ) -> impl Future<Output = Result<Vec<Option<Vec<u8>>>, HostError>> {
        let arr = Array::new();
        for k in &keys {
            arr.push(&bytes(k));
        }
        let p = self.backend.get_many(arr);
        async move {
            let got: Array = await_promise(p).await?.unchecked_into();
            if got.length() as usize != keys.len() {
                return Err(HostError::Storage(format!(
                    "getMany returned {} values for {} keys",
                    got.length(),
                    keys.len()
                )));
            }
            Ok(got
                .iter()
                .map(|v| {
                    (!v.is_undefined() && !v.is_null())
                        .then(|| v.unchecked_into::<Uint8Array>().to_vec())
                })
                .collect())
        }
    }

    #[allow(clippy::type_complexity)]
    fn scan(
        &self,
        range: KeyRange,
        dir: Dir,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<(Vec<u8>, Vec<u8>)>, HostError>> {
        let (lower, lower_open) = bound(&range.start);
        let (upper, upper_open) = bound(&range.end);
        let p = self.backend.scan(
            lower,
            lower_open,
            upper,
            upper_open,
            dir == Dir::Reverse,
            limit,
        );
        async move {
            let flat: Array = await_promise(p).await?.unchecked_into();
            let n = flat.length();
            if n % 2 != 0 || n / 2 > limit {
                return Err(HostError::Storage(format!(
                    "scan returned {n} items for a limit of {limit} pairs"
                )));
            }
            let mut out = Vec::with_capacity((n / 2) as usize);
            for i in (0..n).step_by(2) {
                let k: Uint8Array = flat.get(i).unchecked_into();
                let v: Uint8Array = flat.get(i + 1).unchecked_into();
                out.push((k.to_vec(), v.to_vec()));
            }
            Ok(out)
        }
    }

    fn write(&self, batch: Vec<KvOp>) -> impl Future<Output = Result<(), HostError>> {
        let ops = Array::new();
        for op in &batch {
            match op {
                KvOp::Put(k, v) => {
                    ops.push(&OP_PUT.into());
                    ops.push(&bytes(k));
                    ops.push(&bytes(v));
                }
                KvOp::Delete(k) => {
                    ops.push(&OP_DELETE.into());
                    ops.push(&bytes(k));
                }
                KvOp::DeleteRange(r) => {
                    let (lower, lower_open) = bound(&r.start);
                    let (upper, upper_open) = bound(&r.end);
                    ops.push(&OP_DELETE_RANGE.into());
                    ops.push(&opt_bytes(lower));
                    ops.push(&lower_open.into());
                    ops.push(&opt_bytes(upper));
                    ops.push(&upper_open.into());
                }
            }
        }
        drop(batch);
        let p = self.backend.write(ops);
        async move { await_promise(p).await.map(|_| ()) }
    }
}

/// A held Web Lock; dropping it releases the lock.
pub struct JsLock {
    handle: JsLockHandle,
}

impl Drop for JsLock {
    fn drop(&mut self) {
        self.handle.release();
    }
}

/// The browser has no WebSocket host in M2: Jetstream arrives with M5 and `live_tail` polls.
pub struct NoWebSocket;

impl WebSocket for NoWebSocket {
    async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
        None
    }

    async fn send_text(&mut self, _text: String) -> Result<(), HostError> {
        Err(HostError::Unsupported("WebSocket".into()))
    }

    async fn close(&mut self) {}
}

/// The worker's host.
pub struct JsHost {
    imports: JsHostImports,
    cache: JsKv,
    state: JsKv,
}

impl JsHost {
    /// A host over the worker's JS host object.
    pub fn new(imports: JsHostImports) -> Self {
        let cache = JsKv::new(imports.cache());
        let state = JsKv::new(imports.state());
        Self {
            imports,
            cache,
            state,
        }
    }
}

fn redirect_count(r: Redirects) -> u8 {
    match r {
        Redirects::None => 0,
        Redirects::Follow(n) => n.min(limits::MAX_REDIRECTS),
    }
}

fn response(v: JsValue) -> HttpResponse {
    let r: JsHttpResponse = v.unchecked_into();
    let flat = r.headers();
    let mut headers = Vec::with_capacity((flat.length() / 2) as usize);
    for i in (0..flat.length().saturating_sub(1)).step_by(2) {
        if let (Some(k), Some(v)) = (flat.get(i).as_string(), flat.get(i + 1).as_string()) {
            headers.push((k.to_ascii_lowercase(), v));
        }
    }
    HttpResponse {
        status: r.status(),
        headers,
        body: r.body().to_vec(),
    }
}

impl Host for JsHost {
    type Kv = JsKv;
    type Ws = NoWebSocket;
    type Lock = JsLock;

    fn cache(&self) -> &JsKv {
        &self.cache
    }

    fn state(&self) -> &JsKv {
        &self.state
    }

    fn http(&self, req: HttpRequest) -> impl Future<Output = Result<HttpResponse, HostError>> {
        let (url, account, path) = match &req.target {
            Target::Anon(u) => (Some(u.as_str().to_owned()), None, None),
            Target::UserPds {
                account,
                path_and_query,
            } => (
                None,
                Some(account.as_str().to_owned()),
                Some(path_and_query.clone()),
            ),
        };
        let headers = Array::new();
        for (k, v) in &req.headers {
            headers.push(&JsValue::from_str(k));
            headers.push(&JsValue::from_str(v));
        }
        let redirects = match req.target {
            Target::UserPds { .. } => 0,
            Target::Anon(_) => redirect_count(req.redirects),
        };
        let p = self.imports.http(
            req.method.as_str(),
            url,
            account,
            path,
            headers,
            req.body.as_deref().map(bytes),
            req.max_bytes as f64,
            req.timeout_ms,
            redirects,
        );
        async move {
            let r = response(await_promise(p).await?);
            if r.body.len() as u64 > req.max_bytes {
                return Err(HostError::TooLarge);
            }
            Ok(r)
        }
    }

    fn fetch_blob(
        &self,
        pds: url::Url,
        did: lessdugin_atp::syntax::Did,
        cid: String,
        max_bytes: u64,
    ) -> impl Future<Output = Result<BlobHandle, HostError>> {
        let p = self
            .imports
            .fetch_blob(pds.as_str(), did.as_str(), &cid, max_bytes as f64);
        async move {
            let h: JsBlobHandle = await_promise(p).await?.unchecked_into();
            if h.cid() != cid {
                return Err(HostError::Other("fetchBlob returned another CID".into()));
            }
            Ok(BlobHandle {
                cid: h.cid(),
                size: h.size() as u64,
                locator: h.locator(),
            })
        }
    }

    async fn websocket(&self, _url: url::Url, _max: u32) -> Result<NoWebSocket, HostError> {
        Err(HostError::Unsupported(
            "WebSocket (Jetstream) is not available in the browser before M5".into(),
        ))
    }

    async fn resolve_txt_system(&self, _name: String) -> Result<Option<Vec<String>>, HostError> {
        Err(HostError::Unsupported(
            "the browser has no system DNS resolver (DoH is used)".into(),
        ))
    }

    fn lock(
        &self,
        name: &str,
        wait: bool,
    ) -> impl Future<Output = Result<Option<JsLock>, HostError>> {
        let p = self.imports.lock(name, wait);
        async move {
            let v = await_promise(p).await?;
            Ok((!v.is_null() && !v.is_undefined()).then(|| JsLock {
                handle: v.unchecked_into(),
            }))
        }
    }

    fn sleep(&self, ms: u64) -> impl Future<Output = ()> {
        let p = self.imports.sleep(ms as f64);
        async move {
            let _ = JsFuture::from(p).await;
        }
    }

    fn now_ms(&self) -> i64 {
        self.imports.now() as i64
    }

    fn fill_random(&self, buf: &mut [u8]) {
        let r = self.imports.random(buf.len() as u32);
        if r.length() as usize == buf.len() {
            r.copy_to(buf);
        } else {
            // never fall back to weak randomness
            panic!(
                "crypto.getRandomValues returned {} bytes, not {}",
                r.length(),
                buf.len()
            );
        }
    }
}
