// SPDX-License-Identifier: AGPL-3.0-or-later
//! `Engine::verify` (`sync.md` §8.8) over `MemHost` with a fake PDS serving real proof CARs.

mod common;

use common::*;
use lessdugin_core::dto::{VerifyOutcome, VerifyReport};
use lessdugin_core::engine::ErrorCode;
use lessdugin_core::host::{HostError, Redirects};
use lessdugin_core::verify::checks;
use lessdugin_testkit::identity::{DOH, PLC, did_doc, fake_key, install_doc, multikey};
use lessdugin_testkit::repo::{tid_at_micros, tid_at_ms};
use lessdugin_testkit::resp;
use serde_json::json;

fn names(r: &VerifyReport) -> Vec<&str> {
    r.checks.iter().map(|c| c.name.as_str()).collect()
}

fn failed_check(r: &VerifyReport) -> &str {
    let failed: Vec<&str> = r
        .checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(failed.len(), 1, "{:?}", r.checks);
    failed[0]
}

#[test]
fn a_good_proof_is_proven() {
    let w = World::new();
    let e = engine(&w.host);
    let r = block_on(e.verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Proven, "{:?}", r.checks);
    assert_eq!(r.uri, w.uri(RKEY));
    assert_eq!(r.key_source, "plc #atproto k256");
    assert_eq!(r.rev, Some(tid_at_ms(w.host.now() - 60_000).to_string()));
    assert!(r.commit_cid.as_deref().unwrap().starts_with("bafyrei"));
    assert_eq!(
        names(&r),
        vec![
            checks::DID_DOCUMENT,
            checks::PDS_ENDPOINT,
            checks::SIGNING_KEY,
            checks::PROOF_FETCHED,
            checks::CAR,
            checks::COMMIT,
            checks::REV_NOT_IN_FUTURE,
            checks::COMMIT_SIGNATURE,
            checks::MST_PATH,
            checks::RECORD_CID,
            checks::RECORD_JSON,
        ]
    );
    assert!(r.checks.iter().all(|c| c.ok));
    let cid = w
        .pds
        .update(&w.alice.did, None, |repo| repo.cid(POST, RKEY))
        .unwrap();
    let rc = r
        .checks
        .iter()
        .find(|c| c.name == checks::RECORD_CID)
        .unwrap();
    assert_eq!(rc.detail.as_deref(), Some(cid.to_string().as_str()));

    // The proof fetch: anonymous, no redirects, the proof cap and the CAR timeout.
    let proof = w
        .host
        .requests_to(&format!("{PDS}/xrpc/com.atproto.sync.getRecord"));
    assert_eq!(proof.len(), 1);
    assert_eq!(proof[0].redirects, Redirects::None);
    assert_eq!(proof[0].max_bytes, 1_000_000);
    assert_eq!(proof[0].timeout_ms, 60_000);
    assert!(proof[0].headers.is_empty());
    // The document was fetched now, even though it was cached.
    w.host.clear_requests();
    block_on(e.verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(w.host.requests_to(PLC).len(), 1);

    // DTO shape.
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["dto"], 1);
    assert_eq!(v["result"], "proven");
    assert_eq!(v["keySource"], "plc #atproto k256");
    assert!(v["commitCid"].is_string());
}

