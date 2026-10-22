// SPDX-License-Identifier: AGPL-3.0-or-later
//! M1 acceptance (`docs/plan.md` §3 M1), offline: "Concurrent refresh is safe: parallel CLI
//! processes refreshing one session never invalidate it."
//!
//! [`PROCESSES`] `lessdugin whoami` processes share one data directory and one stored
//! app-password session (the 0600 session file; the OS keyring is off), against a tiny PLC +
//! PDS on 127.0.0.1 (the CLI in devnet mode) with the strictest refresh rules: a refresh token
//! is single use, and presenting a spent one revokes every token of the session (the reference
//! PDS's OAuth replay detection; its app-password refresh tokens have a grace period this
//! server does not give). `refreshSession` answers slowly, so the processes pile up on the
//! account's refresh lock.
//!
//! - Proactive: the stored access token is marked expired (`accessExpMs` rewritten in the
//!   session file, which the server knows nothing about).
//! - Reactive: the server stops accepting the access token (`400 InvalidToken`) while the stored
//!   one still looks valid; `refreshSession` is held until every process has been refused, so
//!   all of them have loaded the old session and race for the refresh.
//!
//! After each race: every process succeeded, the server saw exactly one `refreshSession` and no
//! spent refresh token, the stored session holds the one live refresh token, and the session
//! still works (another `whoami`, and another forced refresh).
//!
//! The same race against the reference PDS (app password and OAuth) is in `e2e_devnet.rs`
//! (`LESSDUGIN_E2E=1`).

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for real processes

#[path = "support/race.rs"]
mod race;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use race::{ADOPTED, race, request_line};

const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
const HANDLE: &str = "alice.test";
const APP_PASSWORD: &str = "abcd-efgh-ijkl-mnop";
/// How many processes race.
const PROCESSES: usize = 8;
/// How long `refreshSession` takes to answer (the others wait on the refresh lock meanwhile).
const REFRESH_DELAY: Duration = Duration::from_millis(1000);
/// The longest `refreshSession` waits for every process to have been refused (reactive race).
const GATE_TIMEOUT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------------------------
// The PLC + PDS

#[derive(Default)]
struct State {
    next: usize,
    /// Access tokens the server accepts.
    access: HashSet<String>,
    /// Refresh tokens not used yet.
    live_refresh: HashSet<String>,
    /// Refresh tokens already used once.
    spent_refresh: HashSet<String>,
    /// Successful `refreshSession` calls.
    refreshes: usize,
    /// `refreshSession` calls with a spent refresh token (each revoked the session).
    replays: usize,
    /// `getSession` calls refused for their access token.
    refused: usize,
    /// `refreshSession` waits until `refused` reaches this (0: no wait).
    gate: usize,
}

#[allow(clippy::disallowed_methods)]
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// base64url without padding.
fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// A JWT-shaped token (the client reads `exp`; the signature is fake).
fn jwt(scope: &str, n: usize, exp_secs: i64) -> String {
    let header = b64url(br#"{"typ":"at+jwt","alg":"ES256K"}"#);
    let payload = b64url(
        json!({"scope": scope, "sub": DID, "exp": exp_secs, "jti": format!("{scope}-{n}")})
            .to_string()
            .as_bytes(),
    );
    format!("{header}.{payload}.sig{n}")
}

impl State {
    fn issue(&mut self) -> Value {
        self.next += 1;
        let access = jwt("com.atproto.appPass", self.next, now_secs() + 7200);
        let refresh = jwt("com.atproto.refresh", self.next, now_secs() + 90 * 86400);
        self.access.insert(access.clone());
        self.live_refresh.insert(refresh.clone());
        json!({"accessJwt": access, "refreshJwt": refresh, "handle": HANDLE, "did": DID, "active": true})
    }
}

fn bearer(h: &HeaderMap) -> String {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned()
}

fn xrpc_error(name: &str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({"error": name, "message": message})),
    )
        .into_response()
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

type Shared = Arc<Mutex<State>>;

