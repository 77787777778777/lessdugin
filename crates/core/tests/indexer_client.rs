// SPDX-License-Identifier: AGPL-3.0-or-later
//! M6 assisted indexer integration over real signed fake repositories and MemHost.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{POST, RKEY, World, block_on};
use lessdugin_atp::Ipld;
use lessdugin_atp::car::write_car;
use lessdugin_atp::dagcbor;
use lessdugin_atp::syntax::AtUri;
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::engine::{Engine, EngineConfig, IndexerConsent, IndexerMode, Sources};
use lessdugin_core::host::Host;
use lessdugin_core::indexer::{EndpointPolicy, LiveProofBatch, ProofStatus};
use lessdugin_core::store::records::RecordStore;
use lessdugin_core::store::{Level, Source};
use lessdugin_core::sync::{Priority, Stats};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::records::{Forum, Membership, Post};
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::identity::fake_key;
use lessdugin_testkit::repo::{TestRepo, tid_at_ms};
use lessdugin_testkit::{FakeNet, MemHost, resp};
use serde_json::{Value, json};

const INDEX: &str = "https://index.example.com";

fn endpoint(method: &str) -> String {
    format!(
        "{INDEX}/xrpc/{}",
        lessdugin_core::indexer::contract::method_nsid(method)
    )
}

fn engine(host: &MemHost, sources: Sources) -> Engine<MemHost> {
    Engine::new(
        host.clone(),
        EngineConfig {
            sources,
            indexers: vec![IndexerConsent {
                url: url::Url::parse(INDEX).unwrap(),
            }],
            ..EngineConfig::default()
        },
    )
}

fn describe(host: &MemHost, features: &[&str], seq: i64) {
    host.respond_get(
        &endpoint("describeIndexer"),
        resp::ok_json(&json!({
            "did": "did:web:index.example.com",
            "version": "1",
            "coreVersion": "test",
            "policies": [],
            "features": features,
            "forums": [],
            "asOf": {"seq": seq, "timeUs": 1_760_000_000_000_000i64, "lagMs": 0},
            "indexer": {"did": "did:web:index.example.com", "coreVersion": "test"}
        })),
    );
}

fn with_meta(mut value: Value, seq: i64) -> Value {
    let metadata = json!({
        "asOf": {"seq": seq, "timeUs": 1_760_000_000_000_000i64, "lagMs": 0},
        "indexer": {"did": "did:web:index.example.com", "coreVersion": "test"}
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(metadata.as_object().unwrap().clone());
    value
}

fn bundle(repo: &TestRepo, rev: lessdugin_atp::tid::Tid, forged: bool) -> Vec<u8> {
    bundle_many(repo, rev, &[RKEY], forged)
}

fn bundle_many(
    repo: &TestRepo,
    rev: lessdugin_atp::tid::Tid,
    rkeys: &[&str],
    forged: bool,
) -> Vec<u8> {
    bundle_in(repo, rev, POST, rkeys, forged)
}

fn bundle_in(
    repo: &TestRepo,
    rev: lessdugin_atp::tid::Tid,
    coll: &str,
    rkeys: &[&str],
    forged: bool,
) -> Vec<u8> {
    let commit = if forged {
        repo.commit_signed_by(rev, &fake_key("99"))
    } else {
        repo.commit(rev)
    };
    let mut items = Vec::new();
    let mut all = BTreeMap::new();
    for rkey in rkeys {
        let mut item = BTreeMap::new();
        item.insert(
            "uri".to_owned(),
            Ipld::String(format!("at://{}/{coll}/{rkey}", repo.did())),
        );
        item.insert(
            "cid".to_owned(),
            repo.cid(coll, rkey).map_or(Ipld::Null, Ipld::Link),
        );
        item.insert("commit".to_owned(), Ipld::Link(commit.cid));
        items.push(Ipld::Map(item));
        all.extend(repo.proof_blocks(coll, rkey, &commit));
    }
    let mut manifest = BTreeMap::new();
    manifest.insert("v".to_owned(), Ipld::Integer(1));
    manifest.insert("items".to_owned(), Ipld::List(items));
    let manifest = dagcbor::encode(&Ipld::Map(manifest)).unwrap();
    let root = lessdugin_atp::cid::for_dag_cbor(&manifest);
    all.insert(root, manifest);
    write_car(&[root], all.iter().map(|(c, b)| (c, b.as_slice())))
}

fn current_proof_world(sources: Sources, forged: bool) -> (World, Engine<MemHost>, AtUri) {
    let w = World::new();
    let rev = tid_at_ms(w.host.now() - 60_000);
    let car = w
        .pds
        .update(&w.alice.did, None, |repo| bundle(repo, rev, forged));
    describe(&w.host, &["getRecordProofs"], 0);
    w.host
        .respond_get(&endpoint("getRecordProofs"), resp::car(car));
    let e = engine(&w.host, sources);
    let uri = AtUri::parse(&w.uri(RKEY)).unwrap();
    (w, e, uri)
}

#[test]
fn direct_mode_generates_no_indexer_traffic() {
    let (w, e, uri) = current_proof_world(Sources::Direct, false);
    w.host.clear_requests();
    let report = block_on(
        e.hydrate_indexer_records(&format!("at://{}/{}/f", w.alice.did, nsid::FORUM), &[uri]),
    )
    .unwrap();
    assert!(report.is_none());
    let query =
        lessdugin_core::dto::ListPostsQuery::new(format!("at://{}/{}/f", w.alice.did, nsid::FORUM));
    assert!(block_on(e.hydrate_indexer_page(&query)).unwrap().is_none());
    assert!(w.host.requests_to(INDEX).is_empty());
    assert!(
        !block_on(RecordStore::new(w.host.cache()).repo(w.alice.did.as_str()))
            .unwrap()
            .is_some_and(|r| r.head_seen.is_some())
    );
}

#[test]
fn none_mode_ignores_configured_and_stored_consent_without_indexer_traffic() {
    let (w, _, uri) = current_proof_world(Sources::Assisted, false);
    let e = Engine::new(
        w.host.clone(),
        EngineConfig {
            sources: Sources::Assisted,
            indexer_mode: IndexerMode::None,
            indexers: vec![IndexerConsent {
                url: url::Url::parse(INDEX).unwrap(),
            }],
            ..EngineConfig::default()
        },
    );
    block_on(e.set_setting("indexers", Some(&json!([{"url": INDEX, "grantedAtMs": 1}])))).unwrap();
    w.host.clear_requests();
    let forum = format!("at://{}/{}/f", w.alice.did, nsid::FORUM);
    assert!(
        block_on(e.hydrate_indexer_records(&forum, &[uri]))
            .unwrap()
            .is_none()
    );
    assert!(w.host.requests_to(INDEX).is_empty());
}

fn declaration_with_indexers(w: &World, indexers: Value) -> String {
    w.pds.update(&w.alice.did, None, |repo| {
        repo.put(
            nsid::FORUM,
            "f",
            &json!({
                "$type": nsid::FORUM,
                "name": "Forum",
                "membership": "open",
                "defaultTier": "personal",
                "createdAt": "2025-01-01T00:00:00Z",
                "discovery": {"indexers": indexers}
            }),
        );
    });
    format!("at://{}/{}/f", w.alice.did, nsid::FORUM)
}

#[test]
fn forum_consent_requires_the_exact_cached_validated_declaration_endpoint() {
    let w = World::new();
    let forum = declaration_with_indexers(
        &w,
        json!([{
            "url": INDEX,
            "did": "did:web:index.example.com",
            "note": "community indexer"
        }]),
    );
    let e = Engine::new(
        w.host.clone(),
        EngineConfig {
            sources: Sources::Assisted,
            ..EngineConfig::default()
        },
    );
    block_on(e.set_setting(
        "indexers",
        Some(&json!([
            {"url": "https://attacker.example.com", "did": "did:web:attacker.example.com",
             "forum": forum, "grantedAtMs": 1},
            {"url": INDEX, "did": "did:web:attacker.example.com",
             "forum": forum, "grantedAtMs": 2}
        ])),
    ))
    .unwrap();

    let declared = block_on(e.forum_indexers(&forum)).unwrap();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].url, INDEX);
    assert_eq!(
        declared[0].did.as_deref(),
        Some("did:web:index.example.com")
    );
    assert_eq!(declared[0].note.as_deref(), Some("community indexer"));
    assert!(declared[0].consented);

    // Selection replaces the untrusted stored DID with the validated declaration DID.
    let rev = tid_at_ms(w.host.now() - 60_000);
    let car = w
        .pds
        .update(&w.alice.did, None, |repo| bundle(repo, rev, false));
    describe(&w.host, &["getRecordProofs"], 0);
    w.host
        .respond_get(&endpoint("getRecordProofs"), resp::car(car));
    let uri = AtUri::parse(&w.uri(RKEY)).unwrap();
    assert_eq!(
        block_on(e.hydrate_indexer_records(&forum, &[uri]))
            .unwrap()
            .unwrap()
            .admitted,
        1
    );
    assert!(
        w.host
            .requests_to("https://attacker.example.com")
            .is_empty()
    );
}

