// SPDX-License-Identifier: AGPL-3.0-or-later
//! App-password sessions against a fake PDS (axum on 127.0.0.1, reached through the guard's
//! devnet allowance): `createSession`, bearer requests through the host, proactive and
//! reactive refresh, single-use refresh tokens under the refresh lock with two sessions
//! standing in for two processes, no redirects for `UserPds`, and no secrets in `Debug`.
//!
//! The same server also acts as the devnet PLC directory (`GET /<did>`) and handle resolver, for
//! the whole login: the login target (resolved now, never through a hint; confirmation when the
//! input handle is not the verified one), the ordered login, `restore_session` refusing a
//! migrated account, `logout` with an unsafe session file, and adoption of stored sessions only
//! when strictly newer.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use lessdugin_atp::syntax::Did;
use lessdugin_core::engine::ErrorCode;
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_core::host::{Host, HostError, HttpRequest, Target};
use lessdugin_core::store::mem::MemKv;
use lessdugin_native::auth::app_password::{create_session, session_data};
use lessdugin_native::auth::{
    AppPassword, AppPasswordSession, SecretStore, StoredSession, login_app_password,
    logout_session, resolve_login_target, restore_session,
};
use lessdugin_native::config::Paths;
use lessdugin_native::host::{AuthSession, NativeHost, Net, NetPolicy};
use serde_json::{Value, json};
use url::Url;

const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
const APP_PASSWORD: &str = "abcd-efgh-ijkl-mnop";

#[derive(Default)]
struct Pds {
    next: AtomicUsize,
    access: Mutex<HashSet<String>>,
    refresh: Mutex<HashSet<String>>,
    refreshes: AtomicUsize,
    target_hits: AtomicUsize,
    /// Issue access tokens that are already expired (by their `exp`).
    expired_access: Mutex<bool>,
    /// Answer `getSession` with a redirect.
    redirect_get_session: Mutex<bool>,
    get_session_hits: AtomicUsize,
    /// The DID document served at `GET /<did>` (the server is also the devnet PLC).
    doc: Mutex<Option<Value>>,
    /// `com.atproto.identity.resolveHandle` answers.
    handles: Mutex<HashMap<String, String>>,
    deleted_sessions: AtomicUsize,
}

fn jwt(kind: &str, n: usize, exp_secs: i64) -> String {
    let b64 = |v: &Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
    };
    format!(
        "{}.{}.sig{kind}{n}",
        b64(&json!({"alg": "ES256K"})),
        b64(&json!({"scope": kind, "sub": DID, "exp": exp_secs, "n": n}))
    )
}

#[allow(clippy::disallowed_methods)] // the fake server's own clock
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

impl Pds {
    fn issue(&self) -> Value {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let exp = if *self.expired_access.lock().unwrap() {
            now_secs() - 10
        } else {
            now_secs() + 3600
        };
        let access = jwt("access", n, exp);
        let refresh = jwt("refresh", n, now_secs() + 86400);
        self.access.lock().unwrap().insert(access.clone());
        self.refresh.lock().unwrap().insert(refresh.clone());
        json!({"accessJwt": access, "refreshJwt": refresh, "handle": "alice.test", "did": DID})
    }
}

fn bearer(h: &HeaderMap) -> String {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned()
}

fn xrpc_error(status: StatusCode, name: &str) -> Response {
    (status, axum::Json(json!({"error": name, "message": name}))).into_response()
}

