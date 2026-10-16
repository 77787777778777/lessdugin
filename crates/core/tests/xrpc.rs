// SPDX-License-Identifier: AGPL-3.0-or-later
//! The XRPC client over `MemHost`: error mapping, retries and backoff, caps, headers, targets.

mod common;

use common::*;
use lessdugin_atp::syntax::{AtIdentifier, Did, Nsid, RecordKey};
use lessdugin_atp::xrpc::{CommitMeta, GetLatestCommit, GetRecord};
use lessdugin_core::host::{HostError, Redirects, Target};
use lessdugin_core::xrpc::{CallOpts, RetryPolicy, Service, Xrpc, XrpcCallError};
use lessdugin_testkit::{MemHost, resp};
use serde_json::{Value, json};

const BASE: &str = "https://pds.example.com";
const NSID: &str = "com.example.thing";

fn service() -> Service {
    Service::Anon(url::Url::parse(BASE).unwrap())
}

fn did() -> Did {
    Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap()
}

fn query(host: &MemHost) -> Result<Value, XrpcCallError> {
    block_on(Xrpc::new(host).query(&service(), NSID, &[("q", "a b/c")], &CallOpts::default()))
}

#[test]
fn query_urls_are_percent_encoded() {
    let host = MemHost::new();
    host.respond_get(BASE, resp::ok_json(&json!({"ok": true})));
    assert_eq!(query(&host).unwrap(), json!({"ok": true}));
    let r = host.requests();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].url, format!("{BASE}/xrpc/{NSID}?q=a%20b%2Fc"));
    assert!(
        r[0].headers.is_empty(),
        "no custom headers on anonymous GETs"
    );
    assert_eq!(r[0].redirects, Redirects::None);
    assert!(host.sleeps().is_empty());
}

#[test]
fn server_errors_are_retried_with_backoff() {
    let host = MemHost::new();
    host.respond_seq(
        BASE,
        vec![
            Ok(resp::status(503)),
            Ok(resp::status(502)),
            Ok(resp::ok_json(&json!({"n": 1}))),
        ],
    );
    assert_eq!(query(&host).unwrap(), json!({"n": 1}));
    assert_eq!(host.requests().len(), 3);
    let s = host.sleeps();
    assert_eq!(s.len(), 2);
    assert!(s[0] <= 250 && s[1] <= 500, "{s:?}");
    // The wait happened on the host clock.
    let r = host.requests();
    assert_eq!(r[2].at_ms - r[0].at_ms, (s[0] + s[1]) as i64);
}

#[test]
fn retries_are_bounded() {
    let host = MemHost::new();
    host.fail(BASE, HostError::Network("connection reset".into()));
    assert_eq!(
        query(&host),
        Err(XrpcCallError::Network("connection reset".into()))
    );
    assert_eq!(host.requests().len(), 4, "1 attempt + 3 retries");
    let s = host.sleeps();
    assert_eq!(s.len(), 3);
    for (i, d) in s.iter().enumerate() {
        assert!(*d <= (250u64 << i).min(8_000));
    }
    // Timeouts too.
    let host = MemHost::new();
    host.fail(BASE, HostError::Timeout);
    assert_eq!(query(&host), Err(XrpcCallError::Timeout));
    assert_eq!(host.requests().len(), 4);
    // The policy is configurable.
    let host = MemHost::new();
    host.fail(BASE, HostError::Timeout);
    let x = Xrpc::new(&host).with_policy(RetryPolicy::NONE);
    let r: Result<Value, _> = block_on(x.query(&service(), NSID, &[], &CallOpts::default()));
    assert_eq!(r, Err(XrpcCallError::Timeout));
    assert_eq!(host.requests().len(), 1);
}

#[test]
fn rate_limits_honour_retry_after() {
    let host = MemHost::new();
    host.respond_seq(
        BASE,
        vec![
            Ok(resp::with_header(
                resp::xrpc_error(429, "RateLimitExceeded", "slow down"),
                "retry-after",
                "2",
            )),
            Ok(resp::ok_json(&json!({}))),
        ],
    );
    assert!(query(&host).is_ok());
    assert_eq!(
        host.sleeps(),
        vec![2_000],
        "Retry-After is a lower bound on the wait"
    );

    // HTTP-date form, relative to the host clock.
    let host = MemHost::new();
    host.set_now_ms(784_111_770_000);
    host.respond_seq(
        BASE,
        vec![
            Ok(resp::with_header(
                resp::status(429),
                "retry-after",
                "Sun, 06 Nov 1994 08:49:37 GMT",
            )),
            Ok(resp::ok_json(&json!({}))),
        ],
    );
    assert!(query(&host).is_ok());
    assert_eq!(host.sleeps(), vec![7_000]);

    // A wait longer than the in-call limit ends the call as RateLimited.
    let host = MemHost::new();
    host.respond_get(
        BASE,
        resp::with_header(resp::status(429), "retry-after", "120"),
    );
    assert_eq!(
        query(&host),
        Err(XrpcCallError::RateLimited {
            retry_after_ms: Some(120_000)
        })
    );
    assert_eq!(host.requests().len(), 1);
    assert!(host.sleeps().is_empty());

    // RateLimit-Reset when there is no Retry-After.
    let host = MemHost::new();
    host.respond_get(
        BASE,
        resp::with_header(resp::status(429), "ratelimit-reset", "300"),
    );
    assert_eq!(
        query(&host),
        Err(XrpcCallError::RateLimited {
            retry_after_ms: Some(300_000)
        })
    );
}

