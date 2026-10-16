// SPDX-License-Identifier: MIT OR Apache-2.0
//! `proofs/1`: verification of a single-record repo proof (`com.atproto.sync.getRecord` CAR),
//! per `docs/design/synthesis-v1.md` §3.6.
//!
//! ```text
//! verify_record_proof(car, did, coll, rkey, key, now) -> Present{rev, cid} | Absent{rev} | Err
//!   CARv1 parse with caps (1 MB, ≤ 2,000 blocks, no preallocation), every block CID-checked
//!   root[0] = commit (blessed dag-cbor): version 3, did == did, rev ≤ now + 5 min,
//!            low-S ECDSA by `key` over dag-cbor(commit without sig)
//!   MST walk from commit.data toward "coll/rkey": every node hash-checked, keys sorted,
//!            layer == leading-zero-pairs(sha256(key)), depth ≤ 32
//!   leaf → record CID, whose block must be in the CAR; or proven absence
//! ```

use ::cid::Cid;

use crate::car::{CarError, CarLimits, read_car};
use crate::commit::{Commit, CommitError};
use crate::crypto::PublicKey;
use crate::mst::{self, MstError, Proof};
use crate::syntax::{Did, Nsid, RecordKey, Tid};

/// Caps for proof verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProofCaps {
    /// CAR caps (default [`CarLimits::PROOF`]).
    pub car: CarLimits,
}

