// SPDX-License-Identifier: AGPL-3.0-or-later
//! The record store (`sync.md` §5.6, §10.2–§10.5): one current version per uri, tombstones,
//! order keys, first-seen times, and the `absent`/`obs` fold inputs.
//!
//! **Version order (§5.6.1).** Every admitted observation carries an [`OrderKey`]
//! `(revLo, sentAt)`, compared by `revLo` bytewise (`""` lowest), then `sentAt`.
//!
//! **Admission into the store** ([`admit_version`], §5.6.2), per uri:
//! ```text
//! tombstone with key t, key(o) < t          → Suppressed (I4)
//! stored s, s.cid = o.cid                   → Merged: level, rev, key = max; exact = ∨;
//!                                             idHint = ∧; firstSeen = min; src of the max (level, src)
//! s.exact ∧ o.exact ∧ s.revLo = o.revLo     → Conflict (I3): the higher (level, ¬idHint) is kept,
//!                                             the stored one at equal rank; the key stays monotone
//! none, or key(o) > key(s)                  → Inserted / Replaced (a tombstone is removed)
//! otherwise                                 → Older (dropped)
//! ```
//! The conflict test compares `revLo` rather than the whole key: two exact observations at the
//! same rev describe the same repo state, whatever their send times, so different CIDs there are
//! an equivocation, not an update (I3 "the same exact rev with a different CID is a conflict").
//!
//! **Tombstones** ([`apply_delete`], §5.6.3) are written only with a [`DeleteEvidence`]: a complete
//! stable listing, a `RecordNotFound` from the author's PDS, or an own delete. A tombstone with key
//! `t` replaces a version only if `t > key(s)`, keeps the greatest key, and suppresses every
//! observation with a smaller key; an observation with an equal key wins. A uri never has both a
//! row and a tombstone.
//!
//! **Invariants enforced here.** I2: every stored version hashes to its CID
//! ([`Batch::put_version`] refuses anything else). I3/I4: the per-uri key never decreases. I5:
//! [`Level`] has only `pds` and `proven`; for the same `(uri, cid)` the level only increases.
//! I9: every write is idempotent and key-monotone, and a [`Batch`] is committed atomically with
//! one [`KvStore::write`].
//!
//! **First-seen times (§5.6.5).** A row keeps `firstSeen` of its current CID plus
//! `(firstCid, firstSeenUri)`, the earliest `(time, cid)` of any version ever observed for the
//! uri; observations that are dropped as older still lower it, and tombstones carry it, so
//! `firstSeenUri` does not depend on arrival order. First-seen values never move later.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use lessdugin_atp::syntax::AtUri;
use lessdugin_atp::{Cid, Ipld};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::host::{Dir, HostError, KeyRange, KvOp, KvStore};
use crate::limits;
use crate::store::admit::{RECORD_LIMITS, Validity};
use crate::store::blob::{BlobRow, CursorRow, ForumRow};
use crate::store::repo::{AccountStatus, HostRow, RepoRow};
use crate::store::row::{self, ROW_VERSION, RowError};
use crate::store::schema::{self, RecKey, tag};

/// Default page size of store scans.
pub const SCAN_PAGE: u32 = 512;
/// Keys per `get_many` request of a [`Batch`].
pub const READ_CHUNK: usize = 1_000;

/// Content trust level of a stored version (`sync.md` §1.1). Hints are never stored (I5), so
/// there is no `hint` level here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Fetched from the author's PDS with a matching CID.
    Pds,
    /// Proven by `proofs/1`.
    Proven,
}

/// Where a stored version came from. The declaration order is the precedence used to pick the
/// source of a merged version at equal level.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// A point fetch (`getRecord`).
    Get,
    /// A range listing (`listRecords`).
    List,
    /// A current third-party proof from an indexer (its DID).
    Indexer(String),
    /// A proof from the author's PDS (`sync.getRecord`).
    Proof,
    /// An own write confirmed by the PDS.
    Own,
}

impl Source {
    /// `"get"`, `"list"`, `"indexer:<did>"`, `"proof"` or `"own"`.
    pub fn as_string(&self) -> String {
        match self {
            Self::Get => "get".into(),
            Self::List => "list".into(),
            Self::Indexer(d) => format!("indexer:{d}"),
            Self::Proof => "proof".into(),
            Self::Own => "own".into(),
        }
    }

    /// Parses [`Source::as_string`].
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "get" => Self::Get,
            "list" => Self::List,
            "proof" => Self::Proof,
            "own" => Self::Own,
            _ => Self::Indexer(s.strip_prefix("indexer:")?.to_owned()),
        })
    }
}

impl Serialize for Source {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_string())
    }
}

impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Source::parse(&s).ok_or_else(|| serde::de::Error::custom("unknown record source"))
    }
}

/// The order key `(revLo, sentAt)` of an observation (`sync.md` §5.6.1). Field order gives the
/// comparison: `revLo` bytewise (`""` sorts first), then `sentAt`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderKey {
    /// A rev known to be at most the repo state observed (a TID, or `""`).
    pub rev_lo: String,
    /// When the request was sent (ms).
    pub sent_at: i64,
}

impl OrderKey {
    /// A key.
    pub fn new(rev_lo: impl Into<String>, sent_at: i64) -> Self {
        Self {
            rev_lo: rev_lo.into(),
            sent_at,
        }
    }
}

/// Why a delete is confirmed (`sync.md` §5.6.3). Hints (Jetstream deletes, indexer changes) are
/// not evidence: they only schedule a point fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeleteEvidence {
    /// A complete, stable listing of the uri's range did not contain it.
    CompleteListing,
    /// The author's PDS answered `getRecord` with `RecordNotFound`.
    RecordNotFound,
    /// An own delete, confirmed by the PDS.
    OwnDelete,
}

/// `RecordRow` (`sync.md` §10.5): the current version of one uri.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordRow {
    /// Row version.
    pub v: u32,
    /// The record CID (dag-cbor, sha2-256).
    pub cid: Cid,
    /// The DAG-CBOR bytes of the value; they hash to `cid` (I2).
    #[serde(with = "serde_bytes")]
    pub cbor: Vec<u8>,
    /// Trust level.
    pub level: Level,
    /// The commit rev of the proof, for `proven`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Order key: rev lower bound.
    pub rev_lo: String,
    /// Order key: send time (ms).
    pub sent_at: i64,
    /// `rev_lo` is the exact rev of an observation of this version.
    pub exact: bool,
    /// Source (of the highest-ranked observation).
    pub src: Source,
    /// Fetched while the author's identity was only a hint (§1.5).
    pub id_hint: bool,
    /// Passed `data-model.md` §7.
    pub valid: bool,
    /// The first failing rule ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Rule violations, for the raw view (bounded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
    /// First-seen time of `cid` (µs).
    pub first_seen: i64,
    /// The CID of the earliest observed version of the uri.
    pub first_cid: Cid,
    /// The first-seen time of `first_cid` (µs): the uri's first-seen time.
    pub first_seen_uri: i64,
    /// When this version was last fetched (ms).
    pub fetched_at: i64,
}

