// SPDX-License-Identifier: AGPL-3.0-or-later
//! Identity resolution over `MemHost` (`sync.md` §3): DoH parsing and fallback, well-known,
//! bidirectional verification, did:web policy, caching TTLs and invalidation, devnet mode,
//! system DNS and Slingshot hints.

mod common;

use common::*;
use lessdugin_core::dto::HandleState;
use lessdugin_core::engine::{Engine, EngineConfig, ErrorCode};
use lessdugin_core::host::{HostError, Redirects};
use lessdugin_core::identity::{
    DID_DOC_TTL_MS, DnsMode, DocSource, HANDLE_TTL_MS, HandleResolution, IdentityLevel,
    ResolutionMethod,
};
use lessdugin_testkit::identity::{
    DOH, DOH2, PLC, did_doc, install_doc, install_nxdomain, install_txt, install_wellknown,
};
use lessdugin_testkit::{FakeIdentity, FakePds, MemHost, resp};
use serde_json::json;

#[test]
fn doh_handle_is_verified_both_ways() {
    let host = MemHost::new();
    let alice = alice();
    alice.install(&host, PLC, DOH);
    let e = engine(&host);

    let a = block_on(e.resolve_actor("alice.example.com")).unwrap();
    assert_eq!(a.did, alice.did.as_str());
    assert_eq!(a.handle.as_deref(), Some("alice.example.com"));
    assert_eq!(a.handle_state, HandleState::Verified);
    assert_eq!(
        a.short_did,
        format!("did:plc:{}…", &alice.did.as_str()[8..16])
    );

    // One DoH query (the bidirectional check hits the cache), one PLC fetch.
    let doh = host.requests_to(DOH);
    assert_eq!(doh.len(), 1);
    assert_eq!(
        doh[0].url,
        "https://cloudflare-dns.com/dns-query?name=_atproto.alice.example.com&type=TXT"
    );
    // Only the CORS-safelisted accept header; no redirects for DoH.
    assert_eq!(
        doh[0].headers,
        vec![("accept".to_owned(), "application/dns-json".to_owned())]
    );
    assert_eq!(doh[0].redirects, Redirects::None);
    let plc = host.requests_to(PLC);
    assert_eq!(plc.len(), 1);
    assert_eq!(plc[0].url, format!("https://plc.directory/{}", alice.did));
    assert!(
        plc[0].headers.is_empty(),
        "anonymous GETs carry no custom headers"
    );
    assert_eq!(plc[0].redirects, Redirects::Follow(3));
    assert_eq!(plc[0].max_bytes, 65_536);
    assert!(host.unmatched().is_empty());

    // Other input forms resolve to the same actor from the cache.
    host.clear_requests();
    for input in [
        alice.did.as_str(),
        "@alice.example.com",
        "at://Alice.Example.COM",
        " alice.example.com/ ",
    ] {
        let b = block_on(e.resolve_actor(input)).unwrap();
        assert_eq!(b, a, "{input}");
    }
    assert!(host.requests().is_empty(), "everything was cached");

    let detail = block_on(e.resolve_identity("alice.example.com")).unwrap();
    assert_eq!(detail.identity.pds.as_deref(), Some(PDS));
    assert_eq!(
        detail.identity.signing_key.as_deref(),
        Some(alice.multikey().as_str())
    );
    assert_eq!(detail.identity.handle_method, Some(ResolutionMethod::Doh));
    assert_eq!(detail.identity.level, IdentityLevel::Doc);
    assert_eq!(detail.identity.source, DocSource::Plc);
    assert!(!detail.identity.dns_unauthenticated);
    assert!(matches!(
        detail.resolution,
        Some(HandleResolution::Did {
            method: ResolutionMethod::Doh,
            ..
        })
    ));
}

#[test]
fn doh_falls_back_to_the_next_provider() {
    let host = MemHost::new();
    let alice = alice();
    alice.install(&host, PLC, DOH2);
    host.respond_get(DOH, resp::status(503));
    let e = engine(&host);
    let a = block_on(e.resolve_actor("alice.example.com")).unwrap();
    assert_eq!(a.handle_state, HandleState::Verified);
    // The failing provider was retried 3 times with full-jitter backoff, then the next answered.
    assert_eq!(host.requests_to(DOH).len(), 4);
    assert_eq!(host.requests_to(DOH2).len(), 1);
    let sleeps = host.sleeps();
    assert_eq!(sleeps.len(), 3);
    for (i, s) in sleeps.iter().enumerate() {
        assert!(*s <= 250 << i, "sleep {i} = {s}");
    }
}

