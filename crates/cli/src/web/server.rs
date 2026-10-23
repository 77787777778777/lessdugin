// SPDX-License-Identifier: AGPL-3.0-or-later
//! The HTTP server of `lessdugin web` (`threat-model.md` §7).
//!
//! There is no routing table: one handler checks every request in a fixed order, so that no
//! framework answer (a 405 from a method router, a 404) can come before the Host check.
//!
//! ```text
//! every request:  Host ≠ 127.0.0.1:<port>                  → 403, empty body   (WEB-3)
//!                 OPTIONS                                   → 405              (WEB-4)
//! /bridge/…:      not session|login|kv|lock|xrpc/<nsid>     → 404 (/bridge/fetch is reserved)
//!                 method ≠ POST                             → 405              (§7.3 #2)
//!                 Origin ≠ http://127.0.0.1:<port>           → 403              (#3)
//!                 no live `Authorization: Bearer <token>` (not /bridge/session) → 401 (#4)
//!                 Content-Type ≠ application/json (uploadBlob: a §8.8 type)    → 415 (#5)
//!                 body over its limit                       → 413              (#6)
//!                 then the endpoint
//! anything else:  GET/HEAD of an embedded file, or /lessdugin-config.json; else 404/405
//! ```
//!
//! Every response then gets the CSP and §6.3 headers ([`policy::apply`]); bridge responses get
//! `Cache-Control: no-store`. Bridge errors are JSON `{"error": <engine error code>,
//! "message": …}`; answers forwarded from a PDS carry `X-Lessdugin-Upstream: pds`.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use lessdugin_core::engine::{Engine, EngineError, ErrorCode};
use lessdugin_native::auth::SecretStore;
use lessdugin_native::config::Paths;
use lessdugin_native::host::NativeHost;
use serde_json::json;

use super::assets::Assets;
use super::caps::{Caps, Exchange};
use super::login::{LoginSlot, Prompt};
use super::policy::{
    self, BRIDGE_BLOB_MAX_BODY_BYTES, BRIDGE_JSON_MAX_BODY_BYTES, BRIDGE_KV_MAX_BODY_BYTES,
};
use super::{kv, lock, login, xrpc};
use crate::out::{Printer, Style, lit};

/// The body of `/lessdugin-config.json` (WEB-5).
pub const CONFIG_JSON: &str = "{\"oauth\":\"bridge\"}";

/// The server's state.
pub struct Web {
    /// The port on 127.0.0.1 (part of the origin).
    pub port: u16,
    /// `--dev`: the §6.2 development policy.
    pub dev: bool,
    /// The embedded SPA.
    pub assets: Assets,
    /// Capabilities and sessions.
    pub caps: Caps,
    /// The native engine (accounts, sessions, the `state` store).
    pub engine: Engine<NativeHost>,
    /// The session store.
    pub store: SecretStore,
    /// Paths (refresh locks).
    pub paths: Paths,
    /// Terminal output.
    pub out: Printer,
    /// The pending browser login, if any.
    pub login: LoginSlot,
    /// Questions to the terminal (a browser sign-in's confirmation).
    pub prompt: Prompt,
    /// Set to `true` when the server stops (held `/bridge/lock` bodies end).
    pub shutdown: tokio::sync::watch::Sender<bool>,
}

impl Web {
    /// The allowed `Host` values (WEB-3): the listener is bound to 127.0.0.1 only (WEB-1), so
    /// no browser reaches it under `[::1]`.
    fn hosts(&self) -> [String; 1] {
        [format!("127.0.0.1:{}", self.port)]
    }

    /// The allowed `Origin` values (§7.3 #3).
    fn origins(&self) -> [String; 1] {
        [self.origin()]
    }

    /// The bridge's own origin, for links and redirects.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 32 bytes from the OS CSPRNG.
    pub fn random32(&self) -> [u8; 32] {
        use lessdugin_core::host::Host as _;
        let mut b = [0u8; 32];
        self.engine.host().fill_random(&mut b);
        b
    }

    /// The wall clock (ms).
    pub fn now_ms(&self) -> i64 {
        use lessdugin_core::host::Host as _;
        self.engine.host().now_ms()
    }