impl RecordRow {
    /// The order key.
    pub fn key(&self) -> OrderKey {
        OrderKey::new(self.rev_lo.clone(), self.sent_at)
    }

    /// The validity verdict.
    pub fn validity(&self) -> Validity {
        Validity {
            rule: self.rule.clone(),
            violations: self.violations.clone(),
        }
    }

    /// The value as IPLD (strict DAG-CBOR under [`RECORD_LIMITS`], which admission wrote it with).
    pub fn ipld(&self) -> Result<Ipld, lessdugin_atp::datamodel::DataModelError> {
        lessdugin_atp::dagcbor::decode_with(&self.cbor, &RECORD_LIMITS)
    }

    /// The value as atproto JSON.
    pub fn value_json(
        &self,
    ) -> Result<serde_json::Value, lessdugin_atp::datamodel::DataModelError> {
        lessdugin_atp::datamodel::ipld_to_json(&self.ipld()?)
    }

    fn carry(&self) -> Option<(Cid, i64)> {
        Some((self.first_cid, self.first_seen_uri))
    }

    /// Lowers first-seen times with an observation of `(cid, t)`; returns `true` if changed.
    fn lower_first_seen(&mut self, cid: Cid, t: i64) -> bool {
        let before = (self.first_seen, self.first_cid, self.first_seen_uri);
        let (fc, ft) = carry_min(self.carry(), (cid, t));
        self.first_cid = fc;
        self.first_seen_uri = ft;
        if cid == self.cid {
            self.first_seen = self.first_seen.min(t);
        }
        if self.first_cid == self.cid {
            self.first_seen = self.first_seen.min(self.first_seen_uri);
        }
        before != (self.first_seen, self.first_cid, self.first_seen_uri)
    }

    /// Merges an observation of the same CID; returns `true` if anything changed.
    fn merge_same(&mut self, v: Version) -> bool {
        let before = self.meta();
        if (v.level, &v.src) > (self.level, &self.src) {
            self.src = v.src.clone();
        }
        self.level = self.level.max(v.level);
        self.rev = self.rev.take().max(v.proven_rev);
        if v.key > self.key() {
            self.rev_lo = v.key.rev_lo;
            self.sent_at = v.key.sent_at;
        }
        self.exact |= v.exact;
        self.id_hint &= v.id_hint;
        self.valid = v.validity.is_valid();
        self.rule = v.validity.rule;
        self.violations = v.validity.violations;
        self.fetched_at = self.fetched_at.max(v.fetched_at_ms);
        self.lower_first_seen(v.cid, v.first_seen_us);
        before != self.meta()
    }

    #[allow(clippy::type_complexity)]
    fn meta(
        &self,
    ) -> (
        Level,
        Source,
        Option<String>,
        OrderKey,
        bool,
        bool,
        Validity,
        (i64, Cid, i64),
        i64,
    ) {
        (
            self.level,
            self.src.clone(),
            self.rev.clone(),
            self.key(),
            self.exact,
            self.id_hint,
            self.validity(),
            (self.first_seen, self.first_cid, self.first_seen_uri),
            self.fetched_at,
        )
    }
}

/// `TombRow` (`sync.md` §10.5), plus the uri's first-seen carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TombRow {
    /// Row version.
    pub v: u32,
    /// Order key: rev lower bound.
    pub rev_lo: String,
    /// Order key: send time (ms).
    pub sent_at: i64,
    /// When the tombstone was written or last raised (ms; garbage collection).
    pub at: i64,
    /// The evidence of the delete.
    pub ev: DeleteEvidence,
    /// The CID of the uri's earliest observed version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_cid: Option<Cid>,
    /// Its first-seen time (µs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen_uri: Option<i64>,
}

impl TombRow {
    fn new(d: &Delete, carry: Option<(Cid, i64)>) -> Self {
        Self {
            v: ROW_VERSION,
            rev_lo: d.key.rev_lo.clone(),
            sent_at: d.key.sent_at,
            at: d.at_ms,
            ev: d.evidence,
            first_cid: carry.map(|c| c.0),
            first_seen_uri: carry.map(|c| c.1),
        }
    }

    /// The order key.
    pub fn key(&self) -> OrderKey {
        OrderKey::new(self.rev_lo.clone(), self.sent_at)
    }

    fn carry(&self) -> Option<(Cid, i64)> {
        Some((self.first_cid?, self.first_seen_uri?))
    }

    fn lower_first_seen(&mut self, cid: Cid, t: i64) -> bool {
        let before = self.carry();
        let (c, t) = carry_min(before, (cid, t));
        self.first_cid = Some(c);
        self.first_seen_uri = Some(t);
        before != self.carry()
    }
}

/// The earlier of two `(cid, time)` pairs: by time, then by the CID string.
fn carry_min(a: Option<(Cid, i64)>, b: (Cid, i64)) -> (Cid, i64) {
    match a {
        None => b,
        Some(a) => {
            let b_first = b.1 < a.1 || (b.1 == a.1 && b.0.to_string() < a.0.to_string());
            if b_first { b } else { a }
        }
    }
}

/// A version to admit into the store: an admitted record (`super::admit`) plus its observation
/// context (§5.6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// The record uri.
    pub uri: AtUri,
    /// The record CID.
    pub cid: Cid,
    /// The DAG-CBOR bytes (must hash to `cid`).
    pub cbor: Vec<u8>,
    /// The validity verdict.
    pub validity: Validity,
    /// Trust level.
    pub level: Level,
    /// For `proven`: the commit rev of the proof.
    pub proven_rev: Option<String>,
    /// The author's identity was only a hint.
    pub id_hint: bool,
    /// The order key.
    pub key: OrderKey,
    /// `key.rev_lo` is exact (§5.6.1 table).
    pub exact: bool,
    /// Source.
    pub src: Source,
    /// First-seen time of this exact version (µs): the earliest of the admission time and any
    /// hint time held in memory.
    pub first_seen_us: i64,
    /// Fetch time (ms).
    pub fetched_at_ms: i64,
}

