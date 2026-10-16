// SPDX-License-Identifier: MIT OR Apache-2.0
//! Merkle Search Tree (`/specs/repository`, "MST Structure"): node codec, key heights,
//! inclusion/exclusion proofs and a deterministic builder.
//!
//! Node schema: `{l: link|null, e: [{p: int, k: bytes, v: link, t: link|null}]}`. Keys are
//! prefix-compressed within a node (`p` = bytes reused from the previous key). A key's layer is
//! the number of leading zero bits of `sha256(key)` divided by two (fanout 4).
//!
//! [`prove`] and [`list_entries`] implement the `proofs/1` walk of `sync.md` §8.5 exactly, so that
//! every client reaches the same verdict on the same blocks, malformed trees included:
//!
//! ```text
//! walk(cid, target, depth, lo, hi, ceiling)
//!   depth ≤ MAX_DEPTH (the root is depth 0)                      else DepthExceeded
//!   node = strict_decode(block(cid))                             missing → MissingBlock
//!   keys: k_0 has p = 0; k_i = k_{i−1}[..p_i] ‖ suffix_i, p_i ≤ |k_{i−1}|; every key valid;
//!         strictly increasing; lo < k_i < hi
//!   entries: one layer λ = layer(k_i) for all i, λ < ceiling;  ceiling' = λ
//!   no entries: l ≠ null unless depth = 0;                      ceiling' = ceiling
//!   found → Present(v); else descend into the gap's subtree; a null subtree → Absent
//! ```
//!
//! The walk always descends to the target's gap: there is no early exit when the target's layer
//! is at or above the node's, so a proof must contain the whole search path (as the reference
//! implementation's covering proofs do). Subtree links are only checked when followed (their
//! block must be present, dag-cbor, and hash to the link). Errors on structure are
//! [`MstError::Malformed`], [`MstError::InvalidKey`], [`MstError::Ordering`],
//! [`MstError::PrefixNotCanonical`] and [`MstError::Layer`] (`MstInvalid` in `sync.md` terms).
//!
//! [`build`] produces the canonical atproto tree (exact layers, empty intermediate nodes where a
//! level would be skipped, maximal prefix compression), which the walk accepts.

use std::collections::{BTreeMap, HashMap};

use ::cid::Cid;

use crate::Ipld;
use crate::car::Car;
use crate::cid::CidError;
use crate::dagcbor::{decode_scalar, list_items_raw, map_entries_raw};
use crate::datamodel::{DataModelError, Limits};
use crate::syntax::{Nsid, RecordKey};

/// Maximum repo path (MST key) length accepted.
pub const MAX_KEY_LEN: usize = 1024;
/// Maximum node depth on a path (`MST_DEPTH_MAX`): the root is depth 0, so a path visits at most
/// `MAX_DEPTH + 1` nodes.
pub const MAX_DEPTH: usize = 32;

