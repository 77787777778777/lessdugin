// SPDX-License-Identifier: AGPL-3.0-or-later
//! `OAuthSession` offline, against a fake PDS + authorization server (axum on 127.0.0.1, reached
//! through the guard's devnet allowance): DPoP proofs (`htu` without query, `ath`, the PDS's
//! nonce), one retry on `use_dpop_nonce`, refresh after a 401 `invalid_token`, proactive
//! refresh, single-use refresh tokens shared by two "processes" (one refreshes, the other
//! adopts), never adopting an older stored copy, and the write allowlist (AUTH-4) on the OAuth
//! path.

#![cfg(feature = "oauth")]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use jacquard_oauth::session::DpopReqData;
use jacquard_oauth::types::OAuthAuthorizationServerMetadata;
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{HostError, HttpRequest, Target};
use lessdugin_native::auth::oauth::OAuthSession;
use lessdugin_native::auth::redact::{Secret, SecretJson};
use lessdugin_native::auth::store::{OAuthData, SESSION_VERSION};
use lessdugin_native::auth::{SecretStore, StoredSession};
use lessdugin_native::config::Paths;
use lessdugin_native::host::{AuthSession, Net, NetPolicy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

/// One DPoP proof as the PDS saw it.
#[derive(Clone, Debug)]
struct Proof {
    htu: String,
    htm: String,
    nonce: Option<String>,
    ath_ok: bool,
}

#[derive(Default)]
struct Server {
    next: AtomicUsize,
    access: Mutex<HashSet<String>>,
    refresh: Mutex<HashSet<String>>,
    refreshes: AtomicUsize,
    /// The nonce the PDS currently wants in DPoP proofs.
    nonce: Mutex<String>,
    /// The DPoP proofs the PDS saw.
    proofs: Mutex<Vec<Proof>>,
    pds_hits: AtomicUsize,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The claims of a compact JWS (unverified: the fake only checks what the client put in).
fn claims(jwt: &str) -> Value {
    let payload = jwt.split('.').nth(1).unwrap_or("");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap_or_default();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

impl Server {
    fn issue(&self) -> (String, String) {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let (a, r) = (format!("access-{n}"), format!("refresh-{n}"));
        self.access.lock().unwrap().insert(a.clone());
        self.refresh.lock().unwrap().insert(r.clone());
        (a, r)
    }
}

fn dpop_challenge(error: &str, nonce: Option<&str>) -> Response {
    let mut b = Response::builder().status(401).header(
        "www-authenticate",
        format!(r#"DPoP algs="ES256", error="{error}""#),
    );
    if let Some(n) = nonce {
        b = b.header("dpop-nonce", n);
    }
    b.body(axum::body::Body::from(
        json!({"error": error, "message": error}).to_string(),
    ))
    .unwrap()
}

async fn serve(srv: Arc<Server>) -> Url {
    let s1 = srv.clone();
    let s2 = srv.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("http://127.0.0.1:{port}");
    let o1 = origin.clone();
    let app = Router::new()
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(move |h: HeaderMap, uri: Uri| {
                let srv = s1.clone();
                let origin = o1.clone();
                async move {
                    srv.pds_hits.fetch_add(1, Ordering::SeqCst);
                    let auth = h
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    let Some(token) = auth.strip_prefix("DPoP ") else {
                        return dpop_challenge("invalid_token", None);
                    };
                    let proof = h.get("dpop").and_then(|v| v.to_str().ok()).unwrap_or("");
                    let c = claims(proof);
                    let ath_ok = c["ath"].as_str() == Some(&b64(&Sha256::digest(token.as_bytes())));
                    let nonce = c["nonce"].as_str().map(str::to_owned);
                    srv.proofs.lock().unwrap().push(Proof {
                        htu: c["htu"].as_str().unwrap_or("").to_owned(),
                        htm: c["htm"].as_str().unwrap_or("").to_owned(),
                        nonce: nonce.clone(),
                        ath_ok,
                    });
                    assert_eq!(c["htu"], format!("{origin}{}", uri.path()));
                    let want = srv.nonce.lock().unwrap().clone();
                    if nonce.as_deref() != Some(want.as_str()) {
                        return dpop_challenge("use_dpop_nonce", Some(&want));
                    }
                    if !ath_ok || !srv.access.lock().unwrap().contains(token) {
                        return dpop_challenge("invalid_token", None);
                    }
                    axum::Json(json!({"did": DID, "handle": "alice.test"})).into_response()
                }
            }),
        )
        .route(
            "/oauth/token",
            post(move |h: HeaderMap, body: String| {
                let srv = s2.clone();
                async move {
                    assert!(h.get("dpop").is_some(), "the token request is DPoP-bound");
                    let form: HashMap<String, String> =
                        url::form_urlencoded::parse(body.as_bytes())
                            .into_owned()
                            .collect();
                    assert_eq!(form["grant_type"], "refresh_token");
                    let rt = form["refresh_token"].clone();
                    // Refresh tokens are single use.
                    if !srv.refresh.lock().unwrap().remove(&rt) {
                        return (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({"error": "invalid_grant"})),
                        )
                            .into_response();
                    }
                    srv.refreshes.fetch_add(1, Ordering::SeqCst);
                    let (a, r) = srv.issue();
                    axum::Json(json!({
                        "access_token": a, "token_type": "DPoP", "refresh_token": r,
                        "expires_in": 3600, "scope": "atproto", "sub": DID,
                    }))
                    .into_response()
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Url::parse(&origin).unwrap()
}

struct Fixture {
    srv: Arc<Server>,
    url: Url,
    net: Net,
    store: SecretStore,
    paths: Paths,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let srv = Arc::new(Server::default());
    *srv.nonce.lock().unwrap() = "n1".into();
    let url = serve(srv.clone()).await;
    let net = Net::new(NetPolicy::devnet([&url])).unwrap();
    let dir = tempfile::tempdir().unwrap();
    Fixture {
        srv,
        url,
        net,
        store: SecretStore::file_only(dir.path().join("sessions")),
        paths: Paths::under(dir.path()),
        _dir: dir,
    }
}

impl Fixture {
    fn origin(&self) -> String {
        self.url.as_str().trim_end_matches('/').to_owned()
    }

    /// A freshly "logged in" session, saved to the store.
    async fn login(&self, expires_at_ms: Option<i64>) -> OAuthData {
        let (access, refresh) = self.srv.issue();
        let mut server: OAuthAuthorizationServerMetadata<jacquard_common::SmolStr> =
            OAuthAuthorizationServerMetadata {
                dpop_signing_alg_values_supported: Some(vec!["ES256".into()]),
                ..Default::default()
            };
        let key = jacquard_oauth::utils::generate_dpop_key(&mut server).unwrap();
        let dpop = DpopReqData {
            dpop_key: key,
            dpop_authserver_nonce: None,
        };
        let data = OAuthData {
            v: SESSION_VERSION,
            did: DID.into(),
            pds: self.origin(),
            issuer: self.origin(),
            token_endpoint: format!("{}/oauth/token", self.origin()),
            revocation_endpoint: None,
            client_id: "http://localhost?scope=atproto".into(),
            redirect_uri: "http://127.0.0.1:1/callback".into(),
            scope: Some("atproto".into()),
            access_token: Secret::new(access),
            refresh_token: Some(Secret::new(refresh)),
            expires_at_ms,
            dpop: SecretJson(serde_json::to_value(&dpop).unwrap()),
            dpop_host_nonce: None,
            created_ms: 1,
            saved_ms: lessdugin_native::auth::store::next_saved_ms(0),
        };
        self.store
            .save(&StoredSession::Oauth(data.clone()))
            .await
            .unwrap();
        data
    }

    fn session(&self, data: OAuthData) -> OAuthSession {
        OAuthSession::new(data, self.net.clone(), self.store.clone(), &self.paths).unwrap()
    }
}

fn get_session(query: &str) -> HttpRequest {
    HttpRequest::get(Target::UserPds {
        account: Did::parse(DID).unwrap(),
        path_and_query: format!("/xrpc/com.atproto.server.getSession{query}"),
    })
}

#[tokio::test]
async fn dpop_nonce_retry_and_proof_contents() {
    let f = fixture().await;
    let s = f.session(f.login(None).await);
    // First attempt has no nonce: the PDS asks for one, the session retries once.
    let resp = s.send(get_session("?x=1")).await.unwrap();
    assert_eq!(resp.status, 200);
    let proofs = f.srv.proofs.lock().unwrap().clone();
    assert_eq!(proofs.len(), 2);
    assert_eq!(proofs[0].nonce, None);
    assert_eq!(proofs[1].nonce.as_deref(), Some("n1"));
    for p in &proofs {
        assert!(!p.htu.contains('?'), "htu has no query: {p:?}");
        assert_eq!(p.htm, "GET");
        assert!(p.ath_ok);
    }
    // The nonce is remembered: the next request needs no retry.
    s.send(get_session("")).await.unwrap();
    assert_eq!(f.srv.proofs.lock().unwrap().len(), 3);
    // The PDS rotates its nonce: one more retry, never a loop.
    *f.srv.nonce.lock().unwrap() = "n2".into();
    assert_eq!(s.send(get_session("")).await.unwrap().status, 200);
    assert_eq!(f.srv.proofs.lock().unwrap().len(), 5);
    assert_eq!(f.srv.refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_token_refreshes_once_then_retries() {
    let f = fixture().await;
    let s = f.session(f.login(None).await);
    s.send(get_session("")).await.unwrap();
    // The server revokes the access token: 401 invalid_token → refresh → retry.
    f.srv.access.lock().unwrap().clear();
    let resp = s.send(get_session("")).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(f.srv.refreshes.load(Ordering::SeqCst), 1);
    // The refreshed session was saved.
    let Some(StoredSession::Oauth(saved)) = f.store.load(DID).await.unwrap() else {
        panic!("not saved");
    };
    assert_eq!(saved.refresh_token, s.data().await.refresh_token);
    // Both tokens revoked: the 401 comes back after one refresh attempt, as auth_required.
    f.srv.access.lock().unwrap().clear();
    f.srv.refresh.lock().unwrap().clear();
    let err = s.send(get_session("")).await.unwrap_err();
    assert!(matches!(err, HostError::AuthRequired(_)), "{err:?}");
}

#[tokio::test]
async fn two_processes_share_single_use_refresh_tokens() {
    let f = fixture().await;
    // Expired: both sessions refresh before their first request.
    let data = f.login(Some(1)).await;
    let first = f.session(data.clone());
    let second = f.session(data);
    let (a, b) = tokio::join!(first.send(get_session("")), second.send(get_session("")));
    assert_eq!(a.unwrap().status, 200);
    assert_eq!(b.unwrap().status, 200);
    // One refresh at the server; the other session adopted the stored result.
    assert_eq!(f.srv.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        first.data().await.refresh_token,
        second.data().await.refresh_token
    );
}

#[tokio::test]
async fn an_older_stored_copy_is_not_adopted() {
    let f = fixture().await;
    let data = f.login(None).await;
    let s = f.session(data.clone());
    // A failed save left an older copy with spent tokens.
    let stale = OAuthData {
        access_token: Secret::new("spent-access"),
        refresh_token: Some(Secret::new("spent-refresh")),
        saved_ms: data.saved_ms - 1,
        ..data.clone()
    };
    f.store.save(&StoredSession::Oauth(stale)).await.unwrap();
    f.srv.access.lock().unwrap().clear();
    let resp = s.send(get_session("")).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(f.srv.refreshes.load(Ordering::SeqCst), 1);
    assert!(s.data().await.saved_ms > data.saved_ms);
}

#[tokio::test]
async fn the_allowlist_applies_to_oauth_sessions() {
    let f = fixture().await;
    let s = f.session(f.login(None).await);
    let did = Did::parse(DID).unwrap();
    for (path, body) in [
        ("/xrpc/com.atproto.repo.listRecords?repo=x", None),
        (
            "/xrpc/com.atproto.identity.updateHandle",
            Some(json!({"handle": "x.test"})),
        ),
        (
            "/xrpc/com.atproto.repo.createRecord",
            Some(json!({"repo": "did:plc:other", "collection": "app.bsky.feed.post"})),
        ),
        ("//evil.example/xrpc/com.atproto.server.getSession", None),
    ] {
        let target = Target::UserPds {
            account: did.clone(),
            path_and_query: path.into(),
        };
        let req = match body {
            Some(b) => HttpRequest::post_json(target, serde_json::to_vec(&b).unwrap()),
            None => HttpRequest::get(target),
        };
        assert!(
            matches!(s.send(req).await, Err(HostError::Blocked(_))),
            "{path}"
        );
    }
    assert_eq!(
        f.srv.pds_hits.load(Ordering::SeqCst),
        0,
        "nothing reached the PDS"
    );
}