/// Proof errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// The CAR is malformed, over a cap, or has a block that does not match its CID.
    #[error("proof CAR: {0}")]
    Car(#[from] CarError),
    /// The CAR has no root.
    #[error("proof CAR has no root")]
    NoRoot,
    /// The root is not a blessed dag-cbor CID.
    #[error("proof root is not a blessed dag-cbor CID")]
    BadRoot,
    /// The commit block is missing.
    #[error("commit block {0} missing from proof")]
    MissingCommit(Cid),
    /// The commit is malformed or fails verification.
    #[error("commit: {0}")]
    Commit(#[from] CommitError),
    /// The MST walk failed (missing node, malformed node, ordering/layer/depth violation).
    #[error("MST: {0}")]
    Mst(#[from] MstError),
    /// The MST proves presence but the record block is not in the CAR.
    #[error("record block {0} missing from proof")]
    MissingRecord(Cid),
}

/// A successful verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofOutcome {
    /// The commit revision.
    pub rev: Tid,
    /// The commit CID (CAR root).
    pub commit_cid: Cid,
    /// `Some((record_cid, record_block_bytes))` if proven present (bytes hash to the CID);
    /// `None` if proven absent.
    pub record: Option<(Cid, Vec<u8>)>,
}

impl ProofOutcome {
    /// `true` if the record is proven present.
    pub fn is_present(&self) -> bool {
        self.record.is_some()
    }

    /// The record CID, if present.
    pub fn record_cid(&self) -> Option<&Cid> {
        self.record.as_ref().map(|(c, _)| c)
    }
}

/// Verifies a single-record proof CAR for `at://did/collection/rkey` against the repo signing
/// `key` (from the DID document's `#atproto` method) at time `now_micros` (µs since the Unix
/// epoch). Returns the proven presence (with record bytes) or absence.
pub fn verify_record_proof(
    car: &[u8],
    did: &Did,
    collection: &Nsid,
    rkey: &RecordKey,
    key: &PublicKey,
    now_micros: i64,
    caps: &ProofCaps,
) -> Result<ProofOutcome, ProofError> {
    let car = read_car(car, &caps.car)?;
    let commit_cid = *car.roots.first().ok_or(ProofError::NoRoot)?;
    crate::cid::check_blessed_dag_cbor(&commit_cid).map_err(|_| ProofError::BadRoot)?;
    let commit_bytes = car
        .get(&commit_cid)
        .ok_or(ProofError::MissingCommit(commit_cid))?;
    let commit = Commit::decode(commit_bytes)?;
    commit.verify(did, key, now_micros)?;
    let path = mst::repo_path(collection, rkey);
    let record = match mst::prove(&commit.data, path.as_bytes(), &car)? {
        Proof::Absent => None,
        Proof::Present(cid) => {
            // The CAR reader verified every block against its CID.
            let bytes = car.get(&cid).ok_or(ProofError::MissingRecord(cid))?;
            Some((cid, bytes.to_vec()))
        }
    };
    Ok(ProofOutcome {
        rev: commit.rev,
        commit_cid,
        record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::car::write_car;
    use crate::commit::UnsignedCommit;
    use k256::ecdsa::signature::Signer as _;
    use k256::ecdsa::{Signature, SigningKey};
    use serde_json::json;
    use std::collections::BTreeMap;

    const NOW: i64 = 1_760_000_000_000_000;

    struct Repo {
        did: Did,
        sk: SigningKey,
        records: BTreeMap<String, Vec<u8>>,
    }

    impl Repo {
        fn new() -> Self {
            let mut records = BTreeMap::new();
            for i in 0..50 {
                let v = json!({"$type": "online.7778777.lessdugin.post", "text": format!("post {i}"), "n": i});
                records.insert(
                    format!(
                        "online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2z{:03}",
                        i
                    ),
                    crate::datamodel::record_to_cbor(&v).unwrap(),
                );
            }
            let profile = json!({"$type": "online.7778777.lessdugin.profile", "displayName": "x"});
            records.insert(
                "online.7778777.lessdugin.profile/self".into(),
                crate::datamodel::record_to_cbor(&profile).unwrap(),
            );
            Self {
                did: Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap(),
                sk: SigningKey::from_slice(&[3u8; 32]).unwrap(),
                records,
            }
        }

        fn key(&self) -> PublicKey {
            PublicKey::Secp256k1(*self.sk.verifying_key())
        }

        /// A proof CAR for `path` containing exactly the commit, the path's nodes and the record.
        fn proof_car(&self, path: &str, rev_micros: i64) -> Vec<u8> {
            let entries = self
                .records
                .iter()
                .map(|(k, v)| (k.as_bytes(), crate::cid::for_dag_cbor(v)));
            let (root, nodes) = mst::build(entries).unwrap();
            let u = UnsignedCommit {
                did: self.did.clone(),
                data: root,
                rev: Tid::from_parts(rev_micros as u64, 1).unwrap(),
                prev: None,
            };
            let sig: Signature = self.sk.sign(&u.signing_bytes());
            let (ccid, cbytes) = u.signed(&sig.to_bytes());
            let mut blocks: Vec<(Cid, Vec<u8>)> = vec![(ccid, cbytes)];
            // walk the path manually to include only the needed nodes
            let mut cid = root;
            loop {
                let bytes = nodes[&cid].clone();
                blocks.push((cid, bytes.clone()));
                let n = mst::Node::decode(&bytes).unwrap();
                let keys = n.keys().unwrap();
                let idx = keys.partition_point(|k| k.as_slice() < path.as_bytes());
                if keys
                    .get(idx)
                    .is_some_and(|k| k.as_slice() == path.as_bytes())
                {
                    let rec = &self.records[path];
                    blocks.push((crate::cid::for_dag_cbor(rec), rec.clone()));
                    break;
                }
                let next = if idx == 0 {
                    n.left
                } else {
                    n.entries[idx - 1].tree
                };
                match next {
                    Some(c) => cid = c,
                    None => break,
                }
            }
            write_car(&[ccid], blocks.iter().map(|(c, b)| (c, b.as_slice())))
        }
    }

    fn coll() -> Nsid {
        Nsid::parse("online.7778777.lessdugin.post").unwrap()
    }

    #[test]
    fn end_to_end_present() {
        let repo = Repo::new();
        let rkey = RecordKey::parse("uetjixxr3a33-3l2s5xxv2z007").unwrap();
        let car = repo.proof_car(
            "online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2z007",
            NOW,
        );
        let out = verify_record_proof(
            &car,
            &repo.did,
            &coll(),
            &rkey,
            &repo.key(),
            NOW,
            &ProofCaps::default(),
        )
        .unwrap();
        assert!(out.is_present());
        let (cid, bytes) = out.record.clone().unwrap();
        assert_eq!(crate::cid::for_dag_cbor(&bytes), cid);
        let json = crate::datamodel::record_cbor_to_json(&bytes).unwrap();
        assert_eq!(json["text"], "post 7");
        assert_eq!(out.rev.timestamp_micros(), NOW as u64);
    }

    #[test]
    fn end_to_end_absent() {
        let repo = Repo::new();
        let rkey = RecordKey::parse("uetjixxr3a33-3l2s5xxv2z007x").unwrap();
        let path = "online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2z007x";
        let car = repo.proof_car(path, NOW);
        let out = verify_record_proof(
            &car,
            &repo.did,
            &coll(),
            &rkey,
            &repo.key(),
            NOW,
            &ProofCaps::default(),
        )
        .unwrap();
        assert!(!out.is_present());
        assert!(out.record_cid().is_none());
        // another collection entirely
        let other = Nsid::parse("online.7778777.lessdugin.comment").unwrap();
        let car = repo.proof_car("online.7778777.lessdugin.comment/x", NOW);
        let k = RecordKey::parse("x").unwrap();
        assert!(
            !verify_record_proof(
                &car,
                &repo.did,
                &other,
                &k,
                &repo.key(),
                NOW,
                &ProofCaps::default()
            )
            .unwrap()
            .is_present()
        );
    }

    #[test]
    fn failures() {
        let repo = Repo::new();
        let rkey = RecordKey::parse("uetjixxr3a33-3l2s5xxv2z007").unwrap();
        let path = "online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2z007";
        let car = repo.proof_car(path, NOW);
        let caps = ProofCaps::default();
        // future rev
        let future = repo.proof_car(path, NOW + 6 * 60 * 1_000_000);
        assert!(matches!(
            verify_record_proof(&future, &repo.did, &coll(), &rkey, &repo.key(), NOW, &caps),
            Err(ProofError::Commit(CommitError::FutureRev(_)))
        ));
        // wrong key / did
        let other =
            PublicKey::Secp256k1(*SigningKey::from_slice(&[4u8; 32]).unwrap().verifying_key());
        assert!(verify_record_proof(&car, &repo.did, &coll(), &rkey, &other, NOW, &caps).is_err());
        let did2 = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(verify_record_proof(&car, &did2, &coll(), &rkey, &repo.key(), NOW, &caps).is_err());
        // a key whose path nodes are not included
        let far = RecordKey::parse("uetjixxr3a33-3l2s5xxv2z049").unwrap();
        let r = verify_record_proof(&car, &repo.did, &coll(), &far, &repo.key(), NOW, &caps);
        assert!(
            matches!(r, Err(ProofError::Mst(MstError::MissingBlock(_))))
                || r.is_ok_and(|o| o.is_present())
        );
        // every single-byte change fails
        for i in 0..car.len() {
            let mut t = car.clone();
            t[i] ^= 0x01;
            assert!(
                verify_record_proof(&t, &repo.did, &coll(), &rkey, &repo.key(), NOW, &caps)
                    .is_err(),
                "byte {i}"
            );
        }
        // record block dropped
        let parsed = read_car(&car, &CarLimits::PROOF).unwrap();
        let rcid = crate::cid::for_dag_cbor(&repo.records[path]);
        let without: Vec<_> = parsed
            .blocks
            .iter()
            .filter(|(c, _)| **c != rcid)
            .map(|(c, b)| (c, b.as_slice()))
            .collect();
        let car2 = write_car(&parsed.roots, without);
        assert_eq!(
            verify_record_proof(&car2, &repo.did, &coll(), &rkey, &repo.key(), NOW, &caps),
            Err(ProofError::MissingRecord(rcid))
        );
        // no roots / commit missing
        assert_eq!(
            verify_record_proof(
                &write_car(&[], []),
                &repo.did,
                &coll(),
                &rkey,
                &repo.key(),
                NOW,
                &caps
            ),
            Err(ProofError::NoRoot)
        );
        let raw_root = crate::cid::for_raw(b"x");
        assert_eq!(
            verify_record_proof(
                &write_car(&[raw_root], []),
                &repo.did,
                &coll(),
                &rkey,
                &repo.key(),
                NOW,
                &caps
            ),
            Err(ProofError::BadRoot)
        );
        let missing = crate::cid::for_dag_cbor(b"\xa0");
        assert_eq!(
            verify_record_proof(
                &write_car(&[missing], []),
                &repo.did,
                &coll(),
                &rkey,
                &repo.key(),
                NOW,
                &caps
            ),
            Err(ProofError::MissingCommit(missing))
        );
        // CAR caps
        let tight = ProofCaps {
            car: CarLimits {
                max_blocks: 2,
                ..CarLimits::PROOF
            },
        };
        assert!(matches!(
            verify_record_proof(&car, &repo.did, &coll(), &rkey, &repo.key(), NOW, &tight),
            Err(ProofError::Car(CarError::TooManyBlocks(2)))
        ));
    }
}
