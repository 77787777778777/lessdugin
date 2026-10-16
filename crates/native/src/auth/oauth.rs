// SPDX-License-Identifier: AGPL-3.0-or-later
//! OAuth for native clients, a facade over `jacquard-oauth` =0.12.2
//! (`docs/design/synthesis-v1.md` §6.7, `data-model.md` §10, `threat-model.md` AUTH-1, AUTH-5,
//! AUTH-7, AUTH-8, AUTH-10).
//!
//! The flow, with what comes from jacquard and what lessdugin adds:
//!
//! 1. [`begin`]: the identity (DID, PDS) comes from `lessdugin-core`; the PDS's
//!    `oauth-protected-resource` names exactly one authorization server, whose metadata is
//!    fetched with jacquard's `resolve_*` helpers through `Bridge` (SSRF guard).
//! 2. PAR with jacquard's `oauth_request` (DPoP with `use_dpop_nonce` retry, PKCE S256 and
//!    `state` from jacquard's generators), but with lessdugin's own parameters: `response_mode=
//!    query` (jacquard's `par()` sends none), the loopback `client_id` form of `data-model.md`
//!    §10.4 (jacquard builds `http://localhost/?…`), the scope candidates of [`scope`], the
//!    hosted client metadata after `invalid_client`.
//! 3. The redirect arrives at the loopback listener, or is pasted back (`--no-browser`);
//!    `state` and `iss` are checked (module `callback`).
//! 4. [`PendingAuthorization::exchange`]: the code and the PKCE verifier go to the token
//!    endpoint (jacquard's `oauth_request`). jacquard's `exchange_code` is not used: its
//!    issuer check resolves the `sub` with jacquard's own resolver. Instead
//!    [`OAuthGrant::finish`] runs the login binding of `sync.md` §3.6 in the engine (fresh DID
//!    document of `sub`, its PDS's protected-resource metadata must list `iss`, TOFU pin of the
//!    issuer) **before** the tokens are stored or used.
//! 5. [`OAuthSession`] sends `UserPds` requests with `Authorization: DPoP` and a DPoP proof
//!    (`htu` without query, `ath`, the PDS's nonce, one retry on `use_dpop_nonce`), refreshes
//!    under the account's refresh lock, and never follows redirects.

mod bridge;
mod callback;
pub mod scope;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures::future::BoxFuture;
use jacquard_common::SmolStr;
use jacquard_oauth::dpop::build_dpop_proof;
use jacquard_oauth::request::{
    OAuthMetadata, OAuthRequest, RequestError, RequestErrorKind, oauth_request,
};
use jacquard_oauth::resolver::{resolve_authorization_server, resolve_protected_resource_info};
use jacquard_oauth::session::DpopReqData;
use jacquard_oauth::types::{
    AuthorizationCodeChallengeMethod, AuthorizationResponseMode, AuthorizationResponseType,
    OAuthAuthorizationServerMetadata, OAuthClientMetadata, OAuthParResponse, OAuthTokenResponse,
    OAuthTokenType, ParParameters, RefreshRequestParameters, RevocationRequestParameters,
    TokenGrantType, TokenRequestParameters,
};
use jacquard_oauth::utils::{generate_dpop_key, generate_nonce, generate_pkce};
use lessdugin_atp::syntax::Did;
use lessdugin_core::engine::{AccountInfo, Engine, EngineError, ErrorCode};
use lessdugin_core::host::{HostError, HttpRequest, HttpResponse, KvStore};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::Url;

use self::bridge::{Bridge, BridgeError};
pub use self::callback::AuthorizationCode;
use self::callback::Expected;
use super::redact::{Secret, SecretJson, redact_secrets};
use super::store::{OAuthData, SESSION_VERSION, SecretStore, StoredSession};
use super::{LoginOutcome, expiring, now_ms};
use crate::config::{FileLock, Paths};
use crate::host::{AuthSession, NativeHost, NetRequest};

/// Which client metadata to use (`--oauth-client`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClientKind {
    /// `http://localhost?redirect_uri=…&scope=…` (no metadata document).
    #[default]
    Loopback,
    /// `https://lessdugin.7778777.online/oauth/cli-client-metadata.json`.
    Hosted,
}