#[test]
fn stored_consent_survives_restart_and_other_forum_consent_does_not_leak() {
    let w = World::new();
    let forum = declaration_with_indexers(&w, json!([{ "url": INDEX }]));
    let e = Engine::new(
        w.host.clone(),
        EngineConfig {
            sources: Sources::Assisted,
            ..EngineConfig::default()
        },
    );
    block_on(e.set_setting(
        "indexers",
        Some(&json!([
            {"url": INDEX, "forum": "at://did:plc:other/online.7778777.lessdugin.temp.forum/f",
             "grantedAtMs": 1}
        ])),
    ))
    .unwrap();
    assert!(!block_on(e.forum_indexers(&forum)).unwrap()[0].consented);

    block_on(e.set_setting(
        "indexers",
        Some(&json!([{"url": INDEX, "forum": forum, "grantedAtMs": 2}])),
    ))
    .unwrap();
    let restarted = Engine::new(
        w.host.clone(),
        EngineConfig {
            sources: Sources::Assisted,
            ..EngineConfig::default()
        },
    );
    assert!(block_on(restarted.forum_indexers(&forum)).unwrap()[0].consented);

    let rev = tid_at_ms(w.host.now() - 60_000);
    let car = w
        .pds
        .update(&w.alice.did, None, |repo| bundle(repo, rev, false));
    describe(&w.host, &["getRecordProofs"], 0);
    w.host
        .respond_get(&endpoint("getRecordProofs"), resp::car(car));
    let uri = AtUri::parse(&w.uri(RKEY)).unwrap();
    assert_eq!(
        block_on(restarted.hydrate_indexer_records(&forum, &[uri]))
            .unwrap()
            .unwrap()
            .admitted,
        1
    );
}

#[test]
fn oversized_bundles_halve_then_use_pds_for_one_oversized_item() {
    let w = World::new();
    let rev = tid_at_ms(w.host.now() - 60_000);
    describe(&w.host, &["getRecordProofs"], 0);
    let first = w
        .pds
        .update(&w.alice.did, None, |repo| bundle(repo, rev, false));
    let second = w.pds.update(&w.alice.did, None, |repo| {
        bundle_many(repo, rev, &["3l2s5xxv2zaa3"], false)
    });
    w.host.respond_seq(
        &endpoint("getRecordProofs"),
        vec![
            Err(lessdugin_core::host::HostError::TooLarge),
            Ok(resp::car(first)),
            Ok(resp::car(second)),
        ],
    );
    let e = engine(&w.host, Sources::Assisted);
    let forum = format!("at://{}/{}/f", w.alice.did, nsid::FORUM);
    let uris = [RKEY, "3l2s5xxv2zaa3"].map(|r| AtUri::parse(&w.uri(r)).unwrap());
    let report = block_on(e.hydrate_indexer_records(&forum, &uris))
        .unwrap()
        .unwrap();
    assert_eq!(report.admitted, 2, "{report:?}");
    assert_eq!(w.host.requests_to(&endpoint("getRecordProofs")).len(), 3);

    let w = World::new();
    describe(&w.host, &["getRecordProofs"], 0);
    w.host.fail(
        &endpoint("getRecordProofs"),
        lessdugin_core::host::HostError::TooLarge,
    );
    let e = engine(&w.host, Sources::Assisted);
    let uri = AtUri::parse(&w.uri(RKEY)).unwrap();
    let report = block_on(e.hydrate_indexer_records(&forum, std::slice::from_ref(&uri)))
        .unwrap()
        .unwrap();
    // Still too large at one URI: fetched from the PDS, not a failed proof (sync.md §8.6).
    assert_eq!(report.items[0].status, ProofStatus::TooLarge);
    assert_eq!((report.too_large, report.failed), (1, 0));
    assert_eq!(report.point_fetched, 1);
    assert_eq!(w.host.requests_to(&endpoint("getRecordProofs")).len(), 1);
    assert_eq!(
        block_on(RecordStore::new(w.host.cache()).get(&uri))
            .unwrap()
            .unwrap()
            .level,
        Level::Pds
    );
}

#[test]
fn unavailable_proof_service_falls_back_to_the_authors_pds() {
    let w = World::new();
    describe(&w.host, &["getRecordProofs"], 0);
    w.host.fail(
        &endpoint("getRecordProofs"),
        lessdugin_core::host::HostError::Network("offline".into()),
    );
    let e = engine(&w.host, Sources::Assisted);
    let forum = format!("at://{}/{}/f", w.alice.did, nsid::FORUM);
    let uri = AtUri::parse(&w.uri(RKEY)).unwrap();
    let report = block_on(e.hydrate_indexer_records(&forum, std::slice::from_ref(&uri)))
        .unwrap()
        .unwrap();
    assert_eq!(report.point_fetched, 1, "{report:?}");
    // No proof was presented, so nothing failed proofs/1 (sync.md §15.4 A5).
    assert_eq!(report.items[0].status, ProofStatus::Unavailable);
    assert_eq!((report.unavailable, report.failed), (1, 0));
    assert_eq!(
        block_on(RecordStore::new(w.host.cache()).get(&uri))
            .unwrap()
            .unwrap()
            .level,
        Level::Pds
    );
}