#[test]
fn an_absent_record_is_proven_absent() {
    let w = World::new();
    let r = block_on(engine(&w.host).verify(&w.uri("3l2s5xxv2zzzz"), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Absent, "{:?}", r.checks);
    assert!(r.checks.iter().all(|c| c.ok));
    assert!(!names(&r).contains(&checks::RECORD_CID));
    let mst = r
        .checks
        .iter()
        .find(|c| c.name == checks::MST_PATH)
        .unwrap();
    assert_eq!(mst.detail.as_deref(), Some("absent"));
    // A PDS that serves a record its own absence proof excludes is caught.
    w.host.respond_get(
        &format!("{PDS}/xrpc/com.atproto.repo.getRecord"),
        resp::ok_json(&json!({"uri": "x", "cid": "bafyx", "value": {"$type": POST}})),
    );
    let r = block_on(engine(&w.host).verify(&w.uri("3l2s5xxv2zzzz"), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    assert_eq!(failed_check(&r), checks::RECORD_JSON);
}

#[test]
fn a_tampered_proof_fails() {
    let w = World::new();
    let rev = tid_at_ms(w.host.now() - 60_000);
    let mut repo = w.alice.repo();
    repo.put(
        POST,
        RKEY,
        &json!({"$type": POST, "title": "hello", "n": 1}),
    );
    repo.put(
        POST,
        "3l2s5xxv2zaa3",
        &json!({"$type": POST, "title": "second"}),
    );
    let good = repo.proof_car(POST, RKEY, rev);
    let route = format!("{PDS}/xrpc/com.atproto.sync.getRecord");
    let e = engine(&w.host);
    for i in (0..good.len()).step_by(7) {
        let mut car = good.clone();
        car[i] ^= 0x20;
        w.host.respond_get(&route, resp::car(car));
        let r = block_on(e.verify(&w.uri(RKEY), false)).unwrap();
        assert_eq!(r.result, VerifyOutcome::Failed, "byte {i}: {:?}", r.checks);
        assert!(r.checks.iter().any(|c| !c.ok));
    }
    // A record swapped for another one under the same path fails the MST/record checks.
    let mut forged = repo.clone();
    forged.put(POST, RKEY, &json!({"$type": POST, "title": "forged"}));
    let honest_commit = repo.commit(rev);
    let mut blocks = forged.proof_blocks(POST, RKEY, &forged.commit(rev));
    blocks[0] = (honest_commit.cid, honest_commit.block.clone());
    let car = lessdugin_atp::car::write_car(
        &[honest_commit.cid],
        blocks.iter().map(|(c, b)| (c, b.as_slice())),
    );
    w.host.respond_get(&route, resp::car(car));
    let r = block_on(e.verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    assert_eq!(failed_check(&r), checks::MST_PATH);
}

#[test]
fn a_proof_signed_by_another_key_fails() {
    let w = World::new();
    // The DID document now names a different key (e.g. after a rotation).
    let other = fake_key("someone-else");
    let doc = did_doc(
        w.alice.did.as_str(),
        Some("alice.example.com"),
        &multikey(&other),
        PDS,
    );
    install_doc(&w.host, PLC, &w.alice.did, &doc);
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    assert_eq!(failed_check(&r), checks::COMMIT_SIGNATURE);
    let sig = r.checks.last().unwrap();
    assert!(
        sig.detail.as_deref().unwrap().starts_with("BadSignature"),
        "{sig:?}"
    );
    assert!(
        r.commit_cid.is_some(),
        "the commit CID is reported even when the proof fails"
    );
    assert_eq!(
        names(&r).last(),
        Some(&checks::COMMIT_SIGNATURE),
        "checks stop at the failure"
    );
    assert!(
        r.rev.is_some(),
        "the rev is reported even when the signature fails"
    );
}

#[test]
fn a_future_rev_fails() {
    let w = World::new();
    let future = tid_at_micros((w.host.now() + 6 * 60 * 1000) * 1000);
    w.pds.update(&w.alice.did, Some(future), |_| ());
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    assert_eq!(failed_check(&r), checks::REV_NOT_IN_FUTURE);
    let rev = r.checks.last().unwrap();
    assert!(
        rev.detail.as_deref().unwrap().starts_with("FutureRev"),
        "{rev:?}"
    );
    // Within the 5-minute skew it is fine.
    let near = tid_at_micros((w.host.now() + 4 * 60 * 1000) * 1000);
    w.pds.update(&w.alice.did, Some(near), |_| ());
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Proven);
}

#[test]
fn json_that_disagrees_with_the_signed_record_fails() {
    let w = World::new();
    w.host.respond_get(
        &format!("{PDS}/xrpc/com.atproto.repo.getRecord"),
        resp::ok_json(&json!({
            "uri": w.uri(RKEY),
            "value": {"$type": POST, "title": "edited behind the signature", "n": 1},
        })),
    );
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    assert_eq!(failed_check(&r), checks::RECORD_JSON);
    let c = r
        .checks
        .iter()
        .find(|c| c.name == checks::RECORD_JSON)
        .unwrap();
    assert!(c.detail.as_deref().unwrap().contains("hashes to"));
    // RecordNotFound for a proven record is a failure too.
    w.host.respond_get(
        &format!("{PDS}/xrpc/com.atproto.repo.getRecord"),
        resp::xrpc_error(400, "RecordNotFound", "gone"),
    );
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(failed_check(&r), checks::RECORD_JSON);
}

#[test]
fn deep_is_reported_but_does_not_decide() {
    let w = World::new();
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), true)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Proven);
    let deep = r.checks.last().unwrap();
    assert_eq!(deep.name, checks::DEEP);
    assert!(!deep.ok);
    assert_eq!(deep.detail.as_deref(), Some("not implemented in M1"));
}

