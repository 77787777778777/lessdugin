// SPDX-License-Identifier: AGPL-3.0-or-later
//! M6 cold-start fixture: real signed FakeNet repositories and a native indexer HTTP server.
//!
//! The process seeds 2,000 accounts (including the owner), pre-indexes every repository, writes
//! the loopback endpoints to JSON, and stays alive for the focused Playwright acceptance test.
//! Besides the owner's post, [`MEMBER_POSTS`] distinct members each post once, so the first page
//! the browser paints holds posts by distinct participants: one proof per post, and one head
//! check and one DID document per author on screen (`sync.md` §14.3).
//! It never configures or contacts a production service.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use lessdugin_atp::proof::{ProofBundle, verify_proof};
use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_atp::xrpc::{GetLatestCommit, SyncGetRecord};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::xrpc::{CallOpts, Service};
use lessdugin_indexer::host::PoliteHost;
use lessdugin_indexer::store::Store;
use lessdugin_indexer::{Indexer, IndexerConfig, api, ingest};
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::nsid;
use lessdugin_lexicon::records::{AccessAction, Action, Forum, Membership, ModAction, Post};
use lessdugin_lexicon::values::{AccessLevel, MembershipMode};
use lessdugin_native::config::{Config, DevnetProfile, NetworkProfile, Paths};
use lessdugin_native::host::NativeHost;
use lessdugin_testkit::FakeNet;
use lessdugin_testkit::fake_pds::SystemClock;
use lessdugin_testkit::repo::tid_at_ms;
use serde_json::json;
use url::Url;

const TITLE: &str = "M6 cold start: proof-backed title from 2,000 signed participants";
/// Members with one post each (more than a first page: the SPA lists 20, the CLI 30).
const MEMBER_POSTS: usize = 30;

