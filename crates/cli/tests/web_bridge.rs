// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin web` (`threat-model.md` §6, §7), offline: the real binary on 127.0.0.1, driven
//! with reqwest and raw TCP.
//!
//! - every response, static or bridge, carries the §6.1 CSP (the §6.2 policy under `--dev`)
//!   and the §6.3 headers, and no `Access-Control-*` header; `OPTIONS` is 405;
//! - a `Host` other than `127.0.0.1:<port>` / `[::1]:<port>` gets an empty 403 before routing
//!   (DNS rebinding), also without a Host or with an absolute-form target;
//! - every row of the §7.3 table, in its order: method (405), Origin (403), bearer token (401),
//!   Content-Type (415), body limit (413); `/bridge/fetch` is 404;
//! - a capability is single use, and using it twice revokes its session;
//! - `/bridge/kv` reaches only the `out`, `out.budget`, `loc`, `notif` and `meta` scopes of the
//!   native `state` store, atomically, and persists across restarts;
//! - `/bridge/xrpc` forwards only the AUTH-4 methods, collections and own repo through the
//!   CLI's session (against a tiny PLC + PDS in devnet mode), passing through only the status,
//!   body, `Content-Type`, `Retry-After` and `RateLimit-*`;
//! - `/bridge/login` refuses what it cannot sign in;
//! - the port: `web.port` from the configuration, `--port`, and a busy port is an error.

#![allow(clippy::disallowed_methods)] // a real clock for the fake PDS's tokens

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::Path as UrlPath;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use lessdugin_core::store::key::{Part, encode};
use lessdugin_lexicon::ns::NS;
use serde_json::{Value, json};

const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
const OTHER_DID: &str = "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa";
const HANDLE: &str = "alice.test";
const APP_PASSWORD: &str = "abcd-efgh-ijkl-mnop";
/// One byte over `BRIDGE_JSON_MAX_BODY_BYTES`.
const JSON_LIMIT_PLUS_ONE: usize = 1_048_577;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The §6.1 policy, as the hosted site sends it (`deploy/site/_headers`).
fn production_csp() -> String {
    let text = std::fs::read_to_string(root().join("deploy/site/_headers")).unwrap();
    text.lines()
        .find_map(|l| l.trim().strip_prefix("Content-Security-Policy: "))
        .unwrap()
        .to_owned()
}

/// The §6.2 development policy.
fn dev_csp() -> String {
    production_csp().replace(
        "connect-src 'self' https: wss:;",
        "connect-src 'self' https: wss: http: ws:;",
    )
}

// ---------------------------------------------------------------------------------------------
// HTTP helpers

struct Resp {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&self.body)))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The CSP and the §6.3 headers, and nothing CORS.
    fn assert_policy(&self, csp: &str, what: &str) {
        assert_eq!(self.header("content-security-policy"), Some(csp), "{what}");
        for (name, value) in [
            ("referrer-policy", "no-referrer"),
            ("x-content-type-options", "nosniff"),
            ("cross-origin-opener-policy", "same-origin"),
            (
                "permissions-policy",
                "camera=(), microphone=(), geolocation=(), payment=()",
            ),
        ] {
            assert_eq!(self.header(name), Some(value), "{name} on {what}");
        }
        assert!(
            !self
                .headers
                .keys()
                .any(|k| k.as_str().starts_with("access-control-")),
            "CORS header on {what}"
        );
    }
}

struct Http {
    rt: tokio::runtime::Runtime,
    client: reqwest::Client,
}

