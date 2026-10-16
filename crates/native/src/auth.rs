// SPDX-License-Identifier: AGPL-3.0-or-later
//! Authentication: app passwords and the OAuth facade (loopback, hosted native, no-browser)
//! (`docs/design/synthesis-v1.md` §6.7, `threat-model.md` §5.8).
//!
//! - [`app_password`]: `createSession` with an app password (never a main password, never from
//!   argv), [`app_password::AppPasswordSession`].
//! - [`oauth`] (feature `oauth`): PAR + PKCE + DPoP through `jacquard-oauth`,
//!   [`oauth::OAuthSession`].
//! - [`store`]: sessions in the OS keyring, or a 0600 file when there is none.
//! - [`allowlist`]: the only requests a session authenticates (AUTH-4).
//! - [`redact`]: [`redact::Secret`] and [`redact::redact_secrets`] for logs and `Debug`.
//!
//! Every login ends with the engine's login binding ([`Engine::add_account`], `sync.md` §3.6)
//! **before** the session is stored or used; [`restore_session`] registers a stored session
//! with the host only if it is still bound to the account's recorded PDS (AUTH-8).

pub mod allowlist;
pub mod app_password;
#[cfg(feature = "oauth")]
pub mod oauth;
pub mod redact;
pub mod store;

use std::sync::Arc;

use lessdugin_atp::syntax::Did;
use lessdugin_core::dto::AccountView;
use lessdugin_core::engine::{AccountInfo, Engine, EngineError, ErrorCode, LoginBinding};
use lessdugin_core::host::KvStore;
use url::Url;

pub use app_password::{APP_PASSWORD_ENV, AppPassword, AppPasswordSession};
pub use store::{Backend, KeyringStatus, SecretStore, StoredSession};

use crate::config::Paths;
use crate::host::NativeHost;

/// Tokens are refreshed this long before they expire.
pub const REFRESH_MARGIN_MS: i64 = 60_000;

/// Milliseconds since the Unix epoch (the native host's clock).
pub(crate) fn now_ms() -> i64 {
    // The native crates own the clock; the ban on clocks applies to the wasm-safe crates.
    #[allow(clippy::disallowed_methods)]
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// True if a token expiring at `exp_ms` should be refreshed now.
pub(crate) fn expiring(exp_ms: i64, now: i64) -> bool {
    exp_ms.saturating_sub(REFRESH_MARGIN_MS) <= now
}

/// The result of a successful login.
#[derive(Clone, Debug)]
pub struct LoginOutcome {
    /// The recorded account (now the active one).
    pub account: AccountView,
    /// The verified binding (PDS, handle, authorization-server pin).
    pub binding: LoginBinding,
    /// Where the session was stored.
    pub backend: Backend,
}

fn refresh_lock(paths: &Paths, did: &str) -> Result<std::path::PathBuf, EngineError> {
    paths
        .lock_file(&format!("refresh:{did}"))
        .ok_or_else(|| EngineError::internal("no refresh lock"))
}

/// App-password login (`lessdugin login --app-password`): resolves `input`, creates a session at
/// the DID document's PDS with `createSession`, checks the binding (the session's DID is the
/// resolved one, the PDS is the document's), then stores and registers the session.
pub async fn login_app_password<K: KvStore + 'static>(
    engine: &Engine<NativeHost<K>>,
    store: &SecretStore,
    paths: &Paths,
    input: &str,
    password: &AppPassword,
) -> Result<LoginOutcome, EngineError> {
    let resolved = engine.resolve_identity(input).await?;
    let ident = &resolved.identity;
    let pds = ident.pds.clone().ok_or_else(|| {
        EngineError::not_found(format!(
            "{} has no usable PDS: {}",
            ident.did,
            ident.pds_problem.as_deref().unwrap_or("unknown")
        ))
    })?;
    let pds_url = Url::parse(&pds).map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let net = engine.host().net().clone();
    let session =
        app_password::create_session(&net, &pds_url, ident.did.as_str(), password).await?;
    let info = AccountInfo::app_password(&session.did, &pds).with_intended(ident.did.as_str());
    let data = app_password::session_data(&session, &pds, now_ms());
    let bound = async {
        let binding = engine.bind_login(&info).await?;
        let account = engine.add_account(info).await?;
        Ok::<_, EngineError>((binding, account))
    }
    .await;
    let (binding, account) = match bound {
        Ok(b) => b,
        Err(e) => {
            // Never keep a session that failed the binding.
            let _ = app_password::delete_session(&net, &pds_url, &data.refresh_jwt).await;
            return Err(e);
        }
    };
    let backend = store
        .save(&StoredSession::AppPassword(data.clone()))
        .await?;
    let session =
        AppPasswordSession::new(data, net, store.clone(), refresh_lock(paths, &account.did)?)?;
    engine.host().set_session(Arc::new(session));
    Ok(LoginOutcome {
        account,
        binding,
        backend,
    })
}

