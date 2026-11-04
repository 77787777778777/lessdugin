// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::Arc;

use futures::executor::block_on;
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_core::discovery::sources::{
    CONSTELLATION_HINTS_MAX, CONSTELLATION_PAGE_LIMIT, CorePlusHints, DIRECTORY_OWNERS_MAX,
    DidHints, GET_BACKLINKS, GET_BACKLINKS_COUNT, LIST_REPOS_BY_COLLECTION,
    NEWCOMER_RELAY_UNION_LIMIT, NewcomerHints, RECENT_POSTS_DISCOVERY, RELAY_PAGE_LIMIT,
    SourceEndpoints, SourceError, Sources,
};
use lessdugin_core::discovery::state::{
    DISC_ROWS_MAX, DiscRow, DiscState, disc_rows, put_disc_rows, relay_rows,
};
use lessdugin_core::discovery::{ForumKey, NEWCOMER_BUDGET, SyncOpts};
use lessdugin_core::engine::{Engine, EngineConfig, Platform, Sources as SourceMode};
use lessdugin_core::host::{Host, HttpRequest, KvOp, KvStore, Target};
use lessdugin_core::store::RecordStore;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{Forum, Membership};
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_testkit::fake_pds::ServiceKind;
use lessdugin_testkit::{FakeNet, MemHost, resp};
use serde_json::{Value, json};
use url::Url;

const CONSTELLATION: &str = "https://constellation.example.com";
const RELAY1: &str = "https://relay-one.example.com";
const RELAY2: &str = "https://relay-two.example.com";

fn forum() -> ForumKey {
    ForumKey::new(&Did::parse("did:web:forum.example.com").unwrap(), "main").unwrap()
}

fn endpoints() -> SourceEndpoints {
    SourceEndpoints {
        constellation: Some(Url::parse(CONSTELLATION).unwrap()),
        relays: vec![Url::parse(RELAY1).unwrap(), Url::parse(RELAY2).unwrap()],
    }
}

fn service(host: &MemHost) -> Sources<MemHost> {
    Sources::new(host.clone(), endpoints())
}

fn engine(host: &MemHost, mode: SourceMode) -> Engine<MemHost> {
    Engine::new(
        host.clone(),
        EngineConfig {
            sources: mode,
            constellation: Some(Url::parse(CONSTELLATION).unwrap()),
            relays: vec![Url::parse(RELAY1).unwrap()],
            ..EngineConfig::default()
        },
    )
}

fn backlink(did: &str, collection: &str, rkey: &str) -> Value {
    json!({"did": did, "collection": collection, "rkey": rkey})
}

fn repo(did: &str) -> Value {
    json!({"did": did})
}

fn request_url(req: &HttpRequest) -> &Url {
    match &req.target {
        Target::Anon(url) => url,
        Target::UserPds { .. } => panic!("CORE+ request carried credentials"),
    }
}

fn query(req: &HttpRequest, name: &str) -> Option<String> {
    request_url(req)
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

#[test]
fn direct_mode_refuses_without_network() {
    let host = MemHost::new();
    let engine = engine(&host, SourceMode::Direct);
    let err = block_on(engine.discover_recently_active(&forum())).unwrap_err();
    assert_eq!(err, SourceError::DirectMode);
    assert!(host.requests().is_empty());
}

#[test]
fn mode_change_between_pages_stops_the_next_request() {
    let host = MemHost::new();
    let engine = engine(&host, SourceMode::Assisted);
    let route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}");
    let writer = host.clone();
    host.on_get(&route, move |_| {
        let write = writer.state().write(vec![KvOp::Put(
            lessdugin_core::key!("loc", "setting", "sources"),
            b"direct".to_vec(),
        )]);
        futures::pin_mut!(write);
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);
        let std::task::Poll::Ready(result) = write.as_mut().poll(&mut cx) else {
            panic!("MemKv write unexpectedly pending");
        };
        result.unwrap();
        Ok(resp::ok_json(&json!({
            "records": [],
            "cursor": "next"
        })))
    });
    let err = block_on(engine.discover_recently_active(&forum())).unwrap_err();
    assert_eq!(err, SourceError::DirectMode);
    assert_eq!(host.requests_to(&route).len(), 1);
}

#[test]
fn constellation_short_page_continues_and_filters_exactly() {
    let host = MemHost::new();
    let f = forum();
    let route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}");
    host.on_get(&route, move |req| {
        assert_eq!(query(req, "subject").as_deref(), Some(f.uri.as_str()));
        assert_eq!(
            query(req, "source").as_deref(),
            Some(format!("{}:forum", nsid::MEMBERSHIP).as_str())
        );
        assert_eq!(query(req, "limit").as_deref(), Some("100"));
        let body = match query(req, "cursor").as_deref() {
            None => json!({
                // Deliberately short, but not the end.
                "records": [backlink("did:web:wrong.example.com", nsid::MEMBERSHIP, "not-the-fk")],
                "cursor": "page-2"
            }),
            Some("page-2") => json!({
                "records": [
                    backlink("did:web:newest.example.com", nsid::MEMBERSHIP, &f.fk),
                    backlink("did:web:other.example.com", nsid::POST, &f.fk),
                    backlink("did:web:oldest.example.com", nsid::MEMBERSHIP, &f.fk)
                ],
                "cursor": null
            }),
            other => panic!("unexpected cursor {other:?}"),
        };
        Ok(resp::ok_json(&body))
    });
    let hints = block_on(service(&host).constellation_newcomers(&forum())).unwrap();
    assert_eq!(
        hints.dids,
        ["did:web:newest.example.com", "did:web:oldest.example.com"]
    );
    assert_eq!(hints.stats.requests, 2);
    assert_eq!(hints.stats.pages, 2);
    assert_eq!(hints.stats.returned, 2);
    assert_eq!(hints.stats.dropped, 2);
    assert!(
        host.requests()
            .iter()
            .all(|r| r.url.starts_with(CONSTELLATION))
    );
}

