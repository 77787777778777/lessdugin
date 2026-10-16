// SPDX-License-Identifier: MIT OR Apache-2.0
//! `proofs/1`: verification of single-record repo proofs (`sync.md` §8): a
//! `com.atproto.sync.getRecord` CAR ([`verify_proof`]), any verified block set with a known commit
//! ([`verify_proof_in`]), and indexer proof bundles ([`ProofBundle`], §8.6).
//!
//! ```text
//! verify_proof(car, did, coll, rkey, key, now) -> Proven{rev, commitCid, cid, value} | ProvenAbsent{rev, commitCid} | Err(kind)
//!   CARv1 parse with caps (1,000,000 bytes, ≤ 2,000 blocks, no preallocation), every block
//!          CID-checked, exactly one root (§8.2)
//!   commit = strict_decode(block(root)): version 3, did, rev TID, rev ≤ now + 5 min,
//!          low-S ECDSA by `key` over dag-cbor(commit without sig) (§8.4)
//!   MST walk from commit.data toward "coll/rkey" (§8.5, see crate::mst)
//!   Found(v) → v is CIDv1 dag-cbor sha2-256; value = strict_decode(block(v))
//! ```
//!
//! Failures carry a [`ProofErrorKind`] (the `Err` kinds of §8.1, via [`ProofError::kind`]), and
//! [`ProofFailure`] adds the commit CID and rev once they are known, so a `BadSignature` can be
//! classified `stale-proof` (§8.7) without parsing the CAR again.
//!
//! **Strictness and memory.** Every block is checked as strict DAG-CBOR with nesting ≤ 32 and no
//! other caps ([`Limits::STRICT`], without allocating), so the verdict depends only on the
//! protocol rules. The proven record's value is then materialised under the S-class caps of
//! [`Limits::DEFAULT`]; a record over them is still proven, with [`ProvenRecord::value`] `None`
//! ("unreadable"), and its bytes are always returned.

use std::cell::Cell;

use ::cid::{Cid, Version};

use crate::Ipld;
use crate::car::{Car, CarError, CarLimits, read_car};
use crate::cid::{CidError, DAG_CBOR, SHA2_256};
use crate::commit::{Commit, CommitError};
use crate::crypto::PublicKey;
use crate::dagcbor::{decode_scalar, list_items_raw, map_entries_raw};
use crate::datamodel::{DataModelError, Limits};
use crate::mst::{self, BlockSource, MstError, Proof};
use crate::syntax::{AtUri, Did, Nsid, RecordKey, Tid};

/// Caps for [`verify_proof`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProofCaps {
    /// CAR caps (default [`CarLimits::PROOF`]).
    pub car: CarLimits,
}

/// Caps on what one verification reads from its block source (`PROOF_CAR_MAX_BYTES` and
/// `PROOF_CAR_MAX_BLOCKS` applied to "the blocks its verification reads", `limits.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadCaps {
    /// Total bytes of the blocks read (commit, nodes, record).
    pub max_bytes: usize,
    /// Number of blocks read.
    pub max_blocks: usize,
}

impl ReadCaps {
    /// 1,000,000 bytes and 2,000 blocks.
    pub const PROOF: Self = Self {
        max_bytes: 1_000_000,
        max_blocks: 2_000,
    };
}

impl Default for ReadCaps {
    fn default() -> Self {
        Self::PROOF
    }
}

/// Caps for [`ProofBundle::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleCaps {
    /// CAR caps (default [`CarLimits::BUNDLE`]).
    pub car: CarLimits,
    /// Per-item read caps (default [`ReadCaps::PROOF`]).
    pub item: ReadCaps,
    /// Maximum manifest items (25).
    pub max_items: usize,
}

impl Default for BundleCaps {
    fn default() -> Self {
        Self {
            car: CarLimits::BUNDLE,
            item: ReadCaps::PROOF,
            max_items: 25,
        }
    }
}

/// The `Err` kinds of `proofs/1` (`sync.md` §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProofErrorKind {
    /// The CAR (or bundle manifest) is malformed, or does not have exactly one root.
    CarMalformed,
    /// The CAR, a block, or the bytes one verification reads exceed the byte cap.
    CarTooLarge,
    /// The CAR, or the blocks one verification reads, exceed the block cap.
    TooManyBlocks,
    /// A block does not hash to its CID.
    BlockHashMismatch,
    /// A needed block (commit, MST node, record) is missing.
    MissingBlock,
    /// A commit, node or record block is not strict DAG-CBOR, or the record CID is not
    /// CIDv1 dag-cbor sha2-256.
    NonCanonical,
    /// The commit is malformed: field types, `version`, `rev` syntax, or a root that is not
    /// dag-cbor.
    BadCommit,
    /// The commit's `did` is not the repo's.
    DidMismatch,
    /// The commit's `rev` is more than 5 minutes ahead of `now`.
    FutureRev,
    /// The signature is malformed, high-S, or does not verify.
    BadSignature,
    /// The DID document has no usable `#atproto` key (never produced by verification itself;
    /// see [`crate::did_doc::KeyError`]).
    UnsupportedKey,
    /// The MST violates the walk's rules (schema, keys, order, layers, links).
    MstInvalid,
    /// The MST path is deeper than `MST_DEPTH_MAX`.
    MstTooDeep,
    /// A bundle item's claimed CID (or absence) differs from what its proof shows.
    RecordMismatch,
}

impl ProofErrorKind {
    /// The kind's name as `sync.md` §8.1 spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CarMalformed => "CarMalformed",
            Self::CarTooLarge => "CarTooLarge",
            Self::TooManyBlocks => "TooManyBlocks",
            Self::BlockHashMismatch => "BlockHashMismatch",
            Self::MissingBlock => "MissingBlock",
            Self::NonCanonical => "NonCanonical",
            Self::BadCommit => "BadCommit",
            Self::DidMismatch => "DidMismatch",
            Self::FutureRev => "FutureRev",
            Self::BadSignature => "BadSignature",
            Self::UnsupportedKey => "UnsupportedKey",
            Self::MstInvalid => "MstInvalid",
            Self::MstTooDeep => "MstTooDeep",
            Self::RecordMismatch => "RecordMismatch",
        }
    }
}

