// SPDX-License-Identifier: AGPL-3.0-or-later
//! App-password sessions (`com.atproto.server.createSession`, `threat-model.md` AUTH-3).
//!
//! - [`AppPassword`] accepts only the shape `xxxx-xxxx-xxxx-xxxx`; anything else (a main
//!   password) is refused before any network call. It is read from a no-echo prompt (the CLI),
//!   stdin ([`AppPassword::from_reader`]) or `LESSDUGIN_APP_PASSWORD` ([`AppPassword::from_env`]),
//!   never from argv, and never persisted.
//! - [`create_session`] sends it to the PDS of the account's DID document, without redirects.
//! - [`AppPasswordSession`] is the [`AuthSession`] for the account: bearer tokens to the PDS bound
//!   at login, `refreshSession` when the access JWT expires (under the account's refresh lock,
//!   re-reading the stored session first, because refresh JWTs are single use and the CLI, TUI
//!   and `web` may run at once).

use std::io::BufRead;
use std::path::PathBuf;

use base64::Engine as _;
use futures::future::BoxFuture;
use lessdugin_atp::syntax::Did;
use lessdugin_atp::xrpc::{CreateSession, Session};
use lessdugin_core::engine::{EngineError, ErrorCode, is_app_password_shape};
use lessdugin_core::host::{HostError, HttpRequest, HttpResponse};
use lessdugin_core::limits;
use lessdugin_core::xrpc::XrpcCallError;
use tokio::sync::Mutex;
use url::Url;

use super::redact::Secret;
use super::store::{AppPasswordData, SESSION_VERSION, SecretStore, StoredSession, next_saved_ms};
use super::{expiring, now_ms};
use crate::config::FileLock;
use crate::host::{AuthSession, Net, NetRequest, user_pds_url};

/// Environment variable holding an app password (AUTH-3).
pub const APP_PASSWORD_ENV: &str = "LESSDUGIN_APP_PASSWORD";

/// A validated app password. `Debug` redacts it.
#[derive(Clone, Debug)]
pub struct AppPassword(Secret);

/// The refusal for anything that is not app-password shaped.
pub fn not_an_app_password() -> EngineError {
    EngineError::invalid_input(
        "that is not an app password: lessdugin accepts only app passwords \
         (xxxx-xxxx-xxxx-xxxx, created in your PDS's settings), never your main password",
    )
    .with_detail("kind", "notAnAppPassword")
}

impl AppPassword {
    /// Accepts `value` (one trailing line break is removed) if it has the app-password shape.
    pub fn new(value: impl Into<String>) -> Result<Self, EngineError> {
        let mut v = Secret::new(value);
        let trimmed = v
            .expose()
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s));
        if let Some(t) = trimmed {
            v = Secret::new(t);
        }
        if is_app_password_shape(v.expose()) {
            Ok(Self(v))
        } else {
            Err(not_an_app_password())
        }
    }

    /// `LESSDUGIN_APP_PASSWORD`, if set and non-empty.
    pub fn from_env() -> Option<Result<Self, EngineError>> {
        let v = std::env::var(APP_PASSWORD_ENV).ok()?;
        (!v.is_empty()).then(|| Self::new(v))
    }

    /// The first line of `reader` (`--password-stdin`), at most 1 KiB.
    pub fn from_reader(reader: impl BufRead) -> Result<Self, EngineError> {
        let mut line = String::new();
        let mut limited = reader.take(1024);
        limited.read_line(&mut line).map_err(|e| {
            EngineError::invalid_input(format!("cannot read the app password: {e}"))
        })?;
        if line.is_empty() {
            return Err(EngineError::invalid_input(
                "no app password on standard input",
            ));
        }
        Self::new(line)
    }

    /// The password, for the one request that sends it.
    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

/// The `exp` claim of a JWT in milliseconds, without verifying it (only to schedule refreshes).
pub fn jwt_exp_ms(jwt: &str) -> Option<i64> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp")?.as_i64()?.checked_mul(1000)
}

fn xrpc_error(resp: &HttpResponse) -> EngineError {
    EngineError::from(XrpcCallError::from_response(resp, now_ms()))
}

fn error_name(resp: &HttpResponse) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&resp.body).ok()?;
    v.get("error")?.as_str().map(str::to_owned)
}