#[test]
fn references_are_parsed_and_handles_resolved() {
    let w = World::new();
    let e = engine(&w.host);
    let by_handle = format!("at://alice.example.com/{POST}/{RKEY}");
    let r = block_on(e.verify(&by_handle, false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Proven);
    assert_eq!(r.uri, w.uri(RKEY), "the report names the canonical DID URI");
    let lenient = format!("  @alice.example.com/{POST}/{RKEY}/ ");
    assert_eq!(
        block_on(e.verify(&lenient, false)).unwrap().result,
        VerifyOutcome::Proven
    );
    for bad in [
        format!("at://{}/{POST}", w.alice.did),
        "nonsense".to_owned(),
        format!("at://{}", w.alice.did),
    ] {
        assert_eq!(
            block_on(e.verify(&bad, false)).unwrap_err().code,
            ErrorCode::InvalidRef,
            "{bad}"
        );
    }
    let unknown = format!("at://nobody.example.com/{POST}/{RKEY}");
    let err = block_on(direct_engine(&w.host).verify(&unknown, false)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn identity_problems_are_failed_reports() {
    // No usable key.
    let w = World::new();
    let mut doc = w.alice.doc();
    doc["verificationMethod"][0]["type"] = json!("JsonWebKey2020");
    install_doc(&w.host, PLC, &w.alice.did, &doc);
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(r.result, VerifyOutcome::Failed);
    assert_eq!(failed_check(&r), checks::SIGNING_KEY);
    assert_eq!(r.key_source, "unavailable");
    // No PDS.
    let w = World::new();
    let mut doc = w.alice.doc();
    doc["service"] = json!([]);
    install_doc(&w.host, PLC, &w.alice.did, &doc);
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(failed_check(&r), checks::PDS_ENDPOINT);
    // Unknown DID.
    let w = World::new();
    w.host
        .respond_get(&format!("{PLC}/{}", w.alice.did), resp::status(404));
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(failed_check(&r), checks::DID_DOCUMENT);
    // The repo is gone from the PDS.
    let w = World::new();
    w.host.respond_get(
        &format!("{PDS}/xrpc/com.atproto.sync.getRecord"),
        resp::xrpc_error(400, "RepoTakendown", "taken down"),
    );
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(failed_check(&r), checks::PROOF_FETCHED);
    // Unsupported DID methods are reference errors.
    let err = block_on(engine(&w.host).verify(&format!("at://did:key:zQ3sh/{POST}/{RKEY}"), false))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);
}

#[test]
fn transport_failures_are_errors() {
    let w = World::new();
    w.host.fail(
        &format!("{PDS}/xrpc/com.atproto.sync.getRecord"),
        HostError::Network("reset".into()),
    );
    let err = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    assert!(err.retryable);
    // After the retries.
    assert_eq!(
        w.host
            .requests_to(&format!("{PDS}/xrpc/com.atproto.sync.getRecord"))
            .len(),
        4
    );
    // An oversized proof is a failed report (not retried).
    let w = World::new();
    w.host.respond_get(
        &format!("{PDS}/xrpc/com.atproto.sync.getRecord"),
        resp::car(vec![0; 1_000_001]),
    );
    let r = block_on(engine(&w.host).verify(&w.uri(RKEY), false)).unwrap();
    assert_eq!(failed_check(&r), checks::PROOF_FETCHED);
}

#[test]
fn engine_futures_are_send() {
    let w = World::new();
    let e = engine(&w.host);
    let uri = w.uri(RKEY);
    let f = e.verify(&uri, false);
    assert_send(&f);
    let g = e.resolve_actor("alice.example.com");
    assert_send(&g);
    let h = e.accounts();
    assert_send(&h);
    let i = e.add_account(lessdugin_core::engine::AccountInfo::app_password(
        "did:plc:x",
        PDS,
    ));
    assert_send(&i);
    let _ = DOH;
}