impl core::fmt::Display for ProofErrorKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Proof errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// The CAR is malformed, over a cap, has a block that does not match its CID, or does not have
    /// exactly one root; or a verification read too much (`TooLarge` / `TooManyBlocks`).
    #[error("proof CAR: {0}")]
    Car(#[from] CarError),
    /// The CAR has no root.
    #[error("proof CAR has no root")]
    NoRoot,
    /// The commit CID is not a dag-cbor CID.
    #[error("proof root is not a dag-cbor CID")]
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
    /// The MST proves presence but the record block is not available.
    #[error("record block {0} missing from proof")]
    MissingRecord(Cid),
    /// The proven record's CID is not CIDv1 dag-cbor sha2-256, or its block is not strict
    /// DAG-CBOR.
    #[error("record {cid}: {error}")]
    BadRecord {
        /// The record CID from the MST.
        cid: Cid,
        /// What is wrong.
        error: Box<DataModelError>,
    },
    /// A bundle item claims a CID (or absence) that its proof contradicts.
    #[error("bundle item claims {:?} but the proof shows {:?}", .0.claimed, .0.proven)]
    RecordMismatch(Box<Mismatch>),
}

/// What a bundle item claimed, and what its proof showed ([`ProofError::RecordMismatch`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The item's `cid` (`None` = claimed absent).
    pub claimed: Option<Cid>,
    /// The proven record CID (`None` = proven absent).
    pub proven: Option<Cid>,
}

impl ProofError {
    /// The `sync.md` §8.1 error kind.
    pub fn kind(&self) -> ProofErrorKind {
        use ProofErrorKind as K;
        match self {
            Self::Car(e) => match e {
                CarError::TooLarge { .. } | CarError::BlockTooLarge { .. } => K::CarTooLarge,
                CarError::TooManyBlocks(_) => K::TooManyBlocks,
                CarError::HashMismatch(_)
                | CarError::Cid {
                    err: CidError::Mismatch(_),
                    ..
                } => K::BlockHashMismatch,
                CarError::Varint(_)
                | CarError::Truncated(_)
                | CarError::Header(_)
                | CarError::Cid { .. }
                | CarError::ConflictingDuplicate(_)
                | CarError::Manifest(_) => K::CarMalformed,
            },
            Self::NoRoot => K::CarMalformed,
            Self::BadRoot => K::BadCommit,
            Self::MissingCommit(_) | Self::MissingRecord(_) => K::MissingBlock,
            Self::Commit(e) => match e {
                CommitError::Decode(_) => K::NonCanonical,
                CommitError::Malformed(_) | CommitError::Version(_) => K::BadCommit,
                CommitError::DidMismatch { .. } => K::DidMismatch,
                CommitError::FutureRev(_) => K::FutureRev,
                CommitError::Signature(_) => K::BadSignature,
            },
            Self::Mst(e) => match e {
                MstError::MissingBlock(_) => K::MissingBlock,
                MstError::Decode(_) => K::NonCanonical,
                MstError::Cid(CidError::Mismatch(_)) => K::BlockHashMismatch,
                MstError::DepthExceeded(_) => K::MstTooDeep,
                MstError::Cid(_)
                | MstError::Malformed(_)
                | MstError::InvalidKey(_)
                | MstError::Ordering
                | MstError::PrefixNotCanonical
                | MstError::Layer => K::MstInvalid,
            },
            Self::BadRecord { .. } => K::NonCanonical,
            Self::RecordMismatch(_) => K::RecordMismatch,
        }
    }
}

/// A failed verification, with what was learned before it failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct ProofFailure {
    /// The error (boxed, so that `Result<_, ProofFailure>` stays small).
    pub error: Box<ProofError>,
    /// The commit CID, once known (the CAR root, or a bundle item's `commit`).
    pub commit_cid: Option<Cid>,
    /// The commit's rev, once the commit block decoded with a TID `rev`, even if verification
    /// then failed (for the `stale-proof` rule, `sync.md` §8.7).
    pub rev: Option<Tid>,
}

impl ProofFailure {
    fn new(error: impl Into<ProofError>) -> Self {
        Self::with(error.into(), None, None)
    }

    fn with(error: ProofError, commit_cid: Option<Cid>, rev: Option<Tid>) -> Self {
        Self {
            error: Box::new(error),
            commit_cid,
            rev,
        }
    }

    fn replace_error(mut self, error: ProofError) -> Self {
        *self.error = error;
        self
    }

    /// The `sync.md` §8.1 error kind.
    pub fn kind(&self) -> ProofErrorKind {
        self.error.kind()
    }

    /// The error, without the context.
    pub fn into_error(self) -> ProofError {
        *self.error
    }
}

impl From<ProofFailure> for ProofError {
    fn from(f: ProofFailure) -> Self {
        f.into_error()
    }
}

/// A proven record.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvenRecord {
    /// The record CID (from the MST; the block hashes to it).
    pub cid: Cid,
    /// The record block: strict DAG-CBOR, nesting ≤ 32.
    pub bytes: Vec<u8>,
    /// The decoded value, or `None` if it exceeds the S-class caps of [`Limits::DEFAULT`]
    /// (unreadable, though proven). Its data-model and lexicon validity are the caller's to check.
    pub value: Option<Ipld>,
}

/// A successful verification.
#[derive(Debug, Clone, PartialEq)]
pub struct ProofOutcome {
    /// The commit revision.
    pub rev: Tid,
    /// The commit CID.
    pub commit_cid: Cid,
    /// `Some` if proven present (`Proven`), `None` if proven absent (`ProvenAbsent`).
    pub record: Option<ProvenRecord>,
}

impl ProofOutcome {
    /// `true` if the record is proven present.
    pub fn is_present(&self) -> bool {
        self.record.is_some()
    }

    /// The record CID, if present.
    pub fn record_cid(&self) -> Option<&Cid> {
        self.record.as_ref().map(|r| &r.cid)
    }

    /// The decoded record value, if present and readable.
    pub fn value(&self) -> Option<&Ipld> {
        self.record.as_ref().and_then(|r| r.value.as_ref())
    }
}

/// Verifies a single-record proof CAR (`com.atproto.sync.getRecord`) for
/// `at://did/collection/rkey` against the repo signing `key` (the `#atproto` key of a DID
/// document at level `doc`) at time `now_micros` (µs since the Unix epoch).
pub fn verify_proof(
    car: &[u8],
    did: &Did,
    collection: &Nsid,
    rkey: &RecordKey,
    key: &PublicKey,
    now_micros: i64,
    caps: &ProofCaps,
) -> Result<ProofOutcome, ProofFailure> {
    let car = read_car(car, &caps.car).map_err(ProofFailure::new)?;
    let commit_cid = match car.roots.as_slice() {
        [c] => *c,
        [] => return Err(ProofFailure::new(ProofError::NoRoot)),
        _ => {
            return Err(ProofFailure::new(CarError::Header(
                "a proof CAR must have exactly one root",
            )));
        }
    };
    verify_blocks(&car, commit_cid, did, collection, rkey, key, now_micros)
}