/// `com.atproto.server.createSession` at `pds` (an origin, the DID document's PDS) with an
/// app password. Never follows redirects; the password never appears in logs or errors.
pub async fn create_session(
    net: &Net,
    pds: &Url,
    identifier: &str,
    password: &AppPassword,
) -> Result<Session, EngineError> {
    let url = user_pds_url(pds, "/xrpc/com.atproto.server.createSession")?;
    let body = serde_json::to_vec(&CreateSession {
        identifier: identifier.to_owned(),
        password: password.expose().to_owned(),
        auth_factor_token: None,
        allow_takendown: None,
    })
    .map_err(|e| EngineError::internal(e.to_string()))?;
    let req = NetRequest {
        method: http::Method::POST,
        url,
        headers: vec![("content-type".into(), "application/json".into())],
        body: Some(body),
        max_bytes: limits::SMALL_RESPONSE_MAX_BYTES,
        timeout_ms: limits::REQUEST_TIMEOUT_MS,
        max_redirects: 0,
    };
    let resp = net.execute(req).await?;
    if !(200..300).contains(&resp.status) {
        return Err(match error_name(&resp).as_deref() {
            Some("AuthFactorTokenRequired") => EngineError::invalid_input(
                "the PDS asked for an email sign-in code, which only main passwords need: \
                 use an app password",
            ),
            Some("AuthenticationRequired") => EngineError::new(
                ErrorCode::AuthRequired,
                "the PDS rejected the identifier or app password",
            )
            .with_detail("error", "AuthenticationRequired"),
            _ => xrpc_error(&resp).context("createSession"),
        });
    }
    serde_json::from_slice::<Session>(&resp.body)
        .map_err(|e| EngineError::new(ErrorCode::Network, format!("createSession: {e}")))
}

/// `com.atproto.server.deleteSession` with the refresh JWT (logout, best effort).
pub async fn delete_session(net: &Net, pds: &Url, refresh_jwt: &Secret) -> Result<(), EngineError> {
    let url = user_pds_url(pds, "/xrpc/com.atproto.server.deleteSession")?;
    let req = NetRequest {
        method: http::Method::POST,
        url,
        headers: vec![(
            "authorization".into(),
            format!("Bearer {}", refresh_jwt.expose()),
        )],
        body: None,
        max_bytes: limits::SMALL_RESPONSE_MAX_BYTES,
        timeout_ms: limits::REQUEST_TIMEOUT_MS,
        max_redirects: 0,
    };
    let resp = net.execute(req).await?;
    if (200..300).contains(&resp.status) {
        Ok(())
    } else {
        Err(xrpc_error(&resp).context("deleteSession"))
    }
}

/// The stored form of a fresh `createSession` / `refreshSession` result (saved now).
pub fn session_data(session: &Session, pds: &str, created_ms: i64) -> AppPasswordData {
    AppPasswordData {
        v: SESSION_VERSION,
        did: session.did.clone(),
        handle: session.handle.clone(),
        pds: pds.to_owned(),
        access_jwt: Secret::new(session.access_jwt.clone()),
        refresh_jwt: Secret::new(session.refresh_jwt.clone()),
        access_exp_ms: jwt_exp_ms(&session.access_jwt),
        refresh_exp_ms: jwt_exp_ms(&session.refresh_jwt),
        created_ms,
        saved_ms: next_saved_ms(0),
    }
}

/// An app-password session for one account, bound to the PDS of its login.
pub struct AppPasswordSession {
    did: Did,
    pds: Url,
    net: Net,
    store: SecretStore,
    lock_file: PathBuf,
    data: Mutex<AppPasswordData>,
}

impl std::fmt::Debug for AppPasswordSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppPasswordSession")
            .field("did", &self.did)
            .field("pds", &self.pds.as_str())
            .finish_non_exhaustive()
    }
}

/// True for the answers that mean "the access token is no longer valid".
fn token_expired(resp: &HttpResponse) -> bool {
    matches!(resp.status, 400 | 401)
        && matches!(
            error_name(resp).as_deref(),
            Some("ExpiredToken" | "InvalidToken")
        )
}

impl AppPasswordSession {
    /// A session from stored data. `lock_file` is the account's refresh lock
    /// (`Paths::lock_file("refresh:<did>")`).
    pub fn new(
        data: AppPasswordData,
        net: Net,
        store: SecretStore,
        lock_file: PathBuf,
    ) -> Result<Self, EngineError> {
        let did = Did::parse(&data.did).map_err(|e| EngineError::invalid_input(e.to_string()))?;
        let pds = Url::parse(&data.pds)
            .map_err(|e| EngineError::invalid_input(format!("stored PDS: {e}")))?;
        Ok(Self {
            did,
            pds,
            net,
            store,
            lock_file,
            data: Mutex::new(data),
        })
    }

    /// The PDS this session talks to.
    pub fn pds(&self) -> &Url {
        &self.pds
    }

    /// The current session data (tokens redacted in `Debug`).
    pub async fn data(&self) -> AppPasswordData {
        self.data.lock().await.clone()
    }

    async fn access_token(&self, force_refresh: bool) -> Result<Secret, HostError> {
        let mut data = self.data.lock().await;
        if force_refresh
            || data
                .access_exp_ms
                .is_some_and(|exp| expiring(exp, now_ms()))
        {
            self.refresh(&mut data, force_refresh).await?;
        }
        Ok(data.access_jwt.clone())
    }

