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
        Self::with(dir, net, owner, forum, &[]).await
    }
    /// With `jetstream` endpoints the service follows them live and does not poll.
    async fn with(
        dir: &Path,
        net: FakeNet,
        owner: AccountHandle,
        forum: ForumKey,
        jetstream: &[Url],
    ) -> Self {
        let paths = Paths::under(dir);
        let config = devnet_config(&net, &owner, jetstream);
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
                jetstream: jetstream.to_vec(),
                poll: jetstream.is_empty().then(|| owner.pds().url()),
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

/// sync.md §15.1 (m6-acceptance-9): a forum whose sync fails retries on its own back-off, from one
/// minute, while discovery keeps its six-hour cadence; a seed whose declaration is proven absent
/// is quarantined instead of being retried as a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_backfill_sync_retries_within_one_minute() {
    let fixture = Fixture::new().await;
    let missing = fixture.forum.uri.as_str().replace("/m6", "/missing");
    fixture.indexer.store.add_forum_seed(&missing).unwrap();
    let unreachable = fixture.owner.pds().add_account("unreachable.test");
    let broken = ForumKey::new(unreachable.did(), "broken").unwrap();
    unreachable.put(
        nsid::FORUM,
        "broken",
        &serde_json::to_value(Forum::create(
            "Broken",
            Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap(),
            MembershipMode::Open,
        ))
        .unwrap(),
    );
    fixture
        .indexer
        .store
        .add_forum_seed(broken.uri.as_str())
        .unwrap();
    fixture.net.inject(
        lessdugin_testkit::fake_pds::Fault::status(500).on_repo(unreachable.did().as_str()),
    );
    let started = fixture.indexer.now_ms();
    let task = ingest::spawn(fixture.indexer.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    let row = |uri: &str| fixture.indexer.store.forum_row(uri).unwrap();
    while row(broken.uri.as_str()).failures == 0 || row(&missing).absent_ms == 0 {
        assert!(Instant::now() < deadline, "backfill did not run");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let retry = row(broken.uri.as_str()).retry_ms;
    assert!(
        retry <= fixture.indexer.now_ms() + 60_001,
        "failed forum sync scheduled {retry} at {}, errors={}",
        fixture.indexer.now_ms(),
        fixture.indexer.metrics.sync_errors.load(Ordering::Relaxed),
    );
    assert!(fixture.indexer.metrics.sync_errors.load(Ordering::Relaxed) > 0);
    // Discovery is not re-run because one forum failed or one declaration is absent.
    let backfill = fixture.indexer.store.scheduled_at("backfill").unwrap();
    assert!(
        backfill >= started + ingest::BACKFILL_EVERY_MS,
        "discovery rescheduled at {backfill}"
    );
    // The absent seed leaves every pass until its daily recheck; the healthy forum is indexed.
    assert!(
        !fixture
            .indexer
            .store
            .forums_for_pass(fixture.indexer.now_ms())
            .contains(&missing)
    );
    assert!(
        fixture
            .indexer
            .store
            .forum(fixture.forum.uri.as_str())
            .is_some()
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

fn devnet_config(net: &FakeNet, owner: &AccountHandle, jetstream: &[Url]) -> Config {
    Config {
        network: NetworkProfile::Devnet(DevnetProfile {
            plc: Url::parse(&net.plc()).unwrap(),
            pds: owner.pds().url(),
            jetstream: None,
        }),
        // Loopback relay endpoints used only by discovery tests, and every loopback PDS and
        // mini-Jetstream origin of the fixture (the devnet guard allowlists host and port).
        // Production endpoint policy is unchanged and still rejects plain HTTP.
        relays: net
            .relays()
            .into_iter()
            .map(|r| Url::parse(&r).unwrap())
            .chain(net.pds_hosts().into_iter().map(|p| p.url()))
            .chain(jetstream.iter().map(|j| {
                let mut http = j.clone();
                http.set_scheme("http").unwrap();
                http
            }))
            .collect(),
        ..Config::default()
    }
}

fn urlencoding(s: &str) -> String {
    lessdugin_atp::xrpc::percent_encode(s)
}

// ---------------------------------------------------------------------------------------------
// Live ingest over the testkit mini-Jetstream (sync.md §15.1).

use lessdugin_indexer::live;
use lessdugin_testkit::jetstream::{Jetstream, Wire as JetstreamWire};

/// The fixture forum, followed live from two mini-Jetstream endpoints (no polling).
async fn live_fixture() -> (Fixture, Vec<Jetstream>, Vec<Url>) {
    let base = Fixture::new().await;
    let jetstreams = vec![
        Jetstream::start(&base.net, JetstreamWire::V2),
        Jetstream::start(&base.net, JetstreamWire::V2),
    ];
    let urls = jetstreams
        .iter()
        .map(|j| Url::parse(j.base_url()).unwrap())
        .collect::<Vec<_>>();
    let fixture = Fixture::with(
        &base.dir,
        base.net.clone(),
        base.owner.clone(),
        base.forum.clone(),
        &urls,
    )
    .await;
    (fixture, jetstreams, urls)
}

/// Keeps the scheduler to retries: no backfill, reconcile or GC pass can find a write for
/// the live path.
fn quiet(indexer: &Indexer) {
    let far = indexer.now_ms() + 365 * 24 * 60 * 60 * 1000;
    for name in ["backfill", "reconcile", "gc"] {
        indexer.store.schedule_next(name, far).unwrap();
    }
}

async fn connected(indexer: &Indexer, n: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while indexer.metrics.live_connections.load(Ordering::Relaxed) < n {
        assert!(
            Instant::now() < deadline,
            "Jetstream endpoints did not connect"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The subscription is open; let the mini-Jetstream take its first snapshot.
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Both endpoints' cursors passed the last event, and no forum sync is pending.
async fn drained(f: &Fixture, urls: &[Url]) {
    let last = f.net.events().last().map_or(0, |e| e.time_us);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let cursors = urls.iter().all(|u| {
            f.indexer
                .store
                .cursor(&format!("jetstream:{}", u.origin().ascii_serialization()))
                .and_then(|c| c.parse::<i64>().ok())
                .is_some_and(|c| c >= last)
        });
        let pending = f
            .indexer
            .store
            .forums(None)
            .iter()
            .any(|r| r.sync_due_ms > 0);
        if cursors && !pending {
            return;
        }
        assert!(Instant::now() < deadline, "the live replay did not drain");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn post_by(f: &Fixture, author: &AccountHandle, title: &str) -> String {
    static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1_000);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let now = Datetime::from_millis(f.net.now_ms() - 3_600_000).unwrap();
    let p = Post::article(f.forum.uri.clone(), title, "A live signed body.", now);
    let rkey = keys::post_rkey(
        f.forum.uri.as_str(),
        lessdugin_testkit::repo::tid_at_ms(f.net.now_ms() + sequence).as_ref(),
    )
    .unwrap();
    author.put(nsid::POST, &rkey, &serde_json::to_value(p).unwrap());
    author.uri(nsid::POST, &rkey)
}

/// Whether `uri` is served by both getChanges (after `since`) and listPosts.
async fn served(f: &Fixture, uri: &str, since: i64) -> bool {
    let base = format!("/xrpc/{NS}.index");
    let encoded = urlencoding(f.forum.uri.as_ref());
    let (_, changes, _) = f
        .request(&format!(
            "{base}.getChanges?forum={encoded}&since={since}&limit=1000"
        ))
        .await;
    let (_, posts, _) = f
        .request(&format!(
            "{base}.listPosts?forum={encoded}&sort=new&limit=100"
        ))
        .await;
    changes["changes"]
        .as_array()
        .is_some_and(|c| c.iter().any(|c| c["uri"] == uri))
        && posts["posts"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p["ref"]["uri"] == uri))
}

async fn until_served(f: &Fixture, uri: &str, since: i64, within: Duration) -> Duration {
    let start = Instant::now();
    loop {
        if served(f, uri, since).await {
            return start.elapsed();
        }
        assert!(
            start.elapsed() < within,
            "{uri} was not served within {within:?}; metrics:\n{}",
            f.indexer.metrics.render(true)
        );
        // Two requests per probe stay under the 20 requests/s per-IP limit.
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

/// m6-acceptance-7, m6-acceptance-1, m6-acceptance-3 and robustness-2: a write made while the
/// service runs reaches getChanges and listPosts within 5 s through Jetstream alone, without
/// a forum-wide sync; the change log carries the commit's own rev and CID; the second endpoint's
/// copy is deduplicated without either endpoint reconnecting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_ingest_on_two_endpoints_serves_a_write_within_five_seconds() {
    let (f, jetstreams, _) = live_fixture().await;
    ingest::sync_forum(&f.indexer, f.forum.uri.as_str())
        .await
        .unwrap();
    let since = f.indexer.store.latest_seq();
    let tasks = live::spawn(f.indexer.clone());
    connected(&f.indexer, 2).await;
    f.net.clear_requests();

    let mut posts = Vec::new();
    for n in 0..3 {
        let post = f.post(&format!("Live {n}"));
        let (cid, rev) = f.owner.head();
        let elapsed = until_served(&f, &post, since, Duration::from_secs(5)).await;
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        posts.push((post, cid, rev));
    }
    let checks = f.net.requests_for("com.atproto.sync.getLatestCommit");
    assert!(
        checks.is_empty(),
        "a live write ran a forum sync: {checks:?}"
    );

    let log = f
        .indexer
        .store
        .changes(Some(f.forum.uri.as_str()), since, 100);
    for (post, _, rev) in &posts {
        let rows = log.iter().filter(|c| &c.uri == post).collect::<Vec<_>>();
        assert_eq!(rows.len(), 1, "applied once: {rows:?}");
        assert_eq!(
            rows[0].rev,
            rev.to_string(),
            "the live commit's rev is logged"
        );
        let record = f
            .indexer
            .store
            .record_cid(f.forum.uri.as_str(), post)
            .unwrap();
        assert_eq!(rows[0].cid.as_deref(), Some(record.as_str()));
    }
    // Both endpoints delivered every frame, each once, on one connection each.
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.indexer.metrics.live_duplicates.load(Ordering::Relaxed) < 3 {
        assert!(
            Instant::now() < deadline,
            "the second endpoint's copies were not deduplicated"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for j in &jetstreams {
        let connections = j.requests().iter().filter(|r| r.options.is_none()).count();
        assert_eq!(
            connections,
            1,
            "an endpoint reconnected: {:?}",
            j.requests()
        );
    }
    for t in tasks {
        t.abort();
    }
}

/// m6-acceptance-1: in a forum of hundreds of participants a live write costs its point fetch
/// and proof only, never one head check per participant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_write_in_a_large_forum_costs_no_forum_wide_sync() {
    const MEMBERS: usize = 200;
    const HOSTS: usize = 8;
    let net = FakeNet::serve_http(SystemClock);
    let hosts = (0..HOSTS)
        .map(|i| net.add_pds(&format!("large-{i}.test")))
        .collect::<Vec<_>>();
    let owner = hosts[0].add_account("owner.test");
    let forum = ForumKey::new(owner.did(), "large").unwrap();
    let now = Datetime::from_millis(net.now_ms() - 3_600_000).unwrap();
    let members = (0..MEMBERS)
        .map(|i| hosts[i % HOSTS].add_account(&format!("member-{i:03}.test")))
        .collect::<Vec<_>>();
    for m in &members {
        let membership = Membership::new(forum.uri.clone(), now.clone());
        m.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(membership).unwrap(),
        );
    }
    owner.batch(|repo| {
        let declaration = Forum::create("Large", now.clone(), MembershipMode::Approval);
        repo.put(
            nsid::FORUM,
            "large",
            &serde_json::to_value(declaration).unwrap(),
        );
        let membership = Membership::new(forum.uri.clone(), now.clone());
        repo.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(membership).unwrap(),
        );
        for m in &members {
            use lessdugin_lexicon::records::{AccessAction, Action, ModAction};
            use lessdugin_lexicon::values::AccessLevel;
            let grant = ModAction::new(
                forum.uri.clone(),
                m.did().as_str(),
                Action::Access(AccessAction::new(AccessLevel::Member)),
                now.clone(),
            );
            let value = serde_json::to_value(grant).unwrap();
            let rkey = keys::mod_action_rkey(
                lessdugin_lexicon::ns::NS,
                forum.uri.as_str(),
                None,
                m.did().as_str(),
                value["action"].as_object().unwrap(),
            )
            .unwrap();
            repo.put(nsid::MOD_ACTION, &rkey, &value);
        }
    });
    net.flush();
    // Setup, not the measured path: an ordinary client fills the shared cache once.
    let dir = TempDir::new().unwrap().keep();
    {
        let config = devnet_config(&net, &owner, &[]);
        let host = NativeHost::open(&Paths::under(&dir), &config).unwrap();
        let engine = lessdugin_core::engine::Engine::new(
            host,
            lessdugin_native::engine::engine_config(&config),
        );
        engine
            .sync_forum(
                forum.uri.as_str(),
                &SyncOpts {
                    full: true,
                    ..SyncOpts::default()
                },
            )
            .await
            .unwrap();
    }
    let jetstreams = [
        Jetstream::start(&net, JetstreamWire::V2),
        Jetstream::start(&net, JetstreamWire::V2),
    ];
    let urls = jetstreams
        .iter()
        .map(|j| Url::parse(j.base_url()).unwrap())
        .collect::<Vec<_>>();
    let f = Fixture::with(&dir, net, owner, forum, &urls).await;
    ingest::project_forum(&f.indexer, &f.forum, None, &Default::default())
        .await
        .unwrap();
    assert_eq!(
        f.indexer
            .store
            .participants(f.forum.uri.as_str(), None)
            .len(),
        MEMBERS + 1
    );
    let since = f.indexer.store.latest_seq();
    let tasks = live::spawn(f.indexer.clone());
    connected(&f.indexer, 2).await;
    f.net.clear_requests();

    let author = &members[MEMBERS / 2];
    let post = post_by(&f, author, "Live in a large forum");
    author.head();
    let elapsed = until_served(&f, &post, since, Duration::from_secs(5)).await;
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    let checks = f.net.requests_for("com.atproto.sync.getLatestCommit");
    let listings = f.net.requests_for("com.atproto.repo.listRecords");
    assert!(
        checks.is_empty() && listings.is_empty(),
        "one live write cost {} head checks and {} listings",
        checks.len(),
        listings.len()
    );
    for t in tasks {
        t.abort();
    }
}

/// security-net-3, robustness-1, m6-acceptance-7: a point fetch that fails, a proof that cannot
/// be fetched and a deleted declaration never hold the cursor. Later writes are served, the
/// failed repository is retried on its own back-off, and the deleted forum is quarantined.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failing_live_events_are_handed_off_and_never_wedge_ingest() {
    let (f, _jetstreams, urls) = live_fixture().await;
    // A participant on its own repository, so faults can target it alone.
    let member = f.owner.pds().add_account("member.test");
    let at = Datetime::from_millis(f.net.now_ms() - 3_600_000).unwrap();
    let membership_cid = member.put(
        nsid::MEMBERSHIP,
        &keys::membership_rkey(f.forum.uri.as_str()).unwrap(),
        &serde_json::to_value(Membership::new(f.forum.uri.clone(), at.clone())).unwrap(),
    );
    f.indexer
        .store
        .add_candidate(
            f.forum.uri.as_str(),
            member.did().as_str(),
            &membership_cid.to_string(),
            10,
        )
        .unwrap();
    // A second forum of the same owner, deleted while the service runs.
    let other = ForumKey::new(f.owner.did(), "other").unwrap();
    f.owner.put(
        nsid::FORUM,
        "other",
        &serde_json::to_value(Forum::create("Other", at.clone(), MembershipMode::Open)).unwrap(),
    );
    f.indexer.store.add_forum_seed(other.uri.as_str()).unwrap();
    for forum in [&f.forum, &other] {
        ingest::sync_forum(&f.indexer, forum.uri.as_str())
            .await
            .unwrap();
    }
    assert!(
        f.indexer
            .store
            .participants(f.forum.uri.as_str(), None)
            .iter()
            .any(|p| p.did == member.did().as_str())
    );
    quiet(&f.indexer);
    let since = f.indexer.store.latest_seq();
    let mut tasks = live::spawn(f.indexer.clone());
    tasks.push(ingest::spawn(f.indexer.clone()));
    connected(&f.indexer, 2).await;
    // The replay of the setup writes (and the roster syncs it asks for) is done first, so only
    // the live path can serve what follows.
    drained(&f, &urls).await;

    // 1. The member's PDS refuses every read of its repository: its post cannot be fetched,
    //    and neither can its retry until the PDS recovers.
    let refused = f
        .net
        .inject(lessdugin_testkit::fake_pds::Fault::status(500).on_repo(member.did().as_str()));
    // 2. No proof can be fetched for the owner's next post.
    let no_proof = f.net.inject(
        lessdugin_testkit::fake_pds::Fault::status(500)
            .on_repo(f.owner.did().as_str())
            .on_nsid("com.atproto.sync.getRecord"),
    );
    let failed = post_by(&f, &member, "Refused by its PDS");
    member.head();
    // 3. The other forum's declaration is deleted.
    f.owner.delete(nsid::FORUM, "other");
    let later = f.post("After the failures");
    f.owner.head();

    let elapsed = until_served(&f, &later, since, Duration::from_secs(5)).await;
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert!(
        f.indexer.store.proof(&later).is_none(),
        "no proof is cached while sync.getRecord fails"
    );
    assert!(!served(&f, &failed, since).await);
    // The refused fetch ends (its transport retries are exhausted) in a durable hand-off.
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f
        .indexer
        .store
        .repo_retries()
        .iter()
        .any(|r| r.did == member.did().as_str() && r.forum == f.forum.uri.as_str())
    {
        assert!(
            Instant::now() < deadline,
            "the refused event was not handed off: {:?}",
            f.indexer.store.repo_retries()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while f
        .indexer
        .store
        .forum_row(other.uri.as_str())
        .is_none_or(|r| r.absent_ms == 0)
    {
        assert!(
            Instant::now() < deadline,
            "the deleted forum was not quarantined"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(f.indexer.store.forum(other.uri.as_str()).is_none());
    let deleted = f
        .indexer
        .store
        .changes(Some(other.uri.as_str()), since, 100);
    assert!(
        deleted
            .iter()
            .any(|c| c.uri == other.uri.as_str() && c.op == "delete"),
        "{deleted:?}"
    );
    // Both cursors moved past every failed frame.
    for url in &urls {
        let name = format!("jetstream:{}", url.origin().ascii_serialization());
        let cursor: i64 = f.indexer.store.cursor(&name).unwrap().parse().unwrap();
        let last = f.net.events().last().unwrap().time_us;
        assert!(cursor >= last, "{name} stopped at {cursor}, before {last}");
    }

    // The PDS recovers: the repository retry path indexes the refused post, not the stream.
    f.net.remove_fault(refused);
    f.net.remove_fault(no_proof);
    until_served(&f, &failed, since, Duration::from_secs(20)).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.indexer.store.repo_retries().is_empty() {
        assert!(Instant::now() < deadline, "the retry was not cleared");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for t in tasks {
        t.abort();
    }
}

/// m6-acceptance-7: a restart resumes from the persisted cursor. A write made while the service
/// was down is served from the Jetstream replay alone, without any ordinary forum sync.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_restart_resumes_from_the_persisted_cursor() {
    let (f, jetstreams, urls) = live_fixture().await;
    ingest::sync_forum(&f.indexer, f.forum.uri.as_str())
        .await
        .unwrap();
    let since = f.indexer.store.latest_seq();
    let tasks = live::spawn(f.indexer.clone());
    connected(&f.indexer, 2).await;
    let first = f.post("Before the restart");
    f.owner.head();
    until_served(&f, &first, since, Duration::from_secs(5)).await;
    let names = urls
        .iter()
        .map(|u| format!("jetstream:{}", u.origin().ascii_serialization()))
        .collect::<Vec<_>>();
    let deadline = Instant::now() + Duration::from_secs(5);
    let persisted = loop {
        let cursors = names
            .iter()
            .map(|n| {
                f.indexer
                    .store
                    .cursor(n)
                    .and_then(|c| c.parse::<i64>().ok())
            })
            .collect::<Option<Vec<_>>>();
        if let Some(c) = cursors {
            break c;
        }
        assert!(Instant::now() < deadline, "cursors were not persisted");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    for t in tasks {
        t.abort();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let down = f.post("Written while the service was down");
    f.owner.head();
    let restarted = Fixture::with(
        &f.dir,
        f.net.clone(),
        f.owner.clone(),
        f.forum.clone(),
        &urls,
    )
    .await;
    restarted.net.clear_requests();
    let before = jetstreams
        .iter()
        .map(|j| j.requests().len())
        .collect::<Vec<_>>();
    let tasks = live::spawn(restarted.indexer.clone());
    until_served(&restarted, &down, since, Duration::from_secs(5)).await;
    let checks = restarted
        .net
        .requests_for("com.atproto.sync.getLatestCommit");
    assert!(
        checks.is_empty(),
        "the restart ran a forum sync: {checks:?}"
    );
    // Each endpoint resumed ten seconds before its own persisted cursor.
    for ((j, n), cursor) in jetstreams.iter().zip(before).zip(persisted) {
        let url = &j.requests()[n].url;
        assert!(
            url.contains(&format!("cursor={}", cursor - 10_000_000)),
            "{url} does not resume from {cursor}"
        );
    }
    for t in tasks {
        t.abort();
    }
}

// ---------------------------------------------------------------------------------------------
// The query surface: cost, blocklist, response shape.

/// Every JSON object member that is null (absent optional fields must be left out).
fn nulls(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Null => out.push(path.to_owned()),
        Value::Object(map) => {
            for (k, v) in map {
                nulls(v, &format!("{path}.{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                nulls(v, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// m6-acceptance-12: only synced forums with a declaration CID are listed, and no response
/// carries a null (a last page has no cursor, a delete no cid, a top-level comment no parent).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn listings_omit_seeds_and_absent_fields() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Shape");
    let gone = fixture.post("Deleted");
    // A seed any membership could name: never synced, so never listed.
    let seed = fixture.forum.uri.as_str().replace("/m6", "/seed");
    fixture.indexer.store.add_forum_seed(&seed).unwrap();
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    fixture
        .owner
        .delete(nsid::POST, gone.rsplit('/').next().unwrap());
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let base = format!("/xrpc/{NS}.index");
    let encoded = urlencoding(fixture.forum.uri.as_ref());
    let (_, forums, _) = fixture.request(&format!("{base}.listForums")).await;
    let listed = forums["forums"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{forums}");
    assert_eq!(listed[0]["ref"]["uri"], fixture.forum.uri.as_str());
    assert!(!listed[0]["ref"]["cid"].as_str().unwrap().is_empty());
    assert!(!listed[0]["ref"]["firstSeen"].as_str().unwrap().is_empty());
    let (_, describe, _) = fixture.request(&format!("{base}.describeIndexer")).await;
    assert!(
        describe["forums"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["uri"] != seed.as_str()),
        "{describe}"
    );
    let mut found = Vec::new();
    for path in [
        format!("{base}.listForums"),
        format!("{base}.describeIndexer"),
        format!("{base}.listParticipants?forum={encoded}"),
        format!("{base}.getRepoHeads?dids={}", fixture.owner.did()),
        format!("{base}.getChanges?forum={encoded}&since=0"),
        format!("{base}.listPosts?forum={encoded}&sort=new"),
        format!("{base}.listRecentDiscussion?forum={encoded}"),
        format!("{base}.getThread?post={}", urlencoding(&post)),
    ] {
        let (status, body, _) = fixture.request(&path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        nulls(&body, &path, &mut found);
    }
    assert!(found.is_empty(), "null members: {found:?}");
    let (_, changes, _) = fixture
        .request(&format!("{base}.getChanges?forum={encoded}&since=0"))
        .await;
    let delete = changes["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["uri"] == gone.as_str() && c["op"] == "delete")
        .expect("the delete is logged");
    assert!(delete.get("cid").is_none(), "{delete}");
}

/// security-net-12: a blocked PDS host hides its accounts from every method, and forums owned
/// on it are no longer advertised.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_host_is_hidden_from_participants_heads_and_forum_listings() {
    let mut fixture = Fixture::new().await;
    fixture.post("Hosted");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let base = format!("/xrpc/{NS}.index");
    let encoded = urlencoding(fixture.forum.uri.as_ref());
    let host = fixture.owner.pds().url().host_str().unwrap().to_owned();
    Arc::get_mut(&mut fixture.indexer.config).unwrap().blocklist = Arc::new(BTreeSet::from([host]));
    let (_, participants, _) = fixture
        .request(&format!("{base}.listParticipants?forum={encoded}"))
        .await;
    assert!(
        participants["participants"].as_array().unwrap().is_empty(),
        "{participants}"
    );
    let (_, heads, _) = fixture
        .request(&format!("{base}.getRepoHeads?dids={}", fixture.owner.did()))
        .await;
    assert!(heads["heads"].as_array().unwrap().is_empty(), "{heads}");
    let (_, forums, _) = fixture.request(&format!("{base}.listForums")).await;
    assert!(forums["forums"].as_array().unwrap().is_empty(), "{forums}");
    let (_, describe, _) = fixture.request(&format!("{base}.describeIndexer")).await;
    assert!(
        describe["forums"].as_array().unwrap().is_empty(),
        "{describe}"
    );
}

/// security-net-13: list and thread queries read the projected model; no request refolds the
/// forum. After a restart the model is folded once, then reused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queries_reuse_the_projected_model_instead_of_refolding() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Folded once");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let base = format!("/xrpc/{NS}.index");
    let encoded = urlencoding(fixture.forum.uri.as_ref());
    let paths = [
        format!("{base}.listPosts?forum={encoded}"),
        format!("{base}.listPosts?forum={encoded}&sort=top"),
        format!("{base}.listRecentDiscussion?forum={encoded}"),
        format!("{base}.getThread?post={}", urlencoding(&post)),
    ];
    for path in &paths {
        for _ in 0..3 {
            let (status, body, _) = fixture.request(path).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }
    let builds = |f: &Fixture| f.indexer.metrics.query_model_builds.load(Ordering::Relaxed);
    assert_eq!(builds(&fixture), 0, "a query refolded a projected forum");
    let restarted = Fixture::at(
        &fixture.dir,
        fixture.net.clone(),
        fixture.owner.clone(),
        fixture.forum.clone(),
    )
    .await;
    for path in &paths {
        for _ in 0..3 {
            let (status, body, _) = restarted.request(path).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    }
    assert_eq!(builds(&restarted), 1, "folded once after the restart");
}

/// security-net-10: getRecordProofs never queues on the ingest budget. A missing proof is
/// verified against the cached (at most one hour old) DID document, without a PLC request, and
/// fetched through a small pool: a request waits a bounded time for a slot, then omits the item.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_proofs_use_cached_identities_and_a_bounded_pool() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Proof on demand");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    // Make the cached proof unusable for the current snapshot, so the request must fetch.
    let mut stale = fixture.indexer.store.proof(&post).unwrap();
    stale.cid = Some("bafyreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    stale.rev = "zzzzzzzzzzzzz".into();
    fixture
        .indexer
        .store
        .put_proof(&stale, fixture.indexer.now_ms())
        .unwrap();
    let path = format!(
        "/xrpc/{NS}.index.getRecordProofs?uris={}",
        urlencoding(&post)
    );
    let fetch = |indexer: Indexer| {
        let path = path.clone();
        async move {
            let response = lessdugin_indexer::api::router(indexer)
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let car = axum::body::to_bytes(response.into_body(), 20_000_000)
                .await
                .unwrap();
            ProofBundle::parse(&car, &Default::default())
                .unwrap()
                .items
                .len()
        }
    };
    // Pool busy for longer than the request's bound: omitted when the bound passes, nothing
    // sent to the author's PDS or to PLC.
    let held = Arc::clone(&fixture.indexer.api_proofs)
        .try_acquire_many_owned(lessdugin_indexer::API_PROOF_FETCHES as u32)
        .unwrap();
    fixture.net.clear_requests();
    let busy = tokio::time::timeout(
        lessdugin_indexer::API_PROOF_WAIT + Duration::from_secs(20),
        fetch(fixture.indexer.clone()),
    )
    .await
    .expect("a busy pool held the request past its bound");
    assert_eq!(busy, 0);
    assert!(
        fixture.net.requests().is_empty(),
        "{:?}",
        fixture.net.requests()
    );
    // A slot that frees within the bound is used: the request waits for it instead of
    // omitting, then the proof is fetched and verified against the cached document.
    fixture.net.clear_requests();
    let waiting = tokio::spawn(fetch(fixture.indexer.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiting.is_finished(), "the request did not wait for a slot");
    drop(held);
    assert_eq!(waiting.await.unwrap(), 1);
    let plc = Url::parse(&fixture.net.plc()).unwrap();
    let to_plc = fixture
        .net
        .requests()
        .into_iter()
        .filter(|r| Url::parse(&r.url).is_ok_and(|u| u.origin() == plc.origin()))
        .collect::<Vec<_>>();
    assert!(
        to_plc.is_empty(),
        "a proof request resolved PLC: {to_plc:?}"
    );
    assert_eq!(
        fixture.net.requests_for("com.atproto.sync.getRecord").len(),
        1
    );
}

/// security-net-10: requests queued on one slow origin (PLC, a hanging PDS) wait for that
/// origin's permit and rate slot before taking a global permit, so they never hold the global
/// budget that every other origin needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_origin_never_holds_the_global_request_budget() {
    use lessdugin_core::host::{HttpRequest, Target};
    let net = FakeNet::serve_http(SystemClock);
    let slow = net.add_pds("slow.test");
    let fast = net.add_pds("fast.test");
    let owner = slow.add_account("owner.test");
    let forum = ForumKey::new(owner.did(), "m6").unwrap();
    let f = Fixture::with(&TempDir::new().unwrap().keep(), net, owner, forum, &[]).await;
    f.net
        .inject(lessdugin_testkit::fake_pds::Fault::latency(3_000).on_host(slow.base()));
    let get = |base: Url| {
        let host = f.indexer.engine.host().clone();
        async move {
            let url = base
                .join("/xrpc/com.atproto.server.describeServer")
                .unwrap();
            host.http(HttpRequest::get(Target::Anon(url))).await
        }
    };
    // More requests queued on the slow origin than there are global permits (200).
    let queued = (0..210)
        .map(|_| tokio::spawn(get(slow.url())))
        .collect::<Vec<_>>();
    tokio::time::sleep(Duration::from_millis(200)).await;
    get(fast.url()).await.unwrap();
    // The other origin was answered while the slow origin's first requests still waited.
    assert!(
        queued.iter().all(|t| !t.is_finished()),
        "a request to another origin waited for the slow origin's queue"
    );
    for t in queued {
        t.abort();
    }
}

/// robustness-12: the daily pass drops tombstones past the 30-day retention from the core record
/// store, and cached proofs of records no forum indexes any more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daily_gc_drops_expired_tombstones_and_orphaned_proofs() {
    let fixture = Fixture::new().await;
    let post = fixture.post("Short-lived");
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    assert!(fixture.indexer.store.proof(&post).is_some());
    fixture
        .owner
        .delete(nsid::POST, post.rsplit('/').next().unwrap());
    ingest::sync_forum(&fixture.indexer, fixture.forum.uri.as_str())
        .await
        .unwrap();
    let uri = lessdugin_atp::syntax::AtUri::parse(&post).unwrap();
    let records =
        lessdugin_core::store::records::RecordStore::new(fixture.indexer.engine.host().cache());
    assert!(records.tombstone_of(&uri).await.unwrap().is_some());
    // Within the retention nothing is dropped.
    ingest::gc(&fixture.indexer, fixture.indexer.now_ms()).await;
    assert!(records.tombstone_of(&uri).await.unwrap().is_some());
    assert!(fixture.indexer.store.proof(&post).is_some());
    let later = fixture.indexer.now_ms() + lessdugin_core::limits::TOMBSTONE_MIN_RETENTION_MS + 1;
    ingest::gc(&fixture.indexer, later).await;
    assert!(records.tombstone_of(&uri).await.unwrap().is_none());
    assert!(fixture.indexer.store.proof(&post).is_none());
    // The scheduler runs the pass once a day.
    let before = fixture.indexer.now_ms();
    ingest::tick(&fixture.indexer).await;
    let next = fixture.indexer.store.scheduled_at("gc").unwrap();
    assert!(next >= before + lessdugin_core::limits::TOMBSTONE_GC_EVERY_MS);
}

/// security-net-11: one account's memberships cannot seed more than its quota of forums.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_caps_the_forum_seeds_of_one_account() {
    let mut fixture = Fixture::new().await;
    let spammer = fixture.owner.pds().add_account("spammer.test");
    let at = Datetime::from_millis(fixture.net.now_ms() - 3_600_000).unwrap();
    let names = (0..lessdugin_indexer::store::SEEDS_PER_DID + 8)
        .map(|n| format!("nowhere{n}"))
        .collect::<Vec<_>>();
    spammer.batch(|repo| {
        for name in &names {
            let forum = ForumKey::new(spammer.did(), name).unwrap();
            let m = Membership::new(forum.uri.clone(), at.clone());
            repo.put(
                nsid::MEMBERSHIP,
                &forum.fk,
                &serde_json::to_value(m).unwrap(),
            );
        }
    });
    Arc::get_mut(&mut fixture.indexer.config).unwrap().relays = fixture
        .net
        .relays()
        .into_iter()
        .map(|r| Url::parse(&r).unwrap())
        .collect();
    ingest::discover(&fixture.indexer).await.unwrap();
    let seeds = fixture.indexer.store.forum_seeds();
    let from_spammer = seeds
        .iter()
        .filter(|s| s.contains(spammer.did().as_str()))
        .count() as i64;
    assert_eq!(from_spammer, lessdugin_indexer::store::SEEDS_PER_DID);
    // The real forum is still discovered.
    assert!(seeds.contains(&fixture.forum.uri.to_string()));
}

/// m6-acceptance-11: the image workflow validates branch pushes and pull requests and publishes
/// only from main or an explicit dispatch.
#[test]
fn the_image_workflow_builds_every_branch_and_publishes_only_from_main() {
    let workflow = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/indexer.yml"),
    )
    .unwrap();
    let on = workflow
        .split("\non:")
        .nth(1)
        .and_then(|rest| rest.split("\npermissions:").next())
        .unwrap();
    assert!(on.contains("pull_request:"), "{on}");
    assert!(on.contains("\"claude/**\""), "{on}");
    let gate = "if: github.event_name == 'workflow_dispatch' || github.ref == 'refs/heads/main'";
    for step in ["docker/login-action", "publish the immutable SHA tag"] {
        let at = workflow.find(step).unwrap();
        assert!(
            workflow[at..].lines().take(3).any(|l| l.trim() == gate),
            "{step} is not gated"
        );
    }
    assert!(
        workflow
            .contains("if: github.ref == 'refs/heads/main'\n        run: |\n          podman tag"),
        "latest is published only from main"
    );
}

/// A live identity or account event of a seed's owner never quarantines a seed that was not
/// synced yet: its declaration is missing from the cache because it was never fetched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_event_never_quarantines_an_unsynced_seed() {
    let (f, _jetstreams, urls) = live_fixture().await;
    let pending = ForumKey::new(f.owner.did(), "pending").unwrap();
    f.owner.put(
        nsid::FORUM,
        "pending",
        &serde_json::to_value(Forum::create(
            "Pending",
            Datetime::from_millis(f.net.now_ms() - 3_600_000).unwrap(),
            MembershipMode::Open,
        ))
        .unwrap(),
    );
    f.indexer
        .store
        .add_forum_seed(pending.uri.as_str())
        .unwrap();
    ingest::sync_forum(&f.indexer, f.forum.uri.as_str())
        .await
        .unwrap();
    quiet(&f.indexer);
    let tasks = live::spawn(f.indexer.clone());
    connected(&f.indexer, 2).await;
    // The replay includes the owner's identity and account events and its commits.
    let deadline = Instant::now() + Duration::from_secs(20);
    let last = f.net.events().last().unwrap().time_us;
    while urls.iter().any(|u| {
        f.indexer
            .store
            .cursor(&format!("jetstream:{}", u.origin().ascii_serialization()))
            .and_then(|c| c.parse::<i64>().ok())
            .is_none_or(|c| c < last)
    }) {
        assert!(Instant::now() < deadline, "the replay did not drain");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let row = f.indexer.store.forum_row(pending.uri.as_str()).unwrap();
    assert_eq!(row.absent_ms, 0, "an unsynced seed was quarantined");
    for t in tasks {
        t.abort();
    }
}