/// Loads the stored session of `account` and registers it with the host. Fails with
/// `auth_required` if there is none, and refuses a session bound to another PDS than the one
/// recorded at login (`needs-relogin`, AUTH-8).
pub async fn restore_session<K: KvStore + 'static>(
    host: &NativeHost<K>,
    store: &SecretStore,
    paths: &Paths,
    account: &AccountView,
) -> Result<(), EngineError> {
    let did = Did::parse(&account.did).map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let Some(stored) = store.load(did.as_str()).await? else {
        return Err(EngineError::new(
            ErrorCode::AuthRequired,
            format!("no stored session for {did}; log in again"),
        )
        .with_detail("did", did.as_str()));
    };
    if stored.pds() != account.pds {
        return Err(EngineError::new(
            ErrorCode::AuthRequired,
            format!(
                "the stored session of {did} is bound to {}, not {}; log in again",
                stored.pds(),
                account.pds
            ),
        )
        .with_detail("kind", "needsRelogin"));
    }
    let net = host.net().clone();
    match stored {
        StoredSession::AppPassword(data) => {
            let s = AppPasswordSession::new(
                data,
                net,
                store.clone(),
                refresh_lock(paths, did.as_str())?,
            )?;
            host.set_session(Arc::new(s));
        }
        #[cfg(feature = "oauth")]
        StoredSession::Oauth(data) => {
            let s = oauth::OAuthSession::new(data, net, store.clone(), paths)?;
            host.set_session(Arc::new(s));
        }
        #[cfg(not(feature = "oauth"))]
        StoredSession::Oauth(_) => {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                "this build has no OAuth support; log in with --app-password",
            ));
        }
    }
    Ok(())
}

/// What `logout` did.
#[derive(Clone, Debug, Default)]
pub struct LogoutReport {
    /// The session was revoked at the server.
    pub revoked: bool,
    /// Why revocation failed, if it did (the local session is deleted anyway).
    pub revoke_error: Option<String>,
    /// A stored session was found.
    pub had_session: bool,
}

/// Revokes the stored session of `did` at its server (best effort) and deletes it from the
/// keyring and the fallback file. The account record is the engine's
/// ([`Engine::remove_account`]).
pub async fn logout_session<K: KvStore + 'static>(
    host: &NativeHost<K>,
    store: &SecretStore,
    did: &str,
) -> Result<LogoutReport, EngineError> {
    let mut report = LogoutReport::default();
    let stored = match store.load(did).await {
        Ok(s) => s,
        Err(e) if e.code == ErrorCode::AuthRequired => None,
        Err(e) => return Err(e),
    };
    if let Some(stored) = stored {
        report.had_session = true;
        let net = host.net();
        let result = match &stored {
            StoredSession::AppPassword(d) => match Url::parse(&d.pds) {
                Ok(pds) => app_password::delete_session(net, &pds, &d.refresh_jwt).await,
                Err(e) => Err(EngineError::invalid_input(e.to_string())),
            },
            #[cfg(feature = "oauth")]
            StoredSession::Oauth(d) => oauth::revoke(d, net).await,
            #[cfg(not(feature = "oauth"))]
            StoredSession::Oauth(_) => Err(EngineError::new(
                ErrorCode::Unsupported,
                "this build has no OAuth support",
            )),
        };
        match result {
            Ok(()) => report.revoked = true,
            Err(e) => report.revoke_error = Some(e.message),
        }
    }
    store.delete(did).await?;
    if let Ok(d) = Did::parse(did) {
        host.remove_session(&d);
    }
    Ok(report)
}