    /// Refreshes under the account's refresh lock. Another process may have refreshed since
    /// this one loaded the session: the stored session is re-read first and adopted if it is
    /// strictly newer (an older stored copy, left by a failed save, is never adopted).
    async fn refresh(&self, data: &mut AppPasswordData, forced: bool) -> Result<(), HostError> {
        let _lock = FileLock::acquire_async(self.lock_file.clone())
            .await
            .map_err(|e| HostError::Storage(format!("refresh lock: {e}")))?;
        if let Ok(Some(StoredSession::AppPassword(stored))) =
            self.store.load(self.did.as_str()).await
            && stored.refresh_jwt != data.refresh_jwt
            && stored.saved_ms > data.saved_ms
        {
            *data = stored;
            let fresh = !data.access_exp_ms.is_some_and(|e| expiring(e, now_ms()));
            if fresh || forced {
                tracing::debug!(did = %self.did, "adopted a session refreshed by another process");
                return Ok(());
            }
        }
        let url = user_pds_url(&self.pds, "/xrpc/com.atproto.server.refreshSession")?;
        let req = NetRequest {
            method: http::Method::POST,
            url,
            headers: vec![(
                "authorization".into(),
                format!("Bearer {}", data.refresh_jwt.expose()),
            )],
            body: None,
            max_bytes: limits::SMALL_RESPONSE_MAX_BYTES,
            timeout_ms: limits::REQUEST_TIMEOUT_MS,
            max_redirects: 0,
        };
        let resp = self.net.execute(req).await?;
        if matches!(resp.status, 400 | 401) {
            tracing::debug!(did = %self.did, status = resp.status, "refreshSession refused");
            return Err(HostError::AuthRequired(self.did.to_string()));
        }
        if !(200..300).contains(&resp.status) {
            return Err(HostError::HttpStatus(resp.status, error_name(&resp)));
        }
        let session: Session = serde_json::from_slice(&resp.body)
            .map_err(|e| HostError::Other(format!("refreshSession: {e}")))?;
        if session.did != self.did.as_str() {
            return Err(HostError::Blocked(
                "refreshSession returned another account".into(),
            ));
        }
        *data = AppPasswordData {
            saved_ms: next_saved_ms(data.saved_ms),
            ..session_data(&session, &data.pds, data.created_ms)
        };
        self.store
            .save(&StoredSession::AppPassword(data.clone()))
            .await
            .map_err(|e| HostError::Storage(e.message))?;
        tracing::debug!(did = %self.did, "refreshed the app-password session");
        Ok(())
    }

    async fn send_inner(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        super::allowlist::check(&self.did, &req)?;
        let mut token = self.access_token(false).await?;
        let mut refreshed = false;
        loop {
            let mut nr = NetRequest::for_user_pds(&self.pds, req.clone())?;
            nr.headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
            nr.headers
                .push(("authorization".into(), format!("Bearer {}", token.expose())));
            let resp = self.net.execute(nr).await?;
            if !refreshed && token_expired(&resp) {
                refreshed = true;
                token = self.access_token(true).await?;
                continue;
            }
            return Ok(resp);
        }
    }
}

impl AuthSession for AppPasswordSession {
    fn account(&self) -> &Did {
        &self.did
    }

    fn send(&self, req: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HostError>> {
        Box::pin(self.send_inner(req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_password_sources() {
        assert!(AppPassword::new("abcd-efgh-ijkl-mnop").is_ok());
        assert!(AppPassword::new("abcd-efgh-ijkl-mnop\n").is_ok());
        assert!(AppPassword::new("abcd-efgh-ijkl-mnop\r\n").is_ok());
        let e = AppPassword::new("correct horse battery staple").unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidInput);
        assert!(!e.message.contains("horse"));
        assert!(AppPassword::new("abcd-efgh-ijkl-mnop\n\n").is_err());
        assert!(AppPassword::new(" abcd-efgh-ijkl-mnop").is_err());
        let p = AppPassword::from_reader(&b"wxyz-1234-abcd-5678\nrest"[..]).unwrap();
        assert_eq!(p.expose(), "wxyz-1234-abcd-5678");
        assert_eq!(format!("{p:?}"), "AppPassword(<redacted>)");
        assert!(AppPassword::from_reader(&b""[..]).is_err());
        assert!(AppPassword::from_reader(&b"hunter2\n"[..]).is_err());
    }

    #[test]
    fn jwt_expiry() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"scope":"com.atproto.access","exp":1700000000}"#);
        let jwt = format!("eyJhbGciOiJIUzI1NiJ9.{payload}.sig");
        assert_eq!(jwt_exp_ms(&jwt), Some(1_700_000_000_000));
        assert_eq!(jwt_exp_ms("not a jwt"), None);
        assert_eq!(jwt_exp_ms("a.!!!.c"), None);
    }
}