    /// Mints a fresh single-use link (§7.2).
    pub fn mint_link(&self) -> String {
        let cap = self.caps.mint(&self.random32(), self.now_ms());
        format!("{}/#lessdugin-cap={cap}", self.origin())
    }
}

/// True if the request's `Host` (and, for an absolute-form or HTTP/2 request, the URI's
/// authority) is exactly one of `allowed`. Several `Host` headers fail.
pub fn host_ok(req: &Request, allowed: &[String]) -> bool {
    let mut hosts = req.headers().get_all(header::HOST).iter();
    let host = hosts.next();
    if hosts.next().is_some() {
        return false;
    }
    let is_allowed = |v: &str| allowed.iter().any(|a| a == v);
    let authority = req.uri().authority().map(|a| a.as_str());
    match (host, authority) {
        (None, None) => false,
        (Some(h), a) => h.to_str().is_ok_and(is_allowed) && a.is_none_or(is_allowed),
        (None, Some(a)) => is_allowed(a),
    }
}

/// An empty response.
pub fn empty(status: StatusCode) -> Response {
    (status, Body::empty()).into_response()
}

/// A JSON response.
pub fn json_response(status: StatusCode, value: &serde_json::Value) -> Response {
    let mut r = (status, value.to_string()).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
}

/// A bridge error: `{"error": code, "message": message}`.
pub fn error(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, &json!({"error": code, "message": message}))
}

