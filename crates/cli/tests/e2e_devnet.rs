// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end tests of `lessdugin` against the local devnet (`packages/e2e/devnet.mjs`,
//! ADR 0006, ADR 0007). They run only with `LESSDUGIN_E2E=1`, so plain `cargo test` stays
//! offline. `LESSDUGIN_DEVNET_JSON=<file>` reuses a running devnet; otherwise one is started
//! and killed at the end.
//!
//! 1. App password: `login --password-stdin` → `whoami` → `resolve alice.test` → a record
//!    written with raw XRPC → `verify` is `proven` (and a missing rkey `absent`) → proactive
//!    and reactive token refresh → `logout`.
//! 2. OAuth, `--no-browser`: headless Chromium (Playwright) signs in as alice and approves; the
//!    failed redirect is pasted back → `whoami` (DPoP) → a write through the stored OAuth
//!    session (DPoP `putRecord`) → `verify` → refresh → `logout` (revocation).
//! 3. OAuth, loopback: the browser's redirect reaches the CLI's listener on 127.0.0.1.
//!
//! `e2e_concurrent_refresh_against_devnet` is the M1 acceptance test "parallel CLI processes
//! refreshing one session never invalidate it" against the reference PDS, for an app-password
//! and an OAuth session (the offline variant is `concurrent_refresh.rs`).

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

#[path = "support/race.rs"]
mod race;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{HttpRequest, Target};
use lessdugin_native::auth::oauth::OAuthSession;
use lessdugin_native::auth::{SecretStore, StoredSession};
use lessdugin_native::config::{DevnetProfile, FileLock, Paths};
use lessdugin_native::host::{AuthSession, Net, NetPolicy};
use serde_json::{Value, json};

const COLLECTION: &str = "online.7778777.lessdugin.temp.post";

struct Devnet {
    info: Value,
    _child: Option<lessdugin_testkit::node_devnet::NodeDevnet>,
}

impl Devnet {
    fn start() -> Option<Self> {
        if std::env::var("LESSDUGIN_E2E").ok().as_deref() != Some("1") {
            eprintln!("skipped: set LESSDUGIN_E2E=1 to run against the devnet");
            return None;
        }
        if let Ok(path) = std::env::var("LESSDUGIN_DEVNET_JSON") {
            let info = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            return Some(Self { info, _child: None });
        }
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        // one devnet per test: the tests of a binary run in parallel
        let (child, info) = lessdugin_testkit::node_devnet::NodeDevnet::start(root, "cli");
        Some(Self {
            info,
            _child: Some(child),
        })
    }

    fn s(&self, path: &[&str]) -> String {
        let mut v = &self.info;
        for p in path {
            v = &v[*p];
        }
        v.as_str()
            .unwrap_or_else(|| panic!("devnet.{}", path.join(".")))
            .to_owned()
    }

    fn pds(&self) -> String {
        self.s(&["pds"])
    }

    fn plc(&self) -> String {
        self.s(&["plc"])
    }

    /// `lessdugin --dev` with its own data directory and no OS keyring (`--dev` has no
    /// environment variable; the devnet URLs do).
    fn cli(&self, data: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
        for (k, _) in std::env::vars() {
            if k.starts_with("LESSDUGIN_") {
                c.env_remove(k);
            }
        }
        c.arg("--dev")
            .env("LESSDUGIN_DATA_DIR", data)
            .env("LESSDUGIN_KEYRING", "off")
            .env("LESSDUGIN_PLC_URL", self.plc())
            .env("LESSDUGIN_PDS_URL", self.pds())
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG");
        c
    }

    /// The paths the CLI uses for this devnet under `data` (its own `devnet-<id>` directories).
    fn paths(&self, data: &Path) -> Paths {
        Paths::under(data).for_devnet(&DevnetProfile {
            plc: url::Url::parse(&self.plc()).unwrap(),
            pds: url::Url::parse(&self.pds()).unwrap(),
            jetstream: None,
        })
    }
}

