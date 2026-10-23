// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin web` against the local devnet (`packages/e2e/devnet.mjs`, ADR 0006, ADR 0007).
//! Runs only with `LESSDUGIN_E2E=1` (`LESSDUGIN_DEVNET_JSON=<file>` reuses a running devnet).
//!
//! The browser sign-in of `threat-model.md` §7.4, end to end: a bridge session from a printed
//! link, `POST /bridge/login {"actor": "alice.test"}`, headless Chromium signs in and approves
//! at the devnet PDS, the redirect reaches the `web` process's callback listener, which checks
//! `state`/`iss`, exchanges the code (PKCE, DPoP), runs the login binding, stores the session
//! and answers with a 303 back to the bridge's `/#/`. Then the stored OAuth session serves the
//! tab: `getSession` and a `createRecord` through `/bridge/xrpc` (DPoP, the real PDS), and the
//! record is readable at the PDS.

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lessdugin_lexicon::ns::NS;
use serde_json::{Value, json};

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
        let (child, info) = lessdugin_testkit::node_devnet::NodeDevnet::start(root, "web");
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
            .env("LESSDUGIN_PLC_URL", self.s(&["plc"]))
            .env("LESSDUGIN_PDS_URL", self.s(&["pds"]))
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .env_remove("BROWSER");
        c
    }
}

struct Web {
    child: Child,
    links: Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Web {
    fn start(mut cmd: Command) -> Self {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, links) = channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
        Self {
            child,
            links,
            stderr,
        }
    }

    /// Types `answer` on the process's stdin once `needle` shows on its stderr (the terminal's
    /// confirmation of a browser sign-in), from a thread, so a blocking step can run meanwhile.
    fn answer_when(&mut self, needle: &'static str, answer: &'static str) {
        let mut stdin = self.child.stdin.take().expect("stdin is piped");
        let stderr = self.stderr.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(120);
            while Instant::now() < deadline {
                if stderr.lock().unwrap().contains(needle) {
                    let _ = stdin.write_all(answer.as_bytes());
                    let _ = stdin.flush();
                    // keep stdin open: its end would refuse later questions
                    std::thread::sleep(Duration::from_secs(120));
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }

    fn wait_for_stderr(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let text = self.stderr.lock().unwrap().clone();
            if text.contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "no {needle:?} in: {text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn approve(url: &str, password: &str, origin: &str) -> String {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/web_e2e_approve.mjs");
    let mut cmd = Command::new("node");
    cmd.arg(script).arg(url).arg(password).arg(origin);
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

#[test]
fn e2e_browser_sign_in_and_oauth_writes_through_the_bridge() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    let pds = devnet.s(&["pds"]);
    let alice = devnet.s(&["accounts", "alice", "did"]);
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut cmd = devnet.cli(dir.path());
    cmd.args(["web", "--no-open", "--port", &port.to_string()]);
    let mut web = Web::start(cmd);
    let link = web
        .links
        .recv_timeout(Duration::from_secs(60))
        .expect("a link");
    let origin = format!("http://127.0.0.1:{port}");
    let cap = link.split_once("#lessdugin-cap=").unwrap().1.to_owned();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let post = |path: &str, token: Option<&str>, body: Value| -> (u16, Value, Option<String>) {
        rt.block_on(async {
            let mut rb = client
                .post(format!("{origin}{path}"))
                .header("origin", &origin)
                .header("content-type", "application/json")
                .body(body.to_string());
            if let Some(t) = token {
                rb = rb.header("authorization", format!("Bearer {t}"));
            }
            let r = rb.send().await.unwrap();
            let status = r.status().as_u16();
            let upstream = r
                .headers()
                .get("x-lessdugin-upstream")
                .map(|v| v.to_str().unwrap().to_owned());
            (status, r.json().await.unwrap_or(Value::Null), upstream)
        })
    };
    let (status, v, _) = post("/bridge/session", None, json!({"cap": cap}));
    assert_eq!(status, 200, "{v}");
    let token = v["token"].as_str().unwrap().to_owned();

    // Nobody is logged in yet.
    let (status, v, _) = post(
        &format!("/bridge/xrpc/com.atproto.server.getSession?account={alice}"),
        Some(&token),
        json!({}),
    );
    assert_eq!(status, 403, "{v}");
    assert_eq!(v["error"], "auth_required");

    // The browser sign-in.
    let (status, v, _) = post(
        "/bridge/login",
        Some(&token),
        json!({"actor": "alice.test"}),
    );
    assert_eq!(status, 200, "{v}");
    let url = v["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with(&format!("{pds}/oauth/authorize?")), "{url}");
    web.wait_for_stderr("A browser tab started a sign-in as @alice.test");
    // The terminal confirms the sign-in before the session is stored (F27).
    web.answer_when("Accept this sign-in? [y/N]", "y\n");
    let out = approve(&url, &devnet.s(&["accounts", "alice", "password"]), &origin);
    assert!(out.contains(&format!("FINAL {origin}/#/\n")), "{out}");
    let via = out
        .lines()
        .find_map(|l| l.strip_prefix("VIA "))
        .unwrap_or_else(|| panic!("no callback in {out}"));
    assert_ne!(
        via,
        format!("{origin}/callback"),
        "the callback has its own port"
    );
    web.wait_for_stderr("Signed in from the browser as @alice.test");

    // The account is recorded with an OAuth session.
    let accounts = devnet
        .cli(dir.path())
        .args(["--json", "accounts"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&accounts.stdout);
    assert!(
        text.contains(&alice) && text.contains("\"oauth\""),
        "{text}"
    );

    // The tab now writes through the stored OAuth session (DPoP, the real PDS).
    let (status, v, upstream) = post(
        &format!("/bridge/xrpc/com.atproto.server.getSession?account={alice}"),
        Some(&token),
        json!({}),
    );
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["did"], alice.as_str());
    assert_eq!(upstream.as_deref(), Some("pds"));
    let collection = format!("{NS}.post");
    let (status, v, _) = post(
        &format!("/bridge/xrpc/com.atproto.repo.createRecord?account={alice}"),
        Some(&token),
        json!({
            "repo": alice,
            "collection": collection,
            "record": {"$type": collection, "title": "from the bridge", "createdAt": "2026-10-22T00:00:00.000Z"},
        }),
    );
    assert_eq!(status, 200, "{v}");
    let uri = v["uri"].as_str().unwrap().to_owned();
    let rkey = uri.rsplit('/').next().unwrap();
    let record: Value = rt.block_on(async {
        client
            .get(format!(
                "{pds}/xrpc/com.atproto.repo.getRecord?repo={alice}&collection={collection}&rkey={rkey}"
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    });
    assert_eq!(record["value"]["title"], "from the bridge", "{record}");
    // Still not past the allowlist.
    let (status, _, upstream) = post(
        &format!("/bridge/xrpc/com.atproto.repo.createRecord?account={alice}"),
        Some(&token),
        json!({"repo": alice, "collection": "app.bsky.feed.post", "record": {}}),
    );
    assert_eq!(status, 403);
    assert!(upstream.is_none());
    // No secret reached the terminal.
    let err = web.stderr.lock().unwrap().clone();
    assert!(!err.contains(&token) && !err.contains(&cap), "{err}");
}
