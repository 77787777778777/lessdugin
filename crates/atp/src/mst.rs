// SPDX-License-Identifier: MIT OR Apache-2.0
//! Merkle Search Tree (`/specs/repository`, "MST Structure"): node codec, key heights,
//! inclusion/exclusion proofs and a deterministic builder.
//!
//! Node schema: `{l: link|null, e: [{p: int, k: bytes, v: link, t: link|null}]}`. Keys are
//! prefix-compressed within a node (`p` = bytes shared with the previous key, mandatory and exact).
//! A key's height is the number of leading zero bits of `sha256(key)` divided by two (fanout 4).
//!
//! Every visited node is checked: its block hashes to its CID, subtree links are blessed dag-cbor
//! CIDs, keys are valid repo paths, strictly increasing, within the bounds implied by the parent,
//! all at the node's layer, and child layers are exactly one less than their parent's. The walk is
//! iterative and bounded by [`MAX_DEPTH`] nodes.

use std::collections::{BTreeMap, HashMap};

use ::cid::Cid;

use crate::Ipld;
use crate::car::Car;
use crate::cid::CidError;
use crate::datamodel::{DataModelError, Limits};
use crate::syntax::{Nsid, RecordKey};

/// Maximum repo path (MST key) length accepted.
pub const MAX_KEY_LEN: usize = 1024;
/// Maximum number of nodes on one root-to-leaf path.
pub const MAX_DEPTH: usize = 32;
/// Maximum entries in one node (statistically unreachable for honest repos with fanout 4).
pub const MAX_NODE_ENTRIES: usize = 1024;

/// MST errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MstError {
    /// A needed node block is not available.
    #[error("missing MST block {0}")]
    MissingBlock(Cid),
    /// A node block is not valid DAG-CBOR.
    #[error("MST node is not valid DAG-CBOR: {0}")]
    Decode(DataModelError),
    /// A node does not match the schema.
    #[error("malformed MST node: {0}")]
    Malformed(&'static str),
    /// A link is not a blessed CID, or a block does not hash to its CID.
    #[error("MST link: {0}")]
    Cid(CidError),
    /// A key is not a valid repo path.
    #[error("invalid MST key: {0}")]
    InvalidKey(&'static str),
    /// Keys are not strictly increasing, or fall outside their subtree's range.
    #[error("MST key ordering violation")]
    Ordering,
    /// A prefix length is not the exact common prefix with the previous key.
    #[error("MST key prefix compression is not canonical")]
    PrefixNotCanonical,
    /// A key sits at the wrong layer, or a subtree skips or repeats a layer.
    #[error("MST layer violation")]
    Layer,
    /// The path is longer than [`MAX_DEPTH`] nodes.
    #[error("MST deeper than {0} nodes")]
    DepthExceeded(usize),
}

/// One node entry (prefix-compressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Bytes shared with the previous key in the node (`p`).
    pub prefix_len: usize,
    /// The rest of the key (`k`).
    pub key_suffix: Vec<u8>,
    /// The record CID (`v`).
    pub value: Cid,
    /// The subtree to the right of this entry (`t`).
    pub tree: Option<Cid>,
}

/// One MST node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    /// The leftmost subtree (`l`).
    pub left: Option<Cid>,
    /// Entries (`e`).
    pub entries: Vec<TreeEntry>,
}

fn node_limits() -> Limits {
    Limits {
        max_depth: 4,
        max_container_len: MAX_NODE_ENTRIES,
        max_string_bytes: 256,
        max_bytes_len: MAX_KEY_LEN + 128,
        max_total_nodes: MAX_NODE_ENTRIES * 6 + 16,
    }
}

fn opt_link(v: Option<&Ipld>, what: &'static str) -> Result<Option<Cid>, MstError> {
    match v {
        None | Some(Ipld::Null) => Ok(None),
        Some(Ipld::Link(c)) => {
            crate::cid::check_blessed_dag_cbor(c).map_err(MstError::Cid)?;
            Ok(Some(*c))
        }
        Some(_) => Err(MstError::Malformed(what)),
    }
}