/// A confirmed delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delete {
    /// The order key (a lower bound observed from the PDS, never a hint rev).
    pub key: OrderKey,
    /// The evidence.
    pub evidence: DeleteEvidence,
    /// Now (ms).
    pub at_ms: i64,
}

/// The state of one uri.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // short-lived; boxing would only add an allocation
pub enum Slot {
    /// Nothing known.
    Empty,
    /// A current version.
    Rec(RecordRow),
    /// A confirmed delete.
    Tomb(TombRow),
}

impl Slot {
    /// The order key of the current state.
    pub fn key(&self) -> Option<OrderKey> {
        match self {
            Slot::Empty => None,
            Slot::Rec(r) => Some(r.key()),
            Slot::Tomb(t) => Some(t.key()),
        }
    }

    /// The current CID.
    pub fn cid(&self) -> Option<Cid> {
        match self {
            Slot::Rec(r) => Some(r.cid),
            _ => None,
        }
    }

    /// The uri's first-seen time (µs), if known.
    pub fn first_seen_uri(&self) -> Option<i64> {
        match self {
            Slot::Empty => None,
            Slot::Rec(r) => Some(r.first_seen_uri),
            Slot::Tomb(t) => t.first_seen_uri,
        }
    }
}

/// A same-rev conflict (I3): logged by the caller, which also enqueues the DID at P1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The rev both exact observations claim.
    pub rev: String,
    /// The stored CID and source.
    pub stored: (Cid, Source),
    /// The observed CID and source.
    pub observed: (Cid, Source),
    /// The observation won (higher level).
    pub replaced: bool,
}

/// What [`admit_version`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Stored; there was no current version (a tombstone with a smaller or equal key was
    /// cleared).
    Inserted,
    /// Stored, replacing an older version.
    Replaced {
        /// The replaced CID.
        previous: Cid,
    },
    /// The same version was stored; its attributes were merged.
    Merged,
    /// Dropped: a tombstone with a greater key (I4).
    Suppressed,
    /// Dropped: the stored version is newer.
    Older,
    /// Same exact rev, different CID (I3).
    Conflict(Conflict),
}

impl Admission {
    /// `true` if the observed version is now the current one.
    pub fn is_current(&self) -> bool {
        match self {
            Admission::Inserted | Admission::Replaced { .. } | Admission::Merged => true,
            Admission::Conflict(c) => c.replaced,
            Admission::Suppressed | Admission::Older => false,
        }
    }
}

/// What [`apply_delete`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TombOutcome {
    /// A tombstone was written (removing the version if there was one).
    Created {
        /// The removed CID.
        removed: Option<Cid>,
    },
    /// An existing tombstone got a greater key.
    Raised,
    /// An existing tombstone already had a greater or equal key.
    Unchanged,
    /// The stored version's key is greater or equal: the delete is older, nothing changes.
    Ignored,
}

/// Rank for conflicts: the level, then a doc-level identity over an identity hint.
fn rank(level: Level, id_hint: bool) -> (Level, bool) {
    (level, !id_hint)
}

fn new_row(v: Version, carry: Option<(Cid, i64)>) -> RecordRow {
    let (first_cid, first_seen_uri) = carry_min(carry, (v.cid, v.first_seen_us));
    let mut first_seen = v.first_seen_us;
    if first_cid == v.cid {
        first_seen = first_seen.min(first_seen_uri);
    }
    RecordRow {
        v: ROW_VERSION,
        cid: v.cid,
        cbor: v.cbor,
        level: v.level,
        rev: v.proven_rev,
        rev_lo: v.key.rev_lo,
        sent_at: v.key.sent_at,
        exact: v.exact,
        src: v.src,
        id_hint: v.id_hint,
        valid: v.validity.is_valid(),
        rule: v.validity.rule,
        violations: v.validity.violations,
        first_seen,
        first_cid,
        first_seen_uri,
        fetched_at: v.fetched_at_ms,
    }
}

/// Admits `v` into the state `slot` of its uri (§5.6.2). Returns the new state, what happened,
/// and whether the state changed.
pub fn admit_version(slot: Slot, v: Version) -> (Slot, Admission, bool) {
    match slot {
        Slot::Empty => (Slot::Rec(new_row(v, None)), Admission::Inserted, true),
        Slot::Tomb(mut t) => {
            if v.key < t.key() {
                let changed = t.lower_first_seen(v.cid, v.first_seen_us);
                (Slot::Tomb(t), Admission::Suppressed, changed)
            } else {
                let carry = t.carry();
                (Slot::Rec(new_row(v, carry)), Admission::Inserted, true)
            }
        }
        Slot::Rec(mut s) => {
            if s.cid == v.cid {
                let changed = s.merge_same(v);
                (Slot::Rec(s), Admission::Merged, changed)
            } else if s.exact && v.exact && s.rev_lo == v.key.rev_lo {
                let conflict = |replaced| Conflict {
                    rev: s.rev_lo.clone(),
                    stored: (s.cid, s.src.clone()),
                    observed: (v.cid, v.src.clone()),
                    replaced,
                };
                if rank(v.level, v.id_hint) > rank(s.level, s.id_hint) {
                    let c = conflict(true);
                    let key = s.key().max(v.key.clone());
                    let mut row = new_row(v, s.carry());
                    row.rev_lo = key.rev_lo;
                    row.sent_at = key.sent_at;
                    (Slot::Rec(row), Admission::Conflict(c), true)
                } else {
                    let c = conflict(false);
                    let changed = s.lower_first_seen(v.cid, v.first_seen_us);
                    (Slot::Rec(s), Admission::Conflict(c), changed)
                }
            } else if v.key > s.key() {
                let previous = s.cid;
                let row = new_row(v, s.carry());
                (Slot::Rec(row), Admission::Replaced { previous }, true)
            } else {
                let changed = s.lower_first_seen(v.cid, v.first_seen_us);
                (Slot::Rec(s), Admission::Older, changed)
            }
        }
    }
}