#[test]
fn constellation_repeated_cursor_is_an_error() {
    let host = MemHost::new();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
        resp::ok_json(&json!({"records": [], "cursor": "same"})),
    );
    let err = block_on(service(&host).recently_active(&forum())).unwrap_err();
    assert!(matches!(
        err,
        SourceError::RepeatedCursor {
            service: "Constellation",
            page: 2
        }
    ));
    assert_eq!(host.requests().len(), 2);
}

#[test]
fn malformed_constellation_records_and_invalid_dids_are_rejected() {
    for records in [
        json!([null]),
        json!([{"did": 7, "collection": nsid::MEMBERSHIP, "rkey": "x"}]),
        json!([backlink("not-a-did", nsid::MEMBERSHIP, "x")]),
        json!([backlink("did:plc:short", nsid::MEMBERSHIP, "x")]),
    ] {
        let host = MemHost::new();
        host.respond_get(
            &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
            resp::ok_json(&json!({"records": records, "cursor": null})),
        );
        let err = block_on(service(&host).recently_active(&forum())).unwrap_err();
        assert!(matches!(err, SourceError::Malformed { .. }), "{err}");
        assert_eq!(host.requests().len(), 1);
    }
}

#[test]
fn recent_activity_uses_post_forum_and_preserves_order() {
    let host = MemHost::new();
    let route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}");
    host.on_get(&route, |req| {
        assert_eq!(
            query(req, "source").as_deref(),
            Some(format!("{}:forum", nsid::POST).as_str())
        );
        Ok(resp::ok_json(&json!({
            "records": [
                backlink("did:web:new.example.com", nsid::POST, "any-rkey"),
                backlink("did:web:old.example.com", nsid::POST, "another-rkey")
            ],
            "cursor": null
        })))
    });
    let hints = block_on(service(&host).recently_active(&forum())).unwrap();
    assert_eq!(
        hints.dids,
        ["did:web:new.example.com", "did:web:old.example.com"]
    );
}

#[test]
fn constellation_count_is_a_single_exact_anonymous_query() {
    let host = MemHost::new();
    let f = forum();
    let route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS_COUNT}");
    host.on_get(&route, move |req| {
        assert_eq!(query(req, "subject").as_deref(), Some(f.uri.as_str()));
        assert_eq!(
            query(req, "source").as_deref(),
            Some(format!("{}:forum", nsid::MEMBERSHIP).as_str())
        );
        assert_eq!(query(req, "limit"), None);
        Ok(resp::ok_json(&json!({"total": 42})))
    });
    let count = block_on(service(&host).membership_count(&forum())).unwrap();
    assert_eq!(count.total, 42);
    assert_eq!(count.stats.requests, 1);
    assert_eq!(count.stats.pages, 0);
}

#[test]
fn relay_pages_and_deduplicates_across_relays() {
    let host = MemHost::new();
    let route = format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}");
    host.on_get(&route, |req| {
        assert_eq!(query(req, "collection").as_deref(), Some(nsid::MEMBERSHIP));
        assert_eq!(query(req, "limit").as_deref(), Some("500"));
        let body = if query(req, "cursor").is_none() {
            json!({
                "repos": [repo("did:web:a.example.com"), repo("did:web:b.example.com")],
                "cursor": "next"
            })
        } else {
            json!({
                "repos": [repo("did:web:b.example.com"), repo("did:web:c.example.com")]
            })
        };
        Ok(resp::ok_json(&body))
    });
    host.respond_get(
        &format!("{RELAY2}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({
            "repos": [repo("did:web:c.example.com"), repo("did:web:d.example.com")],
            "cursor": null
        })),
    );
    let hints = block_on(service(&host).relay_newcomers()).unwrap();
    assert_eq!(
        hints.dids,
        [
            "did:web:a.example.com",
            "did:web:b.example.com",
            "did:web:c.example.com",
            "did:web:d.example.com"
        ]
    );
    // relay DIDs are candidates for PDS confirmation (§4.2.1): no Constellation query each
    assert_eq!(hints.stats.pages, 3);
    assert_eq!(hints.stats.returned, 6);
    assert!(host.requests_to(CONSTELLATION).is_empty());
}

#[test]
fn relay_repeated_cursor_is_an_error() {
    let host = MemHost::new();
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": [], "cursor": "same"})),
    );
    let mut eps = endpoints();
    eps.relays.truncate(1);
    let err = block_on(Sources::new(host.clone(), eps).relay_newcomers()).unwrap_err();
    assert!(matches!(
        err,
        SourceError::RepeatedCursor {
            service: "relay",
            page: 2
        }
    ));
}

