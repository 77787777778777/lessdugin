// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end: identity resolution, login binding and `verify` against the local devnet
//! (`packages/e2e/devnet.mjs`, ADR 0006). Runs only with `LESSDUGIN_E2E=1`; otherwise every test
//! returns immediately so plain `cargo test` stays offline.
//!
//! `LESSDUGIN_DEVNET_JSON=<file>` reuses a running devnet; otherwise the test starts one with
//! `node packages/e2e/devnet.mjs --json <tmp>` and kills it at the end.
//!
//! The host here is a minimal reqwest host (no SSRF guard; loopback only), standing in for the
//! native host: Anon requests follow redirects only when asked, `UserPds` requests carry a Bearer
//! token and never follow redirects.

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lessdugin_atp::syntax::{AtIdentifier, Did, Nsid};
use lessdugin_atp::xrpc::{CreateRecord, CreateSession, Session, WriteRecordOutput};
use lessdugin_core::dto::{HandleState, VerifyOutcome};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, PinStatus};
use lessdugin_core::host::{
    BlobHandle, Host, HostError, HttpRequest, HttpResponse, Redirects, Target, WebSocket, WsMessage,
};
use lessdugin_core::store::mem::MemKv;
use lessdugin_core::xrpc::{CallOpts, Service};
use serde_json::{Value, json};

struct NoWs;

impl WebSocket for NoWs {
    async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
        None
    }
    async fn send_text(&mut self, _: String) -> Result<(), HostError> {
        Err(HostError::Unsupported("websocket".into()))
    }
    async fn close(&mut self) {}
}

struct NetHost {
    cache: MemKv,
    state: MemKv,
    no_redirects: reqwest::Client,
    redirects: reqwest::Client,
    sessions: Mutex<HashMap<String, (String, String)>>,
    rng: Mutex<u64>,
}

impl NetHost {
    fn new() -> Self {
        let build = |p| reqwest::Client::builder().redirect(p).build().unwrap();
        Self {
            cache: MemKv::new(),
            state: MemKv::new(),
            no_redirects: build(reqwest::redirect::Policy::none()),
            redirects: build(reqwest::redirect::Policy::limited(3)),
            sessions: Mutex::new(HashMap::new()),
            rng: Mutex::new(0x1234_5678),
        }
    }

    fn login(&self, did: &str, pds: &str, access_jwt: &str) {
        self.sessions
            .lock()
            .unwrap()
            .insert(did.to_owned(), (pds.to_owned(), access_jwt.to_owned()));
    }
}

impl Host for NetHost {
    type Kv = MemKv;
    type Ws = NoWs;

    fn cache(&self) -> &MemKv {
        &self.cache
    }

    fn state(&self) -> &MemKv {
        &self.state
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        let (client, url, auth) = match &req.target {
            Target::Anon(u) => {
                let c = match req.redirects {
                    Redirects::None => &self.no_redirects,
                    Redirects::Follow(_) => &self.redirects,
                };
                (c, u.to_string(), None)
            }
            Target::UserPds {
                account,
                path_and_query,
            } => {
                let (pds, jwt) = self
                    .sessions
                    .lock()
                    .unwrap()
                    .get(account.as_str())
                    .cloned()
                    .ok_or_else(|| HostError::AuthRequired(account.to_string()))?;
                (
                    &self.no_redirects,
                    format!("{pds}{path_and_query}"),
                    Some(jwt),
                )
            }
        };
        let mut rb = client
            .request(req.method.clone(), &url)
            .timeout(Duration::from_millis(u64::from(req.timeout_ms)));
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(jwt) = auth {
            rb = rb.bearer_auth(jwt);
        }
        if let Some(b) = req.body.clone() {
            rb = rb.body(b);
        }
        let resp = rb.send().await.map_err(|e| {
            if e.is_timeout() {
                HostError::Timeout
            } else {
                HostError::Network(e.to_string())
            }
        })?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or("").to_owned(),
                )
            })
            .collect();
        let body = resp
            .bytes()
            .await
            .map_err(|e| HostError::Network(e.to_string()))?
            .to_vec();
        if body.len() as u64 > req.max_bytes {
            return Err(HostError::TooLarge);
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    async fn fetch_blob(
        &self,
        _: url::Url,
        _: Did,
        _: String,
        _: u64,
    ) -> Result<BlobHandle, HostError> {
        Err(HostError::Unsupported("blobs".into()))
    }

    async fn websocket(&self, _: url::Url, _: u32) -> Result<NoWs, HostError> {
        Err(HostError::Unsupported("websocket".into()))
    }

    async fn resolve_txt_system(&self, _: String) -> Result<Option<Vec<String>>, HostError> {
        Err(HostError::Unsupported("system DNS".into()))
    }

    async fn sleep(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn fill_random(&self, buf: &mut [u8]) {
        let mut s = self.rng.lock().unwrap();
        for b in buf {
            *s ^= *s << 13;
            *s ^= *s >> 7;
            *s ^= *s << 17;
            *b = *s as u8;
        }
    }
}

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
            std::env::temp_dir().join(format!("lessdugin-devnet-{}.json", std::process::id()));
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
}