#[test]
fn visible_page_uses_only_uri_hints_then_hydrates_proofs() {
    let (w, e, outside) = current_proof_world(Sources::Assisted, false);
    let forum = format!("at://{}/{}/f", w.alice.did, nsid::FORUM);
    // listPosts(forum) names posts of that forum, whose rkeys are in its {fk}- range
    // (data-model.md §6.3); the world's own post is outside every forum's range.
    let rkey = keys::post_rkey(&forum, tid_at_ms(w.host.now() - 120_000).as_ref()).unwrap();
    let rev = tid_at_ms(w.host.now() - 60_000);
    let car = w.pds.update(&w.alice.did, None, |repo| {
        let value = json!({"$type": nsid::POST, "title": "hello", "n": 1});
        repo.put(nsid::POST, &rkey, &value);
        bundle_in(repo, rev, nsid::POST, &[&rkey], false)
    });
    w.host
        .respond_get(&endpoint("getRecordProofs"), resp::car(car));
    let uri = AtUri::parse(&format!("at://{}/{}/{rkey}", w.alice.did, nsid::POST)).unwrap();
    describe(&w.host, &["listPosts", "getRecordProofs"], 0);
    w.host.respond_get(
        &endpoint("listPosts"),
        resp::ok_json(&with_meta(
            json!({
                "posts": [{"ref": {"uri": outside.to_string()}, "effectiveAt": "2025-01-01T00:00:00Z"}, {
                    "ref": {"uri": uri.to_string(), "value": {"title": "UNTRUSTED"}},
                    "effectiveAt": "2025-01-01T00:00:00Z",
                    "state": "visible",
                    "tally": {"subject": uri.to_string(), "policy": "karma/1", "score": 999999}
                }],
                "policy": {"karma": "karma/1", "ranking": "magic/1"},
                "inputDigest": "00"
            }),
            0,
        )),
    );
    let query = lessdugin_core::dto::ListPostsQuery::new(&forum);
    let report = block_on(e.hydrate_indexer_page(&query)).unwrap().unwrap();
    assert_eq!(report.admitted, 1, "{report:?}");
    let stored = block_on(RecordStore::new(w.host.cache()).get(&uri))
        .unwrap()
        .unwrap();
    assert_eq!(stored.value_json().unwrap()["title"], "hello");
    assert_ne!(stored.value_json().unwrap()["title"], "UNTRUSTED");
    let ordering = w.host.requests_to(&endpoint("listPosts"));
    assert_eq!(ordering.len(), 1);
    assert!(ordering[0].url.contains("hydrate=false"));
    // The listed record outside the forum's range was neither requested nor fetched.
    assert_eq!(report.requested, 1, "{report:?}");
    let proofs = w.host.requests_to(&endpoint("getRecordProofs"));
    assert!(proofs.iter().all(|r| !r.url.contains(RKEY)), "{proofs:?}");
    assert!(
        block_on(RecordStore::new(w.host.cache()).get(&outside))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_current_proof_has_direct_sync_content_parity() {
    let (w, assisted, uri) = current_proof_world(Sources::Assisted, false);
    let forum = format!("at://{}/{}/f", w.alice.did, nsid::FORUM);
    let report = block_on(assisted.hydrate_indexer_records(&forum, std::slice::from_ref(&uri)))
        .unwrap()
        .unwrap();
    assert_eq!(report.admitted, 1, "{report:?}");
    assert_eq!(report.failed, 0);
    let proven = block_on(RecordStore::new(w.host.cache()).get(&uri))
        .unwrap()
        .unwrap();
    assert_eq!(proven.level, Level::Proven);
    assert_eq!(
        proven.src,
        Source::Indexer("did:web:index.example.com".into())
    );

    let direct_world = World::new();
    let direct = common::direct_engine(&direct_world.host);
    let syncer = direct.syncer();
    let session = syncer.session(Priority::P0, Arc::new(Stats::capped(None)));
    block_on(syncer.point_fetch(&session, &uri, None)).unwrap();
    let pds = block_on(RecordStore::new(direct_world.host.cache()).get(&uri))
        .unwrap()
        .unwrap();
    assert_eq!(proven.cid, pds.cid);
    assert_eq!(proven.cbor, pds.cbor);
    assert_eq!(proven.valid, pds.valid);
    assert_eq!(proven.value_json().unwrap(), pds.value_json().unwrap());
    let requests = w.host.requests_to(&endpoint("getRecordProofs"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].max_bytes, 8 * 1024 * 1024);
    assert_eq!(
        requests[0].header("accept"),
        Some("application/vnd.ipld.car")
    );
}

#[test]
fn stale_and_forged_proofs_never_remain_proven() {
    for (forged, advance_head) in [(false, true), (true, true), (true, false)] {
        let (w, e, uri) = current_proof_world(Sources::Assisted, forged);
        if advance_head {
            // The old-rev CAR has a bad signature against the current key. Advance only the PDS
            // head to exercise key-rotation stale-proof handling (not a current-head forgery).
            w.pds
                .update(&w.alice.did, Some(tid_at_ms(w.host.now())), |_| {});
        }
        let report = block_on(e.hydrate_indexer_records(
            &format!("at://{}/{}/f", w.alice.did, nsid::FORUM),
            std::slice::from_ref(&uri),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(report.admitted, 0, "{report:?}");
        assert_eq!(
            report.items[0].status,
            if advance_head {
                ProofStatus::StaleProof
            } else {
                ProofStatus::Failed
            }
        );
        assert_eq!(report.point_fetched, 1);
        let row = block_on(RecordStore::new(w.host.cache()).get(&uri))
            .unwrap()
            .unwrap();
        if forged && advance_head {
            // A bad signature below the new head is stale-proof: recover with a fresh PDS CAR
            // without blaming the indexer, rather than treating it as a current-head forgery.
            assert_eq!(row.level, Level::Proven, "{report:?} {row:?}");
            assert_eq!(row.src, Source::Proof);
        } else {
            assert_eq!(row.level, Level::Pds);
            assert_eq!(row.src, Source::Get);
        }
        let origin = url::Url::parse(INDEX)
            .unwrap()
            .origin()
            .ascii_serialization();
        let strikes = block_on(RecordStore::new(w.host.cache()).host_caps(&origin))
            .unwrap()
            .unwrap_or_default()
            .strikes;
        assert_eq!(strikes, u32::from(forged && !advance_head));
    }
}

#[test]
fn cold_page_falls_back_to_pds_sync_when_the_indexer_is_unavailable() {
    let w = forum_world();
    w.host.fail(
        &endpoint("listPosts"),
        lessdugin_core::host::HostError::Network("offline".into()),
    );
    let mut query = lessdugin_core::dto::ListPostsQuery::new(w.key.uri.to_string());
    query.filter = lessdugin_core::dto::PostFilter::All;
    let page = block_on(
        w.engine
            .list_posts_page(&query, &lessdugin_core::dto::SyncOpts::default()),
    )
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].title.as_deref(), Some("Title"));
    assert_eq!(
        block_on(RecordStore::new(w.host.cache()).get(&AtUri::parse(&w.post).unwrap()))
            .unwrap()
            .unwrap()
            .level,
        Level::Pds
    );

    // No consented/usable indexer is also a cold PDS fallback, not a reason to stay empty.
    let w = forum_world();
    block_on(w.engine.set_setting("indexers", Some(&json!([])))).unwrap();
    let page = block_on(
        w.engine
            .list_posts_page(&query, &lessdugin_core::dto::SyncOpts::default()),
    )
    .unwrap();
    assert_eq!(page.items[0].title.as_deref(), Some("Title"));
}

#[test]
fn live_batches_are_bounded_and_timed() {
    let mut batch = LiveProofBatch::new(1_000, 0);
    assert!(!batch.is_due(2_999));
    let uri = AtUri::parse(&format!(
        "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{POST}/{RKEY}"
    ))
    .unwrap();
    for i in 0..25 {
        let u = AtUri::parse(&format!(
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{POST}/3l2s5xxv2z{i:03}"
        ))
        .unwrap();
        batch.push(u.clone());
        batch.push(u);
    }
    assert!(batch.is_full());
    assert!(batch.is_due(1_000));
    assert_eq!(batch.take(1_000).len(), 25, "duplicate URIs coalesce");
    batch.push(uri.clone());
    batch.push(uri);
    assert_eq!(batch.len(), 1);
}

struct ForumWorld {
    host: MemHost,
    _net: FakeNet,
    engine: Engine<MemHost>,
    owner: AccountHandle,
    key: ForumKey,
    post: String,
}

fn forum_world() -> ForumWorld {
    forum_world_in(MembershipMode::Approval)
}

fn forum_world_in(mode: MembershipMode) -> ForumWorld {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("alice.example.com");
    let key = ForumKey::new(owner.did(), "meta").unwrap();
    let dt = lessdugin_atp::syntax::Datetime::from_millis(host.now() - 3_600_000).unwrap();
    let forum = Forum::create("meta", dt.clone(), mode);
    owner.put(nsid::FORUM, "meta", &serde_json::to_value(forum).unwrap());
    let membership = Membership::new(key.uri.clone(), dt.clone());
    owner.put(
        nsid::MEMBERSHIP,
        &key.fk,
        &serde_json::to_value(membership).unwrap(),
    );
    let post_value = Post::article(key.uri.clone(), "Title", "Body", dt);
    let rkey =
        keys::post_rkey(key.uri.as_str(), tid_at_ms(host.now() - 3_600_000).as_ref()).unwrap();
    let post = owner.uri(nsid::POST, &rkey);
    owner.put(
        nsid::POST,
        &rkey,
        &serde_json::to_value(post_value).unwrap(),
    );
    describe(&host, &["getChanges"], 0);
    host.respond_get(
        &endpoint("getChanges"),
        resp::ok_json(&with_meta(json!({"changes": [], "cursor": 0}), 0)),
    );
    let e = engine(&host, Sources::Assisted);
    let report = block_on(e.sync_forum(
        key.uri.as_str(),
        &SyncOpts {
            full: true,
            ..SyncOpts::default()
        },
    ))
    .unwrap();
    assert!(!report.partial, "{report:?}");
    ForumWorld {
        host,
        _net: net,
        engine: e,
        owner,
        key,
        post,
    }
}

#[test]
fn head_hints_only_queue_and_changes_restart_at_the_durable_cursor() {
    let w = forum_world();
    describe(&w.host, &["getRepoHeads", "getChanges"], 0);
    let (cid, rev) = w.owner.head();
    w.host.respond_get(
        &endpoint("getRepoHeads"),
        resp::ok_json(&with_meta(json!({"heads": [{"did": w.owner.did(), "rev": rev.to_string(), "commit": cid.to_string()}]}), 0)),
    );
    let before = block_on(RecordStore::new(w.host.cache()).repo(w.owner.did().as_str()))
        .unwrap()
        .and_then(|r| r.head_seen);
    assert_eq!(
        block_on(
            w.engine
                .indexer_head_hints(w.key.uri.as_str(), std::slice::from_ref(w.owner.did()))
        )
        .unwrap(),
        Some(1)
    );
    let after = block_on(RecordStore::new(w.host.cache()).repo(w.owner.did().as_str()))
        .unwrap()
        .and_then(|r| r.head_seen);
    assert_eq!(before, after, "an indexer head is not a PDS observation");

    w.host.respond_seq(
        &endpoint("getChanges"),
        vec![
            Ok(resp::ok_json(&with_meta(json!({
                "changes": [{"seq": 7, "uri": w.post, "cid": w.owner.cid(nsid::POST, w.post.split('/').next_back().unwrap()).unwrap().to_string(), "did": w.owner.did(), "rev": rev.to_string(), "op": "update"}],
                "cursor": 7
            }), 7))),
            Ok(resp::ok_json(&with_meta(json!({"changes": [], "cursor": 7}), 7))),
        ],
    );
    let polled = block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert_eq!(polled.cursor, 7);
    assert_eq!(polled.changes, 1);
    let source = format!("indexer:did:web:index.example.com:{}", w.key.fk);
    assert_eq!(
        block_on(RecordStore::new(w.host.cache()).cursor(&source))
            .unwrap()
            .and_then(|c| c.seq),
        Some(7)
    );
    assert!(!w.engine.syncer().queue_snapshot().is_empty());

    let restarted = engine(&w.host, Sources::Assisted);
    w.host.respond_get(
        &endpoint("getChanges"),
        resp::ok_json(&with_meta(json!({"changes": [], "cursor": 7}), 7)),
    );
    w.host.clear_requests();
    block_on(restarted.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    let calls = w.host.requests_to(&endpoint("getChanges"));
    assert_eq!(calls.len(), 1);
    assert!(calls[0].url.contains("since=7"), "{}", calls[0].url);
}

#[test]
fn malformed_changes_do_not_advance_and_old_cursors_reconcile() {
    let w = forum_world();
    describe(&w.host, &["getChanges"], 0);
    w.host.respond_get(
        &endpoint("getChanges"),
        resp::ok_json(&with_meta(
            json!({"changes": [{"seq": 1, "uri": "not a URI", "did": w.owner.did(), "rev": "3l2s5xxv2z2aa", "op": "update"}], "cursor": 1}),
            1,
        )),
    );
    assert!(block_on(w.engine.poll_indexer_changes(w.key.uri.as_str())).is_err());
    let source = format!("indexer:did:web:index.example.com:{}", w.key.fk);
    assert_eq!(
        block_on(RecordStore::new(w.host.cache()).cursor(&source))
            .unwrap()
            .and_then(|c| c.seq),
        Some(0)
    );

    describe(&w.host, &["getChanges"], 50);
    w.host.respond_seq(
        &endpoint("getChanges"),
        vec![
            Ok(resp::xrpc_error(400, "CursorTooOld", "cursor expired")),
            Ok(resp::ok_json(&with_meta(
                json!({"changes": [], "cursor": 50}),
                50,
            ))),
        ],
    );
    let recovered = block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert!(recovered.restarted);
    assert_eq!(recovered.cursor, 50);
    assert!(recovered.queued >= 1, "ordinary reconciliation is queued");
    assert_eq!(
        block_on(RecordStore::new(w.host.cache()).cursor(&source))
            .unwrap()
            .and_then(|c| c.seq),
        Some(50)
    );
}

#[test]
fn audit_reports_omissions_and_forged_proofs() {
    let w = forum_world();
    w.host.advance_ms(180_000);
    let (_, rev) = w.owner.head();
    describe(
        &w.host,
        &["getRepoHeads", "getChanges", "listPosts", "getRecordProofs"],
        0,
    );
    w.host.respond_get(
        &endpoint("getRepoHeads"),
        resp::ok_json(&with_meta(
            json!({"heads": [{"did": w.owner.did(), "rev": rev.to_string()}]}),
            0,
        )),
    );
    w.host.respond_get(
        &endpoint("listPosts"),
        resp::ok_json(&with_meta(
            json!({
                "posts": [], "policy": {"karma": "karma/1", "ranking": "magic/1"},
                "inputDigest": "00"
            }),
            0,
        )),
    );
    w.host.respond_get(
        &endpoint("getRecordProofs"),
        resp::car(b"not a car".to_vec()),
    );
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert!(report.has_contradiction_or_omission(), "{report:?}");
    assert!(
        report
            .findings()
            .any(|f| f.check == "A3 records"
                && f.kind == lessdugin_core::indexer::AuditKind::Omission)
    );
    assert!(
        report.findings().any(|f| f.check == "A5 proofs"
            && f.kind == lessdugin_core::indexer::AuditKind::Contradiction)
    );
    assert!(report.counts.contradictions >= 2);
}

/// An honest audit indexer for `w`: the owner's real head and the given `listPosts` page.
fn serve_honest_audit(w: &ForumWorld, posts: Value) {
    let (_, rev) = w.owner.head();
    describe(
        &w.host,
        &["getRepoHeads", "getChanges", "listPosts", "getRecordProofs"],
        0,
    );
    w.host.respond_get(
        &endpoint("getRepoHeads"),
        resp::ok_json(&with_meta(
            json!({"heads": [{"did": w.owner.did(), "rev": rev.to_string()}]}),
            0,
        )),
    );
    w.host.respond_get(
        &endpoint("listPosts"),
        resp::ok_json(&with_meta(
            json!({
                "posts": posts, "policy": {"karma": "karma/1", "ranking": "magic/1"},
                "inputDigest": "00"
            }),
            0,
        )),
    );
}

fn check<'a>(
    report: &'a lessdugin_core::indexer::AuditReport,
    name: &str,
) -> &'a lessdugin_core::indexer::AuditCheck {
    report.checks.iter().find(|c| c.name == name).unwrap()
}

#[test]
fn audit_does_not_count_a_hidden_post_as_an_omission() {
    use lessdugin_lexicon::records::{Action, HideAction, ModAction};
    let w = forum_world();
    // The owner hides their own post: it stays a valid record but the fold no longer lists it,
    // so an honest indexer's listPosts leaves it out (sync.md §15.4 A3).
    let hide = lessdugin_core::outbox::records::mod_action(&ModAction::new(
        w.key.uri.clone(),
        w.post.as_str(),
        Action::Hide(HideAction::hide()),
        lessdugin_atp::syntax::Datetime::from_millis(w.host.now()).unwrap(),
    ))
    .unwrap();
    w.owner.put(nsid::MOD_ACTION, &hide.rkey, &hide.value);
    block_on(w.engine.sync_forum(
        w.key.uri.as_str(),
        &SyncOpts {
            full: true,
            ..SyncOpts::default()
        },
    ))
    .unwrap();
    let mut query = lessdugin_core::dto::ListPostsQuery::new(w.key.uri.to_string());
    query.filter = lessdugin_core::dto::PostFilter::All;
    assert!(
        block_on(w.engine.list_posts(&query))
            .unwrap()
            .items
            .is_empty(),
        "the hidden post is not listed locally"
    );
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([]));
    w.host.clear_requests();
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    let a3 = check(&report, "A3 records");
    assert!(a3.findings.is_empty(), "{report:?}");
    assert_eq!(report.counts.omissions, 0, "{report:?}");
    // The A5 sample is drawn from listed posts too: nothing to prove here.
    assert!(w.host.requests_to(&endpoint("getRecordProofs")).is_empty());
}

