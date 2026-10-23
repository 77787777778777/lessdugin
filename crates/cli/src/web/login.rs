// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /bridge/login` and the OAuth redirect (`threat-model.md` §7.4, AUTH-5, F25).
//!
//! ```text
//! POST /bridge/login {"actor": <handle or DID>}
//!   the target is resolved now (never through a hint, `resolve_login_target`); a handle that
//!   is not the account's handle verified both ways is refused (403 blocked,
//!   detail.kind = "unverifiedHandle", detail.did): the tab can retry with the DID
//!   native loopback OAuth starts (`oauth::begin`, PAR + PKCE + DPoP, `response_mode=query`)
//!   with the redirect URI http://127.0.0.1:<p>/callback, served by this process
//!   → 200 {"url": <authorization URL>}: the tab navigates there
//! GET http://127.0.0.1:<p>/callback?code&state&iss
//!   Host must be 127.0.0.1:<p>; `state` and `iss` are checked (PKCE at the exchange), the
//!   login binding runs (`OAuthGrant::finish`, ID-11, `sync.md` §3.6), the session goes to the
//!   keyring, and the answer is a 303 to http://127.0.0.1:<port>/#/ (on failure, to
//!   /#/login?error=<engine error code>; the terminal says why)
//! ```
//!
//! At most one login is pending: a new `/bridge/login` cancels the previous one, and a pending
//! login expires after `OAUTH_LOGIN_TTL_MS` (10 minutes).
//!
//! **Deviation (M2).** §7.4 names the bridge's own port as the redirect target. The native
//! OAuth facade (`lessdugin_native::auth::oauth::begin`) reserves its own ephemeral loopback
//! port for the redirect URI, so the redirect goes to that port, which this process serves
//! (a second listener on 127.0.0.1, only `GET /callback`, with the same Host check and
//! headers) for the lifetime of the pending login. The tab returns to the bridge's origin with
//! the 303, so its `sessionStorage` is kept as §7.4 intends. Once `begin` can take a given
//! redirect URI, the callback can move to the bridge port without changing the SPA.
//! An authorization server that changed since the account's last login is refused here (it
//! needs the terminal's confirmation: `lessdugin login`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use lessdugin_core::engine::{EngineError, ErrorCode};
use lessdugin_native::auth::oauth::{self, ClientKind, OAuthOptions, PendingAuthorization};
use lessdugin_native::auth::resolve_login_target;
use serde_json::json;
use tokio::sync::Notify;

use super::policy::{self, OAUTH_LOGIN_TTL_MS};
use super::server::{Web, empty, engine_error, error, host_ok, json_response};
use crate::out::{Style, cat, lit, name, tt};

/// The pending browser login.
#[derive(Default)]
pub struct LoginSlot {
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl LoginSlot {
    /// Replaces the pending login (the previous one is cancelled).
    fn replace(&self, task: tokio::task::JoinHandle<()>) {
        let old = self
            .task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(task);
        if let Some(old) = old {
            old.abort();
        }
    }

    /// Cancels the pending login, if any.
    pub fn cancel(&self) {
        if let Some(t) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            t.abort();
        }
    }
}

/// `POST /bridge/login`.
pub async fn start(web: &Arc<Web>, body: &[u8]) -> Response {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Req {
        actor: String,
    }
    let actor = match serde_json::from_slice::<Req>(body) {
        Ok(r) if !r.actor.trim().is_empty() && r.actor.len() <= 2048 => r.actor.trim().to_owned(),
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "expected {\"actor\": <handle or DID>}",
            );
        }
    };
    match begin(web, &actor).await {
        Ok(url) => json_response(StatusCode::OK, &json!({"url": url})),
        Err(e) => engine_error(&e),
    }
}

async fn begin(web: &Arc<Web>, actor: &str) -> Result<String, EngineError> {
    if lessdugin_core::engine::is_app_password_shape(actor) {
        return Err(EngineError::invalid_input(
            "that looks like an app password, not a handle or DID",
        ));
    }
    let target = resolve_login_target(&web.engine, actor).await?;
    if target.needs_confirmation() {
        return Err(EngineError::blocked(format!(
            "the handle is not verified for {}; sign in with the DID instead",
            target.did
        ))
        .with_detail("kind", "unverifiedHandle")
        .with_detail("did", target.did.as_str()));
    }
    let opts = OAuthOptions {
        client: ClientKind::Loopback,
        hosted_fallback: true,
        // The redirect port is reserved by `begin` and served below.
        listen: false,
    };
    let pending = oauth::begin(&web.engine, &target, &opts).await?;
    let port = pending
        .redirect_uri()
        .port()
        .ok_or_else(|| EngineError::internal("the redirect URI has no port"))?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|e| {
            EngineError::new(
                ErrorCode::Network,
                format!(
                    "cannot listen for the sign-in redirect on 127.0.0.1:{port} ({e}); try again"
                ),
            )
        })?;
    let url = pending.authorize_url().to_string();
    web.out.err_line(&[
        (lit("A browser tab started a sign-in as "), Style::Plain),
        (name(&target.display_name()), Style::Bold),
        (lit(" at "), Style::Plain),
        (tt(&pending.authorization_server_host()), Style::Bold),
        (lit("."), Style::Plain),
    ]);
    let cb = Arc::new(Callback {
        web: web.clone(),
        port,
        pending: Mutex::new(Some(pending)),
        done: Notify::new(),
    });
    let task = tokio::spawn(serve_callback(cb, listener));
    web.login.replace(task);
    Ok(url)
}