#[tokio::test]
async fn devnet_identity_login_and_verify() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    let pds = devnet.s(&["pds"]);
    let plc = devnet.s(&["plc"]);
    let did = devnet.s(&["accounts", "alice", "did"]);
    let handle = devnet.s(&["accounts", "alice", "handle"]);
    let app_password = devnet.s(&["accounts", "alice", "appPassword"]);
    assert!(lessdugin_core::engine::is_app_password_shape(&app_password));

    let host = std::sync::Arc::new(NetHost::new());
    let cfg = EngineConfig::devnet(
        url::Url::parse(&plc).unwrap(),
        url::Url::parse(&pds).unwrap(),
    );
    let e = Engine::with_shared_host(host.clone(), cfg);

    // Identity: handle → DID through the PDS, PLC document over http, both directions verified.
    let a = e.resolve_actor(&handle).await.unwrap();
    assert_eq!(a.did, did);
    assert_eq!(a.handle.as_deref(), Some(handle.as_str()));
    assert_eq!(a.handle_state, HandleState::Verified);
    let id = e.resolve_identity(&did).await.unwrap().identity;
    assert_eq!(id.pds.as_deref(), Some(pds.as_str()));
    assert!(id.signing_key.is_some(), "{:?}", id.key_problem);

    // App-password login (createSession at the document's PDS), then the binding check.
    let service = Service::Anon(url::Url::parse(id.pds.as_deref().unwrap()).unwrap());
    let session: Session = e
        .xrpc()
        .procedure_endpoint(
            &service,
            &CreateSession {
                identifier: did.clone(),
                password: app_password,
                auth_factor_token: None,
                allow_takendown: None,
            },
            &CallOpts::default(),
        )
        .await
        .unwrap();
    assert_eq!(session.did, did);
    let account = e
        .add_account(AccountInfo::app_password(&session.did, &pds).with_intended(&a.did))
        .await
        .unwrap();
    assert_eq!(account.handle.as_deref(), Some(handle.as_str()));
    assert!(account.active);
    // The devnet PDS is its own authorization server.
    let b = e
        .bind_login(&AccountInfo::oauth(&did, &pds, &pds))
        .await
        .unwrap();
    assert_eq!(b.pin, PinStatus::New);
    assert!(
        e.bind_login(&AccountInfo::oauth(&did, &pds, "http://localhost:1"))
            .await
            .is_err()
    );

    // Write a record through Target::UserPds, then verify it from the PDS's proof.
    host.login(&did, &pds, &session.access_jwt);
    let user = Service::UserPds(Did::parse(&did).unwrap());
    let created: WriteRecordOutput = e
        .xrpc()
        .procedure_endpoint(
            &user,
            &CreateRecord {
                repo: AtIdentifier::Did(Did::parse(&did).unwrap()),
                collection: Nsid::parse("online.7778777.lessdugin.post").unwrap(),
                rkey: None,
                validate: None,
                record: json!({
                    "$type": "online.7778777.lessdugin.post",
                    "title": "e2e",
                    "tags": ["a", "b"],
                    "n": 42,
                    "createdAt": "2026-10-16T12:00:00.000Z",
                }),
                swap_commit: None,
            },
            &CallOpts::default(),
        )
        .await
        .unwrap();
    let report = e.verify(&created.uri, false).await.unwrap();
    assert_eq!(report.result, VerifyOutcome::Proven, "{:#?}", report.checks);
    assert_eq!(report.key_source, "plc #atproto k256");
    let record_cid = report
        .checks
        .iter()
        .find(|c| c.name == "record cid")
        .unwrap();
    assert_eq!(record_cid.detail.as_deref(), Some(created.cid.as_str()));
    if let Some(commit) = &created.commit {
        assert_eq!(report.rev.as_deref(), Some(commit.rev.as_str()));
        assert_eq!(report.commit_cid.as_deref(), Some(commit.cid.as_str()));
    }
    // The same record by handle URI.
    let rkey = created.uri.rsplit('/').next().unwrap();
    let by_handle = format!("at://{handle}/online.7778777.lessdugin.post/{rkey}");
    assert_eq!(
        e.verify(&by_handle, true).await.unwrap().result,
        VerifyOutcome::Proven
    );
    // A record that does not exist is proven absent.
    let absent = format!("at://{did}/online.7778777.lessdugin.post/3l2s5xxv2zzzz");
    let r = e.verify(&absent, false).await.unwrap();
    assert_eq!(r.result, VerifyOutcome::Absent, "{:#?}", r.checks);
}
