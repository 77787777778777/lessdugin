// SPDX-License-Identifier: AGPL-3.0-or-later

use std::sync::Arc;

use futures::executor::block_on;
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_core::discovery::sources::{
    CorePlusHints, DidHints, GET_BACKLINKS, GET_BACKLINKS_COUNT, LIST_REPOS_BY_COLLECTION,
    NEWCOMER_RELAY_UNION_LIMIT, NewcomerHints, SourceEndpoints, SourceError, Sources,
};
use lessdugin_core::discovery::state::{DiscState, disc_rows};
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::engine::{Engine, EngineConfig, Sources as SourceMode};
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
    host.on_get(&format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}"), |req| {
        let did = query(req, "did").expect("relay candidate DID filter");
        assert_eq!(
            query(req, "source").as_deref(),
            Some(format!("{}:forum", nsid::MEMBERSHIP).as_str())
        );
        Ok(resp::ok_json(&json!({
            "records": [backlink(&did, nsid::MEMBERSHIP, &forum().fk)],
            "cursor": null
        })))
    });
    let hints = block_on(service(&host).relay_newcomers(&forum())).unwrap();
    assert_eq!(
        hints.dids,
        [
            "did:web:a.example.com",
            "did:web:b.example.com",
            "did:web:c.example.com",
            "did:web:d.example.com"
        ]
    );
    assert_eq!(hints.stats.pages, 7);
    assert_eq!(hints.stats.returned, 10);
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
    let err = block_on(Sources::new(host.clone(), eps).relay_newcomers(&forum())).unwrap_err();
    assert!(matches!(
        err,
        SourceError::RepeatedCursor {
            service: "relay",
            page: 2
        }
    ));
}

#[test]
fn newcomer_cap_applies_to_the_union_across_relays() {
    let host = MemHost::new();
    let first: Vec<Value> = (0..NEWCOMER_RELAY_UNION_LIMIT)
        .map(|i| repo(&format!("did:web:user{i:04}.example.com")))
        .collect();
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": first})),
    );
    host.respond_get(
        &format!("{RELAY2}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": [repo("did:web:overflow.example.com")]})),
    );
    let err = block_on(service(&host).relay_newcomers(&forum())).unwrap_err();
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
    let owners: Vec<Value> = (0..=NEWCOMER_RELAY_UNION_LIMIT)
        .map(|i| repo(&format!("did:web:owner{i:04}.example.com")))
        .collect();
    host.respond_get(
        &format!("{RELAY1}/xrpc/{LIST_REPOS_BY_COLLECTION}"),
        resp::ok_json(&json!({"repos": owners})),
    );
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
        let err = block_on(Sources::new(host.clone(), eps).relay_newcomers(&forum())).unwrap_err();
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

#[test]
fn engine_sync_forum_assisted_scopes_newcomers_and_direct_uses_no_core_plus() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let selected = net.add_account("selected.example.com");
    let unrelated = net.add_account("unrelated.example.com");
    let key = ForumKey::new(owner.did(), "open").unwrap();
    let at = Datetime::from_millis(host.now() - 60_000).unwrap();
    let declaration = Forum::create("open", at.clone(), MembershipMode::Open);
    owner.put(
        nsid::FORUM,
        "open",
        &serde_json::to_value(declaration).unwrap(),
    );
    for account in [&owner, &selected, &unrelated] {
        let membership = Membership::new(key.uri.clone(), at.clone());
        account.put(
            nsid::MEMBERSHIP,
            &key.fk,
            &serde_json::to_value(membership).unwrap(),
        );
    }

    // Both newcomers are present in every relay's global membership directory. Constellation's
    // exact, DID-filtered backlink response selects only one for this forum.
    let backlink_route = format!("{CONSTELLATION}/xrpc/{GET_BACKLINKS}");
    host.on_get(&backlink_route, {
        let key = key.clone();
        let selected = selected.did().to_string();
        move |req| {
            assert_eq!(query(req, "subject").as_deref(), Some(key.uri.as_str()));
            let source = query(req, "source").unwrap();
            if source == format!("{}:forum", nsid::POST) {
                assert_eq!(query(req, "did"), None);
                return Ok(resp::ok_json(&json!({"records": [], "cursor": null})));
            }
            assert_eq!(source, format!("{}:forum", nsid::MEMBERSHIP));
            let records = match query(req, "did") {
                Some(did) if did == selected => {
                    vec![backlink(&did, nsid::MEMBERSHIP, &key.fk)]
                }
                _ => vec![],
            };
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
    let disc = block_on(disc_rows(host.cache(), &key.fk)).unwrap();
    assert_eq!(
        disc.get(selected.did().as_str()).map(|row| row.state),
        Some(DiscState::Confirmed)
    );
    assert!(!disc.contains_key(unrelated.did().as_str()));
    let membership_uri = AtUri::parse(&selected.uri(nsid::MEMBERSHIP, &key.fk)).unwrap();
    assert!(
        block_on(RecordStore::new(host.cache()).get(&membership_uri))
            .unwrap()
            .is_some(),
        "the selected newcomer's valid PDS membership was confirmed and stored"
    );
    assert!(
        net.requests()
            .iter()
            .all(|request| request.did.as_deref() != Some(unrelated.did().as_str())),
        "an unselected relay repository must not cause unrelated PDS reads"
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

#[test]
fn hint_list_defaults_are_useful_public_results() {
    // Keep the result types' shapes explicit for integrators using the standalone service.
    let DidHints { dids, stats } = DidHints::default();
    assert!(dids.is_empty());
    assert_eq!(stats.requests, 0);
    let NewcomerHints { candidates, .. } = NewcomerHints::default();
    assert!(candidates.is_empty());
    let all = CorePlusHints::default();
    assert!(all.recently_active.is_empty() && all.newcomers.is_empty());
    let _ = Arc::new(all);
}