/// Options of an OAuth login.
#[derive(Clone, Debug)]
pub struct OAuthOptions {
    /// The client metadata to start with.
    pub client: ClientKind,
    /// Switch to the hosted metadata after `invalid_client` (loopback only).
    pub hosted_fallback: bool,
    /// Listen on the loopback port for the redirect. `false` for `--no-browser`: the port is
    /// only reserved for the redirect URI, and the user pastes the failed redirect back.
    pub listen: bool,
}

impl Default for OAuthOptions {
    fn default() -> Self {
        Self {
            client: ClientKind::Loopback,
            hosted_fallback: true,
            listen: true,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Errors

/// Maps a jacquard request error. Host errors (network, SSRF guard) keep their engine codes;
/// OAuth error bodies become messages with `detail.error`. Messages are redacted.
fn request_error(e: RequestError, what: &str) -> EngineError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(s) = source {
        if let Some(BridgeError(h)) = s.downcast_ref::<BridgeError>() {
            return EngineError::from(h.clone()).context(what);
        }
        source = s.source();
    }
    match e.kind() {
        RequestErrorKind::HttpStatusWithBody { status, body } => {
            let err = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
            let desc = body
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let code = match err {
                "invalid_grant" | "access_denied" | "invalid_token" => ErrorCode::AuthRequired,
                "invalid_client" | "unauthorized_client" | "invalid_client_metadata" => {
                    ErrorCode::Blocked
                }
                _ if status.is_server_error() => ErrorCode::Network,
                _ => ErrorCode::InvalidInput,
            };
            EngineError::new(
                code,
                redact_secrets(&format!("{what}: {err}: {desc} (HTTP {})", status.as_u16())),
            )
            .with_detail("error", err)
            .with_detail("status", status.as_u16())
        }
        RequestErrorKind::HttpStatus(s) if s.is_server_error() => {
            EngineError::new(ErrorCode::Network, format!("{what}: HTTP {}", s.as_u16()))
        }
        _ => EngineError::new(ErrorCode::Blocked, redact_secrets(&format!("{what}: {e}"))),
    }
}

fn oauth_error_name(e: &RequestError) -> Option<&str> {
    match e.kind() {
        RequestErrorKind::HttpStatusWithBody { body, .. } => {
            body.get("error").and_then(|v| v.as_str())
        }
        _ => None,
    }
}

fn resolver_error(e: jacquard_oauth::resolver::ResolverError, what: &str) -> EngineError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
    while let Some(s) = source {
        if let Some(BridgeError(h)) = s.downcast_ref::<BridgeError>() {
            return EngineError::from(h.clone()).context(what);
        }
        source = s.source();
    }
    EngineError::blocked(redact_secrets(&format!("{what}: {e}")))
}

// ---------------------------------------------------------------------------------------------
// Client and server metadata

fn client_metadata(client_id: &str, redirect_uri: &str, scope: &str) -> OAuthClientMetadata {
    OAuthClientMetadata {
        client_id: client_id.into(),
        client_uri: None,
        redirect_uris: vec![redirect_uri.into()],
        scope: (!scope.is_empty()).then(|| scope.into()),
        application_type: Some("native".into()),
        grant_types: Some(vec!["authorization_code".into(), "refresh_token".into()]),
        token_endpoint_auth_method: Some("none".into()),
        response_types: vec!["code".into()],
        dpop_bound_access_tokens: Some(true),
        jwks_uri: None,
        jwks: None,
        token_endpoint_auth_signing_alg: None,
        client_name: None,
        logo_uri: None,
        tos_uri: None,
        privacy_policy_uri: None,
    }
}

/// The token and revocation endpoints bound at login, as server metadata for refreshes.
fn bound_server(data: &OAuthData) -> OAuthAuthorizationServerMetadata {
    OAuthAuthorizationServerMetadata {
        issuer: data.issuer.as_str().into(),
        token_endpoint: data.token_endpoint.as_str().into(),
        revocation_endpoint: data.revocation_endpoint.as_deref().map(SmolStr::from),
        token_endpoint_auth_methods_supported: Some(vec!["none".into()]),
        ..Default::default()
    }
}

fn host_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_owned();
            Some(match u.port() {
                Some(p) => format!("{host}:{p}"),
                None => host,
            })
        })
        .unwrap_or_else(|| url.to_owned())
}

// ---------------------------------------------------------------------------------------------
// Starting a login

