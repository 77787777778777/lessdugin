// SPDX-License-Identifier: AGPL-3.0-or-later
//! The MST proof walk (`lessdugin_atp::mst::prove`, `list_entries`; `sync.md` §8.5) and the whole
//! `proofs/1` pipeline over structured repos: random bytes almost never hash to the CIDs that link
//! a repo together, so this target builds the blocks from a structured input and computes every
//! link itself. The tree is either the canonical tree of a key set (`mst::build`) or hand-built
//! nodes (any shape: wrong layers, bad prefixes, wrong field types, dangling or wrong-codec links,
//! long chains); the commit is signed with a known key (or deliberately not); blocks can then be
//! dropped, corrupted or duplicated, and the result is verified as a `getRecord` CAR, through a
//! read-capped block source, and as an indexer proof bundle.
//!
//! Contract: never panics. For a canonical tree with a valid commit and no damage, the walk is
//! also exact: every key is proven present with its value, other keys absent, the listing is the
//! key set, `verify_proof` proves the record CID, and a bundle item verifies iff its claim is
//! true.

#![no_main]

use std::collections::BTreeMap;

use arbitrary::Arbitrary;
use lessdugin_atp::car::write_car;
use lessdugin_atp::commit::UnsignedCommit;
use lessdugin_atp::mst::{self, Proof};
use lessdugin_atp::proof::{
    BundleCaps, ProofBundle, ProofCaps, ProofErrorKind, ReadCaps, verify_proof, verify_proof_in,
};
use lessdugin_atp::syntax::{AtUri, Did, Nsid, RecordKey, Tid};
use lessdugin_atp::{Cid, Ipld, cid, dagcbor, datamodel::Limits};
use lessdugin_fuzz::common::{COLLECTIONS, NOW, OTHER_DID, REV_MICROS, RKEY_ALPHABET, rkey_from};
use lessdugin_fuzz::{FUZZ_DID, FUZZ_KEY, sign};
use libfuzzer_sys::fuzz_target;

const MAX_KEYS: usize = 256;
const MAX_NODES: usize = 64;

#[derive(Arbitrary, Debug)]
struct Input {
    tree: Tree,
    records: Vec<RecordSpec>,
    commit: CommitSpec,
    targets: Vec<Target>,
    damage: Vec<Damage>,
    read_caps: (u16, u16),
    bundle: Vec<(Target, Claim)>,
}

#[derive(Arbitrary, Debug)]
enum Tree {
    /// The canonical tree of these keys.
    Canonical(Vec<KeySpec>),
    /// Hand-built nodes; `nodes[0]` is the root. A node may link only to later nodes (links to
    /// itself or earlier ones become dangling CIDs), which keeps every hash computable.
    Nodes(Vec<NodeSpec>),
}

#[derive(Arbitrary, Debug)]
struct KeySpec {
    coll: u8,
    rkey: Vec<u8>,
    record: u8,
}

#[derive(Arbitrary, Debug)]
struct NodeSpec {
    left: Link,
    entries: Vec<EntrySpec>,
    /// An unknown field (ignored by readers).
    extra: bool,
    /// Replaces the node's encoding (still hashed and linked).
    raw: Option<Vec<u8>>,
}

