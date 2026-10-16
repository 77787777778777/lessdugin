// SPDX-License-Identifier: AGPL-3.0-or-later
//! One account's repository: records as DAG-CBOR blocks, the canonical MST
//! (`lessdugin_atp::mst::build`), signed v3 commits, proof and full-repo CARs, and listing.
//!
//! Writes accumulate as pending operations; [`Repo::recommit`] builds the MST, signs a commit at
//! a new revision and returns the (coalesced) operations of that commit.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::sync::Arc;

use k256::ecdsa::signature::Signer as _;
use k256::ecdsa::{Signature, SigningKey};
use lessdugin_atp::Cid;
use lessdugin_atp::car::write_car;
use lessdugin_atp::commit::UnsignedCommit;
use lessdugin_atp::datamodel::{self, BlobRef};
use lessdugin_atp::mst::{self, Node};
use lessdugin_atp::syntax::{Did, Tid};
use serde_json::Value;

/// A stored record.
#[derive(Clone, Debug)]
pub(crate) struct Rec {
    /// Canonical DAG-CBOR.
    pub cbor: Arc<[u8]>,
    /// CID of `cbor`.
    pub cid: Cid,
    /// Blob references anywhere in the record.
    pub blobs: Vec<BlobRef>,
}

impl Rec {
    /// A record from DAG-CBOR bytes (blob refs extracted when the bytes decode).
    pub fn from_cbor(cbor: Vec<u8>) -> Self {
        let cid = lessdugin_atp::cid::for_dag_cbor(&cbor);
        let blobs = datamodel::record_from_cbor(&cbor)
            .map(|v| datamodel::blob_refs(&v))
            .unwrap_or_default();
        Self {
            cbor: cbor.into(),
            cid,
            blobs,
        }
    }

    /// The record as atproto JSON (`Value::Null` if the stored bytes do not decode).
    pub fn json(&self) -> Value {
        datamodel::record_cbor_to_json(&self.cbor).unwrap_or(Value::Null)
    }
}

/// A signed commit and its MST.
#[derive(Clone, Debug)]
pub(crate) struct Commit {
    pub cid: Cid,
    pub block: Vec<u8>,
    pub root: Cid,
    pub nodes: BTreeMap<Cid, Vec<u8>>,
    pub rev: Tid,
}

/// What a commit did to one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpAction {
    /// The path was created.
    Create,
    /// The path was updated (possibly to an identical value, as `applyWrites` allows).
    Update,
    /// The path was deleted.
    Delete,
}

impl OpAction {
    /// `create`, `update` or `delete`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// One operation of a commit (as a firehose or Jetstream would report it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoOp {
    /// Create, update or delete.
    pub action: OpAction,
    /// Collection NSID.
    pub collection: String,
    /// Record key.
    pub rkey: String,
    /// The new record CID (create/update).
    pub cid: Option<Cid>,
    /// The previous record CID (update/delete).
    pub prev: Option<Cid>,
    /// The new record's DAG-CBOR (create/update).
    pub record: Option<Arc<[u8]>>,
}

impl RepoOp {
    /// The new record as atproto JSON (create/update).
    pub fn record_json(&self) -> Option<Value> {
        self.record
            .as_ref()
            .and_then(|b| datamodel::record_cbor_to_json(b).ok())
    }
}

#[derive(Clone, Debug)]
struct Pending {
    path: String,
    prev: Option<Cid>,
    next: Option<Rec>,
    /// Set by an explicit update (kept even when the value is identical).
    touched: bool,
}

/// An account repository.
#[derive(Clone, Debug)]
pub(crate) struct Repo {
    pub records: BTreeMap<String, Rec>,
    pub commit: Commit,
    pending: Vec<Pending>,
    force: bool,
}

pub(crate) fn path(collection: &str, rkey: &str) -> String {
    format!("{collection}/{rkey}")
}

fn split_path(p: &str) -> (&str, &str) {
    p.split_once('/').unwrap_or((p, ""))
}

fn sign(did: &Did, key: &SigningKey, root: Cid, rev: Tid) -> (Cid, Vec<u8>) {
    let unsigned = UnsignedCommit {
        did: did.clone(),
        data: root,
        rev,
        prev: None,
    };
    let sig: Signature = key.sign(&unsigned.signing_bytes());
    unsigned.signed(&sig.to_bytes())
}

fn build_commit<'a>(
    did: &Did,
    key: &SigningKey,
    rev: Tid,
    records: impl Iterator<Item = (&'a String, &'a Rec)>,
) -> Commit {
    let (root, nodes) =
        mst::build(records.map(|(p, r)| (p.as_bytes().to_vec(), r.cid))).expect("valid MST keys");
    let (cid, block) = sign(did, key, root, rev);
    Commit {
        cid,
        block,
        root,
        nodes,
        rev,
    }
}

impl Repo {
    /// An empty repository with an initial commit at `rev`.
    pub fn new(did: &Did, key: &SigningKey, rev: Tid) -> Self {
        let records = BTreeMap::new();
        let commit = build_commit(did, key, rev, records.iter());
        Self {
            records,
            commit,
            pending: Vec::new(),
            force: false,
        }
    }

