// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real native service over the signed loopback devnet: no in-process PDS shortcut.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use lessdugin_atp::proof::ProofBundle;
use lessdugin_atp::syntax::Datetime;
use lessdugin_core::discovery::ForumKey;
use lessdugin_indexer::host::PoliteHost;
use lessdugin_indexer::store::Store;
use lessdugin_indexer::{Indexer, IndexerConfig, NS, ingest, proofs};
use lessdugin_lexicon::keys;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::records::{Forum, Membership, Post};
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_native::config::{Config, DevnetProfile, NetworkProfile, Paths};
use lessdugin_native::host::NativeHost;
use lessdugin_testkit::FakeNet;
use lessdugin_testkit::fake_pds::{AccountHandle, SystemClock};
use serde_json::Value;
use tempfile::TempDir;
use tokio::time::Instant;
use tower::ServiceExt;
use url::Url;

struct Fixture {
    dir: std::path::PathBuf,
    _keep: Option<TempDir>,
    indexer: Indexer,
    net: FakeNet,
    owner: AccountHandle,
    forum: ForumKey,
}
impl Fixture {
    async fn at(dir: &Path, net: FakeNet, owner: AccountHandle, forum: ForumKey) -> Self {
        let paths = Paths::under(dir);
        let config = Config {
            network: NetworkProfile::Devnet(DevnetProfile {
                plc: Url::parse(&net.plc()).unwrap(),
                pds: owner.pds().url(),
                jetstream: None,
            }),
            ..Config::default()
        };
        let host = NativeHost::open(&paths, &config).unwrap();
        let engine = lessdugin_core::engine::Engine::new(
            PoliteHost::new(host),
            lessdugin_native::engine::engine_config(&config),
        );
        let store = Store::open(&paths.state_db()).unwrap();
        store.add_forum_seed(forum.uri.as_str()).unwrap();
        let indexer = Indexer::new(
            engine,
            store,
            IndexerConfig {
                did: "did:web:indexer.test".into(),
                source_url: "https://source.test/lessdugin".into(),
                plc: Url::parse(&net.plc()).unwrap(),
                relays: vec![],
                jetstream: vec![],
                poll: Some(owner.pds().url()),
                poll_interval_ms: 250,
                blocklist: std::sync::Arc::new(BTreeSet::new()),
            },
        );
        Self {
            dir: dir.to_path_buf(),
            _keep: None,
            indexer,
            net,
            owner,
            forum,
        }
    }
    async fn new() -> Self {
        let net = FakeNet::serve_http(SystemClock);
        let pds = net.default_pds();
        let owner = pds.add_account("owner.test");
        let forum = ForumKey::new(owner.did(), "m6").unwrap();
        let now = Datetime::from_millis(net.now_ms() - 3_600_000).unwrap();
        let f = Forum::create("M6", now.clone(), MembershipMode::Open);
        owner.put(nsid::FORUM, "m6", &serde_json::to_value(f).unwrap());
        let m = Membership::new(forum.uri.clone(), now);
        owner.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(m).unwrap(),
        );
        let dir = TempDir::new().unwrap();
        let keep = dir.keep();
        Self::at(&keep, net, owner, forum).await
    }
    fn post(&self, title: &str) -> String {
        let now = Datetime::from_millis(self.net.now_ms() - 3_600_000).unwrap();
        let p = Post::article(self.forum.uri.clone(), title, "A real signed body.", now);
        let rkey = keys::post_rkey(
            self.forum.uri.as_str(),
            lessdugin_testkit::repo::tid_at_ms(self.net.now_ms()).as_ref(),
        )
        .unwrap();
        self.owner
            .put(nsid::POST, &rkey, &serde_json::to_value(p).unwrap());
        self.owner.uri(nsid::POST, &rkey)
    }
    async fn request(&self, path: &str) -> (StatusCode, Value, axum::http::HeaderMap) {
        let response = lessdugin_indexer::api::router(self.indexer.clone())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 20_000_000)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            headers,
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_devnet_all_first_pass_methods_and_restart() {
    let fixture = Fixture::new().await;
    let post = fixture.post("First");
    let started = Instant::now();
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let _ = started;
    let base = format!("/xrpc/{NS}.index");
    let encoded = urlencoding(fixture.forum.uri.as_ref());
    let (status, describe, headers) = fixture.request(&format!("{base}.describeIndexer")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(describe["features"].as_array().unwrap().len(), 9);
    assert_eq!(headers[header::CACHE_CONTROL], "max-age=5");
    let (status, forums, _) = fixture
        .request(&format!("{base}.listForums?limit=10"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        forums["forums"][0]["ref"]["uri"],
        fixture.forum.uri.as_str()
    );
    let (status, participants, _) = fixture
        .request(&format!("{base}.listParticipants?forum={encoded}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        participants["participants"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["did"] == fixture.owner.did().as_str())
    );
    let (status, heads, _) = fixture
        .request(&format!("{base}.getRepoHeads?dids={}", fixture.owner.did()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(heads["heads"][0]["rev"].is_string());
    let (status, changes, _) = fixture
        .request(&format!("{base}.getChanges?forum={encoded}&since=0"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!changes["changes"].as_array().unwrap().is_empty());
    let since = changes["cursor"].as_i64().unwrap();
    let (status, posts, _) = fixture
        .request(&format!("{base}.listPosts?forum={encoded}&hydrate=true"))
        .await;
    assert_eq!(status, StatusCode::OK, "{posts}");
    assert_eq!(posts["posts"][0]["ref"]["uri"], post);
    assert_eq!(posts["inputDigest"].as_str().unwrap().len(), 64);
    assert!(posts["posts"][0]["ref"]["value"].is_object());
    let (status, recent, _) = fixture
        .request(&format!("{base}.listRecentDiscussion?forum={encoded}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(recent["items"].is_array());
    let (status, thread, _) = fixture
        .request(&format!("{base}.getThread?post={}", urlencoding(&post)))
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert_eq!(thread["post"]["ref"]["uri"], post);
    assert_eq!(thread["inputDigest"], posts["inputDigest"]);
    let (status, _, _) = fixture
        .request(&format!(
            "{base}.getRecordProofs?uris={}",
            urlencoding(&post)
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let proof = proofs::fetch(&fixture.indexer, &post).await.unwrap();
    let bundle = proofs::bundle(&fixture.indexer, std::slice::from_ref(&post))
        .await
        .unwrap();
    let parsed = ProofBundle::parse(&bundle, &Default::default()).unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(proof.uri, post);
    let ident = fixture
        .indexer
        .engine
        .resolver()
        .resolve_did_fresh(fixture.owner.did(), 0)
        .await
        .unwrap();
    parsed
        .verify_item(
            0,
            &ident.public_key().unwrap(),
            fixture.indexer.now_ms() * 1000,
        )
        .unwrap();
    let second = fixture.post("After restart");
    drop(fixture.indexer.engine.clone());
    let restarted = Fixture::at(
        &fixture.dir,
        fixture.net.clone(),
        fixture.owner.clone(),
        fixture.forum.clone(),
    )
    .await;
    ingest::sync_forum(&restarted.indexer, restarted.forum.uri.as_str())
        .await
        .unwrap();
    let (status, changes, _) = restarted
        .request(&format!("{base}.getChanges?forum={encoded}&since={since}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        changes["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["uri"] == second),
        "restart gap: {changes}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_poll_converges_within_five_seconds() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Polled");
    let task = ingest::spawn(fixture.indexer.clone());
    let start = Instant::now();
    loop {
        if fixture
            .indexer
            .store
            .forums(None)
            .iter()
            .any(|f| f.posts == 1)
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "poll did not converge: {post}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    task.abort();
    let seq = fixture.indexer.store.latest_seq();
    let again = Store::open(&fixture.dir.join("state.sqlite")).unwrap();
    assert_eq!(again.latest_seq(), seq);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proof_omission_and_forgery_are_detectable() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Proof");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let missing = "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.post/aaaaaaaaaaaaa-3l2s5xxv2zaaa";
    let bundle = proofs::bundle(&fixture.indexer, &[post.clone(), missing.into()])
        .await
        .unwrap();
    let parsed = ProofBundle::parse(&bundle, &Default::default()).unwrap();
    assert_eq!(parsed.items.len(), 1, "failed item must be omitted");
    let mut forged = proofs::bundle(&fixture.indexer, &[post]).await.unwrap();
    let n = forged.len();
    forged[n / 2] ^= 1;
    assert!(
        ProofBundle::parse(&forged, &Default::default()).is_err(),
        "existing verifier detects a forged bundle"
    );
}

fn urlencoding(s: &str) -> String {
    lessdugin_atp::xrpc::percent_encode(s)
}