/// [`verify_proof`] with a plain [`ProofError`] (no commit context).
pub fn verify_record_proof(
    car: &[u8],
    did: &Did,
    collection: &Nsid,
    rkey: &RecordKey,
    key: &PublicKey,
    now_micros: i64,
    caps: &ProofCaps,
) -> Result<ProofOutcome, ProofError> {
    verify_proof(car, did, collection, rkey, key, now_micros, caps)
        .map_err(ProofFailure::into_error)
}

/// Verifies the proof of `uri` under the commit `commit_cid`, reading blocks from `blocks` (for
/// example the blocks of an indexer bundle, `sync.md` §8.6). Blocks are hash-checked as they are
/// read, and the reads are capped by `read` (exceeding a cap fails with `CarTooLarge` or
/// `TooManyBlocks`).
pub fn verify_proof_in(
    blocks: &impl BlockSource,
    commit_cid: &Cid,
    uri: &AtUri,
    key: &PublicKey,
    now_micros: i64,
    read: &ReadCaps,
) -> Result<ProofOutcome, ProofFailure> {
    let budget = Budgeted::new(blocks, read);
    let r = verify_blocks(
        &budget,
        *commit_cid,
        uri.did(),
        uri.collection(),
        uri.rkey(),
        key,
        now_micros,
    );
    match (r, budget.exceeded.get()) {
        (Err(f), Some(cap)) => Err(f.replace_error(cap.error(read).into())),
        (r, _) => r,
    }
}

/// Which read cap was exceeded.
#[derive(Debug, Clone, Copy)]
enum Exceeded {
    Bytes(usize),
    Blocks,
}

impl Exceeded {
    fn error(self, caps: &ReadCaps) -> CarError {
        match self {
            Self::Bytes(len) => CarError::TooLarge {
                len,
                max: caps.max_bytes,
            },
            Self::Blocks => CarError::TooManyBlocks(caps.max_blocks),
        }
    }
}

/// A block source that stops serving once a read cap is exceeded.
struct Budgeted<'a, S: BlockSource> {
    inner: &'a S,
    caps: ReadCaps,
    bytes: Cell<usize>,
    blocks: Cell<usize>,
    exceeded: Cell<Option<Exceeded>>,
}

impl<'a, S: BlockSource> Budgeted<'a, S> {
    fn new(inner: &'a S, caps: &ReadCaps) -> Self {
        Self {
            inner,
            caps: *caps,
            bytes: Cell::new(0),
            blocks: Cell::new(0),
            exceeded: Cell::new(None),
        }
    }
}

impl<S: BlockSource> BlockSource for Budgeted<'_, S> {
    fn get_block(&self, cid: &Cid) -> Option<&[u8]> {
        if self.exceeded.get().is_some() {
            return None;
        }
        let b = self.inner.get_block(cid)?;
        let blocks = self.blocks.get() + 1;
        let bytes = self.bytes.get().saturating_add(b.len());
        self.blocks.set(blocks);
        self.bytes.set(bytes);
        if blocks > self.caps.max_blocks {
            self.exceeded.set(Some(Exceeded::Blocks));
            return None;
        }
        if bytes > self.caps.max_bytes {
            self.exceeded.set(Some(Exceeded::Bytes(bytes)));
            return None;
        }
        Some(b)
    }
}

/// Hash-checks a block read from a source that may not have (a [`Car`] always has).
fn checked(cid: &Cid, bytes: &[u8]) -> Result<(), ProofError> {
    match crate::cid::verify(cid, bytes) {
        Ok(()) => Ok(()),
        Err(CidError::Mismatch(c)) => Err(CarError::HashMismatch(c).into()),
        Err(err) => Err(CarError::Cid { offset: 0, err }.into()),
    }
}

/// `true` for CIDv1, dag-cbor, sha2-256 with a 32-byte digest.
fn is_dag_cbor_sha256(c: &Cid) -> bool {
    c.version() == Version::V1
        && c.codec() == DAG_CBOR
        && c.hash().code() == SHA2_256
        && c.hash().size() == 32
}

fn verify_blocks(
    blocks: &impl BlockSource,
    commit_cid: Cid,
    did: &Did,
    collection: &Nsid,
    rkey: &RecordKey,
    key: &PublicKey,
    now_micros: i64,
) -> Result<ProofOutcome, ProofFailure> {
    // What a failure reports besides the error (the rev once the commit decoded).
    let mut ctx: Option<Tid> = None;
    let fail = |rev: &Option<Tid>, e: ProofError| ProofFailure::with(e, Some(commit_cid), *rev);
    let commit_bytes = blocks
        .get_block(&commit_cid)
        .ok_or_else(|| fail(&ctx, ProofError::MissingCommit(commit_cid)))?;
    if commit_cid.codec() != DAG_CBOR {
        return Err(fail(&ctx, ProofError::BadRoot));
    }
    checked(&commit_cid, commit_bytes).map_err(|e| fail(&ctx, e))?;
    let commit = Commit::decode(commit_bytes).map_err(|e| fail(&ctx, e.into()))?;
    ctx = commit.rev_tid();
    let rev = commit
        .verify(did, key, now_micros)
        .map_err(|e| fail(&ctx, e.into()))?;
    let path = mst::repo_path(collection, rkey);
    let record = match mst::prove(&commit.data, path.as_bytes(), blocks)
        .map_err(|e| fail(&ctx, e.into()))?
    {
        Proof::Absent => None,
        Proof::Present(cid) => {
            if !is_dag_cbor_sha256(&cid) {
                return Err(fail(
                    &ctx,
                    ProofError::BadRecord {
                        cid,
                        error: Box::new(DataModelError::InvalidLink(
                            "a record CID must be CIDv1 dag-cbor sha2-256".into(),
                        )),
                    },
                ));
            }
            let bytes = blocks
                .get_block(&cid)
                .ok_or_else(|| fail(&ctx, ProofError::MissingRecord(cid)))?;
            checked(&cid, bytes).map_err(|e| fail(&ctx, e))?;
            crate::dagcbor::validate_with(bytes, &Limits::STRICT).map_err(|error| {
                fail(
                    &ctx,
                    ProofError::BadRecord {
                        cid,
                        error: Box::new(error),
                    },
                )
            })?;
            Some(ProvenRecord {
                cid,
                bytes: bytes.to_vec(),
                value: crate::dagcbor::decode(bytes).ok(),
            })
        }
    };
    Ok(ProofOutcome {
        rev,
        commit_cid,
        record,
    })
}