/// The HTTP status of an engine error code.
pub fn status_of(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::InvalidInput | ErrorCode::InvalidRef => StatusCode::BAD_REQUEST,
        ErrorCode::AuthRequired | ErrorCode::Blocked => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::Network | ErrorCode::TooLarge => StatusCode::BAD_GATEWAY,
        ErrorCode::Unsupported => StatusCode::NOT_IMPLEMENTED,
        ErrorCode::Partial | ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// An engine error as a bridge error (message redacted, `detail` kept).
pub fn engine_error(e: &EngineError) -> Response {
    let message = lessdugin_native::auth::redact::redact_secrets(&e.message);
    let mut body = json!({"error": e.code.as_str(), "message": message});
    if let Some(d) = &e.detail {
        body["detail"] = d.clone();
    }
    json_response(status_of(e.code), &body)
}

/// The one handler.
pub async fn handle(State(web): State<Arc<Web>>, req: Request) -> Response {
    let mut resp = dispatch(&web, req).await;
    policy::apply(resp.headers_mut(), web.dev);
    resp
}

async fn dispatch(web: &Arc<Web>, req: Request) -> Response {
    if !host_ok(&req, &web.hosts()) {
        return empty(StatusCode::FORBIDDEN);
    }
    if req.method() == Method::OPTIONS {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let path = req.uri().path();
    if path == "/bridge" || path.starts_with("/bridge/") {
        let mut resp = bridge(web, req).await;
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return resp;
    }
    static_file(web, &req)
}

// ---------------------------------------------------------------------------------------------
// Static files

fn static_file(web: &Web, req: &Request) -> Response {
    let head = req.method() == Method::HEAD;
    if req.method() != Method::GET && !head {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let found = if req.uri().path() == "/lessdugin-config.json" {
        Some(("application/json", "no-cache", CONFIG_JSON.as_bytes()))
    } else {
        web.assets
            .get(req.uri().path())
            .map(|a| (a.content_type, a.cache_control, a.body))
    };
    let (status, (content_type, cache_control, body)) = match found {
        Some(f) => (StatusCode::OK, f),
        None => (
            StatusCode::NOT_FOUND,
            ("text/plain; charset=utf-8", "no-cache", &b"not found\n"[..]),
        ),
    };
    let mut r = if head {
        let mut r = empty(status);
        r.headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        r
    } else {
        (status, Body::from(body)).into_response()
    };
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    r
}

// ---------------------------------------------------------------------------------------------
// The bridge

/// A bridge endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Route {
    Session,
    Login,
    Kv,
    Lock,
    Xrpc(String),
}

impl Route {
    fn parse(path: &str) -> Option<Self> {
        match path {
            "/bridge/session" => Some(Self::Session),
            "/bridge/login" => Some(Self::Login),
            "/bridge/kv" => Some(Self::Kv),
            "/bridge/lock" => Some(Self::Lock),
            p => p
                .strip_prefix("/bridge/xrpc/")
                .filter(|n| !n.is_empty())
                .map(|n| Self::Xrpc(n.to_owned())),
        }
    }

    fn is_upload(&self) -> bool {
        matches!(self, Self::Xrpc(n) if n == xrpc::UPLOAD_BLOB)
    }

    fn max_body(&self) -> usize {
        match self {
            Self::Kv => BRIDGE_KV_MAX_BODY_BYTES,
            r if r.is_upload() => BRIDGE_BLOB_MAX_BODY_BYTES,
            _ => BRIDGE_JSON_MAX_BODY_BYTES,
        }
    }
}

/// The single value of a header, if there is exactly one.
fn single(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    let mut all = headers.get_all(name).iter();
    let first = all.next()?;
    if all.next().is_some() {
        return None;
    }
    first.to_str().ok()
}

/// The media type of a `Content-Type` value, lowercased, without parameters.
pub fn media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

async fn bridge(web: &Arc<Web>, req: Request) -> Response {
    let Some(route) = Route::parse(req.uri().path()) else {
        return error(
            StatusCode::NOT_FOUND,
            "not_found",
            "no such bridge endpoint",
        );
    };
    // #2 method
    if req.method() != Method::POST {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_input",
            "bridge endpoints take POST only",
        );
    }
    let (parts, body) = req.into_parts();
    // #3 Origin
    let origin_ok = single(&parts.headers, header::ORIGIN)
        .is_some_and(|o| web.origins().iter().any(|a| a == o));
    if !origin_ok {
        return error(StatusCode::FORBIDDEN, "blocked", "wrong or missing Origin");
    }
    // #4 session token
    let token = single(&parts.headers, header::AUTHORIZATION)
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned();
    if route != Route::Session && !web.caps.is_session(&token) {
        return error(
            StatusCode::UNAUTHORIZED,
            "auth_required",
            "no live bridge session: open a fresh link from the terminal",
        );
    }
    // #5 Content-Type
    let content_type = single(&parts.headers, header::CONTENT_TYPE).map(media_type);
    let type_ok = if route.is_upload() {
        content_type
            .as_deref()
            .is_some_and(|t| xrpc::DECLARED_TYPES.contains(&t))
    } else {
        content_type.as_deref() == Some("application/json")
    };
    if !type_ok {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_input",
            if route.is_upload() {
                "uploadBlob takes a declared media type (data-model.md 8.8)"
            } else {
                "the body must be application/json"
            },
        );
    }
    // #6 body limit
    let limit = route.max_body();
    let declared = single(&parts.headers, header::CONTENT_LENGTH).and_then(|v| v.parse().ok());
    let bytes = match read_body(body, limit, declared).await {
        Ok(b) => b,
        Err(BodyError::TooLarge) => return too_large(limit),
        Err(BodyError::Broken) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "the body could not be read",
            );
        }
    };
    match route {
        Route::Session => session(web, &bytes),
        Route::Login => login::start(web, &token, &bytes).await,
        Route::Kv => kv::handle(web, &bytes).await,
        Route::Lock => lock::handle(web, &token, &bytes).await,
        Route::Xrpc(nsid) => {
            xrpc::forward(
                web,
                &nsid,
                parts.uri.query(),
                content_type.as_deref().unwrap_or(""),
                bytes,
            )
            .await
        }
    }
}

/// Why a body was not read.
enum BodyError {
    /// Over the limit.
    TooLarge,
    /// The connection failed.
    Broken,
}

/// Extra bytes of an oversized body read and discarded before answering 413, so that the client
/// (still sending) sees the answer rather than a reset connection.
const DRAIN_EXTRA_BYTES: usize = 16 * 1024 * 1024;

