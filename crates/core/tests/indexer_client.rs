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
use lessdugin_core::engine::{Engine, EngineConfig, IndexerConsent, Sources};
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
const DESCRIBE: &str = "online.7778777.lessdugin.index.describeIndexer";
const HEADS: &str = "online.7778777.lessdugin.index.getRepoHeads";
const CHANGES: &str = "online.7778777.lessdugin.index.getChanges";
const POSTS: &str = "online.7778777.lessdugin.index.listPosts";
const PROOFS: &str = "online.7778777.lessdugin.index.getRecordProofs";

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
        &format!("{INDEX}/xrpc/{DESCRIBE}"),
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
    let commit = if forged {
        repo.commit_signed_by(rev, &fake_key("99"))
    } else {
        repo.commit(rev)
    };
    let blocks = repo.proof_blocks(POST, RKEY, &commit);
    let mut item = BTreeMap::new();
    item.insert(
        "uri".to_owned(),
        Ipld::String(format!("at://{}/{POST}/{RKEY}", repo.did())),
    );
    item.insert(
        "cid".to_owned(),
        repo.cid(POST, RKEY).map_or(Ipld::Null, Ipld::Link),
    );
    item.insert("commit".to_owned(), Ipld::Link(commit.cid));
    let mut manifest = BTreeMap::new();
    manifest.insert("v".to_owned(), Ipld::Integer(1));
    manifest.insert("items".to_owned(), Ipld::List(vec![Ipld::Map(item)]));
    let manifest = dagcbor::encode(&Ipld::Map(manifest)).unwrap();
    let root = lessdugin_atp::cid::for_dag_cbor(&manifest);
    let mut all = vec![(root, manifest)];
    all.extend(blocks);
    write_car(&[root], all.iter().map(|(c, b)| (c, b.as_slice())))
}

fn current_proof_world(sources: Sources, forged: bool) -> (World, Engine<MemHost>, AtUri) {
    let w = World::new();
    let rev = tid_at_ms(w.host.now() - 60_000);
    let car = w
        .pds
        .update(&w.alice.did, None, |repo| bundle(repo, rev, forged));
    describe(&w.host, &[PROOFS], 0);
    w.host
        .respond_get(&format!("{INDEX}/xrpc/{PROOFS}"), resp::car(car));
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
    assert!(w.host.requests_to(INDEX).is_empty());
    assert!(
        !block_on(RecordStore::new(w.host.cache()).repo(w.alice.did.as_str()))
            .unwrap()
            .is_some_and(|r| r.head_seen.is_some())
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
    let requests = w.host.requests_to(&format!("{INDEX}/xrpc/{PROOFS}"));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].max_bytes, 8 * 1024 * 1024);
    assert_eq!(
        requests[0].header("accept"),
        Some("application/vnd.ipld.car")
    );
}

#[test]
fn stale_and_forged_proofs_never_remain_proven() {
    for forged in [false, true] {
        let (w, e, uri) = current_proof_world(Sources::Assisted, forged);
        if !forged {
            // The CAR was built at the old head. Advance only the PDS head to make it stale.
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
            if forged {
                ProofStatus::Failed
            } else {
                ProofStatus::StaleProof
            }
        );
        assert_eq!(report.point_fetched, 1);
        let row = block_on(RecordStore::new(w.host.cache()).get(&uri))
            .unwrap()
            .unwrap();
        assert_eq!(row.level, Level::Pds);
        assert_eq!(row.src, Source::Get);
        let origin = url::Url::parse(INDEX)
            .unwrap()
            .origin()
            .ascii_serialization();
        let strikes = block_on(RecordStore::new(w.host.cache()).host_caps(&origin))
            .unwrap()
            .unwrap_or_default()
            .strikes;
        assert_eq!(strikes, u32::from(forged));
    }
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
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("alice.example.com");
    let key = ForumKey::new(owner.did(), "meta").unwrap();
    let dt = lessdugin_atp::syntax::Datetime::from_millis(host.now() - 3_600_000).unwrap();
    let forum = Forum::create("meta", dt.clone(), MembershipMode::Approval);
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
    describe(&host, &[CHANGES], 0);
    host.respond_get(
        &format!("{INDEX}/xrpc/{CHANGES}"),
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
    describe(&w.host, &[HEADS, CHANGES], 0);
    let (cid, rev) = w.owner.head();
    w.host.respond_get(
        &format!("{INDEX}/xrpc/{HEADS}"),
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
        &format!("{INDEX}/xrpc/{CHANGES}"),
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
        &format!("{INDEX}/xrpc/{CHANGES}"),
        resp::ok_json(&with_meta(json!({"changes": [], "cursor": 7}), 7)),
    );
    w.host.clear_requests();
    block_on(restarted.poll_indexer_changes(w.key.uri.as_str()))
        .unwrap()
        .unwrap();
    let calls = w.host.requests_to(&format!("{INDEX}/xrpc/{CHANGES}"));
    assert_eq!(calls.len(), 1);
    assert!(calls[0].url.contains("since=7"), "{}", calls[0].url);
}

#[test]
fn malformed_changes_do_not_advance_and_old_cursors_reconcile() {
    let w = forum_world();
    describe(&w.host, &[CHANGES], 0);
    w.host.respond_get(
        &format!("{INDEX}/xrpc/{CHANGES}"),
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

    describe(&w.host, &[CHANGES], 50);
    w.host.respond_seq(
        &format!("{INDEX}/xrpc/{CHANGES}"),
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
    describe(&w.host, &[HEADS, CHANGES, POSTS, PROOFS], 0);
    w.host.respond_get(
        &format!("{INDEX}/xrpc/{HEADS}"),
        resp::ok_json(&with_meta(
            json!({"heads": [{"did": w.owner.did(), "rev": rev.to_string()}]}),
            0,
        )),
    );
    w.host.respond_get(
        &format!("{INDEX}/xrpc/{POSTS}"),
        resp::ok_json(&with_meta(
            json!({
                "posts": [], "policy": {"karma": "karma/1", "ranking": "magic/1"},
                "inputDigest": "00"
            }),
            0,
        )),
    );
    w.host.respond_get(
        &format!("{INDEX}/xrpc/{PROOFS}"),
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
