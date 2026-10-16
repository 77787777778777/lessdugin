// SPDX-License-Identifier: AGPL-3.0-or-later
//! NativeHost against a local axum server bound to 127.0.0.1. The server is reached through
//! the guard's devnet allowance (loopback and `http://` for configured origins only), and a
//! fixed DNS table simulates other names, private answers and DNS rebinding.

// Timing assertions need the real clock (the ban targets the wasm-safe crates).
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use futures::future::BoxFuture;
use lessdugin_atp::cid as atp_cid;
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{
    Host, HostError, HttpRequest, HttpResponse, Redirects, Target, WebSocket, WsMessage,
};
use lessdugin_core::store::mem::MemKv;
use lessdugin_native::host::{AuthSession, FixedDns, NativeHost, Net, NetPolicy, NetRequest};
use url::Url;

#[derive(Clone, Default)]
struct Srv {
    hits: Arc<AtomicUsize>,
    blob_hits: Arc<AtomicUsize>,
    blobs: Arc<Mutex<BlobTable>>,
}

/// cid → (status, body) served by the fake `getBlob`.
type BlobTable = HashMap<String, (u16, Vec<u8>)>;

async fn hello(State(s): State<Srv>) -> impl IntoResponse {
    s.hits.fetch_add(1, Ordering::SeqCst);
    ([("x-test", "1")], "hi")
}

async fn user_agent(headers: HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

async fn big(Query(q): Query<HashMap<String, String>>) -> Vec<u8> {
    let n: usize = q.get("n").and_then(|n| n.parse().ok()).unwrap_or(0);
    vec![b'a'; n]
}

async fn gzip_bomb() -> Response {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(&vec![0u8; 1 << 20]).unwrap();
    let gz = enc.finish().unwrap();
    assert!(gz.len() < 10_000);
    ([(header::CONTENT_ENCODING, "gzip")], gz).into_response()
}

async fn redirect(Query(q): Query<HashMap<String, String>>) -> Response {
    let status = q
        .get("status")
        .and_then(|s| s.parse().ok())
        .unwrap_or(302u16);
    let to = q.get("to").cloned().unwrap_or_else(|| "/hello".into());
    Response::builder()
        .status(status)
        .header(header::LOCATION, to)
        .body(Body::empty())
        .unwrap()
}

async fn method(m: Method, body: Bytes) -> String {
    format!("{m} {}", body.len())
}

async fn echo_auth(headers: HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none")
        .to_owned()
}

async fn slow() -> &'static str {
    tokio::time::sleep(Duration::from_secs(3)).await;
    "late"
}

async fn trickle() -> Body {
    let stream = futures::stream::unfold(0u32, |i| async move {
        if i == 10 {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), i + 1))
    });
    Body::from_stream(stream)
}

async fn get_blob(State(s): State<Srv>, Query(q): Query<HashMap<String, String>>) -> Response {
    s.blob_hits.fetch_add(1, Ordering::SeqCst);
    let cid = q.get("cid").cloned().unwrap_or_default();
    let entry = s.blobs.lock().unwrap().get(&cid).cloned();
    match entry {
        Some((status, _)) if (300..400).contains(&status) => Response::builder()
            .status(status)
            .header(header::LOCATION, "/hello")
            .body(Body::empty())
            .unwrap(),
        Some((status, body)) => (StatusCode::from_u16(status).unwrap(), body).into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"BlobNotFound","message":"Blob not found"}"#,
        )
            .into_response(),
    }
}

