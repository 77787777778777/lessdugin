// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end: the node devnet's CORE+ services (`lessdugin_testkit::devnet_services`, the ones
//! the Playwright parity scenario configures in its `assisted` variant) against real writes on
//! the devnet PDS. Runs only with `LESSDUGIN_E2E=1` (and the `http` feature, which `cargo xtask
//! e2e --rust` passes); otherwise the test returns at once so plain `cargo test` stays offline.
//!
//! It checks what the parity scenario relies on: the firehose mirror sees the writes; the
//! Constellation and relay pages are short and still carry a cursor (the last one to an empty
//! final page); the Jetstream serves both wires from one listener, its v2 wildcard carries the
//! identity and account noise of unrelated DIDs, which the core's own filter drops, while the
//! real commit becomes a point fetch; a v1 `options_update` DID filter leaves the noise out; and
//! no frame carries record text.

#![allow(clippy::disallowed_methods)] // a real clock and real sleeps for a real network

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::live::{Action, Context, EventFilter, Wire as CoreWire, wire};
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::records::{Forum, Membership, Post};
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_testkit::devnet_services::{DevnetServices, NOISE_DIDS};
use lessdugin_testkit::jetstream::Wire;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const TITLE: &str = "A post only the services hint at";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

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
        let (child, info) = lessdugin_testkit::node_devnet::NodeDevnet::start(root, "services");
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
}

/// An XRPC procedure on the devnet PDS.
async fn procedure(
    http: &reqwest::Client,
    pds: &str,
    nsid: &str,
    jwt: Option<&str>,
    body: Value,
) -> Value {
    let mut req = http.post(format!("{pds}/xrpc/{nsid}")).json(&body);
    if let Some(jwt) = jwt {
        req = req.bearer_auth(jwt);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    let value: Value = res.json().await.unwrap();
    assert!(status.is_success(), "{nsid}: {status} {value}");
    value
}

async fn get(http: &reqwest::Client, url: &str) -> Value {
    let res = http.get(url).send().await.unwrap();
    assert_eq!(res.status(), 200, "{url}");
    res.json().await.unwrap()
}

/// Follows a short-page cursor listing to its end: every page holds at most one item and carries
/// a cursor, except the empty final page.
async fn pages(http: &reqwest::Client, base: &str, items: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0.. {
        assert!(page < 100, "the listing never ended: {base}");
        let url = match &cursor {
            Some(c) => format!("{base}&cursor={c}"),
            None => base.to_owned(),
        };
        let v = get(http, &url).await;
        let list = v[items].as_array().unwrap();
        assert!(list.len() <= 1, "pages are short: {v}");
        out.extend(list.iter().map(|r| r[key].as_str().unwrap().to_owned()));
        match v.get("cursor").and_then(Value::as_str) {
            Some(c) => {
                assert!(!list.is_empty(), "only the final page is empty: {v}");
                cursor = Some(c.to_owned());
            }
            None => {
                assert!(
                    list.is_empty(),
                    "the last non-empty page still carries a cursor: {v}"
                );
                break;
            }
        }
    }
    out
}

async fn connect(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "xrpc.v1.json".parse().unwrap());
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// Text frames until `done` holds for the frames so far, or 15 s pass.
async fn frames_until(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    done: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done(&out) && Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => out.push(t.to_string()),
            Ok(Some(Ok(_))) | Err(_) => {}
            Ok(Some(Err(e))) => panic!("the Jetstream socket failed: {e}"),
            Ok(None) => break,
        }
    }
    out
}