/// Reads a body of at most `limit` bytes. An oversized one (by its `Content-Length`, or as it
/// arrives) is drained (discarded, up to [`DRAIN_EXTRA_BYTES`] more) and refused.
async fn read_body(body: Body, limit: usize, declared: Option<u64>) -> Result<Bytes, BodyError> {
    use futures::StreamExt as _;
    let mut stream = body.into_data_stream();
    let mut buf = Vec::new();
    let mut total = 0usize;
    if declared.is_some_and(|n| n > limit as u64) {
        // Known to be too large: never buffer it.
        total = limit.saturating_add(1);
    }
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| BodyError::Broken)?;
        total = total.saturating_add(chunk.len());
        if total > limit {
            buf = Vec::new();
            if total > limit.saturating_add(DRAIN_EXTRA_BYTES) {
                return Err(BodyError::TooLarge);
            }
        } else {
            buf.extend_from_slice(&chunk);
        }
    }
    if total > limit {
        Err(BodyError::TooLarge)
    } else {
        Ok(Bytes::from(buf))
    }
}

fn too_large(limit: usize) -> Response {
    error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "too_large",
        &format!("the body is over the limit of {limit} bytes"),
    )
}

/// `POST /bridge/session {"cap": c}` (§7.2).
fn session(web: &Web, body: &[u8]) -> Response {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Req {
        cap: String,
    }
    let Ok(req) = serde_json::from_slice::<Req>(body) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "expected {\"cap\": \"…\"}",
        );
    };
    match web.caps.exchange(&req.cap, &web.random32(), web.now_ms()) {
        Exchange::Token(token) => json_response(StatusCode::OK, &json!({"token": token})),
        Exchange::Reused => {
            web.out.err_line(&[
                (lit("warning: "), Style::Warn),
                (
                    lit("a lessdugin web link was used twice; its session was revoked"),
                    Style::Plain,
                ),
            ]);
            // A sign-in the revoked session started dies with it (F27: whoever raced the
            // link must not finish a login the legitimate tab never asked for).
            web.login.cancel_if_revoked(web);
            expired()
        }
        Exchange::Refused => expired(),
    }
}

fn expired() -> Response {
    error(
        StatusCode::FORBIDDEN,
        "auth_required",
        "this link was already used or has expired; open a fresh link from the terminal",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(host: Option<&str>, uri: &str) -> Request {
        let mut b = Request::builder().uri(uri);
        if let Some(h) = host {
            b = b.header(header::HOST, h);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn host_check() {
        let allowed = ["127.0.0.1:47778".to_owned()];
        assert!(host_ok(&req(Some("127.0.0.1:47778"), "/"), &allowed));
        for bad in [
            "[::1]:47778",
            "localhost:47778",
            "127.0.0.1",
            "127.0.0.1:80",
            "evil.example:47778",
            "127.0.0.1:47778.evil.example",
            "127.0.0.1:047778",
            "",
        ] {
            assert!(!host_ok(&req(Some(bad), "/"), &allowed), "{bad}");
        }
        assert!(!host_ok(&req(None, "/"), &allowed));
        // An absolute-form target must agree with Host.
        assert!(!host_ok(
            &req(Some("127.0.0.1:47778"), "http://evil.example/"),
            &allowed
        ));
        assert!(host_ok(
            &req(Some("127.0.0.1:47778"), "http://127.0.0.1:47778/"),
            &allowed
        ));
        // Two Host headers.
        let two = Request::builder()
            .uri("/")
            .header(header::HOST, "127.0.0.1:47778")
            .header(header::HOST, "evil.example")
            .body(Body::empty())
            .unwrap();
        assert!(!host_ok(&two, &allowed));
    }

    #[test]
    fn routes() {
        assert_eq!(Route::parse("/bridge/session"), Some(Route::Session));
        assert_eq!(Route::parse("/bridge/lock"), Some(Route::Lock));
        assert_eq!(
            Route::parse("/bridge/xrpc/com.atproto.repo.putRecord"),
            Some(Route::Xrpc("com.atproto.repo.putRecord".into()))
        );
        assert!(Route::parse("/bridge/xrpc/").is_none());
        assert!(Route::parse("/bridge/fetch").is_none());
        assert!(Route::parse("/bridge/session/").is_none());
        assert!(Route::parse("/bridge").is_none());
        assert_eq!(
            media_type("Application/JSON; charset=utf-8"),
            "application/json"
        );
    }
}