/// A well-formed `proofs/1` bundle with no items: it omits every requested URI.
fn empty_bundle() -> Vec<u8> {
    let mut manifest = BTreeMap::new();
    manifest.insert("v".to_owned(), Ipld::Integer(1));
    manifest.insert("items".to_owned(), Ipld::List(Vec::new()));
    let manifest = dagcbor::encode(&Ipld::Map(manifest)).unwrap();
    let root = lessdugin_atp::cid::for_dag_cbor(&manifest);
    write_car(&[root], [(&root, manifest.as_slice())])
}

#[test]
fn audit_proof_check_counts_only_rejected_proofs_as_contradictions() {
    use lessdugin_core::host::HostError;
    use lessdugin_core::indexer::AuditKind;
    // An unreachable proof endpoint and a proof still over the cap at one URI present nothing
    // to verify: neither is a contradiction, and the item is not counted as checked. It is an
    // `unchecked` finding, counted as such (sync.md §15.4, engine-api.md §4.3).
    for failure in [HostError::Network("offline".into()), HostError::TooLarge] {
        let w = forum_world();
        w.host.advance_ms(180_000);
        serve_honest_audit(&w, json!([]));
        w.host.fail(&endpoint("getRecordProofs"), failure);
        let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
            .unwrap()
            .unwrap();
        let a5 = check(&report, "A5 proofs");
        assert_eq!(a5.findings.len(), 1, "{report:?}");
        assert_eq!(a5.findings[0].kind, AuditKind::Unchecked, "{report:?}");
        assert_eq!(a5.findings[0].subject, w.post);
        assert!(!a5.findings[0].detail.is_empty());
        assert_eq!(a5.counts.checked, 0, "{report:?}");
        assert_eq!(
            (
                a5.counts.unchecked,
                a5.counts.contradictions,
                a5.counts.omissions
            ),
            (1, 0, 0),
            "{report:?}"
        );
        assert_eq!(report.counts.unchecked, 1, "{report:?}");
        // the count is in the report's JSON (omitted only when 0)
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["counts"]["unchecked"], 1);
        assert_eq!(json["checks"][4]["findings"][0]["kind"], "unchecked");
        assert!(json["checks"][0]["counts"].get("unchecked").is_none());
        assert!(!w.host.requests_to(&endpoint("getRecordProofs")).is_empty());
    }

    // A bundle that leaves out the requested URI presented no proof: an omission.
    let w = forum_world();
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([]));
    w.host
        .respond_get(&endpoint("getRecordProofs"), resp::car(empty_bundle()));
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    let a5 = check(&report, "A5 proofs");
    assert_eq!(a5.findings.len(), 1, "{report:?}");
    assert_eq!(a5.findings[0].kind, AuditKind::Omission);
    assert_eq!(a5.findings[0].subject, w.post);
    assert_eq!(a5.counts.contradictions, 0);
}