/// The devnet data directory the CLI created under `data` (exactly one).
fn devnet_data(data: &Path) -> PathBuf {
    let mut dirs: Vec<_> = std::fs::read_dir(data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("devnet-"))
        })
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    assert!(
        !data.join("state.sqlite").exists(),
        "devnet runs never touch the production stores"
    );
    dirs.pop().unwrap()
}

fn run(cmd: &mut Command, stdin: Option<&str>) -> Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("lessdugin");
    {
        let mut si = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).unwrap();
        }
    }
    child.wait_with_output().unwrap()
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

fn session_file(data: &Path) -> PathBuf {
    let dir = devnet_data(data).join("sessions");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    files.pop().unwrap()
}

/// Rewrites the stored session file (keeping it private).
fn edit_session(data: &Path, f: impl FnOnce(&mut Value)) {
    let path = session_file(data);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    f(&mut v);
    lessdugin_native::fsutil::write_private_atomic(&path, &serde_json::to_vec(&v).unwrap())
        .unwrap();
}

fn read_session(data: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(session_file(data)).unwrap()).unwrap()
}

/// A record written with plain XRPC and an app-password session.
async fn put_record_raw(pds: &str, did: &str, app_password: &str, rkey: &str) -> String {
    let http = reqwest::Client::new();
    let session: Value = http
        .post(format!("{pds}/xrpc/com.atproto.server.createSession"))
        .json(&json!({"identifier": did, "password": app_password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jwt = session["accessJwt"].as_str().unwrap();
    let resp: Value = http
        .post(format!("{pds}/xrpc/com.atproto.repo.putRecord"))
        .bearer_auth(jwt)
        .json(&json!({
            "repo": did,
            "collection": COLLECTION,
            "rkey": rkey,
            "record": {"$type": COLLECTION, "text": "hello from the e2e test", "createdAt": "2026-10-16T00:00:00.000Z"},
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("putRecord: {resp}"))
        .to_owned()
}

fn verify_proven(devnet: &Devnet, data: &Path, uri: &str) {
    let out = run(devnet.cli(data).args(["--json", "verify", uri]), None);
    let report = ok_json(&out, "verify");
    assert_eq!(report["result"], "proven", "{report}");
    assert_eq!(report["uri"], uri);
    let out = run(devnet.cli(data).args(["verify", uri]), None);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("proven"));
}

fn playwright(url: &str, password: &str, mode: &str) -> String {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/oauth_approve.mjs"
    );
    let mut cmd = Command::new("node");
    cmd.arg(script).arg(url).arg(password).arg(mode);
    if std::env::var_os("PLAYWRIGHT_MODULE").is_none()
        && Path::new("/opt/node22/lib/node_modules/playwright").exists()
    {
        cmd.env(
            "PLAYWRIGHT_MODULE",
            "/opt/node22/lib/node_modules/playwright",
        );
    }
    if std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").is_none()
        && Path::new("/opt/pw-browsers").exists()
    {
        cmd.env("PLAYWRIGHT_BROWSERS_PATH", "/opt/pw-browsers");
    }
    let out = cmd.output().expect("node");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "playwright: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// Starts `lessdugin login …` and returns the child and the authorization URL it printed.
// The child is returned to the caller, which waits for it.
#[allow(clippy::zombie_processes)]
fn start_oauth_login(
    cmd: &mut Command,
    pds: &str,
) -> (Child, BufReader<std::process::ChildStderr>, String) {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("lessdugin");
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let prefix = format!("{pds}/oauth/authorize?");
    let mut seen = String::new();
    loop {
        let mut line = String::new();
        let n = stderr.read_line(&mut line).unwrap();
        assert!(n > 0, "login exited before printing a URL: {seen}");
        seen.push_str(&line);
        if line.starts_with(&prefix) {
            return (child, stderr, line.trim().to_owned());
        }
    }
}

#[test]
fn e2e_cli_against_devnet() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pds = devnet.pds();
    let alice_did = devnet.s(&["accounts", "alice", "did"]);
    let alice_app_pw = devnet.s(&["accounts", "alice", "appPassword"]);

    // ---- 1. App password -------------------------------------------------------------------
    let a = tempfile::tempdir().unwrap();
    let data = a.path();
    let out = run(
        devnet
            .cli(data)
            .args(["--json", "login", "alice.test", "--password-stdin"]),
        Some(&format!("{alice_app_pw}\n")),
    );
    let account = ok_json(&out, "app-password login");
    assert_eq!(account["dto"], 1);
    assert_eq!(account["did"], alice_did.as_str());
    assert_eq!(account["handle"], "alice.test");
    assert_eq!(account["authKind"], "appPassword");
    assert_eq!(account["active"], true);
    // under --json the warning is a JSON line
    assert!(String::from_utf8_lossy(&out.stderr).contains("\"warning\":\"devnet mode"));
    assert_eq!(devnet_data(data), devnet.paths(data).data_dir);
    assert!(!String::from_utf8_lossy(&out.stderr).contains(&alice_app_pw));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(session_file(data))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let me = ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        "whoami",
    );
    assert_eq!(me["did"], alice_did.as_str());
    assert_eq!(me["dto"], 1);

    let r = ok_json(
        &run(
            devnet.cli(data).args(["--json", "resolve", "alice.test"]),
            None,
        ),
        "resolve",
    );
    assert_eq!(r["dto"], 1);
    assert_eq!(r["identity"]["did"], alice_did.as_str());
    assert_eq!(r["identity"]["handleState"], "verified");
    assert_eq!(r["identity"]["pds"], pds.as_str());

    let uri = rt.block_on(put_record_raw(&pds, &alice_did, &alice_app_pw, "e2eapppw1"));
    verify_proven(&devnet, data, &uri);
    let absent = format!("at://alice.test/{COLLECTION}/doesnotexist");
    let report = ok_json(
        &run(devnet.cli(data).args(["--json", "verify", &absent]), None),
        "verify absent",
    );
    assert_eq!(report["result"], "absent", "{report}");

    // Proactive refresh: the stored access token is marked expired.
    let before = read_session(data)["refreshJwt"].clone();
    edit_session(data, |v| v["accessExpMs"] = json!(1));
    ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        "whoami after expiry",
    );
    let after = read_session(data);
    assert_ne!(
        after["refreshJwt"], before,
        "the session was refreshed and saved"
    );
    // Reactive refresh: the PDS rejects the access token (400 InvalidToken).
    edit_session(data, |v| {
        v["accessJwt"] = json!("eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQwMDAwMDAwMDB9.invalid");
        v["accessExpMs"] = json!(4_000_000_000_000i64);
    });
    ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        "whoami after rejection",
    );
    assert_ne!(read_session(data)["refreshJwt"], after["refreshJwt"]);

    let list = ok_json(
        &run(devnet.cli(data).args(["--json", "accounts"]), None),
        "accounts",
    );
    assert_eq!(list["dto"], 1);
    assert_eq!(list["accounts"].as_array().unwrap().len(), 1);
    let bye = ok_json(
        &run(devnet.cli(data).args(["--json", "logout"]), None),
        "logout",
    );
    assert_eq!(bye["revoked"], true);
    let out = run(devnet.cli(data).args(["whoami"]), None);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        std::fs::read_dir(devnet_data(data).join("sessions"))
            .unwrap()
            .next()
            .is_none()
    );

    // ---- 2. OAuth, --no-browser (paste-back) -------------------------------------------------
    let b = tempfile::tempdir().unwrap();
    let data = b.path();
    let (mut child, _stderr, url) = start_oauth_login(
        devnet
            .cli(data)
            .args(["--json", "login", "alice.test", "--no-browser"]),
        &pds,
    );
    assert!(
        url.contains("client_id=http%3A%2F%2Flocalhost%3Fredirect_uri%3D"),
        "{url}"
    );
    let pw_out = playwright(
        &url,
        &devnet.s(&["accounts", "alice", "password"]),
        "capture",
    );
    let redirect = pw_out
        .lines()
        .find_map(|l| l.strip_prefix("REDIRECT "))
        .expect("redirect")
        .to_owned();
    assert!(redirect.starts_with("http://127.0.0.1:") && redirect.contains("code="));
    {
        let mut si = child.stdin.take().unwrap();
        si.write_all(format!("{redirect}\n").as_bytes()).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let account = ok_json(&out, "oauth login (no browser)");
    assert_eq!(account["did"], alice_did.as_str());
    assert_eq!(account["authKind"], "oauth");
    let stored = read_session(data);
    assert_eq!(stored["kind"], "oauth");
    assert_eq!(stored["issuer"], pds.as_str());
    assert!(
        stored["scope"]
            .as_str()
            .unwrap()
            .starts_with("atproto repo?collection=")
    );
    assert!(!stored["scope"].as_str().unwrap().contains("transition:"));

    let me = ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        "oauth whoami",
    );
    assert_eq!(me["authKind"], "oauth");

    // A write through the stored OAuth session (DPoP), with the native facade.
    let uri = rt.block_on(async {
        let paths = devnet.paths(data);
        let store = SecretStore::file_only(paths.data_dir.join("sessions"));
        let Some(StoredSession::Oauth(stored)) = store.load(&alice_did).await.unwrap() else {
            panic!("no OAuth session");
        };
        let plc = url::Url::parse(&devnet.plc()).unwrap();
        let pds_url = url::Url::parse(&pds).unwrap();
        let net = Net::new(NetPolicy::devnet([&plc, &pds_url])).unwrap();
        let session = OAuthSession::new(stored, net, store, &paths).unwrap();
        let body = serde_json::to_vec(&json!({
            "repo": alice_did,
            "collection": COLLECTION,
            "rkey": "e2eoauth1",
            "record": {"$type": COLLECTION, "text": "written with DPoP", "createdAt": "2026-10-16T00:00:00.000Z"},
        }))
        .unwrap();
        let req = HttpRequest::post_json(
            Target::UserPds {
                account: Did::parse(&alice_did).unwrap(),
                path_and_query: "/xrpc/com.atproto.repo.putRecord".into(),
            },
            body,
        );
        let resp = session.send(req).await.unwrap();
        assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));
        let v: Value = serde_json::from_slice(&resp.body).unwrap();
        v["uri"].as_str().unwrap().to_owned()
    });
    assert!(uri.ends_with("/e2eoauth1"));
    verify_proven(&devnet, data, &uri);

    // Refresh (DPoP-bound refresh token, rotated and saved).
    let before = read_session(data)["refreshToken"].clone();
    edit_session(data, |v| v["expiresAtMs"] = json!(1));
    ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        "oauth whoami after expiry",
    );
    assert_ne!(read_session(data)["refreshToken"], before);

    let bye = ok_json(
        &run(devnet.cli(data).args(["--json", "logout"]), None),
        "oauth logout",
    );
    assert_eq!(bye["revoked"], true);
    assert_eq!(
        run(devnet.cli(data).arg("whoami"), None).status.code(),
        Some(3)
    );

    // ---- 3. OAuth, loopback listener -------------------------------------------------------
    let c = tempfile::tempdir().unwrap();
    let data = c.path();
    let bob_did = devnet.s(&["accounts", "bob", "did"]);
    let (child, _stderr, url) = start_oauth_login(
        devnet
            .cli(data)
            .env("BROWSER", "true")
            .args(["login", "bob.test"]),
        &pds,
    );
    let pw_out = playwright(&url, &devnet.s(&["accounts", "bob", "password"]), "follow");
    assert!(pw_out.contains("lessdugin: signed in"), "{pw_out}");
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Logged in as @bob.test"), "{text}");
    assert!(
        text.contains("pinned"),
        "first login shows the issuer pin: {text}"
    );
    let me = ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        "bob whoami",
    );
    assert_eq!(me["did"], bob_did.as_str());
}