async fn refresh_session(st: Shared, h: HeaderMap) -> Response {
    let token = bearer(&h);
    // The token is spent on arrival, however long the answer takes.
    let answer = {
        let mut s = st.lock().unwrap();
        if s.live_refresh.remove(&token) {
            s.spent_refresh.insert(token);
            s.refreshes += 1;
            Ok(s.issue())
        } else if s.spent_refresh.contains(&token) {
            // Replay: the whole session is revoked.
            s.replays += 1;
            s.access.clear();
            s.live_refresh.clear();
            Err(xrpc_error("ExpiredToken", "Token has been revoked"))
        } else {
            Err(xrpc_error("InvalidToken", "Token could not be verified"))
        }
    };
    let started = Instant::now();
    loop {
        let (refused, gate) = {
            let s = st.lock().unwrap();
            (s.refused, s.gate)
        };
        if refused >= gate || started.elapsed() > GATE_TIMEOUT {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(REFRESH_DELAY).await;
    match answer {
        Ok(v) => axum::Json(v).into_response(),
        Err(r) => r,
    }
}

struct Server {
    url: String,
    state: Shared,
    _rt: tokio::runtime::Runtime,
}

fn serve() -> Server {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let state: Shared = Arc::default();
    let (s1, s2, s3) = (state.clone(), state.clone(), state.clone());
    let doc = did_doc(&url);
    let app = Router::new()
        // The PLC directory.
        .route(
            "/{did}",
            get(move |axum::extract::Path(did): axum::extract::Path<String>| {
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
        // The devnet PDS resolves handles.
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(
                |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
                    if q.get("handle").map(String::as_str) == Some(HANDLE) {
                        axum::Json(json!({"did": DID})).into_response()
                    } else {
                        xrpc_error("HandleNotFound", "Unable to resolve handle")
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
                        axum::Json(st.lock().unwrap().issue()).into_response()
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            axum::Json(json!({"error": "AuthenticationRequired"})),
                        )
                            .into_response()
                    }
                }
            }),
        )
        .route(
            "/xrpc/com.atproto.server.refreshSession",
            post(move |h: HeaderMap| refresh_session(s2.clone(), h)),
        )
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(move |h: HeaderMap| {
                let st = s3.clone();
                async move {
                    let mut s = st.lock().unwrap();
                    if s.access.contains(&bearer(&h)) {
                        axum::Json(json!({"did": DID, "handle": HANDLE, "active": true}))
                            .into_response()
                    } else {
                        s.refused += 1;
                        xrpc_error("InvalidToken", "Token could not be verified")
                    }
                }
            }),
        );
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        url,
        state,
        _rt: rt,
    }
}

// ---------------------------------------------------------------------------------------------
// The CLI

/// `lessdugin --dev` against the server, with its own data directory and no OS keyring.
fn cli(server: &Server, data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
    for (k, _) in std::env::vars() {
        if k.starts_with("LESSDUGIN_") {
            c.env_remove(k);
        }
    }
    c.args(["--dev", "--plc-url", &server.url, "--pds-url", &server.url])
        .env("LESSDUGIN_DATA_DIR", data)
        .env("LESSDUGIN_KEYRING", "off")
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env_remove("BROWSER");
    c
}

fn whoami(server: &Server, data: &Path) -> Command {
    let mut c = cli(server, data);
    c.args(["--json", "whoami"]);
    c
}

fn ok_json(out: &Output, what: &str) -> Value {
    assert!(
        out.status.success(),
        "{what} failed ({:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{what}: {e}: {}", String::from_utf8_lossy(&out.stdout)))
}

/// The session directory of the devnet data under `data` (exactly one).
fn sessions_dir(data: &Path) -> PathBuf {
    let dirs: Vec<_> = std::fs::read_dir(data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("devnet-"))
        })
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].join("sessions")
}

/// The one file in the session directory (no temporary file left behind).
fn session_file(data: &Path) -> PathBuf {
    let mut files: Vec<_> = std::fs::read_dir(sessions_dir(data))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.pop().unwrap()
}

fn read_session(data: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(session_file(data)).unwrap()).unwrap()
}