fn post_hint(uri: &str) -> Value {
    json!({"ref": {"uri": uri}, "effectiveAt": "2025-01-01T00:00:00Z", "state": "visible"})
}

fn posts_page(posts: Value, cursor: Option<&str>) -> lessdugin_core::host::HttpResponse {
    resp::ok_json(&with_meta(
        json!({
            "posts": posts, "cursor": cursor,
            "policy": {"karma": "karma/1", "ranking": "magic/1"}, "inputDigest": "00"
        }),
        0,
    ))
}

#[test]
fn audit_leaves_an_unreachable_pds_unchecked_instead_of_failing() {
    use lessdugin_testkit::fake_pds::Fault;
    let w = forum_world();
    w.host.advance_ms(180_000);
    // The indexer lists the local post and one more of the same author that is not local.
    let rkey = keys::post_rkey(
        w.key.uri.as_str(),
        tid_at_ms(w.host.now() - 600_000).as_ref(),
    )
    .unwrap();
    let other = w.owner.uri(nsid::POST, &rkey);
    serve_honest_audit(&w, json!([post_hint(&w.post), post_hint(&other)]));
    // From now on the author's PDS answers nothing but 503s.
    w._net
        .inject(Fault::status(503).on_repo(w.owner.did().as_str()));
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    use lessdugin_core::indexer::AuditKind;
    let unchecked = |c: &lessdugin_core::indexer::AuditCheck| -> Vec<String> {
        assert!(
            c.findings.iter().all(|f| f.kind == AuditKind::Unchecked),
            "{c:?}"
        );
        assert_eq!(c.counts.unchecked, c.findings.len(), "{c:?}");
        c.findings.iter().map(|f| f.subject.clone()).collect()
    };
    let a1 = check(&report, "A1 heads");
    assert_eq!(a1.counts.checked, 0, "{report:?}");
    // The head check failed: the repo is unchecked, a finding that says so (sync.md §15.4).
    assert_eq!(unchecked(a1), [w.owner.did().to_string()], "{report:?}");
    let a3 = check(&report, "A3 records");
    // The local post was compared; the other one could not be fetched and stays unchecked.
    assert_eq!(a3.counts.checked, 2, "{report:?}");
    assert_eq!(unchecked(a3), std::slice::from_ref(&other), "{report:?}");
    // The proof of the sampled post could be had from neither the indexer nor the PDS.
    assert_eq!(
        unchecked(check(&report, "A5 proofs")),
        std::slice::from_ref(&w.post),
        "{report:?}"
    );
    assert_eq!(report.counts.omissions, 0, "{report:?}");
    assert_eq!(report.counts.unchecked, 3, "{report:?}");
}