#[derive(Arbitrary, Debug)]
struct EntrySpec {
    /// 0–250: `p` itself; 251: negative; 252: huge; 253: absent; 254: a string; 255: null.
    p: u8,
    /// Key bytes: mostly the key alphabet and `/`, sometimes a raw byte.
    k: Vec<u8>,
    /// Write `k` as a string instead of bytes.
    k_as_string: bool,
    v: Link,
    t: Link,
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum Link {
    Null,
    Absent,
    Node(u8),
    Record(u8),
    /// A dag-cbor CID with no block.
    Dangling(u8),
    /// A raw-codec CID (of a present raw block).
    RawCodec,
    /// Not a link at all.
    Integer,
}

#[derive(Arbitrary, Debug)]
enum RecordSpec {
    Text(String),
    Raw(Vec<u8>),
}

#[derive(Arbitrary, Debug)]
struct CommitSpec {
    other_did: bool,
    /// Seconds relative to `REV_MICROS`; `i32::MAX` is far in the future of `NOW`.
    rev_offset: i32,
    prev: Option<u8>,
    sig: SigSpec,
    /// `data` other than the tree root.
    data: Option<Link>,
    /// Replaces the commit block.
    raw: Option<Vec<u8>>,
}

#[derive(Arbitrary, Debug)]
enum SigSpec {
    Valid,
    Flip(u8, u8),
    HighS,
    Raw(Vec<u8>),
}

#[derive(Arbitrary, Debug, Clone)]
enum Target {
    /// The i-th key of the tree.
    Key(u8),
    /// A key built from parts.
    Fresh { coll: u8, rkey: Vec<u8> },
    /// Any bytes.
    Bytes(Vec<u8>),
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum Claim {
    Actual,
    Absent,
    Other(u8),
    MalformedUri,
    MalformedCid,
}

#[derive(Arbitrary, Debug)]
enum Damage {
    Drop(u8),
    Flip { block: u8, pos: u16, xor: u8 },
    Duplicate(u8),
    ExtraRoot,
}

/// n − s for the secp256k1 group order n (a high-S twin of a low-S signature).
fn negate_s(sig: &mut [u8]) {
    const N: [u8; 32] = [
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFE, 0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B, 0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36,
        0x41, 0x41,
    ];
    if sig.len() != 64 {
        return;
    }
    let s = &mut sig[32..];
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let mut d = i16::from(N[i]) - i16::from(s[i]) - borrow;
        borrow = i16::from(d < 0);
        if d < 0 {
            d += 256;
        }
        s[i] = d as u8;
    }
}

fn key_byte(b: u8) -> u8 {
    if b >= 0xF0 {
        b // outside the alphabet (an invalid key)
    } else if b.is_multiple_of(23) {
        b'/'
    } else {
        RKEY_ALPHABET[usize::from(b) % RKEY_ALPHABET.len()]
    }
}

fn fresh_key(coll: u8, rkey: &[u8]) -> String {
    format!(
        "{}/{}",
        COLLECTIONS[usize::from(coll) % COLLECTIONS.len()],
        rkey_from(rkey)
    )
}

fn split_key(key: &[u8]) -> Option<(Nsid, RecordKey)> {
    let s = std::str::from_utf8(key).ok()?;
    let (c, r) = s.split_once('/')?;
    Some((Nsid::parse(c).ok()?, RecordKey::parse(r).ok()?))
}

struct Repo {
    blocks: BTreeMap<Cid, Vec<u8>>,
    /// Blocks in CAR order (commit first).
    order: Vec<Cid>,
    commit_cid: Cid,
    root: Cid,
    /// Canonical mode: the expected entries.
    entries: Option<BTreeMap<Vec<u8>, Cid>>,
    /// Every key the tree mentions (for `Target::Key`).
    keys: Vec<Vec<u8>>,
    /// Records CIDs (for `Claim::Other`).
    record_cids: Vec<Cid>,
    /// No damage, a valid commit and only well-formed records.
    clean: bool,
}

fn link_ipld(l: Link, nodes: &[Option<Cid>], at: usize, records: &[Cid], raw: Cid) -> Option<Ipld> {
    Some(match l {
        Link::Null => Ipld::Null,
        Link::Absent => return None,
        Link::Node(j) => {
            let j = usize::from(j);
            match nodes.get(j).copied().flatten() {
                Some(c) if j > at => Ipld::Link(c),
                _ => Ipld::Link(cid::for_dag_cbor(&[0xa0, j as u8])),
            }
        }
        Link::Record(i) if !records.is_empty() => {
            Ipld::Link(records[usize::from(i) % records.len()])
        }
        Link::Record(i) | Link::Dangling(i) => Ipld::Link(cid::for_dag_cbor(&[0xa1, i])),
        Link::RawCodec => Ipld::Link(raw),
        Link::Integer => Ipld::Integer(7),
    })
}

/// Adds a dag-cbor block (in CAR order); returns its CID.
fn add(bytes: Vec<u8>, blocks: &mut BTreeMap<Cid, Vec<u8>>, order: &mut Vec<Cid>) -> Cid {
    let c = cid::for_dag_cbor(&bytes);
    if blocks.insert(c, bytes).is_none() {
        order.push(c);
    }
    c
}

fn encode(v: &Ipld) -> Vec<u8> {
    dagcbor::encode_with(v, &Limits::uncapped(16)).unwrap_or_default()
}

fn build(input: &Input) -> Repo {
    let mut blocks = BTreeMap::new();
    let mut order = Vec::new();
    let mut clean = true;

    // Records.
    let mut record_cids = Vec::new();
    let default_record = RecordSpec::Text(String::new());
    let specs: Vec<&RecordSpec> = if input.records.is_empty() {
        vec![&default_record]
    } else {
        input.records.iter().take(32).collect()
    };
    for r in specs {
        let bytes = match r {
            RecordSpec::Text(t) => {
                let mut m = BTreeMap::new();
                m.insert("$type".to_owned(), Ipld::String("app.x.record".into()));
                m.insert("text".to_owned(), Ipld::String(t.clone()));
                encode(&Ipld::Map(m))
            }
            RecordSpec::Raw(b) => {
                if dagcbor::validate_with(b, &Limits::STRICT).is_err() {
                    clean = false;
                }
                b.clone()
            }
        };
        record_cids.push(add(bytes, &mut blocks, &mut order));
    }
    let raw_block = b"raw".to_vec();
    let raw_cid = cid::for_raw(&raw_block);
    blocks.insert(raw_cid, raw_block);
    order.push(raw_cid);

    // The tree.
    let (root, entries, keys) = match &input.tree {
        Tree::Canonical(specs) => {
            let mut want = BTreeMap::new();
            for s in specs.iter().take(MAX_KEYS) {
                let key = fresh_key(s.coll, &s.rkey).into_bytes();
                want.insert(key, record_cids[usize::from(s.record) % record_cids.len()]);
            }
            let (root, nodes) =
                mst::build(want.iter().map(|(k, v)| (k.as_slice(), *v))).expect("valid keys");
            for (c, b) in nodes {
                if blocks.insert(c, b).is_none() {
                    order.push(c);
                }
            }
            let keys = want.keys().cloned().collect();
            (root, Some(want), keys)
        }
        Tree::Nodes(specs) => {
            let specs = &specs[..specs.len().min(MAX_NODES)];
            let mut cids: Vec<Option<Cid>> = vec![None; specs.len()];
            let mut keys = Vec::new();
            for (i, n) in specs.iter().enumerate().rev() {
                let bytes = match &n.raw {
                    Some(raw) => raw.clone(),
                    None => {
                        let mut m = BTreeMap::new();
                        if let Some(l) = link_ipld(n.left, &cids, i, &record_cids, raw_cid) {
                            m.insert("l".to_owned(), l);
                        }
                        let mut es = Vec::new();
                        for e in n.entries.iter().take(32) {
                            let mut em = BTreeMap::new();
                            let p = match e.p {
                                0..=250 => Some(Ipld::Integer(e.p.into())),
                                251 => Some(Ipld::Integer(-1)),
                                252 => Some(Ipld::Integer(i64::MAX.into())),
                                253 => None,
                                254 => Some(Ipld::String("0".into())),
                                _ => Some(Ipld::Null),
                            };
                            if let Some(p) = p {
                                em.insert("p".to_owned(), p);
                            }
                            let k: Vec<u8> = e.k.iter().take(80).map(|b| key_byte(*b)).collect();
                            em.insert(
                                "k".to_owned(),
                                if e.k_as_string {
                                    Ipld::String(String::from_utf8_lossy(&k).into_owned())
                                } else {
                                    Ipld::Bytes(k)
                                },
                            );
                            if let Some(v) = link_ipld(e.v, &cids, i, &record_cids, raw_cid) {
                                em.insert("v".to_owned(), v);
                            }
                            if let Some(t) = link_ipld(e.t, &cids, i, &record_cids, raw_cid) {
                                em.insert("t".to_owned(), t);
                            }
                            es.push(Ipld::Map(em));
                        }
                        m.insert("e".to_owned(), Ipld::List(es));
                        if n.extra {
                            m.insert("zz".to_owned(), Ipld::Bool(true));
                        }
                        encode(&Ipld::Map(m))
                    }
                };
                if let Ok(node) = mst::Node::decode(&bytes)
                    && let Ok(ks) = node.keys()
                {
                    keys.extend(ks);
                }
                cids[i] = Some(add(bytes, &mut blocks, &mut order));
            }
            let root = cids.first().copied().flatten().unwrap_or_else(|| {
                add(
                    encode(&mst::Node::default().to_ipld()),
                    &mut blocks,
                    &mut order,
                )
            });
            (root, None, keys)
        }
    };

    // The commit.
    let c = &input.commit;
    let did = if c.other_did {
        clean = false;
        Did::parse(OTHER_DID).expect("DID")
    } else {
        FUZZ_DID.clone()
    };
    let rev_micros = (REV_MICROS as i64).saturating_add(i64::from(c.rev_offset) * 1_000_000);
    let rev = Tid::from_parts(rev_micros.clamp(0, (1 << 53) - 1) as u64, 0)
        .unwrap_or_else(|_| Tid::from_u64(0));
    if rev.timestamp_micros() as i64 > NOW + lessdugin_atp::commit::MAX_FUTURE_SKEW_MICROS {
        clean = false;
    }
    let data = match c.data {
        None => root,
        Some(l) => {
            clean = false;
            match link_ipld(l, &[], 0, &record_cids, raw_cid) {
                Some(Ipld::Link(x)) => x,
                _ => root,
            }
        }
    };
    let unsigned = UnsignedCommit {
        did,
        data,
        rev,
        prev: c.prev.map(|i| cid::for_dag_cbor(&[0xa2, i])),
    };
    let mut sig = sign(&unsigned.signing_bytes());
    match &c.sig {
        SigSpec::Valid => {}
        SigSpec::Flip(pos, x) => {
            if *x != 0 {
                clean = false;
            }
            sig[usize::from(*pos) % 64] ^= x;
        }
        SigSpec::HighS => {
            clean = false;
            negate_s(&mut sig);
        }
        SigSpec::Raw(b) => {
            clean = false;
            sig = b.clone();
        }
    }
    let commit_bytes = match &c.raw {
        Some(raw) => {
            clean = false;
            raw.clone()
        }
        None => unsigned.signed(&sig).1,
    };
    let commit_cid = add(commit_bytes, &mut blocks, &mut order);
    // The commit goes first in the CAR, as in `getRecord` responses.
    order.retain(|x| *x != commit_cid);
    order.insert(0, commit_cid);

    // Damage.
    for d in input.damage.iter().take(8) {
        match *d {
            Damage::Drop(i) if !order.is_empty() => {
                let c = order.remove(usize::from(i) % order.len());
                blocks.remove(&c);
                clean = false;
            }
            Damage::Flip { block, pos, xor } if !order.is_empty() && xor != 0 => {
                let c = order[usize::from(block) % order.len()];
                if let Some(b) = blocks.get_mut(&c)
                    && !b.is_empty()
                {
                    let n = b.len();
                    b[usize::from(pos) % n] ^= xor;
                    clean = false;
                }
            }
            _ => {}
        }
    }

    Repo {
        blocks,
        order,
        commit_cid,
        root,
        entries,
        keys,
        record_cids,
        clean,
    }
}

fn car_bytes(repo: &Repo, roots: &[Cid], damage: &[Damage], extra: &[(Cid, Vec<u8>)]) -> Vec<u8> {
    let mut list: Vec<(&Cid, &[u8])> = repo
        .order
        .iter()
        .filter_map(|c| repo.blocks.get(c).map(|b| (c, b.as_slice())))
        .collect();
    list.extend(extra.iter().map(|(c, b)| (c, b.as_slice())));
    let mut roots = roots.to_vec();
    for d in damage.iter().take(8) {
        match *d {
            Damage::Duplicate(i) if !list.is_empty() => {
                let x = list[usize::from(i) % list.len()];
                list.push(x);
            }
            Damage::ExtraRoot => roots.push(repo.root),
            _ => {}
        }
    }
    write_car(&roots, list)
}

fn target_key(t: &Target, repo: &Repo) -> Vec<u8> {
    match t {
        Target::Key(i) if !repo.keys.is_empty() => {
            repo.keys[usize::from(*i) % repo.keys.len()].clone()
        }
        Target::Key(i) => fresh_key(*i, &[]).into_bytes(),
        Target::Fresh { coll, rkey } => fresh_key(*coll, rkey).into_bytes(),
        Target::Bytes(b) => b.iter().take(1100).copied().collect(),
    }
}

fuzz_target!(|input: Input| {
    let repo = build(&input);
    let structural_damage = input
        .damage
        .iter()
        .take(8)
        .any(|d| matches!(d, Damage::Duplicate(_) | Damage::ExtraRoot));
    let car = car_bytes(&repo, &[repo.commit_cid], &input.damage, &[]);

    // The walk and the listing.
    let listed = mst::list_entries(&repo.root, &repo.blocks, 4096);
    if repo.clean
        && let Some(want) = &repo.entries
    {
        let want: Vec<(Vec<u8>, Cid)> = want.iter().map(|(k, v)| (k.clone(), *v)).collect();
        assert_eq!(listed.as_ref(), Ok(&want), "listing of a canonical tree");
    }
    let mut targets: Vec<Vec<u8>> = input
        .targets
        .iter()
        .take(16)
        .map(|t| target_key(t, &repo))
        .collect();
    if let Some(want) = &repo.entries {
        targets.extend(want.keys().take(16).cloned());
    }
    for key in &targets {
        let proof = mst::prove(&repo.root, key, &repo.blocks);
        let expected =
            repo.entries
                .as_ref()
                .filter(|_| repo.clean)
                .map(|want| match want.get(key) {
                    Some(v) => Proof::Present(*v),
                    None => Proof::Absent,
                });
        if let Some(expected) = expected
            && mst::validate_key(key).is_ok()
        {
            assert_eq!(proof, Ok(expected), "walk of a canonical tree for {key:?}");
        }
        let Some((coll, rkey)) = split_key(key) else {
            continue;
        };
        let out = verify_proof(
            &car,
            &FUZZ_DID,
            &coll,
            &rkey,
            &FUZZ_KEY,
            NOW,
            &ProofCaps::default(),
        );
        if let Some(expected) = expected
            && !structural_damage
        {
            let proven = out.as_ref().map(|o| o.record_cid().copied());
            let want = match expected {
                Proof::Present(v) => Some(v),
                Proof::Absent => None,
            };
            assert_eq!(
                proven.map_err(|f| f.kind()),
                Ok(want),
                "verify_proof of a clean repo for {key:?}"
            );
        }
        if let Ok(uri) = AtUri::new(FUZZ_DID.clone(), coll, rkey) {
            let caps = ReadCaps {
                max_bytes: usize::from(input.read_caps.0) * 64,
                max_blocks: usize::from(input.read_caps.1 % 64),
            };
            let capped =
                verify_proof_in(&repo.blocks, &repo.commit_cid, &uri, &FUZZ_KEY, NOW, &caps);
            let full = verify_proof_in(
                &repo.blocks,
                &repo.commit_cid,
                &uri,
                &FUZZ_KEY,
                NOW,
                &ReadCaps::PROOF,
            );
            // Read caps only ever turn a verdict into a cap error.
            match &capped {
                Err(f)
                    if matches!(
                        f.kind(),
                        ProofErrorKind::CarTooLarge | ProofErrorKind::TooManyBlocks
                    ) => {}
                _ => assert_eq!(capped, full, "read caps changed the verdict"),
            }
        }
    }

    // An indexer bundle over the same blocks.
    if input.bundle.is_empty() {
        return;
    }
    let mut items = Vec::new();
    let mut claims_ok = Vec::new();
    for (t, claim) in input.bundle.iter().take(30) {
        let key = target_key(t, &repo);
        let parts = split_key(&key);
        let uri = parts
            .as_ref()
            .and_then(|(c, r)| AtUri::new(FUZZ_DID.clone(), c.clone(), r.clone()).ok());
        let actual = repo.entries.as_ref().map(|want| want.get(&key).copied());
        let mut m = BTreeMap::new();
        m.insert(
            "uri".to_owned(),
            match (claim, &uri) {
                (Claim::MalformedUri, _) | (_, None) => {
                    Ipld::String(String::from_utf8_lossy(&key).into_owned())
                }
                (_, Some(u)) => Ipld::String(u.as_str().to_owned()),
            },
        );
        let claimed = match claim {
            Claim::Actual => actual.flatten(),
            Claim::Absent => None,
            Claim::Other(i) => Some(repo.record_cids[usize::from(*i) % repo.record_cids.len()]),
            Claim::MalformedUri | Claim::MalformedCid => None,
        };
        m.insert(
            "cid".to_owned(),
            match (claim, claimed) {
                (Claim::MalformedCid, _) => Ipld::Integer(1),
                (_, Some(c)) => Ipld::Link(c),
                (_, None) => Ipld::Null,
            },
        );
        m.insert("commit".to_owned(), Ipld::Link(repo.commit_cid));
        items.push(Ipld::Map(m));
        let well_formed = uri
            .as_ref()
            .is_some_and(|u| AtUri::parse(u.as_str()).is_ok())
            && !matches!(claim, Claim::MalformedUri | Claim::MalformedCid);
        claims_ok.push(actual.filter(|_| well_formed).map(|a| a == claimed));
    }
    let mut manifest = BTreeMap::new();
    manifest.insert("v".to_owned(), Ipld::Integer(1));
    manifest.insert("items".to_owned(), Ipld::List(items));
    let manifest = encode(&Ipld::Map(manifest));
    let manifest_cid = cid::for_dag_cbor(&manifest);
    let bundle_car = car_bytes(
        &repo,
        &[manifest_cid],
        &input.damage,
        &[(manifest_cid, manifest)],
    );
    let Ok(bundle) = ProofBundle::parse(&bundle_car, &BundleCaps::default()) else {
        return;
    };
    for (i, expect) in claims_ok.iter().enumerate().take(bundle.items.len()) {
        let r = bundle.verify_item(i, &FUZZ_KEY, NOW);
        if repo.clean
            && !structural_damage
            && let Some(ok) = expect
        {
            assert_eq!(r.is_ok(), *ok, "bundle item {i}: {r:?}");
            if let Err(f) = &r {
                assert_eq!(f.kind(), ProofErrorKind::RecordMismatch);
            }
        }
    }
});
