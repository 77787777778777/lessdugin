// SPDX-License-Identifier: MIT OR Apache-2.0
//! A real `com.atproto.sync.getRecord` proof captured from the network (`testdata/interop/live`).

mod common;

use lessdugin_atp::commit::{Commit, CommitError, MAX_FUTURE_SKEW_MICROS};
use lessdugin_atp::did_doc::DidDocument;
use lessdugin_atp::proof::{ProofCaps, ProofError, verify_record_proof};
use lessdugin_atp::syntax::{Did, Nsid, RecordKey};
use lessdugin_atp::{car, datamodel};

const DID: &str = "did:plc:z72i7hdynmk6r22z27h6tvur";

struct Fixture {
    did: Did,
    doc: DidDocument,
    present: Vec<u8>,
    absent: Vec<u8>,
    record: serde_json::Value,
    now: i64,
}

fn fixture() -> Fixture {
    let did = Did::parse(DID).unwrap();
    let doc = DidDocument::parse_for(&common::read("live/bsky.app-did.json"), &did).unwrap();
    let present = common::read("live/bsky.app-profile-self.car");
    let absent = common::read("live/bsky.app-profile-absent.car");
    let record = common::json("live/bsky.app-profile-self.record.json");
    // "now" = one minute after the captured commit's rev (deterministic).
    let parsed = car::read_car(&present, &car::CarLimits::PROOF).unwrap();
    let commit = Commit::decode(parsed.get(&parsed.roots[0]).unwrap()).unwrap();
    let now = commit.rev.timestamp_micros() as i64 + 60_000_000;
    Fixture {
        did,
        doc,
        present,
        absent,
        record,
        now,
    }
}

fn coll() -> Nsid {
    Nsid::parse("app.bsky.actor.profile").unwrap()
}

#[test]
fn did_document() {
    let f = fixture();
    let id = f.doc.atproto();
    assert_eq!(id.handle.unwrap().as_str(), "bsky.app");
    assert_eq!(
        id.pds.unwrap(),
        "https://puffball.us-east.host.bsky.network"
    );
    assert_eq!(
        id.signing_key.unwrap().multikey(),
        "zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA"
    );
}

#[test]
fn present_proof_verifies() {
    let f = fixture();
    let key = f.doc.signing_key().unwrap();
    let rkey = RecordKey::parse("self").unwrap();
    let out = verify_record_proof(
        &f.present,
        &f.did,
        &coll(),
        &rkey,
        &key,
        f.now,
        &ProofCaps::default(),
    )
    .unwrap();
    assert_eq!(out.rev.as_str(), "3mxxmh55uhk2l");
    let (cid, bytes) = out.record.expect("present");
    assert_eq!(cid.to_string(), f.record["cid"].as_str().unwrap());
    assert_eq!(
        datamodel::record_cbor_to_json(&bytes).unwrap(),
        f.record["value"]
    );
    assert_eq!(datamodel::record_cid(&f.record["value"]).unwrap(), cid);
    assert_eq!(
        datamodel::record_to_cbor(&f.record["value"]).unwrap(),
        bytes
    );
}

#[test]
fn absent_proof_verifies() {
    let f = fixture();
    let key = f.doc.signing_key().unwrap();
    let rkey = RecordKey::parse("lessdugin-absent").unwrap();
    let out = verify_record_proof(
        &f.absent,
        &f.did,
        &coll(),
        &rkey,
        &key,
        f.now,
        &ProofCaps::default(),
    )
    .unwrap();
    assert!(out.record.is_none());
    assert_eq!(out.rev.as_str(), "3mxxmh55uhk2l");
    // the presence CAR cannot be used to claim absence of `self`, nor vice versa
    let selfk = RecordKey::parse("self").unwrap();
    let r = verify_record_proof(
        &f.absent,
        &f.did,
        &coll(),
        &selfk,
        &key,
        f.now,
        &ProofCaps::default(),
    );
    assert!(!matches!(r, Ok(ref o) if o.record.is_none()), "{r:?}");
}

#[test]
fn rejects_future_rev_wrong_did_wrong_key() {
    let f = fixture();
    let key = f.doc.signing_key().unwrap();
    let rkey = RecordKey::parse("self").unwrap();
    let caps = ProofCaps::default();
    let early = f.now - 60_000_000 - MAX_FUTURE_SKEW_MICROS - 1;
    assert!(matches!(
        verify_record_proof(&f.present, &f.did, &coll(), &rkey, &key, early, &caps),
        Err(ProofError::Commit(CommitError::FutureRev(_)))
    ));
    let other = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
    assert!(matches!(
        verify_record_proof(&f.present, &other, &coll(), &rkey, &key, f.now, &caps),
        Err(ProofError::Commit(CommitError::DidMismatch { .. }))
    ));
    let wrong = lessdugin_atp::crypto::PublicKey::from_did_key(
        "did:key:zQ3shqwJEJyMBsBXCWyCBpUBMqxcon9oHB7mCvx4sSpMdLJwc",
    )
    .unwrap();
    assert!(matches!(
        verify_record_proof(&f.present, &f.did, &coll(), &rkey, &wrong, f.now, &caps),
        Err(ProofError::Commit(CommitError::Signature(_)))
    ));
}

#[test]
fn every_one_byte_tamper_fails() {
    let f = fixture();
    let key = f.doc.signing_key().unwrap();
    let caps = ProofCaps::default();
    for (car, rkey) in [(&f.present, "self"), (&f.absent, "lessdugin-absent")] {
        let rkey = RecordKey::parse(rkey).unwrap();
        assert!(verify_record_proof(car, &f.did, &coll(), &rkey, &key, f.now, &caps).is_ok());
        for i in 0..car.len() {
            for flip in [0x01u8, 0x80] {
                let mut t = car.clone();
                t[i] ^= flip;
                let r = verify_record_proof(&t, &f.did, &coll(), &rkey, &key, f.now, &caps);
                assert!(r.is_err(), "byte {i} ^ {flip:#x} still verifies: {r:?}");
            }
        }
    }
}