impl Node {
    /// Decodes a node block (structure only; see [`Node::keys`] for key validation).
    pub fn decode(bytes: &[u8]) -> Result<Self, MstError> {
        let v = crate::dagcbor::decode_with(bytes, &node_limits()).map_err(MstError::Decode)?;
        let Ipld::Map(map) = v else {
            return Err(MstError::Malformed("node is not a map"));
        };
        let left = opt_link(map.get("l"), "l must be a link or null")?;
        let Some(Ipld::List(raw)) = map.get("e") else {
            return Err(MstError::Malformed("e must be a list"));
        };
        let mut entries = Vec::new();
        for e in raw {
            let Ipld::Map(e) = e else {
                return Err(MstError::Malformed("entry is not a map"));
            };
            let prefix_len = match e.get("p") {
                Some(Ipld::Integer(p)) if *p >= 0 && *p <= MAX_KEY_LEN as i128 => *p as usize,
                _ => {
                    return Err(MstError::Malformed(
                        "p must be a small non-negative integer",
                    ));
                }
            };
            let Some(Ipld::Bytes(key_suffix)) = e.get("k") else {
                return Err(MstError::Malformed("k must be bytes"));
            };
            let Some(Ipld::Link(value)) = e.get("v") else {
                return Err(MstError::Malformed("v must be a link"));
            };
            let tree = opt_link(e.get("t"), "t must be a link or null")?;
            entries.push(TreeEntry {
                prefix_len,
                key_suffix: key_suffix.clone(),
                value: *value,
                tree,
            });
        }
        Ok(Self { left, entries })
    }

    /// The node as IPLD (`l` and `t` always present, `null` when empty).
    pub fn to_ipld(&self) -> Ipld {
        let link = |c: &Option<Cid>| c.map_or(Ipld::Null, Ipld::Link);
        let entries = self
            .entries
            .iter()
            .map(|e| {
                let mut m = BTreeMap::new();
                m.insert("p".to_owned(), Ipld::Integer(e.prefix_len as i128));
                m.insert("k".to_owned(), Ipld::Bytes(e.key_suffix.clone()));
                m.insert("v".to_owned(), Ipld::Link(e.value));
                m.insert("t".to_owned(), link(&e.tree));
                Ipld::Map(m)
            })
            .collect();
        let mut m = BTreeMap::new();
        m.insert("l".to_owned(), link(&self.left));
        m.insert("e".to_owned(), Ipld::List(entries));
        Ipld::Map(m)
    }

    /// Canonical DAG-CBOR encoding.
    pub fn encode(&self) -> Vec<u8> {
        // Depth 3, no floats, no large integers: encoding cannot fail.
        crate::dagcbor::encode(&self.to_ipld()).unwrap_or_default()
    }

    /// Reconstructs the full keys, checking prefix compression, ordering and key syntax.
    pub fn keys(&self) -> Result<Vec<Vec<u8>>, MstError> {
        let mut out: Vec<Vec<u8>> = Vec::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            let prev: &[u8] = out.last().map_or(&[], Vec::as_slice);
            if i == 0 && e.prefix_len != 0 {
                return Err(MstError::PrefixNotCanonical);
            }
            if e.prefix_len > prev.len() {
                return Err(MstError::PrefixNotCanonical);
            }
            let mut key = Vec::with_capacity(e.prefix_len + e.key_suffix.len());
            key.extend_from_slice(&prev[..e.prefix_len]);
            key.extend_from_slice(&e.key_suffix);
            validate_key(&key)?;
            if i > 0 {
                if common_prefix_len(prev, &key) != e.prefix_len {
                    return Err(MstError::PrefixNotCanonical);
                }
                if key.as_slice() <= prev {
                    return Err(MstError::Ordering);
                }
            }
            out.push(key);
        }
        Ok(out)
    }
}