async fn serve(pds: Arc<Pds>) -> Url {
    let p1 = pds.clone();
    let p2 = pds.clone();
    let p3 = pds.clone();
    let p4 = pds.clone();
    let p5 = pds.clone();
    let p6 = pds.clone();
    let p7 = pds.clone();
    let app = Router::new()
        .route(
            "/{did}",
            get(
                move |axum::extract::Path(did): axum::extract::Path<String>| {
                    let pds = p5.clone();
                    async move {
                        match pds.doc.lock().unwrap().clone() {
                            Some(doc) if did == DID => axum::Json(doc).into_response(),
                            _ => StatusCode::NOT_FOUND.into_response(),
                        }
                    }
                },
            ),
        )
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(
                move |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| {
                    let pds = p6.clone();
                    async move {
                        let handle = q.get("handle").cloned().unwrap_or_default();
                        match pds.handles.lock().unwrap().get(&handle) {
                            Some(did) => axum::Json(json!({"did": did})).into_response(),
                            None => xrpc_error(StatusCode::BAD_REQUEST, "HandleNotFound"),
                        }
                    }
                },
            ),
        )
        .route(
            "/xrpc/com.atproto.server.deleteSession",
            post(move |h: HeaderMap| {
                let pds = p7.clone();
                async move {
                    pds.refresh.lock().unwrap().remove(&bearer(&h));
                    pds.deleted_sessions.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.server.createSession",
            post(move |axum::Json(body): axum::Json<Value>| {
                let pds = p1.clone();
                async move {
                    if body["password"] == APP_PASSWORD && body["identifier"] == DID {
                        axum::Json(pds.issue()).into_response()
                    } else {
                        xrpc_error(StatusCode::UNAUTHORIZED, "AuthenticationRequired")
                    }
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.server.refreshSession",
            post(move |h: HeaderMap| {
                let pds = p2.clone();
                async move {
                    // Refresh tokens are single use.
                    if pds.refresh.lock().unwrap().remove(&bearer(&h)) {
                        pds.refreshes.fetch_add(1, Ordering::SeqCst);
                        *pds.expired_access.lock().unwrap() = false;
                        axum::Json(pds.issue()).into_response()
                    } else {
                        xrpc_error(StatusCode::BAD_REQUEST, "ExpiredToken")
                    }
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(move |h: HeaderMap| {
                let pds = p3.clone();
                async move {
                    pds.get_session_hits.fetch_add(1, Ordering::SeqCst);
                    if *pds.redirect_get_session.lock().unwrap() {
                        return Response::builder()
                            .status(302)
                            .header(header::LOCATION, "/xrpc/target")
                            .body(axum::body::Body::empty())
                            .unwrap();
                    }
                    if pds.access.lock().unwrap().contains(&bearer(&h)) {
                        axum::Json(json!({"did": DID, "handle": "alice.test"})).into_response()
                    } else {
                        xrpc_error(StatusCode::BAD_REQUEST, "InvalidToken")
                    }
                }
            }),
        )
        .route(
            "/xrpc/target",
            get(move || {
                let pds = p4.clone();
                async move {
                    pds.target_hits.fetch_add(1, Ordering::SeqCst);
                    "target"
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Url::parse(&format!("http://127.0.0.1:{}", addr.port())).unwrap()
}

struct Fixture {
    pds: Arc<Pds>,
    url: Url,
    net: Net,
    store: SecretStore,
    paths: Paths,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let pds = Arc::new(Pds::default());
    let url = serve(pds.clone()).await;
    let net = Net::new(NetPolicy::devnet([&url])).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let store = SecretStore::file_only(dir.path().join("sessions"));
    Fixture {
        pds,
        url,
        net,
        store,
        paths,
        _dir: dir,
    }
}

impl Fixture {
    fn pds_str(&self) -> String {
        self.url.as_str().trim_end_matches('/').to_owned()
    }

    async fn login(&self) -> AppPasswordSession {
        let pw = AppPassword::new(APP_PASSWORD).unwrap();
        let s = create_session(&self.net, &self.url, DID, &pw)
            .await
            .unwrap();
        let data = session_data(&s, &self.pds_str(), 0);
        self.store
            .save(&StoredSession::AppPassword(data.clone()))
            .await
            .unwrap();
        self.session(data)
    }

    fn session(&self, data: lessdugin_native::auth::store::AppPasswordData) -> AppPasswordSession {
        AppPasswordSession::new(
            data,
            self.net.clone(),
            self.store.clone(),
            self.paths.lock_file(&format!("refresh:{DID}")).unwrap(),
        )
        .unwrap()
    }
}

fn get_session_req() -> HttpRequest {
    HttpRequest::get(Target::UserPds {
        account: Did::parse(DID).unwrap(),
        path_and_query: "/xrpc/com.atproto.server.getSession".into(),
    })
}

#[tokio::test]
async fn create_session_refuses_wrong_credentials() {
    let f = fixture().await;
    let pw = AppPassword::new("wxyz-wxyz-wxyz-wxyz").unwrap();
    let e = create_session(&f.net, &f.url, DID, &pw).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthRequired);
    assert!(!e.message.contains("wxyz"));
}

#[tokio::test]
async fn bearer_requests_through_the_host() {
    let f = fixture().await;
    let session = Arc::new(f.login().await);
    let dbg = format!("{session:?} {:?}", session.data().await);
    assert!(
        !dbg.contains("sigaccess") && !dbg.contains("sigrefresh"),
        "{dbg}"
    );
    let host = NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        f.paths.blobs_dir(),
        f.net.clone(),
    );
    // Without a registered session: auth_required.
    assert!(matches!(
        host.http(get_session_req()).await,
        Err(HostError::AuthRequired(_))
    ));
    host.set_session(session.clone());
    let resp = host.http(get_session_req()).await.unwrap();
    assert_eq!(resp.status, 200);
    // UserPds requests never follow redirects.
    *f.pds.redirect_get_session.lock().unwrap() = true;
    let resp = host.http(get_session_req()).await.unwrap();
    assert_eq!(resp.status, 302);
    assert_eq!(f.pds.target_hits.load(Ordering::SeqCst), 0);
    // Requests off the write allowlist (AUTH-4) never reach the PDS.
    let hits = f.pds.get_session_hits.load(Ordering::SeqCst);
    let list = HttpRequest::get(Target::UserPds {
        account: Did::parse(DID).unwrap(),
        path_and_query: "/xrpc/com.atproto.repo.listRecords?repo=x".into(),
    });
    assert!(matches!(host.http(list).await, Err(HostError::Blocked(_))));
    let mut wrong_repo = HttpRequest::post_json(
        Target::UserPds {
            account: Did::parse(DID).unwrap(),
            path_and_query: "/xrpc/com.atproto.repo.createRecord".into(),
        },
        serde_json::to_vec(&json!({"repo": "did:plc:other", "collection": "app.bsky.feed.post"}))
            .unwrap(),
    );
    assert!(matches!(
        host.http(wrong_repo.clone()).await,
        Err(HostError::Blocked(_))
    ));
    wrong_repo.method = axum::http::Method::GET;
    assert!(matches!(
        host.http(wrong_repo).await,
        Err(HostError::Blocked(_))
    ));
    assert_eq!(f.pds.get_session_hits.load(Ordering::SeqCst), hits);
    // A path cannot leave the PDS origin.
    let escape = HttpRequest::get(Target::UserPds {
        account: Did::parse(DID).unwrap(),
        path_and_query: "//evil.example/xrpc/x".into(),
    });
    assert!(matches!(
        host.http(escape).await,
        Err(HostError::Blocked(_))
    ));
}

#[tokio::test]
async fn proactive_and_reactive_refresh() {
    let f = fixture().await;
    *f.pds.expired_access.lock().unwrap() = true;
    let session = f.login().await;
    // The access JWT's exp is in the past: refreshed before the request.
    let resp = session.send(get_session_req()).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(f.pds.refreshes.load(Ordering::SeqCst), 1);
    let Some(StoredSession::AppPassword(saved)) = f.store.load(DID).await.unwrap() else {
        panic!("not saved");
    };
    assert_eq!(saved.refresh_jwt, session.data().await.refresh_jwt);

    // The server revokes the access token: 400 InvalidToken → refresh → retry.
    f.pds.access.lock().unwrap().clear();
    let resp = session.send(get_session_req()).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(f.pds.refreshes.load(Ordering::SeqCst), 2);

    // The refresh token is revoked too: auth_required.
    f.pds.access.lock().unwrap().clear();
    f.pds.refresh.lock().unwrap().clear();
    let err = session.send(get_session_req()).await.unwrap_err();
    assert!(matches!(err, HostError::AuthRequired(_)), "{err:?}");
}

#[tokio::test]
async fn two_processes_refresh_once() {
    let f = fixture().await;
    *f.pds.expired_access.lock().unwrap() = true;
    let first = f.login().await;
    // A second "process" loaded the same stored session.
    let Some(StoredSession::AppPassword(data)) = f.store.load(DID).await.unwrap() else {
        panic!();
    };
    let second = f.session(data);
    let (a, b) = tokio::join!(
        first.send(get_session_req()),
        second.send(get_session_req())
    );
    assert_eq!(a.unwrap().status, 200);
    assert_eq!(b.unwrap().status, 200);
    // The single-use refresh token was spent once; the other session adopted the result.
    assert_eq!(f.pds.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        first.data().await.refresh_jwt,
        second.data().await.refresh_jwt
    );
}

// ---------------------------------------------------------------------------------------------
// Logins over a devnet-mode engine whose PLC and PDS are the fake server

/// alice's DID document, with `pds` as her PDS and `handle` as her claimed handle.
fn doc(pds: &str, handle: &str) -> Value {
    json!({
        "id": DID,
        "alsoKnownAs": [format!("at://{handle}")],
        "verificationMethod": [{
            "id": format!("{DID}#atproto"),
            "type": "Multikey",
            "controller": DID,
            "publicKeyMultibase": "zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF",
        }],
        "service": [{
            "id": "#atproto_pds",
            "type": "AtprotoPersonalDataServer",
            "serviceEndpoint": pds,
        }],
    })
}

impl Fixture {
    fn engine(&self) -> Engine<NativeHost<MemKv>> {
        {
            let mut d = self.pds.doc.lock().unwrap();
            if d.is_none() {
                *d = Some(doc(&self.pds_str(), "alice.test"));
            }
        }
        self.pds
            .handles
            .lock()
            .unwrap()
            .insert("alice.test".into(), DID.into());
        let host = NativeHost::with_parts(
            MemKv::new(),
            MemKv::new(),
            self.paths.blobs_dir(),
            self.net.clone(),
        );
        Engine::new(
            host,
            EngineConfig::devnet(self.url.clone(), self.url.clone()),
        )
    }
}

#[tokio::test]
async fn app_password_login_end_to_end() {
    let f = fixture().await;
    let e = f.engine();
    let target = resolve_login_target(&e, "alice.test").await.unwrap();
    assert_eq!(target.did.as_str(), DID);
    assert_eq!(target.pds, f.pds_str());
    assert!(!target.needs_confirmation());
    assert_eq!(target.display_name(), "@alice.test");
    let pw = AppPassword::new(APP_PASSWORD).unwrap();
    let out = login_app_password(&e, &f.store, &f.paths, &target, &pw)
        .await
        .unwrap();
    assert_eq!(out.account.did, DID);
    assert_eq!(out.account.handle.as_deref(), Some("alice.test"));
    assert!(out.account.active);
    assert!(f.store.load(DID).await.unwrap().is_some());
    // The registered session works.
    let resp = e.host().http(get_session_req()).await.unwrap();
    assert_eq!(resp.status, 200);
    // restore_session re-checks the account and registers the stored session.
    let host = e.host();
    host.remove_session(&Did::parse(DID).unwrap());
    let a = restore_session(&e, &f.store, &f.paths, &out.account)
        .await
        .unwrap();
    assert_eq!(a.pds, f.pds_str());
    assert_eq!(e.host().http(get_session_req()).await.unwrap().status, 200);
}

#[tokio::test]
async fn an_unverified_input_handle_needs_confirmation() {
    let f = fixture().await;
    let e = f.engine();
    // other.test points at alice's DID, but her document claims alice.test.
    f.pds
        .handles
        .lock()
        .unwrap()
        .insert("other.test".into(), DID.into());
    let t = resolve_login_target(&e, "other.test").await.unwrap();
    assert!(t.needs_confirmation());
    assert_eq!(t.handle.as_deref(), Some("alice.test"));
    assert_eq!(t.display_name(), "@alice.test");
    // A DID input never needs confirmation; with no verified handle, the DID is the name.
    *f.pds.doc.lock().unwrap() = Some(doc(&f.pds_str(), "nobody.test"));
    let t = resolve_login_target(&e, DID).await.unwrap();
    assert!(!t.needs_confirmation());
    assert_eq!(t.handle, None);
    assert_eq!(t.display_name(), DID);
}

#[tokio::test]
async fn the_login_target_uses_a_document_fetched_now() {
    let f = fixture().await;
    let e = f.engine();
    // Cached: the document names this PDS.
    e.resolve_identity(DID).await.unwrap();
    // The document changes (a poisoned or stale cache would still say the old one).
    *f.pds.doc.lock().unwrap() = Some(doc("http://127.0.0.1:1", "alice.test"));
    let t = resolve_login_target(&e, "alice.test").await.unwrap();
    assert_eq!(t.pds, "http://127.0.0.1:1");
}

#[tokio::test]
async fn a_migrated_account_needs_relogin() {
    let f = fixture().await;
    let e = f.engine();
    let target = resolve_login_target(&e, DID).await.unwrap();
    let pw = AppPassword::new(APP_PASSWORD).unwrap();
    let out = login_app_password(&e, &f.store, &f.paths, &target, &pw)
        .await
        .unwrap();
    // The account moves to another PDS.
    *f.pds.doc.lock().unwrap() = Some(doc("http://127.0.0.1:1", "alice.test"));
    e.invalidate_identity(DID).await.unwrap();
    let err = restore_session(&e, &f.store, &f.paths, &out.account)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthRequired);
    assert_eq!(err.detail.unwrap()["kind"], "needsRelogin");
    assert!(
        e.host().session(&Did::parse(DID).unwrap()).is_none(),
        "no session stays registered"
    );
    assert!(matches!(
        e.host().http(get_session_req()).await,
        Err(HostError::AuthRequired(_))
    ));
}

#[tokio::test]
async fn a_failed_binding_deletes_the_new_session() {
    let f = fixture().await;
    let e = f.engine();
    let target = resolve_login_target(&e, DID).await.unwrap();
    // Between resolution and the binding, the document moves the account elsewhere.
    *f.pds.doc.lock().unwrap() = Some(doc("http://127.0.0.1:1", "alice.test"));
    let pw = AppPassword::new(APP_PASSWORD).unwrap();
    let err = login_app_password(&e, &f.store, &f.paths, &target, &pw)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(f.pds.deleted_sessions.load(Ordering::SeqCst), 1);
    assert!(f.store.load(DID).await.unwrap().is_none());
    assert!(e.accounts().await.unwrap().is_empty());
}

#[tokio::test]
async fn logout_deletes_an_unsafe_session_file() {
    let f = fixture().await;
    let session = f.login().await;
    drop(session);
    let path = f.store.session_file(DID);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let host = NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        f.paths.blobs_dir(),
        f.net.clone(),
    );
    #[cfg(unix)]
    assert_eq!(
        f.store.load(DID).await.unwrap_err().code,
        ErrorCode::Blocked
    );
    let report = logout_session(&host, &f.store, DID).await.unwrap();
    #[cfg(unix)]
    {
        assert!(report.unreadable.is_some(), "{report:?}");
        assert!(!report.revoked && !report.had_session);
    }
    assert!(!path.exists(), "the exposed secret is gone");
    assert_eq!(f.store.load(DID).await.unwrap(), None);
}

#[tokio::test]
async fn an_older_stored_copy_is_never_adopted() {
    let f = fixture().await;
    let session = f.login().await;
    let current = session.data().await;
    // A failed save left an older copy (an already-used refresh token) in storage.
    let stale = lessdugin_native::auth::store::AppPasswordData {
        refresh_jwt: lessdugin_native::auth::redact::Secret::new("stale-refresh"),
        access_jwt: lessdugin_native::auth::redact::Secret::new("stale-access"),
        saved_ms: current.saved_ms - 1,
        ..current.clone()
    };
    f.store
        .save(&StoredSession::AppPassword(stale))
        .await
        .unwrap();
    // The PDS rejects the access token: the session refreshes with its own refresh token.
    f.pds.access.lock().unwrap().clear();
    let resp = session.send(get_session_req()).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(f.pds.refreshes.load(Ordering::SeqCst), 1);
    let now = session.data().await;
    assert!(now.saved_ms > current.saved_ms);
    let Some(StoredSession::AppPassword(saved)) = f.store.load(DID).await.unwrap() else {
        panic!("not saved");
    };
    assert_eq!(saved.refresh_jwt, now.refresh_jwt);
}

#[tokio::test]
async fn a_hint_resolution_is_refused_for_login() {
    use lessdugin_testkit::identity::{DOH, PLC, install_doc, install_nxdomain};
    use lessdugin_testkit::{FakeIdentity, MemHost, resp};
    let host = MemHost::new();
    let alice = FakeIdentity::plc("alice", "alice.example.com", "https://pds.example.com");
    install_doc(&host, PLC, &alice.did, &alice.doc());
    install_nxdomain(&host, DOH, "alice.example.com");
    host.respond_get("https://alice.example.com/", resp::status(404));
    host.respond_get(
        "https://slingshot.microcosm.blue/xrpc/com.bad-example.identity.resolveMiniDoc",
        resp::ok_json(&json!({"did": alice.did.as_str(), "handle": "alice.example.com"})),
    );
    let e = Engine::new(host.clone(), EngineConfig::default());
    let err = resolve_login_target(&e, "alice.example.com")
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Blocked);
    let d = err.detail.unwrap();
    assert_eq!(d["kind"], "hintResolution");
    assert_eq!(d["did"], alice.did.as_str());
    // The DID itself is fine (the document comes from PLC).
    let t = resolve_login_target(&e, alice.did.as_str()).await.unwrap();
    assert_eq!(t.pds, "https://pds.example.com");
    assert!(!t.needs_confirmation());
}
