// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`TestRepo`]: a small in-memory atproto repository that produces signed commits, the MST and
//! `com.atproto.sync.getRecord`-style proof CARs, using `lessdugin_atp`'s canonical MST builder
//! (`mst::build`) and commit encoder (`commit::UnsignedCommit`), signed with a k256 key.

use std::collections::BTreeMap;

use k256::ecdsa::signature::Signer as _;
use k256::ecdsa::{Signature, SigningKey};
use lessdugin_atp::Cid;
use lessdugin_atp::car::write_car;
use lessdugin_atp::commit::UnsignedCommit;
use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::mst::{self, Node};
use lessdugin_atp::syntax::{Did, Tid};
use serde_json::Value;

/// A TID for a time in microseconds (clock id 0).
pub fn tid_at_micros(micros: i64) -> Tid {
    Tid::from_parts(u64::try_from(micros).expect("non-negative time"), 0).expect("valid TID time")
}

/// A TID for a time in milliseconds (clock id 0).
pub fn tid_at_ms(ms: i64) -> Tid {
    tid_at_micros(ms * 1000)
}

/// A signed commit with its MST.
#[derive(Clone, Debug)]
pub struct SignedCommit {
    /// The commit CID (the CAR root).
    pub cid: Cid,
    /// The commit block.
    pub block: Vec<u8>,
    /// The MST root.
    pub root: Cid,
    /// Every MST node block.
    pub nodes: BTreeMap<Cid, Vec<u8>>,
    /// The revision.
    pub rev: Tid,
}

/// An in-memory repository.
#[derive(Clone)]
pub struct TestRepo {
    did: Did,
    key: SigningKey,
    /// repo path (`collection/rkey`) → record DAG-CBOR.
    records: BTreeMap<String, Vec<u8>>,
}

impl std::fmt::Debug for TestRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestRepo")
            .field("did", &self.did)
            .field("records", &self.records.len())
            .finish_non_exhaustive()
    }
}

fn path(collection: &str, rkey: &str) -> String {
    format!("{collection}/{rkey}")
}

impl TestRepo {
    /// An empty repository for `did`, signed by `key`.
    pub fn new(did: Did, key: SigningKey) -> Self {
        Self {
            did,
            key,
            records: BTreeMap::new(),
        }
    }

    /// The repo DID.
    pub fn did(&self) -> &Did {
        &self.did
    }

    /// The signing key.
    pub fn signing_key(&self) -> &SigningKey {
        &self.key
    }

    /// The public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::Secp256k1(*self.key.verifying_key())
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True if empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Creates or replaces a record from atproto JSON; returns its CID. Panics on JSON that is not
    /// a valid record or a key that is not a valid repo path (test helper).
    pub fn put(&mut self, collection: &str, rkey: &str, value: &Value) -> Cid {
        let cbor = lessdugin_atp::datamodel::record_to_cbor(value).expect("valid record JSON");
        self.put_cbor(collection, rkey, cbor)
    }

    /// Creates or replaces a record from DAG-CBOR bytes; returns its CID.
    pub fn put_cbor(&mut self, collection: &str, rkey: &str, cbor: Vec<u8>) -> Cid {
        let p = path(collection, rkey);
        mst::validate_key(p.as_bytes()).expect("valid repo path");
        let cid = lessdugin_atp::cid::for_dag_cbor(&cbor);
        self.records.insert(p, cbor);
        cid
    }

    /// Deletes a record; true if it existed.
    pub fn delete(&mut self, collection: &str, rkey: &str) -> bool {
        self.records.remove(&path(collection, rkey)).is_some()
    }

    /// A record's CID.
    pub fn cid(&self, collection: &str, rkey: &str) -> Option<Cid> {
        self.records
            .get(&path(collection, rkey))
            .map(|b| lessdugin_atp::cid::for_dag_cbor(b))
    }

    /// A record as atproto JSON.
    pub fn get(&self, collection: &str, rkey: &str) -> Option<Value> {
        let b = self.records.get(&path(collection, rkey))?;
        lessdugin_atp::datamodel::record_cbor_to_json(b).ok()
    }

    /// The record DAG-CBOR bytes.
    pub fn get_cbor(&self, collection: &str, rkey: &str) -> Option<&[u8]> {
        self.records.get(&path(collection, rkey)).map(Vec::as_slice)
    }