    /// The record at `collection/rkey`.
    pub fn get(&self, collection: &str, rkey: &str) -> Option<&Rec> {
        self.records.get(&path(collection, rkey))
    }

    /// Creates or replaces a record; returns its CID.
    pub fn put(&mut self, collection: &str, rkey: &str, rec: Rec) -> Cid {
        let p = path(collection, rkey);
        mst::validate_key(p.as_bytes()).expect("valid repo path");
        let cid = rec.cid;
        let prev = self.records.insert(p.clone(), rec.clone()).map(|r| r.cid);
        self.pending.push(Pending {
            path: p,
            prev,
            next: Some(rec),
            touched: true,
        });
        cid
    }

    /// Deletes a record; returns its CID if it existed.
    pub fn delete(&mut self, collection: &str, rkey: &str) -> Option<Cid> {
        let p = path(collection, rkey);
        let prev = self.records.remove(&p).map(|r| r.cid)?;
        self.pending.push(Pending {
            path: p,
            prev: Some(prev),
            next: None,
            touched: true,
        });
        Some(prev)
    }

    /// True if writes are waiting for a commit.
    pub fn is_dirty(&self) -> bool {
        self.force || !self.pending.is_empty()
    }

    /// Requests a new commit even without pending writes.
    pub fn force_commit(&mut self) {
        self.force = true;
    }

    /// Builds the MST and signs a commit at `rev`; returns the commit's operations (one per
    /// path, in path order).
    pub fn recommit(&mut self, did: &Did, key: &SigningKey, rev: Tid) -> Vec<RepoOp> {
        self.commit = build_commit(did, key, rev, self.records.iter());
        self.force = false;
        let mut by_path: BTreeMap<String, Pending> = BTreeMap::new();
        for p in self.pending.drain(..) {
            match by_path.get_mut(&p.path) {
                Some(e) => {
                    e.next = p.next;
                    e.touched |= p.touched;
                }
                None => {
                    by_path.insert(p.path.clone(), p);
                }
            }
        }
        by_path
            .into_values()
            .filter_map(|p| {
                let (collection, rkey) = split_path(&p.path);
                let action = match (&p.prev, &p.next) {
                    (None, Some(_)) => OpAction::Create,
                    (Some(_), Some(_)) if p.touched => OpAction::Update,
                    (Some(_), None) => OpAction::Delete,
                    _ => return None,
                };
                Some(RepoOp {
                    action,
                    collection: collection.to_owned(),
                    rkey: rkey.to_owned(),
                    cid: p.next.as_ref().map(|r| r.cid),
                    prev: p.prev,
                    record: p.next.map(|r| r.cbor),
                })
            })
            .collect()
    }

    /// Re-signs the current tree at `rev` with `key` without any write (equivocation forks,
    /// wrong-key commits).
    pub fn resign(&mut self, did: &Did, key: &SigningKey, rev: Tid) {
        let (cid, block) = sign(did, key, self.commit.root, rev);
        self.commit.cid = cid;
        self.commit.block = block;
        self.commit.rev = rev;
    }

    /// The collections with at least one record, sorted.
    pub fn collections(&self) -> Vec<String> {
        let mut out: BTreeSet<&str> = BTreeSet::new();
        for p in self.records.keys() {
            out.insert(split_path(p).0);
        }
        out.into_iter().map(str::to_owned).collect()
    }

    /// True if the collection has at least one record.
    pub fn has_collection(&self, collection: &str) -> bool {
        let lo = format!("{collection}/");
        self.records
            .range::<String, _>((Bound::Included(&lo), Bound::Unbounded))
            .next()
            .is_some_and(|(p, _)| p.starts_with(&lo))
    }

    /// Up to `n` records of `collection`, by rkey: descending below `bound` (exclusive), or
    /// ascending above it when `ascending`. No bound: from the top (or the bottom).
    pub fn list(
        &self,
        collection: &str,
        bound: Option<&str>,
        ascending: bool,
        n: usize,
    ) -> Vec<(&str, &Rec)> {
        let lo = format!("{collection}/");
        // '0' is the byte after '/', so every `collection/…` path sorts below `collection0`.
        let hi = format!("{collection}0");
        let b = bound.map(|b| path(collection, b));
        let range = if ascending {
            let start = match &b {
                Some(b) => Bound::Excluded(b.clone()),
                None => Bound::Included(lo),
            };
            self.records.range((start, Bound::Excluded(hi)))
        } else {
            let end = match &b {
                Some(b) => Bound::Excluded(b.clone()),
                None => Bound::Excluded(hi),
            };
            self.records.range((Bound::Included(lo), end))
        };
        // `rkey` byte order inside `collection/` equals path byte order, and every path in the
        // range has the prefix.
        if ascending {
            range.take(n).map(|(p, r)| (split_path(p).1, r)).collect()
        } else {
            range
                .rev()
                .take(n)
                .map(|(p, r)| (split_path(p).1, r))
                .collect()
        }
    }