async fn private_xrpc(headers: HeaderMap) -> Response {
    match headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        Some("Bearer tok") => "secret data".into_response(),
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn serve() -> (SocketAddr, Srv) {
    let srv = Srv::default();
    let app = Router::new()
        .route("/hello", get(hello))
        .route("/ua", get(user_agent))
        .route("/big", get(big))
        .route("/gzip", get(gzip_bomb))
        .route("/redirect", any(redirect))
        .route(
            "/loop",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/loop")]) }),
        )
        .route("/method", any(method))
        .route("/auth", get(echo_auth))
        .route("/slow", get(slow))
        .route("/trickle", get(trickle))
        .route("/xrpc/com.atproto.sync.getBlob", get(get_blob))
        .route("/xrpc/app.test.private", get(private_xrpc))
        .route("/xrpc/app.test.redirect", get(redirect))
        .with_state(srv.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, srv)
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

struct Env {
    host: NativeHost<MemKv>,
    base: Url,
    port: u16,
    srv: Srv,
    dns: Arc<FixedDns>,
    _tmp: tempfile::TempDir,
}

impl Env {
    async fn new() -> Self {
        let (addr, srv) = serve().await;
        let port = addr.port();
        let base = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let dns = Arc::new(FixedDns::new());
        dns.insert("other.test", vec![vec![ip("127.0.0.1")]]);
        dns.insert("evil.test", vec![vec![ip("192.168.1.1")]]);
        dns.insert("mixed.test", vec![vec![ip("127.0.0.1"), ip("10.1.2.3")]]);
        dns.insert(
            "rebind.test",
            vec![vec![ip("127.0.0.2")], vec![ip("127.0.0.1")]],
        );
        let origins: Vec<Url> = [
            "127.0.0.1",
            "other.test",
            "evil.test",
            "mixed.test",
            "rebind.test",
        ]
        .iter()
        .map(|h| Url::parse(&format!("http://{h}:{port}")).unwrap())
        .collect();
        let net = Net::with_dns(NetPolicy::devnet(&origins), dns.clone()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let host =
            NativeHost::with_parts(MemKv::new(), MemKv::new(), tmp.path().join("blobs"), net);
        Self {
            host,
            base,
            port,
            srv,
            dns,
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> Url {
        self.base.join(path).unwrap()
    }

    async fn get(&self, path: &str) -> Result<HttpResponse, HostError> {
        self.host
            .http(HttpRequest::get(Target::Anon(self.url(path))))
            .await
    }
}

#[tokio::test]
async fn anon_get_with_user_agent_and_lowercase_headers() {
    let env = Env::new().await;
    let r = env.get("/hello").await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"hi");
    assert_eq!(r.header("x-test"), Some("1"));
    let ua = env.get("/ua").await.unwrap();
    assert_eq!(
        String::from_utf8(ua.body).unwrap(),
        lessdugin_native::host::USER_AGENT
    );
    // A name from the DNS table that resolves to an allowed (devnet loopback) address.
    let u = Url::parse(&format!("http://other.test:{}/hello", env.port)).unwrap();
    assert_eq!(
        env.host
            .http(HttpRequest::get(Target::Anon(u)))
            .await
            .unwrap()
            .status,
        200
    );
}

#[tokio::test]
async fn production_policy_blocks_loopback_before_connecting() {
    let env = Env::new().await;
    let dns = Arc::new(FixedDns::new());
    dns.insert("pds.example.com", vec![vec![ip("127.0.0.1")]]);
    let net = Net::with_dns(NetPolicy::production(), dns).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let host = NativeHost::with_parts(MemKv::new(), MemKv::new(), tmp.path(), net);
    for url in [
        env.url("/hello").to_string(),                            // http
        format!("https://127.0.0.1:{}/hello", env.port),          // IP literal
        format!("https://pds.example.com:{}/hello", env.port),    // resolves to loopback
        format!("https://[::ffff:127.0.0.1]:{}/hello", env.port), // v4-mapped literal
    ] {
        let r = host
            .http(HttpRequest::get(Target::Anon(Url::parse(&url).unwrap())))
            .await;
        assert!(matches!(r, Err(HostError::Blocked(_))), "{url}: {r:?}");
    }
    assert_eq!(
        env.srv.hits.load(Ordering::SeqCst),
        0,
        "no request reached the server"
    );
}

#[tokio::test]
async fn private_dns_answers_are_blocked() {
    let env = Env::new().await;
    for name in ["evil.test", "mixed.test"] {
        let u = Url::parse(&format!("http://{name}:{}/hello", env.port)).unwrap();
        let r = env.host.http(HttpRequest::get(Target::Anon(u))).await;
        assert!(matches!(r, Err(HostError::Blocked(_))), "{name}: {r:?}");
    }
    assert_eq!(env.srv.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dns_rebinding_is_pinned() {
    // rebind.test answers 127.0.0.2 first (nothing listens there) and 127.0.0.1 (the server)
    // afterwards. A guard that checked one answer and connected with a second lookup would
    // reach the server; the pinned resolver connects to the checked address only.
    let env = Env::new().await;
    let u = Url::parse(&format!("http://rebind.test:{}/hello", env.port)).unwrap();
    let r = env.host.http(HttpRequest::get(Target::Anon(u))).await;
    assert!(matches!(r, Err(HostError::Network(_))), "{r:?}");
    assert_eq!(
        env.dns.lookups("rebind.test"),
        1,
        "exactly one resolution per connection"
    );
    assert_eq!(env.srv.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn body_cap_counts_bytes_after_decompression() {
    let env = Env::new().await;
    let req =
        |path: &str, max: u64| HttpRequest::get(Target::Anon(env.url(path))).with_max_bytes(max);
    assert_eq!(
        env.host
            .http(req("/big?n=10000", 10_000))
            .await
            .unwrap()
            .body
            .len(),
        10_000
    );
    assert_eq!(
        env.host.http(req("/big?n=10000", 9_999)).await.unwrap_err(),
        HostError::TooLarge
    );
    // ~1 KB on the wire, 1 MiB after gzip: the cap applies to the decompressed bytes.
    assert_eq!(
        env.host.http(req("/gzip", 100_000)).await.unwrap_err(),
        HostError::TooLarge
    );
    let r = env.host.http(req("/gzip", 2 << 20)).await.unwrap();
    assert_eq!(r.body.len(), 1 << 20);
    assert!(r.body.iter().all(|b| *b == 0));
}

#[tokio::test]
async fn redirects_follow_only_when_asked_and_are_rechecked() {
    let env = Env::new().await;
    let anon = |path: &str| HttpRequest::get(Target::Anon(env.url(path)));

    // Not followed by default: the 3xx comes back as is.
    let r = env.host.http(anon("/redirect?to=/hello")).await.unwrap();
    assert_eq!(r.status, 302);
    assert_eq!(r.header("location"), Some("/hello"));

    let r = env
        .host
        .http(anon("/redirect?to=/hello").with_redirects(Redirects::Follow(3)))
        .await;
    assert_eq!(r.unwrap().body, b"hi");

    // A loop exhausts the budget.
    let r = env
        .host
        .http(anon("/loop").with_redirects(Redirects::Follow(3)))
        .await;
    assert!(
        matches!(r, Err(HostError::Blocked(ref m)) if m.contains("redirects")),
        "{r:?}"
    );

    // Every hop passes the guard again: private literal, private DNS answer, unconfigured http.
    let port = env.port;
    for to in [
        "http://10.0.0.1/".to_string(),
        "http://169.254.169.254/latest/meta-data/".to_string(),
        format!("http://evil.test:{port}/hello"),
        format!("http://mixed.test:{port}/hello"),
        format!("http://localhost:{port}/hello"),
        "file:///etc/passwd".to_string(),
    ] {
        let path = format!("/redirect?to={}", urlencode(&to));
        let r = env
            .host
            .http(anon(&path).with_redirects(Redirects::Follow(3)))
            .await;
        assert!(
            matches!(r, Err(HostError::Blocked(_))),
            "redirect to {to}: {r:?}"
        );
    }
    assert_eq!(
        env.srv.hits.load(Ordering::SeqCst),
        1,
        "only the allowed redirect arrived"
    );
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[tokio::test]
async fn redirect_methods_and_sensitive_headers() {
    let env = Env::new().await;
    let post = |status: u16, to: &str| {
        let url = env.url(&format!("/redirect?status={status}&to={}", urlencode(to)));
        HttpRequest::post_json(Target::Anon(url), b"12345".to_vec())
            .with_redirects(Redirects::Follow(2))
    };
    let body = |r: HttpResponse| String::from_utf8(r.body).unwrap();
    assert_eq!(
        body(env.host.http(post(303, "/method")).await.unwrap()),
        "GET 0"
    );
    assert_eq!(
        body(env.host.http(post(302, "/method")).await.unwrap()),
        "GET 0"
    );
    assert_eq!(
        body(env.host.http(post(307, "/method")).await.unwrap()),
        "POST 5"
    );
    assert_eq!(
        body(env.host.http(post(308, "/method")).await.unwrap()),
        "POST 5"
    );

    // Same origin keeps the header; another origin loses it.
    let with_auth = |to: &str| {
        HttpRequest::get(Target::Anon(
            env.url(&format!("/redirect?to={}", urlencode(to))),
        ))
        .with_header("authorization", "Bearer x")
        .with_redirects(Redirects::Follow(1))
    };
    assert_eq!(
        body(env.host.http(with_auth("/auth")).await.unwrap()),
        "Bearer x"
    );
    let other = format!("http://other.test:{}/auth", env.port);
    assert_eq!(
        body(env.host.http(with_auth(&other)).await.unwrap()),
        "none"
    );
}

#[tokio::test]
async fn timeouts() {
    let env = Env::new().await;
    let r = env
        .host
        .http(HttpRequest::get(Target::Anon(env.url("/slow"))).with_timeout_ms(300))
        .await;
    assert_eq!(r.unwrap_err(), HostError::Timeout);
    // The total timeout also covers a body that trickles in.
    let r = env
        .host
        .http(HttpRequest::get(Target::Anon(env.url("/trickle"))).with_timeout_ms(700))
        .await;
    assert_eq!(r.unwrap_err(), HostError::Timeout);
    let r = env
        .host
        .http(HttpRequest::get(Target::Anon(env.url("/trickle"))))
        .await
        .unwrap();
    assert_eq!(r.body, b"xxxxxxxxxx");
}

#[tokio::test]
async fn forbidden_request_headers() {
    let env = Env::new().await;
    let r = env
        .host
        .http(HttpRequest::get(Target::Anon(env.url("/hello"))).with_header("host", "evil.com"))
        .await;
    assert!(matches!(r, Err(HostError::Blocked(_))));
}

struct BearerSession {
    did: Did,
    pds: Url,
    net: Net,
}

impl AuthSession for BearerSession {
    fn account(&self) -> &Did {
        &self.did
    }

    fn send(&self, req: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HostError>> {
        Box::pin(async move {
            let mut r = NetRequest::for_user_pds(&self.pds, req)?;
            r.headers
                .push(("authorization".into(), "Bearer tok".into()));
            self.net.execute(r).await
        })
    }
}

#[tokio::test]
async fn user_pds_requests_go_through_the_account_session() {
    let env = Env::new().await;
    let did = Did::parse("did:plc:alice").unwrap();
    let req = |p: &str| {
        HttpRequest::get(Target::UserPds {
            account: did.clone(),
            path_and_query: p.into(),
        })
    };
    let r = env.host.http(req("/xrpc/app.test.private")).await;
    assert_eq!(
        r.unwrap_err(),
        HostError::AuthRequired("did:plc:alice".into())
    );

    env.host.set_session(Arc::new(BearerSession {
        did: did.clone(),
        pds: env.base.clone(),
        net: env.host.net().clone(),
    }));
    let r = env.host.http(req("/xrpc/app.test.private")).await.unwrap();
    assert_eq!(
        (r.status, r.body.as_slice()),
        (200, b"secret data".as_slice())
    );

    // Never follows redirects, even if the caller asked for them.
    let r = env
        .host
        .http(req("/xrpc/app.test.redirect").with_redirects(Redirects::Follow(5)))
        .await
        .unwrap();
    assert_eq!(r.status, 302);
    assert_eq!(env.srv.hits.load(Ordering::SeqCst), 0);

    // Paths cannot leave the PDS origin.
    let r = env.host.http(req("//evil.test/x")).await;
    assert!(matches!(r, Err(HostError::Blocked(_))), "{r:?}");

    assert!(env.host.remove_session(&did).is_some());
    assert!(matches!(
        env.host.http(req("/xrpc/app.test.private")).await,
        Err(HostError::AuthRequired(_))
    ));
}

#[tokio::test]
async fn fetch_blob_verifies_and_stores() {
    let env = Env::new().await;
    let did = Did::parse("did:plc:alice").unwrap();
    let good = b"an image, honestly".repeat(1000);
    let good_cid = atp_cid::for_raw(&good).to_string();
    let bad_cid = atp_cid::for_raw(b"what the record says").to_string();
    let big = vec![7u8; 50_000];
    let big_cid = atp_cid::for_raw(&big).to_string();
    let err_cid = atp_cid::for_raw(b"server error").to_string();
    {
        let mut blobs = env.srv.blobs.lock().unwrap();
        blobs.insert(good_cid.clone(), (200, good.clone()));
        blobs.insert(bad_cid.clone(), (200, b"something else".to_vec()));
        blobs.insert(big_cid.clone(), (200, big.clone()));
        blobs.insert(err_cid.clone(), (500, b"oops".to_vec()));
    }
    let fetch = |cid: &str, max: u64| {
        env.host
            .fetch_blob(env.base.clone(), did.clone(), cid.to_owned(), max)
    };

    let h = fetch(&good_cid, 1 << 20).await.unwrap();
    assert_eq!(h.cid, good_cid);
    assert_eq!(h.size, good.len() as u64);
    assert_eq!(std::fs::read(&h.locator).unwrap(), good);
    let path = std::path::Path::new(&h.locator);
    assert_eq!(
        path.parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        &good_cid[good_cid.len() - 2..]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(env.srv.blob_hits.load(Ordering::SeqCst), 1);
    // A stored blob is returned without a request.
    assert_eq!(fetch(&good_cid, 10).await.unwrap(), h);
    assert_eq!(env.srv.blob_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        env.host.blobs().read(&good_cid, 3, 5).await.unwrap(),
        good[3..8].to_vec()
    );
    assert_eq!(
        env.host.blobs().open(&good_cid).await.unwrap(),
        Some(h.clone())
    );

    // Hash mismatch, over the cap, error statuses: nothing stored.
    assert_eq!(
        fetch(&bad_cid, 1 << 20).await.unwrap_err(),
        HostError::CidMismatch
    );
    assert_eq!(
        fetch(&big_cid, 49_999).await.unwrap_err(),
        HostError::TooLarge
    );
    assert_eq!(
        fetch(&err_cid, 1 << 20).await.unwrap_err(),
        HostError::HttpStatus(500, None)
    );
    let missing = atp_cid::for_raw(b"never uploaded").to_string();
    assert_eq!(
        fetch(&missing, 1 << 20).await.unwrap_err(),
        HostError::HttpStatus(400, Some("BlobNotFound".into()))
    );
    for cid in [&bad_cid, &big_cid, &err_cid, &missing] {
        assert_eq!(
            env.host.blobs().open(cid).await.unwrap(),
            None,
            "{cid} not stored"
        );
    }
    let tmp_dir = env.host.blobs().dir().join(".tmp");
    assert_eq!(
        std::fs::read_dir(tmp_dir).unwrap().count(),
        0,
        "no temporary files left"
    );

    // Only raw sha2-256 CIDv1 strings are accepted (they double as file names).
    let dag = atp_cid::for_dag_cbor(b"\xa0").to_string();
    assert!(matches!(
        fetch(&dag, 10).await,
        Err(HostError::Unsupported(_))
    ));
    assert!(matches!(
        fetch("../../etc/passwd", 10).await,
        Err(HostError::Unsupported(_))
    ));

    // Big enough with a larger cap; then delete.
    assert_eq!(fetch(&big_cid, 50_000).await.unwrap().size, 50_000);
    env.host.blobs().delete(&big_cid).await.unwrap();
    assert_eq!(env.host.blobs().open(&big_cid).await.unwrap(), None);
}

#[tokio::test]
async fn fetch_blob_does_not_follow_redirects() {
    let env = Env::new().await;
    let did = Did::parse("did:plc:alice").unwrap();
    let pds = env.url("/");
    let cid = atp_cid::for_raw(b"x").to_string();
    env.srv
        .blobs
        .lock()
        .unwrap()
        .insert(cid.clone(), (302, Vec::new()));
    let r = env.host.fetch_blob(pds, did, cid, 100).await;
    assert_eq!(r.unwrap_err(), HostError::HttpStatus(302, None));
    assert_eq!(
        env.srv.hits.load(Ordering::SeqCst),
        0,
        "the Location was not followed"
    );
}

async fn ws_server(messages: Vec<tokio_tungstenite::tungstenite::Message>) -> u16 {
    use futures::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        // Echo the first text message, then send the scripted ones.
        if let Some(Ok(m)) = ws.next().await {
            let _ = ws.send(m).await;
        }
        for m in messages {
            if ws.send(m).await.is_err() {
                return;
            }
        }
        let _ = ws.close(None).await;
    });
    port
}

#[tokio::test]
async fn websocket_caps_messages() {
    use tokio_tungstenite::tungstenite::Message;
    let port = ws_server(vec![
        Message::Text("hello".into()),
        Message::Binary(vec![1, 2, 3].into()),
        Message::Text("y".repeat(2_000).into()),
        Message::Text("never seen".into()),
    ])
    .await;
    let origin = Url::parse(&format!("ws://127.0.0.1:{port}")).unwrap();
    let net = Net::new(NetPolicy::devnet([&origin])).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let host = NativeHost::with_parts(MemKv::new(), MemKv::new(), tmp.path(), net);
    let mut ws = host
        .websocket(origin.join("/subscribe").unwrap(), 1_000)
        .await
        .unwrap();
    ws.send_text("ping".into()).await.unwrap();
    assert_eq!(ws.next().await, Some(Ok(WsMessage::Text("ping".into()))));
    assert_eq!(ws.next().await, Some(Ok(WsMessage::Text("hello".into()))));
    assert_eq!(ws.next().await, Some(Ok(WsMessage::Binary(vec![1, 2, 3]))));
    assert_eq!(ws.next().await, Some(Err(HostError::TooLarge)));
    assert_eq!(
        ws.next().await,
        None,
        "an oversized message ends the stream"
    );
    ws.close().await;

    // Production rules: no ws://, no loopback.
    let prod = NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        tmp.path(),
        Net::new(NetPolicy::production()).unwrap(),
    );
    let r = prod.websocket(origin.clone(), 1_000).await;
    assert!(matches!(r, Err(HostError::Blocked(_))));
    let r = prod
        .websocket(
            Url::parse(&format!("wss://127.0.0.1:{port}/")).unwrap(),
            1_000,
        )
        .await;
    assert!(matches!(r, Err(HostError::Blocked(_))));
}

#[tokio::test]
async fn clock_randomness_sleep_and_dns() {
    let env = Env::new().await;
    let h = &env.host;
    let t = h.now_ms();
    assert!(t > 1_700_000_000_000, "{t}");
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    h.fill_random(&mut a);
    h.fill_random(&mut b);
    assert_ne!(a, b);
    let start = std::time::Instant::now();
    h.sleep(30).await;
    assert!(start.elapsed() >= Duration::from_millis(30));
    assert!(matches!(
        h.resolve_txt_system("_atproto.example.com".into()).await,
        Err(HostError::Unsupported(_))
    ));
}

#[tokio::test]
async fn open_creates_private_stores() {
    use lessdugin_core::host::{KvOp, KvStore};
    let tmp = tempfile::tempdir().unwrap();
    let paths = lessdugin_native::config::Paths::under(tmp.path());
    let cfg = lessdugin_native::config::Config::default();
    let host = NativeHost::open(&paths, &cfg).unwrap();
    host.state()
        .write(vec![KvOp::Put(b"k".to_vec(), b"v".to_vec())])
        .await
        .unwrap();
    assert_eq!(
        host.state().get_many(vec![b"k".to_vec()]).await.unwrap(),
        vec![Some(b"v".to_vec())]
    );
    assert_eq!(
        host.cache().get_many(vec![b"k".to_vec()]).await.unwrap(),
        vec![None]
    );
    assert!(paths.state_db().is_file() && paths.cache_db().is_file() && paths.blobs_dir().is_dir());
    assert!(!host.net().policy().is_devnet());
}

#[tokio::test]
async fn send_http_adapter_is_guarded() {
    let env = Env::new().await;
    let net = env.host.net();
    let req = http::Request::builder()
        .method("POST")
        .uri(env.url("/method").as_str())
        .header("content-length", "5")
        .header("content-type", "application/json")
        .body(b"12345".to_vec())
        .unwrap();
    let resp = net.send_http(req, 1024, 5_000).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body().as_slice(), b"POST 5");

    // Redirects are returned, not followed.
    let req = http::Request::get(env.url("/redirect?to=/hello").as_str())
        .body(Vec::new())
        .unwrap();
    assert_eq!(net.send_http(req, 1024, 5_000).await.unwrap().status(), 302);
    // The guard applies.
    let req = http::Request::get("http://169.254.169.254/latest")
        .body(Vec::new())
        .unwrap();
    assert!(matches!(
        net.send_http(req, 1024, 5_000).await,
        Err(HostError::Blocked(_))
    ));
    let req = http::Request::get(format!("http://evil.test:{}/", env.port))
        .body(Vec::new())
        .unwrap();
    assert!(matches!(
        net.send_http(req, 1024, 5_000).await,
        Err(HostError::Blocked(_))
    ));
    assert_eq!(env.srv.hits.load(Ordering::SeqCst), 0);
}