// ---------------------------------------------------------------------------------------------
// Indexer proof bundles (sync.md §8.6)

/// One item of a bundle manifest: `{uri, cid: CID | null, commit: CID}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleItem {
    /// The record (a canonical at-uri).
    pub uri: AtUri,
    /// The claimed record CID; `None` claims absence.
    pub cid: Option<Cid>,
    /// The commit the proof is under.
    pub commit: Cid,
}

/// A parsed `getRecordProofs` CAR: the manifest's items and the deduplicated blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofBundle {
    /// The bundle's blocks (each verified against its CID).
    pub car: Car,
    /// The manifest CID (the CAR root).
    pub manifest: Cid,
    /// Manifest items in order. A malformed item is an `Err` (it fails on its own, the others
    /// are still verified).
    pub items: Vec<Result<BundleItem, ProofError>>,
    item_caps: ReadCaps,
}

fn manifest_item(raw: &[u8]) -> Result<BundleItem, ProofError> {
    let bad = |why| ProofError::Car(CarError::Manifest(why));
    let fields = map_entries_raw(raw).ok_or(bad("item is not a map"))?;
    let (mut uri, mut cid, mut commit) = (None, None, None);
    for (k, v) in fields {
        match k {
            "uri" => uri = Some(v),
            "cid" => cid = Some(v),
            "commit" => commit = Some(v),
            _ => {}
        }
    }
    let uri = match uri.and_then(decode_scalar) {
        Some(Ipld::String(s)) => {
            AtUri::parse(&s).map_err(|_| bad("item uri is not a canonical record at-uri"))?
        }
        _ => return Err(bad("item uri must be a string")),
    };
    let cid = match cid.and_then(decode_scalar) {
        Some(Ipld::Link(c)) => Some(c),
        Some(Ipld::Null) => None,
        _ => return Err(bad("item cid must be a link or null")),
    };
    let commit = match commit.and_then(decode_scalar) {
        Some(Ipld::Link(c)) => c,
        _ => return Err(bad("item commit must be a link")),
    };
    Ok(BundleItem { uri, cid, commit })
}

impl ProofBundle {
    /// Parses a bundle: the CAR under `caps.car` with exactly one root, the root block as a
    /// strict DAG-CBOR manifest `{v: 1, items: [...]}` (unknown keys ignored, at most
    /// `caps.max_items` items). Items are parsed one by one; see [`ProofBundle::items`].
    pub fn parse(car: &[u8], caps: &BundleCaps) -> Result<Self, ProofError> {
        let car = read_car(car, &caps.car)?;
        let manifest = match car.roots.as_slice() {
            [c] => *c,
            [] => return Err(ProofError::NoRoot),
            _ => {
                return Err(CarError::Header("a proof bundle must have exactly one root").into());
            }
        };
        let bad = |why| ProofError::Car(CarError::Manifest(why));
        let bytes = car
            .get(&manifest)
            .ok_or(bad("the manifest block is missing"))?;
        crate::dagcbor::validate_with(bytes, &Limits::STRICT)
            .map_err(|_| bad("manifest is not strict DAG-CBOR"))?;
        let fields = map_entries_raw(bytes).ok_or(bad("manifest is not a map"))?;
        let (mut v, mut items) = (None, None);
        for (k, val) in fields {
            match k {
                "v" => v = Some(val),
                "items" => items = Some(val),
                _ => {}
            }
        }
        if v.and_then(decode_scalar) != Some(Ipld::Integer(1)) {
            return Err(bad("manifest v must be 1"));
        }
        let list = items
            .and_then(list_items_raw)
            .ok_or(bad("manifest items must be a list"))?;
        if list.declared_len() > caps.max_items as u64 {
            return Err(bad("too many manifest items"));
        }
        let items = list.map(manifest_item).collect();
        Ok(Self {
            car,
            manifest,
            items,
            item_caps: caps.item,
        })
    }