/// Applies a confirmed delete to the state `slot` of its uri (§5.6.3).
pub fn apply_delete(slot: Slot, d: &Delete) -> (Slot, TombOutcome, bool) {
    match slot {
        Slot::Empty => (
            Slot::Tomb(TombRow::new(d, None)),
            TombOutcome::Created { removed: None },
            true,
        ),
        Slot::Tomb(mut t) => {
            if d.key > t.key() {
                t.rev_lo = d.key.rev_lo.clone();
                t.sent_at = d.key.sent_at;
                t.ev = d.evidence;
                t.at = t.at.max(d.at_ms);
                (Slot::Tomb(t), TombOutcome::Raised, true)
            } else {
                (Slot::Tomb(t), TombOutcome::Unchanged, false)
            }
        }
        Slot::Rec(s) => {
            if d.key > s.key() {
                let removed = Some(s.cid);
                (
                    Slot::Tomb(TombRow::new(d, s.carry())),
                    TombOutcome::Created { removed },
                    true,
                )
            } else {
                (Slot::Rec(s), TombOutcome::Ignored, false)
            }
        }
    }
}

/// Store errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The key-value store failed.
    #[error(transparent)]
    Host(#[from] HostError),
    /// A row could not be encoded.
    #[error(transparent)]
    Row(#[from] RowError),
    /// A version's bytes do not hash to its CID, or the CID is not dag-cbor (I2).
    #[error("record bytes of {uri} do not hash to {cid}")]
    CidMismatch {
        /// The record uri.
        uri: String,
        /// The claimed CID.
        cid: String,
    },
}

/// A stored version with its key parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRecord {
    /// Scope, collection, DID and rkey.
    pub key: RecKey,
    /// The row.
    pub row: RecordRow,
}

impl StoredRecord {
    /// The at-uri.
    pub fn uri(&self) -> String {
        self.key.uri()
    }
}

/// Decodes a current-version row; anything undecodable or of another version is absent.
fn decode_versioned<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    v: impl Fn(&T) -> u32,
) -> Option<T> {
    row::decode::<T>(bytes).ok().filter(|r| v(r) == ROW_VERSION)
}

fn decode_rec(bytes: &[u8]) -> Option<RecordRow> {
    decode_versioned(bytes, |r: &RecordRow| r.v)
}

fn decode_tomb(bytes: &[u8]) -> Option<TombRow> {
    decode_versioned(bytes, |r: &TombRow| r.v)
}

fn slot_from(rec: Option<RecordRow>, tomb: Option<TombRow>) -> Slot {
    match (rec, tomb) {
        // Never written by this module; repaired on the next write of the uri.
        (Some(r), Some(t)) => {
            if t.key() > r.key() {
                Slot::Tomb(t)
            } else {
                Slot::Rec(r)
            }
        }
        (Some(r), None) => Slot::Rec(r),
        (None, Some(t)) => Slot::Tomb(t),
        (None, None) => Slot::Empty,
    }
}

/// I2 for a version about to be stored.
fn check_version_bytes(v: &Version) -> Result<(), StoreError> {
    let ok = v.cid.codec() == lessdugin_atp::cid::DAG_CBOR
        && lessdugin_atp::cid::verify(&v.cid, &v.cbor).is_ok();
    if ok {
        Ok(())
    } else {
        Err(StoreError::CidMismatch {
            uri: v.uri.to_string(),
            cid: v.cid.to_string(),
        })
    }
}

/// Reads one page of `range` and narrows `range` past it; `None` when the range is exhausted.
async fn scan_page<K: KvStore>(
    kv: &K,
    range: &mut Option<KeyRange>,
    dir: Dir,
    limit: u32,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StoreError> {
    let Some(r) = range.clone() else {
        return Ok(Vec::new());
    };
    let limit = limit.clamp(1, 10_000);
    let page = kv.scan(r.clone(), dir, limit).await?;
    *range = match page.last() {
        Some((last, _)) if page.len() as u32 == limit => Some(match dir {
            Dir::Forward => KeyRange {
                start: Bound::Excluded(last.clone()),
                end: r.end,
            },
            Dir::Reverse => KeyRange {
                start: r.start,
                end: Bound::Excluded(last.clone()),
            },
        }),
        _ => None,
    };
    Ok(page)
}

/// A paged scan over record rows, in key order (rkey order within one `(scope, coll, did)`).
pub struct RecordScan<'k, K: KvStore> {
    kv: &'k K,
    range: Option<KeyRange>,
    dir: Dir,
    page: u32,
}

impl<'k, K: KvStore> RecordScan<'k, K> {
    fn new(kv: &'k K, prefix: &[u8], dir: Dir) -> Self {
        Self {
            kv,
            range: Some(KeyRange::prefix(prefix)),
            dir,
            page: SCAN_PAGE,
        }
    }

    /// Sets the page size (1..=10,000).
    pub fn with_page_size(mut self, n: u32) -> Self {
        self.page = n.clamp(1, 10_000);
        self
    }

    /// The next page of decodable rows; empty at the end.
    pub async fn next_page(&mut self) -> Result<Vec<StoredRecord>, StoreError> {
        while self.range.is_some() {
            let page = scan_page(self.kv, &mut self.range, self.dir, self.page).await?;
            let out: Vec<StoredRecord> = page
                .iter()
                .filter_map(|(k, v)| {
                    Some(StoredRecord {
                        key: RecKey::parse(k, tag::REC)?,
                        row: decode_rec(v)?,
                    })
                })
                .collect();
            if !out.is_empty() {
                return Ok(out);
            }
        }
        Ok(Vec::new())
    }

    /// Every remaining row.
    pub async fn collect(self) -> Result<Vec<StoredRecord>, StoreError> {
        self.take(usize::MAX).await
    }

    /// At most `n` rows.
    pub async fn take(mut self, n: usize) -> Result<Vec<StoredRecord>, StoreError> {
        let mut out = Vec::new();
        while out.len() < n {
            let page = self.next_page().await?;
            if page.is_empty() {
                break;
            }
            out.extend(page);
        }
        out.truncate(n);
        Ok(out)
    }
}

/// The record store over the `cache` [`KvStore`].
pub struct RecordStore<'k, K: KvStore> {
    kv: &'k K,
}

impl<K: KvStore> Clone for RecordStore<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: KvStore> Copy for RecordStore<'_, K> {}

impl<'k, K: KvStore> RecordStore<'k, K> {
    /// A store over `kv` (normally `Host::cache()`).
    pub fn new(kv: &'k K) -> Self {
        Self { kv }
    }