/// The redirect listener of one pending login.
struct Callback {
    web: Arc<Web>,
    port: u16,
    pending: Mutex<Option<PendingAuthorization>>,
    done: Notify,
}

async fn serve_callback(cb: Arc<Callback>, listener: tokio::net::TcpListener) {
    let app = Router::new()
        .fallback(callback_handler)
        .with_state(cb.clone());
    let done = cb.clone();
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        done.done.notified().await;
    });
    let ttl = Duration::from_millis(OAUTH_LOGIN_TTL_MS);
    if tokio::time::timeout(ttl, serve).await.is_err() {
        cb.web
            .out
            .note(&[(lit("The pending browser sign-in expired."), Style::Dim)]);
    }
}

async fn callback_handler(State(cb): State<Arc<Callback>>, req: Request) -> Response {
    let mut resp = callback(&cb, req).await;
    policy::apply(resp.headers_mut(), cb.web.dev);
    resp
}

fn redirect(to: &str) -> Response {
    let mut r = empty(StatusCode::SEE_OTHER);
    if let Ok(v) = HeaderValue::from_str(to) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn page(status: StatusCode, text: &'static str) -> Response {
    let body =
        format!("<!doctype html><meta charset=\"utf-8\"><title>lessdugin</title><p>{text}</p>");
    let mut r = (status, body).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    r
}

async fn callback(cb: &Callback, req: Request) -> Response {
    let allowed = [
        format!("127.0.0.1:{}", cb.port),
        format!("[::1]:{}", cb.port),
    ];
    if !host_ok(&req, &allowed) {
        return empty(StatusCode::FORBIDDEN);
    }
    if req.method() == Method::OPTIONS {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    if req.uri().path() != "/callback" {
        return empty(StatusCode::NOT_FOUND);
    }
    if req.method() != Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/callback");
    let url = format!("http://127.0.0.1:{}{pq}", cb.port);
    let checked = {
        let guard = cb.pending.lock().unwrap_or_else(|e| e.into_inner());
        let Some(p) = guard.as_ref() else {
            return page(StatusCode::NOT_FOUND, "This sign-in is already finished.");
        };
        p.parse_pasted(&url)
    };
    let origin = cb.web.origin();
    let code = match checked {
        Ok(code) => code,
        Err(e) if detail_str(&e, "kind") == "stateMismatch" => {
            // Not for this login: ignore it, keep waiting.
            return page(
                StatusCode::BAD_REQUEST,
                "This callback does not belong to the pending sign-in.",
            );
        }
        Err(e) => {
            cb.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
            cb.done.notify_one();
            report_failure(cb, &e);
            return redirect(&format!("{origin}/#/login?error={}", e.code.as_str()));
        }
    };
    let Some(pending) = cb.pending.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return page(StatusCode::NOT_FOUND, "This sign-in is already finished.");
    };
    let result = finish(cb, pending, code).await;
    cb.done.notify_one();
    match result {
        Ok(()) => redirect(&format!("{origin}/#/")),
        Err(e) => {
            report_failure(cb, &e);
            redirect(&format!("{origin}/#/login?error={}", e.code.as_str()))
        }
    }
}

async fn finish(
    cb: &Callback,
    pending: PendingAuthorization,
    code: oauth::AuthorizationCode,
) -> Result<(), EngineError> {
    let web = &cb.web;
    let grant = pending.exchange(code).await?;
    match grant
        .finish(&web.engine, &web.store, &web.paths, false)
        .await
    {
        Ok(outcome) => {
            let who = match &outcome.account.handle {
                Some(h) => cat([
                    lit("@"),
                    name(h),
                    lit(" ("),
                    tt(&outcome.account.did),
                    lit(")"),
                ]),
                None => tt(&outcome.account.did),
            };
            web.out.err_line(&[
                (lit("Signed in from the browser as "), Style::Plain),
                (who, Style::Bold),
                (lit("."), Style::Plain),
            ]);
            Ok(())
        }
        Err(e) => {
            grant.abandon().await;
            Err(e)
        }
    }
}

fn detail_str(e: &EngineError, key: &str) -> String {
    e.detail
        .as_ref()
        .and_then(|d| d.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

fn report_failure(cb: &Callback, e: &EngineError) {
    let msg = lessdugin_native::auth::redact::redact_secrets(&e.message);
    cb.web.out.err_line(&[
        (lit("warning: "), Style::Warn),
        (lit("the browser sign-in failed: "), Style::Plain),
        (tt(&msg), Style::Plain),
    ]);
    if e.code == ErrorCode::Conflict && detail_str(e, "kind") == "issuerChanged" {
        cb.web.out.err_line(&[(
            lit(
                "hint: the account's authorization server changed; run `lessdugin login` in a \
                 terminal to review and confirm it",
            ),
            Style::Dim,
        )]);
    }
}