/// The MST height (layer) of a key: leading zero bits of `sha256(key)`, divided by two.
pub fn key_height(key: &[u8]) -> u32 {
    let h = crate::cid::sha256(key);
    let mut zeros = 0u32;
    for b in h {
        if b == 0 {
            zeros += 8;
        } else {
            zeros += b.leading_zeros();
            break;
        }
    }
    zeros / 2
}

/// Length of the common prefix of two byte strings.
pub fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Validates a repo path / MST key generically: 1..=[`MAX_KEY_LEN`] bytes of
/// `[A-Za-z0-9._:~-]` and exactly one `/` separating two non-empty segments, neither `.` nor
/// `..`. (Segments are not required to be a valid NSID / record key, matching the interop
/// fixtures; use [`repo_path`] to build strict keys.)
pub fn validate_key(key: &[u8]) -> Result<(), MstError> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err(MstError::InvalidKey("length"));
    }
    let Some(slash) = key.iter().position(|&b| b == b'/') else {
        return Err(MstError::InvalidKey("missing '/'"));
    };
    let (coll, rkey) = (&key[..slash], &key[slash + 1..]);
    for seg in [coll, rkey] {
        if seg.is_empty() || seg == b"." || seg == b".." {
            return Err(MstError::InvalidKey("empty or dot segment"));
        }
        if !seg.iter().all(|&b| crate::syntax::is_rkey_byte(b)) {
            return Err(MstError::InvalidKey("invalid character"));
        }
    }
    Ok(())
}

/// The MST key for a record: `<collection>/<rkey>`.
pub fn repo_path(collection: &Nsid, rkey: &RecordKey) -> String {
    format!("{}/{}", collection.as_str(), rkey.as_str())
}

/// A source of blocks by CID.
pub trait BlockSource {
    /// The block bytes for `cid`, if available.
    fn get_block(&self, cid: &Cid) -> Option<&[u8]>;
}

impl BlockSource for Car {
    fn get_block(&self, cid: &Cid) -> Option<&[u8]> {
        self.get(cid)
    }
}

impl BlockSource for BTreeMap<Cid, Vec<u8>> {
    fn get_block(&self, cid: &Cid) -> Option<&[u8]> {
        self.get(cid).map(Vec::as_slice)
    }
}

impl<S: std::hash::BuildHasher> BlockSource for HashMap<Cid, Vec<u8>, S> {
    fn get_block(&self, cid: &Cid) -> Option<&[u8]> {
        self.get(cid).map(Vec::as_slice)
    }
}

/// Result of a proof walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    /// The key maps to this record CID.
    Present(Cid),
    /// The key is provably not in the tree.
    Absent,
}

/// A node loaded and fully checked for a walk.
struct Loaded {
    node: Node,
    keys: Vec<Vec<u8>>,
    layer: Option<u32>,
}

fn load(cid: &Cid, blocks: &impl BlockSource) -> Result<Loaded, MstError> {
    crate::cid::check_blessed_dag_cbor(cid).map_err(MstError::Cid)?;
    let bytes = blocks.get_block(cid).ok_or(MstError::MissingBlock(*cid))?;
    crate::cid::verify(cid, bytes).map_err(MstError::Cid)?;
    let node = Node::decode(bytes)?;
    let keys = node.keys()?;
    let mut layer = None;
    for k in &keys {
        let h = key_height(k);
        match layer {
            None => layer = Some(h),
            Some(l) if l == h => {}
            Some(_) => return Err(MstError::Layer),
        }
    }
    Ok(Loaded { node, keys, layer })
}