    /// The underlying key-value store.
    pub fn kv(&self) -> &'k K {
        self.kv
    }

    /// A new atomic write batch.
    pub fn batch(&self) -> Batch<'k, K> {
        Batch::new(self.kv)
    }

    async fn get_raw(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, StoreError> {
        Ok(self.kv.get_many(keys).await?)
    }

    async fn get_one<T>(
        &self,
        key: Vec<u8>,
        decode: fn(&[u8]) -> Option<T>,
    ) -> Result<Option<T>, StoreError> {
        Ok(self
            .get_raw(vec![key])
            .await?
            .pop()
            .flatten()
            .and_then(|b| decode(&b)))
    }

    /// The current version of `uri`.
    pub async fn get(&self, uri: &AtUri) -> Result<Option<RecordRow>, StoreError> {
        self.get_key(&RecKey::of_uri(uri)).await
    }

    /// The current version at a key.
    pub async fn get_key(&self, k: &RecKey) -> Result<Option<RecordRow>, StoreError> {
        self.get_one(k.rec(), decode_rec).await
    }

    /// The current versions of several keys (one read).
    pub async fn get_many(&self, keys: &[RecKey]) -> Result<Vec<Option<RecordRow>>, StoreError> {
        let raw = self.get_raw(keys.iter().map(RecKey::rec).collect()).await?;
        Ok(raw
            .into_iter()
            .map(|b| b.and_then(|b| decode_rec(&b)))
            .collect())
    }

    /// The tombstone of `uri`.
    pub async fn tombstone_of(&self, uri: &AtUri) -> Result<Option<TombRow>, StoreError> {
        self.get_one(RecKey::of_uri(uri).tomb(), decode_tomb).await
    }

    /// The state of a uri.
    pub async fn slot(&self, k: &RecKey) -> Result<Slot, StoreError> {
        let mut raw = self.get_raw(vec![k.rec(), k.tomb()]).await?;
        let tomb = raw.pop().flatten().and_then(|b| decode_tomb(&b));
        let rec = raw.pop().flatten().and_then(|b| decode_rec(&b));
        Ok(slot_from(rec, tomb))
    }

    /// Every stored version of a forum (`("rec", fk)`), or of one collection of it, in key order.
    pub fn scan_forum(&self, fk: &str, collection: Option<&str>) -> RecordScan<'k, K> {
        let prefix = match collection {
            Some(c) => schema::prefix(tag::REC, &[fk, c]),
            None => schema::prefix(tag::REC, &[fk]),
        };
        RecordScan::new(self.kv, &prefix, Dir::Forward)
    }

    /// The stored rows of one repo's `(fk, collection)` sub-range in rkey order (`Reverse`:
    /// descending, as the fetch caps count). Includes the membership row for
    /// `collection = NS.membership`.
    pub fn scan_repo_range(
        &self,
        did: &str,
        collection: &str,
        fk: &str,
        dir: Dir,
    ) -> RecordScan<'k, K> {
        let prefix = schema::prefix(tag::REC, &[fk, collection, did]);
        RecordScan::new(self.kv, &prefix, dir)
    }

    /// Global-scope rows (`"_"`) of one repo's collection (declarations, profiles, keys outside
    /// any fk).
    pub fn scan_global(&self, did: &str, collection: &str) -> RecordScan<'k, K> {
        let prefix = schema::prefix(tag::REC, &[schema::GLOBAL_SCOPE, collection, did]);
        RecordScan::new(self.kv, &prefix, Dir::Forward)
    }

    /// The repo state of `did`.
    pub async fn repo(&self, did: &str) -> Result<Option<RepoRow>, StoreError> {
        self.get_one(schema::repo_key(did), |b| {
            decode_versioned(b, |r: &RepoRow| r.v)
        })
        .await
    }

    /// The repo states of several DIDs (one read).
    pub async fn repos(&self, dids: &[&str]) -> Result<Vec<Option<RepoRow>>, StoreError> {
        let raw = self
            .get_raw(dids.iter().map(|d| schema::repo_key(d)).collect())
            .await?;
        Ok(raw
            .into_iter()
            .map(|b| b.and_then(|b| decode_versioned(&b, |r: &RepoRow| r.v)))
            .collect())
    }

    /// The capabilities of a host origin.
    pub async fn host_caps(&self, origin: &str) -> Result<Option<HostRow>, StoreError> {
        self.get_one(schema::host_key(origin), |b| {
            decode_versioned(b, |r: &HostRow| r.v)
        })
        .await
    }

    /// Blob metadata.
    pub async fn blob(&self, cid: &str) -> Result<Option<BlobRow>, StoreError> {
        self.get_one(schema::blob_key(cid), |b| {
            decode_versioned(b, |r: &BlobRow| r.v)
        })
        .await
    }

    /// A live-source cursor.
    pub async fn cursor(&self, source: &str) -> Result<Option<CursorRow>, StoreError> {
        self.get_one(schema::cur_key(source), |b| {
            decode_versioned(b, |r: &CursorRow| r.v)
        })
        .await
    }

    /// A forum's access row.
    pub async fn forum(&self, fk: &str) -> Result<Option<ForumRow>, StoreError> {
        self.get_one(schema::forum_key(fk), |b| {
            decode_versioned(b, |r: &ForumRow| r.v)
        })
        .await
    }

    /// The fold's `absent` set for forum `fk` (§5.6.4), given the `subject.uri` of every opinion
    /// in `records`. Non-canonical uris are never absent.
    pub async fn absent<'a, I>(&self, fk: &str, subjects: I) -> Result<BTreeSet<String>, StoreError>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let uris: BTreeMap<String, AtUri> = subjects
            .into_iter()
            .filter_map(|s| AtUri::parse(s).ok().map(|u| (s.to_owned(), u)))
            .collect();
        let keys: Vec<RecKey> = uris.values().map(RecKey::of_uri).collect();
        let raw = self
            .get_raw(keys.iter().flat_map(|k| [k.rec(), k.tomb()]).collect())
            .await?;
        let mut states = Vec::with_capacity(keys.len());
        let mut dids = BTreeSet::new();
        for (i, u) in uris.values().enumerate() {
            let at = |j: usize| raw.get(j).and_then(|v| v.as_deref());
            let has_rec = at(2 * i).and_then(decode_rec).is_some();
            let has_tomb = at(2 * i + 1).and_then(decode_tomb).is_some();
            if !has_rec && !has_tomb {
                dids.insert(u.did().as_str());
            }
            states.push((has_rec, has_tomb));
        }
        let dids: Vec<&str> = dids.into_iter().collect();
        let repos: BTreeMap<&str, RepoRow> = dids
            .iter()
            .copied()
            .zip(self.repos(&dids).await?)
            .filter_map(|(d, r)| Some((d, r?)))
            .collect();
        Ok(uris
            .iter()
            .zip(states)
            .filter(|((_, u), (has_rec, has_tomb))| {
                is_absent(fk, u, *has_rec, *has_tomb, repos.get(u.did().as_str()))
            })
            .map(|((s, _), _)| s.clone())
            .collect())
    }

    /// Deletes tombstones written more than `retention_ms` ago (never less than
    /// [`limits::TOMBSTONE_MIN_RETENTION_MS`]), and undecodable ones. Returns the count.
    pub async fn gc_tombstones(&self, now_ms: i64, retention_ms: i64) -> Result<usize, StoreError> {
        let retention = retention_ms.max(limits::TOMBSTONE_MIN_RETENTION_MS);
        let mut range = Some(KeyRange::prefix(&schema::prefix(tag::TOMB, &[])));
        let mut doomed = Vec::new();
        while range.is_some() {
            for (k, v) in scan_page(self.kv, &mut range, Dir::Forward, 1_000).await? {
                let expired =
                    decode_tomb(&v).is_none_or(|t| now_ms.saturating_sub(t.at) > retention);
                if expired {
                    doomed.push(KvOp::Delete(k));
                }
            }
        }
        let n = doomed.len();
        while !doomed.is_empty() {
            let rest = doomed.split_off(doomed.len().min(1_000));
            self.kv.write(doomed).await?;
            doomed = rest;
        }
        Ok(n)
    }
}