fn arg<T: std::str::FromStr>(name: &str, default: T) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let args = std::env::args().collect::<Vec<_>>();
    let Some(i) = args.iter().position(|a| a == name) else {
        return Ok(default);
    };
    let value = args.get(i + 1).context("missing value for {name}")?;
    Ok(value.parse()?)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    let output = arg("--json", PathBuf::from("cold-start-devnet.json"))?;
    let participants = arg("--participants", 2_000usize)?;
    let pds_count = arg("--pds-count", 40usize)?;
    if participants <= MEMBER_POSTS || pds_count == 0 {
        bail!("need more than {MEMBER_POSTS} participants and one PDS");
    }

    // Wall-clock elapsed time is benchmark evidence only, never input to repository state.
    #[allow(clippy::disallowed_methods)]
    let seed_started = Instant::now();
    let net = FakeNet::serve_http(SystemClock);
    let pds = (0..pds_count)
        .map(|i| net.add_pds(&format!("cold-{i}.test")))
        .collect::<Vec<_>>();
    let owner = pds[0].add_account("owner.test");
    let forum = ForumKey::new(owner.did(), "cold-start")?;
    let now = Datetime::from_millis(net.now_ms() - 3_600_000)?;
    let declaration = Forum::create("M6 cold start", now.clone(), MembershipMode::Approval);
    let membership = Membership::new(forum.uri.clone(), now.clone());

    let accounts = (1..participants)
        .map(|i| pds[i % pds_count].add_account(&format!("member-{i:04}.test")))
        .collect::<Vec<_>>();
    for account in &accounts {
        let membership = Membership::new(forum.uri.clone(), now.clone());
        account.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(membership)?,
        );
    }
    // one post by each of the first MEMBER_POSTS members, a minute apart and all older than the
    // owner's post (dated `now`), so `new` order is fixed: the owner's, then member 1's, …
    let mut member_posts = Vec::new();
    for (i, account) in accounts.iter().take(MEMBER_POSTS).enumerate() {
        let at_ms = now.timestamp_micros() / 1000 - 60_000 * (i64::try_from(i)? + 1);
        let rkey = keys::post_rkey(forum.uri.as_str(), tid_at_ms(at_ms).as_ref())?;
        let title = format!("M6 cold start post {} of {MEMBER_POSTS}", i + 1);
        let post = Post::article(
            forum.uri.clone(),
            title.clone(),
            "A post by a distinct signed participant.",
            Datetime::from_millis(at_ms)?,
        );
        account.put(nsid::POST, &rkey, &serde_json::to_value(post)?);
        member_posts.push(json!({
            "uri": account.uri(nsid::POST, &rkey),
            "title": title,
            "author": account.did(),
        }));
    }

    let post_rkey = keys::post_rkey(forum.uri.as_str(), tid_at_ms(net.now_ms()).as_ref())?;
    let post_uri = owner.uri(nsid::POST, &post_rkey);
    owner.batch(|repo| {
        repo.put(
            nsid::FORUM,
            "cold-start",
            &serde_json::to_value(declaration).unwrap(),
        );
        repo.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(membership).unwrap(),
        );
        for account in &accounts {
            let action = Action::Access(AccessAction::new(AccessLevel::Member));
            let grant = ModAction::new(
                forum.uri.clone(),
                account.did().as_str(),
                action,
                now.clone(),
            );
            let value = serde_json::to_value(grant).unwrap();
            let rkey = keys::mod_action_rkey(
                NS,
                forum.uri.as_str(),
                None,
                account.did().as_str(),
                value["action"].as_object().unwrap(),
            )
            .unwrap();
            repo.put(nsid::MOD_ACTION, &rkey, &value);
        }
        let post = Post::article(
            forum.uri.clone(),
            TITLE,
            "This body is admitted from a real signed repository proof.",
            now.clone(),
        );
        repo.put(nsid::POST, &post_rkey, &serde_json::to_value(post).unwrap());
    });
    // Check the visible record with the same lexicon validator used by clients.
    let post_value = owner.get(nsid::POST, &post_rkey).context("seeded post")?;
    let validity = lessdugin_lexicon::validity::check_uri(&AtUri::parse(&post_uri)?, &post_value);
    assert!(validity.first_rule().is_none(), "{validity:?}");
    assert_eq!(owner.list(nsid::MOD_ACTION).len(), participants - 1);
    for account in &accounts {
        let membership_uri = account.uri(nsid::MEMBERSHIP, &forum.fk);
        let value = account
            .get(nsid::MEMBERSHIP, &forum.fk)
            .with_context(|| format!("membership for {}", account.did()))?;
        let validity =
            lessdugin_lexicon::validity::check_uri(&AtUri::parse(&membership_uri)?, &value);
        assert!(
            validity.first_rule().is_none(),
            "invalid membership for {}: {validity:?}",
            account.did()
        );
    }
    // Account writes are lazily committed by FakeNet; make every repo head signed before the
    // native indexer reads it, rather than relying on whichever PDS call happens to commit first.
    net.flush();
    let seed_ms = seed_started.elapsed().as_millis();

    let dir = tempfile::tempdir()?;
    let paths = Paths::under(dir.path());
    let config = Config {
        network: NetworkProfile::Devnet(DevnetProfile {
            plc: Url::parse(&net.plc())?,
            pds: owner.pds().url(),
            jetstream: None,
        }),
        // Native devnet policy permits plain HTTP only for explicitly configured origins. These
        // are not discovery relays (the engine stays direct); the field also allowlists every
        // participant's actual loopback PDS for this fixture's pre-index pass.
        relays: pds.iter().map(|p| p.url()).collect(),
        ..Config::default()
    };
    let host = NativeHost::open(&paths, &config)?;
    let engine = lessdugin_core::engine::Engine::new(
        PoliteHost::new(host),
        lessdugin_native::engine::engine_config(&config),
    );
    let store = Store::open(&paths.state_db())?;
    store.add_forum_seed(forum.uri.as_str())?;
    let indexer = Indexer::new(
        engine.clone(),
        store,
        IndexerConfig {
            did: "did:web:indexer.test".into(),
            source_url: "https://source.test/lessdugin".into(),
            plc: Url::parse(&net.plc())?,
            relays: vec![],
            jetstream: vec![],
            poll: Some(owner.pds().url()),
            poll_interval_ms: 250,
            blocklist: std::sync::Arc::new(BTreeSet::new()),
        },
    );

    #[allow(clippy::disallowed_methods)]
    let index_started = Instant::now();
    // The service owns its real, unbudgeted pre-index pass. Do not run a second client-side
    // full sync here: all 2,000 repos must be indexed, but setup time is not the timed visit.
    ingest::sync_forum(&indexer, forum.uri.as_str()).await?;
    let preindex_ms = index_started.elapsed().as_millis();
    let indexed = indexer
        .store
        .forum(forum.uri.as_str())
        .context("indexed forum")?;
    if indexed.participants != participants || indexed.posts != 1 + MEMBER_POSTS {
        bail!(
            "pre-index incomplete: {} participants (expected {participants}), {} posts",
            indexed.participants,
            indexed.posts
        );
    }
    let participant_dids = indexer
        .store
        .participants(forum.uri.as_str(), None)
        .into_iter()
        .map(|p| p.did)
        .collect::<Vec<_>>();
    let unique_dids = participant_dids.iter().collect::<BTreeSet<_>>();
    assert_eq!(participant_dids.len(), participants);
    assert_eq!(unique_dids.len(), participants);
    for account in &accounts {
        assert!(
            indexer
                .store
                .heads(&[account.did().to_string()])
                .iter()
                .any(|h| h.did == account.did().as_str() && h.rev.is_some()),
            "missing indexed head for {}",
            account.did()
        );
    }

    // Independently prove the exact record the browser must display, then check the service's
    // served bundle against that head. This uses the real proof verifier, not the engine badge.
    let at = AtUri::parse(&post_uri)?;
    let proof_params = SyncGetRecord {
        did: at.did().clone(),
        collection: at.collection().clone(),
        rkey: at.rkey().clone(),
    };
    let pds_car = engine
        .xrpc()
        .query_raw(
            &Service::Anon(owner.pds().url()),
            &proof_params,
            &CallOpts::default(),
        )
        .await
        .context("fetch PDS proof")?
        .body;
    let proven = verify_proof(
        &pds_car,
        owner.did(),
        at.collection(),
        at.rkey(),
        &owner.public_key(),
        net.now_ms() * 1000,
        &Default::default(),
    )?;
    let proven_value = lessdugin_atp::datamodel::record_cbor_to_json(
        &proven
            .record
            .as_ref()
            .context("post absent in proof")?
            .bytes,
    )?;
    assert_eq!(proven_value["title"], TITLE);
    let head: lessdugin_atp::xrpc::CommitMeta = engine
        .xrpc()
        .query_endpoint(
            &Service::Anon(owner.pds().url()),
            &GetLatestCommit {
                did: owner.did().clone(),
            },
            &CallOpts::default(),
        )
        .await?;
    assert_eq!(head.rev, proven.rev.to_string());
    assert_eq!(head.cid, proven.commit_cid.to_string());
    let bundle = lessdugin_indexer::proofs::bundle(&indexer, std::slice::from_ref(&post_uri))
        .await
        .context("pre-indexed post proof")?;
    let parsed = ProofBundle::parse(&bundle, &Default::default())?;
    assert_eq!(parsed.items.len(), 1);
    let served = parsed.verify_item(0, &owner.public_key(), net.now_ms() * 1000)?;
    assert_eq!(served.rev, proven.rev);
    assert_eq!(served.commit_cid, proven.commit_cid);
    assert_eq!(served.record_cid(), proven.record_cid());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let indexer_url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let state = json!({
        "schema": 1,
        "pds": owner.pds_base(),
        "plc": net.plc(),
        "indexer": indexer_url,
        "forum": forum.uri,
        "post": post_uri,
        "title": TITLE,
        "ownerDid": owner.did(),
        // every post, newest first: the owner's, then the members'
        "posts": std::iter::once(json!({"uri": post_uri, "title": TITLE, "author": owner.did()}))
            .chain(member_posts)
            .collect::<Vec<_>>(),
        "participants": participants,
        "participantDids": participant_dids,
        "pdsCount": pds_count,
        "seedMs": seed_ms,
        "preindexMs": preindex_ms,
        "preindex": {
            "participants": indexed.participants,
            "posts": indexed.posts,
            "signedAccounts": net.accounts().len(),
            "uniqueParticipantDids": unique_dids.len(),
            "validatedMemberships": accounts.len() + 1,
            "indexedHeads": participants,
            "postProof": {
                "source": "PDS com.atproto.sync.getRecord",
                "bytes": pds_car.len(),
                "rev": proven.rev.to_string(),
                "commitCid": proven.commit_cid.to_string(),
                "recordCid": proven.record_cid().map(ToString::to_string),
                "titleVerified": true,
                "currentHeadVerified": true,
            },
            "servedProofBundle": {
                "bytes": bundle.len(),
                "items": parsed.items.len(),
                "verifiedAgainstPdsHead": true,
            },
            "fakeNetRequests": net.requests().len(),
        },
    });
    let text = serde_json::to_string_pretty(&state)?;
    std::fs::write(&output, &text).with_context(|| format!("write {}", output.display()))?;
    println!("{text}");
    eprintln!(
        "M6 fixture ready: {participants} participants on {pds_count} PDSes; seed {seed_ms} ms; pre-index {preindex_ms} ms; indexer {indexer_url}"
    );

    axum::serve(
        listener,
        api::router(indexer).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