/// Serves `dids` from `relay` in pages of at most `RELAY_PAGE_LIMIT`, with numeric cursors.
fn paged_relay(host: &MemHost, relay: &str, dids: Vec<String>) {
    host.on_get(
        &format!("{relay}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        move |req| {
            let at: usize = query(req, "cursor").map_or(0, |c| c.parse().unwrap());
            let end = (at + RELAY_PAGE_LIMIT).min(dids.len());
            let repos: Vec<Value> = dids[at..end].iter().map(|d| repo(d)).collect();
            let cursor = (end < dids.len()).then(|| end.to_string());
            Ok(resp::ok_json(&json!({"repos": repos, "cursor": cursor})))
        },
    );
}

#[test]
fn newcomer_cap_applies_to_the_union_across_relays() {
    let host = MemHost::new();
    // pages of at most the requested limit (a larger page is malformed, security-net-5)
    let first: Vec<String> = (0..NEWCOMER_RELAY_UNION_LIMIT)
        .map(|i| format!("did:web:user{i:04}.example.com"))
        .collect();
    paged_relay(&host, RELAY1, first);
    host.respond_get(
        &format!("{RELAY2}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": [repo("did:web:overflow.example.com")]})),
    );
    let err = block_on(service(&host).relay_newcomers()).unwrap_err();
    assert_eq!(
        err,
        SourceError::NewcomerUnionTooLarge {
            limit: NEWCOMER_RELAY_UNION_LIMIT,
            observed: NEWCOMER_RELAY_UNION_LIMIT + 1
        }
    );
}

#[test]
fn directory_owner_listing_is_not_newcomer_capped() {
    let host = MemHost::new();
    let owners: Vec<String> = (0..=NEWCOMER_RELAY_UNION_LIMIT)
        .map(|i| format!("did:web:owner{i:04}.example.com"))
        .collect();
    paged_relay(&host, RELAY1, owners);
    let mut eps = endpoints();
    eps.relays.truncate(1);
    let hints = block_on(Sources::new(host.clone(), eps).relay_repositories(nsid::FORUM)).unwrap();
    assert_eq!(hints.dids.len(), NEWCOMER_RELAY_UNION_LIMIT + 1);
}

#[test]
fn malformed_relay_entries_and_wildcards_are_rejected() {
    for repos in [
        json!([42]),
        json!([{"did": false}]),
        json!([repo("did:web:UPPER.example.com")]),
    ] {
        let host = MemHost::new();
        host.respond_get(
            &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
            resp::ok_json(&json!({"repos": repos})),
        );
        let mut eps = endpoints();
        eps.relays.truncate(1);
        let err = block_on(Sources::new(host.clone(), eps).relay_newcomers()).unwrap_err();
        assert!(matches!(err, SourceError::Malformed { .. }), "{err}");
    }
    let host = MemHost::new();
    let err =
        block_on(service(&host).relay_repositories("online.7778777.lessdugin.*")).unwrap_err();
    assert!(matches!(err, SourceError::InvalidInput(_)));
    assert!(host.requests().is_empty());
}

#[test]
fn requests_have_caps_no_headers_no_userinfo_and_no_redirects() {
    let host = MemHost::new();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS_COUNT}"),
        resp::ok_json(&json!({"total": 0})),
    );
    block_on(service(&host).membership_count(&forum())).unwrap();
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert!(matches!(req.target, Target::Anon(_)));
    assert!(req.headers.is_empty());
    assert!(req.body.is_none());
    assert_eq!(
        req.max_bytes,
        lessdugin_core::limits::DEFAULT_RESPONSE_MAX_BYTES
    );
    assert_eq!(req.timeout_ms, lessdugin_core::limits::REQUEST_TIMEOUT_MS);
    assert_eq!(req.redirects, lessdugin_core::host::Redirects::None);
    let Target::Anon(url) = &req.target else {
        panic!("CORE+ request carried credentials");
    };
    assert!(url.username().is_empty());

    let host = MemHost::new();
    let eps = SourceEndpoints {
        constellation: Some(Url::parse("https://user:password@constellation.example.com").unwrap()),
        relays: vec![],
    };
    let err = block_on(Sources::new(host.clone(), eps).membership_count(&forum())).unwrap_err();
    assert!(matches!(err, SourceError::InvalidInput(_)));
    assert!(host.requests().is_empty());
}

#[test]
fn transport_http_and_json_errors_are_structured() {
    let host = MemHost::new();
    host.fail(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS_COUNT}"),
        lessdugin_core::host::HostError::Timeout,
    );
    assert!(matches!(
        block_on(service(&host).membership_count(&forum())),
        Err(SourceError::Transport { .. })
    ));

    let host = MemHost::new();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS_COUNT}"),
        resp::status(503),
    );
    assert!(matches!(
        block_on(service(&host).membership_count(&forum())),
        Err(SourceError::Http { status: 503, .. })
    ));

    let host = MemHost::new();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS_COUNT}"),
        resp::text(200, "not json"),
    );
    assert!(matches!(
        block_on(service(&host).membership_count(&forum())),
        Err(SourceError::Malformed { .. })
    ));
}