/// Checks a loaded node against its expected layer and key range; returns its layer.
fn check_node(
    n: &Loaded,
    expected: Option<u32>,
    low: Option<&[u8]>,
    high: Option<&[u8]>,
) -> Result<u32, MstError> {
    let layer = match (n.layer, expected) {
        (Some(l), Some(e)) if l != e => return Err(MstError::Layer),
        (Some(l), _) => l,
        // An empty intermediate node must point to a subtree.
        (None, Some(e)) if n.node.left.is_some() => e,
        (None, _) => return Err(MstError::Malformed("empty non-root node")),
    };
    if layer == 0 && (n.node.left.is_some() || n.node.entries.iter().any(|e| e.tree.is_some())) {
        return Err(MstError::Layer);
    }
    if let (Some(first), Some(lo)) = (n.keys.first(), low)
        && first.as_slice() <= lo
    {
        return Err(MstError::Ordering);
    }
    if let (Some(last), Some(hi)) = (n.keys.last(), high)
        && last.as_slice() >= hi
    {
        return Err(MstError::Ordering);
    }
    Ok(layer)
}

/// Walks from `root` toward `key` using only `blocks`, proving presence or absence.
///
/// Fails on a missing block, a malformed node, an ordering or layer violation, or a path longer
/// than [`MAX_DEPTH`] nodes. `key` itself must be a valid repo path.
pub fn prove(root: &Cid, key: &[u8], blocks: &impl BlockSource) -> Result<Proof, MstError> {
    validate_key(key)?;
    let target_height = key_height(key);
    let mut cid = *root;
    let mut expected: Option<u32> = None;
    let mut low: Option<Vec<u8>> = None;
    let mut high: Option<Vec<u8>> = None;
    for _ in 0..MAX_DEPTH {
        let n = load(&cid, blocks)?;
        if expected.is_none() && n.keys.is_empty() {
            // Root: either the empty tree, or an invalid empty node pointing down.
            return if n.node.left.is_none() {
                Ok(Proof::Absent)
            } else {
                Err(MstError::Malformed("root node is empty but has a subtree"))
            };
        }
        let layer = check_node(&n, expected, low.as_deref(), high.as_deref())?;
        let idx = n.keys.partition_point(|k| k.as_slice() < key);
        if n.keys.get(idx).is_some_and(|k| k.as_slice() == key) {
            return Ok(Proof::Present(n.node.entries[idx].value));
        }
        if target_height >= layer {
            // The key would have to be in this node (or an ancestor, already searched).
            return Ok(Proof::Absent);
        }
        let child = if idx == 0 {
            n.node.left
        } else {
            n.node.entries[idx - 1].tree
        };
        let Some(child) = child else {
            return Ok(Proof::Absent);
        };
        if idx > 0 {
            low = Some(n.keys[idx - 1].clone());
        }
        if let Some(k) = n.keys.get(idx) {
            high = Some(k.clone());
        }
        expected = Some(layer - 1);
        cid = child;
    }
    Err(MstError::DepthExceeded(MAX_DEPTH))
}