/// A login waiting for the browser: show [`PendingAuthorization::authorize_url`], then get the
/// code from the listener or from a pasted redirect, then [`PendingAuthorization::exchange`].
pub struct PendingAuthorization {
    authorize_url: Url,
    issuer: String,
    redirect_uri: Url,
    client_id: String,
    client_kind: ClientKind,
    scope: String,
    intended: Did,
    pds: String,
    state: Secret,
    verifier: Secret,
    dpop: DpopReqData,
    server: OAuthAuthorizationServerMetadata,
    listener: Option<TcpListener>,
    bridge: Bridge,
}

impl std::fmt::Debug for PendingAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAuthorization")
            .field("issuer", &self.issuer)
            .field("redirect_uri", &self.redirect_uri.as_str())
            .field("client_kind", &self.client_kind)
            .field("intended", &self.intended)
            .finish_non_exhaustive()
    }
}

enum ParFailure {
    Scope(EngineError),
    Client(EngineError),
    Other(EngineError),
}

#[allow(clippy::too_many_arguments)]
async fn push_request(
    bridge: &Bridge,
    server: &OAuthAuthorizationServerMetadata,
    dpop: &mut DpopReqData,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    challenge: &str,
    login_hint: &str,
) -> Result<OAuthParResponse, ParFailure> {
    let metadata: OAuthMetadata = OAuthMetadata {
        server_metadata: server.clone(),
        client_metadata: client_metadata(client_id, redirect_uri, scope),
        keyset: None,
    };
    let params = ParParameters {
        response_type: AuthorizationResponseType::Code,
        redirect_uri,
        state,
        scope: Some(scope),
        response_mode: Some(AuthorizationResponseMode::Query),
        code_challenge: challenge,
        code_challenge_method: AuthorizationCodeChallengeMethod::S256,
        login_hint: Some(login_hint),
        prompt: None,
    };
    oauth_request::<OAuthParResponse, Bridge, DpopReqData, SmolStr>(
        bridge,
        dpop,
        OAuthRequest::PushedAuthorizationRequest(params),
        &metadata,
    )
    .await
    .map_err(|e| {
        let name = oauth_error_name(&e).map(str::to_owned);
        let err = request_error(e, "pushed authorization request");
        match name.as_deref() {
            Some("invalid_scope") => ParFailure::Scope(err),
            Some("invalid_client" | "invalid_client_metadata" | "unauthorized_client") => {
                ParFailure::Client(err)
            }
            _ => ParFailure::Other(err),
        }
    })
}