/// `absent` for one subject (§5.6.4, refined): no row, and either a tombstone, or the subject
/// lies in a `RANGED` collection's `{fk}-` range of a repo whose scope `fk` was completely listed,
/// above the cap boundary (or the repo has no such collection, or the range was not capped).
pub fn is_absent(
    fk: &str,
    uri: &AtUri,
    has_rec: bool,
    has_tomb: bool,
    repo: Option<&RepoRow>,
) -> bool {
    if has_rec {
        return false;
    }
    if has_tomb {
        return true;
    }
    let (coll, rkey) = (uri.collection().as_str(), uri.rkey().as_str());
    if !schema::is_ranged(coll) || !schema::in_range(fk, rkey) {
        return false;
    }
    let Some(r) = repo else {
        return false;
    };
    if r.listed_rev(fk).is_empty() {
        return false;
    }
    if !r.colls.iter().any(|c| c == coll) {
        return true;
    }
    match r.cap_boundary(fk, coll) {
        None => true,
        Some(b) => rkey > b,
    }
}

/// Fold-input eligibility of a stored version (§1.5, I7, I8): not fetched under an identity hint,
/// and its repo not inactive (no repo row: active). Stored `proven` versions are current by
/// construction, since stale third-party proofs are never stored (I5).
pub fn eligible(row: &RecordRow, repo: Option<&RepoRow>) -> bool {
    !row.id_hint && repo.is_none_or(|r| r.status != AccountStatus::Inactive)
}

/// The stored rows of forum `fk` that may enter the fold's `records` (§13, `data-model.md`
/// §7.4): membership rows with rkey `fk`, and the `RANGED` rows of the `{fk}-` range, keeping for
/// each `(did, collection)` the first `cap(collection)` keys in descending rkey order. Every
/// stored key counts toward the cap (invalid and ineligible ones too), so the boundary depends
/// only on the key set; apply [`eligible`] afterwards. The result is in key order.
pub fn fold_rows(
    fk: &str,
    rows: Vec<StoredRecord>,
    cap: impl Fn(&str) -> usize,
) -> Vec<StoredRecord> {
    let mut groups: BTreeMap<(String, String), Vec<StoredRecord>> = BTreeMap::new();
    let mut out = Vec::new();
    for r in rows {
        let k = &r.key;
        if k.scope != fk {
            continue;
        }
        if schema::is_membership(&k.collection) {
            if k.rkey == fk {
                out.push(r);
            }
        } else if schema::is_ranged(&k.collection) && schema::in_range(fk, &k.rkey) {
            groups
                .entry((k.did.clone(), k.collection.clone()))
                .or_default()
                .push(r);
        }
    }
    for ((_, coll), mut g) in groups {
        g.sort_by(|a, b| b.key.rkey.cmp(&a.key.rkey));
        g.truncate(cap(&coll));
        out.extend(g);
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// [`fold_rows`] with the normative caps ([`schema::fetch_cap`]).
pub fn fold_rows_capped(fk: &str, rows: Vec<StoredRecord>) -> Vec<StoredRecord> {
    fold_rows(fk, rows, |c| schema::fetch_cap(c) as usize)
}

/// The fold's `obs` entries of one stored version (§5.6.5): `(uri, cid) → firstSeen` and
/// `(uri, firstCid) → firstSeenUri`.
pub fn obs_entries(uri: &str, row: &RecordRow) -> [((String, String), i64); 2] {
    [
        ((uri.to_owned(), row.cid.to_string()), row.first_seen),
        (
            (uri.to_owned(), row.first_cid.to_string()),
            row.first_seen_uri,
        ),
    ]
}

/// The fold's `obs` map over stored versions (the minimum on equal keys).
pub fn obs_map<'a, I>(rows: I) -> BTreeMap<(String, String), i64>
where
    I: IntoIterator<Item = &'a StoredRecord>,
{
    let mut out: BTreeMap<(String, String), i64> = BTreeMap::new();
    for r in rows {
        for (k, t) in obs_entries(&r.uri(), &r.row) {
            out.entry(k).and_modify(|e| *e = (*e).min(t)).or_insert(t);
        }
    }
    out
}

/// Result of [`Batch::put_version`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmitResult {
    /// What happened.
    pub admission: Admission,
    /// The stored state changed.
    pub changed: bool,
}

/// Result of [`Batch::tombstone`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TombResult {
    /// What happened.
    pub outcome: TombOutcome,
    /// The stored state changed.
    pub changed: bool,
}

/// Result of [`Batch::reconcile_range`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeDiff {
    /// Stored rows not listed, above the cap boundary: tombstoned (rkey, outcome).
    pub tombstoned: Vec<(String, TombOutcome)>,
    /// Stored rows not listed, at or below the cap boundary: removed without a tombstone.
    pub dropped: Vec<String>,
}

/// Result of [`Batch::commit`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    /// Uris whose stored state changed.
    pub changed: BTreeSet<String>,
    /// Key-value operations written.
    pub ops: usize,
}

/// An atomic write batch with read-your-writes: every read through the batch sees the batch's
/// own pending writes, and [`Batch::commit`] applies them with one [`KvStore::write`].
///
/// The store has no interactive transactions (IndexedDB), so a batch reads, decides and writes;
/// concurrent writers of the same repo are excluded by the repo lock (I9), and every decision is
/// idempotent and key-monotone, so a replayed batch converges.
pub struct Batch<'k, K: KvStore> {
    kv: &'k K,
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    read: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    changed: BTreeSet<String>,
}