/// Relay hints name repos with some membership, not a forum: each relay DID is a candidate
/// confirmed at its own PDS (§4.4, §4.2.1), without a Constellation query per DID
/// (m5-acceptance-2). A member found only through the relay is confirmed; a relay-listed repo
/// with no membership of this forum is rejected after one membership read and never listed.
/// One `sync_forum` runs discovery once; direct mode contacts no CORE+ service.
#[test]
fn engine_sync_forum_assisted_confirms_relay_candidates_and_direct_uses_no_core_plus() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let selected = net.add_account("selected.example.com");
    let relayed = net.add_account("relayed.example.com");
    let outsider = net.add_account("outsider.example.com");
    let key = ForumKey::new(owner.did(), "open").unwrap();
    let elsewhere = ForumKey::new(owner.did(), "elsewhere").unwrap();
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    for slug in ["open", "elsewhere"] {
        let declaration = Forum::create(slug, at.clone(), MembershipMode::Open);
        owner.put(
            nsid::FORUM,
            slug,
            &serde_json::to_value(declaration).unwrap(),
        );
    }
    for account in [&owner, &selected, &relayed] {
        let membership = Membership::new(key.uri.clone(), at.clone());
        account.put(
            nsid::MEMBERSHIP,
            &key.fk,
            &serde_json::to_value(membership).unwrap(),
        );
    }
    // the outsider is in every relay's membership directory, but for another forum
    let membership = Membership::new(elsewhere.uri.clone(), at.clone());
    outsider.put(
        nsid::MEMBERSHIP,
        &elsewhere.fk,
        &serde_json::to_value(membership).unwrap(),
    );

    // Constellation knows only the selected newcomer.
    let backlink_route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}");
    host.on_get(&backlink_route, {
        let key = key.clone();
        let selected = selected.did().to_string();
        move |req| {
            assert_eq!(query(req, "subject").as_deref(), Some(key.uri.as_str()));
            assert_eq!(query(req, "did"), None);
            let source = query(req, "source").unwrap();
            if source == format!("{}:forum", nsid::POST) {
                return Ok(resp::ok_json(&json!({"records": [], "cursor": null})));
            }
            assert_eq!(source, format!("{}:forum", nsid::MEMBERSHIP));
            let records = vec![backlink(&selected, nsid::MEMBERSHIP, &key.fk)];
            Ok(resp::ok_json(&json!({"records": records, "cursor": null})))
        }
    });
    let engine = Engine::new(
        host.clone(),
        EngineConfig {
            sources: SourceMode::Assisted,
            constellation: Some(Url::parse(CONSTELLATION).unwrap()),
            relays: net
                .relays()
                .into_iter()
                .map(|relay| Url::parse(&relay).unwrap())
                .collect(),
            ..EngineConfig::default()
        },
    );
    let report = block_on(engine.sync_forum(key.uri.as_str(), &SyncOpts::default())).unwrap();
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(
        host.requests_to(&backlink_route).len(),
        2,
        "one recently-active and one newcomer listing: discovery runs once per sync"
    );
    let disc = block_on(disc_rows(host.cache(), &key.fk)).unwrap();
    let state = |d: &Did| disc.get(d.as_str()).map(|row| row.state);
    assert_eq!(state(selected.did()), Some(DiscState::Confirmed));
    assert_eq!(state(relayed.did()), Some(DiscState::Confirmed));
    // a relay-only candidate is a disc row only once confirmed: its rejection is a relay row,
    // which keeps the back-off (sync.md §4.4)
    assert_eq!(state(outsider.did()), None);
    let rejected = block_on(relay_rows(host.cache(), &key.fk)).unwrap();
    assert_eq!(
        rejected.keys().collect::<Vec<_>>(),
        [&outsider.did().to_string()]
    );
    assert_eq!(rejected[outsider.did().as_str()].state, DiscState::Rejected);
    let store = RecordStore::new(host.cache());
    for member in [&selected, &relayed] {
        let membership_uri = AtUri::parse(&member.uri(nsid::MEMBERSHIP, &key.fk)).unwrap();
        assert!(
            block_on(store.get(&membership_uri)).unwrap().is_some(),
            "a newcomer's valid PDS membership was confirmed and stored"
        );
    }
    let outsider_reads: Vec<_> = net
        .requests()
        .into_iter()
        .filter(|r| r.did.as_deref() == Some(outsider.did().as_str()))
        .filter_map(|r| r.nsid)
        .collect();
    assert!(
        outsider_reads
            .iter()
            .all(|n| n != "com.atproto.repo.listRecords"),
        "a rejected relay candidate is never listed: {outsider_reads:?}"
    );

    // a background pass keeps the rejected relay candidate in its back-off: nothing of it is
    // read again
    net.clear_requests();
    let background = SyncOpts {
        background: true,
        ..SyncOpts::default()
    };
    block_on(engine.sync_forum(key.uri.as_str(), &background)).unwrap();
    assert!(
        net.requests()
            .iter()
            .all(|r| r.did.as_deref() != Some(outsider.did().as_str())),
        "a relay hint names no forum: it does not end a rejection's back-off"
    );

    let direct_host = MemHost::new();
    net.install(&direct_host);
    let direct = Engine::new(
        direct_host.clone(),
        EngineConfig {
            sources: SourceMode::Direct,
            constellation: Some(Url::parse(CONSTELLATION).unwrap()),
            ..EngineConfig::default()
        },
    );
    net.clear_requests();
    block_on(direct.sync_forum(key.uri.as_str(), &SyncOpts::default())).unwrap();
    assert!(
        direct_host
            .requests()
            .iter()
            .all(|request| !request.url.starts_with(CONSTELLATION))
    );
    assert!(
        net.requests()
            .iter()
            .all(|request| request.service != ServiceKind::Relay)
    );
}