/// Starts an OAuth login for `input` (a handle or DID): resolves the account and its
/// authorization server, reserves the loopback port and pushes the authorization request.
pub async fn begin<K: KvStore + 'static>(
    engine: &Engine<NativeHost<K>>,
    input: &str,
    opts: &OAuthOptions,
) -> Result<PendingAuthorization, EngineError> {
    let resolved = engine.resolve_identity(input).await?;
    let ident = &resolved.identity;
    let pds = ident.pds.clone().ok_or_else(|| {
        EngineError::not_found(format!(
            "{} has no usable PDS: {}",
            ident.did,
            ident.pds_problem.as_deref().unwrap_or("unknown")
        ))
    })?;
    let bridge = Bridge::new(engine.host().net().clone());

    let prm = resolve_protected_resource_info(&bridge, &pds)
        .await
        .map_err(|e| resolver_error(e, "oauth-protected-resource"))?;
    let servers = prm.authorization_servers.unwrap_or_default();
    let issuer = match servers.as_slice() {
        [one] => one.to_string(),
        [] => {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                format!("the PDS {pds} names no OAuth authorization server; use --app-password"),
            ));
        }
        _ => {
            return Err(EngineError::blocked(format!(
                "the PDS {pds} names more than one authorization server"
            )));
        }
    };
    let mut server = resolve_authorization_server(&bridge, &issuer)
        .await
        .map_err(|e| resolver_error(e, "oauth-authorization-server"))?;
    if server.pushed_authorization_request_endpoint.is_none() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            format!("the authorization server {issuer} does not support PAR"),
        ));
    }
    let authorize_base = Url::parse(&server.authorization_endpoint)
        .map_err(|e| EngineError::blocked(format!("authorization endpoint: {e}")))?;
    bridge
        .net()
        .policy()
        .check_url(&authorize_base)
        .map_err(|e| EngineError::from(e).context("authorization endpoint"))?;

    let std_listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| EngineError::internal(format!("cannot listen on 127.0.0.1: {e}")))?;
    let port = std_listener
        .local_addr()
        .map_err(|e| EngineError::internal(e.to_string()))?
        .port();
    let listener = if opts.listen {
        std_listener
            .set_nonblocking(true)
            .map_err(|e| EngineError::internal(e.to_string()))?;
        Some(
            TcpListener::from_std(std_listener)
                .map_err(|e| EngineError::internal(e.to_string()))?,
        )
    } else {
        drop(std_listener);
        None
    };
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let dpop_key = generate_dpop_key(&mut server).ok_or_else(|| {
        EngineError::new(
            ErrorCode::Unsupported,
            "the authorization server supports no DPoP algorithm lessdugin has (ES256)",
        )
    })?;
    let mut dpop = DpopReqData {
        dpop_key,
        dpop_authserver_nonce: None,
    };
    let login_hint = resolved
        .input_handle
        .clone()
        .unwrap_or_else(|| ident.did.to_string());
    let clients = match (opts.client, opts.hosted_fallback) {
        (ClientKind::Loopback, true) => vec![ClientKind::Loopback, ClientKind::Hosted],
        (kind, _) => vec![kind],
    };
    let scopes = scope::scope_candidates();
    let mut last_err = None;
    'clients: for client_kind in clients {
        let client_id = match client_kind {
            ClientKind::Loopback => scope::loopback_client_id(&redirect_uri),
            ClientKind::Hosted => scope::HOSTED_CLIENT_ID.to_owned(),
        };
        for scope in &scopes {
            let state = generate_nonce();
            let (challenge, verifier) = generate_pkce();
            let result = push_request(
                &bridge,
                &server,
                &mut dpop,
                &client_id,
                &redirect_uri,
                scope,
                &state,
                &challenge,
                &login_hint,
            )
            .await;
            match result {
                Ok(par) => {
                    let mut authorize_url = authorize_base.clone();
                    authorize_url
                        .query_pairs_mut()
                        .append_pair("client_id", &client_id)
                        .append_pair("request_uri", &par.request_uri);
                    tracing::debug!(
                        client = ?client_kind,
                        issuer = %issuer,
                        "pushed authorization request accepted"
                    );
                    return Ok(PendingAuthorization {
                        authorize_url,
                        issuer,
                        redirect_uri: Url::parse(&redirect_uri)
                            .map_err(|e| EngineError::internal(e.to_string()))?,
                        client_id,
                        client_kind,
                        scope: scope.clone(),
                        intended: ident.did.clone(),
                        pds,
                        state: Secret::new(state.to_string()),
                        verifier: Secret::new(verifier.to_string()),
                        dpop,
                        server,
                        listener,
                        bridge,
                    });
                }
                Err(ParFailure::Scope(e)) => {
                    tracing::debug!(error = %e, "scope refused at PAR; trying the next form");
                    last_err = Some(e);
                }
                Err(ParFailure::Client(e)) => {
                    tracing::debug!(error = %e, "client refused at PAR");
                    last_err = Some(e);
                    continue 'clients;
                }
                Err(ParFailure::Other(e)) => return Err(e),
            }
        }
        // Every scope form was refused for this client: login fails (AUTH-1).
        break;
    }
    Err(last_err.unwrap_or_else(|| EngineError::internal("no OAuth client configuration")))
}

impl PendingAuthorization {
    /// The URL to open in a browser.
    pub fn authorize_url(&self) -> &Url {
        &self.authorize_url
    }

    /// The authorization server's issuer.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The authorization server's host (shown before the browser opens).
    pub fn authorization_server_host(&self) -> String {
        host_of(&self.issuer)
    }

    /// The redirect URI (`http://127.0.0.1:<port>/callback`).
    pub fn redirect_uri(&self) -> &Url {
        &self.redirect_uri
    }

    /// The client metadata in use.
    pub fn client_kind(&self) -> ClientKind {
        self.client_kind
    }

    /// The scope that was requested.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// The account the user asked for.
    pub fn intended(&self) -> &Did {
        &self.intended
    }

    /// True if the loopback listener is waiting for the redirect.
    pub fn is_listening(&self) -> bool {
        self.listener.is_some()
    }