impl<'k, K: KvStore> Batch<'k, K> {
    /// An empty batch over `kv`.
    pub fn new(kv: &'k K) -> Self {
        Self {
            kv,
            pending: BTreeMap::new(),
            read: BTreeMap::new(),
            changed: BTreeSet::new(),
        }
    }

    /// Uris whose state this batch changes.
    pub fn changed(&self) -> &BTreeSet<String> {
        &self.changed
    }

    /// `true` if nothing is pending.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Loads keys that are neither pending nor already read, in one read.
    async fn load(&mut self, keys: impl IntoIterator<Item = Vec<u8>>) -> Result<(), StoreError> {
        let missing: BTreeSet<Vec<u8>> = keys
            .into_iter()
            .filter(|k| !self.pending.contains_key(k) && !self.read.contains_key(k))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let missing: Vec<Vec<u8>> = missing.into_iter().collect();
        for chunk in missing.chunks(READ_CHUNK) {
            let values = self.kv.get_many(chunk.to_vec()).await?;
            if values.len() != chunk.len() {
                return Err(StoreError::Host(HostError::Storage(
                    "get_many returned the wrong number of values".into(),
                )));
            }
            for (k, v) in chunk.iter().zip(values) {
                self.read.insert(k.clone(), v);
            }
        }
        Ok(())
    }

    /// The current value of a loaded key (pending writes first).
    fn current(&self, key: &[u8]) -> Option<&[u8]> {
        match self.pending.get(key) {
            Some(v) => v.as_deref(),
            None => self.read.get(key).and_then(|v| v.as_deref()),
        }
    }

    fn put_raw(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.pending.insert(key, Some(value));
    }

    fn delete_raw(&mut self, key: Vec<u8>) {
        let known_absent =
            !self.pending.contains_key(&key) && matches!(self.read.get(&key), Some(None));
        if !known_absent {
            self.pending.insert(key, None);
        }
    }

    async fn get_decoded<T>(
        &mut self,
        key: Vec<u8>,
        decode: fn(&[u8]) -> Option<T>,
    ) -> Result<Option<T>, StoreError> {
        self.load([key.clone()]).await?;
        Ok(self.current(&key).and_then(decode))
    }

    fn put_row<T: Serialize>(&mut self, key: Vec<u8>, value: &T) -> Result<(), StoreError> {
        let bytes = row::encode(value)?;
        self.put_raw(key, bytes);
        Ok(())
    }

    /// The state of a uri as this batch sees it.
    pub async fn slot(&mut self, k: &RecKey) -> Result<Slot, StoreError> {
        let (rk, tk) = (k.rec(), k.tomb());
        self.load([rk.clone(), tk.clone()]).await?;
        let rec = self.current(&rk).and_then(decode_rec);
        let tomb = self.current(&tk).and_then(decode_tomb);
        Ok(slot_from(rec, tomb))
    }

    fn write_slot(&mut self, k: &RecKey, slot: &Slot) -> Result<(), StoreError> {
        match slot {
            Slot::Rec(r) => {
                self.put_row(k.rec(), r)?;
                self.delete_raw(k.tomb());
            }
            Slot::Tomb(t) => {
                self.put_row(k.tomb(), t)?;
                self.delete_raw(k.rec());
            }
            Slot::Empty => {
                self.delete_raw(k.rec());
                self.delete_raw(k.tomb());
            }
        }
        self.changed.insert(k.uri());
        Ok(())
    }

    /// Admits a version (§5.6.2). Fails with [`StoreError::CidMismatch`] unless its bytes hash to
    /// its dag-cbor CID (I2).
    pub async fn put_version(&mut self, v: Version) -> Result<AdmitResult, StoreError> {
        check_version_bytes(&v)?;
        let k = RecKey::of_uri(&v.uri);
        let slot = self.slot(&k).await?;
        let (new, admission, changed) = admit_version(slot, v);
        if changed {
            self.write_slot(&k, &new)?;
        }
        Ok(AdmitResult { admission, changed })
    }

    /// Admits several versions in order, reading their current states in one request.
    pub async fn put_versions(
        &mut self,
        versions: Vec<Version>,
    ) -> Result<Vec<AdmitResult>, StoreError> {
        let keys: Vec<Vec<u8>> = versions
            .iter()
            .flat_map(|v| {
                let k = RecKey::of_uri(&v.uri);
                [k.rec(), k.tomb()]
            })
            .collect();
        self.load(keys).await?;
        let mut out = Vec::with_capacity(versions.len());
        for v in versions {
            out.push(self.put_version(v).await?);
        }
        Ok(out)
    }

    /// Records a confirmed delete of `uri` (§5.6.3).
    pub async fn tombstone(&mut self, uri: &AtUri, d: Delete) -> Result<TombResult, StoreError> {
        self.tombstone_key(&RecKey::of_uri(uri), d).await
    }

    /// [`Batch::tombstone`] by key.
    pub async fn tombstone_key(&mut self, k: &RecKey, d: Delete) -> Result<TombResult, StoreError> {
        let slot = self.slot(k).await?;
        let (new, outcome, changed) = apply_delete(slot, &d);
        if changed {
            self.write_slot(k, &new)?;
        }
        Ok(TombResult { outcome, changed })
    }

