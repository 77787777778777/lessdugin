// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real native service over the signed loopback devnet: no in-process PDS shortcut.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use lessdugin_atp::proof::ProofBundle;
use lessdugin_atp::syntax::Datetime;
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::host::Host;
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
            // Loopback relay endpoints used only by discovery tests. Production endpoint policy
            // is unchanged and still rejects plain HTTP.
            relays: net
                .relays()
                .into_iter()
                .map(|r| Url::parse(&r).unwrap())
                .collect(),
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
        static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let now = Datetime::from_millis(self.net.now_ms() - 3_600_000).unwrap();
        let p = Post::article(self.forum.uri.clone(), title, "A real signed body.", now);
        let rkey = keys::post_rkey(
            self.forum.uri.as_str(),
            lessdugin_testkit::repo::tid_at_ms(self.net.now_ms() + sequence).as_ref(),
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
async fn open_mode_newcomer_confirmation_clears_the_durable_hint() {
    let fixture = Fixture::new().await;
    let accepted = fixture.owner.pds().add_account("accepted.test");
    let rkey = keys::membership_rkey(fixture.forum.uri.as_str()).unwrap();
    let at = Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap();
    let cid = accepted.put(
        nsid::MEMBERSHIP,
        &rkey,
        &serde_json::to_value(Membership::new(fixture.forum.uri.clone(), at)).unwrap(),
    );
    fixture
        .indexer
        .store
        .add_candidate(
            fixture.forum.uri.as_str(),
            accepted.did().as_str(),
            &cid.to_string(),
            10,
        )
        .unwrap();
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    assert!(
        fixture
            .indexer
            .store
            .candidates(fixture.forum.uri.as_str(), fixture.indexer.now_ms(),)
            .is_empty()
    );
    assert!(
        fixture
            .indexer
            .store
            .participants(fixture.forum.uri.as_str(), None,)
            .iter()
            .any(|p| p.did == accepted.did().as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_relay_hint_does_not_block_a_new_member() {
    let fixture = Fixture::new().await;
    let rejected = fixture.owner.pds().add_account("rejected.test");
    let accepted = fixture.owner.pds().add_account("accepted.test");
    let at = Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap();
    let rkey = keys::membership_rkey(fixture.forum.uri.as_str()).unwrap();
    // The relay may advertise a CID before the PDS can supply its membership. A missing
    // membership is a definitive rejection; the PDS's own record CID is not invented here.
    let cid = accepted.put(
        nsid::MEMBERSHIP,
        &rkey,
        &serde_json::to_value(Membership::new(fixture.forum.uri.clone(), at.clone())).unwrap(),
    );
    fixture
        .indexer
        .store
        .add_candidate(
            fixture.forum.uri.as_str(),
            rejected.did().as_str(),
            &cid.to_string(),
            10,
        )
        .unwrap();
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    assert!(
        fixture
            .indexer
            .store
            .candidates(fixture.forum.uri.as_str(), fixture.indexer.now_ms())
            .is_empty()
    );

    accepted.put(
        nsid::MEMBERSHIP,
        &rkey,
        &serde_json::to_value(Membership::new(fixture.forum.uri.clone(), at)).unwrap(),
    );
    fixture
        .indexer
        .engine
        .sync_forum(
            fixture.forum.uri.as_str(),
            &SyncOpts {
                newcomers: vec![(accepted.did().to_string(), "relay".into())],
                background: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let membership =
        lessdugin_core::store::records::RecordStore::new(fixture.indexer.engine.host().cache())
            .get(
                &lessdugin_atp::syntax::AtUri::parse(&format!(
                    "at://{}/{}/{rkey}",
                    accepted.did(),
                    nsid::MEMBERSHIP
                ))
                .unwrap(),
            )
            .await
            .unwrap();
    assert!(membership.is_some_and(|row| row.valid));
    fixture
        .indexer
        .store
        .add_candidate(
            fixture.forum.uri.as_str(),
            rejected.did().as_str(),
            &cid.to_string(),
            10,
        )
        .unwrap();
    assert!(
        fixture
            .indexer
            .store
            .candidates(fixture.forum.uri.as_str(), fixture.indexer.now_ms())
            .is_empty()
    );
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
async fn a_failed_backfill_sync_retries_within_one_minute() {
    let fixture = Fixture::new().await;
    let missing = fixture.forum.uri.as_str().replace("/m6", "/missing");
    fixture.indexer.store.add_forum_seed(&missing).unwrap();
    let task = ingest::spawn(fixture.indexer.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let failed = fixture.indexer.metrics.sync_errors.load(Ordering::Relaxed) > 0;
        if failed
            && !fixture
                .indexer
                .store
                .schedule_due("backfill", fixture.indexer.now_ms())
        {
            break;
        }
        assert!(Instant::now() < deadline, "backfill did not run");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let retry = fixture.indexer.store.scheduled_at("backfill").unwrap();
    assert!(
        retry <= fixture.indexer.now_ms() + 60_001,
        "failed discovery/sync scheduled {retry} at {}, errors={}",
        fixture.indexer.now_ms(),
        fixture.indexer.metrics.sync_errors.load(Ordering::Relaxed),
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_repository_discovery_is_not_a_successful_backfill() {
    let net = FakeNet::serve_http(SystemClock);
    let pds = net.default_pds();
    let owner = pds.add_account("owner.test");
    let forum = ForumKey::new(owner.did(), "m6").unwrap();
    let declaration = Forum::create(
        "M6",
        Datetime::from_millis(net.now_ms() - 3_600_000).unwrap(),
        MembershipMode::Open,
    );
    owner.put(
        nsid::FORUM,
        "m6",
        &serde_json::to_value(declaration).unwrap(),
    );
    let mut fixture = Fixture::at(&TempDir::new().unwrap().keep(), net, owner, forum).await;
    let member = fixture.owner.pds().add_account("failed-member.test");
    let membership = Membership::new(
        fixture.forum.uri.clone(),
        Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap(),
    );
    member.put(
        nsid::MEMBERSHIP,
        &fixture.forum.fk,
        &serde_json::to_value(membership).unwrap(),
    );
    Arc::get_mut(&mut fixture.indexer.config).unwrap().relays = fixture
        .net
        .relays()
        .into_iter()
        .map(|r| Url::parse(&r).unwrap())
        .collect();
    fixture.net.inject(
        lessdugin_testkit::fake_pds::Fault::status(500)
            .on_repo(member.did().as_str())
            .on_nsid("com.atproto.repo.listRecords"),
    );
    let error = ingest::discover(&fixture.indexer).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("repository discovery passes failed"),
        "{error}"
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
async fn backfill_follows_membership_hints_and_pages_every_record() {
    let net = FakeNet::serve_http(SystemClock);
    let pds = net.default_pds();
    let owner = pds.add_account("owner.test");
    let forum = ForumKey::new(owner.did(), "m6").unwrap();
    let f = Forum::create(
        "M6",
        Datetime::from_millis(net.now_ms() - 3_600_000).unwrap(),
        MembershipMode::Open,
    );
    let declaration = serde_json::to_value(f).unwrap();
    owner.put(nsid::FORUM, "m6", &declaration);
    let membership = Membership::new(
        forum.uri.clone(),
        Datetime::from_millis(net.now_ms() - 3_600_000).unwrap(),
    );
    owner.put(
        nsid::MEMBERSHIP,
        &forum.fk,
        &serde_json::to_value(membership).unwrap(),
    );

    // 101 distinct memberships force a second PDS page. No declaration scan can find the forum
    // from these repositories, so this also tests discovery through membership records.
    for n in 0..101 {
        let member = pds.add_account(&format!("member-{n}.test"));
        let m = Membership::new(
            forum.uri.clone(),
            Datetime::from_millis(net.now_ms() - 3_600_000).unwrap(),
        );
        member.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(m).unwrap(),
        );
    }
    let mut candidate =
        Fixture::at(&TempDir::new().unwrap().keep(), net, owner, forum.clone()).await;
    let relays = candidate
        .net
        .relays()
        .into_iter()
        .map(|r| Url::parse(&r).unwrap())
        .collect::<Vec<_>>();
    Arc::get_mut(&mut candidate.indexer.config).unwrap().relays = relays;
    ingest::discover(&candidate.indexer).await.unwrap();
    assert!(
        candidate
            .indexer
            .store
            .forum_seeds()
            .contains(&forum.uri.to_string())
    );
    let membership_pages = candidate
        .net
        .requests_for("com.atproto.repo.listRecords")
        .into_iter()
        .filter(|r| r.url.contains("membership"))
        .count();
    assert!(
        membership_pages >= 102,
        "only one PDS page was read: {membership_pages}"
    );
    ingest::sync_forum(&candidate.indexer, forum.uri.as_str())
        .await
        .unwrap();
    assert_eq!(
        candidate
            .indexer
            .store
            .participants(forum.uri.as_str(), None)
            .len(),
        102
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daily_reconcile_checks_moved_heads_and_indexes_new_records() {
    let fixture = Fixture::new().await;
    fixture.post("Before reconcile");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();

    fixture.net.clear_requests();
    let post = fixture.post("Found by daily reconcile");
    ingest::reconcile(&fixture.indexer).await;

    let checks = fixture.net.requests_for("com.atproto.sync.getLatestCommit");
    assert!(
        checks
            .iter()
            .any(|r| r.did.as_deref() == Some(fixture.owner.did().as_str())),
        "reconcile did not check the changed PDS head: {checks:?}"
    );
    assert!(
        fixture
            .indexer
            .store
            .record_cid(fixture.forum.uri.as_str(), &post)
            .is_some(),
        "the new signed record did not enter index state"
    );
    assert!(
        fixture
            .indexer
            .store
            .changes(Some(fixture.forum.uri.as_str()), 0, 500)
            .iter()
            .any(|c| c.uri == post),
        "the reconciled record is missing from the durable change log"
    );

    // The owner is also a participant. An unchanged pass must still issue exactly one head check,
    // not one per forum role, and must not need another snapshot refresh to converge.
    fixture.net.clear_requests();
    ingest::reconcile(&fixture.indexer).await;
    let checks = fixture.net.requests_for("com.atproto.sync.getLatestCommit");
    assert_eq!(
        checks.len(),
        1,
        "owner/participant DID was not deduplicated"
    );
    assert_eq!(checks[0].did.as_deref(), Some(fixture.owner.did().as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_rate_limit_uses_the_tcp_peer_not_forwarding_headers() {
    let fixture = Fixture::new().await;
    let endpoint = serve(&fixture.indexer).await.0;
    let client = reqwest::Client::new();
    let url = format!("{endpoint}xrpc/{NS}.index.describeIndexer");
    let mut limited = false;
    for _ in 0..21 {
        let response = client
            .get(&url)
            // A hostile client cannot choose another quota bucket with forwarding headers.
            .header("x-forwarded-for", "203.0.113.99")
            .header("forwarded", "for=203.0.113.99")
            .send()
            .await
            .unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            assert_eq!(response.headers()[header::RETRY_AFTER], "1");
            assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["error"], "RateLimitExceeded");
            break;
        }
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert!(limited, "TCP-peer rate limit did not fire");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proof_bundles_stay_with_indexed_records_and_current_cids() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Current");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let old = proofs::bundle(&fixture.indexer, std::slice::from_ref(&post))
        .await
        .unwrap();
    assert_eq!(
        ProofBundle::parse(&old, &Default::default())
            .unwrap()
            .items
            .len(),
        1
    );

    // A signed record outside the forum's scope is not a proof-source invitation.
    fixture.owner.put(
        nsid::POST,
        "outside",
        &serde_json::to_value(Post::article(
            fixture.forum.uri.clone(),
            "Not indexed",
            "Not an indexed record.",
            Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap(),
        ))
        .unwrap(),
    );
    let unrelated = fixture.owner.uri(nsid::POST, "outside");
    let bundle = proofs::bundle(&fixture.indexer, &[unrelated])
        .await
        .unwrap();
    assert!(
        ProofBundle::parse(&bundle, &Default::default())
            .unwrap()
            .items
            .is_empty()
    );

    let rkey = lessdugin_atp::syntax::AtUri::parse(&post)
        .unwrap()
        .rkey()
        .to_string();
    let edited = Post::article(
        fixture.forum.uri.clone(),
        "Edited",
        "A new signed CID.",
        Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap(),
    );
    fixture
        .owner
        .put(nsid::POST, &rkey, &serde_json::to_value(edited).unwrap());
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let bundle = proofs::bundle(&fixture.indexer, &[post]).await.unwrap();
    let parsed = ProofBundle::parse(&bundle, &Default::default()).unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert_ne!(
        parsed.car.blocks,
        lessdugin_atp::car::read_car(&old, &Default::default())
            .unwrap()
            .blocks
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocklist_filters_content_and_changes_without_stalling_the_cursor() {
    let mut fixture = Fixture::new().await;
    let first = fixture.post("First");
    let second = fixture.post("Blocked");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let cid = fixture
        .indexer
        .store
        .record_cid(fixture.forum.uri.as_str(), &second)
        .unwrap();
    let (debug_status, debug_posts, _) = fixture
        .request(&format!(
            "/xrpc/{NS}.index.listPosts?forum={}&limit=100",
            urlencoding(fixture.forum.uri.as_ref())
        ))
        .await;
    assert_eq!(debug_status, StatusCode::OK);
    assert_eq!(debug_posts["posts"].as_array().unwrap().len(), 2);
    Arc::get_mut(&mut fixture.indexer.config).unwrap().blocklist =
        Arc::new(BTreeSet::from([second.clone(), cid.clone()]));

    let base = format!("/xrpc/{NS}.index");
    let encoded = urlencoding(fixture.forum.uri.as_ref());
    let (status, posts, _) = fixture
        .request(&format!("{base}.listPosts?forum={encoded}&limit=100"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let uris = posts["posts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["ref"]["uri"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(uris.contains(&first.as_str()));
    assert!(!uris.contains(&second.as_str()));

    let seq = fixture.indexer.store.latest_seq();
    let (status, changes, _) = fixture
        .request(&format!(
            "{base}.getChanges?forum={encoded}&since=0&limit=1"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let before = changes["cursor"].as_i64().unwrap();
    let (status, changes, _) = fixture
        .request(&format!(
            "{base}.getChanges?forum={encoded}&since={before}&limit=100"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!changes.to_string().contains(&second));
    assert!(!changes.to_string().contains(&cid));
    assert_eq!(changes["cursor"].as_i64().unwrap(), seq);

    // Both records come from one PDS. Blocking its host hides both, and does not regress the
    // cursor that has already consumed their change entries.
    let host = fixture.owner.pds().url().host_str().unwrap().to_owned();
    Arc::get_mut(&mut fixture.indexer.config).unwrap().blocklist = Arc::new(BTreeSet::from([host]));
    let (status, posts, _) = fixture
        .request(&format!("{base}.listPosts?forum={encoded}&limit=100"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(posts["posts"].as_array().unwrap().is_empty());
    let (status, changes, _) = fixture
        .request(&format!(
            "{base}.getChanges?forum={encoded}&since=0&limit=100"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(changes["changes"].as_array().unwrap().is_empty());
    assert_eq!(changes["cursor"].as_i64().unwrap(), seq);
}

fn without_badges(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("badges");
            for child in map.values_mut() {
                without_badges(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                without_badges(child);
            }
        }
        _ => {}
    }
}

async fn serve(indexer: &Indexer) -> (Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = spawn_server(indexer, listener).await;
    (url, task)
}

async fn serve_at(indexer: &Indexer, url: &Url) -> tokio::task::JoinHandle<()> {
    let address = url.socket_addrs(|| None).unwrap()[0];
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    spawn_server(indexer, listener).await
}

async fn spawn_server(
    indexer: &Indexer,
    listener: tokio::net::TcpListener,
) -> tokio::task::JoinHandle<()> {
    let app = lessdugin_indexer::api::router(indexer.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    })
}

async fn serve_flaky_proofs(
    indexer: &Indexer,
    failures: usize,
) -> (
    Url,
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let inner = indexer.clone();
    let app = axum::Router::new().fallback(move |req: Request<Body>| {
        let inner = inner.clone();
        let calls = Arc::clone(&calls);
        async move {
            if req.uri().path().ends_with(".getRecordProofs")
                && calls.fetch_add(1, Ordering::Relaxed) < failures
            {
                // Exhaust the core transport's initial attempt plus three retries. The live
                // frame itself must then remain unacknowledged and be replayed by live_tail.
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({
                        "error": "InternalServerError",
                        "message": "injected proof outage"
                    })),
                )
                    .into_response();
            }
            lessdugin_indexer::api::router(inner)
                .oneshot(req)
                .await
                .unwrap()
        }
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task, observed)
}

fn client_engine(
    dir: &Path,
    net: &FakeNet,
    owner: &AccountHandle,
    indexer: Url,
    sources: lessdugin_native::config::Sources,
) -> lessdugin_core::engine::Engine<NativeHost> {
    let mut config = Config::devnet(Url::parse(&net.plc()).unwrap(), owner.pds().url());
    config.sources = Some(sources);
    config.indexer = lessdugin_native::config::IndexerSetting::Urls(vec![indexer]);
    let host = NativeHost::open(&Paths::under(dir), &config).unwrap();
    lessdugin_core::engine::Engine::new(host, lessdugin_native::engine::engine_config(&config))
}

/// Joint contract coverage over real TCP, real NativeHost HTTP and the actual service router.
/// No MemHost or in-process service shortcut is used by the client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_service_and_client_have_proof_and_view_parity() {
    use lessdugin_atp::syntax::AtUri;
    use lessdugin_core::discovery::SyncOpts;
    use lessdugin_core::dto::{ListPostsQuery, PostSort};
    use lessdugin_core::host::Host;
    use lessdugin_core::indexer::{Client, Consent, EndpointPolicy, ProofStatus};
    use lessdugin_core::store::records::RecordStore;
    use lessdugin_core::store::{Level, Source};
    use lessdugin_core::sync::{Priority, Stats};
    use lessdugin_native::config::Sources;

    let fixture = Fixture::new().await;
    let post = fixture.post("Joint client");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let (endpoint, server) = serve(&fixture.indexer).await;
    let direct_dir = TempDir::new().unwrap();
    let assisted_dir = TempDir::new().unwrap();
    let direct = client_engine(
        direct_dir.path(),
        &fixture.net,
        &fixture.owner,
        endpoint.clone(),
        Sources::Direct,
    );
    let assisted = client_engine(
        assisted_dir.path(),
        &fixture.net,
        &fixture.owner,
        endpoint.clone(),
        Sources::Assisted,
    );

    // Direct mode has explicit configuration available but still makes no indexer request.
    let query = ListPostsQuery {
        sort: PostSort::New,
        filter: lessdugin_core::dto::PostFilter::All,
        ..ListPostsQuery::new(fixture.forum.uri.as_str())
    };
    let before = fixture.indexer.metrics.requests.load(Ordering::Relaxed);
    assert!(direct.hydrate_indexer_page(&query).await.unwrap().is_none());
    assert_eq!(
        fixture.indexer.metrics.requests.load(Ordering::Relaxed),
        before
    );

    direct
        .sync_forum(
            fixture.forum.uri.as_str(),
            &SyncOpts {
                full: true,
                ..SyncOpts::default()
            },
        )
        .await
        .unwrap();
    assisted
        .sync_forum(
            fixture.forum.uri.as_str(),
            &SyncOpts {
                full: true,
                ..SyncOpts::default()
            },
        )
        .await
        .unwrap();
    let hydrated = assisted
        .hydrate_indexer_page(&query)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hydrated.admitted, 1, "{hydrated:?}");
    let uri = AtUri::parse(&post).unwrap();
    let proven = RecordStore::new(assisted.host().cache())
        .get(&uri)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proven.level, Level::Proven);
    assert_eq!(proven.src, Source::Indexer("did:web:indexer.test".into()));

    direct.verify_handles(100).await;
    assisted.verify_handles(100).await;
    let mut direct_view = serde_json::to_value(direct.list_posts(&query).await.unwrap()).unwrap();
    let mut assisted_view =
        serde_json::to_value(assisted.list_posts(&query).await.unwrap()).unwrap();
    assert_eq!(assisted_view["items"][0]["title"], "Joint client");
    assert_ne!(
        direct_view["items"][0]["badges"]["verified"],
        assisted_view["items"][0]["badges"]["verified"]
    );
    without_badges(&mut direct_view);
    without_badges(&mut assisted_view);
    assert_eq!(direct_view, assisted_view, "only trust badges may differ");

    // Exercise every client method against the actual service, including repeated array keys.
    let session = assisted
        .syncer()
        .session(Priority::P0, Arc::new(Stats::capped(None)));
    let client = Client::connect(
        &session,
        &Consent {
            url: endpoint.clone(),
            did: Some("did:web:indexer.test".parse().unwrap()),
            forum: Some(fixture.forum.uri.to_string()),
        },
        EndpointPolicy::Devnet,
    )
    .await
    .unwrap();
    for feature in lessdugin_indexer::FEATURES {
        assert!(client.supports(feature), "missing feature {feature}");
    }
    assert_eq!(
        client
            .list_forums(&session, None, 10, None)
            .await
            .unwrap()
            .forums
            .len(),
        1
    );
    assert!(
        client
            .list_participants(&session, &fixture.forum.uri, 100, None)
            .await
            .unwrap()
            .participants
            .iter()
            .any(|p| p.did == fixture.owner.did().as_str())
    );
    assert_eq!(
        client
            .get_repo_heads(&session, std::slice::from_ref(fixture.owner.did()))
            .await
            .unwrap()
            .heads
            .len(),
        1
    );
    let changes = client
        .get_changes(&session, Some(&fixture.forum.uri), 0, 500)
        .await
        .unwrap();
    assert!(!changes.changes.is_empty());
    let since = changes.cursor;
    assert!(
        client
            .list_posts(&session, &fixture.forum.uri, "new", 25, None)
            .await
            .unwrap()
            .posts
            .iter()
            .any(|p| p.record.uri == post)
    );
    client
        .recent_discussion_ordering(&session, &fixture.forum.uri, 20, None)
        .await
        .unwrap();
    client.thread_ordering(&session, &uri, "new").await.unwrap();
    let car = client
        .get_record_proofs(&session, std::slice::from_ref(&uri))
        .await
        .unwrap();
    assert_eq!(
        ProofBundle::parse(&car, &Default::default())
            .unwrap()
            .items
            .len(),
        1
    );

    // A valid but old cached service proof cannot pin an edited record. The client checks the
    // author's head and point-fetches the current value from the PDS.
    let rkey = post.rsplit('/').next().unwrap();
    let now = Datetime::from_millis(fixture.net.now_ms()).unwrap();
    let edited = Post::article(
        fixture.forum.uri.clone(),
        "Edited at the PDS",
        "Current body",
        now,
    );
    fixture
        .owner
        .put(nsid::POST, rkey, &serde_json::to_value(edited).unwrap());
    let stale = assisted
        .hydrate_indexer_records(fixture.forum.uri.as_str(), std::slice::from_ref(&uri))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale.items[0].status, ProofStatus::StaleProof, "{stale:?}");
    assert!(stale.items[0].point_fetched);
    let current = RecordStore::new(assisted.host().cache())
        .get(&uri)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.level, Level::Pds);
    assert_eq!(current.value_json().unwrap()["title"], "Edited at the PDS");

    // A forged cached CAR is never admitted, independently of the stale-proof path.
    let mut forged = fixture.indexer.store.proof(&post).unwrap();
    let middle = forged.car.len() / 2;
    forged.car[middle] ^= 1;
    fixture
        .indexer
        .store
        .put_proof(&forged, fixture.indexer.now_ms())
        .unwrap();
    let rejected = assisted
        .hydrate_indexer_records(fixture.forum.uri.as_str(), std::slice::from_ref(&uri))
        .await
        .unwrap()
        .unwrap();
    assert!(rejected.items[0].point_fetched, "{rejected:?}");
    assert!(rejected.admitted + rejected.absent == 0, "{rejected:?}");
    assert_eq!(
        RecordStore::new(assisted.host().cache())
            .get(&uri)
            .await
            .unwrap()
            .unwrap()
            .level,
        Level::Pds
    );

    // Restart the real service on its existing state. The client's durable change cursor must
    // resume without skipping a write made before that restart.
    let second = fixture.post("Across service restart");
    let first_poll = assisted
        .poll_indexer_changes(fixture.forum.uri.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(first_poll.cursor >= first_poll.from);
    server.abort();
    let _ = server.await;
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
    let restarted_server = serve_at(&restarted.indexer, &endpoint).await;
    let resumed = assisted
        .poll_indexer_changes(fixture.forum.uri.as_str())
        .await
        .unwrap()
        .unwrap();
    assert!(resumed.from >= since, "{resumed:?}");
    assert!(resumed.changes >= 1, "{resumed:?}");
    assert!(resumed.cursor > resumed.from, "{resumed:?}");
    assert!(
        restarted
            .indexer
            .store
            .changes(Some(restarted.forum.uri.as_str()), since, 500)
            .iter()
            .any(|c| c.uri == second)
    );
    restarted_server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_proof_batches_replay_failures_without_losing_frames() {
    use lessdugin_atp::syntax::AtUri;
    use lessdugin_core::discovery::SyncOpts;
    use lessdugin_core::host::Host;
    use lessdugin_core::store::Level;
    use lessdugin_core::store::records::RecordStore;
    use lessdugin_native::config::Sources;
    use lessdugin_testkit::jetstream::{Jetstream, Wire};

    let fixture = Fixture::new().await;
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let indexing = ingest::spawn(fixture.indexer.clone());
    let (endpoint, server, proof_calls) = serve_flaky_proofs(&fixture.indexer, 4).await;
    let jetstream = Jetstream::start(&fixture.net, Wire::V2);
    let mut config = Config::devnet(
        Url::parse(&fixture.net.plc()).unwrap(),
        fixture.owner.pds().url(),
    );
    config.sources = Some(Sources::Assisted);
    config.indexer = lessdugin_native::config::IndexerSetting::Urls(vec![endpoint]);
    let NetworkProfile::Devnet(devnet) = &mut config.network else {
        unreachable!()
    };
    devnet.jetstream = Some(Url::parse(jetstream.base_url()).unwrap());
    let client_dir = TempDir::new().unwrap();
    let host = NativeHost::open(&Paths::under(client_dir.path()), &config).unwrap();
    let engine =
        lessdugin_core::engine::Engine::new(host, lessdugin_native::engine::engine_config(&config));
    engine.forum_open(fixture.forum.uri.as_str()).await.unwrap();
    engine
        .sync_forum(
            fixture.forum.uri.as_str(),
            &SyncOpts {
                full: true,
                ..SyncOpts::default()
            },
        )
        .await
        .unwrap();
    engine
        .forum_follow(fixture.forum.uri.as_str(), None)
        .await
        .unwrap();
    fixture.net.clear_requests();

    let tail = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.live_tail().await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let first = fixture.post("Live proof one");
    let second = fixture.post("Live proof two");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let first_view = engine.get_post(&first).await;
        let second_view = engine.get_post(&second).await;
        if let (Ok(first_view), Ok(second_view)) = (first_view, second_view) {
            assert_eq!(first_view.post.title.as_deref(), Some("Live proof one"));
            assert_eq!(second_view.post.title.as_deref(), Some("Live proof two"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "live proof replay stalled; calls={}; requests={:?}",
            proof_calls.load(Ordering::Relaxed),
            fixture.net.requests()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(proof_calls.load(Ordering::Relaxed) >= 5);
    let proof_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let row = RecordStore::new(engine.host().cache())
            .get(&AtUri::parse(&second).unwrap())
            .await
            .unwrap()
            .unwrap();
        if row.level == Level::Proven {
            break;
        }
        assert!(
            Instant::now() < proof_deadline,
            "newest proof never became current; requests={:?}",
            fixture.net.requests()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let older = RecordStore::new(engine.host().cache())
        .get(&AtUri::parse(&first).unwrap())
        .await
        .unwrap()
        .unwrap();
    // The first may already have become proven before the second commit arrived. A later head
    // does not revoke that record's earlier verified admission.
    assert!(matches!(older.level, Level::Pds | Level::Proven));
    let point_fetches = fixture.net.requests_for("com.atproto.repo.getRecord");
    assert!(
        !point_fetches.iter().any(|r| {
            r.did.as_deref() == Some(fixture.owner.did().as_str())
                && r.url.contains("lessdugin.temp.post")
        }),
        "successful live proofs must not fall back to post point fetches: {point_fetches:?}"
    );
    engine.live_stop();
    tokio::time::timeout(Duration::from_secs(5), tail)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    indexing.abort();
    server.abort();
}

fn urlencoding(s: &str) -> String {
    lessdugin_atp::xrpc::percent_encode(s)
}