#[test]
fn audit_pages_listings_past_short_pages_until_the_cursor_ends() {
    let w = forum_world();
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([]));
    // Page 1 is short (every row was blocklisted after paging) but carries a cursor; the post
    // is on page 2. The last answer serves the digest read.
    w.host.respond_seq(
        &endpoint("listPosts"),
        vec![
            Ok(posts_page(json!([]), Some("c1"))),
            Ok(posts_page(json!([post_hint(&w.post)]), Some("c2"))),
            Ok(posts_page(json!([]), None)),
            Ok(posts_page(json!([]), None)),
        ],
    );
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    let a3 = check(&report, "A3 records");
    assert!(a3.findings.is_empty(), "{report:?}");
    let pages = w.host.requests_to(&endpoint("listPosts"));
    assert!(pages[1].url.contains("cursor=c1"), "{}", pages[1].url);
    assert!(pages[2].url.contains("cursor=c2"), "{}", pages[2].url);

    // A repeated cursor ends the listing instead of failing the audit.
    let w = forum_world();
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([]));
    w.host.respond_seq(
        &endpoint("listPosts"),
        vec![
            Ok(posts_page(json!([post_hint(&w.post)]), Some("c1"))),
            Ok(posts_page(json!([]), Some("c1"))),
            Ok(posts_page(json!([]), None)),
        ],
    );
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert!(
        check(&report, "A3 records").findings.is_empty(),
        "{report:?}"
    );
}

/// A forum change log at an indexer: rows `floor < seq <= head`, generated on demand. `rows`
/// are placed at their seq, `blocked` seqs are dropped after paging (the cursor still covers
/// them), every other seq is an unrelated filler change. `since < floor` is `CursorTooOld`.
fn serve_change_log(
    host: &MemHost,
    floor: i64,
    head: i64,
    rows: BTreeMap<i64, Value>,
    blocked: std::ops::RangeInclusive<i64>,
) {
    describe(
        host,
        &["getRepoHeads", "getChanges", "listPosts", "getRecordProofs"],
        head,
    );
    host.on_get(&endpoint("getChanges"), move |req| {
        let url = match &req.target {
            lessdugin_core::host::Target::Anon(url) => url.clone(),
            _ => panic!("getChanges carried credentials"),
        };
        let q = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.parse::<i64>().unwrap())
                .unwrap()
        };
        let (since, limit) = (q("since"), q("limit"));
        if since < floor {
            return Ok(resp::xrpc_error(400, "CursorTooOld", "cursor expired"));
        }
        let last = head.min(since + limit);
        let changes: Vec<Value> = (since + 1..=last)
            .filter(|seq| !blocked.contains(seq))
            .map(|seq| {
                let mut row = rows.get(&seq).cloned().unwrap_or_else(|| {
                    json!({
                        "uri": format!("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/{POST}/{seq}"),
                        "did": "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
                        "rev": "3l2s5xxv2zaa2", "op": "create", "cid": null
                    })
                });
                row["seq"] = json!(seq);
                row
            })
            .collect();
        Ok(resp::ok_json(&with_meta(
            json!({"changes": changes, "cursor": last.max(since)}),
            head,
        )))
    });
}

fn observe(w: &ForumWorld, rev: &str, cid: Option<String>) {
    w.engine
        .syncer()
        .note_observed_change(lessdugin_core::sync::ObservedChange {
            fk: w.key.fk.clone(),
            uri: w.post.clone(),
            did: w.owner.did().to_string(),
            rev: rev.to_owned(),
            cid,
            observed_us: w.host.now() * 1000,
        });
}

fn change_row(w: &ForumWorld, rev: &str, cid: Option<&str>, op: &str) -> Value {
    json!({"uri": w.post, "did": w.owner.did(), "rev": rev, "cid": cid, "op": op})
}

fn a2_after(w: &ForumWorld) -> lessdugin_core::indexer::AuditCheck {
    w.host.clear_requests();
    let report = block_on(w.engine.audit_indexer(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    check(&report, "A2 changes").clone()
}

#[test]
fn audit_change_check_accepts_a_row_at_or_after_the_observed_change() {
    let post_rkey = |w: &ForumWorld| w.post.rsplit('/').next().unwrap().to_owned();
    let observed = tid_at_ms(1_700_000_000_000).to_string();
    let later = tid_at_ms(1_700_000_060_000).to_string();
    let other_cid = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm";
    // (rev, CID: the observed one, another or none, op, expected omissions): a snapshot row at
    // the repo's later head rev with the observed CID, a later edit, a later delete and the exact
    // commit all match; the observed rev with another CID, or no row at all, does not.
    let cases = [
        (Some(&later), Some(true), "create", 0),
        (Some(&later), Some(false), "update", 0),
        (Some(&later), None, "delete", 0),
        (Some(&observed), Some(true), "create", 0),
        (Some(&observed), Some(false), "create", 1),
        (None, None, "create", 1),
    ];
    for (i, (rev, same_cid, op, omissions)) in cases.into_iter().enumerate() {
        let w = forum_world();
        let cid = w.owner.cid(nsid::POST, &post_rkey(&w)).unwrap().to_string();
        observe(&w, &observed, Some(cid.clone()));
        w.host.advance_ms(180_000);
        serve_honest_audit(&w, json!([post_hint(&w.post)]));
        let row_cid = same_cid.map(|same| if same { cid.as_str() } else { other_cid });
        let rows = rev
            .map(|rev| (40, change_row(&w, rev, row_cid, op)))
            .into_iter()
            .collect();
        serve_change_log(&w.host, 0, 100, rows, 0..=0);
        let a2 = a2_after(&w);
        assert_eq!(a2.counts.checked, 1, "case {i}: {a2:?}");
        assert_eq!(a2.counts.omissions, omissions, "case {i}: {a2:?}");
    }

    // An observed delete matches a delete at its rev and any row at a later rev.
    for (rows, omissions) in [
        (
            vec![(5, json!({"rev": observed, "cid": null, "op": "delete"}))],
            0,
        ),
        (
            vec![(5, json!({"rev": later, "cid": other_cid, "op": "create"}))],
            0,
        ),
        (
            vec![(
                5,
                json!({"rev": observed, "cid": other_cid, "op": "create"}),
            )],
            1,
        ),
    ] {
        let w = forum_world();
        observe(&w, &observed, None);
        w.host.advance_ms(180_000);
        serve_honest_audit(&w, json!([post_hint(&w.post)]));
        let rows = rows
            .into_iter()
            .map(|(seq, mut row): (i64, Value)| {
                row["uri"] = json!(w.post);
                row["did"] = json!(w.owner.did());
                (seq, row)
            })
            .collect();
        serve_change_log(&w.host, 0, 100, rows, 0..=0);
        assert_eq!(a2_after(&w).counts.omissions, omissions);
    }
}

#[test]
fn audit_change_check_reads_back_from_the_head_past_a_pruned_log() {
    let observed = tid_at_ms(1_700_000_000_000).to_string();
    // The log was pruned below seq 12,000 and its head is 20,000. The session's change is at
    // 13,000, below the first window; reading from 0 is refused as too old.
    let w = forum_world();
    let cid = w
        .owner
        .cid(nsid::POST, w.post.rsplit('/').next().unwrap())
        .unwrap()
        .to_string();
    observe(&w, &observed, Some(cid.clone()));
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([post_hint(&w.post)]));
    let rows = BTreeMap::from([(13_000, change_row(&w, &observed, Some(&cid), "create"))]);
    serve_change_log(&w.host, 12_000, 20_000, rows, 0..=0);
    let a2 = a2_after(&w);
    assert_eq!((a2.counts.checked, a2.counts.omissions), (1, 0), "{a2:?}");
    // It started near the head, not at the oldest rows, and never asked for the refused range
    // twice.
    let calls = w.host.requests_to(&endpoint("getChanges"));
    assert!(calls[0].url.contains("since=15000"), "{}", calls[0].url);
    assert!(calls.iter().all(|c| !c.url.contains("since=0&")));

    // Pruned past the change: once the scan reaches the floor it is an omission.
    let w = forum_world();
    observe(&w, &observed, Some(cid.clone()));
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([post_hint(&w.post)]));
    serve_change_log(&w.host, 12_000, 20_000, BTreeMap::new(), 0..=0);
    let a2 = a2_after(&w);
    assert_eq!((a2.counts.checked, a2.counts.omissions), (1, 1), "{a2:?}");

    // An empty page whose cursor still advances (every row blocklisted) is not the end.
    let w = forum_world();
    observe(&w, &observed, Some(cid.clone()));
    w.host.advance_ms(180_000);
    serve_honest_audit(&w, json!([post_hint(&w.post)]));
    let rows = BTreeMap::from([(1_500, change_row(&w, &observed, Some(&cid), "create"))]);
    serve_change_log(&w.host, 0, 2_500, rows, 1..=1_000);
    let a2 = a2_after(&w);
    assert_eq!((a2.counts.checked, a2.counts.omissions), (1, 0), "{a2:?}");
}