/// A relay entry names a repo with some membership, not one of this forum: of an honest relay's
/// directory, the relay-only candidates that are checked and rejected, or left over the budget,
/// must not become repos `live_tail` polls or DIDs of the browser's Jetstream filter (they get no
/// discovery row; the review of m5-acceptance-2 found each of them a followed repo).
#[test]
fn rejected_or_over_budget_relay_candidates_are_not_followed_live() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let relayed = net.add_account("relayed.example.com");
    let rejected = net.add_account("rejected.example.com");
    let over = net.add_account("over.example.com");
    let key = ForumKey::new(owner.did(), "open").unwrap();
    let elsewhere = ForumKey::new(owner.did(), "elsewhere").unwrap();
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    for slug in ["open", "elsewhere"] {
        let declaration = Forum::create(slug, at.clone(), MembershipMode::Open);
        owner.put(
            nsid::FORUM,
            slug,
            &serde_json::to_value(declaration).unwrap(),
        );
    }
    for (account, forum) in [
        (&relayed, &key),
        (&rejected, &elsewhere),
        (&over, &elsewhere),
    ] {
        let membership = Membership::new(forum.uri.clone(), at.clone());
        account.put(
            nsid::MEMBERSHIP,
            &forum.fk,
            &serde_json::to_value(membership).unwrap(),
        );
    }
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
        resp::ok_json(&json!({"records": [], "cursor": null})),
    );
    // the relay lists the member and one non-member within the budget, then more repos than
    // the budget holds (whose DIDs do not resolve), then another non-member
    let mut listed = vec![relayed.did().to_string(), rejected.did().to_string()];
    listed.extend((0..NEWCOMER_BUDGET + 10).map(|i| format!("did:web:user{i:04}.example.com")));
    listed.push(over.did().to_string());
    paged_relay(&host, RELAY1, listed);
    let engine = Engine::new(
        host.clone(),
        EngineConfig {
            sources: SourceMode::Assisted,
            constellation: Some(Url::parse(CONSTELLATION).unwrap()),
            relays: vec![Url::parse(RELAY1).unwrap()],
            jetstream: vec![Url::parse("wss://jetstream.example/subscribe").unwrap()],
            platform: Platform::Browser,
            ..EngineConfig::default()
        },
    );
    block_on(engine.forum_follow(key.uri.as_str(), None)).unwrap();
    block_on(engine.sync_forum(key.uri.as_str(), &SyncOpts::default())).unwrap();
    let disc = block_on(disc_rows(host.cache(), &key.fk)).unwrap();
    assert_eq!(
        disc.get(relayed.did().as_str()).map(|r| r.state),
        Some(DiscState::Confirmed),
        "a relay-only member is confirmed at its PDS"
    );
    let others: Vec<&String> = disc
        .keys()
        .filter(|d| d.as_str() != relayed.did().as_str())
        .collect();
    assert!(
        others.is_empty(),
        "only the confirmed relay candidate is a discovery row; {} others, from {:?}",
        others.len(),
        others.first()
    );
    let checked = block_on(relay_rows(host.cache(), &key.fk)).unwrap();
    assert_eq!(
        checked.keys().collect::<Vec<_>>(),
        [&rejected.did().to_string()],
        "the checked non-member keeps its back-off; the one over the budget is not kept"
    );

    let outsiders = [rejected.did().to_string(), over.did().to_string()];
    let names = |url: &str| {
        outsiders
            .iter()
            .any(|d| url.contains(d.as_str()) || url.contains(&d.replace(':', "%3A")))
    };
    host.clear_requests();
    net.clear_requests();
    for _ in 0..10 {
        block_on(engine.live_step()).unwrap();
        host.advance_ms(2 * 60 * 60_000);
    }
    let touched: Vec<String> = host
        .requests()
        .into_iter()
        .map(|r| r.url)
        .filter(|u| names(u))
        .collect();
    assert!(
        touched.is_empty(),
        "live_step polled outsiders: {touched:?}"
    );
    assert!(
        net.requests().iter().all(|r| r
            .did
            .as_deref()
            .is_none_or(|d| !outsiders.iter().any(|o| o == d))),
        "no request of live_step is for an outsider's repo"
    );

    // the browser's Jetstream filter: the forum's DIDs, not the relay's directory (in the
    // connection URL up to 150 DIDs, else in an `options_update` sent once connected)
    host.script_websocket("wss://jetstream.example/", Vec::new());
    host.park_sleeps(true);
    block_on(async {
        let tail = engine.live_tail();
        futures::pin_mut!(tail);
        assert!(futures::poll!(tail.as_mut()).is_pending());
        engine.live_stop();
        host.park_sleeps(false);
        tail.await.unwrap();
    });
    let urls = host.websocket_urls();
    let mut filters: Vec<String> = urls
        .iter()
        .flat_map(|u| {
            Url::parse(u)
                .unwrap()
                .query_pairs()
                .filter(|(k, _)| k == "dids")
                .map(|(_, v)| v.into_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    for sent in host.websocket_sent() {
        let sent: Value = serde_json::from_str(&sent).unwrap();
        if let Some(dids) = sent["payload"]["wantedDids"].as_array() {
            filters.extend(dids.iter().filter_map(|d| Some(d.as_str()?.to_owned())));
        }
    }
    assert!(
        filters.iter().any(|d| d == owner.did().as_str()),
        "a DID-filtered connection was opened: {urls:?}"
    );
    let named: Vec<&String> = filters.iter().filter(|d| outsiders.contains(d)).collect();
    assert!(
        named.is_empty(),
        "the Jetstream filter of {} DIDs names outsiders: {named:?}",
        filters.len()
    );
}

/// Discovery's own writes keep at most `DISC_ROWS_MAX` discovery rows per forum, dropping the
/// oldest rows that are not confirmed, as the Jetstream candidate write does (sync.md §4.4,
/// §6.4): hint sources that name ever new DIDs cannot grow the cache without bound.
#[test]
fn discovery_keeps_at_most_disc_rows_max_rows_per_forum() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let key = ForumKey::new(owner.did(), "open").unwrap();
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    let declaration = Forum::create("open", at.clone(), MembershipMode::Open);
    owner.put(
        nsid::FORUM,
        "open",
        &serde_json::to_value(declaration).unwrap(),
    );
    let membership = Membership::new(key.uri.clone(), at.clone());
    owner.put(
        nsid::MEMBERSHIP,
        &key.fk,
        &serde_json::to_value(membership).unwrap(),
    );
    // a full forum: the oldest row is confirmed, the next two are the oldest unconfirmed
    let old = host.now() - 30 * 86_400_000;
    let rows: std::collections::BTreeMap<String, DiscRow> = (0..DISC_ROWS_MAX)
        .map(|i| {
            let mut row = DiscRow::new("hint:constellation", old + i as i64);
            if i == 0 {
                row.set(DiscState::Confirmed, old);
            }
            (format!("did:web:old{i:05}.example.com"), row)
        })
        .collect();
    block_on(put_disc_rows(host.cache(), &key.fk, &rows)).unwrap();
    let opts = SyncOpts {
        background: true,
        newcomers: vec![
            (
                "did:web:new1.example.com".to_owned(),
                "jetstream".to_owned(),
            ),
            (
                "did:web:new2.example.com".to_owned(),
                "jetstream".to_owned(),
            ),
        ],
        ..SyncOpts::default()
    };
    let direct = engine(&host, SourceMode::Direct);
    block_on(direct.sync_forum(key.uri.as_str(), &opts)).unwrap();
    let disc = block_on(disc_rows(host.cache(), &key.fk)).unwrap();
    assert_eq!(disc.len(), DISC_ROWS_MAX);
    assert!(
        disc.contains_key("did:web:old00000.example.com"),
        "confirmed"
    );
    for dropped in [
        "did:web:old00001.example.com",
        "did:web:old00002.example.com",
    ] {
        assert!(
            !disc.contains_key(dropped),
            "{dropped} is the oldest unconfirmed"
        );
    }
    assert!(disc.contains_key("did:web:old00003.example.com"));
    for new in ["did:web:new1.example.com", "did:web:new2.example.com"] {
        assert!(disc.contains_key(new), "{new} was just hinted");
    }
}

#[test]
fn combined_hints_feed_existing_syncopt_sinks_and_deduplicate() {
    let host = MemHost::new();
    let f = forum();
    let backlink_route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}");
    host.on_get(&backlink_route, {
        let f = f.clone();
        move |req| {
            let source = query(req, "source").unwrap();
            let collection = if source == format!("{}:forum", nsid::POST) {
                nsid::POST
            } else {
                nsid::MEMBERSHIP
            };
            Ok(resp::ok_json(&json!({
                "records": [backlink("did:web:shared.example.com", collection, &f.fk)],
                "cursor": null
            })))
        }
    });
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": [repo("did:web:shared.example.com")]})),
    );
    let engine = engine(&host, SourceMode::Assisted);
    let hints = block_on(engine.discover_forum_hints(&f)).unwrap();
    assert_eq!(hints.recently_active, ["did:web:shared.example.com"]);
    assert_eq!(
        hints.newcomers,
        [(
            "did:web:shared.example.com".to_owned(),
            "constellation".to_owned()
        )]
    );
    let mut opts = SyncOpts {
        newcomers: vec![(
            "did:web:existing.example.com".to_owned(),
            "jetstream".to_owned(),
        )],
        ..SyncOpts::default()
    };
    hints.apply_to(&mut opts);
    assert_eq!(opts.recently_active, hints.recently_active);
    assert_eq!(opts.newcomers.len(), 2);
    assert_eq!(opts.newcomers[0].1, "jetstream");
}