    fn expected(&self) -> Expected<'_> {
        Expected {
            state: self.state.expose(),
            issuer: &self.issuer,
            iss_required: self.server.authorization_response_iss_parameter_supported == Some(true),
        }
    }

    /// Waits for the browser's redirect on the loopback listener.
    pub async fn wait_for_callback(
        &self,
        timeout: Duration,
    ) -> Result<AuthorizationCode, EngineError> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| EngineError::internal("this login does not listen for the redirect"))?;
        callback::accept(listener, &self.expected(), timeout).await
    }

    /// Checks a pasted redirect URL (`--no-browser`).
    pub fn parse_pasted(&self, pasted: &str) -> Result<AuthorizationCode, EngineError> {
        callback::parse_pasted(pasted, &self.redirect_uri, &self.expected())
    }

    /// Exchanges the code (with the PKCE verifier) for DPoP-bound tokens. The result is not
    /// trusted until [`OAuthGrant::finish`] has checked the login binding.
    pub async fn exchange(mut self, code: AuthorizationCode) -> Result<OAuthGrant, EngineError> {
        let metadata: OAuthMetadata = OAuthMetadata {
            server_metadata: self.server.clone(),
            client_metadata: client_metadata(
                &self.client_id,
                self.redirect_uri.as_str(),
                &self.scope,
            ),
            keyset: None,
        };
        let params = TokenRequestParameters {
            grant_type: TokenGrantType::AuthorizationCode,
            code: code.0.expose(),
            redirect_uri: self.redirect_uri.as_str(),
            code_verifier: self.verifier.expose(),
        };
        let token = oauth_request::<OAuthTokenResponse, Bridge, DpopReqData, SmolStr>(
            &self.bridge,
            &mut self.dpop,
            OAuthRequest::Token(params),
            &metadata,
        )
        .await
        .map_err(|e| request_error(e, "token request"))?;
        let sub = token
            .sub
            .as_deref()
            .ok_or_else(|| EngineError::blocked("the token response has no sub"))?;
        if token.token_type != OAuthTokenType::DPoP {
            return Err(EngineError::blocked(
                "the authorization server issued a token that is not DPoP-bound",
            ));
        }
        if let Some(granted) = &token.scope
            && !granted.split(' ').any(|s| s == "atproto")
        {
            return Err(EngineError::blocked(
                "the granted scope does not include atproto",
            ));
        }
        let now = now_ms();
        let dpop = serde_json::to_value(&self.dpop)
            .map_err(|e| EngineError::internal(format!("DPoP key: {e}")))?;
        let data = OAuthData {
            v: SESSION_VERSION,
            did: sub.to_owned(),
            pds: self.pds.clone(),
            issuer: self.issuer.clone(),
            token_endpoint: self.server.token_endpoint.to_string(),
            revocation_endpoint: self
                .server
                .revocation_endpoint
                .as_ref()
                .map(|s| s.to_string()),
            client_id: self.client_id.clone(),
            redirect_uri: self.redirect_uri.to_string(),
            scope: token.scope.as_ref().map(|s| s.to_string()),
            access_token: Secret::new(token.access_token.to_string()),
            refresh_token: token
                .refresh_token
                .as_ref()
                .map(|t| Secret::new(t.to_string())),
            expires_at_ms: token
                .expires_in
                .and_then(|s| s.checked_mul(1000))
                .map(|ms| now.saturating_add(ms)),
            dpop: SecretJson(dpop),
            dpop_host_nonce: None,
            created_ms: now,
        };
        Ok(OAuthGrant {
            data,
            intended: self.intended,
            bridge: self.bridge,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Finishing a login

/// Tokens from a code exchange, not yet bound: [`OAuthGrant::finish`] checks the login binding
/// and stores the session; [`OAuthGrant::abandon`] revokes it.
pub struct OAuthGrant {
    data: OAuthData,
    intended: Did,
    bridge: Bridge,
}

impl std::fmt::Debug for OAuthGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthGrant")
            .field("did", &self.data.did)
            .field("issuer", &self.data.issuer)
            .finish_non_exhaustive()
    }
}

impl OAuthGrant {
    /// The token's `sub`.
    pub fn did(&self) -> &str {
        &self.data.did
    }

    /// The authorization server's issuer.
    pub fn issuer(&self) -> &str {
        &self.data.issuer
    }

    /// The granted scope.
    pub fn scope(&self) -> Option<&str> {
        self.data.scope.as_deref()
    }

    /// Checks the login binding (`sync.md` §3.6; with `confirm_issuer_change` after the user
    /// confirmed a changed authorization server), records the account, stores the session and
    /// registers it with the host. A `conflict` with `detail.kind = "issuerChanged"` can be
    /// retried with confirmation; the grant stays usable.
    pub async fn finish<K: KvStore + 'static>(
        &self,
        engine: &Engine<NativeHost<K>>,
        store: &SecretStore,
        paths: &Paths,
        confirm_issuer_change: bool,
    ) -> Result<LoginOutcome, EngineError> {
        let mut info = AccountInfo::oauth(&self.data.did, &self.data.pds, &self.data.issuer)
            .with_intended(self.intended.as_str());
        if confirm_issuer_change {
            info = info.confirming_issuer_change();
        }
        let binding = engine.bind_login(&info).await?;
        let account = engine.add_account(info).await?;
        let mut data = self.data.clone();
        data.pds = binding.pds.clone();
        let backend = store.save(&StoredSession::Oauth(data.clone())).await?;
        let session = OAuthSession::new(data, self.bridge.net().clone(), store.clone(), paths)?;
        engine.host().set_session(Arc::new(session));
        Ok(LoginOutcome {
            account,
            binding,
            backend,
        })
    }

    /// Revokes the tokens (after a refused binding or a declined confirmation).
    pub async fn abandon(self) {
        if let Err(e) = revoke(&self.data, self.bridge.net()).await {
            tracing::debug!(error = %e, "revoking an abandoned grant failed");
        }
    }
}