/// A syntactically valid DID that no test account has.
fn stranger(i: usize) -> String {
    let tail: String = [b'a' + (i / 26) as u8, b'a' + (i % 26) as u8]
        .iter()
        .map(|b| *b as char)
        .collect();
    format!("did:plc:strangerxxxxxxxxxxxxxx{tail}")
}

const SOME_CID: &str = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm";

fn feed_change(seq: i64, did: &str, coll: &str, rkey: &str) -> Value {
    json!({
        "seq": seq, "uri": format!("at://{did}/{coll}/{rkey}"), "cid": SOME_CID, "did": did,
        "rev": tid_at_ms(1_700_000_000_000).to_string(), "op": "create"
    })
}

fn drain_queue(e: &Engine<MemHost>) {
    while e.syncer().pop_ready(i64::MAX).is_some() {}
}

#[test]
fn a_hostile_change_feed_cannot_steer_fetches_at_arbitrary_dids() {
    let w = forum_world();
    let other_fk = keys::fk("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.forum/other").unwrap();
    let mut changes = Vec::new();
    for i in 0..10 {
        // Strangers' records in this forum's range, in another forum's range, and memberships
        // at this forum's key (an approval forum the viewer does not moderate takes none).
        let seq = 3 * i as i64;
        changes.push(feed_change(
            seq + 1,
            &stranger(i),
            nsid::POST,
            &format!("{}-3l2s5xxv2z{i:03}", w.key.fk),
        ));
        changes.push(feed_change(
            seq + 2,
            &stranger(i + 10),
            nsid::POST,
            &format!("{other_fk}-3l2s5xxv2z{i:03}"),
        ));
        changes.push(feed_change(
            seq + 3,
            &stranger(i + 20),
            nsid::MEMBERSHIP,
            &w.key.fk,
        ));
    }
    // The owner's own change in the forum is in scope.
    let rkey = w.post.rsplit('/').next().unwrap();
    changes.push(feed_change(31, w.owner.did().as_str(), nsid::POST, rkey));
    describe(&w.host, &["getChanges"], 31);
    w.host.respond_get(
        &endpoint("getChanges"),
        resp::ok_json(&with_meta(json!({"changes": changes, "cursor": 31}), 31)),
    );
    drain_queue(&w.engine);
    w.host.clear_requests();
    let polled = block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert_eq!((polled.changes, polled.dropped), (1, 30), "{polled:?}");
    assert!(polled.candidates.is_empty(), "{polled:?}");
    assert_eq!(polled.cursor, 31);
    let queued: Vec<String> = w
        .engine
        .syncer()
        .queue_snapshot()
        .ordered()
        .iter()
        .map(|i| i.did.clone())
        .collect();
    assert_eq!(queued, vec![w.owner.did().to_string()]);
    for r in w.host.requests() {
        assert!(
            !r.url.contains("strangerxx"),
            "contacted on a hint: {}",
            r.url
        );
    }
    // Nor does the next round of the dirty queue (or a sync) reach any of them.
    block_on(w.engine.live_step()).unwrap();
    for r in w.host.requests() {
        assert!(
            !r.url.contains("strangerxx"),
            "contacted on a hint: {}",
            r.url
        );
    }
}

#[test]
fn a_change_feed_newcomer_goes_through_discovery_confirmation() {
    let w = forum_world_in(MembershipMode::Open);
    let newbie = w._net.add_account("newbie.example.com");
    let dt = lessdugin_atp::syntax::Datetime::from_millis(w.host.now()).unwrap();
    newbie.put(
        nsid::MEMBERSHIP,
        &w.key.fk,
        &serde_json::to_value(Membership::new(w.key.uri.clone(), dt)).unwrap(),
    );
    describe(&w.host, &["getChanges"], 2);
    w.host.respond_get(
        &endpoint("getChanges"),
        resp::ok_json(&with_meta(
            json!({"changes": [
                feed_change(1, newbie.did().as_str(), nsid::MEMBERSHIP, &w.key.fk),
                feed_change(2, newbie.did().as_str(), nsid::POST, &format!("{}-3l2s5xxv2zaa2", w.key.fk)),
            ], "cursor": 2}),
            2,
        )),
    );
    drain_queue(&w.engine);
    w.host.clear_requests();
    let polled = block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    // The membership is a candidate and the post is not fetched: nothing is queued or sent.
    assert_eq!(
        polled.candidates,
        vec![newbie.did().to_string()],
        "{polled:?}"
    );
    assert_eq!((polled.changes, polled.dropped), (0, 1), "{polled:?}");
    assert!(w.engine.syncer().queue_snapshot().ordered().is_empty());
    assert!(
        w.host
            .requests()
            .iter()
            .all(|r| !r.url.contains(newbie.did().as_str()))
    );

    // A sync confirms it like any newcomer hint, within the discovery budget.
    let source = format!("indexer:did:web:index.example.com:{}", w.key.fk);
    let mut rewind = lessdugin_core::store::blob::CursorRow::new(w.host.now());
    rewind.seq = Some(0);
    let mut batch = RecordStore::new(w.host.cache()).batch();
    batch.put_cursor(&source, &rewind).unwrap();
    block_on(batch.commit()).unwrap();
    block_on(
        w.engine
            .sync_forum(w.key.uri.as_str(), &SyncOpts::default()),
    )
    .unwrap();
    let disc = block_on(lessdugin_core::discovery::state::disc_rows(
        w.host.cache(),
        &w.key.fk,
    ))
    .unwrap();
    let row = disc.get(newbie.did().as_str()).expect("a discovery row");
    assert!(row.src.iter().any(|s| s == "hint:indexer"), "{row:?}");
    assert_eq!(
        row.state,
        lessdugin_core::discovery::DiscState::Confirmed,
        "{row:?}"
    );
}