/// Rewrites the stored session file (keeping it private).
fn edit_session(data: &Path, f: impl FnOnce(&mut Value)) {
    let path = session_file(data);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    f(&mut v);
    lessdugin_native::fsutil::write_private_atomic(&path, &serde_json::to_vec(&v).unwrap())
        .unwrap();
}

fn login(server: &Server, data: &Path) {
    let mut c = cli(server, data);
    c.args(["--json", "login", DID, "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(format!("{APP_PASSWORD}\n").as_bytes())
            .unwrap();
    }
    let account = ok_json(&child.wait_with_output().unwrap(), "login");
    assert_eq!(account["did"], DID);
    assert_eq!(account["authKind"], "appPassword");
}

/// Races [`PROCESSES`] `whoami`s and checks the outcome; returns how many processes adopted
/// the session refreshed by another one.
fn race_whoami(server: &Server, data: &Path, what: &str) -> usize {
    let before = server.state.lock().unwrap().refreshes;
    let r = race((0..PROCESSES).map(|_| whoami(server, data)).collect());
    let refresh = request_line(
        "POST",
        &format!("{}/xrpc/com.atproto.server.refreshSession", server.url),
    );
    let summary = r.summary(&refresh);
    eprintln!("{what}: {summary}");
    for me in r.all_ok_json(what) {
        assert_eq!(me["did"], DID, "{what}");
    }
    let s = server.state.lock().unwrap();
    assert_eq!(s.replays, 0, "{what}: a spent refresh token was presented");
    assert_eq!(
        s.refreshes,
        before + 1,
        "{what}: exactly one refresh: {summary}\n{}",
        r.logs()
    );
    assert_eq!(
        r.count(&refresh),
        (1, 1),
        "{what}: one process sent refreshSession: {summary}"
    );
    // The stored session holds the one live refresh token.
    assert_eq!(s.live_refresh.len(), 1);
    let stored = read_session(data);
    assert!(
        s.live_refresh
            .contains(stored["refreshJwt"].as_str().unwrap()),
        "{what}: the stored refresh token is the live one"
    );
    r.count(ADOPTED).0
}

/// The session still works: a `whoami` without a refresh, then one after a forced refresh
/// (which only succeeds with the live refresh token).
fn session_still_works(server: &Server, data: &Path, what: &str) {
    let before = server.state.lock().unwrap().refreshes;
    let me = ok_json(&whoami(server, data).output().unwrap(), what);
    assert_eq!(me["did"], DID);
    assert_eq!(server.state.lock().unwrap().refreshes, before, "{what}");
    edit_session(data, |v| v["accessExpMs"] = json!(1));
    let me = ok_json(&whoami(server, data).output().unwrap(), what);
    assert_eq!(me["did"], DID);
    let s = server.state.lock().unwrap();
    assert_eq!(s.refreshes, before + 1, "{what}");
    assert_eq!(s.replays, 0, "{what}");
}

#[test]
fn parallel_processes_refresh_one_session_once() {
    let server = serve();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    login(&server, data);
    let me = ok_json(&whoami(&server, data).output().unwrap(), "whoami");
    assert_eq!(me["did"], DID);
    assert_eq!(server.state.lock().unwrap().refreshes, 0);

    // ---- Proactive: the stored access token is marked expired --------------------------------
    edit_session(data, |v| v["accessExpMs"] = json!(1));
    let adopted = race_whoami(&server, data, "proactive");
    // The others were still waiting on the refresh lock when the refresh was saved (the
    // server took a second to answer): the race really happened.
    assert!(adopted >= 1, "proactive: no process adopted the refresh");
    session_still_works(&server, data, "after the proactive race");

    // ---- Reactive: the server refuses the access token ---------------------------------------
    {
        let mut s = server.state.lock().unwrap();
        s.access.clear();
        s.refused = 0;
        s.gate = PROCESSES;
    }
    let adopted = race_whoami(&server, data, "reactive");
    // Every process was refused before the refresh was answered: one refreshed, the others
    // adopted its session.
    assert_eq!(adopted, PROCESSES - 1, "reactive");
    server.state.lock().unwrap().gate = 0;
    session_still_works(&server, data, "after the reactive race");
}
