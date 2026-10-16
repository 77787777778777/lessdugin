// SPDX-License-Identifier: AGPL-3.0-or-later
//! App-password sessions against a fake PDS (axum on 127.0.0.1, reached through the guard's
//! devnet allowance): `createSession`, bearer requests through the host, proactive and
//! reactive refresh, single-use refresh tokens under the refresh lock with two sessions
//! standing in for two processes, no redirects for `UserPds`, and no secrets in `Debug`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use lessdugin_atp::syntax::Did;
use lessdugin_core::engine::ErrorCode;
use lessdugin_core::host::{Host, HostError, HttpRequest, Target};
use lessdugin_core::store::mem::MemKv;
use lessdugin_native::auth::app_password::{create_session, session_data};
use lessdugin_native::auth::{AppPassword, AppPasswordSession, SecretStore, StoredSession};
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
    let app = Router::new()
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
