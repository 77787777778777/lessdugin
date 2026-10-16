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
//! Every login starts with [`resolve_login_target`]: the input is resolved **now** (no cached
//! handle answer or DID document), a handle found only through a hint service (Slingshot) is
//! refused, and the PDS comes from a DID document fetched now — before any credential leaves the
//! machine. A [`LoginTarget`] can only be made by that function. When the input handle is not
//! the identity's handle verified both ways, [`LoginTarget::needs_confirmation`] is set and the
//! client must show the DID and ask before going on (`threat-model.md` F25).
//!
//! Every login then checks the engine's login binding ([`Engine::bind_login`], `sync.md` §3.6)
//! **before** the session is stored or used, stores the session, and only then records the
//! account ([`Engine::record_account`]), so a failed save never leaves an account without a
//! session. [`restore_session`] re-checks the account against its DID document
//! ([`Engine::refresh_account`]) and registers a stored session with the host only if it is
//! still bound to the account's PDS (AUTH-8).

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
use lessdugin_core::host::{Host, KvStore};
use lessdugin_core::identity::{HandleResolution, IdentityLevel, ResolutionMethod};
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

/// Proof that a [`LoginTarget`] came from [`resolve_login_target`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Checked;

/// The account a login is for, resolved without the cache before any credential is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginTarget {
    /// The input as given.
    pub input: String,
    /// The input handle (normalized), when the input was a handle.
    pub input_handle: Option<String>,
    /// The DID.
    pub did: Did,
    /// The PDS, from a DID document fetched now.
    pub pds: String,
    /// The identity's handle, verified both ways, if any.
    pub handle: Option<String>,
    /// The claimed handle of the DID document (unverified; for messages only).
    pub claimed_handle: Option<String>,
    _checked: Checked,
}

impl LoginTarget {
    /// True when the input was a handle that is not the identity's handle verified both ways
    /// (the handle → DID answer is one-directional): the client must show the DID and ask
    /// before sending credentials, or refuse when it cannot ask.
    pub fn needs_confirmation(&self) -> bool {
        self.input_handle.is_some() && self.input_handle != self.handle
    }

    /// The name to show for the account: `@handle` when verified both ways, else the DID.
    pub fn display_name(&self) -> String {
        match &self.handle {
            Some(h) => format!("@{h}"),
            None => self.did.to_string(),
        }
    }
}

/// Resolves the login input (a handle or DID) for a login (`threat-model.md` F25): the handle is
/// looked up and the DID document fetched now; a handle found only through a hint (Slingshot)
/// is refused (`blocked`, `detail.kind = "hintResolution"`: log in with the DID instead), and so
/// is a hint-only identity or one without a usable PDS.
pub async fn resolve_login_target<H: Host>(
    engine: &Engine<H>,
    input: &str,
) -> Result<LoginTarget, EngineError> {
    let resolved = engine.resolve_identity_fresh(input).await?;
    if let Some(HandleResolution::Did {
        method: ResolutionMethod::Hint,
        did,
    }) = &resolved.resolution
    {
        return Err(EngineError::blocked(format!(
            "the handle {} was found only through a hint service, which cannot be trusted with \
             a login; log in with the account's DID instead ({did}, if that is your account)",
            resolved.input_handle.as_deref().unwrap_or(input)
        ))
        .with_detail("kind", "hintResolution")
        .with_detail("did", did.as_str()));
    }
    let ident = resolved.identity;
    if ident.level != IdentityLevel::Doc {
        return Err(EngineError::blocked(format!(
            "only a hint identity is available for {}; login needs its DID document",
            ident.did
        )));
    }
    let pds = ident.pds.clone().ok_or_else(|| {
        EngineError::not_found(format!(
            "{} has no usable PDS: {}",
            ident.did,
            ident.pds_problem.as_deref().unwrap_or("unknown")
        ))
    })?;
    Ok(LoginTarget {
        input: resolved.input,
        input_handle: resolved.input_handle,
        handle: ident.handle().map(str::to_owned),
        claimed_handle: ident.claimed_handle.clone(),
        did: ident.did,
        pds,
        _checked: Checked,
    })
}