// ---- M1 acceptance: concurrent refresh -------------------------------------------------------

/// How many `lessdugin` processes race for one refresh.
const RACERS: usize = 8;

/// Races [`RACERS`] `whoami`s sharing `data` and its stored session of `did`, and checks that
/// all succeeded, exactly one process sent the refresh (`refresh`, the request's log line; at
/// most `max_requests` of them: an OAuth token request may be retried once with a fresh DPoP
/// nonce, which spends nothing), every other one adopted the session it saved, and the stored
/// `token_field` changed.
///
/// A refresh on the devnet takes milliseconds, so a process that starts late would simply load
/// the refreshed session and never race. The test therefore holds the account's refresh lock
/// until every process is waiting for it (each has loaded the old session and needs a refresh),
/// then lets them all go at once.
fn refresh_race(
    devnet: &Devnet,
    data: &Path,
    did: &str,
    what: &str,
    (refresh, max_requests): (&str, usize),
    token_field: &str,
) {
    let before = read_session(data)[token_field].clone();
    let lock_path = devnet
        .paths(data)
        .lock_file(&format!("refresh:{did}"))
        .unwrap();
    let held = FileLock::acquire(&lock_path).unwrap();
    let mut waiting = None;
    let r = race::race_with(
        (0..RACERS)
            .map(|_| {
                let mut c = devnet.cli(data);
                c.args(["--json", "whoami"]);
                c
            })
            .collect(),
        || {
            waiting = race::wait_for_lock_waiters(&lock_path, RACERS, Duration::from_secs(30));
            drop(held);
        },
    );
    let summary = r.summary(refresh);
    eprintln!("{what}: {summary} ({waiting:?} waited for the held refresh lock)");
    for me in r.all_ok_json(what) {
        assert_eq!(me["did"], did, "{what}");
    }
    let (refreshers, requests) = r.count(refresh);
    assert!(
        refreshers == 1 && requests <= max_requests,
        "{what}: exactly one process refreshes: {summary}\n{}",
        r.logs()
    );
    assert!(
        waiting.is_none_or(|n| n == RACERS),
        "{what}: {waiting:?} of {RACERS} processes waited for the refresh lock"
    );
    assert_eq!(
        r.count(race::ADOPTED).0,
        RACERS - 1,
        "{what}: every other process adopted the refreshed session: {summary}\n{}",
        r.logs()
    );
    assert_ne!(
        read_session(data)[token_field],
        before,
        "{what}: refreshed and saved"
    );
}

