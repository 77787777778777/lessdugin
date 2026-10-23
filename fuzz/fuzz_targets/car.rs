// SPDX-License-Identifier: AGPL-3.0-or-later
//! CAR reader (`lessdugin_atp::car`) and everything that takes a CAR from the network: the
//! `proofs/1` pipeline (`proof::verify_proof`), indexer proof bundles (`proof::ProofBundle`) and
//! the MST listing of a CAR's root.
//!
//! Proofs are checked for the keys the CAR's own commit lists (so a seed that is a valid proof
//! verifies all the way, under the live proof's key or the generated repos' key, by the commit's
//! DID) and for a fixed key.
//!
//! Contract: never panics. A CAR that parses holds only hash-verified blocks within the caps it
//! was read under, and writing it back reads back the same.

#![no_main]

use lessdugin_atp::car::{Car, CarLimits, read_car, write_car};
use lessdugin_atp::commit::Commit;
use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::proof::{BundleCaps, ProofBundle, ProofCaps, verify_proof};
use lessdugin_atp::syntax::{Did, Nsid, RecordKey};
use lessdugin_atp::{cid, dagcbor, mst};
use lessdugin_fuzz::common::{COLLECTIONS, NOW};
use lessdugin_fuzz::{FUZZ_DID, FUZZ_KEY, LIVE, fnv};
use libfuzzer_sys::fuzz_target;

/// Up to 4 `(collection, rkey)` keys of the repo whose commit is the CAR's first root.
fn listed_keys(car: &Car) -> Vec<(Nsid, RecordKey)> {
    let Some(commit) = car
        .roots
        .first()
        .and_then(|r| car.get(r))
        .and_then(|b| Commit::decode(b).ok())
    else {
        return Vec::new();
    };
    let Ok(entries) = mst::list_entries(&commit.data, car, 64) else {
        return Vec::new();
    };
    entries
        .iter()
        .take(4)
        .filter_map(|(k, _)| {
            let (c, r) = std::str::from_utf8(k).ok()?.split_once('/')?;
            Some((Nsid::parse(c).ok()?, RecordKey::parse(r).ok()?))
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let mut keys = Vec::new();
    let mut signer: Option<(Did, &PublicKey)> = None;
    for limits in [CarLimits::PROOF, CarLimits::BUNDLE] {
        let Ok(car) = read_car(data, &limits) else {
            continue;
        };
        assert!(car.roots.len() <= limits.max_roots);
        assert!(car.blocks.len() <= limits.max_blocks);
        for (c, bytes) in &car.blocks {
            assert!(bytes.len() <= limits.max_block_bytes);
            assert_eq!(cid::verify(c, bytes), Ok(()), "unverified block {c}");
            let _ = dagcbor::decode(bytes);
            let _ = mst::Node::decode(bytes).map(|n| n.keys());
        }
        let again = write_car(
            &car.roots,
            car.blocks.iter().map(|(c, b)| (c, b.as_slice())),
        );
        assert_eq!(
            read_car(&again, &limits).as_ref(),
            Ok(&car),
            "CAR round trip"
        );
        for root in &car.roots {
            let _ = mst::list_entries(root, &car, 64);
        }
        if keys.is_empty() {
            keys = listed_keys(&car);
            let did = car
                .roots
                .first()
                .and_then(|r| car.get(r))
                .and_then(|b| Commit::decode(b).ok())
                .and_then(|c| Did::parse(&c.did).ok());
            signer = did.map(|d| {
                let key = if d == LIVE.0 { &LIVE.3 } else { &*FUZZ_KEY };
                (d, key)
            });
        }
    }

    // proofs/1 for the listed keys under the commit's own DID …
    if let Some((did, key)) = &signer {
        for (coll, rkey) in &keys {
            let _ = verify_proof(data, did, coll, rkey, key, NOW, &ProofCaps::default());
        }
    }
    // … and for fixed keys (the live proof's, and one of the generated repos').
    let (did, coll, rkey, key) = &*LIVE;
    let _ = verify_proof(data, did, coll, rkey, key, NOW, &ProofCaps::default());
    let coll =
        Nsid::parse(COLLECTIONS[(fnv(data) % COLLECTIONS.len() as u64) as usize]).expect("NSID");
    let rkey = RecordKey::parse("self").expect("rkey");
    let _ = verify_proof(
        data,
        &FUZZ_DID,
        &coll,
        &rkey,
        &FUZZ_KEY,
        NOW,
        &ProofCaps::default(),
    );

    if let Ok(bundle) = ProofBundle::parse(data, &BundleCaps::default()) {
        assert!(bundle.items.len() <= BundleCaps::default().max_items);
        for i in 0..=bundle.items.len() {
            let _ = bundle.verify_item(i, &FUZZ_KEY, NOW);
        }
    }
});