/// Revokes an OAuth session at its authorization server (`logout`, AUTH-11). Revoking the
/// refresh token ends the whole session.
pub async fn revoke(data: &OAuthData, net: &crate::host::Net) -> Result<(), EngineError> {
    if data.revocation_endpoint.is_none() {
        return Ok(());
    }
    let bridge = Bridge::new(net.clone());
    let mut dpop: DpopReqData = serde_json::from_value(data.dpop.0.clone())
        .map_err(|e| EngineError::internal(format!("stored DPoP key: {e}")))?;
    let metadata: OAuthMetadata = OAuthMetadata {
        server_metadata: bound_server(data),
        client_metadata: client_metadata(&data.client_id, &data.redirect_uri, ""),
        keyset: None,
    };
    let token = data.refresh_token.as_ref().unwrap_or(&data.access_token);
    let result = oauth_request::<(), Bridge, DpopReqData, SmolStr>(
        &bridge,
        &mut dpop,
        OAuthRequest::Revocation(RevocationRequestParameters {
            token: token.expose(),
        }),
        &metadata,
    )
    .await;
    match result {
        Ok(()) => Ok(()),
        // Servers answer 200 or 204.
        Err(e) if matches!(e.kind(), RequestErrorKind::HttpStatus(s) if s.is_success()) => Ok(()),
        Err(e) => Err(request_error(e, "token revocation")),
    }
}

// ---------------------------------------------------------------------------------------------
// The session

struct OAuthState {
    data: OAuthData,
    dpop: DpopReqData,
}

/// An OAuth session for one account: DPoP-bound requests to the PDS bound at login.
pub struct OAuthSession {
    did: Did,
    pds: Url,
    bridge: Bridge,
    store: SecretStore,
    lock_file: PathBuf,
    state: Mutex<OAuthState>,
}

impl std::fmt::Debug for OAuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthSession")
            .field("did", &self.did)
            .field("pds", &self.pds.as_str())
            .finish_non_exhaustive()
    }
}