/// One relay source failing, the 5,000-DID union cap included, used to throw away every CORE+
/// hint of the sync (m5-acceptance-2). The sources are independent: Constellation's newcomer and
/// the recently active author survive, the error is reported, and above the cap the relay source
/// is off for the rest of the session (no further relay request).
#[test]
fn relay_union_cap_keeps_the_other_hints_and_turns_the_relay_off_for_the_session() {
    let host = MemHost::new();
    let f = forum();
    host.on_get(&format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"), {
        let f = f.clone();
        move |req| {
            let post = query(req, "source").unwrap() == format!("{}:forum", nsid::POST);
            let record = if post {
                backlink("did:web:active.example.com", nsid::POST, "any")
            } else {
                backlink("did:web:newcomer.example.com", nsid::MEMBERSHIP, &f.fk)
            };
            Ok(resp::ok_json(&json!({"records": [record], "cursor": null})))
        }
    });
    let many: Vec<String> = (0..=NEWCOMER_RELAY_UNION_LIMIT)
        .map(|i| format!("did:web:user{i:04}.example.com"))
        .collect();
    paged_relay(&host, RELAY1, many);
    let engine = engine(&host, SourceMode::Assisted);
    let hints = block_on(engine.discover_forum_hints(&f)).unwrap();
    assert_eq!(hints.recently_active, ["did:web:active.example.com"]);
    assert_eq!(
        hints.newcomers,
        [(
            "did:web:newcomer.example.com".to_owned(),
            "constellation".to_owned()
        )]
    );
    assert_eq!(
        hints.errors,
        [SourceError::NewcomerUnionTooLarge {
            limit: NEWCOMER_RELAY_UNION_LIMIT,
            observed: NEWCOMER_RELAY_UNION_LIMIT + 1
        }]
    );
    let relay_route = format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}");
    assert_eq!(host.requests_to(&relay_route).len(), 11);

    host.clear_requests();
    let again = block_on(engine.discover_forum_hints(&f)).unwrap();
    assert_eq!(again.newcomers, hints.newcomers);
    assert!(again.errors.is_empty(), "{:?}", again.errors);
    assert!(
        host.requests_to(&relay_route).is_empty(),
        "the relay newcomer source is disabled for the session"
    );
    // another engine is another session
    let fresh = self::engine(&host, SourceMode::Assisted);
    block_on(fresh.discover_forum_hints(&f)).unwrap();
    assert_eq!(host.requests_to(&relay_route).len(), 11);
}