/// Lists every `(key, value)` in the tree in key order, validating every node like [`prove`].
/// Visits at most `max_nodes` nodes (a cap on work for untrusted input).
pub fn list_entries(
    root: &Cid,
    blocks: &impl BlockSource,
    max_nodes: usize,
) -> Result<Vec<(Vec<u8>, Cid)>, MstError> {
    // Frame: either a node to expand (with constraints) or an entry to emit.
    enum Item {
        Node {
            cid: Cid,
            expected: Option<u32>,
            low: Option<Vec<u8>>,
            high: Option<Vec<u8>>,
            depth: usize,
        },
        Entry(Vec<u8>, Cid),
    }
    let mut out = Vec::new();
    let mut stack = vec![Item::Node {
        cid: *root,
        expected: None,
        low: None,
        high: None,
        depth: 1,
    }];
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        let (cid, expected, low, high, depth) = match item {
            Item::Entry(k, v) => {
                out.push((k, v));
                continue;
            }
            Item::Node {
                cid,
                expected,
                low,
                high,
                depth,
            } => (cid, expected, low, high, depth),
        };
        visited += 1;
        if visited > max_nodes {
            return Err(MstError::Malformed("too many nodes"));
        }
        if depth > MAX_DEPTH {
            return Err(MstError::DepthExceeded(MAX_DEPTH));
        }
        let n = load(&cid, blocks)?;
        if expected.is_none() && n.keys.is_empty() {
            if n.node.left.is_some() {
                return Err(MstError::Malformed("root node is empty but has a subtree"));
            }
            continue;
        }
        let layer = check_node(&n, expected, low.as_deref(), high.as_deref())?;
        // Push in reverse so items pop in key order: left, e0, t0, e1, t1, …
        let child_layer = layer.checked_sub(1);
        let mut frames = Vec::new();
        let child =
            |c: Option<Cid>, lo: Option<Vec<u8>>, hi: Option<Vec<u8>>, frames: &mut Vec<Item>| {
                if let Some(c) = c {
                    frames.push(Item::Node {
                        cid: c,
                        expected: child_layer,
                        low: lo,
                        high: hi,
                        depth: depth + 1,
                    });
                }
            };
        child(
            n.node.left,
            low.clone(),
            n.keys.first().cloned().or(high.clone()),
            &mut frames,
        );
        for (i, e) in n.node.entries.iter().enumerate() {
            frames.push(Item::Entry(n.keys[i].clone(), e.value));
            let hi = n.keys.get(i + 1).cloned().or(high.clone());
            child(e.tree, Some(n.keys[i].clone()), hi, &mut frames);
        }
        stack.extend(frames.into_iter().rev());
    }
    Ok(out)
}

/// Builds the canonical MST for `entries` (keys must be valid repo paths; later duplicates win).
/// Returns the root CID and every node block. Intended for tests, fixtures and fake servers.
pub fn build<K: AsRef<[u8]>>(
    entries: impl IntoIterator<Item = (K, Cid)>,
) -> Result<(Cid, BTreeMap<Cid, Vec<u8>>), MstError> {
    let mut sorted = BTreeMap::new();
    for (k, v) in entries {
        let k = k.as_ref().to_vec();
        validate_key(&k)?;
        sorted.insert(k, v);
    }
    let items: Vec<(Vec<u8>, Cid, u32)> = sorted
        .into_iter()
        .map(|(k, v)| {
            let h = key_height(&k);
            (k, v, h)
        })
        .collect();
    let mut blocks = BTreeMap::new();
    let root = match items.iter().map(|i| i.2).max() {
        None => {
            let bytes = Node::default().encode();
            let cid = crate::cid::for_dag_cbor(&bytes);
            blocks.insert(cid, bytes);
            cid
        }
        Some(top) => build_node(&items, top, &mut blocks)?,
    };
    Ok((root, blocks))
}