/// `ath`: base64url(SHA-256(access token)).
fn access_token_hash(token: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

/// `htu`: the request URI without query and fragment (RFC 9449 §4.2).
fn htu(url: &Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}

/// The `error` of a `WWW-Authenticate: DPoP …` challenge, or of an XRPC error body.
fn auth_error(resp: &HttpResponse) -> Option<String> {
    if let Some(h) = resp.header("www-authenticate")
        && let Some(i) = h.find("error=\"")
    {
        let rest = &h[i + 7..];
        return rest.split('"').next().map(str::to_owned);
    }
    let v: serde_json::Value = serde_json::from_slice(&resp.body).ok()?;
    v.get("error")?.as_str().map(str::to_owned)
}

fn parse_dpop(data: &OAuthData) -> Result<DpopReqData, EngineError> {
    serde_json::from_value(data.dpop.0.clone())
        .map_err(|e| EngineError::internal(format!("stored DPoP key: {e}")))
}

impl OAuthSession {
    /// A session from stored data, sending through `net`, refreshing under
    /// `paths.lock_file("refresh:<did>")`.
    pub fn new(
        data: OAuthData,
        net: crate::host::Net,
        store: SecretStore,
        paths: &Paths,
    ) -> Result<Self, EngineError> {
        let did = Did::parse(&data.did).map_err(|e| EngineError::invalid_input(e.to_string()))?;
        let pds = Url::parse(&data.pds)
            .map_err(|e| EngineError::invalid_input(format!("stored PDS: {e}")))?;
        let lock_file = paths
            .lock_file(&format!("refresh:{did}"))
            .ok_or_else(|| EngineError::internal("no refresh lock"))?;
        let dpop = parse_dpop(&data)?;
        Ok(Self {
            did,
            pds,
            bridge: Bridge::new(net),
            store,
            lock_file,
            state: Mutex::new(OAuthState { data, dpop }),
        })
    }

    /// The PDS this session talks to.
    pub fn pds(&self) -> &Url {
        &self.pds
    }

    /// The current session data (secrets redacted in `Debug`).
    pub async fn data(&self) -> OAuthData {
        let st = self.state.lock().await;
        let mut d = st.data.clone();
        if let Ok(v) = serde_json::to_value(&st.dpop) {
            d.dpop = SecretJson(v);
        }
        d
    }

    /// Refreshes under the account's refresh lock, adopting a session another process
    /// refreshed meanwhile.
    async fn refresh(&self, st: &mut OAuthState, forced: bool) -> Result<(), HostError> {
        let _lock = FileLock::acquire_async(self.lock_file.clone())
            .await
            .map_err(|e| HostError::Storage(format!("refresh lock: {e}")))?;
        if let Ok(Some(StoredSession::Oauth(stored))) = self.store.load(self.did.as_str()).await
            && stored.refresh_token != st.data.refresh_token
            && let Ok(dpop) = parse_dpop(&stored)
        {
            st.data = stored;
            st.dpop = dpop;
            let fresh = !st.data.expires_at_ms.is_some_and(|e| expiring(e, now_ms()));
            if fresh || forced {
                tracing::debug!(did = %self.did, "adopted a session refreshed by another process");
                return Ok(());
            }
        }
        let Some(refresh_token) = st.data.refresh_token.clone() else {
            return Err(HostError::AuthRequired(self.did.to_string()));
        };
        let metadata: OAuthMetadata = OAuthMetadata {
            server_metadata: bound_server(&st.data),
            client_metadata: client_metadata(
                &st.data.client_id,
                &st.data.redirect_uri,
                st.data.scope.as_deref().unwrap_or(""),
            ),
            keyset: None,
        };
        let result = oauth_request::<OAuthTokenResponse, Bridge, DpopReqData, SmolStr>(
            &self.bridge,
            &mut st.dpop,
            OAuthRequest::Refresh(RefreshRequestParameters {
                grant_type: TokenGrantType::RefreshToken,
                refresh_token: refresh_token.expose(),
                scope: None,
            }),
            &metadata,
        )
        .await;
        let token = match result {
            Ok(t) => t,
            Err(e) if e.is_permanent() => {
                tracing::debug!(did = %self.did, "refresh token refused; login needed");
                return Err(HostError::AuthRequired(self.did.to_string()));
            }
            Err(e) => {
                let err = request_error(e, "token refresh");
                return Err(match err.code {
                    ErrorCode::Network => HostError::Network(err.message),
                    ErrorCode::AuthRequired => HostError::AuthRequired(self.did.to_string()),
                    _ => HostError::Other(err.message),
                });
            }
        };
        if let Some(sub) = &token.sub
            && sub.as_str() != self.did.as_str()
        {
            return Err(HostError::Blocked(
                "the refreshed token belongs to another account".into(),
            ));
        }
        let now = now_ms();
        st.data.access_token = Secret::new(token.access_token.to_string());
        st.data.refresh_token = token
            .refresh_token
            .as_ref()
            .map(|t| Secret::new(t.to_string()));
        st.data.expires_at_ms = token
            .expires_in
            .and_then(|s| s.checked_mul(1000))
            .map(|ms| now.saturating_add(ms));
        if let Some(scope) = &token.scope {
            st.data.scope = Some(scope.to_string());
        }
        st.data.dpop = SecretJson(
            serde_json::to_value(&st.dpop).map_err(|e| HostError::Other(e.to_string()))?,
        );
        self.store
            .save(&StoredSession::Oauth(st.data.clone()))
            .await
            .map_err(|e| HostError::Storage(e.message))?;
        tracing::debug!(did = %self.did, "refreshed the OAuth session");
        Ok(())
    }

    /// One authenticated attempt: returns the response and whether the PDS asked for a new
    /// DPoP nonce.
    async fn attempt(&self, req: &HttpRequest) -> Result<(HttpResponse, bool), HostError> {
        let mut nr = NetRequest::for_user_pds(&self.pds, req.clone())?;
        nr.headers.retain(|(k, _)| {
            !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("dpop")
        });
        let sent_nonce = {
            let st = self.state.lock().await;
            let token = st.data.access_token.expose();
            let nonce = st.data.dpop_host_nonce.clone();
            let proof = build_dpop_proof(
                &st.dpop.dpop_key,
                nr.method.as_str(),
                &htu(&nr.url),
                nonce.as_deref(),
                Some(&access_token_hash(token)),
            )
            .map_err(|e| HostError::Other(format!("DPoP proof: {e}")))?;
            nr.headers
                .push(("authorization".into(), format!("DPoP {token}")));
            nr.headers.push(("dpop".into(), proof.to_string()));
            nonce
        };
        let resp = self.bridge.net().execute(nr).await?;
        let new_nonce = resp.header("dpop-nonce").map(str::to_owned);
        let changed = new_nonce.is_some() && new_nonce != sent_nonce;
        if changed {
            self.state.lock().await.data.dpop_host_nonce = new_nonce;
        }
        let wants_nonce = changed
            && matches!(resp.status, 400 | 401)
            && auth_error(&resp).as_deref() == Some("use_dpop_nonce");
        Ok((resp, wants_nonce))
    }

    async fn send_inner(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        super::allowlist::check(&self.did, &req)?;
        {
            let mut st = self.state.lock().await;
            if st.data.expires_at_ms.is_some_and(|e| expiring(e, now_ms())) {
                self.refresh(&mut st, false).await?;
            }
        }
        let mut refreshed = false;
        let mut nonce_retried = false;
        loop {
            let (resp, wants_nonce) = self.attempt(&req).await?;
            if wants_nonce && !nonce_retried {
                nonce_retried = true;
                continue;
            }
            if resp.status == 401
                && !refreshed
                && matches!(
                    auth_error(&resp).as_deref(),
                    Some("invalid_token" | "InvalidToken" | "ExpiredToken")
                )
            {
                refreshed = true;
                let mut st = self.state.lock().await;
                self.refresh(&mut st, true).await?;
                continue;
            }
            return Ok(resp);
        }
    }
}

impl AuthSession for OAuthSession {
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
    fn dpop_helpers() {
        let u =
            Url::parse("https://pds.example.com/xrpc/com.atproto.repo.getRecord?repo=x#f").unwrap();
        assert_eq!(
            htu(&u),
            "https://pds.example.com/xrpc/com.atproto.repo.getRecord"
        );
        // RFC 9449 appendix example: ath of "Kz~8mXK1EalYznwH-LC-1fBAo.4Ljp~zsPE_NeO.gxU".
        assert_eq!(
            access_token_hash("Kz~8mXK1EalYznwH-LC-1fBAo.4Ljp~zsPE_NeO.gxU"),
            "fUHyO2r2Z3DZ53EsNrWBb0xWXoaNy59IiKCAqksmQEo"
        );
        let resp = HttpResponse {
            status: 401,
            headers: vec![(
                "www-authenticate".into(),
                r#"DPoP algs="ES256", error="use_dpop_nonce", error_description="x""#.into(),
            )],
            body: Vec::new(),
        };
        assert_eq!(auth_error(&resp).as_deref(), Some("use_dpop_nonce"));
        let resp = HttpResponse {
            status: 400,
            headers: Vec::new(),
            body: br#"{"error":"ExpiredToken"}"#.to_vec(),
        };
        assert_eq!(auth_error(&resp).as_deref(), Some("ExpiredToken"));
        assert_eq!(host_of("http://localhost:2583"), "localhost:2583");
        assert_eq!(host_of("https://bsky.social"), "bsky.social");
    }

    #[test]
    fn client_metadata_is_public_native() {
        let m = client_metadata(
            "http://localhost?x",
            "http://127.0.0.1:1/callback",
            "atproto",
        );
        assert_eq!(m.token_endpoint_auth_method.as_deref(), Some("none"));
        assert_eq!(m.dpop_bound_access_tokens, Some(true));
        assert_eq!(m.redirect_uris.len(), 1);
    }
}