/// MST errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MstError {
    /// A needed node block is not available.
    #[error("missing MST block {0}")]
    MissingBlock(Cid),
    /// A node block is not strict DAG-CBOR.
    #[error("MST node is not valid DAG-CBOR: {0}")]
    Decode(DataModelError),
    /// A node does not match the schema.
    #[error("malformed MST node: {0}")]
    Malformed(&'static str),
    /// A followed link is not a dag-cbor CID, or a block does not hash to its CID.
    #[error("MST link: {0}")]
    Cid(CidError),
    /// A key is not a valid repo path.
    #[error("invalid MST key: {0}")]
    InvalidKey(&'static str),
    /// Keys are not strictly increasing, or fall outside their subtree's range.
    #[error("MST key ordering violation")]
    Ordering,
    /// A prefix length is impossible: the first entry's is not 0, or a later one exceeds the
    /// previous key's length.
    #[error("MST key prefix length is invalid")]
    PrefixNotCanonical,
    /// A node mixes layers, or its layer is not below its parent's.
    #[error("MST layer violation")]
    Layer,
    /// The path is deeper than [`MAX_DEPTH`].
    #[error("MST deeper than {0}")]
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

/// `link | null` (an absent field counts as `null`).
fn opt_link(raw: Option<&[u8]>, what: &'static str) -> Result<Option<Cid>, MstError> {
    match raw.map(decode_scalar) {
        None | Some(Some(Ipld::Null)) => Ok(None),
        Some(Some(Ipld::Link(c))) => Ok(Some(c)),
        Some(_) => Err(MstError::Malformed(what)),
    }
}

fn decode_entry(raw: &[u8]) -> Result<TreeEntry, MstError> {
    let fields = map_entries_raw(raw).ok_or(MstError::Malformed("entry is not a map"))?;
    let (mut p, mut k, mut v, mut t) = (None, None, None, None);
    for (key, val) in fields {
        match key {
            "p" => p = Some(val),
            "k" => k = Some(val),
            "v" => v = Some(val),
            "t" => t = Some(val),
            _ => {}
        }
    }
    let prefix_len = match p.and_then(decode_scalar) {
        Some(Ipld::Integer(p)) if p >= 0 => usize::try_from(p).unwrap_or(usize::MAX),
        _ => {
            return Err(MstError::Malformed(
                "p must be a non-negative integer",
            ));
        }
    };
    let Some(Ipld::Bytes(key_suffix)) = k.and_then(decode_scalar) else {
        return Err(MstError::Malformed("k must be bytes"));
    };
    let Some(Ipld::Link(value)) = v.and_then(decode_scalar) else {
        return Err(MstError::Malformed("v must be a link"));
    };
    let tree = opt_link(t, "t must be a link or null")?;
    Ok(TreeEntry {
        prefix_len,
        key_suffix,
        value,
        tree,
    })
}

impl Node {
    /// Decodes a node block: strict DAG-CBOR (`sync.md` §8.3, nesting ≤ 32, checked without
    /// allocating), then the schema. Unknown fields are ignored; links are not checked until
    /// followed. See [`Node::keys`] for key validation.
    pub fn decode(bytes: &[u8]) -> Result<Self, MstError> {
        crate::dagcbor::validate_with(bytes, &Limits::STRICT).map_err(MstError::Decode)?;
        let fields = map_entries_raw(bytes).ok_or(MstError::Malformed("node is not a map"))?;
        let (mut l, mut e) = (None, None);
        for (key, val) in fields {
            match key {
                "l" => l = Some(val),
                "e" => e = Some(val),
                _ => {}
            }
        }
        let left = opt_link(l, "l must be a link or null")?;
        let items = e
            .and_then(list_items_raw)
            .ok_or(MstError::Malformed("e must be a list"))?;
        let mut entries = Vec::new();
        for item in items {
            entries.push(decode_entry(item)?);
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
        // Depth 3, no floats, no large integers, no size caps: encoding cannot fail.
        crate::dagcbor::encode_with(&self.to_ipld(), &Limits::uncapped(8)).unwrap_or_default()
    }

    /// Reconstructs the full keys: the first entry's `p` must be 0 and every `p` at most the
    /// previous key's length; every key must be valid ([`validate_key`]) and keys strictly
    /// increasing. (Maximal prefix compression is what writers produce, but `sync.md` §8.5 does
    /// not require it of readers.)
    pub fn keys(&self) -> Result<Vec<Vec<u8>>, MstError> {
        let mut out: Vec<Vec<u8>> = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            let prev: &[u8] = out.last().map_or(&[], Vec::as_slice);
            if e.prefix_len > prev.len() {
                // Includes a first entry with p ≠ 0 (there is no previous key).
                return Err(MstError::PrefixNotCanonical);
            }
            let mut key = Vec::with_capacity(e.prefix_len + e.key_suffix.len());
            key.extend_from_slice(&prev[..e.prefix_len]);
            key.extend_from_slice(&e.key_suffix);
            validate_key(&key)?;
            if !out.is_empty() && key.as_slice() <= prev {
                return Err(MstError::Ordering);
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

/// Validates an MST key as `sync.md` §8.5 defines it: at most [`MAX_KEY_LEN`] bytes, exactly one
/// `/`, and otherwise only `[A-Za-z0-9._:~-]`. (Segments are not required to be a valid NSID or
/// record key; use [`repo_path`] to build keys from validated parts.)
pub fn validate_key(key: &[u8]) -> Result<(), MstError> {
    if key.len() > MAX_KEY_LEN {
        return Err(MstError::InvalidKey("length"));
    }
    let mut slashes = 0usize;
    for &b in key {
        if b == b'/' {
            slashes += 1;
        } else if !crate::syntax::is_rkey_byte(b) {
            return Err(MstError::InvalidKey("invalid character"));
        }
    }
    if slashes != 1 {
        return Err(MstError::InvalidKey("must contain exactly one '/'"));
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

impl<T: BlockSource + ?Sized> BlockSource for &T {
    fn get_block(&self, cid: &Cid) -> Option<&[u8]> {
        (**self).get_block(cid)
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

/// A node loaded and checked on its own (block, schema, keys, one layer).
struct Loaded {
    node: Node,
    keys: Vec<Vec<u8>>,
    layer: Option<u32>,
}

fn load(cid: &Cid, blocks: &impl BlockSource) -> Result<Loaded, MstError> {
    let bytes = blocks.get_block(cid).ok_or(MstError::MissingBlock(*cid))?;
    crate::cid::check_blessed_dag_cbor(cid).map_err(MstError::Cid)?;
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

/// Checks a loaded node at `depth` against the bounds and ceiling implied by its parent; returns
/// the ceiling for its children (`None` = unbounded).
fn check_node(
    n: &Loaded,
    depth: usize,
    ceiling: Option<u32>,
    low: Option<&[u8]>,
    high: Option<&[u8]>,
) -> Result<Option<u32>, MstError> {
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
    match n.layer {
        Some(l) if ceiling.is_some_and(|c| l >= c) => Err(MstError::Layer),
        Some(l) => Ok(Some(l)),
        None if depth > 0 && n.node.left.is_none() => Err(MstError::Malformed(
            "empty node below the root without a subtree",
        )),
        None => Ok(ceiling),
    }
}

/// Walks from `root` toward `key` using only `blocks`, proving presence or absence
/// (`sync.md` §8.5; see the module docs). `key` itself must be a valid MST key.
pub fn prove(root: &Cid, key: &[u8], blocks: &impl BlockSource) -> Result<Proof, MstError> {
    validate_key(key)?;
    let mut cid = *root;
    let mut ceiling: Option<u32> = None;
    let mut low: Option<Vec<u8>> = None;
    let mut high: Option<Vec<u8>> = None;
    let mut depth = 0usize;
    loop {
        if depth > MAX_DEPTH {
            return Err(MstError::DepthExceeded(MAX_DEPTH));
        }
        let n = load(&cid, blocks)?;
        ceiling = check_node(&n, depth, ceiling, low.as_deref(), high.as_deref())?;
        let idx = n.keys.partition_point(|k| k.as_slice() < key);
        if n.keys.get(idx).is_some_and(|k| k.as_slice() == key) {
            return Ok(Proof::Present(n.node.entries[idx].value));
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
        cid = child;
        depth += 1;
    }
}

/// Lists every `(key, value)` in the tree in key order, validating every node with the rules of
/// [`prove`]. Visits at most `max_nodes` nodes (a cap on work for untrusted input).
pub fn list_entries(
    root: &Cid,
    blocks: &impl BlockSource,
    max_nodes: usize,
) -> Result<Vec<(Vec<u8>, Cid)>, MstError> {
    // Frame: either a node to expand (with constraints) or an entry to emit.
    enum Item {
        Node {
            cid: Cid,
            ceiling: Option<u32>,
            low: Option<Vec<u8>>,
            high: Option<Vec<u8>>,
            depth: usize,
        },
        Entry(Vec<u8>, Cid),
    }
    let mut out = Vec::new();
    let mut stack = vec![Item::Node {
        cid: *root,
        ceiling: None,
        low: None,
        high: None,
        depth: 0,
    }];
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        let (cid, ceiling, low, high, depth) = match item {
            Item::Entry(k, v) => {
                out.push((k, v));
                continue;
            }
            Item::Node {
                cid,
                ceiling,
                low,
                high,
                depth,
            } => (cid, ceiling, low, high, depth),
        };
        visited += 1;
        if visited > max_nodes {
            return Err(MstError::Malformed("too many nodes"));
        }
        if depth > MAX_DEPTH {
            return Err(MstError::DepthExceeded(MAX_DEPTH));
        }
        let n = load(&cid, blocks)?;
        let child_ceiling = check_node(&n, depth, ceiling, low.as_deref(), high.as_deref())?;
        // Push in reverse so items pop in key order: left, e0, t0, e1, t1, …
        let mut frames = Vec::new();
        let child =
            |c: Option<Cid>, lo: Option<Vec<u8>>, hi: Option<Vec<u8>>, frames: &mut Vec<Item>| {
                if let Some(c) = c {
                    frames.push(Item::Node {
                        cid: c,
                        ceiling: child_ceiling,
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

/// Builds the canonical MST for `entries` (keys must be valid MST keys; later duplicates win).
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
        // sync.md §8.5 only asks for one '/' and the charset, so these are valid MST keys
        for ok in [&b"/x"[..], b"x/", b"/", b"a/..", b"./a", b"~:-_./."] {
            assert!(validate_key(ok).is_ok(), "{:?}", String::from_utf8_lossy(ok));
        }
        assert!(validate_key(&[b'a'; MAX_KEY_LEN - 2].iter().chain(b"/b").copied().collect::<Vec<_>>()).is_ok());
        for bad in [
            &b""[..],
            b"noslash",
            b"a/b/c",
            b"//",
            b"a/b c",
            b"a/b#",
            "a/\u{e9}".as_bytes(),
        ] {
            assert!(
                validate_key(bad).is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
        let mut long = vec![b'a'; MAX_KEY_LEN];
        long[0] = b'/';
        assert!(validate_key(&long).is_ok());
        long.push(b'a');
        assert_eq!(validate_key(&long), Err(MstError::InvalidKey("length")));
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
        // a shorter-than-maximal prefix still reconstructs the key (sync.md §8.5 asks only
        // p ≤ |previous key|)
        let short_prefix = Node {
            left: None,
            entries: vec![e(0, &ks[0]), e(1, &ks[1][1..])],
        };
        let (r, b) = forged_root(&short_prefix);
        assert_eq!(
            prove(&r, ks[1].as_bytes(), &b).unwrap(),
            Proof::Present(v)
        );
        let long_prefix = Node {
            left: None,
            entries: vec![e(0, &ks[0]), e(ks[0].len() + 1, "1")],
        };
        let (r, b) = forged_root(&long_prefix);
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
        // an empty root that only points to a subtree is walked (sync.md §8.5 allows it)
        let (inner, mut b) = forged_root(&good);
        let top = Node {
            left: Some(inner),
            entries: vec![],
        };
        let (r, b2) = forged_root(&top);
        b.extend(b2);
        assert_eq!(prove(&r, ks[0].as_bytes(), &b).unwrap(), Proof::Present(v));
        assert_eq!(list_entries(&r, &b, 10).unwrap().len(), 3);
        // but an empty node below the root must point to a subtree
        let (dead, mut b) = forged_root(&Node::default());
        let (r, b2) = forged_root(&Node {
            left: Some(dead),
            entries: vec![],
        });
        b.extend(b2);
        assert!(matches!(
            prove(&r, b"a/1", &b),
            Err(MstError::Malformed(_))
        ));
        // wrong field types
        let mut m = BTreeMap::new();
        m.insert("e".to_owned(), Ipld::Integer(1));
        let bytes = crate::dagcbor::encode(&Ipld::Map(m)).unwrap();
        assert!(matches!(Node::decode(&bytes), Err(MstError::Malformed(_))));
        assert!(matches!(Node::decode(&[0xff]), Err(MstError::Decode(_))));
        // subtree links are checked only when followed
        let raw = crate::cid::for_raw(b"x");
        let raw_link = Node {
            left: Some(raw),
            entries: vec![],
        };
        assert_eq!(Node::decode(&raw_link.encode()).unwrap(), raw_link);
        let mut t = good.clone();
        t.entries[2].tree = Some(raw);
        let (r, mut b) = forged_root(&t);
        // not on the path to ks[0]
        assert_eq!(prove(&r, ks[0].as_bytes(), &b).unwrap(), Proof::Present(v));
        // on the path: missing, then present but not dag-cbor
        assert_eq!(prove(&r, b"a/zzz", &b), Err(MstError::MissingBlock(raw)));
        b.insert(raw, b"x".to_vec());
        assert_eq!(
            prove(&r, b"a/zzz", &b),
            Err(MstError::Cid(CidError::NotBlessed))
        );
        // unknown fields are ignored (at any depth up to 32), but must be strict DAG-CBOR
        let mut m = match good.to_ipld() {
            Ipld::Map(m) => m,
            _ => unreachable!(),
        };
        let mut deep = Ipld::List(vec![]);
        for _ in 0..30 {
            deep = Ipld::List(vec![deep]);
        }
        m.insert("zz".to_owned(), deep.clone());
        let bytes = crate::dagcbor::encode(&Ipld::Map(m.clone())).unwrap();
        assert_eq!(Node::decode(&bytes).unwrap(), good);
        m.insert("zz".to_owned(), Ipld::List(vec![deep]));
        let bytes = crate::dagcbor::encode_with(&Ipld::Map(m), &Limits::uncapped(64)).unwrap();
        assert!(matches!(
            Node::decode(&bytes),
            Err(MstError::Decode(DataModelError::DepthExceeded { max: 32 }))
        ));
        // more than 1,024 entries is fine
        let many: Vec<String> = (0..20_000)
            .map(|i| format!("a/{i:05}"))
            .filter(|k| key_height(k.as_bytes()) == 0)
            .take(1_100)
            .collect();
        let mut n = Node::default();
        let mut prev: &[u8] = &[];
        for k in &many {
            let p = common_prefix_len(prev, k.as_bytes());
            n.entries.push(e(p, &k[p..]));
            prev = k.as_bytes();
        }
        let (r, b) = forged_root(&n);
        assert_eq!(
            prove(&r, many[1_099].as_bytes(), &b).unwrap(),
            Proof::Present(v)
        );
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

        // a layer-0 node's subtree cannot hold any key: following it fails, but a key found in
        // the node itself is proven (sync.md §8.5 only checks what the walk visits)
        let (r, b) = forged_root(&Node {
            left: Some(c),
            entries: vec![entry(&h0)],
        });
        blocks.extend(b);
        assert_eq!(prove(&r, h0.as_bytes(), &blocks).unwrap(), Proof::Present(v));
        assert_eq!(prove(&r, low0.as_bytes(), &blocks), Err(MstError::Layer));
        // a child at the same layer as its parent
        let (r, b) = forged_root(&Node {
            left: Some(c),
            entries: vec![entry(&find(0, &|k| k > low0.as_str()))],
        });
        blocks.extend(b);
        assert_eq!(prove(&r, low0.as_bytes(), &blocks), Err(MstError::Layer));
        // a child above its parent
        let above_parent = find(2, &|k| k > h1.as_str());
        let (c2, b) = forged_root(&Node {
            left: None,
            entries: vec![entry(&above_parent)],
        });
        blocks.extend(b);
        let mut top = entry(&h1);
        top.tree = Some(c2);
        let (r, b) = forged_root(&Node {
            left: None,
            entries: vec![top],
        });
        blocks.extend(b);
        assert_eq!(
            prove(&r, above_parent.as_bytes(), &blocks),
            Err(MstError::Layer)
        );

        // a subtree may skip layers (layer-2 root -> layer-0 child): only λ < ceiling is required
        let (r, b) = forged_root(&Node {
            left: Some(c),
            entries: vec![entry(&h2)],
        });
        blocks.extend(b);
        assert_eq!(prove(&r, low0.as_bytes(), &blocks).unwrap(), Proof::Present(v));
        assert_eq!(list_entries(&r, &blocks, 10).unwrap().len(), 2);

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

    /// A chain of `n` empty nodes (each pointing to the next) above `bottom`.
    fn empty_chain(bottom: Cid, n: usize, blocks: &mut BTreeMap<Cid, Vec<u8>>) -> Cid {
        let mut top = bottom;
        for _ in 0..n {
            let (c, b) = forged_root(&Node {
                left: Some(top),
                entries: vec![],
            });
            blocks.extend(b);
            top = c;
        }
        top
    }

    #[test]
    fn depth_limit_boundary() {
        // MST_DEPTH_MAX = 32 with the root at depth 0: a key at depth 32 is reachable, one at
        // depth 33 is not, whether present or absent
        let v = leaf();
        let k = height0_keys(1).remove(0);
        let (bottom, mut blocks) = forged_root(&Node {
            left: None,
            entries: vec![TreeEntry {
                prefix_len: 0,
                key_suffix: k.as_bytes().to_vec(),
                value: v,
                tree: None,
            }],
        });
        let at_32 = empty_chain(bottom, 32, &mut blocks);
        assert_eq!(prove(&at_32, k.as_bytes(), &blocks).unwrap(), Proof::Present(v));
        assert_eq!(prove(&at_32, b"a/zzzz", &blocks).unwrap(), Proof::Absent);
        assert_eq!(list_entries(&at_32, &blocks, 100).unwrap().len(), 1);
        let at_33 = empty_chain(bottom, 33, &mut blocks);
        assert_eq!(
            prove(&at_33, k.as_bytes(), &blocks),
            Err(MstError::DepthExceeded(MAX_DEPTH))
        );
        assert_eq!(
            list_entries(&at_33, &blocks, 100),
            Err(MstError::DepthExceeded(MAX_DEPTH))
        );
        // the depth check comes before the block lookup
        let mut no_bottom = blocks.clone();
        no_bottom.remove(&bottom);
        assert_eq!(
            prove(&at_33, k.as_bytes(), &no_bottom),
            Err(MstError::DepthExceeded(MAX_DEPTH))
        );
        assert_eq!(
            prove(&at_32, k.as_bytes(), &no_bottom),
            Err(MstError::MissingBlock(bottom))
        );
    }

    #[test]
    fn no_early_exit_on_high_keys() {
        // A target whose layer is at or above the root's still needs the whole search path down
        // to a missing child (sync.md §8.5): with only the root block, the walk cannot finish.
        let v = leaf();
        let ks = keys(400);
        let (root, blocks) = build(ks.iter().map(|k| (k.as_bytes(), v))).unwrap();
        let root_node = Node::decode(&blocks[&root]).unwrap();
        let root_layer = key_height(&root_node.keys().unwrap()[0]);
        let target = (0..2_000_000)
            .map(|i| format!("com.example.record/{i:06}x"))
            .find(|k| key_height(k.as_bytes()) > root_layer)
            .unwrap();
        let mut only_root = BTreeMap::new();
        only_root.insert(root, blocks[&root].clone());
        assert!(matches!(
            prove(&root, target.as_bytes(), &only_root),
            Err(MstError::MissingBlock(_))
        ));
        assert_eq!(prove(&root, target.as_bytes(), &blocks).unwrap(), Proof::Absent);
    }
}
