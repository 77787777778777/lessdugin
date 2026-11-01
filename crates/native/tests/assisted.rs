// SPDX-License-Identifier: AGPL-3.0-or-later
//! Assisted discovery and live updates over the real native HTTP/WebSocket host.

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lessdugin_atp::syntax::{Datetime, Tid};
use lessdugin_core::discovery::state::{DiscState, disc_rows};
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::dto::{ListPostsQuery, PostSort};
use lessdugin_core::engine::Engine;
use lessdugin_core::host::{Host, KvOp, KvStore};
use lessdugin_core::live::SourceStatus;
use lessdugin_core::store::mem::MemKv;
use lessdugin_core::store::schema;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::records::{Forum, Membership, Post};
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_native::config::{Config, NetworkProfile, Sources};
use lessdugin_native::engine::engine_config;
use lessdugin_native::host::{NativeHost, Net};
use lessdugin_testkit::FakeNet;
use lessdugin_testkit::constellation::Constellation;
use lessdugin_testkit::fake_pds::{AccountHandle, SystemClock};
use lessdugin_testkit::jetstream::{Jetstream, Wire};
use url::Url;

fn put_membership(account: &AccountHandle, forum: &ForumKey, now: Datetime) {
    let value = Membership::new(forum.uri.clone(), now);
    account.put(
        nsid::MEMBERSHIP,
        &forum.fk,
        &serde_json::to_value(value).unwrap(),
    );
}

fn put_post(account: &AccountHandle, forum: &ForumKey, title: &str, now: Datetime) -> String {
    let tid = Tid::from_parts(u64::try_from(now.timestamp_micros()).unwrap(), 0).unwrap();
    // Post rkeys are forum-qualified; the live filter must map this record to its forum.
    let rkey = keys::post_rkey(forum.uri.as_str(), tid.as_str()).unwrap();
    let post = Post::article(forum.uri.clone(), title, "live body", now);
    let uri = account.uri(nsid::POST, &rkey);
    account.put(nsid::POST, &rkey, &serde_json::to_value(post).unwrap());
    uri
}