#[test]
fn client_errors_map_and_are_not_retried() {
    let cases = [
        (
            resp::xrpc_error(400, "RecordNotFound", "x"),
            XrpcCallError::RecordNotFound,
        ),
        (
            resp::xrpc_error(400, "InvalidSwap", "x"),
            XrpcCallError::InvalidSwap {
                message: Some("x".into()),
            },
        ),
        (
            resp::xrpc_error(400, "RepoNotFound", "x"),
            XrpcCallError::RepoNotFound,
        ),
        (
            resp::xrpc_error(400, "RepoDeactivated", "x"),
            XrpcCallError::RepoDeactivated,
        ),
        (
            resp::xrpc_error(400, "RepoTakendown", "x"),
            XrpcCallError::RepoTakendown,
        ),
        (
            resp::xrpc_error(401, "ExpiredToken", "x"),
            XrpcCallError::AuthRequired {
                error: Some("ExpiredToken".into()),
                message: Some("x".into()),
            },
        ),
        (
            resp::xrpc_error(400, "InvalidRequest", "bad"),
            XrpcCallError::Xrpc {
                status: 400,
                error: "InvalidRequest".into(),
                message: Some("bad".into()),
            },
        ),
        (resp::text(404, "not here"), XrpcCallError::Http(404)),
        (
            resp::with_header(resp::status(302), "location", "https://elsewhere.example/"),
            XrpcCallError::Http(302),
        ),
    ];
    for (response, expected) in cases {
        let host = MemHost::new();
        host.respond_get(BASE, response);
        assert_eq!(query(&host), Err(expected.clone()));
        assert_eq!(host.requests().len(), 1, "{expected:?} must not be retried");
    }
}

#[test]
fn caps_are_enforced_and_too_large_is_final() {
    let host = MemHost::new();
    host.respond_get(BASE, resp::ok_json(&json!({"big": "x".repeat(2_000)})));
    let r: Result<Value, _> =
        block_on(Xrpc::new(&host).query(&service(), NSID, &[], &CallOpts::capped(1_000)));
    assert_eq!(r, Err(XrpcCallError::TooLarge));
    assert_eq!(host.requests().len(), 1);
    assert_eq!(host.requests()[0].max_bytes, 1_000);
    // A 2xx body that is not the expected JSON.
    let host = MemHost::new();
    host.respond_get(BASE, resp::text(200, "<html>"));
    assert!(matches!(query(&host), Err(XrpcCallError::Decode(_))));
}

#[test]
fn procedures_are_never_retried() {
    let host = MemHost::new();
    host.respond(BASE, resp::status(503));
    let r: Result<Value, _> = block_on(Xrpc::new(&host).procedure(
        &service(),
        NSID,
        &json!({"a": 1}),
        &CallOpts::default(),
    ));
    assert_eq!(r, Err(XrpcCallError::Http(503)));
    let reqs = host.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, http::Method::POST);
    assert_eq!(reqs[0].header("content-type"), Some("application/json"));
    assert_eq!(reqs[0].body.as_deref(), Some(br#"{"a":1}"#.as_slice()));
    assert_eq!(reqs[0].redirects, Redirects::None);
    // An empty success body decodes as ().
    let host = MemHost::new();
    host.respond(BASE, resp::status(200));
    let r: Result<(), _> =
        block_on(Xrpc::new(&host).procedure(&service(), NSID, &json!({}), &CallOpts::default()));
    assert_eq!(r, Ok(()));
}

#[test]
fn typed_endpoints_and_user_pds_targets() {
    let host = MemHost::new();
    host.respond_get(
        &format!("userpds://{}/xrpc/com.atproto.sync.getLatestCommit", did()),
        resp::ok_json(&json!({"cid": "bafy", "rev": "3l2s5xxv2zaa2"})),
    );
    let out: CommitMeta = block_on(Xrpc::new(&host).query_endpoint(
        &Service::UserPds(did()),
        &GetLatestCommit { did: did() },
        &CallOpts::default(),
    ))
    .unwrap();
    assert_eq!(out.rev, "3l2s5xxv2zaa2");
    let r = &host.requests()[0];
    assert_eq!(
        r.target,
        Target::UserPds {
            account: did(),
            path_and_query: format!(
                "/xrpc/com.atproto.sync.getLatestCommit?did={}",
                "did%3Aplc%3Aewvi7nxzyoun6zhxrhs64oiz"
            ),
        }
    );
    // Host-side auth failures map to AuthRequired.
    let host = MemHost::new();
    host.fail("userpds://", HostError::AuthRequired(did().to_string()));
    let r = block_on(Xrpc::new(&host).query_endpoint(
        &Service::UserPds(did()),
        &GetRecord {
            repo: AtIdentifier::Did(did()),
            collection: Nsid::parse(POST).unwrap(),
            rkey: RecordKey::parse("self").unwrap(),
            cid: None,
        },
        &CallOpts::default(),
    ));
    assert!(matches!(r, Err(XrpcCallError::AuthRequired { .. })));
    assert_eq!(host.requests().len(), 1);
}

#[test]
fn jitter_is_deterministic_per_seed() {
    let run = |seed| {
        let host = MemHost::new();
        host.set_seed(seed);
        host.fail(BASE, HostError::Timeout);
        let _ = query(&host);
        host.sleeps()
    };
    assert_eq!(run(1), run(1));
    assert_ne!(run(1), run(2));
}
