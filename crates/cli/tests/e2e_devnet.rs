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

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{HttpRequest, Target};
use lessdugin_native::auth::oauth::OAuthSession;
use lessdugin_native::auth::{SecretStore, StoredSession};
use lessdugin_native::config::Paths;
use lessdugin_native::host::{AuthSession, Net, NetPolicy};
use serde_json::{Value, json};

const COLLECTION: &str = "online.7778777.lessdugin.temp.post";

struct Devnet {
    info: Value,
    child: Option<Child>,
}

impl Drop for Devnet {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Devnet {
    fn start() -> Option<Self> {
        if std::env::var("LESSDUGIN_E2E").ok().as_deref() != Some("1") {
            eprintln!("skipped: set LESSDUGIN_E2E=1 to run against the devnet");
            return None;
        }
        if let Ok(path) = std::env::var("LESSDUGIN_DEVNET_JSON") {
            let info = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            return Some(Self { info, child: None });
        }
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let json =
            std::env::temp_dir().join(format!("lessdugin-cli-devnet-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&json);
        let child = Command::new("node")
            .arg(format!("{root}/packages/e2e/devnet.mjs"))
            .arg("--json")
            .arg(&json)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("node");
        let mut devnet = Self {
            info: Value::Null,
            child: Some(child),
        };
        for _ in 0..600 {
            let parsed = std::fs::read_to_string(&json)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if let Some(v) = parsed {
                devnet.info = v;
                let _ = std::fs::remove_file(&json);
                return Some(devnet);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the devnet did not start within 60 s");
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

    /// `lessdugin` in devnet mode with its own data directory and no OS keyring.
    fn cli(&self, data: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_lessdugin"));
        for (k, _) in std::env::vars() {
            if k.starts_with("LESSDUGIN_") {
                c.env_remove(k);
            }
        }
        c.env("LESSDUGIN_DATA_DIR", data)
            .env("LESSDUGIN_KEYRING", "off")
            .env("LESSDUGIN_DEV", "1")
            .env("LESSDUGIN_PLC_URL", self.plc())
            .env("LESSDUGIN_PDS_URL", self.pds())
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG");
        c
    }
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
    let dir = data.join("sessions");
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
    assert_eq!(account["did"], alice_did.as_str());
    assert_eq!(account["handle"], "alice.test");
    assert_eq!(account["authKind"], "appPassword");
    assert_eq!(account["active"], true);
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

    let r = ok_json(
        &run(
            devnet.cli(data).args(["--json", "resolve", "alice.test"]),
            None,
        ),
        "resolve",
    );
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
    assert_eq!(list.as_array().unwrap().len(), 1);
    let bye = ok_json(
        &run(devnet.cli(data).args(["--json", "logout"]), None),
        "logout",
    );
    assert_eq!(bye["revoked"], true);
    let out = run(devnet.cli(data).args(["whoami"]), None);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        std::fs::read_dir(data.join("sessions"))
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
        let paths = Paths::under(data);
        let store = SecretStore::file_only(data.join("sessions"));
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
