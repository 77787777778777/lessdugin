// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-process tests of the server with assets the binary may not have: the placeholder page
//! (a build without `web/dist`) and the development policy. The other bridge tests run the
//! real binary (`crates/cli/tests/web_bridge.rs`).

use std::sync::Arc;

use axum::Router;
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_native::auth::SecretStore;
use lessdugin_native::config::{Config, Paths};
use lessdugin_native::host::NativeHost;

use super::assets::{Assets, PLACEHOLDER_HTML};
use super::caps::Caps;
use super::login::{LoginSlot, Prompt};
use super::policy::{COMMON_HEADERS, DEV_CSP, PRODUCTION_CSP};
use super::server::{self, Web};
use crate::args::ColorChoice;
use crate::out::Printer;

struct Running {
    origin: String,
    port: u16,
    web: Arc<Web>,
    _dir: tempfile::TempDir,
}

async fn start(assets: Assets, dev: bool) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let host = NativeHost::open(&paths, &Config::default()).unwrap();
    let engine = Engine::new(host, EngineConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let web = Arc::new(Web {
        port,
        dev,
        assets,
        caps: Caps::default(),
        engine,
        store: SecretStore::new(&paths),
        paths,
        out: Printer::new(false, ColorChoice::Never, true),
        login: LoginSlot::default(),
        prompt: Prompt::default(),
        shutdown: tokio::sync::watch::channel(false).0,
    });
    let app = Router::new()
        .fallback(server::handle)
        .with_state(web.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Running {
        origin: format!("http://127.0.0.1:{port}"),
        port,
        web,
        _dir: dir,
    }
}

fn assert_policy(r: &reqwest::Response, csp: &str) {
    let h = r.headers();
    assert_eq!(h["content-security-policy"], csp, "{}", r.url());
    for (name, value) in COMMON_HEADERS {
        assert_eq!(h[name], value, "{name} on {}", r.url());
    }
    assert!(
        !h.keys().any(|k| k.as_str().starts_with("access-control-")),
        "{}",
        r.url()
    );
}

#[tokio::test]
async fn placeholder_page_when_the_spa_is_missing() {
    let s = start(Assets::placeholder(), false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in ["/", "/index.html"] {
        let r = client
            .get(format!("{}{path}", s.origin))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_policy(&r, PRODUCTION_CSP);
        assert_eq!(r.headers()["cache-control"], "no-cache");
        assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(r.text().await.unwrap(), PLACEHOLDER_HTML);
    }
    let r = client
        .get(format!("{}/assets/index.js", s.origin))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_policy(&r, PRODUCTION_CSP);
    // lessdugin-config.json is served either way.
    let r = client
        .get(format!("{}/lessdugin-config.json", s.origin))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/json");
    assert_eq!(r.headers()["cache-control"], "no-cache");
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({"oauth": "bridge"})
    );
    assert_eq!(s.port, s.web.port);
}

#[tokio::test]
async fn dev_mode_serves_the_development_policy_everywhere() {
    let s = start(Assets::placeholder(), true).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = client.get(format!("{}/", s.origin)).send().await.unwrap();
    assert_policy(&r, DEV_CSP);
    let r = client
        .post(format!("{}/bridge/session", s.origin))
        .header("origin", &s.origin)
        .header("content-type", "application/json")
        .body("{\"cap\":\"x\"}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_policy(&r, DEV_CSP);
    assert_eq!(r.headers()["cache-control"], "no-store");
    // A foreign Host still gets the headers (and an empty 403).
    let r = client
        .get(format!("{}/", s.origin))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_policy(&r, DEV_CSP);
    assert!(r.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn minted_links_exchange_once() {
    let s = start(Assets::placeholder(), false).await;
    let link = s.web.mint_link();
    let cap = link
        .strip_prefix(&format!("{}/#lessdugin-cap=", s.origin))
        .unwrap();
    assert!(super::caps::is_secret_shape(cap));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let exchange = || {
        client
            .post(format!("{}/bridge/session", s.origin))
            .header("origin", &s.origin)
            .header("content-type", "application/json")
            .body(serde_json::json!({"cap": cap}).to_string())
            .send()
    };
    let r = exchange().await.unwrap();
    assert_eq!(r.status(), 200);
    let token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(s.web.caps.is_session(&token));
    assert_eq!(exchange().await.unwrap().status(), 403);
    assert!(!s.web.caps.is_session(&token));
}

/// Exchanges a fresh link for a bridge token.
async fn new_token(s: &Running, client: &reqwest::Client) -> (String, String) {
    let link = s.web.mint_link();
    let cap = link.split_once("#lessdugin-cap=").unwrap().1.to_owned();
    let r = client
        .post(format!("{}/bridge/session", s.origin))
        .header("origin", &s.origin)
        .header("content-type", "application/json")
        .body(serde_json::json!({"cap": cap}).to_string())
        .send()
        .await
        .unwrap();
    let token = r.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (cap, token)
}

const LOCK_DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

fn lock_request(
    s: &Running,
    client: &reqwest::Client,
    token: &str,
    body: &serde_json::Value,
) -> reqwest::RequestBuilder {
    client
        .post(format!("{}/bridge/lock", s.origin))
        .header("origin", &s.origin)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.to_string())
}

/// True while some holder has the native `outbox:<did>` lock (a native client asks for it
/// without waiting, as a second process would).
async fn native_lock_busy(s: &Running) -> bool {
    use lessdugin_core::host::Host as _;
    s.web
        .engine
        .host()
        .lock(&format!("outbox:{LOCK_DID}"), false)
        .await
        .unwrap()
        .is_none()
}

async fn wait_until_free(s: &Running) {
    for _ in 0..100 {
        if !native_lock_busy(s).await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the native outbox lock was never released");
}

/// `/bridge/lock` holds the native `outbox:<did>` lock (the one native clients pump under) for
/// as long as its response is open, so a tab and `lessdugin sync` never send the same write.
#[tokio::test]
async fn bridge_lock_holds_the_native_outbox_lock_while_the_response_is_open() {
    let s = start(Assets::placeholder(), false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let (_, token) = new_token(&s, &client).await;
    let name = serde_json::json!({"name": format!("outbox:{LOCK_DID}"), "wait": false});

    let mut held = lock_request(&s, &client, &token, &name)
        .send()
        .await
        .unwrap();
    assert_eq!(held.status(), 200);
    assert_eq!(held.headers()["content-type"], "application/x-ndjson");
    assert_eq!(held.headers()["cache-control"], "no-store");
    assert_policy(&held, PRODUCTION_CSP);
    let first = held.chunk().await.unwrap().unwrap();
    assert_eq!(&first[..], super::lock::HELD_LINE);
    assert!(native_lock_busy(&s).await, "a native client must wait");
    // A second holder (another tab, another process) is refused while it is held.
    let r = lock_request(&s, &client, &token, &name)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({"held": false})
    );
    // The heartbeat keeps coming.
    assert_eq!(&held.chunk().await.unwrap().unwrap()[..], b"\n");
    // Closing the response (the tab aborts it, or dies) releases the lock.
    drop(held);
    wait_until_free(&s).await;

    // Waiting: the request answers once the lock is free.
    use lessdugin_core::host::Host as _;
    let native = s
        .web
        .engine
        .host()
        .lock(&format!("outbox:{LOCK_DID}"), false)
        .await
        .unwrap()
        .unwrap();
    let waiting = lock_request(
        &s,
        &client,
        &token,
        &serde_json::json!({"name": format!("outbox:{LOCK_DID}"), "wait": true}),
    )
    .send();
    let release = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        drop(native);
    };
    let (mut r, ()) = tokio::join!(async { waiting.await.unwrap() }, release);
    assert_eq!(
        &r.chunk().await.unwrap().unwrap()[..],
        super::lock::HELD_LINE
    );
    drop(r);
    wait_until_free(&s).await;

    // Only outbox locks, of valid DIDs.
    for bad in [
        serde_json::json!({"name": "sync-leader"}),
        serde_json::json!({"name": format!("refresh:{LOCK_DID}")}),
        serde_json::json!({"name": "outbox:alice"}),
        serde_json::json!({"name": format!("outbox:{LOCK_DID}"), "extra": 1}),
    ] {
        let r = lock_request(&s, &client, &token, &bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400, "{bad}");
    }
    // And only for a live session.
    let r = lock_request(&s, &client, "A".repeat(43).as_str(), &name)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

/// A revoked session (its link was used twice, §7.2) loses the lock it holds, and the server
/// stopping releases every held lock.
#[tokio::test]
async fn bridge_lock_ends_with_the_session_and_the_server() {
    let s = start(Assets::placeholder(), false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let (cap, token) = new_token(&s, &client).await;
    let name = serde_json::json!({"name": format!("outbox:{LOCK_DID}")});
    let mut held = lock_request(&s, &client, &token, &name)
        .send()
        .await
        .unwrap();
    assert_eq!(
        &held.chunk().await.unwrap().unwrap()[..],
        super::lock::HELD_LINE
    );
    // The link is used a second time: the session is revoked, and the body ends.
    let r = client
        .post(format!("{}/bridge/session", s.origin))
        .header("origin", &s.origin)
        .header("content-type", "application/json")
        .body(serde_json::json!({"cap": cap}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let end = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(c) = held.chunk().await.unwrap() {
            assert_eq!(&c[..], b"\n");
        }
    })
    .await;
    assert!(end.is_ok(), "the body ends when the session is revoked");
    wait_until_free(&s).await;

    // Shutdown.
    let (_, token) = new_token(&s, &client).await;
    let mut held = lock_request(&s, &client, &token, &name)
        .send()
        .await
        .unwrap();
    assert_eq!(
        &held.chunk().await.unwrap().unwrap()[..],
        super::lock::HELD_LINE
    );
    s.web.shutdown.send_replace(true);
    let end = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while held.chunk().await.unwrap().is_some() {}
    })
    .await;
    assert!(end.is_ok(), "the body ends when the server stops");
    wait_until_free(&s).await;
}

/// A browser sign-in is cancelled when the session that started it is revoked (F27).
#[tokio::test]
async fn a_pending_login_dies_with_the_session_that_started_it() {
    let s = start(Assets::placeholder(), false).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let (cap, token) = new_token(&s, &client).await;
    let task = tokio::spawn(std::future::pending::<()>());
    let probe = task.abort_handle();
    s.web.login.test_replace(task, &token);
    assert!(
        !s.web.login.cancel_if_revoked(&s.web),
        "the session is live"
    );
    assert!(!probe.is_finished());
    // The link is used twice: the session, and so the login, are gone.
    let r = client
        .post(format!("{}/bridge/session", s.origin))
        .header("origin", &s.origin)
        .header("content-type", "application/json")
        .body(serde_json::json!({"cap": cap}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    for _ in 0..100 {
        if probe.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(probe.is_finished(), "the pending login was cancelled");
    assert!(
        !s.web.login.cancel_if_revoked(&s.web),
        "nothing is pending any more"
    );
}