#[tokio::test(flavor = "multi_thread")]
async fn open_mode_discovers_and_applies_live_updates_from_the_pds() {
    let net = FakeNet::serve_http(SystemClock);
    let owner = net.add_account("owner.test");
    let newcomer = net.add_account("newcomer.test");
    let unrelated = net.add_account("unrelated.test");
    let forum = ForumKey::new(owner.did(), "main").unwrap();
    let now = Datetime::from_millis(net.now_ms()).unwrap();
    let declaration = Forum::create("main", now.clone(), MembershipMode::Open);
    owner.put(
        nsid::FORUM,
        "main",
        &serde_json::to_value(declaration).unwrap(),
    );
    put_membership(&owner, &forum, now.clone());
    put_membership(&newcomer, &forum, now.clone());
    owner.head();
    newcomer.head();

    let constellation = Constellation::serve_http(&net);
    let jetstream = Jetstream::start(&net, Wire::V2);
    let endpoints = net.config();
    let mut config = Config::devnet(Url::parse(&endpoints.plc).unwrap(), net.default_pds().url());
    config.sources = Some(Sources::Assisted);
    config.relays = endpoints
        .relays
        .iter()
        .map(|r| Url::parse(r).unwrap())
        .collect();
    config.constellation = Some(Url::parse(constellation.base_url()).unwrap());
    config.slingshot = None;
    let NetworkProfile::Devnet(devnet) = &mut config.network else {
        unreachable!()
    };
    devnet.jetstream = Some(Url::parse(jetstream.base_url()).unwrap());

    let tmp = tempfile::tempdir().unwrap();
    let host = NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        tmp.path(),
        Net::new(config.net_policy()).unwrap(),
    );
    let engine = Engine::new(host.clone(), engine_config(&config));
    engine.forum_open(forum.uri.as_str()).await.unwrap();
    engine
        .sync_forum(forum.uri.as_str(), &SyncOpts::default())
        .await
        .unwrap();
    engine.forum_follow(forum.uri.as_str(), None).await.unwrap();
    let discovered = disc_rows(host.cache(), &forum.fk).await.unwrap();
    assert_eq!(
        discovered.get(newcomer.did().as_str()).map(|r| &r.state),
        Some(&DiscState::Confirmed)
    );
    let posts = engine
        .list_posts(&ListPostsQuery {
            sort: PostSort::New,
            ..ListPostsQuery::new(forum.uri.as_str())
        })
        .await
        .unwrap();
    assert!(posts.items.is_empty());

    let tail = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.live_tail().await })
    };
    // Let the source subscribe before writing; the service then streams the new commit live.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let post_uri = put_post(&newcomer, &forum, "Live newcomer", now.clone());
    // Unrelated identity/account/commit traffic must not become forum content.
    put_post(&unrelated, &forum, "Not a member", now);
    newcomer.head();
    unrelated.head();

    // P1 live hints intentionally wait for the 2–30 s dirty-queue debounce (sync.md §6.1).
    let deadline = Instant::now() + Duration::from_secs(35);
    let view = loop {
        match engine.get_post(&post_uri).await {
            Ok(view) => break view,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => panic!("live post was not fetched from the PDS: {error}"),
        }
    };
    assert_eq!(view.post.title.as_deref(), Some("Live newcomer"));
    assert_eq!(view.post.author.did, newcomer.did().as_str());
    engine.live_stop();
    tokio::time::timeout(Duration::from_secs(5), tail)
        .await
        .expect("live tail did not stop")
        .unwrap()
        .unwrap();
    assert!(
        jetstream
            .requests()
            .iter()
            .any(|r| { r.url.contains("network.bsky.jetstream.subscribeEvents") })
    );
    assert!(
        net.requests().iter().any(|r| {
            r.did.as_deref() == Some(newcomer.did().as_str())
                && r.nsid.as_deref() == Some("com.atproto.repo.getRecord")
        }),
        "the socket hint must cause a PDS point fetch"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn native_host_consumes_the_v1_options_update_wire() {
    let net = FakeNet::serve_http(SystemClock);
    let service = Jetstream::start(&net, Wire::V1);
    let author = net.add_account("author.test");
    let forum = ForumKey::new(author.did(), "main").unwrap();
    let now = Datetime::from_millis(net.now_ms()).unwrap();
    put_membership(&author, &forum, now.clone());
    let uri = put_post(&author, &forum, "v1", now.clone());
    author.head();
    let mut config = Config::devnet(
        Url::parse(&net.config().plc).unwrap(),
        net.default_pds().url(),
    );
    let NetworkProfile::Devnet(devnet) = &mut config.network else {
        unreachable!()
    };
    devnet.jetstream = Some(Url::parse(service.base_url()).unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let host = Arc::new(NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        tmp.path(),
        Net::new(config.net_policy()).unwrap(),
    ));
    let mut source = lessdugin_core::live::LiveSource::new(
        Arc::clone(&host),
        lessdugin_core::live::LiveConfig::new(vec![Url::parse(service.base_url()).unwrap()], true),
    );
    let did = author.did().to_string();
    let mut filter_dids = vec![did.clone(); 151];
    filter_dids.extend((0..150).map(|n| format!("did:plc:participant{n:024}")));
    host.cache()
        .write(vec![KvOp::Put(
            schema::cur_key(lessdugin_core::live::CURSOR_KEY),
            lessdugin_core::store::row::encode(&serde_json::json!({
                "v": 1,
                "wire": "v1",
                "instance": service.base_url(),
                "timeUs": now.timestamp_micros() - 1_000,
            }))
            .unwrap(),
        )])
        .await
        .unwrap();
    let context = lessdugin_core::live::Context {
        known_dids: [did.clone()].into(),
        filter_dids,
        own_dids: [did].into(),
        forums: [forum.fk.clone()].into(),
        declarations: [(author.did().to_string(), "main".into())].into(),
        participants_known: true,
        ..lessdugin_core::live::Context::default()
    };
    let mut found = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !found && Instant::now() < deadline {
        let status = source.next(&context).await.unwrap();
        let SourceStatus::Actions(actions) = status else {
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        found |= actions.iter().any(|action| match action {
            lessdugin_core::live::Action::PointFetch { uri: found, .. } => found == &uri,
            _ => false,
        });
        source.acknowledge().await.unwrap();
        if found {
            break;
        }
    }
    assert!(found, "v1 stream did not yield the post point-fetch hint");
    source.stop().await;
    assert!(service.requests().iter().any(|r| {
        r.options.as_ref().is_some_and(|options| {
            options["wantedDids"]
                .as_array()
                .is_some_and(|d| d.len() >= 151)
        })
    }));
}