/// Each source is best-effort: a failing Constellation leaves the relay's candidates, and a
/// failing relay leaves Constellation's; the relay works without any Constellation endpoint.
#[test]
fn failing_sources_do_not_discard_the_others() {
    let f = forum();
    let host = MemHost::new();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
        resp::status(503),
    );
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": [repo("did:web:relayed.example.com")]})),
    );
    let hints = block_on(engine(&host, SourceMode::Assisted).discover_forum_hints(&f)).unwrap();
    assert!(hints.recently_active.is_empty());
    assert_eq!(
        hints.newcomers,
        [("did:web:relayed.example.com".to_owned(), "relay".to_owned())]
    );
    assert_eq!(hints.errors.len(), 2, "{:?}", hints.errors);
    assert!(
        hints
            .errors
            .iter()
            .all(|e| matches!(e, SourceError::Http { status: 503, .. }))
    );

    let host = MemHost::new();
    host.on_get(&format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"), {
        let f = f.clone();
        move |_| {
            Ok(resp::ok_json(&json!({
                "records": [backlink("did:web:hinted.example.com", nsid::MEMBERSHIP, &f.fk)],
                "cursor": null
            })))
        }
    });
    host.fail(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        lessdugin_core::host::HostError::Timeout,
    );
    let hints = block_on(engine(&host, SourceMode::Assisted).discover_newcomers(&f)).unwrap();
    assert_eq!(
        hints.candidates,
        [(
            "did:web:hinted.example.com".to_owned(),
            "constellation".to_owned()
        )]
    );
    assert!(matches!(
        hints.errors[..],
        [SourceError::Transport {
            service: "relay",
            ..
        }]
    ));

    let host = MemHost::new();
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": [repo("did:web:relayed.example.com")]})),
    );
    let eps = SourceEndpoints {
        constellation: None,
        relays: vec![Url::parse(RELAY1).unwrap()],
    };
    let hints = block_on(Sources::new(host.clone(), eps).newcomer_hints(&f)).unwrap();
    assert_eq!(
        hints.candidates,
        [("did:web:relayed.example.com".to_owned(), "relay".to_owned())]
    );
    assert!(hints.errors.is_empty());

    // only `sources = direct` stops the whole discovery
    let host = MemHost::new();
    let err = block_on(engine(&host, SourceMode::Direct).discover_forum_hints(&f)).unwrap_err();
    assert_eq!(err, SourceError::DirectMode);
    assert!(host.requests().is_empty());
}