impl Http {
    fn new() -> Self {
        Self {
            rt: tokio::runtime::Runtime::new().unwrap(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .unwrap(),
        }
    }

    fn send(&self, rb: reqwest::RequestBuilder) -> Resp {
        self.rt.block_on(async {
            let r = rb.send().await.unwrap();
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let body = r.bytes().await.unwrap().to_vec();
            Resp {
                status,
                headers,
                body,
            }
        })
    }

    fn request(&self, method: Method, url: &str) -> reqwest::RequestBuilder {
        self.client.request(method, url)
    }
}

/// One raw HTTP/1.1 exchange (for requests reqwest cannot make); returns the status and the
/// whole response text.
fn raw(port: u16, request: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    (status, text)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// ---------------------------------------------------------------------------------------------
// The `lessdugin web` process

fn cli(data: &Path, global: &[String]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
    for (k, _) in std::env::vars() {
        if k.starts_with("LESSDUGIN_") {
            c.env_remove(k);
        }
    }
    c.args(global)
        .env("LESSDUGIN_DATA_DIR", data)
        .env("LESSDUGIN_KEYRING", "off")
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env_remove("BROWSER");
    c
}

struct Web {
    child: Child,
    stdin: Option<ChildStdin>,
    links: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    port: u16,
    origin: String,
}

impl Web {
    /// Starts `lessdugin [global] web --no-open [--port port]` and waits for its first link.
    fn start(data: &Path, global: &[String], port: Option<u16>) -> (Self, String) {
        let mut c = cli(data, global);
        c.args(["web", "--no-open"]);
        if let Some(p) = port {
            c.args(["--port", &p.to_string()]);
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, links) = channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.starts_with("http://") && tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let err = stderr.clone();
        let mut pipe = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                err.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        let stdin = child.stdin.take();
        let mut web = Self {
            child,
            stdin,
            links,
            stderr,
            port: 0,
            origin: String::new(),
        };
        let link = web.next_link();
        let port: u16 = link
            .strip_prefix("http://127.0.0.1:")
            .and_then(|r| r.split('/').next())
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("link {link}"));
        web.port = port;
        web.origin = format!("http://127.0.0.1:{}", web.port);
        (web, link)
    }

    fn next_link(&mut self) -> String {
        match self.links.recv_timeout(Duration::from_secs(60)) {
            Ok(l) => l,
            Err(_) => {
                let status = self.child.try_wait().ok().flatten();
                panic!(
                    "no link from lessdugin web (exit {status:?}): {}",
                    self.stderr()
                )
            }
        }
    }

    /// Presses Enter in the `web` terminal: a fresh link.
    fn press_enter(&mut self) -> String {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        self.next_link()
    }

    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    /// A bridge POST with the right Origin.
    fn post(&self, http: &Http, path: &str) -> reqwest::RequestBuilder {
        http.request(Method::POST, &self.url(path))
            .header("origin", &self.origin)
    }

    /// Exchanges a link's capability: the response.
    fn exchange(&self, http: &Http, link: &str) -> Resp {
        let cap = cap_of(link);
        http.send(
            self.post(http, "/bridge/session")
                .header("content-type", "application/json")
                .body(json!({"cap": cap}).to_string()),
        )
    }

    /// A fresh session token.
    fn session(&mut self, http: &Http) -> String {
        let link = self.press_enter();
        let r = self.exchange(http, &link);
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json()["token"].as_str().unwrap().to_owned()
    }

    /// An authenticated JSON bridge POST.
    fn call(&self, http: &Http, path: &str, token: &str, body: &Value) -> Resp {
        http.send(
            self.post(http, path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.to_string()),
        )
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn cap_of(link: &str) -> &str {
    link.split_once("#lessdugin-cap=")
        .unwrap_or_else(|| panic!("no capability in {link}"))
        .1
}

// ---------------------------------------------------------------------------------------------
// A tiny PLC + PDS (devnet mode)

/// One request the PDS saw.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Default)]
struct PdsState {
    next: usize,
    access: HashSet<String>,
    seen: Vec<Seen>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A JWT-shaped token (the client reads `exp`; the signature is fake).
fn jwt(scope: &str, n: usize) -> String {
    let header = b64url(br#"{"typ":"at+jwt","alg":"ES256K"}"#);
    let payload = b64url(
        json!({"scope": scope, "sub": DID, "exp": now_secs() + 7200, "jti": format!("{scope}-{n}")})
            .to_string()
            .as_bytes(),
    );
    format!("{header}.{payload}.sig{n}")
}

fn xrpc_error(status: StatusCode, name: &str) -> Response {
    (status, axum::Json(json!({"error": name, "message": name}))).into_response()
}

fn did_doc(pds: &str) -> Value {
    json!({
        "id": DID,
        "alsoKnownAs": [format!("at://{HANDLE}")],
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

type Shared = Arc<Mutex<PdsState>>;

fn record(st: &Shared, method: &str, path: &str, h: &HeaderMap, body: &[u8]) {
    let headers = h
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    st.lock().unwrap().seen.push(Seen {
        method: method.into(),
        path: path.into(),
        headers,
        body: body.to_vec(),
    });
}

fn authorized(st: &Shared, h: &HeaderMap) -> bool {
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    st.lock().unwrap().access.contains(token)
}

/// The PDS's answer to a repo write or upload: a fixed body, `RateLimit-*` and `Retry-After`
/// (passed through by the bridge), and headers the bridge must drop.
async fn pds_write(
    st: Shared,
    UrlPath(nsid): UrlPath<String>,
    h: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    record(&st, "POST", &format!("/xrpc/{nsid}"), &h, &body);
    if !authorized(&st, &h) {
        return xrpc_error(StatusCode::UNAUTHORIZED, "AuthenticationRequired");
    }
    let limited = serde_json::from_slice::<Value>(&body)
        .ok()
        .is_some_and(|v| v["rkey"] == "limited");
    let (status, answer) = if nsid == "com.atproto.repo.uploadBlob" {
        let mime = h
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        (
            StatusCode::OK,
            json!({"blob": {"$type": "blob", "ref": {"$link": "bafkreigh2akiscaildc"}, "mimeType": mime, "size": body.len()}}),
        )
    } else if limited {
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "RateLimitExceeded", "message": "slow down"}),
        )
    } else {
        (
            StatusCode::OK,
            json!({"uri": format!("at://{DID}/{NS}.post/3kabc"), "cid": "bafyreia"}),
        )
    };
    let mut r = (status, axum::Json(answer)).into_response();
    let hm = r.headers_mut();
    for (k, v) in [
        ("ratelimit-limit", "5000"),
        ("ratelimit-remaining", "4999"),
        ("ratelimit-reset", "1900000000"),
        ("ratelimit-policy", "5000;w=3600"),
        ("retry-after", "7"),
        ("set-cookie", "pds=secret"),
        ("x-pds-internal", "1"),
        ("access-control-allow-origin", "*"),
        ("location", "http://evil.example/"),
    ] {
        hm.insert(
            header::HeaderName::from_static(k),
            header::HeaderValue::from_static(v),
        );
    }
    r
}

struct Pds {
    url: String,
    state: Shared,
    _rt: tokio::runtime::Runtime,
}

impl Pds {
    fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    /// The devnet flags pointing at this server.
    fn global(&self) -> Vec<String> {
        ["--dev", "--plc-url", &self.url, "--pds-url", &self.url]
            .map(str::to_owned)
            .to_vec()
    }
}

fn serve_pds() -> Pds {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let state: Shared = Arc::default();
    let doc = did_doc(&url);
    let (s1, s2, s3) = (state.clone(), state.clone(), state.clone());
    let app = Router::new()
        .route(
            "/{did}",
            get(move |UrlPath(did): UrlPath<String>| {
                let doc = doc.clone();
                async move {
                    if did == DID {
                        axum::Json(doc).into_response()
                    } else {
                        StatusCode::NOT_FOUND.into_response()
                    }
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(
                |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
                    if q.get("handle").map(String::as_str) == Some(HANDLE) {
                        axum::Json(json!({"did": DID})).into_response()
                    } else {
                        xrpc_error(StatusCode::BAD_REQUEST, "HandleNotFound")
                    }
                },
            ),
        )
        .route(
            "/xrpc/com.atproto.server.createSession",
            post(move |axum::Json(body): axum::Json<Value>| {
                let st = s1.clone();
                async move {
                    if body["identifier"] == DID && body["password"] == APP_PASSWORD {
                        let mut s = st.lock().unwrap();
                        s.next += 1;
                        let access = jwt("com.atproto.appPass", s.next);
                        s.access.insert(access.clone());
                        axum::Json(json!({
                            "accessJwt": access,
                            "refreshJwt": jwt("com.atproto.refresh", s.next),
                            "handle": HANDLE, "did": DID, "active": true,
                        }))
                        .into_response()
                    } else {
                        xrpc_error(StatusCode::UNAUTHORIZED, "AuthenticationRequired")
                    }
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(move |h: HeaderMap| {
                let st = s2.clone();
                async move {
                    record(&st, "GET", "/xrpc/com.atproto.server.getSession", &h, b"");
                    if authorized(&st, &h) {
                        axum::Json(json!({"did": DID, "handle": HANDLE, "active": true}))
                            .into_response()
                    } else {
                        xrpc_error(StatusCode::BAD_REQUEST, "InvalidToken")
                    }
                }
            }),
        )
        .route(
            "/xrpc/{nsid}",
            post(move |p: UrlPath<String>, h: HeaderMap, b: axum::body::Bytes| {
                pds_write(s3.clone(), p, h, b)
            }),
        );
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    Pds {
        url,
        state,
        _rt: rt,
    }
}

/// `lessdugin login <DID> --password-stdin` against the PDS.
fn login(pds: &Pds, data: &Path) {
    let mut c = cli(data, &pds.global());
    c.args(["--json", "login", DID, "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{APP_PASSWORD}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "login: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn key(parts: &[&str]) -> String {
    STANDARD.encode(encode(
        &parts.iter().map(|p| Part::from(*p)).collect::<Vec<_>>(),
    ))
}

fn succ(parts: &[&str]) -> String {
    let k = encode(&parts.iter().map(|p| Part::from(*p)).collect::<Vec<_>>());
    STANDARD.encode(lessdugin_core::host::prefix_successor(&k).unwrap())
}

// ---------------------------------------------------------------------------------------------
// Tests

#[test]
fn web_help_lists_its_options() {
    let dir = tempfile::tempdir().unwrap();
    let out = cli(dir.path(), &[])
        .args(["web", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.contains("--port") && help.contains("--no-open"),
        "{help}"
    );
    let out = cli(dir.path(), &[])
        .args(["web", "--port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "port 0 is a usage error");
}

#[test]
fn static_files_and_config_carry_the_policy_headers() {
    let dir = tempfile::tempdir().unwrap();
    let (web, link) = Web::start(dir.path(), &[], Some(free_port()));
    let http = Http::new();
    let csp = production_csp();
    // The printed link: the bridge origin, then a 43-character capability in the fragment.
    assert!(link.starts_with(&format!("{}/#lessdugin-cap=", web.origin)));
    let cap = cap_of(&link);
    assert_eq!(cap.len(), 43);
    assert!(
        cap.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );

    let dist = root().join("web/dist");
    let index = std::fs::read(dist.join("index.html")).ok();
    for path in ["/", "/index.html"] {
        let r = http.send(http.request(Method::GET, &web.url(path)));
        assert_eq!(r.status, 200, "{path}");
        r.assert_policy(&csp, path);
        assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{path}");
        match &index {
            Some(i) => assert_eq!(&r.body, i, "{path} is web/dist/index.html"),
            None => assert!(String::from_utf8_lossy(&r.body).contains("not built into")),
        }
    }
    // A build output (when the SPA is embedded).
    if index.is_some() {
        let asset = std::fs::read_dir(dist.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .find(|n| n.ends_with(".js"))
            .unwrap();
        let r = http.send(http.request(Method::GET, &web.url(&format!("/assets/{asset}"))));
        assert_eq!(r.status, 200);
        r.assert_policy(&csp, &asset);
        assert_eq!(
            r.header("content-type"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            r.body,
            std::fs::read(dist.join("assets").join(&asset)).unwrap()
        );
    }
    // lessdugin-config.json says bridge mode.
    let r = http.send(http.request(Method::GET, &web.url("/lessdugin-config.json")));
    assert_eq!(r.status, 200);
    r.assert_policy(&csp, "config");
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-cache"));
    assert_eq!(r.json(), json!({"oauth": "bridge"}));
    // HEAD, 404, and methods static files do not take.
    let r = http.send(http.request(Method::HEAD, &web.url("/")));
    assert_eq!(r.status, 200);
    assert!(r.body.is_empty());
    r.assert_policy(&csp, "HEAD /");
    let r = http.send(http.request(Method::GET, &web.url("/no/such/file")));
    assert_eq!(r.status, 404);
    r.assert_policy(&csp, "404");
    for m in [Method::POST, Method::PUT, Method::DELETE] {
        let r = http.send(http.request(m.clone(), &web.url("/")));
        assert_eq!(r.status, 405, "{m}");
        r.assert_policy(&csp, "405");
    }
    // Bridge answers: the same headers, and no-store.
    let r = web.exchange(&http, "x#lessdugin-cap=nope");
    assert_eq!(r.status, 403);
    r.assert_policy(&csp, "bridge 403");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let r = web.exchange(&http, &link);
    assert_eq!(r.status, 200);
    r.assert_policy(&csp, "bridge 200");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.header("content-type"), Some("application/json"));
}

#[test]
fn foreign_hosts_are_refused_before_routing() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = Web::start(dir.path(), &[], Some(free_port()));
    let http = Http::new();
    let csp = production_csp();
    let p = web.port;
    let hosts = [
        format!("evil.example:{p}"),
        "evil.example".to_owned(),
        format!("localhost:{p}"),
        "127.0.0.1".to_owned(),
        format!("127.0.0.1:{}", p.wrapping_add(1)),
        format!("127.0.0.1.nip.io:{p}"),
        format!("127.0.0.1:{p}.evil.example"),
        "[::1]".to_owned(),
        format!("0.0.0.0:{p}"),
    ];
    for host in &hosts {
        for (method, path) in [
            (Method::GET, "/"),
            (Method::GET, "/lessdugin-config.json"),
            (Method::GET, "/no/such/file"),
            (Method::POST, "/bridge/session"),
            (Method::POST, "/bridge/kv"),
            (Method::POST, "/bridge/fetch"),
            (Method::OPTIONS, "/bridge/session"),
            (Method::PUT, "/"),
        ] {
            let r = http.send(
                http.request(method.clone(), &web.url(path))
                    .header("host", host)
                    .header("origin", &web.origin)
                    .header("content-type", "application/json")
                    .body("{}"),
            );
            assert_eq!(r.status, 403, "{method} {path} with Host {host}");
            assert!(r.body.is_empty(), "{method} {path} with Host {host}");
            r.assert_policy(&csp, host);
        }
    }
    // [::1]:<port> is the other allowed Host.
    let r = http.send(
        http.request(Method::GET, &web.url("/lessdugin-config.json"))
            .header("host", format!("[::1]:{p}")),
    );
    assert_eq!(r.status, 200);
    // No Host at all (HTTP/1.0), an absolute-form target naming another host, two Hosts.
    let (status, text) = raw(p, "GET / HTTP/1.0\r\n\r\n");
    assert_eq!(status, 403, "{text}");
    let (status, text) = raw(
        p,
        &format!(
            "GET http://evil.example/ HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nConnection: close\r\n\r\n"
        ),
    );
    assert_eq!(status, 403, "{text}");
    let (status, text) = raw(
        p,
        &format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nHost: evil.example\r\nConnection: close\r\n\r\n"
        ),
    );
    assert!(status == 403 || status == 400, "{text}");
    assert!(
        !text.contains("<html") && !text.contains("<!doctype"),
        "{text}"
    );
}

#[test]
fn options_and_cors_preflights_fail() {
    let dir = tempfile::tempdir().unwrap();
    let (web, _) = Web::start(dir.path(), &[], Some(free_port()));
    let http = Http::new();
    let csp = production_csp();
    for path in [
        "/",
        "/index.html",
        "/bridge/session",
        "/bridge/kv",
        "/bridge/xrpc/com.atproto.repo.createRecord",
        "/bridge/fetch",
    ] {
        for origin in ["http://evil.example", web.origin.as_str()] {
            let r = http.send(
                http.request(Method::OPTIONS, &web.url(path))
                    .header("origin", origin)
                    .header("access-control-request-method", "POST")
                    .header(
                        "access-control-request-headers",
                        "authorization, content-type",
                    ),
            );
            assert_eq!(r.status, 405, "OPTIONS {path} from {origin}");
            r.assert_policy(&csp, path);
        }
    }
}

#[test]
fn bridge_checks_run_in_the_order_of_the_7_3_table() {
    let dir = tempfile::tempdir().unwrap();
    let (mut web, _) = Web::start(dir.path(), &[], Some(free_port()));
    let http = Http::new();
    let csp = production_csp();
    let token = web.session(&http);
    let p = web.port;
    let empty_get = json!({"op": "get_many", "keys": []}).to_string();

    struct Case {
        what: &'static str,
        method: Method,
        path: &'static str,
        origin: Vec<String>,
        auth: Option<String>,
        content_type: Option<&'static str>,
        body: Vec<u8>,
        want: u16,
    }
    let good = |what, path, body: &str, want| Case {
        what,
        method: Method::POST,
        path,
        origin: vec![web.origin.clone()],
        auth: Some(format!("Bearer {token}")),
        content_type: Some("application/json"),
        body: body.as_bytes().to_vec(),
        want,
    };
    let big = vec![b' '; JSON_LIMIT_PLUS_ONE];
    let mut cases = vec![
        // Passing every check.
        good("all checks pass", "/bridge/kv", &empty_get, 200),
        Case {
            what: "the [::1] origin is allowed",
            origin: vec![format!("http://[::1]:{p}")],
            ..good("", "/bridge/kv", &empty_get, 200)
        },
        Case {
            what: "a charset parameter is fine",
            content_type: Some("application/json; charset=utf-8"),
            ..good("", "/bridge/kv", &empty_get, 200)
        },
        // #2 method: before Origin, token and the rest.
        Case {
            what: "#2 GET",
            method: Method::GET,
            ..good("", "/bridge/session", "", 405)
        },
        Case {
            what: "#2 PUT with nothing else right",
            method: Method::PUT,
            origin: vec!["http://evil.example".into()],
            auth: None,
            content_type: Some("text/plain"),
            ..good("", "/bridge/kv", "", 405)
        },
        Case {
            what: "#2 DELETE",
            method: Method::DELETE,
            ..good("", "/bridge/xrpc/com.atproto.repo.createRecord", "{}", 405)
        },
        Case {
            what: "#2 GET of xrpc",
            method: Method::GET,
            ..good("", "/bridge/xrpc/com.atproto.server.getSession", "", 405)
        },
        // #3 Origin: before the token.
        Case {
            what: "#3 no Origin",
            origin: vec![],
            ..good("", "/bridge/session", "{}", 403)
        },
        Case {
            what: "#3 foreign Origin, no token",
            origin: vec!["http://evil.example".into()],
            auth: None,
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        Case {
            what: "#3 null Origin",
            origin: vec!["null".into()],
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        Case {
            what: "#3 localhost Origin",
            origin: vec![format!("http://localhost:{p}")],
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        Case {
            what: "#3 other port",
            origin: vec![format!("http://127.0.0.1:{}", p.wrapping_add(1))],
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        Case {
            what: "#3 https",
            origin: vec![format!("https://127.0.0.1:{p}")],
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        Case {
            what: "#3 trailing slash",
            origin: vec![format!("http://127.0.0.1:{p}/")],
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        Case {
            what: "#3 two Origins",
            origin: vec![web.origin.clone(), "http://evil.example".into()],
            ..good("", "/bridge/kv", &empty_get, 403)
        },
        // #4 token: before Content-Type; /bridge/session is exempt.
        Case {
            what: "#4 no token, wrong type",
            auth: None,
            content_type: Some("text/plain"),
            ..good("", "/bridge/kv", &empty_get, 401)
        },
        Case {
            what: "#4 unknown token",
            auth: Some(format!("Bearer {}", "A".repeat(43))),
            ..good("", "/bridge/kv", &empty_get, 401)
        },
        Case {
            what: "#4 lowercase scheme",
            auth: Some(format!("bearer {token}")),
            ..good("", "/bridge/kv", &empty_get, 401)
        },
        Case {
            what: "#4 Basic",
            auth: Some(format!("Basic {token}")),
            ..good("", "/bridge/kv", &empty_get, 401)
        },
        Case {
            what: "#4 bare token",
            auth: Some(token.clone()),
            ..good("", "/bridge/xrpc/com.atproto.repo.createRecord", "{}", 401)
        },
        Case {
            what: "#4 login needs a token",
            auth: None,
            ..good("", "/bridge/login", "{\"actor\":\"alice.test\"}", 401)
        },
        Case {
            what: "#4 /bridge/session needs none",
            auth: None,
            ..good("", "/bridge/session", "{\"cap\":\"nope\"}", 403)
        },
        // #5 Content-Type: before the body limit.
        Case {
            what: "#5 text/plain",
            content_type: Some("text/plain"),
            ..good("", "/bridge/kv", &empty_get, 415)
        },
        Case {
            what: "#5 none",
            content_type: None,
            ..good("", "/bridge/kv", &empty_get, 415)
        },
        Case {
            what: "#5 form",
            content_type: Some("application/x-www-form-urlencoded"),
            ..good("", "/bridge/session", "cap=x", 415)
        },
        Case {
            what: "#5 before #6",
            content_type: Some("text/plain"),
            body: big.clone(),
            ..good("", "/bridge/session", "", 415)
        },
        Case {
            what: "#5 uploadBlob takes a declared media type",
            content_type: Some("application/json"),
            ..good("", "/bridge/xrpc/com.atproto.repo.uploadBlob", "{}", 415)
        },
        Case {
            what: "#5 uploadBlob refuses an undeclared type",
            content_type: Some("image/svg+xml"),
            ..good(
                "",
                "/bridge/xrpc/com.atproto.repo.uploadBlob",
                "<svg/>",
                415,
            )
        },
        // #6 body limits.
        Case {
            what: "#6 session over 1 MiB",
            auth: None,
            body: big.clone(),
            ..good("", "/bridge/session", "", 413)
        },
        Case {
            what: "#6 login over 1 MiB",
            body: big.clone(),
            ..good("", "/bridge/login", "", 413)
        },
        Case {
            what: "#6 xrpc over 1 MiB",
            body: big.clone(),
            ..good("", "/bridge/xrpc/com.atproto.repo.createRecord", "", 413)
        },
        Case {
            what: "#6 kv takes up to 64 MiB (so this is just bad JSON)",
            body: vec![b' '; 2 * JSON_LIMIT_PLUS_ONE],
            ..good("", "/bridge/kv", "", 400)
        },
        // Endpoints that do not exist.
        Case {
            what: "/bridge/fetch is reserved",
            ..good("", "/bridge/fetch", "{}", 404)
        },
        Case {
            what: "/bridge/fetch by GET",
            method: Method::GET,
            auth: None,
            ..good("", "/bridge/fetch", "", 404)
        },
        Case {
            what: "/bridge/xrpc without a method",
            ..good("", "/bridge/xrpc/", "{}", 404)
        },
        Case {
            what: "/bridge",
            ..good("", "/bridge", "{}", 404)
        },
    ];
    for c in cases.iter_mut() {
        if c.what.is_empty() {
            c.what = "?";
        }
        let mut rb = http.request(c.method.clone(), &web.url(c.path));
        for o in &c.origin {
            rb = rb.header("origin", o);
        }
        if let Some(a) = &c.auth {
            rb = rb.header("authorization", a);
        }
        if let Some(t) = c.content_type {
            rb = rb.header("content-type", t);
        }
        let r = http.send(rb.body(c.body.clone()));
        assert_eq!(
            r.status,
            c.want,
            "{}: {} {}: {}",
            c.what,
            c.method,
            c.path,
            String::from_utf8_lossy(&r.body[..r.body.len().min(300)])
        );
        r.assert_policy(&csp, c.what);
        assert_eq!(r.header("cache-control"), Some("no-store"), "{}", c.what);
        if c.want != 200 {
            assert!(r.json()["error"].is_string(), "{}: a JSON error", c.what);
        }
    }
}

#[test]
fn a_link_used_twice_revokes_its_session() {
    let dir = tempfile::tempdir().unwrap();
    let (mut web, link1) = Web::start(dir.path(), &[], Some(free_port()));
    let http = Http::new();
    let get = json!({"op": "get_many", "keys": []});

    let r = web.exchange(&http, &link1);
    assert_eq!(r.status, 200);
    let token1 = r.json()["token"].as_str().unwrap().to_owned();
    assert_eq!(token1.len(), 43);
    assert_ne!(token1, cap_of(&link1));
    assert_eq!(web.call(&http, "/bridge/kv", &token1, &get).status, 200);
    // The capability is not a token.
    assert_eq!(
        web.call(&http, "/bridge/kv", cap_of(&link1), &get).status,
        401
    );

    // A second link from Enter is independent.
    let link2 = web.press_enter();
    assert_ne!(cap_of(&link1), cap_of(&link2));
    let r = web.exchange(&http, &link2);
    assert_eq!(r.status, 200);
    let token2 = r.json()["token"].as_str().unwrap().to_owned();

    // The first link used again: refused, and its session revoked.
    let r = web.exchange(&http, &link1);
    assert_eq!(r.status, 403);
    assert_eq!(r.json()["error"], "auth_required");
    assert_eq!(web.call(&http, "/bridge/kv", &token1, &get).status, 401);
    assert_eq!(web.call(&http, "/bridge/kv", &token2, &get).status, 200);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !web
        .stderr()
        .contains("a lessdugin web link was used twice; its session was revoked")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "no warning: {}",
            web.stderr()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // And it stays refused.
    assert_eq!(web.exchange(&http, &link1).status, 403);
    assert_eq!(web.call(&http, "/bridge/kv", &token1, &get).status, 401);

    // Capabilities nobody minted.
    let mut forged = cap_of(&link2).to_owned().into_bytes();
    forged[0] = if forged[0] == b'A' { b'B' } else { b'A' };
    for cap in [
        String::new(),
        "x".into(),
        "A".repeat(43),
        String::from_utf8(forged).unwrap(),
        format!("{}A", cap_of(&link2)),
    ] {
        let r = http.send(
            web.post(&http, "/bridge/session")
                .header("content-type", "application/json")
                .body(json!({"cap": cap}).to_string()),
        );
        assert_eq!(r.status, 403, "{cap}");
    }
    for body in [
        "{}",
        "{\"cap\": 1}",
        "[]",
        "not json",
        "{\"cap\":\"x\",\"more\":1}",
    ] {
        let r = http.send(
            web.post(&http, "/bridge/session")
                .header("content-type", "application/json")
                .body(body),
        );
        assert_eq!(r.status, 400, "{body}");
    }
    // link2's session survived all of that.
    assert_eq!(web.call(&http, "/bridge/kv", &token2, &get).status, 200);
    // Nothing secret reached the terminal's log lines.
    let err = web.stderr();
    assert!(!err.contains(&token1) && !err.contains(&token2), "{err}");
}

#[test]
fn kv_reaches_only_its_scopes_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let (mut web, _) = Web::start(dir.path(), &[], Some(port));
    let http = Http::new();
    let token = web.session(&http);
    let call = |web: &Web, body: Value| web.call(&http, "/bridge/kv", &token, &body);

    let draft = key(&["loc", "draft", "d1"]);
    let value = STANDARD.encode(b"\x00\x01binary\xff");
    let r = call(
        &web,
        json!({"op": "write", "batch": [{"put": [draft, value]}, {"put": [key(&["out", DID, "x"]), STANDARD.encode(b"w")]}]}),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json(), json!({}));
    let r = call(
        &web,
        json!({"op": "get_many", "keys": [draft, key(&["loc", "draft", "none"])]}),
    );
    assert_eq!(r.json(), json!({"values": [value, null]}));
    let r = call(
        &web,
        json!({"op": "scan", "start": {"included": key(&["loc", "draft"])}, "end": {"excluded": succ(&["loc", "draft"])}, "dir": "forward", "limit": 10}),
    );
    assert_eq!(r.json(), json!({"entries": [[draft, value]]}));
    let r = call(
        &web,
        json!({"op": "scan", "start": {"included": key(&["out"])}, "end": {"excluded": succ(&["out"])}, "dir": "reverse", "limit": 10}),
    );
    assert_eq!(r.json()["entries"].as_array().unwrap().len(), 1);

    // Out of scope: the cache's records, made-up scopes, prefixes of a scope, open ranges,
    // ranges across scopes. A batch with one bad operation applies nothing.
    let forbidden = [
        json!({"op": "get_many", "keys": [key(&["rec", "x"])]}),
        json!({"op": "get_many", "keys": [draft, key(&["session", DID])]}),
        json!({"op": "get_many", "keys": [key(&["outx"])]}),
        json!({"op": "get_many", "keys": [STANDARD.encode(b"\x02lo")]}),
        json!({"op": "get_many", "keys": [""]}),
        json!({"op": "write", "batch": [{"put": [key(&["loc", "draft", "d2"]), value]}, {"put": [key(&["id", "did", DID]), value]}]}),
        json!({"op": "write", "batch": [{"delete": key(&["repo", DID])}]}),
        // the authorization-server pins are readable but never written from the tab
        json!({"op": "write", "batch": [{"put": [key(&["loc", "as_pin", DID]), value]}]}),
        json!({"op": "write", "batch": [{"delete": key(&["loc", "as_pin", DID])}]}),
        json!({"op": "write", "batch": [{"deleteRange": [{"included": key(&["loc"])}, {"excluded": succ(&["loc"])}]}]}),
        json!({"op": "write", "batch": [{"deleteRange": [{"included": key(&["loc"])}, {"excluded": succ(&["out"])}]}]}),
        json!({"op": "scan", "start": {"included": ""}, "end": {"excluded": succ(&["meta"])}, "dir": "forward", "limit": 10}),
        json!({"op": "scan", "start": {"included": key(&["loc"])}, "end": {"included": succ(&["loc"])}, "dir": "forward", "limit": 10}),
    ];
    for body in forbidden {
        let r = call(&web, body.clone());
        assert_eq!(r.status, 403, "{body}");
        assert_eq!(r.json()["error"], "blocked");
    }
    let r = call(
        &web,
        json!({"op": "get_many", "keys": [key(&["loc", "draft", "d2"])]}),
    );
    assert_eq!(r.json(), json!({"values": [null]}), "the batch was atomic");
    let r = call(
        &web,
        json!({"op": "get_many", "keys": [key(&["loc", "as_pin", DID])]}),
    );
    assert_eq!(r.status, 200, "pins can be read");

    // Malformed requests.
    for body in [
        json!({"op": "get_many", "keys": ["not base64!"]}),
        json!({"op": "drop"}),
        json!({"op": "scan", "start": {"included": draft}}),
        json!({"keys": []}),
        json!({"op": "write", "batch": [{"put": [draft]}]}),
    ] {
        let r = call(&web, body.clone());
        assert_eq!(r.status, 400, "{body}");
    }

    // The store is native state.sqlite: it survives a restart (and a fresh link).
    drop(web);
    let (mut web, _) = Web::start(dir.path(), &[], Some(port));
    let token = web.session(&http);
    let r = web.call(
        &http,
        "/bridge/kv",
        &token,
        &json!({"op": "get_many", "keys": [draft]}),
    );
    assert_eq!(r.json(), json!({"values": [value]}));
    // deleteRange inside a scope.
    let r = web.call(
        &http,
        "/bridge/kv",
        &token,
        &json!({"op": "write", "batch": [{"deleteRange": [{"included": key(&["loc", "draft"])}, {"excluded": succ(&["loc", "draft"])}]}]}),
    );
    assert_eq!(r.status, 200);
    let r = web.call(
        &http,
        "/bridge/kv",
        &token,
        &json!({"op": "get_many", "keys": [draft]}),
    );
    assert_eq!(r.json(), json!({"values": [null]}));
}

#[test]
fn xrpc_forwards_only_allowlisted_writes_through_the_cli_session() {
    let pds = serve_pds();
    let dir = tempfile::tempdir().unwrap();
    login(&pds, dir.path());
    let (mut web, _) = Web::start(dir.path(), &pds.global(), Some(free_port()));
    let http = Http::new();
    let token = web.session(&http);
    let account = format!("?account={DID}");
    let xrpc = |nsid: &str, query: &str, body: &Value| {
        web.call(&http, &format!("/bridge/xrpc/{nsid}{query}"), &token, body)
    };
    let post = format!("{NS}.post");
    let comment = format!("{NS}.comment");
    let before = pds.seen().len();

    // A write: forwarded with the CLI's credentials, answered with the PDS's status and body.
    let body = json!({"repo": DID, "collection": post, "record": {"$type": post, "title": "hi"}});
    let r = xrpc("com.atproto.repo.createRecord", &account, &body);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.assert_policy(&dev_csp(), "xrpc (devnet)");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.header("x-lessdugin-upstream"), Some("pds"));
    assert_eq!(r.json()["cid"], "bafyreia");
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("ratelimit-limit"), Some("5000"));
    assert_eq!(r.header("ratelimit-remaining"), Some("4999"));
    assert_eq!(r.header("ratelimit-reset"), Some("1900000000"));
    assert_eq!(r.header("ratelimit-policy"), Some("5000;w=3600"));
    assert_eq!(r.header("retry-after"), Some("7"));
    for dropped in ["set-cookie", "x-pds-internal", "location"] {
        assert!(r.header(dropped).is_none(), "{dropped} passed through");
    }
    let seen = pds.seen();
    let sent = seen.last().unwrap();
    assert_eq!(sent.path, "/xrpc/com.atproto.repo.createRecord");
    assert!(
        sent.headers["authorization"].starts_with("Bearer eyJ"),
        "the CLI's session authenticated it"
    );
    assert_ne!(sent.headers["authorization"], format!("Bearer {token}"));
    assert_eq!(serde_json::from_slice::<Value>(&sent.body).unwrap(), body);
    // The other write methods, and a PDS refusal passed through as is.
    let r = xrpc(
        "com.atproto.repo.applyWrites",
        &account,
        &json!({"repo": DID, "writes": [
            {"$type": "com.atproto.repo.applyWrites#create", "collection": comment, "value": {}},
            {"$type": "com.atproto.repo.applyWrites#delete", "collection": post, "rkey": "3kabc"},
        ]}),
    );
    assert_eq!(r.status, 200);
    let r = xrpc(
        "com.atproto.repo.putRecord",
        &account,
        &json!({"repo": DID, "collection": post, "rkey": "limited", "record": {}}),
    );
    assert_eq!(r.status, 429);
    assert_eq!(r.json()["error"], "RateLimitExceeded");
    assert_eq!(r.header("retry-after"), Some("7"));
    assert_eq!(r.header("x-lessdugin-upstream"), Some("pds"));
    let r = xrpc(
        "com.atproto.repo.deleteRecord",
        &format!("?account={}", DID.replace(':', "%3A")),
        &json!({"repo": DID, "collection": post, "rkey": "3kabc"}),
    );
    assert_eq!(r.status, 200);
    // getSession goes to the PDS as a GET.
    let r = xrpc("com.atproto.server.getSession", &account, &json!({}));
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["did"], DID);
    assert_eq!(pds.seen().last().unwrap().method, "GET");
    // uploadBlob: the raw body with its declared type.
    let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    let r = http.send(
        web.post(
            &http,
            &format!("/bridge/xrpc/com.atproto.repo.uploadBlob{account}"),
        )
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "image/png")
        .body(png.clone()),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["blob"]["mimeType"], "image/png");
    let sent = pds.seen().last().unwrap().clone();
    assert_eq!(sent.body, png);
    assert_eq!(sent.headers["content-type"], "image/png");
    let forwarded = pds.seen().len();
    assert_eq!(forwarded - before, 6);

    // Refused locally: the PDS sees none of these.
    let refusals: Vec<(&str, String, Value, u16)> = vec![
        // methods off the allowlist
        (
            "com.atproto.server.deleteAccount",
            account.clone(),
            json!({"did": DID}),
            403,
        ),
        (
            "com.atproto.server.createSession",
            account.clone(),
            json!({}),
            403,
        ),
        (
            "com.atproto.repo.listRecords",
            account.clone(),
            json!({}),
            403,
        ),
        (
            "com.atproto.identity.updateHandle",
            account.clone(),
            json!({"handle": "x.test"}),
            403,
        ),
        ("app.bsky.feed.post", account.clone(), json!({}), 403),
        (
            "com.atproto.repo.createRecord/../deleteAccount",
            account.clone(),
            json!({}),
            403,
        ),
        // collections that are not ours
        (
            "com.atproto.repo.createRecord",
            account.clone(),
            json!({"repo": DID, "collection": "app.bsky.feed.post", "record": {}}),
            403,
        ),
        (
            "com.atproto.repo.putRecord",
            account.clone(),
            json!({"repo": DID, "collection": format!("{NS}.postx"), "rkey": "a", "record": {}}),
            403,
        ),
        (
            "com.atproto.repo.applyWrites",
            account.clone(),
            json!({"repo": DID, "writes": [{"collection": post, "value": {}}, {"collection": "app.bsky.graph.follow", "value": {}}]}),
            403,
        ),
        (
            "com.atproto.repo.deleteRecord",
            account.clone(),
            json!({"repo": DID, "collection": "app.bsky.actor.profile", "rkey": "self"}),
            403,
        ),
        // another repo
        (
            "com.atproto.repo.createRecord",
            account.clone(),
            json!({"repo": OTHER_DID, "collection": post, "record": {}}),
            403,
        ),
        (
            "com.atproto.repo.createRecord",
            account.clone(),
            json!({"repo": HANDLE, "collection": post, "record": {}}),
            403,
        ),
        (
            "com.atproto.repo.createRecord",
            account.clone(),
            json!({"collection": post, "record": {}}),
            403,
        ),
        // the account parameter
        (
            "com.atproto.repo.createRecord",
            format!("?account={OTHER_DID}"),
            json!({"repo": OTHER_DID, "collection": post, "record": {}}),
            403,
        ),
        (
            "com.atproto.repo.createRecord",
            String::new(),
            json!({"repo": DID, "collection": post, "record": {}}),
            400,
        ),
        (
            "com.atproto.repo.createRecord",
            format!("?account={HANDLE}"),
            json!({"repo": DID, "collection": post, "record": {}}),
            400,
        ),
        (
            "com.atproto.repo.createRecord",
            format!("{account}&account={DID}"),
            json!({"repo": DID, "collection": post, "record": {}}),
            400,
        ),
        // not a JSON object
        (
            "com.atproto.repo.createRecord",
            account.clone(),
            json!([1, 2]),
            400,
        ),
    ];
    for (nsid, query, body, want) in &refusals {
        let r = xrpc(nsid, query, body);
        assert_eq!(
            r.status,
            *want,
            "{nsid}{query} {body}: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert!(r.header("x-lessdugin-upstream").is_none());
        assert!(r.json()["error"].is_string());
    }
    let r = xrpc(
        "com.atproto.repo.createRecord",
        &format!("?account={OTHER_DID}"),
        &json!({}),
    );
    assert_eq!(r.json()["error"], "auth_required");
    // uploadBlob whose content is not its declared type.
    for (ctype, bytes) in [
        ("image/png", b"\xff\xd8\xff\xe0jpeg".to_vec()),
        ("image/jpeg", b"<svg onload=alert(1)>".to_vec()),
        ("text/markdown", b"\xff\xfe\x00".to_vec()),
    ] {
        let r = http.send(
            web.post(
                &http,
                &format!("/bridge/xrpc/com.atproto.repo.uploadBlob{account}"),
            )
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", ctype)
            .body(bytes),
        );
        assert_eq!(r.status, 415, "{ctype}");
    }
    assert_eq!(pds.seen().len(), forwarded, "the PDS saw a refused request");
}

#[test]
fn login_refuses_what_it_cannot_sign_in() {
    let pds = serve_pds();
    let dir = tempfile::tempdir().unwrap();
    let (mut web, _) = Web::start(dir.path(), &pds.global(), Some(free_port()));
    let http = Http::new();
    let token = web.session(&http);
    for (body, want) in [
        (json!({}), 400),
        (json!({"actor": ""}), 400),
        (json!({"actor": 7}), 400),
        (json!({"actor": "alice.test", "password": "x"}), 400),
        (json!({"actor": APP_PASSWORD}), 400),
    ] {
        let r = web.call(&http, "/bridge/login", &token, &body);
        assert_eq!(
            r.status,
            want,
            "{body}: {}",
            String::from_utf8_lossy(&r.body)
        );
    }
    // An unknown handle, and a PDS without OAuth: JSON errors, no sign-in started.
    for actor in ["nobody.test", DID] {
        let r = web.call(&http, "/bridge/login", &token, &json!({"actor": actor}));
        assert!(
            (400..600).contains(&r.status),
            "{actor}: {} {}",
            r.status,
            String::from_utf8_lossy(&r.body)
        );
        assert!(r.json()["error"].is_string(), "{actor}");
        assert!(r.json().get("url").is_none());
    }
    assert!(!web.stderr().contains("started a sign-in"));
}

#[test]
fn the_port_comes_from_the_flag_or_web_port_and_a_busy_port_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // web.port in config.toml.
    let configured = free_port();
    std::fs::write(
        dir.path().join("config.toml"),
        format!("[web]\nport = {configured}\n"),
    )
    .unwrap();
    {
        let (web, link) = Web::start(dir.path(), &[], None);
        assert_eq!(web.port, configured);
        assert!(link.starts_with(&format!("http://127.0.0.1:{configured}/#lessdugin-cap=")));
        // --port overrides it.
        let flag = free_port();
        let (web2, _) = Web::start(dir.path(), &[], Some(flag));
        assert_eq!(web2.port, flag);
        // The configured port is now busy: an error, never another port.
        let out = cli(dir.path(), &[])
            .args(["web", "--no-open"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("in use"), "{err}");
        assert!(out.stdout.is_empty(), "no link was printed");
    }
    // A port another program holds.
    let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = other.local_addr().unwrap().port();
    let out = cli(dir.path(), &[])
        .args(["web", "--no-open", "--port", &busy.to_string()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("in use"));
    // A bad web.port is a usage error.
    std::fs::write(dir.path().join("config.toml"), "[web]\nport = 0\n").unwrap();
    let out = cli(dir.path(), &[])
        .args(["web", "--no-open"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