/// After a race the session still works: a `whoami` with the stored tokens as they are (no
/// refresh), then another refresh with the stored refresh token (which the reference PDS
/// refuses for an OAuth session whose refresh token was replayed: the whole session is revoked).
fn still_works(
    devnet: &Devnet,
    data: &Path,
    did: &str,
    what: &str,
    expiry_field: &str,
    token_field: &str,
) {
    let before = read_session(data);
    let me = ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        what,
    );
    assert_eq!(me["did"], did, "{what}");
    assert_eq!(
        read_session(data)[token_field],
        before[token_field],
        "{what}: no refresh"
    );
    edit_session(data, |v| v[expiry_field] = json!(1));
    let me = ok_json(
        &run(devnet.cli(data).args(["--json", "whoami"]), None),
        what,
    );
    assert_eq!(me["did"], did, "{what}");
    assert_ne!(
        read_session(data)[token_field],
        before[token_field],
        "{what}: refreshed again"
    );
}

/// M1 acceptance (`docs/plan.md` §3 M1) against the reference PDS: parallel `lessdugin`
/// processes refreshing one stored session never invalidate it. For an app-password session
/// and then an OAuth session (DPoP; single-use refresh tokens, replay revokes the session), two
/// races of [`RACERS`] `whoami`s sharing one data directory and its 0600 session file:
///
/// - proactive: the stored access token is marked expired (its expiry rewritten in the file);
/// - reactive: the stored access token is replaced by one the PDS refuses, its expiry far off.
///
/// Each race (all processes released together from the held refresh lock, see [`refresh_race`]):
/// every process succeeds, exactly one sends the refresh, the others adopt its session, and the
/// session still works afterwards, including one more refresh.
#[test]
fn e2e_concurrent_refresh_against_devnet() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    let pds = devnet.pds();
    let did = devnet.s(&["accounts", "carol", "did"]);

    // ---- App password ----------------------------------------------------------------------
    let a = tempfile::tempdir().unwrap();
    let data = a.path();
    let out = run(
        devnet
            .cli(data)
            .args(["--json", "login", "carol.test", "--password-stdin"]),
        Some(&format!(
            "{}\n",
            devnet.s(&["accounts", "carol", "appPassword"])
        )),
    );
    assert_eq!(ok_json(&out, "app-password login")["did"], did.as_str());
    let refresh = race::request_line(
        "POST",
        &format!("{pds}/xrpc/com.atproto.server.refreshSession"),
    );
    edit_session(data, |v| v["accessExpMs"] = json!(1));
    refresh_race(
        &devnet,
        data,
        &did,
        "app password, proactive",
        (&refresh, 1),
        "refreshJwt",
    );
    still_works(
        &devnet,
        data,
        &did,
        "app password, after the proactive race",
        "accessExpMs",
        "refreshJwt",
    );
    edit_session(data, |v| {
        v["accessJwt"] = json!("eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQwMDAwMDAwMDB9.invalid");
        v["accessExpMs"] = json!(4_000_000_000_000i64);
    });
    refresh_race(
        &devnet,
        data,
        &did,
        "app password, reactive",
        (&refresh, 1),
        "refreshJwt",
    );
    still_works(
        &devnet,
        data,
        &did,
        "app password, after the reactive race",
        "accessExpMs",
        "refreshJwt",
    );
    let bye = ok_json(
        &run(devnet.cli(data).args(["--json", "logout"]), None),
        "logout",
    );
    assert_eq!(bye["revoked"], true);

    // ---- OAuth -----------------------------------------------------------------------------
    let b = tempfile::tempdir().unwrap();
    let data = b.path();
    let (mut child, _stderr, url) = start_oauth_login(
        devnet
            .cli(data)
            .args(["--json", "login", "carol.test", "--no-browser"]),
        &pds,
    );
    let pw_out = playwright(
        &url,
        &devnet.s(&["accounts", "carol", "password"]),
        "capture",
    );
    let redirect = pw_out
        .lines()
        .find_map(|l| l.strip_prefix("REDIRECT "))
        .expect("redirect")
        .to_owned();
    {
        let mut si = child.stdin.take().unwrap();
        si.write_all(format!("{redirect}\n").as_bytes()).unwrap();
    }
    let account = ok_json(&child.wait_with_output().unwrap(), "oauth login");
    assert_eq!(account["authKind"], "oauth");
    let stored = read_session(data);
    assert_eq!(stored["kind"], "oauth");
    let refresh = race::request_line("POST", stored["tokenEndpoint"].as_str().unwrap());
    edit_session(data, |v| v["expiresAtMs"] = json!(1));
    refresh_race(
        &devnet,
        data,
        &did,
        "OAuth, proactive",
        (&refresh, 2),
        "refreshToken",
    );
    still_works(
        &devnet,
        data,
        &did,
        "OAuth, after the proactive race",
        "expiresAtMs",
        "refreshToken",
    );
    edit_session(data, |v| {
        v["accessToken"] = json!("eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQwMDAwMDAwMDB9.invalid");
        v["expiresAtMs"] = json!(4_000_000_000_000i64);
    });
    refresh_race(
        &devnet,
        data,
        &did,
        "OAuth, reactive",
        (&refresh, 2),
        "refreshToken",
    );
    still_works(
        &devnet,
        data,
        &did,
        "OAuth, after the reactive race",
        "expiresAtMs",
        "refreshToken",
    );
    let bye = ok_json(
        &run(devnet.cli(data).args(["--json", "logout"]), None),
        "oauth logout",
    );
    assert_eq!(bye["revoked"], true);
}