    /// Verifies item `index` (`sync.md` §8.6) with `key`, the `#atproto` key of the item's DID:
    /// the proof under the item's commit, with reads capped per item, must show the claimed CID
    /// (or absence), else `RecordMismatch`.
    pub fn verify_item(
        &self,
        index: usize,
        key: &PublicKey,
        now_micros: i64,
    ) -> Result<ProofOutcome, ProofFailure> {
        let item = match self.items.get(index) {
            Some(Ok(item)) => item,
            Some(Err(e)) => return Err(ProofFailure::new(e.clone())),
            None => {
                return Err(ProofFailure::new(CarError::Manifest(
                    "no such manifest item",
                )));
            }
        };
        let out = verify_proof_in(
            &self.car,
            &item.commit,
            &item.uri,
            key,
            now_micros,
            &self.item_caps,
        )?;
        let proven = out.record_cid().copied();
        if proven != item.cid {
            return Err(ProofFailure::with(
                ProofError::RecordMismatch(Box::new(Mismatch {
                    claimed: item.cid,
                    proven,
                })),
                Some(out.commit_cid),
                Some(out.rev),
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::car::write_car;
    use crate::commit::UnsignedCommit;
    use k256::ecdsa::signature::Signer as _;
    use serde_json::json;
    use std::collections::BTreeMap;

    const NOW: i64 = 1_760_000_000_000_000;
    const POST: &str = "online.7778777.lessdugin.post";

    enum Sk {
        K(k256::ecdsa::SigningKey),
        P(p256::ecdsa::SigningKey),
    }

    impl Sk {
        fn sign(&self, msg: &[u8]) -> Vec<u8> {
            match self {
                Self::K(k) => {
                    let s: k256::ecdsa::Signature = k.sign(msg);
                    s.to_bytes().to_vec()
                }
                Self::P(p) => {
                    let s: p256::ecdsa::Signature = p.sign(msg);
                    s.normalize_s().unwrap_or(s).to_bytes().to_vec()
                }
            }
        }

        fn public(&self) -> PublicKey {
            match self {
                Self::K(k) => PublicKey::Secp256k1(*k.verifying_key()),
                Self::P(p) => PublicKey::P256(*p.verifying_key()),
            }
        }
    }

    struct Repo {
        did: Did,
        sk: Sk,
        /// MST key → (record CID, record block)
        records: BTreeMap<String, (Cid, Vec<u8>)>,
    }

    struct Built {
        car: Vec<u8>,
        commit: Cid,
        blocks: Vec<(Cid, Vec<u8>)>,
    }

    impl Repo {
        fn new(sk: Sk) -> Self {
            let mut r = Self {
                did: Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap(),
                sk,
                records: BTreeMap::new(),
            };
            for i in 0..50 {
                r.put_json(
                    &format!("{POST}/uetjixxr3a33-3l2s5xxv2z{i:03}"),
                    &json!({"$type": POST, "text": format!("post {i}"), "n": i}),
                );
            }
            r.put_json(
                "online.7778777.lessdugin.profile/self",
                &json!({"$type": "online.7778777.lessdugin.profile", "displayName": "x"}),
            );
            r
        }

        fn put_json(&mut self, path: &str, v: &serde_json::Value) {
            let b = crate::datamodel::record_to_cbor(v).unwrap();
            self.put_block(path, crate::cid::for_dag_cbor(&b), b);
        }

        fn put_block(&mut self, path: &str, cid: Cid, bytes: Vec<u8>) {
            self.records.insert(path.to_owned(), (cid, bytes));
        }

        fn key(&self) -> PublicKey {
            self.sk.public()
        }

        /// The commit, the nodes on the search path for `path` and the record block (if
        /// present), with `extra` fields signed into the commit.
        fn build(&self, path: &str, rev_micros: i64, extra: Vec<(&str, Ipld)>) -> Built {
            let entries = self.records.iter().map(|(k, (c, _))| (k.as_bytes(), *c));
            let (root, nodes) = mst::build(entries).unwrap();
            let u = UnsignedCommit {
                did: self.did.clone(),
                data: root,
                rev: Tid::from_parts(rev_micros as u64, 1).unwrap(),
                prev: None,
            };
            let (ccid, cbytes) = if extra.is_empty() {
                u.signed(&self.sk.sign(&u.signing_bytes()))
            } else {
                let mut m = BTreeMap::new();
                m.insert("did".to_owned(), Ipld::String(u.did.to_string()));
                m.insert("version".to_owned(), Ipld::Integer(3));
                m.insert("data".to_owned(), Ipld::Link(root));
                m.insert("rev".to_owned(), Ipld::String(u.rev.to_string()));
                m.insert("prev".to_owned(), Ipld::Null);
                for (k, v) in extra {
                    m.insert(k.to_owned(), v);
                }
                let lim = Limits::uncapped(64);
                let unsigned = crate::dagcbor::encode_with(&Ipld::Map(m.clone()), &lim).unwrap();
                m.insert("sig".to_owned(), Ipld::Bytes(self.sk.sign(&unsigned)));
                let b = crate::dagcbor::encode_with(&Ipld::Map(m), &lim).unwrap();
                (crate::cid::for_dag_cbor(&b), b)
            };
            let mut blocks: Vec<(Cid, Vec<u8>)> = vec![(ccid, cbytes)];
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
                    blocks.push(self.records[path].clone());
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
            let car = write_car(&[ccid], blocks.iter().map(|(c, b)| (c, b.as_slice())));
            Built {
                car,
                commit: ccid,
                blocks,
            }
        }

        fn proof_car(&self, path: &str, rev_micros: i64) -> Vec<u8> {
            self.build(path, rev_micros, vec![]).car
        }

        fn verify(&self, car: &[u8], path: &str) -> Result<ProofOutcome, ProofFailure> {
            let (c, k) = path.split_once('/').unwrap();
            verify_proof(
                car,
                &self.did,
                &Nsid::parse(c).unwrap(),
                &RecordKey::parse(k).unwrap(),
                &self.key(),
                NOW,
                &ProofCaps::default(),
            )
        }
    }

    fn k256_repo() -> Repo {
        Repo::new(Sk::K(
            k256::ecdsa::SigningKey::from_slice(&[3u8; 32]).unwrap(),
        ))
    }

    fn coll() -> Nsid {
        Nsid::parse(POST).unwrap()
    }

    fn path7() -> String {
        format!("{POST}/uetjixxr3a33-3l2s5xxv2z007")
    }

    #[test]
    fn end_to_end_present_k256_and_p256() {
        for repo in [
            k256_repo(),
            Repo::new(Sk::P(
                p256::ecdsa::SigningKey::from_slice(&[9u8; 32]).unwrap(),
            )),
        ] {
            let path = path7();
            let car = repo.proof_car(&path, NOW);
            let out = repo.verify(&car, &path).unwrap();
            assert!(out.is_present());
            let rec = out.record.clone().unwrap();
            assert_eq!(crate::cid::for_dag_cbor(&rec.bytes), rec.cid);
            assert_eq!(rec.cid, repo.records[&path].0);
            let json = crate::datamodel::record_cbor_to_json(&rec.bytes).unwrap();
            assert_eq!(json["text"], "post 7");
            let value = out.value().unwrap();
            assert_eq!(crate::datamodel::ipld_to_json(value).unwrap(), json);
            assert_eq!(out.rev.timestamp_micros(), NOW as u64);
            assert_eq!(out.commit_cid, repo.build(&path, NOW, vec![]).commit);
            // the legacy entry point agrees
            let rkey = RecordKey::parse("uetjixxr3a33-3l2s5xxv2z007").unwrap();
            let legacy = verify_record_proof(
                &car,
                &repo.did,
                &coll(),
                &rkey,
                &repo.key(),
                NOW,
                &ProofCaps::default(),
            )
            .unwrap();
            assert_eq!(legacy, out);
            // a P-256 proof does not verify with a k256 key and vice versa
            let other = match repo.key() {
                PublicKey::Secp256k1(_) => {
                    Sk::P(p256::ecdsa::SigningKey::from_slice(&[9u8; 32]).unwrap()).public()
                }
                PublicKey::P256(_) => {
                    Sk::K(k256::ecdsa::SigningKey::from_slice(&[3u8; 32]).unwrap()).public()
                }
            };
            let r = verify_proof(
                &car,
                &repo.did,
                &coll(),
                &rkey,
                &other,
                NOW,
                &ProofCaps::default(),
            );
            assert_eq!(r.unwrap_err().kind(), ProofErrorKind::BadSignature);
        }
    }

    #[test]
    fn end_to_end_absent() {
        let repo = k256_repo();
        let path = format!("{POST}/uetjixxr3a33-3l2s5xxv2z007x");
        let out = repo.verify(&repo.proof_car(&path, NOW), &path).unwrap();
        assert!(!out.is_present());
        assert!(out.record_cid().is_none() && out.value().is_none());
        // another collection entirely
        let path = "online.7778777.lessdugin.comment/x";
        let out = repo.verify(&repo.proof_car(path, NOW), path).unwrap();
        assert!(!out.is_present());
    }

    #[test]
    fn record_must_be_strict_dag_cbor() {
        let mut repo = k256_repo();
        // a raw-codec record CID (finding: was "proven present")
        let raw = b"not cbor at all".to_vec();
        let path = format!("{POST}/raw");
        repo.put_block(&path, crate::cid::for_raw(&raw), raw);
        let f = repo.verify(&repo.proof_car(&path, NOW), &path).unwrap_err();
        assert!(matches!(*f.error, ProofError::BadRecord { .. }), "{f:?}");
        assert_eq!(f.kind(), ProofErrorKind::NonCanonical);
        assert_eq!(f.rev.unwrap().timestamp_micros(), NOW as u64);
        // a dag-cbor CID over non-canonical CBOR (unsorted keys)
        let unsorted = vec![0xa2, 0x61, b'b', 0x01, 0x61, b'a', 0x02];
        let path = format!("{POST}/unsorted");
        repo.put_block(&path, crate::cid::for_dag_cbor(&unsorted), unsorted);
        let f = repo.verify(&repo.proof_car(&path, NOW), &path).unwrap_err();
        assert!(matches!(
            &*f.error,
            ProofError::BadRecord { error, .. } if matches!(**error, DataModelError::Cbor { .. })
        ));
        assert_eq!(f.kind(), ProofErrorKind::NonCanonical);
        // nesting 33 deep
        let mut deep = Ipld::List(vec![]);
        for _ in 0..32 {
            deep = Ipld::List(vec![deep]);
        }
        let b = crate::dagcbor::encode_with(&deep, &Limits::uncapped(64)).unwrap();
        let path = format!("{POST}/deep");
        repo.put_block(&path, crate::cid::for_dag_cbor(&b), b);
        let f = repo.verify(&repo.proof_car(&path, NOW), &path).unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::NonCanonical);
        // a non-map but strict value is proven (validity is the caller's to check)
        let b = crate::dagcbor::encode(&Ipld::Integer(7)).unwrap();
        let path = format!("{POST}/int");
        repo.put_block(&path, crate::cid::for_dag_cbor(&b), b);
        let out = repo.verify(&repo.proof_car(&path, NOW), &path).unwrap();
        assert_eq!(out.value(), Some(&Ipld::Integer(7)));
        // a record over the S-class materialisation caps is proven but unreadable
        let big = Ipld::Map(
            [(
                "l".to_owned(),
                Ipld::List((0..140_000).map(|_| Ipld::Null).collect()),
            )]
            .into(),
        );
        let b = crate::dagcbor::encode_with(&big, &Limits::uncapped(32)).unwrap();
        let path = format!("{POST}/big");
        repo.put_block(&path, crate::cid::for_dag_cbor(&b), b.clone());
        let out = repo.verify(&repo.proof_car(&path, NOW), &path).unwrap();
        let rec = out.record.unwrap();
        assert!(rec.value.is_none());
        assert_eq!(rec.bytes, b);
    }

    #[test]
    fn exactly_one_root() {
        let repo = k256_repo();
        let path = path7();
        let built = repo.build(&path, NOW, vec![]);
        let x = crate::cid::for_dag_cbor(b"\xa0");
        for roots in [vec![built.commit, x, x], vec![built.commit, built.commit]] {
            let car = write_car(&roots, built.blocks.iter().map(|(c, b)| (c, b.as_slice())));
            let f = repo.verify(&car, &path).unwrap_err();
            assert_eq!(f.kind(), ProofErrorKind::CarMalformed, "{roots:?}");
            // even with caps that let the reader accept several roots
            let caps = ProofCaps {
                car: CarLimits {
                    max_roots: 16,
                    ..CarLimits::PROOF
                },
            };
            let r = verify_proof(
                &car,
                &repo.did,
                &coll(),
                &RecordKey::parse("uetjixxr3a33-3l2s5xxv2z007").unwrap(),
                &repo.key(),
                NOW,
                &caps,
            );
            assert_eq!(
                *r.unwrap_err().error,
                ProofError::Car(CarError::Header("a proof CAR must have exactly one root"))
            );
        }
        let car = write_car(&[], built.blocks.iter().map(|(c, b)| (c, b.as_slice())));
        let f = repo.verify(&car, &path).unwrap_err();
        assert_eq!(*f.error, ProofError::NoRoot);
        assert_eq!(f.kind(), ProofErrorKind::CarMalformed);
    }

    #[test]
    fn unknown_commit_fields_are_signed() {
        let repo = k256_repo();
        let path = path7();
        let mut deep = Ipld::Map(BTreeMap::new());
        for _ in 0..30 {
            deep = Ipld::List(vec![deep]);
        }
        let built = repo.build(
            &path,
            NOW,
            vec![("zext", deep.clone()), ("note", Ipld::String("hi".into()))],
        );
        assert!(repo.verify(&built.car, &path).unwrap().is_present());
        // 33 containers in the commit block: NonCanonical
        let built = repo.build(&path, NOW, vec![("zext", Ipld::List(vec![deep]))]);
        let f = repo.verify(&built.car, &path).unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::NonCanonical);
        assert_eq!(f.commit_cid, Some(built.commit));
        assert_eq!(f.rev, None);
    }

    #[test]
    fn failure_kinds_and_context() {
        let repo = k256_repo();
        let path = path7();
        let built = repo.build(&path, NOW, vec![]);
        let car = &built.car;
        let rkey = RecordKey::parse("uetjixxr3a33-3l2s5xxv2z007").unwrap();
        let caps = ProofCaps::default();
        let run = |car: &[u8], did: &Did, key: &PublicKey, now: i64| {
            verify_proof(car, did, &coll(), &rkey, key, now, &caps)
        };
        // future rev
        let future = repo.proof_car(&path, NOW + 6 * 60 * 1_000_000);
        let f = run(&future, &repo.did, &repo.key(), NOW).unwrap_err();
        assert!(matches!(
            *f.error,
            ProofError::Commit(CommitError::FutureRev(_))
        ));
        assert_eq!(f.kind(), ProofErrorKind::FutureRev);
        // wrong key: BadSignature, with the commit's rev and CID for the stale-proof rule
        let other = PublicKey::Secp256k1(
            *k256::ecdsa::SigningKey::from_slice(&[4u8; 32])
                .unwrap()
                .verifying_key(),
        );
        let f = run(car, &repo.did, &other, NOW).unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::BadSignature);
        assert_eq!(f.rev, Some(Tid::from_parts(NOW as u64, 1).unwrap()));
        assert_eq!(f.commit_cid, Some(built.commit));
        assert!(matches!(
            ProofError::from(f),
            ProofError::Commit(CommitError::Signature(_))
        ));
        // wrong DID
        let did2 = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let f = run(car, &did2, &repo.key(), NOW).unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::DidMismatch);
        // a key whose path nodes are not included
        let far = format!("{POST}/uetjixxr3a33-3l2s5xxv2z049");
        let f = repo.verify(car, &far).unwrap_err();
        assert!(matches!(
            *f.error,
            ProofError::Mst(MstError::MissingBlock(_))
        ));
        assert_eq!(f.kind(), ProofErrorKind::MissingBlock);
        // every single-byte change fails
        for i in 0..car.len() {
            let mut t = car.clone();
            t[i] ^= 0x01;
            assert!(repo.verify(&t, &path).is_err(), "byte {i}");
        }
        // record block dropped
        let rcid = repo.records[&path].0;
        let without: Vec<_> = built
            .blocks
            .iter()
            .filter(|(c, _)| *c != rcid)
            .map(|(c, b)| (c, b.as_slice()))
            .collect();
        let f = repo
            .verify(&write_car(&[built.commit], without), &path)
            .unwrap_err();
        assert_eq!(*f.error, ProofError::MissingRecord(rcid));
        assert_eq!(f.kind(), ProofErrorKind::MissingBlock);
        // commit missing; a raw root (its block present) is not a commit
        let missing = crate::cid::for_dag_cbor(b"\xa0");
        let f = repo.verify(&write_car(&[missing], []), &path).unwrap_err();
        assert_eq!(*f.error, ProofError::MissingCommit(missing));
        assert_eq!(f.kind(), ProofErrorKind::MissingBlock);
        assert_eq!(f.commit_cid, Some(missing));
        let raw_root = crate::cid::for_raw(&built.blocks[0].1);
        let f = repo
            .verify(
                &write_car(&[raw_root], [(&raw_root, built.blocks[0].1.as_slice())]),
                &path,
            )
            .unwrap_err();
        assert_eq!(*f.error, ProofError::BadRoot);
        assert_eq!(f.kind(), ProofErrorKind::BadCommit);
        // CAR caps
        let tight = ProofCaps {
            car: CarLimits {
                max_blocks: 2,
                ..CarLimits::PROOF
            },
        };
        let f = verify_proof(car, &repo.did, &coll(), &rkey, &repo.key(), NOW, &tight).unwrap_err();
        assert_eq!(*f.error, ProofError::Car(CarError::TooManyBlocks(2)));
        assert_eq!(f.kind(), ProofErrorKind::TooManyBlocks);
        let small = ProofCaps {
            car: CarLimits {
                max_total_bytes: 100,
                ..CarLimits::PROOF
            },
        };
        let f = verify_proof(car, &repo.did, &coll(), &rkey, &repo.key(), NOW, &small).unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::CarTooLarge);
        assert_eq!(f.commit_cid, None);
    }

    #[test]
    fn kind_names() {
        assert_eq!(ProofErrorKind::CarMalformed.to_string(), "CarMalformed");
        assert_eq!(ProofErrorKind::RecordMismatch.as_str(), "RecordMismatch");
        let k = |e: ProofError| e.kind();
        let c = crate::cid::for_raw(b"x");
        assert_eq!(
            k(ProofError::Car(CarError::HashMismatch(c))),
            ProofErrorKind::BlockHashMismatch
        );
        assert_eq!(
            k(ProofError::Mst(MstError::Cid(CidError::Mismatch(c)))),
            ProofErrorKind::BlockHashMismatch
        );
        assert_eq!(
            k(ProofError::Mst(MstError::DepthExceeded(32))),
            ProofErrorKind::MstTooDeep
        );
        assert_eq!(
            k(ProofError::Mst(MstError::Layer)),
            ProofErrorKind::MstInvalid
        );
        assert_eq!(
            k(ProofError::Mst(MstError::Decode(DataModelError::Float))),
            ProofErrorKind::NonCanonical
        );
        assert_eq!(
            k(ProofError::Commit(CommitError::Version(2))),
            ProofErrorKind::BadCommit
        );
        assert_eq!(
            k(ProofError::Car(CarError::Manifest("x"))),
            ProofErrorKind::CarMalformed
        );
    }

    #[test]
    fn verify_in_block_set_with_read_caps() {
        let repo = k256_repo();
        let path = path7();
        let built = repo.build(&path, NOW, vec![]);
        let blocks: BTreeMap<Cid, Vec<u8>> = built.blocks.iter().cloned().collect();
        let uri = AtUri::parse(&format!("at://{}/{path}", repo.did)).unwrap();
        let out = verify_proof_in(
            &blocks,
            &built.commit,
            &uri,
            &repo.key(),
            NOW,
            &ReadCaps::PROOF,
        )
        .unwrap();
        assert_eq!(out.record_cid(), Some(&repo.records[&path].0));
        let n = built.blocks.len();
        let total: usize = built.blocks.iter().map(|(_, b)| b.len()).sum();
        // exactly at the caps is fine; one less fails with the cap's kind
        let at = ReadCaps {
            max_bytes: total,
            max_blocks: n,
        };
        assert!(verify_proof_in(&blocks, &built.commit, &uri, &repo.key(), NOW, &at).is_ok());
        let f = verify_proof_in(
            &blocks,
            &built.commit,
            &uri,
            &repo.key(),
            NOW,
            &ReadCaps {
                max_blocks: n - 1,
                ..at
            },
        )
        .unwrap_err();
        assert_eq!(*f.error, ProofError::Car(CarError::TooManyBlocks(n - 1)));
        assert_eq!(f.kind(), ProofErrorKind::TooManyBlocks);
        assert!(f.rev.is_some());
        let f = verify_proof_in(
            &blocks,
            &built.commit,
            &uri,
            &repo.key(),
            NOW,
            &ReadCaps {
                max_bytes: total - 1,
                ..at
            },
        )
        .unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::CarTooLarge);
        // blocks from an unverified source are hash-checked
        let mut bad = blocks.clone();
        let rcid = repo.records[&path].0;
        bad.insert(rcid, b"\xa0".to_vec());
        let f = verify_proof_in(
            &bad,
            &built.commit,
            &uri,
            &repo.key(),
            NOW,
            &ReadCaps::PROOF,
        )
        .unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::BlockHashMismatch);
        let mut bad = blocks.clone();
        bad.insert(built.commit, b"\xa0".to_vec());
        let f = verify_proof_in(
            &bad,
            &built.commit,
            &uri,
            &repo.key(),
            NOW,
            &ReadCaps::PROOF,
        )
        .unwrap_err();
        assert_eq!(f.kind(), ProofErrorKind::BlockHashMismatch);
    }

    fn manifest(items: Vec<Ipld>, extra: Vec<(&str, Ipld)>) -> Vec<u8> {
        let mut m = BTreeMap::new();
        m.insert("v".to_owned(), Ipld::Integer(1));
        m.insert("items".to_owned(), Ipld::List(items));
        for (k, v) in extra {
            m.insert(k.to_owned(), v);
        }
        crate::dagcbor::encode(&Ipld::Map(m)).unwrap()
    }

    fn item(uri: &str, cid: Option<Cid>, commit: Cid) -> Ipld {
        let mut m = BTreeMap::new();
        m.insert("uri".to_owned(), Ipld::String(uri.to_owned()));
        m.insert("cid".to_owned(), cid.map_or(Ipld::Null, Ipld::Link));
        m.insert("commit".to_owned(), Ipld::Link(commit));
        Ipld::Map(m)
    }

    fn bundle_car(manifest: &[u8], blocks: &[(Cid, Vec<u8>)]) -> Vec<u8> {
        let mcid = crate::cid::for_dag_cbor(manifest);
        let mut all = vec![(mcid, manifest.to_vec())];
        all.extend(blocks.iter().cloned());
        write_car(&[mcid], all.iter().map(|(c, b)| (c, b.as_slice())))
    }

    #[test]
    fn bundles() {
        let repo = k256_repo();
        let p7 = path7();
        let absent = format!("{POST}/uetjixxr3a33-3l2s5xxv2z007x");
        let b7 = repo.build(&p7, NOW, vec![]);
        let ba = repo.build(&absent, NOW, vec![]);
        let mut blocks = b7.blocks.clone();
        blocks.extend(ba.blocks.iter().cloned());
        let uri = |p: &str| format!("at://{}/{p}", repo.did);
        let c7 = repo.records[&p7].0;
        let wrong = repo.records[&format!("{POST}/uetjixxr3a33-3l2s5xxv2z008")].0;
        let items = vec![
            item(&uri(&p7), Some(c7), b7.commit),     // 0: ok, present
            item(&uri(&absent), None, ba.commit),     // 1: ok, absent
            item(&uri(&p7), None, b7.commit),         // 2: claims absent, is present
            item(&uri(&absent), Some(c7), ba.commit), // 3: claims present, is absent
            item(&uri(&p7), Some(wrong), b7.commit),  // 4: wrong CID
            item("at://alice.test/a.b.c/x", None, b7.commit), // 5: not canonical
            Ipld::Integer(5),                         // 6: not a map
            item(&uri(&p7), Some(c7), crate::cid::for_dag_cbor(b"\xa1")), // 7: commit missing
        ];
        let car = bundle_car(&manifest(items, vec![("note", Ipld::Bool(true))]), &blocks);
        let b = ProofBundle::parse(&car, &BundleCaps::default()).unwrap();
        assert_eq!(b.items.len(), 8);
        let key = repo.key();
        let out = b.verify_item(0, &key, NOW).unwrap();
        assert_eq!(out.record_cid(), Some(&c7));
        assert!(!b.verify_item(1, &key, NOW).unwrap().is_present());
        for (i, claimed, proven) in [
            (2, None, Some(c7)),
            (3, Some(c7), None),
            (4, Some(wrong), Some(c7)),
        ] {
            let f = b.verify_item(i, &key, NOW).unwrap_err();
            assert_eq!(f.kind(), ProofErrorKind::RecordMismatch, "{i}");
            assert_eq!(
                *f.error,
                ProofError::RecordMismatch(Box::new(Mismatch { claimed, proven })),
                "{i}"
            );
            assert!(f.rev.is_some() && f.commit_cid.is_some());
        }
        for i in [5, 6, 8] {
            let f = b.verify_item(i, &key, NOW).unwrap_err();
            assert_eq!(f.kind(), ProofErrorKind::CarMalformed, "{i}");
        }
        assert_eq!(
            b.verify_item(7, &key, NOW).unwrap_err().kind(),
            ProofErrorKind::MissingBlock
        );
        // per-item read caps
        let tight = BundleCaps {
            item: ReadCaps {
                max_bytes: 1_000_000,
                max_blocks: 2,
            },
            ..BundleCaps::default()
        };
        let bt = ProofBundle::parse(&car, &tight).unwrap();
        assert_eq!(
            bt.verify_item(0, &key, NOW).unwrap_err().kind(),
            ProofErrorKind::TooManyBlocks
        );
        // manifest-level problems
        let parse =
            |m: Vec<u8>| ProofBundle::parse(&bundle_car(&m, &blocks), &BundleCaps::default());
        let mut m = BTreeMap::new();
        m.insert("v".to_owned(), Ipld::Integer(2));
        m.insert("items".to_owned(), Ipld::List(vec![]));
        assert_eq!(
            parse(crate::dagcbor::encode(&Ipld::Map(m)).unwrap()),
            Err(ProofError::Car(CarError::Manifest("manifest v must be 1")))
        );
        let many = (0..26)
            .map(|_| item(&uri(&p7), Some(c7), b7.commit))
            .collect();
        assert_eq!(
            parse(manifest(many, vec![])).unwrap_err().kind(),
            ProofErrorKind::CarMalformed
        );
        let ok = (0..25)
            .map(|_| item(&uri(&p7), Some(c7), b7.commit))
            .collect();
        assert_eq!(parse(manifest(ok, vec![])).unwrap().items.len(), 25);
        assert!(parse(crate::dagcbor::encode(&Ipld::List(vec![])).unwrap()).is_err());
        // exactly one root; manifest present
        let mcid = crate::cid::for_dag_cbor(b"\xa0");
        let two = write_car(&[mcid, mcid], [(&mcid, &b"\xa0"[..])]);
        assert_eq!(
            ProofBundle::parse(&two, &BundleCaps::default())
                .unwrap_err()
                .kind(),
            ProofErrorKind::CarMalformed
        );
        let none = write_car(&[mcid], []);
        assert_eq!(
            ProofBundle::parse(&none, &BundleCaps::default()).unwrap_err(),
            ProofError::Car(CarError::Manifest("the manifest block is missing"))
        );
    }
}