fn build_node(
    items: &[(Vec<u8>, Cid, u32)],
    layer: u32,
    blocks: &mut BTreeMap<Cid, Vec<u8>>,
) -> Result<Cid, MstError> {
    let sub = |s: &[(Vec<u8>, Cid, u32)],
               blocks: &mut BTreeMap<Cid, Vec<u8>>|
     -> Result<Option<Cid>, MstError> {
        if s.is_empty() {
            return Ok(None);
        }
        let lower = layer.checked_sub(1).ok_or(MstError::Layer)?;
        build_node(s, lower, blocks).map(Some)
    };
    let mut node = Node::default();
    let mut start = 0;
    let mut prev: &[u8] = &[];
    for (i, (key, value, h)) in items.iter().enumerate() {
        if *h != layer {
            continue;
        }
        let subtree = sub(&items[start..i], blocks)?;
        match node.entries.last_mut() {
            Some(last) => last.tree = subtree,
            None => node.left = subtree,
        }
        let p = common_prefix_len(prev, key);
        node.entries.push(TreeEntry {
            prefix_len: p,
            key_suffix: key[p..].to_vec(),
            value: *value,
            tree: None,
        });
        prev = key;
        start = i + 1;
    }
    let subtree = sub(&items[start..], blocks)?;
    match node.entries.last_mut() {
        Some(last) => last.tree = subtree,
        None => node.left = subtree,
    }
    let bytes = node.encode();
    let cid = crate::cid::for_dag_cbor(&bytes);
    blocks.insert(cid, bytes);
    Ok(cid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf() -> Cid {
        crate::cid::parse("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454").unwrap()
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("com.example.record/{i:06}"))
            .collect()
    }

    #[test]
    fn heights() {
        assert_eq!(key_height(b""), 0);
        assert_eq!(key_height(b"blue"), 1);
        assert_eq!(key_height(b"app.bsky.feed.post/9adeb165882c"), 8);
    }

    #[test]
    fn keys_validation() {
        assert!(validate_key(b"com.example.foo/self").is_ok());
        assert!(validate_key(b"A0/374913").is_ok());
        for bad in [
            &b""[..],
            b"noslash",
            b"/x",
            b"x/",
            b"a/b/c",
            b"a/..",
            b"./a",
            b"a/b c",
            b"a/b#",
        ] {
            assert!(
                validate_key(bad).is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(validate_key(&[b'a'; MAX_KEY_LEN + 1]).is_err());
    }

    #[test]
    fn empty_tree() {
        let (root, blocks) = build(Vec::<(String, Cid)>::new()).unwrap();
        assert_eq!(
            root.to_string(),
            "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"
        );
        assert_eq!(prove(&root, b"a/b", &blocks).unwrap(), Proof::Absent);
        assert!(list_entries(&root, &blocks, 10).unwrap().is_empty());
    }

    #[test]
    fn present_absent_and_listing() {
        let ks = keys(300);
        let v = leaf();
        let (root, blocks) = build(ks.iter().map(|k| (k.as_bytes(), v))).unwrap();
        assert!(blocks.len() > 10);
        for k in &ks {
            assert_eq!(
                prove(&root, k.as_bytes(), &blocks).unwrap(),
                Proof::Present(v),
                "{k}"
            );
        }
        for k in [
            "com.example.record/000000a",
            "a/b",
            "com.example.record/999999",
            "zzz/zzz",
        ] {
            assert_eq!(
                prove(&root, k.as_bytes(), &blocks).unwrap(),
                Proof::Absent,
                "{k}"
            );
        }
        let listed = list_entries(&root, &blocks, 10_000).unwrap();
        assert_eq!(listed.len(), 300);
        assert!(
            listed
                .iter()
                .zip(&ks)
                .all(|((k, c), e)| k == e.as_bytes() && *c == v)
        );
        assert!(list_entries(&root, &blocks, 2).is_err());
    }

    #[test]
    fn missing_and_tampered() {
        let ks = keys(100);
        let (root, blocks) = build(ks.iter().map(|k| (k.as_bytes(), leaf()))).unwrap();
        let mut missing = blocks.clone();
        missing.remove(&root);
        assert_eq!(
            prove(&root, ks[0].as_bytes(), &missing),
            Err(MstError::MissingBlock(root))
        );
        // swap the root block's bytes for another node: hash mismatch
        let mut swapped = blocks.clone();
        let other = blocks.iter().find(|(c, _)| **c != root).unwrap().1.clone();
        swapped.insert(root, other);
        assert!(matches!(
            prove(&root, ks[0].as_bytes(), &swapped),
            Err(MstError::Cid(_))
        ));
        // every proof uses only the nodes on its path
        let mut needed = 0;
        for k in &ks {
            let mut trimmed = BTreeMap::new();
            let mut cid = root;
            loop {
                let bytes = blocks[&cid].clone();
                trimmed.insert(cid, bytes.clone());
                let n = Node::decode(&bytes).unwrap();
                let nk = n.keys().unwrap();
                let idx = nk.partition_point(|x| x.as_slice() < k.as_bytes());
                if nk.get(idx).is_some_and(|x| x == k.as_bytes()) {
                    break;
                }
                cid = if idx == 0 {
                    n.left.unwrap()
                } else {
                    n.entries[idx - 1].tree.unwrap()
                };
            }
            needed = needed.max(trimmed.len());
            assert!(prove(&root, k.as_bytes(), &trimmed).is_ok());
        }
        assert!(needed >= 2);
    }

    /// Re-encodes a node after mutation and re-links its parent chain up to a new root.
    fn forged_root(node: &Node) -> (Cid, BTreeMap<Cid, Vec<u8>>) {
        let bytes = node.encode();
        let cid = crate::cid::for_dag_cbor(&bytes);
        let mut b = BTreeMap::new();
        b.insert(cid, bytes);
        (cid, b)
    }

    /// `n` keys `a/<i>` of height 0, in sorted order.
    fn height0_keys(n: usize) -> Vec<String> {
        let mut out: Vec<String> = (0..1000)
            .map(|i| format!("a/{i:03}"))
            .filter(|k| key_height(k.as_bytes()) == 0)
            .take(n)
            .collect();
        out.sort();
        out
    }

    #[test]
    fn rejects_malformed_nodes() {
        let v = leaf();
        let e = |p: usize, k: &str| TreeEntry {
            prefix_len: p,
            key_suffix: k.as_bytes().to_vec(),
            value: v,
            tree: None,
        };
        let ks = height0_keys(3);
        let p1 = common_prefix_len(ks[0].as_bytes(), ks[1].as_bytes());
        let p2 = common_prefix_len(ks[1].as_bytes(), ks[2].as_bytes());
        let good = Node {
            left: None,
            entries: vec![e(0, &ks[0]), e(p1, &ks[1][p1..]), e(p2, &ks[2][p2..])],
        };
        let (root, blocks) = forged_root(&good);
        assert_eq!(
            prove(&root, ks[1].as_bytes(), &blocks).unwrap(),
            Proof::Present(v)
        );
        assert_eq!(prove(&root, b"a/zzz", &blocks).unwrap(), Proof::Absent);
        let unsorted = Node {
            left: None,
            entries: vec![e(0, &ks[1]), e(p1, &ks[0][p1..])],
        };
        let (r, b) = forged_root(&unsorted);
        assert_eq!(prove(&r, ks[0].as_bytes(), &b), Err(MstError::Ordering));
        let dup = Node {
            left: None,
            entries: vec![e(0, &ks[0]), e(ks[0].len(), "")],
        };
        let (r, b) = forged_root(&dup);
        assert!(prove(&r, ks[0].as_bytes(), &b).is_err());
        let bad_prefix = Node {
            left: None,
            entries: vec![e(0, &ks[0]), e(1, &ks[1][1..])],
        };
        let (r, b) = forged_root(&bad_prefix);
        assert_eq!(
            prove(&r, ks[1].as_bytes(), &b),
            Err(MstError::PrefixNotCanonical)
        );
        let first_prefix = Node {
            left: None,
            entries: vec![e(1, &ks[0])],
        };
        let (r, b) = forged_root(&first_prefix);
        assert_eq!(
            prove(&r, ks[0].as_bytes(), &b),
            Err(MstError::PrefixNotCanonical)
        );
        let bad_key = Node {
            left: None,
            entries: vec![e(0, "nokey")],
        };
        let (r, b) = forged_root(&bad_key);
        assert!(matches!(
            prove(&r, b"a/1", &b),
            Err(MstError::InvalidKey(_))
        ));
        // empty root with a subtree
        let (inner, mut b) = forged_root(&good);
        let top = Node {
            left: Some(inner),
            entries: vec![],
        };
        let (r, b2) = forged_root(&top);
        b.extend(b2);
        assert!(prove(&r, ks[0].as_bytes(), &b).is_err());
        // wrong field types
        let mut m = BTreeMap::new();
        m.insert("e".to_owned(), Ipld::Integer(1));
        let bytes = crate::dagcbor::encode(&Ipld::Map(m)).unwrap();
        assert!(matches!(Node::decode(&bytes), Err(MstError::Malformed(_))));
        assert!(matches!(Node::decode(&[0xff]), Err(MstError::Decode(_))));
        // subtree link that is not dag-cbor
        let raw_link = Node {
            left: Some(crate::cid::for_raw(b"x")),
            entries: vec![],
        };
        assert!(matches!(
            Node::decode(&raw_link.encode()),
            Err(MstError::Cid(_))
        ));
    }

    #[test]
    fn rejects_layer_and_range_violations() {
        let v = leaf();
        let entry = |k: &str| TreeEntry {
            prefix_len: 0,
            key_suffix: k.as_bytes().to_vec(),
            value: v,
            tree: None,
        };
        let find = |h: u32, pred: &dyn Fn(&str) -> bool| {
            (0..100_000)
                .map(|i| format!("c.d.e/{i}"))
                .find(|k| key_height(k.as_bytes()) == h && pred(k))
                .unwrap()
        };
        let any = |_: &str| true;
        let (h0, h1, h2) = (find(0, &any), find(1, &any), find(2, &any));
        // a height-0 key sorting before every "c.d.e/…" key
        let low0 = (0..100_000)
            .map(|i| format!("a/{i}"))
            .find(|k| key_height(k.as_bytes()) == 0)
            .unwrap();

        // mixed layers in one node
        let (lo, hi) = if h0 < h1 { (&h0, &h1) } else { (&h1, &h0) };
        let p = common_prefix_len(lo.as_bytes(), hi.as_bytes());
        let mut second = entry(hi);
        second.prefix_len = p;
        second.key_suffix = hi.as_bytes()[p..].to_vec();
        let (r, b) = forged_root(&Node {
            left: None,
            entries: vec![entry(lo), second],
        });
        assert_eq!(prove(&r, lo.as_bytes(), &b), Err(MstError::Layer));

        // a layer-0 child under a layer-1 root is fine
        let (c, mut blocks) = forged_root(&Node {
            left: None,
            entries: vec![entry(&low0)],
        });
        let (r, b) = forged_root(&Node {
            left: Some(c),
            entries: vec![entry(&h1)],
        });
        blocks.extend(b);
        assert_eq!(
            prove(&r, low0.as_bytes(), &blocks).unwrap(),
            Proof::Present(v)
        );
        assert_eq!(list_entries(&r, &blocks, 10).unwrap().len(), 2);

        // a layer-0 node may not have children
        let (r, b) = forged_root(&Node {
            left: Some(c),
            entries: vec![entry(&h0)],
        });
        blocks.extend(b);
        assert_eq!(prove(&r, h0.as_bytes(), &blocks), Err(MstError::Layer));

        // a subtree may not skip a layer (layer-2 root -> layer-0 child)
        let (r, b) = forged_root(&Node {
            left: Some(c),
            entries: vec![entry(&h2)],
        });
        blocks.extend(b);
        assert_eq!(prove(&r, low0.as_bytes(), &blocks), Err(MstError::Layer));

        // a left subtree holding a key greater than its parent's first key
        let above = find(0, &|k| k > h1.as_str());
        let (mc, b) = forged_root(&Node {
            left: None,
            entries: vec![entry(&above)],
        });
        blocks.extend(b);
        let (r, b) = forged_root(&Node {
            left: Some(mc),
            entries: vec![entry(&h1)],
        });
        blocks.extend(b);
        // walking left (toward a smaller key) visits the misplaced node and rejects it
        assert_eq!(prove(&r, low0.as_bytes(), &blocks), Err(MstError::Ordering));
        // walking right never sees it: the right slot is empty, so the key is absent
        assert_eq!(prove(&r, above.as_bytes(), &blocks).unwrap(), Proof::Absent);
        assert_eq!(list_entries(&r, &blocks, 10), Err(MstError::Ordering));
    }
}
