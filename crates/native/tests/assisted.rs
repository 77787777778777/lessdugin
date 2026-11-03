// SPDX-License-Identifier: AGPL-3.0-or-later
//! Assisted discovery and live updates over the real native HTTP/WebSocket host.

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lessdugin_atp::syntax::{Datetime, Tid};
use lessdugin_core::discovery::state::{DiscState, disc_rows};
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::dto::{ListPostsQuery, PostFilter, PostSort};
use lessdugin_core::engine::{Engine, Platform};
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

/// A loopback router that sends each WebSocket connection to the v1 or the v2 mini-Jetstream by
/// its request path, as one Jetstream instance serves both wires (`/subscribe` is v1).
async fn wire_router(v1: &Jetstream, v2: &Jetstream) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("ws://{}", listener.local_addr().unwrap());
    let backend = |s: &Jetstream| s.base_url().trim_start_matches("ws://").to_owned();
    let (v1, v2) = (backend(v1), backend(v2));
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let (v1, v2) = (v1.clone(), v2.clone());
            tokio::spawn(async move {
                let mut head = [0u8; 1024];
                let Ok(n) = tcp.peek(&mut head).await else {
                    return;
                };
                let line = String::from_utf8_lossy(&head[..n]);
                let path = line.split_whitespace().nth(1).unwrap_or_default();
                let target = if path.starts_with("/subscribe") { v1 } else { v2 };
                let Ok(mut up) = tokio::net::TcpStream::connect(target).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut up).await;
            });
        }
    });
    base
}