/// App-password login (`lessdugin login --app-password`): creates a session at the target's PDS
/// with `createSession`, checks the binding (the session's DID is the target's, the PDS is the
/// one in a fresh DID document), stores the session, records the account and registers the
/// session. Any failure after `createSession` deletes the session at the server.
pub async fn login_app_password<K: KvStore + 'static>(
    engine: &Engine<NativeHost<K>>,
    store: &SecretStore,
    paths: &Paths,
    target: &LoginTarget,
    password: &AppPassword,
) -> Result<LoginOutcome, EngineError> {
    let pds_url = Url::parse(&target.pds).map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let net = engine.host().net().clone();
    let session =
        app_password::create_session(&net, &pds_url, target.did.as_str(), password).await?;
    let info =
        AccountInfo::app_password(&session.did, &target.pds).with_intended(target.did.as_str());
    let data = app_password::session_data(&session, &target.pds, now_ms());
    let done = async {
        let binding = engine.bind_login(&info).await?;
        let backend = store
            .save(&StoredSession::AppPassword(data.clone()))
            .await?;
        let account = match engine.record_account(&info, &binding).await {
            Ok(a) => a,
            Err(e) => {
                let _ = store.delete(&session.did).await;
                return Err(e);
            }
        };
        Ok::<_, EngineError>((binding, account, backend))
    }
    .await;
    let (binding, account, backend) = match done {
        Ok(b) => b,
        Err(e) => {
            // Never keep a session that failed the binding or could not be stored.
            let _ = app_password::delete_session(&net, &pds_url, &data.refresh_jwt).await;
            return Err(e);
        }
    };
    let session =
        AppPasswordSession::new(data, net, store.clone(), refresh_lock(paths, &account.did)?)?;
    engine.host().set_session(Arc::new(session));
    Ok(LoginOutcome {
        account,
        binding,
        backend,
    })
}

/// Loads the stored session of `account` and registers it with the host; returns the account,
/// re-checked against its DID document ([`Engine::refresh_account`]: the recorded handle follows
/// the identity). Fails with `auth_required` if there is no stored session, and with
/// `auth_required` (`detail.kind = "needsRelogin"`) if the DID document now names another PDS
/// (migration) or the stored session is bound to another PDS than the one recorded at login
/// (AUTH-8). When the identity cannot be resolved (offline), the recorded account is used: its
/// session only ever talks to the PDS bound at login.
pub async fn restore_session<K: KvStore + 'static>(
    engine: &Engine<NativeHost<K>>,
    store: &SecretStore,
    paths: &Paths,
    account: &AccountView,
) -> Result<AccountView, EngineError> {
    let host = engine.host();
    let did = Did::parse(&account.did).map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let account = match engine.refresh_account(did.as_str()).await {
        Ok(a) => a,
        Err(e) if e.code == ErrorCode::AuthRequired => {
            host.remove_session(&did);
            return Err(e);
        }
        Err(e) => {
            tracing::debug!(error = %e.message, "could not re-check the account's identity");
            account.clone()
        }
    };
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
    Ok(account)
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
    /// The stored session could not be read safely (an insecure or foreign session file): it
    /// was deleted without revocation. Revoke the session at the PDS if it may have leaked.
    pub unreadable: Option<String>,
}

/// Revokes the stored session of `did` at its server (best effort) and deletes it from the
/// keyring and the fallback file. A session that cannot be read safely (a file with group or
/// world permissions, a symlink, another account's session) is deleted without revocation and
/// reported in [`LogoutReport::unreadable`]: logging out is exactly what one wants then. The
/// account record is the engine's ([`Engine::remove_account`]).
pub async fn logout_session<K: KvStore + 'static>(
    host: &NativeHost<K>,
    store: &SecretStore,
    did: &str,
) -> Result<LogoutReport, EngineError> {
    let mut report = LogoutReport::default();
    let stored = match store.load(did).await {
        Ok(s) => s,
        Err(e) if e.code == ErrorCode::AuthRequired => None,
        Err(e) if e.code == ErrorCode::Blocked => {
            report.unreadable = Some(e.message);
            None
        }
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
