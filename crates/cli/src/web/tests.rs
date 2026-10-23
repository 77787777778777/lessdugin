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
use super::login::LoginSlot;
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