#[test]
fn indexer_page_and_head_hints_stay_inside_the_forum() {
    let w = forum_world();
    let rkey = w.post.rsplit('/').next().unwrap().to_owned();
    let outsider = format!(
        "at://{}/{}/{}-3l2s5xxv2zaa2",
        stranger(1),
        nsid::POST,
        w.key.fk
    );
    describe(
        &w.host,
        &["listPosts", "getRecordProofs", "getRepoHeads"],
        0,
    );
    w.host.respond_get(
        &endpoint("listPosts"),
        posts_page(json!([post_hint(&outsider), post_hint(&w.post)]), None),
    );
    w.host
        .respond_get(&endpoint("getRecordProofs"), resp::car(empty_bundle()));
    w.host.clear_requests();
    let mut query = lessdugin_core::dto::ListPostsQuery::new(w.key.uri.to_string());
    query.filter = lessdugin_core::dto::PostFilter::All;
    let report = block_on(w.engine.hydrate_indexer_page(&query))
        .unwrap()
        .unwrap();
    // The forum's participants are known: a stranger's post, even in the forum's range, is not
    // requested, head-checked or fetched.
    assert_eq!(report.requested, 1, "{report:?}");
    let proofs = w.host.requests_to(&endpoint("getRecordProofs"));
    assert!(proofs[0].url.contains(&rkey), "{}", proofs[0].url);
    for r in w.host.requests() {
        assert!(
            !r.url.contains("strangerxx"),
            "contacted on a hint: {}",
            r.url
        );
    }

    // A head for a DID that was not asked about queues nothing.
    let (cid, rev) = w.owner.head();
    w.host.respond_get(
        &endpoint("getRepoHeads"),
        resp::ok_json(&with_meta(
            json!({"heads": [
                {"did": w.owner.did(), "rev": rev.to_string(), "commit": cid.to_string()},
                {"did": stranger(2), "rev": rev.to_string(), "commit": cid.to_string()}
            ]}),
            0,
        )),
    );
    drain_queue(&w.engine);
    assert_eq!(
        block_on(
            w.engine
                .indexer_head_hints(w.key.uri.as_str(), std::slice::from_ref(w.owner.did()))
        )
        .unwrap(),
        Some(1)
    );
    let queued = w.engine.syncer().queue_snapshot();
    assert!(queued.ordered().iter().all(|i| i.did != stranger(2)));
}

#[test]
fn an_expired_change_cursor_reconciles_at_reconcile_priority_at_most_daily() {
    use lessdugin_lexicon::records::{AccessAction, Action, ModAction};
    use lessdugin_lexicon::values::AccessLevel;
    let w = forum_world();
    // A granted member: a participant, reconciled at P3.
    let member = w._net.add_account("member.example.com");
    let dt = lessdugin_atp::syntax::Datetime::from_millis(w.host.now()).unwrap();
    member.put(
        nsid::MEMBERSHIP,
        &w.key.fk,
        &serde_json::to_value(Membership::new(w.key.uri.clone(), dt.clone())).unwrap(),
    );
    let grant = lessdugin_core::outbox::records::mod_action(&ModAction::new(
        w.key.uri.clone(),
        member.did().as_str(),
        Action::Access(AccessAction::new(AccessLevel::Member)),
        dt,
    ))
    .unwrap();
    w.owner.put(nsid::MOD_ACTION, &grant.rkey, &grant.value);
    block_on(w.engine.sync_forum(
        w.key.uri.as_str(),
        &SyncOpts {
            full: true,
            ..SyncOpts::default()
        },
    ))
    .unwrap();
    let source = format!("indexer:did:web:index.example.com:{}", w.key.fk);
    let expire = |w: &ForumWorld| {
        describe(&w.host, &["getChanges"], 50);
        w.host.respond_seq(
            &endpoint("getChanges"),
            vec![
                Ok(resp::xrpc_error(400, "CursorTooOld", "cursor expired")),
                Ok(resp::ok_json(&with_meta(
                    json!({"changes": [], "cursor": 50}),
                    50,
                ))),
            ],
        );
        let rewind = lessdugin_core::store::blob::CursorRow {
            seq: Some(7),
            ..block_on(RecordStore::new(w.host.cache()).cursor(&source))
                .unwrap()
                .unwrap()
        };
        let mut batch = RecordStore::new(w.host.cache()).batch();
        batch.put_cursor(&source, &rewind).unwrap();
        block_on(batch.commit()).unwrap();
        drain_queue(&w.engine);
        block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
            .unwrap()
            .unwrap()
    };
    let first = expire(&w);
    assert!(first.restarted);
    assert_eq!(first.queued, 2, "{first:?}");
    let queued = w.engine.syncer().queue_snapshot();
    let items: Vec<(&str, Priority)> = queued
        .ordered()
        .iter()
        .map(|i| (i.did.as_str(), i.priority))
        .collect();
    // As for a Jetstream gap (sync.md §6.3): the authority at P1, the participant at P3; not P0.
    assert_eq!(
        items,
        vec![
            (w.owner.did().as_str(), Priority::P1),
            (member.did().as_str(), Priority::P3)
        ]
    );

    // An indexer that keeps answering CursorTooOld does not get a reconcile every poll.
    w.host.advance_ms(3_600_000);
    let again = expire(&w);
    assert!(again.restarted);
    assert_eq!(again.queued, 0, "{again:?}");
    assert!(w.engine.syncer().queue_snapshot().ordered().is_empty());
    w.host.advance_ms(86_400_000);
    assert_eq!(expire(&w).queued, 2);
}

#[test]
fn a_change_cursor_past_blocklisted_rows_is_followed() {
    let w = forum_world();
    let rkey = w.post.rsplit('/').next().unwrap();
    describe(&w.host, &["getChanges"], 9);
    // Rows 6 and 7 were blocklisted after paging, and so were 8 and 9 on the next page.
    w.host.respond_seq(
        &endpoint("getChanges"),
        vec![
            Ok(resp::ok_json(&with_meta(
                json!({"changes": [feed_change(5, w.owner.did().as_str(), nsid::POST, rkey)], "cursor": 7}),
                9,
            ))),
            Ok(resp::ok_json(&with_meta(json!({"changes": [], "cursor": 9}), 9))),
            Ok(resp::ok_json(&with_meta(json!({"changes": [], "cursor": 9}), 9))),
        ],
    );
    let polled = block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert_eq!((polled.changes, polled.cursor), (1, 7), "{polled:?}");
    let polled = block_on(w.engine.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    assert_eq!(polled.cursor, 9, "{polled:?}");

    // A cursor below a returned row, or past the indexer's own snapshot, is still refused.
    for (cursor, as_of) in [(10, 12), (40, 12)] {
        w.host.respond_get(
            &endpoint("getChanges"),
            resp::ok_json(&with_meta(
                json!({"changes": [feed_change(11, w.owner.did().as_str(), nsid::POST, rkey)], "cursor": cursor}),
                as_of,
            )),
        );
        assert!(block_on(w.engine.poll_indexer_changes(w.key.uri.as_str())).is_err());
    }
}

#[test]
fn production_endpoints_are_exact_and_devnet_is_loopback_only() {
    use lessdugin_core::indexer::endpoint_allowed;
    let ok = |u: &str, p| endpoint_allowed(&url::Url::parse(u).unwrap(), p);
    assert!(ok(INDEX, EndpointPolicy::Production));
    assert!(!ok(
        "https://index.example.com:444",
        EndpointPolicy::Production
    ));
    assert!(!ok("http://index.example.com", EndpointPolicy::Production));
    assert!(!ok("http://192.0.2.1:8080", EndpointPolicy::Devnet));
    assert!(ok("http://127.0.0.1:8080", EndpointPolicy::Devnet));
    assert!(ok("http://localhost:8080", EndpointPolicy::Devnet));
    assert!(!ok(
        "https://user@index.example.com",
        EndpointPolicy::Production
    ));
}
