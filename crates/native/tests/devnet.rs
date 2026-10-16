// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end checks of NativeHost against the local devnet (PLC + PDS, ADR 0006).
//! Runs only with `LESSDUGIN_E2E=1` (it starts `node packages/e2e/devnet.mjs`), so a plain
//! `cargo test` stays offline.

// Waiting for an external process needs the real clock (the ban targets the wasm-safe crates).
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{Host, HostError, HttpRequest, HttpResponse, Target};
use lessdugin_native::config::{Config, Paths};
use lessdugin_native::host::{AuthSession, NativeHost, Net, NetPolicy, NetRequest};
use serde_json::{Value, json};
use url::Url;

fn e2e_enabled() -> bool {
    std::env::var("LESSDUGIN_E2E").as_deref() == Ok("1")
}

/// The devnet process, killed on drop.
struct Devnet {
    child: Child,
    state: Value,
}

impl Drop for Devnet {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Devnet {
    fn start(dir: &Path) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let json = dir.join("devnet.json");
        let child = Command::new("node")
            .arg(root.join("packages/e2e/devnet.mjs"))
            .arg("--json")
            .arg(&json)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start node packages/e2e/devnet.mjs");
        let mut net = Devnet {
            child,
            state: Value::Null,
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Ok(bytes) = std::fs::read(&json)
                && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
            {
                net.state = v;
                return net;
            }
            assert!(Instant::now() < deadline, "the devnet did not start");
            if let Ok(Some(status)) = net.child.try_wait() {
                panic!("the devnet exited early: {status}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn url(&self, key: &str) -> Url {
        Url::parse(self.state[key].as_str().unwrap()).unwrap()
    }
}

struct BearerSession {
    did: Did,
    pds: Url,
    token: String,
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
                .push(("authorization".into(), format!("Bearer {}", self.token)));
            self.net.execute(r).await
        })
    }
}

fn json_of(r: &HttpResponse) -> Value {
    assert!(
        (200..300).contains(&r.status),
        "HTTP {}: {}",
        r.status,
        String::from_utf8_lossy(&r.body)
    );
    serde_json::from_slice(&r.body).unwrap()
}

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn devnet_end_to_end() {
    if !e2e_enabled() {
        eprintln!("skipped: set LESSDUGIN_E2E=1 to run against the devnet");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let devnet = Devnet::start(tmp.path());
    let (pds, plc) = (devnet.url("pds"), devnet.url("plc"));
    let alice = &devnet.state["accounts"]["alice"];
    let did = Did::parse(alice["did"].as_str().unwrap()).unwrap();

    let cfg = Config::devnet(plc.clone(), pds.clone());
    let paths = Paths::under(&tmp.path().join("data"));
    let host = NativeHost::open(&paths, &cfg).unwrap();
    assert!(host.net().policy().is_devnet());

    // Handles resolve through the PDS in devnet mode.
    let mut u = pds
        .join("/xrpc/com.atproto.identity.resolveHandle")
        .unwrap();
    u.query_pairs_mut()
        .append_pair("handle", alice["handle"].as_str().unwrap());
    let r = host.http(HttpRequest::get(Target::Anon(u))).await.unwrap();
    assert_eq!(json_of(&r)["did"].as_str(), Some(did.as_str()));

    // The DID document from the devnet PLC names the devnet PDS.
    let r = host
        .http(HttpRequest::get(Target::Anon(
            plc.join(&format!("/{}", did.as_str())).unwrap(),
        )))
        .await
        .unwrap();
    let doc = json_of(&r);
    let endpoint = doc["service"][0]["serviceEndpoint"].as_str().unwrap();
    assert_eq!(
        endpoint.trim_end_matches('/'),
        pds.as_str().trim_end_matches('/')
    );

    // App-password session (an anonymous POST), then authenticated writes through UserPds.
    let body = json!({"identifier": did.as_str(), "password": alice["appPassword"]});
    let url = pds.join("/xrpc/com.atproto.server.createSession").unwrap();
    let r = host
        .http(HttpRequest::post_json(
            Target::Anon(url),
            serde_json::to_vec(&body).unwrap(),
        ))
        .await
        .unwrap();
    let session = json_of(&r);
    assert_eq!(session["did"].as_str(), Some(did.as_str()));
    host.set_session(Arc::new(BearerSession {
        did: did.clone(),
        pds: pds.clone(),
        token: session["accessJwt"].as_str().unwrap().to_owned(),
        net: host.net().clone(),
    }));

    let bytes = png();
    let mut upload = HttpRequest::post_json(
        Target::UserPds {
            account: did.clone(),
            path_and_query: "/xrpc/com.atproto.repo.uploadBlob".into(),
        },
        bytes.clone(),
    );
    upload.headers = vec![("content-type".into(), "image/png".into())];
    let blob = json_of(&host.http(upload).await.unwrap())["blob"].clone();
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    assert_eq!(cid, lessdugin_atp::cid::for_raw(&bytes).to_string());

    let record = json!({
        "repo": did.as_str(),
        "collection": "app.bsky.actor.profile",
        "rkey": "self",
        "record": {"$type": "app.bsky.actor.profile", "displayName": "Alice", "avatar": blob},
    });
    let put = HttpRequest::post_json(
        Target::UserPds {
            account: did.clone(),
            path_and_query: "/xrpc/com.atproto.repo.putRecord".into(),
        },
        serde_json::to_vec(&record).unwrap(),
    );
    json_of(&host.http(put).await.unwrap());

    // The blob streams back, verified against its CID, into the cache directory.
    let h = host
        .fetch_blob(pds.clone(), did.clone(), cid.clone(), 5 << 20)
        .await
        .unwrap();
    assert_eq!(h.size, bytes.len() as u64);
    assert!(Path::new(&h.locator).starts_with(paths.blobs_dir()));
    assert_eq!(std::fs::read(&h.locator).unwrap(), bytes);
    assert_eq!(
        host.fetch_blob(pds.clone(), did.clone(), cid.clone(), 1)
            .await
            .unwrap(),
        h,
        "a stored blob needs no request"
    );
    let missing = lessdugin_atp::cid::for_raw(b"never uploaded").to_string();
    assert!(matches!(
        host.fetch_blob(pds.clone(), did.clone(), missing, 1 << 20)
            .await,
        Err(HostError::HttpStatus(400, _))
    ));

    // Outside the devnet profile the same endpoints are refused.
    let prod = Net::new(NetPolicy::production()).unwrap();
    let r = prod
        .execute(NetRequest::get(pds.join("/xrpc/_health").unwrap()))
        .await;
    assert!(matches!(r, Err(HostError::Blocked(_))), "{r:?}");
    let mut other = pds.clone();
    other.set_port(Some(1)).unwrap();
    let r = host.http(HttpRequest::get(Target::Anon(other))).await;
    assert!(
        matches!(r, Err(HostError::Blocked(_))),
        "http only for configured origins: {r:?}"
    );
}