/// Listings are bounded per page and in total (security-net-5). A page with more entries than
/// the requested limit is malformed. Constellation is read newest first up to its bound, 1,000
/// membership hints or the 100 newest posts, and paging stops there; the rest is dropped with a
/// warning (`truncated`).
#[test]
fn constellation_listings_are_bounded_per_page_and_in_total() {
    let f = forum();
    let host = MemHost::new();
    let oversized: Vec<Value> = (0..=CONSTELLATION_PAGE_LIMIT)
        .map(|i| {
            backlink(
                &format!("did:web:u{i:06}.example.com"),
                nsid::MEMBERSHIP,
                &f.fk,
            )
        })
        .collect();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
        resp::ok_json(&json!({"records": oversized, "cursor": null})),
    );
    let err = block_on(service(&host).constellation_newcomers(&f)).unwrap_err();
    assert!(matches!(err, SourceError::Malformed { .. }), "{err}");

    // an endless listing of full pages, newest first
    let endless = |host: &MemHost, collection: &'static str| {
        let f = f.clone();
        host.on_get(
            &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
            move |req| {
                let page: usize = query(req, "cursor").map_or(0, |c| c.parse().unwrap());
                let records: Vec<Value> = (0..CONSTELLATION_PAGE_LIMIT)
                    .map(|i| {
                        let n = page * CONSTELLATION_PAGE_LIMIT + i;
                        backlink(&format!("did:web:u{n:06}.example.com"), collection, &f.fk)
                    })
                    .collect();
                Ok(resp::ok_json(
                    &json!({"records": records, "cursor": (page + 1).to_string()}),
                ))
            },
        );
    };
    let host = MemHost::new();
    endless(&host, nsid::MEMBERSHIP);
    let hints = block_on(service(&host).constellation_newcomers(&f)).unwrap();
    assert_eq!(hints.dids.len(), CONSTELLATION_HINTS_MAX);
    assert_eq!(hints.dids[0], "did:web:u000000.example.com", "newest first");
    assert!(hints.stats.truncated);
    assert_eq!(
        host.requests().len(),
        CONSTELLATION_HINTS_MAX / CONSTELLATION_PAGE_LIMIT,
        "paging stops at the bound"
    );

    let host = MemHost::new();
    endless(&host, nsid::POST);
    let hints = block_on(service(&host).recently_active(&f)).unwrap();
    assert_eq!(hints.dids.len(), RECENT_POSTS_DISCOVERY);
    assert!(hints.stats.truncated);
    assert_eq!(host.requests().len(), 1);

    // a listing that ends at the bound is complete, not truncated
    let host = MemHost::new();
    let records: Vec<Value> = (0..CONSTELLATION_PAGE_LIMIT)
        .map(|i| backlink(&format!("did:web:u{i:06}.example.com"), nsid::POST, "x"))
        .collect();
    host.respond_get(
        &format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"),
        resp::ok_json(&json!({"records": records, "cursor": null})),
    );
    let hints = block_on(service(&host).recently_active(&f)).unwrap();
    assert_eq!(hints.dids.len(), RECENT_POSTS_DISCOVERY);
    assert!(!hints.stats.truncated);
}

/// Relay pages are bounded by the requested limit too, and the directory stops at its bound,
/// dropping the remaining owners with a warning (security-net-5).
#[test]
fn relay_listings_are_bounded_per_page_and_in_total() {
    let host = MemHost::new();
    let oversized: Vec<Value> = (0..=RELAY_PAGE_LIMIT)
        .map(|i| repo(&format!("did:web:u{i:06}.example.com")))
        .collect();
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": oversized})),
    );
    let mut eps = endpoints();
    eps.relays.truncate(1);
    let relay = Sources::new(host.clone(), eps.clone());
    for err in [
        block_on(relay.relay_newcomers()).unwrap_err(),
        block_on(relay.relay_repositories(nsid::FORUM)).unwrap_err(),
    ] {
        assert!(matches!(err, SourceError::Malformed { .. }), "{err}");
    }

    let host = MemHost::new();
    let owners: Vec<String> = (0..DIRECTORY_OWNERS_MAX + 2 * RELAY_PAGE_LIMIT)
        .map(|i| format!("did:web:owner{i:06}.example.com"))
        .collect();
    paged_relay(&host, RELAY1, owners);
    let hints =
        block_on(Sources::new(host.clone(), endpoints()).relay_repositories(nsid::FORUM)).unwrap();
    assert_eq!(hints.dids.len(), DIRECTORY_OWNERS_MAX);
    assert!(hints.stats.truncated);
    assert_eq!(
        host.requests().len(),
        DIRECTORY_OWNERS_MAX / RELAY_PAGE_LIMIT,
        "paging stops at the bound, and the second relay is not asked"
    );
}

#[test]
fn hint_list_defaults_are_useful_public_results() {
    // Keep the result types' shapes explicit for integrators using the standalone service.
    let DidHints { dids, stats } = DidHints::default();
    assert!(dids.is_empty());
    assert_eq!(stats.requests, 0);
    let NewcomerHints {
        candidates, errors, ..
    } = NewcomerHints::default();
    assert!(candidates.is_empty() && errors.is_empty());
    let all = CorePlusHints::default();
    assert!(all.recently_active.is_empty() && all.newcomers.is_empty());
    let _ = Arc::new(all);
}