    /// Every `(path, cid)` in key order.
    pub fn entries(&self) -> Vec<(String, Cid)> {
        self.records
            .iter()
            .map(|(p, b)| (p.clone(), lessdugin_atp::cid::for_dag_cbor(b)))
            .collect()
    }

    /// Builds the MST and a commit at `rev`, signed by the repo key.
    pub fn commit(&self, rev: Tid) -> SignedCommit {
        self.commit_signed_by(rev, &self.key)
    }

    /// Builds the MST and a commit at `rev`, signed by `key` (for wrong-key tests).
    pub fn commit_signed_by(&self, rev: Tid, key: &SigningKey) -> SignedCommit {
        let (root, nodes) =
            mst::build(self.entries().into_iter().map(|(p, c)| (p.into_bytes(), c)))
                .expect("MST build");
        let unsigned = UnsignedCommit {
            did: self.did.clone(),
            data: root,
            rev,
            prev: None,
        };
        let sig: Signature = key.sign(&unsigned.signing_bytes());
        let (cid, block) = unsigned.signed(&sig.to_bytes());
        SignedCommit {
            cid,
            block,
            root,
            nodes,
            rev,
        }
    }

    /// The blocks proving `collection/rkey` (presence or absence) under `commit`: the commit, the
    /// MST nodes on the search path, and the record block if present.
    pub fn proof_blocks(
        &self,
        collection: &str,
        rkey: &str,
        commit: &SignedCommit,
    ) -> Vec<(Cid, Vec<u8>)> {
        let target = path(collection, rkey);
        let target = target.as_bytes();
        let mut blocks = vec![(commit.cid, commit.block.clone())];
        let mut cid = commit.root;
        loop {
            let bytes = commit.nodes.get(&cid).expect("node of this commit").clone();
            let node = Node::decode(&bytes).expect("valid node");
            let keys = node.keys().expect("valid keys");
            blocks.push((cid, bytes));
            let idx = keys.partition_point(|k| k.as_slice() < target);
            if keys.get(idx).is_some_and(|k| k.as_slice() == target) {
                let rec = self
                    .records
                    .get(std::str::from_utf8(target).expect("utf-8 path"))
                    .expect("record");
                blocks.push((lessdugin_atp::cid::for_dag_cbor(rec), rec.clone()));
                break;
            }
            let next = if idx == 0 {
                node.left
            } else {
                node.entries[idx - 1].tree
            };
            match next {
                Some(c) => cid = c,
                None => break,
            }
        }
        blocks
    }

    /// A proof CAR for `collection/rkey` at `rev` (what `com.atproto.sync.getRecord` returns).
    pub fn proof_car(&self, collection: &str, rkey: &str, rev: Tid) -> Vec<u8> {
        self.proof_car_for(collection, rkey, &self.commit(rev))
    }

    /// A proof CAR for `collection/rkey` under a given commit.
    pub fn proof_car_for(&self, collection: &str, rkey: &str, commit: &SignedCommit) -> Vec<u8> {
        let blocks = self.proof_blocks(collection, rkey, commit);
        write_car(&[commit.cid], blocks.iter().map(|(c, b)| (c, b.as_slice())))
    }