    /// Removes a stored version without a tombstone (rows beyond a fetch-cap boundary, §5.2; or
    /// `sync --full`). Returns `true` if a row was removed.
    pub async fn drop_record(&mut self, k: &RecKey) -> Result<bool, StoreError> {
        match self.slot(k).await? {
            Slot::Rec(_) => {
                self.write_slot(k, &Slot::Empty)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Applies a first-seen time learned after admission (an indexer's `#recordRef.firstSeen` in
    /// assisted mode, §5.6.5) to the stored row or tombstone of `uri`. Returns `true` if changed.
    pub async fn note_first_seen(
        &mut self,
        uri: &AtUri,
        cid: &Cid,
        t_us: i64,
    ) -> Result<bool, StoreError> {
        let k = RecKey::of_uri(uri);
        let mut slot = self.slot(&k).await?;
        let changed = match &mut slot {
            Slot::Rec(r) => r.lower_first_seen(*cid, t_us),
            Slot::Tomb(t) => t.lower_first_seen(*cid, t_us),
            Slot::Empty => false,
        };
        if changed {
            self.write_slot(&k, &slot)?;
        }
        Ok(changed)
    }

    /// Every key-value pair under `prefix` as this batch sees it.
    async fn scan_merged(
        &mut self,
        prefix: &[u8],
    ) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, StoreError> {
        let mut out = BTreeMap::new();
        let mut range = Some(KeyRange::prefix(prefix));
        while range.is_some() {
            for (k, v) in scan_page(self.kv, &mut range, Dir::Forward, 1_000).await? {
                self.read
                    .entry(k.clone())
                    .or_insert_with(|| Some(v.clone()));
                out.insert(k, v);
            }
        }
        let r = KeyRange::prefix(prefix);
        for (k, v) in self.pending.range((r.start, r.end)) {
            match v {
                Some(v) => out.insert(k.clone(), v.clone()),
                None => out.remove(k),
            };
        }
        Ok(out)
    }

    /// The tombstone step of a complete, stable listing of `did`'s `{fk}-` range of `coll`
    /// (§5.2): every stored row of the range whose rkey is not in `listed` is tombstoned with
    /// `key` (the pass's `h1.rev` and the send time of its first page), or, at or below the cap
    /// `boundary` (the smallest listed rkey of a capped range), removed without a tombstone.
    ///
    /// `listed` holds every rkey the listing returned, including records that failed admission:
    /// those exist, so they must not be tombstoned. Call it only for complete, stable passes.
    #[allow(clippy::too_many_arguments)]
    pub async fn reconcile_range(
        &mut self,
        did: &str,
        coll: &str,
        fk: &str,
        listed: &BTreeSet<String>,
        key: &OrderKey,
        boundary: Option<&str>,
        at_ms: i64,
    ) -> Result<RangeDiff, StoreError> {
        let rows = self
            .scan_merged(&schema::prefix(tag::REC, &[fk, coll, did]))
            .await?;
        let mut diff = RangeDiff::default();
        for kb in rows.keys() {
            let Some(rk) = RecKey::parse(kb, tag::REC) else {
                continue;
            };
            if !schema::in_range(fk, &rk.rkey) || listed.contains(&rk.rkey) {
                continue;
            }
            match boundary {
                Some(b) if rk.rkey.as_str() <= b => {
                    if self.drop_record(&rk).await? {
                        diff.dropped.push(rk.rkey);
                    }
                }
                _ => {
                    let d = Delete {
                        key: key.clone(),
                        evidence: DeleteEvidence::CompleteListing,
                        at_ms,
                    };
                    let r = self.tombstone_key(&rk, d).await?;
                    diff.tombstoned.push((rk.rkey, r.outcome));
                }
            }
        }
        Ok(diff)
    }

    /// Forgets every stored row and tombstone of `did` under `(scope, coll)`: `sync --full`
    /// after a rollback discards the repo's cached state before listing it again, which accepts
    /// the PDS state (`sync.md` §5.9). Returns the uris removed.
    pub async fn forget_range(
        &mut self,
        did: &str,
        coll: &str,
        scope: &str,
    ) -> Result<Vec<String>, StoreError> {
        let mut keys: BTreeSet<RecKey> = BTreeSet::new();
        for t in [tag::REC, tag::TOMB] {
            let rows = self
                .scan_merged(&schema::prefix(t, &[scope, coll, did]))
                .await?;
            keys.extend(rows.keys().filter_map(|kb| RecKey::parse(kb, t)));
        }
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if self.forget(&k).await? {
                out.push(k.uri());
            }
        }
        Ok(out)
    }

    /// Forgets the stored row or tombstone of one uri (see [`Batch::forget_range`]). Returns
    /// `true` if something was removed.
    pub async fn forget(&mut self, k: &RecKey) -> Result<bool, StoreError> {
        if matches!(self.slot(k).await?, Slot::Empty) {
            return Ok(false);
        }
        self.write_slot(k, &Slot::Empty)?;
        Ok(true)
    }

    /// The repo state as this batch sees it.
    pub async fn repo(&mut self, did: &str) -> Result<Option<RepoRow>, StoreError> {
        self.get_decoded(schema::repo_key(did), |b| {
            decode_versioned(b, |r: &RepoRow| r.v)
        })
        .await
    }

    /// Writes the repo state.
    pub fn put_repo(&mut self, did: &str, r: &RepoRow) -> Result<(), StoreError> {
        self.put_row(schema::repo_key(did), r)
    }

    /// Host capabilities as this batch sees them.
    pub async fn host_caps(&mut self, origin: &str) -> Result<Option<HostRow>, StoreError> {
        self.get_decoded(schema::host_key(origin), |b| {
            decode_versioned(b, |r: &HostRow| r.v)
        })
        .await
    }

    /// Writes host capabilities.
    pub fn put_host_caps(&mut self, origin: &str, h: &HostRow) -> Result<(), StoreError> {
        self.put_row(schema::host_key(origin), h)
    }

    /// Writes blob metadata.
    pub fn put_blob(&mut self, cid: &str, b: &BlobRow) -> Result<(), StoreError> {
        self.put_row(schema::blob_key(cid), b)
    }

    /// Adds `delta` to a blob's reference count (never below 0). Returns the new count, or
    /// `None` if there is no row for the CID.
    pub async fn adjust_blob_refs(
        &mut self,
        cid: &str,
        delta: i64,
    ) -> Result<Option<i64>, StoreError> {
        let key = schema::blob_key(cid);
        let Some(mut b) = self
            .get_decoded(key.clone(), |b| decode_versioned(b, |r: &BlobRow| r.v))
            .await?
        else {
            return Ok(None);
        };
        b.refs = b.refs.saturating_add(delta).max(0);
        self.put_row(key, &b)?;
        Ok(Some(b.refs))
    }

    /// Writes a live-source cursor.
    pub fn put_cursor(&mut self, source: &str, c: &CursorRow) -> Result<(), StoreError> {
        self.put_row(schema::cur_key(source), c)
    }

    /// Writes a forum access row.
    pub fn put_forum(&mut self, fk: &str, f: &ForumRow) -> Result<(), StoreError> {
        self.put_row(schema::forum_key(fk), f)
    }

    /// Writes everything with one atomic [`KvStore::write`].
    pub async fn commit(self) -> Result<Committed, StoreError> {
        let ops: Vec<KvOp> = self
            .pending
            .into_iter()
            .map(|(k, v)| match v {
                Some(v) => KvOp::Put(k, v),
                None => KvOp::Delete(k),
            })
            .collect();
        let n = ops.len();
        if n > 0 {
            self.kv.write(ops).await?;
        }
        Ok(Committed {
            changed: self.changed,
            ops: n,
        })
    }
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