#[tokio::test]
async fn the_devnet_core_plus_services_hint_at_real_writes() {
    let Some(devnet) = Devnet::start() else {
        return;
    };
    let pds = devnet.s(&["pds"]);
    let http = reqwest::Client::new();
    let services = DevnetServices::start(&pds);
    let session = |who: &str| {
        let (http, pds) = (http.clone(), pds.clone());
        let id = devnet.s(&["accounts", who, "handle"]);
        let password = devnet.s(&["accounts", who, "password"]);
        async move {
            let s = procedure(
                &http,
                &pds,
                "com.atproto.server.createSession",
                None,
                json!({"identifier": id, "password": password}),
            )
            .await;
            (
                s["did"].as_str().unwrap().to_owned(),
                s["accessJwt"].as_str().unwrap().to_owned(),
            )
        }
    };
    let (alice, alice_jwt) = session("alice").await;
    let (bob, bob_jwt) = session("bob").await;
    let at = lessdugin_atp::syntax::Datetime::from_millis(now_ms()).unwrap();
    let put = |did: &str, jwt: &str, collection: &str, rkey: &str, record: Value| {
        let (http, pds) = (http.clone(), pds.clone());
        let body = json!({"repo": did, "collection": collection, "rkey": rkey, "record": record});
        let jwt = jwt.to_owned();
        async move {
            procedure(&http, &pds, "com.atproto.repo.putRecord", Some(&jwt), body).await;
        }
    };

    // alice's open forum; bob joins and posts (a newcomer: no seed references him)
    let slug = format!("services-{}", now_ms());
    let forum = ForumKey::new(&lessdugin_atp::syntax::Did::parse(&alice).unwrap(), &slug).unwrap();
    put(
        &alice,
        &alice_jwt,
        nsid::FORUM,
        &slug,
        serde_json::to_value(Forum::create("Services", at.clone(), MembershipMode::Open)).unwrap(),
    )
    .await;
    let membership = Membership::new(forum.uri.clone(), at.clone());
    put(
        &bob,
        &bob_jwt,
        nsid::MEMBERSHIP,
        &forum.fk,
        serde_json::to_value(membership).unwrap(),
    )
    .await;
    let post_rkey = keys::post_rkey(
        forum.uri.as_str(),
        lessdugin_testkit::repo::tid_at_ms(now_ms()).as_ref(),
    )
    .unwrap();
    let post = Post::article(
        forum.uri.clone(),
        TITLE,
        "Its text never crosses Jetstream.",
        at,
    );
    put(
        &bob,
        &bob_jwt,
        nsid::POST,
        &post_rkey,
        serde_json::to_value(post).unwrap(),
    )
    .await;
    let post_uri = format!("at://{bob}/{}/{post_rkey}", nsid::POST);

    // the mirror follows the firehose
    let deadline = Instant::now() + Duration::from_secs(20);
    while !services.has_record(&post_uri) {
        assert!(
            Instant::now() < deadline,
            "the firehose mirror never saw {post_uri}: {}",
            services.log()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Constellation: bob's membership backlink, in short pages that still carry a cursor
    let base = services.constellation_url();
    let subject = urlencode(forum.uri.as_str());
    let members = pages(
        &http,
        &format!(
            "{base}/xrpc/blue.microcosm.links.getBacklinks?subject={subject}&source={}:forum&limit=100",
            nsid::MEMBERSHIP
        ),
        "records",
        "did",
    )
    .await;
    assert_eq!(members, std::slice::from_ref(&bob));
    let count = get(
        &http,
        &format!(
            "{base}/xrpc/blue.microcosm.links.getBacklinksCount?subject={subject}&source={}:forum",
            nsid::MEMBERSHIP
        ),
    )
    .await;
    assert_eq!(count["total"], 1);
    // the relay: every repo holding the collection, the same way
    let repos = pages(
        &http,
        &format!(
            "{}/xrpc/com.atproto.sync.listReposByCollection?collection={}&limit=500",
            services.relay_url(),
            nsid::MEMBERSHIP
        ),
        "repos",
        "did",
    )
    .await;
    assert!(repos.contains(&bob), "{repos:?}");
    assert!(
        services.requests().iter().all(|r| r.status == 200),
        "{:?}",
        services.requests()
    );

    // Jetstream v2, the native wildcard: the real commit and the unrelated noise
    let cursor = (now_ms() - 60_000) * 1000;
    let v2 = format!(
        "{}/xrpc/network.bsky.jetstream.subscribeEvents?collections={NS}.*&cursor={cursor}",
        services.jetstream_url()
    );
    let mut ws = connect(&v2).await;
    let noise_kinds = |frames: &[String]| {
        frames
            .iter()
            .filter(|f| NOISE_DIDS.iter().any(|d| f.contains(d)))
            .filter_map(|f| {
                ["#identity", "#account"]
                    .into_iter()
                    .find(|k| f.contains(&format!("subscribeEvents{k}")))
            })
            .collect::<BTreeSet<_>>()
    };
    let frames = frames_until(&mut ws, |f| {
        f.iter().any(|x| x.contains(&post_rkey)) && noise_kinds(f).len() == 2
    })
    .await;
    assert!(frames.iter().any(|f| f.contains(&post_rkey)), "{frames:?}");
    assert_eq!(
        noise_kinds(&frames).len(),
        2,
        "identity and account noise: {frames:?}"
    );
    assert!(
        frames.iter().all(|f| !f.contains(TITLE)),
        "no record text on the wire"
    );
    // the core's own parser and filter: the noise is valid but dropped; the post is fetched
    let cx = Context {
        known_dids: [alice.clone(), bob.clone()].into(),
        forums: [forum.fk.clone()].into(),
        newcomer_forums: [forum.fk.clone()].into(),
        ..Context::default()
    };
    let mut filter = EventFilter::default();
    let mut fetched = Vec::new();
    for f in &frames {
        let event = wire::parse(CoreWire::V2, f)
            .unwrap_or_else(|e| panic!("{e}: {f}"))
            .expect("an event");
        let noise = event
            .did
            .as_deref()
            .is_some_and(|d| NOISE_DIDS.contains(&d));
        let out = filter.apply(
            CoreWire::V2,
            services.jetstream_url(),
            event,
            &cx,
            now_ms().saturating_mul(1_000),
        );
        if noise {
            assert!(
                out.actions.is_empty(),
                "noise is dropped: {f} -> {:?}",
                out.actions
            );
        }
        for a in &out.actions {
            if let Action::PointFetch { uri, .. } = a {
                fetched.push(uri.clone());
            }
        }
        filter.acknowledge();
    }
    assert!(fetched.contains(&post_uri), "{fetched:?}");
    ws.close(None).await.ok();

    // Jetstream v1 on the same listener: requireHello, then a DID filter of bob
    let v1 = format!(
        "{}/subscribe?wantedCollections={NS}.*&requireHello=true&cursor={cursor}",
        services.jetstream_url()
    );
    let mut ws = connect(&v1).await;
    ws.send(Message::Text(
        json!({"type": "options_update", "payload": {
            "wantedCollections": [format!("{NS}.*")], "wantedDids": [bob.clone()],
        }})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let frames = frames_until(&mut ws, |f| f.iter().any(|x| x.contains(&post_rkey))).await;
    // let noise arrive if the filter were broken
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let mut frames = frames;
    while let Ok(Some(Ok(Message::Text(t)))) =
        tokio::time::timeout(Duration::from_millis(200), ws.next()).await
    {
        frames.push(t.to_string());
    }
    assert!(frames.iter().any(|f| f.contains(&post_rkey)), "{frames:?}");
    for f in &frames {
        let event = wire::parse(CoreWire::V1, f)
            .unwrap_or_else(|e| panic!("{e}: {f}"))
            .expect("an event");
        assert_eq!(event.did.as_deref(), Some(bob.as_str()), "only bob: {f}");
        assert!(!f.contains(TITLE));
    }
    ws.close(None).await.ok();
    let log = services.jetstream_requests();
    assert!(
        log.iter()
            .any(|r| r.wire == Wire::V2 && r.url.contains("subscribeEvents"))
    );
    assert!(
        log.iter()
            .any(|r| r.wire == Wire::V1 && r.options.is_some()),
        "{log:?}"
    );
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