#[test]
fn a_bad_doh_answer_is_a_transport_failure() {
    let host = MemHost::new();
    let alice = alice();
    alice.install(&host, PLC, DOH2);
    // SERVFAIL from the first provider: try the next one.
    host.respond_get(DOH, resp::ok_json(&json!({"Status": 2})));
    let a = block_on(engine(&host).resolve_actor("alice.example.com")).unwrap();
    assert_eq!(a.handle_state, HandleState::Verified);
    assert_eq!(
        host.requests_to(DOH).len(),
        1,
        "a DNS error status is not retried"
    );
}

#[test]
fn several_dids_make_the_handle_invalid() {
    let host = MemHost::new();
    let alice = alice();
    let bob = bob();
    alice.install(&host, PLC, DOH);
    install_txt(
        &host,
        DOH,
        "alice.example.com",
        &[
            &format!("did={}", alice.did),
            &format!("did={}", bob.did),
            "unrelated",
        ],
    );
    let e = engine(&host);
    let err = block_on(e.resolve_actor("alice.example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);
    let a = block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert_eq!(a.handle_state, HandleState::Invalid);
    assert_eq!(a.handle, None);
}

#[test]
fn wellknown_answers_after_an_empty_dns_answer() {
    let host = MemHost::new();
    let alice = alice();
    install_doc(&host, PLC, &alice.did, &alice.doc());
    install_nxdomain(&host, DOH, "alice.example.com");
    install_wellknown(&host, "alice.example.com", &format!("  {}\n", alice.did));
    let e = engine(&host);
    let r = block_on(e.resolve_identity("alice.example.com")).unwrap();
    assert_eq!(r.identity.handle_state, HandleState::Verified);
    assert_eq!(r.identity.handle_method, Some(ResolutionMethod::Wellknown));
    // NXDOMAIN is authoritative: the second provider is not asked.
    assert!(host.requests_to(DOH2).is_empty());
    let wk = host.requests_to("https://alice.example.com/.well-known/atproto-did");
    assert_eq!(wk.len(), 1);
    assert_eq!(wk[0].redirects, Redirects::Follow(3));
    assert_eq!(wk[0].max_bytes, 2_048);
    // A well-known body that is not a DID is no answer.
    let host = MemHost::new();
    install_nxdomain(&host, DOH, "alice.example.com");
    install_wellknown(&host, "alice.example.com", "<html>hi</html>");
    let err = block_on(engine(&host).resolve_actor("alice.example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn the_claimed_handle_must_resolve_back() {
    let host = MemHost::new();
    let alice = alice();
    alice.install(&host, PLC, DOH);
    // mallory claims alice's handle, which resolves to alice's DID.
    let mallory = FakeIdentity::plc("mallory", "alice.example.com", PDS);
    install_doc(&host, PLC, &mallory.did, &mallory.doc());
    // and owns evil.example.com, pointing at mallory.
    install_txt(
        &host,
        DOH,
        "evil.example.com",
        &[&format!("did={}", mallory.did)],
    );
    let e = engine(&host);

    let m = block_on(e.resolve_identity(mallory.did.as_str())).unwrap();
    assert_eq!(m.identity.handle_state, HandleState::Unverified);
    assert_eq!(m.identity.handle(), None);
    assert_eq!(
        m.identity.claimed_handle.as_deref(),
        Some("alice.example.com")
    );
    assert_eq!(m.identity.actor_view().handle, None);

    // Resolving evil.example.com finds mallory, whose document does not claim it.
    let v = block_on(e.resolve_actor("evil.example.com")).unwrap();
    assert_eq!(v.did, mallory.did.as_str());
    assert_eq!(v.handle, None);
    assert_eq!(v.handle_state, HandleState::Unverified);

    // A claimed handle with invalid syntax is `invalid`; no claim is `unverified`.
    let odd = FakeIdentity::plc("odd", "x", PDS);
    install_doc(
        &host,
        PLC,
        &odd.did,
        &did_doc(
            odd.did.as_str(),
            Some("not_a_handle!"),
            &odd.multikey(),
            PDS,
        ),
    );
    assert_eq!(
        block_on(e.resolve_actor(odd.did.as_str()))
            .unwrap()
            .handle_state,
        HandleState::Invalid
    );
    let none = FakeIdentity::plc("none", "x", PDS);
    install_doc(
        &host,
        PLC,
        &none.did,
        &did_doc(none.did.as_str(), None, &none.multikey(), PDS),
    );
    assert_eq!(
        block_on(e.resolve_actor(none.did.as_str()))
            .unwrap()
            .handle_state,
        HandleState::Unverified
    );
}

#[test]
fn did_web_policy() {
    let host = MemHost::new();
    let e = engine(&host);
    for bad in [
        "did:web:example.com%3A8080",
        "did:web:example.com:user:alice",
        "did:web:Example.com",
        "did:key:zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF",
        "did:plc:short",
    ] {
        let err = block_on(e.resolve_actor(bad)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRef, "{bad}");
    }
    assert!(
        host.requests().is_empty(),
        "rejected DIDs cause no requests"
    );

    let web = FakeIdentity::web("example.com", "example.com", PDS);
    web.install(&host, PLC, DOH);
    let a = block_on(e.resolve_identity("did:web:example.com")).unwrap();
    assert_eq!(a.identity.source, DocSource::Web);
    assert_eq!(a.identity.handle_state, HandleState::Verified);
    let reqs = host.requests_to("https://example.com/.well-known/did.json");
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].redirects, Redirects::Follow(3));
}

#[test]
fn document_rules_apply() {
    let host = MemHost::new();
    let alice = alice();
    // A document for another DID is rejected.
    let bob = bob();
    install_doc(&host, PLC, &alice.did, &bob.doc());
    let err = block_on(engine(&host).resolve_actor(alice.did.as_str())).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // An http PDS is unusable in production (but the identity still resolves).
    let host = MemHost::new();
    install_doc(
        &host,
        PLC,
        &alice.did,
        &did_doc(
            alice.did.as_str(),
            None,
            &alice.multikey(),
            "http://pds.example.com",
        ),
    );
    let r = block_on(engine(&host).resolve_identity(alice.did.as_str())).unwrap();
    assert_eq!(r.identity.pds, None);
    assert!(r.identity.pds_problem.as_deref().unwrap().contains("https"));
    // An unknown DID is not found.
    let host = MemHost::new();
    host.respond_get(
        PLC,
        resp::json(404, &json!({"message": "DID not registered"})),
    );
    let err = block_on(engine(&host).resolve_actor(alice.did.as_str())).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // A PLC outage is a retryable network error.
    let host = MemHost::new();
    host.respond_get(PLC, resp::status(502));
    let err = block_on(engine(&host).resolve_actor(alice.did.as_str())).unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    assert!(err.retryable);
    assert_eq!(host.requests_to(PLC).len(), 4);
}

#[test]
fn cache_ttls_and_invalidation() {
    let host = MemHost::new();
    let alice = alice();
    alice.install(&host, PLC, DOH);
    let e = engine(&host);
    block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert_eq!(
        (host.requests_to(PLC).len(), host.requests_to(DOH).len()),
        (1, 1)
    );

    // Within both TTLs: no requests.
    host.clear_requests();
    host.advance_ms(HANDLE_TTL_MS - 1);
    block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert!(host.requests().is_empty());

    // Handle TTL expired: the handle state is recomputed, the document is reused.
    host.advance_ms(1);
    block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert_eq!(
        (host.requests_to(PLC).len(), host.requests_to(DOH).len()),
        (0, 1)
    );

    // Document TTL expired: refetched.
    host.clear_requests();
    host.advance_ms(DID_DOC_TTL_MS);
    block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert_eq!(host.requests_to(PLC).len(), 1);

    // Explicit invalidation drops both.
    host.clear_requests();
    block_on(e.invalidate_identity(alice.did.as_str())).unwrap();
    block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert_eq!(
        (host.requests_to(PLC).len(), host.requests_to(DOH).len()),
        (1, 1)
    );

    // A handle change is picked up after invalidation.
    let moved = FakeIdentity {
        handle: Some("alice.example.org".into()),
        ..alice.clone()
    };
    install_doc(&host, PLC, &alice.did, &moved.doc());
    install_txt(
        &host,
        DOH,
        "alice.example.org",
        &[&format!("did={}", alice.did)],
    );
    block_on(e.invalidate_identity(alice.did.as_str())).unwrap();
    let a = block_on(e.resolve_actor(alice.did.as_str())).unwrap();
    assert_eq!(a.handle.as_deref(), Some("alice.example.org"));

    // The cache lives in the cache store, keyed ("id", "did"|"handle", …).
    let rows = block_on(lessdugin_core::host::KvStore::scan(
        host.cache_kv(),
        lessdugin_core::host::KeyRange::prefix(&lessdugin_core::key!("id")),
        lessdugin_core::host::Dir::Forward,
        100,
    ))
    .unwrap();
    assert!(
        rows.iter()
            .any(|(k, _)| *k == lessdugin_core::key!("id", "did", alice.did.as_str()))
    );
    assert!(
        rows.iter()
            .any(|(k, _)| *k == lessdugin_core::key!("id", "handle", "alice.example.org"))
    );
}

#[test]
fn transport_failures_are_not_cached_but_answers_are() {
    let host = MemHost::new();
    let alice = alice();
    install_doc(&host, PLC, &alice.did, &alice.doc());
    host.fail(DOH, HostError::Network("down".into()));
    host.fail(DOH2, HostError::Timeout);
    host.fail(
        "https://alice.example.com/",
        HostError::Network("down".into()),
    );
    let e = direct_engine(&host);
    // No resolver could be reached: a network error (exit 1), not "the handle does not exist".
    let err = block_on(e.resolve_actor("alice.example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Network, "{err:?}");
    assert!(err.retryable);
    let uri = format!(
        "at://alice.example.com/{}/3jzfcijpj2z2a",
        "app.bsky.feed.post"
    );
    assert_eq!(
        block_on(e.verify(&uri, false)).unwrap_err().code,
        ErrorCode::Network
    );
    // DNS comes back: the failure was not cached.
    host.clear_routes();
    alice.install(&host, PLC, DOH);
    assert!(block_on(e.resolve_actor("alice.example.com")).is_ok());

    // An authoritative "no" is cached for the handle TTL.
    let host = MemHost::new();
    install_doc(&host, PLC, &alice.did, &alice.doc());
    install_nxdomain(&host, DOH, "alice.example.com");
    host.respond_get("https://alice.example.com/", resp::status(404));
    let e = direct_engine(&host);
    assert_eq!(
        block_on(e.resolve_actor("alice.example.com"))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    install_txt(
        &host,
        DOH,
        "alice.example.com",
        &[&format!("did={}", alice.did)],
    );
    assert_eq!(
        block_on(e.resolve_actor("alice.example.com"))
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    host.advance_ms(HANDLE_TTL_MS);
    assert!(block_on(e.resolve_actor("alice.example.com")).is_ok());
}

#[test]
fn devnet_resolves_handles_through_the_pds() {
    let plc = "http://localhost:2582";
    let pds_url = "http://localhost:2583";
    let host = MemHost::new();
    let alice = FakeIdentity::plc("alice", "alice.test", pds_url);
    install_doc(&host, plc, &alice.did, &alice.doc());
    let pds = FakePds::install(&host, pds_url);
    pds.set_handle("alice.test", &alice.did);
    let cfg = EngineConfig::devnet(
        url::Url::parse(plc).unwrap(),
        url::Url::parse(pds_url).unwrap(),
    );
    let e = Engine::new(host.clone(), cfg);

    let r = block_on(e.resolve_identity("alice.test")).unwrap();
    assert_eq!(r.identity.handle_state, HandleState::Verified);
    assert_eq!(r.identity.handle_method, Some(ResolutionMethod::Devnet));
    assert_eq!(r.identity.source, DocSource::Devnet);
    assert_eq!(
        r.identity.pds.as_deref(),
        Some(pds_url),
        "http PDS allowed in devnet mode"
    );
    let rh = host.requests_to(&format!(
        "{pds_url}/xrpc/com.atproto.identity.resolveHandle"
    ));
    assert_eq!(rh.len(), 1);
    assert_eq!(
        rh[0].url,
        format!("{pds_url}/xrpc/com.atproto.identity.resolveHandle?handle=alice.test")
    );
    assert!(
        host.requests()
            .iter()
            .all(|r| r.url.starts_with("http://localhost:")),
        "no DoH, no well-known"
    );
    // An unknown handle is not found.
    assert_eq!(
        block_on(e.resolve_actor("nobody.test")).unwrap_err().code,
        ErrorCode::NotFound
    );

    // Devnet mode is never implicit: production rejects .test and http PDSes.
    let prod = engine(&host);
    assert_eq!(
        block_on(prod.resolve_actor("alice.test")).unwrap_err().code,
        ErrorCode::InvalidRef
    );
}

#[test]
fn system_dns_verdicts_are_flagged() {
    let host = MemHost::new();
    let alice = alice();
    install_doc(&host, PLC, &alice.did, &alice.doc());
    host.set_system_txt(
        "_atproto.alice.example.com",
        Ok(Some(vec![format!("did={}", alice.did)])),
    );
    let e = Engine::new(
        host.clone(),
        EngineConfig {
            dns: DnsMode::System,
            ..EngineConfig::default()
        },
    );
    let r = block_on(e.resolve_identity("alice.example.com")).unwrap();
    assert_eq!(r.identity.handle_state, HandleState::Verified);
    assert_eq!(r.identity.handle_method, Some(ResolutionMethod::System));
    assert!(r.identity.dns_unauthenticated);
    assert!(host.requests_to(DOH).is_empty());
}

fn minidoc_route(host: &MemHost, id: &FakeIdentity) {
    host.respond_get(
        &format!("{SLINGSHOT}/xrpc/com.bad-example.identity.resolveMiniDoc"),
        resp::ok_json(&json!({
            "did": id.did.as_str(),
            "handle": id.handle,
            "pds": id.pds,
            "signing_key": id.multikey(),
        })),
    );
}

#[test]
fn slingshot_is_only_a_hint() {
    let host = MemHost::new();
    let alice = alice();
    install_doc(&host, PLC, &alice.did, &alice.doc());
    install_nxdomain(&host, DOH, "alice.example.com");
    host.respond_get("https://alice.example.com/", resp::status(404));
    minidoc_route(&host, &alice);

    // assisted: the hint finds the DID, but never verifies the handle.
    let r = block_on(engine(&host).resolve_identity("alice.example.com")).unwrap();
    assert_eq!(r.identity.did, alice.did);
    assert!(matches!(
        r.resolution,
        Some(HandleResolution::Did {
            method: ResolutionMethod::Hint,
            ..
        })
    ));
    assert_eq!(r.identity.handle_state, HandleState::Unverified);
    assert_eq!(
        r.identity.level,
        IdentityLevel::Doc,
        "the document itself came from PLC"
    );

    // direct: Slingshot is never contacted.
    let host2 = MemHost::new();
    install_doc(&host2, PLC, &alice.did, &alice.doc());
    install_nxdomain(&host2, DOH, "alice.example.com");
    minidoc_route(&host2, &alice);
    let err = block_on(direct_engine(&host2).resolve_actor("alice.example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(host2.requests_to(SLINGSHOT).is_empty());
}

#[test]
fn did_web_cors_failure_falls_back_to_a_hint_identity() {
    let host = MemHost::new();
    let web = FakeIdentity::web("example.com", "example.com", PDS);
    host.fail("https://example.com/.well-known/did.json", HostError::Cors);
    minidoc_route(&host, &web);
    let r = block_on(engine(&host).resolve_identity("did:web:example.com")).unwrap();
    assert_eq!(r.identity.level, IdentityLevel::Hint);
    assert_eq!(r.identity.source, DocSource::Slingshot);
    assert_eq!(r.identity.pds.as_deref(), Some(PDS));
    // Not in direct mode.
    let host = MemHost::new();
    host.fail("https://example.com/.well-known/did.json", HostError::Cors);
    let err = block_on(direct_engine(&host).resolve_actor("did:web:example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Blocked);
}

#[test]
fn bad_inputs() {
    let host = MemHost::new();
    let e = engine(&host);
    for bad in [
        "",
        "   ",
        "@",
        "not a handle",
        "alice",
        "at://alice.example.com/app.bsky.feed.post/x",
        "alice.local",
        "handle.invalid",
    ] {
        let err = block_on(e.resolve_actor(bad)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRef, "{bad:?}");
    }
    assert!(host.requests().is_empty());
}

#[test]
fn an_unreachable_host_after_nxdomain_is_not_found() {
    // DNS answered authoritatively (NXDOMAIN); only the handle's own host is unreachable, which is
    // what a mistyped handle looks like: not found, not a network error.
    let host = MemHost::new();
    install_nxdomain(&host, DOH, "nobody.example.com");
    host.fail(
        "https://nobody.example.com/",
        HostError::Network("no such host".into()),
    );
    let err = block_on(direct_engine(&host).resolve_actor("nobody.example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
    // DNS down, but the host answered 404: a definitive no.
    let host = MemHost::new();
    host.fail(DOH, HostError::Network("down".into()));
    host.fail(DOH2, HostError::Network("down".into()));
    host.respond_get("https://nobody.example.com/", resp::status(404));
    let err = block_on(direct_engine(&host).resolve_actor("nobody.example.com")).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
}

#[test]
fn an_unreachable_devnet_pds_is_a_network_error() {
    let host = MemHost::new();
    host.fail(
        "http://localhost:2583/",
        HostError::Network("refused".into()),
    );
    let cfg = EngineConfig::devnet(
        url::Url::parse("http://localhost:2582").unwrap(),
        url::Url::parse("http://localhost:2583").unwrap(),
    );
    let err = block_on(Engine::new(host.clone(), cfg).resolve_actor("alice.test")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Network, "{err:?}");
}

/// A lying devnet (PLC + PDS) and the public network over one shared cache: devnet answers are
/// never used outside devnet mode, and production answers are never used in devnet mode.
#[test]
fn devnet_and_production_cache_rows_never_mix() {
    let (dev_plc, dev_pds) = ("http://localhost:2582", "http://localhost:2583");
    let host = MemHost::new();
    let dev = Engine::new(
        host.clone(),
        EngineConfig::devnet(
            url::Url::parse(dev_plc).unwrap(),
            url::Url::parse(dev_pds).unwrap(),
        ),
    );
    let prod = engine(&host);

    // The real alice.example.com and the attacker's identity, which claims that handle.
    let alice = alice();
    alice.install(&host, PLC, DOH);
    let mallory = FakeIdentity::plc("mallory", "alice.example.com", "https://evil.example.com");
    install_doc(&host, PLC, &mallory.did, &mallory.doc());
    // The devnet PDS says alice.example.com is mallory; the devnet PLC serves a document for
    // alice's real DID with the attacker's PDS.
    let pds = FakePds::install(&host, dev_pds);
    pds.set_handle("alice.example.com", &mallory.did);
    install_doc(&host, dev_plc, &mallory.did, &mallory.doc());
    let forged = FakeIdentity {
        pds: "http://localhost:6666".into(),
        ..alice.clone()
    };
    install_doc(&host, dev_plc, &alice.did, &forged.doc());

    let d = block_on(dev.resolve_identity("alice.example.com")).unwrap();
    assert_eq!(d.identity.did, mallory.did);
    assert_eq!(d.identity.handle_state, HandleState::Verified, "in devnet");
    let d = block_on(dev.resolve_identity(alice.did.as_str())).unwrap();
    assert_eq!(d.identity.pds.as_deref(), Some("http://localhost:6666"));

    // Production ignores both devnet rows: DoH and the real PLC are asked again.
    host.clear_requests();
    let p = block_on(prod.resolve_identity("alice.example.com")).unwrap();
    assert_eq!(p.identity.did, alice.did);
    assert_eq!(p.identity.handle_state, HandleState::Verified);
    assert_eq!(p.identity.pds.as_deref(), Some(PDS));
    assert_eq!(p.identity.source, DocSource::Plc);
    assert!(!host.requests_to(DOH).is_empty());
    assert_eq!(
        host.requests_to(&format!("{PLC}/{}", alice.did)).len(),
        1,
        "the devnet document was not used"
    );
    let m = block_on(prod.resolve_actor(mallory.did.as_str())).unwrap();
    assert_eq!(
        m.handle, None,
        "mallory's claim is not verified in production"
    );

    // And the other way round: devnet ignores the production rows just written.
    host.clear_requests();
    let d = block_on(dev.resolve_identity(alice.did.as_str())).unwrap();
    assert_eq!(d.identity.pds.as_deref(), Some("http://localhost:6666"));
    assert_eq!(d.identity.source, DocSource::Devnet);
    assert_eq!(host.requests_to(dev_plc).len(), 1);
    assert!(host.requests_to(PLC).is_empty() && host.requests_to(DOH).is_empty());
}

#[test]
fn did_web_follows_the_mode() {
    // devnet: did:web under .test resolves (https, as always for did:web).
    let host = MemHost::new();
    let web = FakeIdentity::web("alice.test", "alice.test", "http://localhost:2583");
    host.respond_get(
        "https://alice.test/.well-known/did.json",
        resp::ok_json(&web.doc()),
    );
    let cfg = EngineConfig::devnet(
        url::Url::parse("http://localhost:2582").unwrap(),
        url::Url::parse("http://localhost:2583").unwrap(),
    );
    let r =
        block_on(Engine::new(host.clone(), cfg).resolve_identity("did:web:alice.test")).unwrap();
    assert_eq!(r.identity.pds.as_deref(), Some("http://localhost:2583"));
    assert_eq!(r.identity.source, DocSource::Devnet);
    // production: refused before any request, with the real reason.
    let host = MemHost::new();
    for did in ["did:web:alice.test", "did:web:pds.local"] {
        let err = block_on(engine(&host).resolve_actor(did)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRef, "{did}");
        assert!(err.message.contains("top-level domain"), "{}", err.message);
        assert!(!err.message.contains("with a port"), "{}", err.message);
    }
    assert!(host.requests().is_empty());
}

#[test]
fn a_hint_identity_is_retried_after_an_hour() {
    let host = MemHost::new();
    let web = FakeIdentity::web("example.com", "example.com", PDS);
    host.fail("https://example.com/.well-known/did.json", HostError::Cors);
    minidoc_route(&host, &web);
    let e = engine(&host);
    let r = block_on(e.resolve_identity("did:web:example.com")).unwrap();
    assert_eq!(r.identity.level, IdentityLevel::Hint);
    // Reused for a while…
    host.clear_requests();
    host.advance_ms(lessdugin_core::identity::HINT_DOC_TTL_MS - 1);
    block_on(e.resolve_identity("did:web:example.com")).unwrap();
    assert!(host.requests_to("https://example.com/").is_empty());
    // …then the real document is tried again (and now it works).
    host.respond_get(
        "https://example.com/.well-known/did.json",
        resp::ok_json(&web.doc()),
    );
    install_txt(&host, DOH, "example.com", &[&format!("did={}", web.did)]);
    host.advance_ms(1);
    let r = block_on(e.resolve_identity("did:web:example.com")).unwrap();
    assert_eq!(r.identity.level, IdentityLevel::Doc);
    assert_eq!(r.identity.handle_state, HandleState::Verified);
}

#[test]
fn local_only_pds_endpoints_are_unusable_in_production() {
    let host = MemHost::new();
    let alice = alice();
    for (i, endpoint) in [
        "https://pds.local",
        "https://router.internal",
        "https://x.localhost",
        "https://pds.test",
        "https://x.home.arpa",
        "https://foo.onion",
    ]
    .into_iter()
    .enumerate()
    {
        install_doc(
            &host,
            PLC,
            &alice.did,
            &did_doc(alice.did.as_str(), None, &alice.multikey(), endpoint),
        );
        host.advance_ms(DID_DOC_TTL_MS * (i as i64 + 1));
        let r = block_on(engine(&host).resolve_identity(alice.did.as_str())).unwrap();
        assert_eq!(r.identity.pds, None, "{endpoint}");
        assert!(r.identity.pds_problem.is_some(), "{endpoint}");
    }
}

#[test]
fn fresh_resolution_bypasses_the_cache() {
    let host = MemHost::new();
    let alice = alice();
    alice.install(&host, PLC, DOH);
    let e = engine(&host);
    block_on(e.resolve_identity("alice.example.com")).unwrap();
    host.clear_requests();
    // The cached answers are used by a normal resolution…
    block_on(e.resolve_identity("alice.example.com")).unwrap();
    assert!(host.requests().is_empty());
    // …but a fresh one asks DNS and PLC again.
    let r = block_on(e.resolve_identity_fresh("alice.example.com")).unwrap();
    assert_eq!(r.identity.handle_state, HandleState::Verified);
    assert_eq!(host.requests_to(DOH).len(), 1);
    assert_eq!(host.requests_to(PLC).len(), 1);
    host.clear_requests();
    block_on(e.resolve_identity_fresh(alice.did.as_str())).unwrap();
    assert_eq!(host.requests_to(PLC).len(), 1);
}