/// The open-mode scenario (plan M5 acceptance) over one Jetstream wire. The native engine always
/// subscribes with v2 `wildcard`; v1 carries only DID filters of more than 150 DIDs, so the v1
/// run is a browser-platform engine whose forum has 150 more members, and its newcomers arrive on
/// the v2 membership side connection (`sync.md` §6.2).
async fn open_mode_scenario(wire: Wire) {
    let net = FakeNet::serve_http(SystemClock);
    let owner = net.add_account("owner.test");
    let newcomer = net.add_account("newcomer.test");
    let late = net.add_account("late.test");
    let unrelated = net.add_account("unrelated.test");
    let stranger = net.add_account("stranger.test");
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
    if wire == Wire::V1 {
        for i in 0..150 {
            let member = net.add_account(&format!("member{i}.test"));
            put_membership(&member, &forum, now.clone());
            member.head();
        }
    }

    let constellation = Constellation::serve_http(&net);
    let v1 = Jetstream::start(&net, Wire::V1);
    let v2 = Jetstream::start(&net, Wire::V2);
    let jetstream = wire_router(&v1, &v2).await;
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
    devnet.jetstream = Some(Url::parse(&jetstream).unwrap());

    let tmp = tempfile::tempdir().unwrap();
    let host = NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        tmp.path(),
        Net::new(config.net_policy()).unwrap(),
    );
    let mut engine_config = engine_config(&config);
    if wire == Wire::V1 {
        engine_config.platform = Platform::Browser;
    }
    let engine = Engine::new(host.clone(), engine_config);
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
    // Wait until the source has subscribed (v1: its options_update; v1 also opens the v2
    // membership side connection), so what follows is streamed live.
    let main = |r: &lessdugin_testkit::jetstream::RequestLog| match wire {
        Wire::V1 => r.options.is_some(),
        Wire::V2 => r.url.contains("subscribeEvents") && !r.url.contains("kinds=commit"),
    };
    let side = |r: &lessdugin_testkit::jetstream::RequestLog| r.url.contains("kinds=commit");
    let deadline = Instant::now() + Duration::from_secs(10);
    let server = if wire == Wire::V1 { &v1 } else { &v2 };
    while !(server.requests().iter().any(main)
        && (wire == Wire::V2 || v2.requests().iter().any(side)))
    {
        assert!(Instant::now() < deadline, "the live source did not subscribe");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let subscribed = net.now_ms();

    // Identity and account events of a DID the client never resolved: network noise.
    unrelated.set_handle("unrelated-renamed.test");
    unrelated.deactivate();
    // A non-member's post is not forum content.
    let stranger_post = put_post(&stranger, &forum, "Not a member", now.clone());
    stranger.head();
    // A confirmed participant posts.
    let post_uri = put_post(&newcomer, &forum, "Live newcomer", now.clone());
    newcomer.head();
    // A newcomer joins after the subscription; only Jetstream announces it to this client.
    put_membership(&late, &forum, now.clone());
    let late_post = put_post(&late, &forum, "Late joiner", now.clone());
    late.head();

    // Only the cache is read until then: get_post would fetch a missing post itself (and
    // resolve its author, who would then be KNOWN before the Jetstream frame is handled). P1
    // live hints intentionally wait for the 2–30 s dirty-queue debounce (sync.md §6.1).
    let listed = |engine: &Engine<NativeHost<MemKv>>| {
        let engine = engine.clone();
        let forum = forum.uri.to_string();
        async move {
            engine
                .list_posts(&ListPostsQuery {
                    sort: PostSort::New,
                    // newcomers' posts are not on the frontpage (social.md V8)
                    filter: PostFilter::All,
                    ..ListPostsQuery::new(&forum)
                })
                .await
                .unwrap()
                .items
                .into_iter()
                .map(|p| p.uri)
                .collect::<Vec<_>>()
        }
    };
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let uris = listed(&engine).await;
        let late_row = disc_rows(host.cache(), &forum.fk)
            .await
            .unwrap()
            .remove(late.did().as_str());
        if uris.contains(&post_uri)
            && uris.contains(&late_post)
            && late_row.is_some_and(|r| r.state == DiscState::Confirmed)
        {
            break;
        }
        if Instant::now() >= deadline {
            let store = lessdugin_core::store::records::RecordStore::new(host.cache());
            let cached = store
                .get(&lessdugin_atp::syntax::AtUri::parse(&post_uri).unwrap())
                .await
                .unwrap()
                .is_some();
            let rows = disc_rows(host.cache(), &forum.fk).await.unwrap();
            panic!(
                "live updates did not arrive: listed {uris:?}; post cached {cached}; late row {:?}; queue {:?}; jetstream sent {:?}",
                rows.get(late.did().as_str()),
                engine.syncer().queue_snapshot(),
                server.requests().iter().map(|r| r.sent.len()).collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let view = engine.get_post(&post_uri).await.unwrap();
    assert_eq!(view.post.title.as_deref(), Some("Live newcomer"));
    assert_eq!(view.post.author.did, newcomer.did().as_str());
    let late_view = engine.get_post(&late_post).await.unwrap();
    assert_eq!(late_view.post.title.as_deref(), Some("Late joiner"));
    let late_row = disc_rows(host.cache(), &forum.fk)
        .await
        .unwrap()
        .remove(late.did().as_str())
        .expect("the late newcomer has no discovery row");
    assert!(
        late_row.src.iter().any(|s| s == "hint:jetstream"),
        "the late newcomer was not found through Jetstream: {:?}",
        late_row.src
    );
    assert!(!listed(&engine).await.contains(&stranger_post));
    engine.live_stop();
    tokio::time::timeout(Duration::from_secs(5), tail)
        .await
        .expect("live tail did not stop")
        .unwrap()
        .unwrap();

    match wire {
        Wire::V1 => {
            assert!(v1.requests().iter().any(|r| {
                r.options.as_ref().is_some_and(|o| {
                    o["wantedDids"].as_array().is_some_and(|d| d.len() > 150)
                })
            }));
            let side: Vec<String> = v2
                .requests()
                .iter()
                .filter(|r| side(r))
                .map(|r| r.url.clone())
                .collect();
            assert!(!side.is_empty());
            assert!(side.iter().all(|u| !u.contains("dids=")), "{side:?}");
        }
        Wire::V2 => assert!(v2.requests().iter().any(main)),
    }
    let requests = net.requests();
    assert!(
        requests.iter().any(|r| {
            r.did.as_deref() == Some(newcomer.did().as_str())
                && r.nsid.as_deref() == Some("com.atproto.repo.getRecord")
        }),
        "the socket hint must cause a PDS point fetch"
    );
    let noise: Vec<String> = requests
        .iter()
        .filter(|r| r.at_ms >= subscribed)
        .filter(|r| {
            r.did.as_deref() == Some(unrelated.did().as_str())
                || r.url.contains(unrelated.did().as_str())
        })
        .map(|r| r.url.clone())
        .collect();
    assert!(
        noise.is_empty(),
        "identity/account noise caused PLC or PDS requests: {noise:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn open_mode_discovers_and_applies_live_updates_over_v2() {
    open_mode_scenario(Wire::V2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_mode_discovers_and_applies_live_updates_over_v1() {
    open_mode_scenario(Wire::V1).await;
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