    /// A CAR of the whole repository at `rev` (commit, every node, every record).
    pub fn full_car(&self, rev: Tid) -> Vec<u8> {
        let commit = self.commit(rev);
        let mut blocks: Vec<(Cid, Vec<u8>)> = vec![(commit.cid, commit.block.clone())];
        blocks.extend(commit.nodes.iter().map(|(c, b)| (*c, b.clone())));
        blocks.extend(
            self.records
                .values()
                .map(|b| (lessdugin_atp::cid::for_dag_cbor(b), b.clone())),
        );
        write_car(&[commit.cid], blocks.iter().map(|(c, b)| (c, b.as_slice())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_atp::proof::{ProofCaps, ProofError, verify_record_proof};
    use lessdugin_atp::syntax::{Nsid, RecordKey};
    use serde_json::json;

    const NOW_US: i64 = 1_760_000_000_000_000;

    fn repo(n: usize) -> TestRepo {
        let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
        let mut r = TestRepo::new(did, SigningKey::from_slice(&[7u8; 32]).unwrap());
        for i in 0..n {
            r.put(
                "online.7778777.lessdugin.post",
                &format!("aaaaaaaaaaaa-3l2s5xxv2z{i:03}"),
                &json!({"$type": "online.7778777.lessdugin.post", "n": i}),
            );
        }
        r.put(
            "online.7778777.lessdugin.profile",
            "self",
            &json!({"$type": "online.7778777.lessdugin.profile"}),
        );
        r
    }

    fn verify(
        r: &TestRepo,
        coll: &str,
        rkey: &str,
        car: &[u8],
    ) -> Result<Option<Cid>, Box<ProofError>> {
        verify_record_proof(
            car,
            r.did(),
            &Nsid::parse(coll).unwrap(),
            &RecordKey::parse(rkey).unwrap(),
            &r.public_key(),
            NOW_US,
            &ProofCaps::default(),
        )
        .map(|o| o.record_cid().copied())
        .map_err(Box::new)
    }

    #[test]
    fn proofs_verify_with_atp() {
        for n in [0, 1, 3, 40, 200] {
            let r = repo(n);
            let rev = tid_at_micros(NOW_US);
            for i in [0, n / 2, n.saturating_sub(1)] {
                if n == 0 {
                    break;
                }
                let rkey = format!("aaaaaaaaaaaa-3l2s5xxv2z{i:03}");
                let car = r.proof_car("online.7778777.lessdugin.post", &rkey, rev);
                let cid = verify(&r, "online.7778777.lessdugin.post", &rkey, &car).unwrap();
                assert_eq!(
                    cid,
                    r.cid("online.7778777.lessdugin.post", &rkey),
                    "n={n} i={i}"
                );
            }
            // absence proofs, before, between and after existing keys
            for rkey in ["aaaaaaaaaaaa-0", "aaaaaaaaaaaa-3l2s5xxv2z000x", "zzzz"] {
                let car = r.proof_car("online.7778777.lessdugin.post", rkey, rev);
                assert_eq!(
                    verify(&r, "online.7778777.lessdugin.post", rkey, &car).unwrap(),
                    None
                );
            }
            let car = r.proof_car("online.7778777.lessdugin.profile", "self", rev);
            assert!(
                verify(&r, "online.7778777.lessdugin.profile", "self", &car)
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn mst_matches_listing_and_layers() {
        let r = repo(120);
        let c = r.commit(tid_at_micros(NOW_US));
        let listed = mst::list_entries(&c.root, &c.nodes, 10_000).unwrap();
        let expected: Vec<(Vec<u8>, Cid)> = r
            .entries()
            .into_iter()
            .map(|(p, c)| (p.into_bytes(), c))
            .collect();
        assert_eq!(listed, expected);
        // a 120-key tree has more than one layer
        assert!(c.nodes.len() > 1);
        // deterministic: same content, same root
        assert_eq!(r.clone().commit(tid_at_micros(NOW_US)).root, c.root);
        // full CAR holds everything
        let car = r.full_car(tid_at_micros(NOW_US));
        let parsed =
            lessdugin_atp::car::read_car(&car, &lessdugin_atp::car::CarLimits::PROOF).unwrap();
        assert_eq!(parsed.blocks.len(), 1 + c.nodes.len() + r.len());
    }

    #[test]
    fn wrong_key_and_tampering_fail() {
        let r = repo(10);
        let rkey = "aaaaaaaaaaaa-3l2s5xxv2z004";
        let other = SigningKey::from_slice(&[9u8; 32]).unwrap();
        let commit = r.commit_signed_by(tid_at_micros(NOW_US), &other);
        let car = r.proof_car_for("online.7778777.lessdugin.post", rkey, &commit);
        let err = verify(&r, "online.7778777.lessdugin.post", rkey, &car).unwrap_err();
        assert!(matches!(
            *err,
            ProofError::Commit(lessdugin_atp::commit::CommitError::Signature(_))
        ));
        let mut car = r.proof_car("online.7778777.lessdugin.post", rkey, tid_at_micros(NOW_US));
        let last = car.len() - 1;
        car[last] ^= 1;
        assert!(verify(&r, "online.7778777.lessdugin.post", rkey, &car).is_err());
    }
}