    /// Every blob CID referenced by a record.
    pub fn referenced_blobs(&self) -> BTreeSet<Cid> {
        self.records
            .values()
            .flat_map(|r| r.blobs.iter().map(|b| b.cid))
            .collect()
    }

    /// The blocks proving `collection/rkey` (presence or absence) under the current commit: the
    /// commit, the MST nodes on the search path, and the record block if present.
    pub fn proof_blocks(&self, collection: &str, rkey: &str) -> Vec<(Cid, Vec<u8>)> {
        let target = path(collection, rkey);
        let target = target.as_bytes();
        let c = &self.commit;
        let mut blocks = vec![(c.cid, c.block.clone())];
        let mut cid = c.root;
        while let Some(bytes) = c.nodes.get(&cid) {
            let node = Node::decode(bytes).expect("own MST node");
            let keys = node.keys().expect("own MST keys");
            blocks.push((cid, bytes.clone()));
            let idx = keys.partition_point(|k| k.as_slice() < target);
            if keys.get(idx).is_some_and(|k| k.as_slice() == target) {
                let value = node.entries[idx].value;
                if let Some(rec) = self
                    .records
                    .get(std::str::from_utf8(target).unwrap_or_default())
                    .filter(|r| r.cid == value)
                {
                    blocks.push((rec.cid, rec.cbor.to_vec()));
                }
                break;
            }
            let next = if idx == 0 {
                node.left
            } else {
                node.entries[idx - 1].tree
            };
            match next {
                Some(n) => cid = n,
                None => break,
            }
        }
        blocks
    }

    /// The `com.atproto.sync.getRecord` CAR for `collection/rkey`.
    pub fn proof_car(&self, collection: &str, rkey: &str) -> Vec<u8> {
        let blocks = self.proof_blocks(collection, rkey);
        write_car(
            &[self.commit.cid],
            blocks.iter().map(|(c, b)| (c, b.as_slice())),
        )
    }

    /// The `com.atproto.sync.getRepo` CAR: the commit, every MST node and every record.
    pub fn full_car(&self) -> Vec<u8> {
        let c = &self.commit;
        let mut blocks: Vec<(Cid, &[u8])> = vec![(c.cid, c.block.as_slice())];
        blocks.extend(c.nodes.iter().map(|(cid, b)| (*cid, b.as_slice())));
        let mut seen = BTreeSet::new();
        for r in self.records.values() {
            if seen.insert(r.cid) {
                blocks.push((r.cid, &r.cbor));
            }
        }
        write_car(&[c.cid], blocks.iter().map(|(c, b)| (c, *b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(v: Value) -> Rec {
        Rec::from_cbor(datamodel::record_to_cbor(&v).unwrap())
    }

    #[test]
    fn listing_bounds_are_exclusive_and_stay_in_the_collection() {
        let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
        let key = SigningKey::from_slice(&[3u8; 32]).unwrap();
        let mut r = Repo::new(&did, &key, Tid::from_parts(1, 0).unwrap());
        for k in ["a", "b", "c"] {
            r.put("x.y.z", k, rec(json!({"k": k})));
        }
        r.put("x.y.za", "a", rec(json!({})));
        r.put("x.y", "q", rec(json!({})));
        let keys =
            |v: Vec<(&str, &Rec)>| v.into_iter().map(|(k, _)| k.to_owned()).collect::<Vec<_>>();
        assert_eq!(keys(r.list("x.y.z", None, false, 10)), ["c", "b", "a"]);
        assert_eq!(keys(r.list("x.y.z", None, true, 10)), ["a", "b", "c"]);
        assert_eq!(keys(r.list("x.y.z", Some("c"), false, 10)), ["b", "a"]);
        assert_eq!(keys(r.list("x.y.z", Some("b"), true, 10)), ["c"]);
        assert_eq!(keys(r.list("x.y.z", Some("zz"), false, 1)), ["c"]);
        assert_eq!(keys(r.list("x.y.z", Some(""), true, 10)), ["a", "b", "c"]);
        assert!(r.list("x.y.z", Some("a"), false, 10).is_empty());
        assert_eq!(r.collections(), ["x.y", "x.y.z", "x.y.za"]);
        assert!(r.has_collection("x.y") && !r.has_collection("x"));
        let ops = r.recommit(&did, &key, Tid::from_parts(2, 0).unwrap());
        assert_eq!(ops.len(), 5);
        assert!(ops.iter().all(|o| o.action == OpAction::Create));
        // create + delete in one commit cancels out; update keeps the first prev
        r.put("x.y.z", "d", rec(json!({})));
        r.delete("x.y.z", "d");
        let old = r.get("x.y.z", "a").unwrap().cid;
        r.put("x.y.z", "a", rec(json!({"v": 2})));
        r.put("x.y.z", "a", rec(json!({"v": 3})));
        let ops = r.recommit(&did, &key, Tid::from_parts(3, 0).unwrap());
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].action, OpAction::Update);
        assert_eq!(ops[0].prev, Some(old));
        assert_eq!(ops[0].record_json().unwrap(), json!({"v": 3}));
    }
}
