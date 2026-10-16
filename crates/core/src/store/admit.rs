// SPDX-License-Identifier: AGPL-3.0-or-later
//! Admission checks (`sync.md` §5.5): what a record must pass before it may be stored.
//!
//! ```text
//! response bytes ──strict scan──▶ malformed / deeper than NESTING_MAX_DOCUMENT → reject the whole
//!   (super::strict_json)           response (R1): nothing admitted, tombstoned or replaced
//!                    issue in a record (duplicate key, fraction/exponent, |n| > 2^53−1) → drop it
//! 1. shape   canonical at-uri of the repo's DID and the requested collection (and rkey); the value
//!            is a JSON object of ≤ 1,000,000 bytes in its response encoding
//! 2. JSON → IPLD → DAG-CBOR under RECORD_LIMITS (depth ≤ 36; deeper than 32 is converted and
//!            then fails G10)
//! 3. CID     the claimed CID is CIDv1 / dag-cbor / sha2-256 and equals the computed one (binary)
//! 4. failure of 1–3: dropped, never stored; the caller logs it and strikes the host
//! 5. validity (data-model.md §7): G6 and G10 here, the rest by a [`RecordValidator`]; an invalid
//!            record is still admitted, flagged with its first failing rule
//! ```
//!
//! The output, an [`AdmittedRecord`], becomes a store [`Version`] with its observation context
//! ([`Observation`]: level, order key, exactness, source, first-seen time).

use lessdugin_atp::datamodel::{self, Limits};
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::{Cid, Ipld};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host::MaybeSendSync;
use crate::limits;
use crate::store::records::{Level, OrderKey, Source, Version};
use crate::store::strict_json::{self, Issue, Scan, Seg};

/// The caps under which admitted record values are converted and encoded, and stored record
/// bytes decoded again (`Limits::DEFAULT` with the document depth, `NESTING_MAX_DOCUMENT`): a
/// value between 33 and 36 deep is still converted and CID-checked, then fails G10.
pub const RECORD_LIMITS: Limits = Limits {
    max_depth: limits::NESTING_MAX_DOCUMENT,
    ..Limits::DEFAULT
};

/// At most this many violations are kept on a row (raw view).
pub const MAX_STORED_VIOLATIONS: usize = 16;
/// Each kept violation is cut to this many bytes.
pub const MAX_VIOLATION_BYTES: usize = 300;

/// One failed validity rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleViolation {
    /// The rule ID (`G2`, `PO3`, …).
    pub rule: String,
    /// What is wrong.
    pub message: String,
}

impl RuleViolation {
    /// A violation.
    pub fn new(rule: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            rule: rule.into(),
            message: message.into(),
        }
    }
}

/// A validity verdict (`data-model.md` §7): valid, or the first failing rule plus details.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validity {
    /// The first failing rule ID; `None` if valid.
    pub rule: Option<String>,
    /// `"<rule>: <message>"` for each violation (bounded).
    pub violations: Vec<String>,
}

impl Validity {
    /// The valid verdict.
    pub fn valid() -> Self {
        Self::default()
    }

    /// `true` if no rule failed.
    pub fn is_valid(&self) -> bool {
        self.rule.is_none()
    }

    /// The verdict for a set of violations: the first rule in rule order (`G1`…`G10`, then the
    /// per-collection rules by name and number), and up to [`MAX_STORED_VIOLATIONS`] details.
    pub fn from_violations(mut v: Vec<RuleViolation>) -> Self {
        v.sort_by_key(|a| rule_order(&a.rule));
        let rule = v.first().map(|x| x.rule.clone());
        let violations = v
            .iter()
            .take(MAX_STORED_VIOLATIONS)
            .map(|x| truncate(format!("{}: {}", x.rule, x.message), MAX_VIOLATION_BYTES))
            .collect();
        Self { rule, violations }
    }
}

/// Sort key of a rule ID: the known IDs in spec order (`G1`…`G10`, then `FO1`…`PF2`, as
/// `lessdugin_lexicon::validity::RuleId` orders them), then any other ID by prefix and number.
type RuleKey = (
    bool,
    Option<lessdugin_lexicon::validity::RuleId>,
    String,
    u32,
);

fn rule_order(rule: &str) -> RuleKey {
    let known = lessdugin_lexicon::validity::RuleId::parse(rule);
    let split = rule
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(rule.len());
    let (prefix, num) = rule.split_at(split);
    let n = num.parse::<u32>().unwrap_or(u32::MAX);
    (known.is_none(), known, prefix.to_owned(), n)
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// The record-validity rules beyond those admission checks itself (G6 size and G10 depth).
pub trait RecordValidator: MaybeSendSync {
    /// The violations of `value` stored at `uri` (empty if valid).
    fn violations(&self, uri: &AtUri, value: &Value) -> Vec<RuleViolation>;
}

/// Record validity through `lessdugin_lexicon::validity` (`data-model.md` §7.2–§7.3): every rule
/// that is a pure function of the uri and the value (G1–G10 and the per-collection rules; the
/// *(x)* and entry-level rules belong to the fold). Collections outside this build's ten (the
/// `app.bsky.actor.profile` fallback) are only checked for `$type` (G1): their lexicons are not
/// embedded.
#[derive(Clone, Copy, Debug, Default)]
pub struct LexiconValidator;

impl RecordValidator for LexiconValidator {
    fn violations(&self, uri: &AtUri, value: &Value) -> Vec<RuleViolation> {
        let nsid = uri.collection().as_str();
        if lessdugin_lexicon::Collection::from_nsid(nsid).is_none() {
            return match value.get("$type") {
                Some(Value::String(t)) if t == nsid => Vec::new(),
                _ => vec![RuleViolation::new("G1", format!("$type is not {nsid:?}"))],
            };
        }
        lessdugin_lexicon::validity::check_uri(uri, value)
            .violations
            .into_iter()
            .map(|v| RuleViolation::new(v.rule_id.as_str(), format!("{}: {}", v.path, v.message)))
            .collect()
    }
}

/// A validator that accepts everything (tests, and callers that validate elsewhere).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoValidator;

impl RecordValidator for NoValidator {
    fn violations(&self, _: &AtUri, _: &Value) -> Vec<RuleViolation> {
        Vec::new()
    }
}

/// Why a record (or a whole response) is not admitted. Every variant is a strike against the
/// host (§5.5 step 4).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmitError {
    /// The response is not acceptable JSON or is deeper than the document limit: it is rejected
    /// whole (R1), nothing in it is admitted, tombstoned or replaced.
    #[error("malformed response: {0}")]
    Malformed(String),
    /// A strict-scan issue inside this record (duplicate key, float, unsafe integer).
    #[error("strict JSON: {0}")]
    Json(String),
    /// The uri, collection, value type or value size is wrong.
    #[error("bad record shape: {0}")]
    Shape(String),
    /// The claimed CID is missing, unparseable or not CIDv1/dag-cbor/sha2-256.
    #[error("bad cid: {0}")]
    Cid(String),
    /// The value does not hash to the claimed CID.
    #[error("cid mismatch: claimed {claimed}, computed {computed}")]
    CidMismatch {
        /// The claimed CID.
        claimed: String,
        /// The CID of the value's DAG-CBOR encoding.
        computed: String,
    },
    /// The value is not in the atproto data model (or exceeds the conversion caps).
    #[error("data model: {0}")]
    DataModel(String),
}

impl AdmitError {
    /// `true` if the whole response was rejected (R1).
    pub fn rejects_response(&self) -> bool {
        matches!(self, AdmitError::Malformed(_))
    }
}

/// What the request asked for.
#[derive(Clone, Copy, Debug)]
pub struct Expect<'a> {
    /// The repo DID.
    pub did: &'a Did,
    /// The requested collection.
    pub collection: &'a str,
    /// The requested record key (`getRecord`), if any.
    pub rkey: Option<&'a str>,
}

/// A record that passed admission.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedRecord {
    /// The canonical uri.
    pub uri: AtUri,
    /// The verified CID.
    pub cid: Cid,
    /// The DAG-CBOR encoding (hashes to `cid`).
    pub cbor: Vec<u8>,
    /// The JSON value.
    pub value: Value,
    /// The validity verdict.
    pub validity: Validity,
}

/// The observation context of an admitted record (`sync.md` §5.6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Trust level.
    pub level: Level,
    /// For `proven`: the commit rev.
    pub proven_rev: Option<String>,
    /// The author's identity was only a hint (§1.5).
    pub id_hint: bool,
    /// The order key.
    pub key: OrderKey,
    /// The key's rev is exact.
    pub exact: bool,
    /// Source.
    pub src: Source,
    /// First-seen time of the version (µs).
    pub first_seen_us: i64,
    /// Fetch time (ms).
    pub fetched_at_ms: i64,
}

impl Observation {
    fn base(src: Source, level: Level, key: OrderKey, exact: bool, now_ms: i64) -> Self {
        Self {
            level,
            proven_rev: None,
            id_hint: false,
            key,
            exact,
            src,
            first_seen_us: now_ms.saturating_mul(1000),
            fetched_at_ms: now_ms,
        }
    }

    /// A row of a listing pass: key `(h1.rev, page sentAt)`, exact iff the pass was stable.
    pub fn listing(h1_rev: &str, page_sent_at: i64, stable: bool, now_ms: i64) -> Self {
        Self::base(
            Source::List,
            Level::Pds,
            OrderKey::new(h1_rev, page_sent_at),
            stable,
            now_ms,
        )
    }

    /// A point fetch: key `(repo.headSeen.rev at send time, sentAt)`, never exact.
    pub fn point_fetch(head_rev: &str, sent_at: i64, now_ms: i64) -> Self {
        Self::base(
            Source::Get,
            Level::Pds,
            OrderKey::new(head_rev, sent_at),
            false,
            now_ms,
        )
    }

    /// An own write confirmed by the PDS: key `(commit.rev, sentAt of the write)`, exact.
    pub fn own_write(commit_rev: &str, sent_at: i64, now_ms: i64) -> Self {
        Self::base(
            Source::Own,
            Level::Pds,
            OrderKey::new(commit_rev, sent_at),
            true,
            now_ms,
        )
    }

    /// A proof (`proven@rev`): from the author's PDS (`Source::Proof`, key sentAt = send time of
    /// `sync.getRecord`) or a current third-party proof (`Source::Indexer`, key sentAt = the head
    /// observation that made it current). Exact.
    pub fn proof(commit_rev: &str, sent_at: i64, src: Source, now_ms: i64) -> Self {
        let mut o = Self::base(
            src,
            Level::Proven,
            OrderKey::new(commit_rev, sent_at),
            true,
            now_ms,
        );
        o.proven_rev = Some(commit_rev.to_owned());
        o
    }

    /// Marks the author's identity as a hint.
    pub fn with_id_hint(mut self, id_hint: bool) -> Self {
        self.id_hint = id_hint;
        self
    }

    /// Lowers the first-seen time to an earlier hint observation (µs).
    pub fn with_first_seen(mut self, us: i64) -> Self {
        self.first_seen_us = self.first_seen_us.min(us);
        self
    }
}

impl AdmittedRecord {
    /// The store version of this record observed in `obs`.
    pub fn into_version(self, obs: Observation) -> Version {
        Version {
            uri: self.uri,
            cid: self.cid,
            cbor: self.cbor,
            validity: self.validity,
            level: obs.level,
            proven_rev: obs.proven_rev,
            id_hint: obs.id_hint,
            key: obs.key,
            exact: obs.exact,
            src: obs.src,
            first_seen_us: obs.first_seen_us,
            fetched_at_ms: obs.fetched_at_ms,
        }
    }
}

/// Parses a claimed record CID: any multibase string of a CIDv1 with codec dag-cbor and a 32-byte
/// sha2-256 digest.
pub fn parse_record_cid(s: &str) -> Result<Cid, AdmitError> {
    let c = Cid::try_from(s).map_err(|e| AdmitError::Cid(format!("{s:?}: {e}")))?;
    let ok = u64::from(c.version()) == 1
        && c.codec() == lessdugin_atp::cid::DAG_CBOR
        && c.hash().code() == lessdugin_atp::cid::SHA2_256
        && c.hash().size() == 32;
    if ok {
        Ok(c)
    } else {
        Err(AdmitError::Cid(format!(
            "{s:?} is not a CIDv1 dag-cbor sha2-256"
        )))
    }
}

/// G6 and G10, then the validator's rules.
fn validity_of(
    uri: &AtUri,
    value: &Value,
    ipld: &Ipld,
    cbor_len: usize,
    validator: &dyn RecordValidator,
) -> Validity {
    let mut v = validator.violations(uri, value);
    // G6 and G10 are decided here on the actual encoding, whatever the validator covers
    if cbor_len > limits::RECORD_CBOR_MAX_BYTES && !v.iter().any(|x| x.rule == "G6") {
        v.push(RuleViolation::new(
            "G6",
            format!(
                "DAG-CBOR encoding is {cbor_len} bytes (max {})",
                limits::RECORD_CBOR_MAX_BYTES
            ),
        ));
    }
    let depth = datamodel::nesting_depth(ipld);
    if depth > limits::NESTING_MAX_RECORD && !v.iter().any(|x| x.rule == "G10") {
        v.push(RuleViolation::new(
            "G10",
            format!("nesting depth {depth} (max {})", limits::NESTING_MAX_RECORD),
        ));
    }
    Validity::from_violations(v)
}

/// Admission of one record from its parts. `value_len` is the size of the value in its response
/// encoding (bytes).
pub fn admit_value(
    uri: Option<&str>,
    cid: Option<&str>,
    value: Option<&Value>,
    value_len: usize,
    expect: &Expect<'_>,
    validator: &dyn RecordValidator,
) -> Result<AdmittedRecord, AdmitError> {
    // 1. shape
    let uri_s = uri.ok_or_else(|| AdmitError::Shape("missing uri".into()))?;
    let uri = AtUri::parse(uri_s)
        .map_err(|e| AdmitError::Shape(format!("non-canonical uri {uri_s:?}: {e}")))?;
    if uri.did() != expect.did {
        return Err(AdmitError::Shape(format!(
            "uri {uri_s:?} is not in repo {}",
            expect.did
        )));
    }
    if uri.collection().as_str() != expect.collection {
        return Err(AdmitError::Shape(format!(
            "uri {uri_s:?} is not in collection {}",
            expect.collection
        )));
    }
    if let Some(k) = expect.rkey
        && uri.rkey().as_str() != k
    {
        return Err(AdmitError::Shape(format!(
            "uri {uri_s:?} does not have the requested rkey {k:?}"
        )));
    }
    let value = match value {
        Some(v @ Value::Object(_)) => v,
        Some(_) => return Err(AdmitError::Shape("value is not an object".into())),
        None => return Err(AdmitError::Shape("missing value".into())),
    };
    if value_len as u64 > limits::RECORD_JSON_MAX_BYTES {
        return Err(AdmitError::Shape(format!(
            "value is {value_len} bytes (max {})",
            limits::RECORD_JSON_MAX_BYTES
        )));
    }
    // 3a. the claimed CID
    let claimed = parse_record_cid(cid.ok_or_else(|| AdmitError::Cid("missing cid".into()))?)?;
    // 2. JSON → IPLD → DAG-CBOR
    let ipld = datamodel::json_to_ipld_with(value, &RECORD_LIMITS)
        .map_err(|e| AdmitError::DataModel(e.to_string()))?;
    let cbor = lessdugin_atp::dagcbor::encode_with(&ipld, &RECORD_LIMITS)
        .map_err(|e| AdmitError::DataModel(e.to_string()))?;
    // 3b. compare
    let computed = lessdugin_atp::cid::for_dag_cbor(&cbor);
    if computed != claimed {
        return Err(AdmitError::CidMismatch {
            claimed: claimed.to_string(),
            computed: computed.to_string(),
        });
    }
    // 5. validity
    let validity = validity_of(&uri, value, &ipld, cbor.len(), validator);
    Ok(AdmittedRecord {
        uri,
        cid: computed,
        cbor,
        value: value.clone(),
        validity,
    })
}

fn scan_response(body: &[u8]) -> Result<Scan, AdmitError> {
    strict_json::scan(body, limits::NESTING_MAX_DOCUMENT, 3)
        .map_err(|e| AdmitError::Malformed(e.to_string()))
}

fn issue_error(i: &Issue) -> AdmitError {
    AdmitError::Json(format!("{} at byte {}", i.kind, i.offset))
}

/// Admission of a `com.atproto.repo.getRecord` response body (`{uri, cid, value}`).
pub fn admit_get_record(
    body: &[u8],
    expect: &Expect<'_>,
    validator: &dyn RecordValidator,
) -> Result<AdmittedRecord, AdmitError> {
    let scan = scan_response(body)?;
    let root: Vec<Seg> = Vec::new();
    if let Some(i) = scan.issues_under(&root).next() {
        return Err(issue_error(i));
    }
    let doc: Value =
        serde_json::from_slice(body).map_err(|e| AdmitError::Malformed(e.to_string()))?;
    let Value::Object(m) = &doc else {
        return Err(AdmitError::Malformed(
            "getRecord output is not an object".into(),
        ));
    };
    let len = scan.span(&[Seg::key("value")]).map_or(0, |s| s.len());
    admit_value(
        m.get("uri").and_then(Value::as_str),
        m.get("cid").and_then(Value::as_str),
        m.get("value"),
        len,
        expect,
        validator,
    )
}

/// Admission of a confirmed own write (§5.8): the exact value sent, and the CID the PDS returned.
pub fn admit_own_write(
    uri: &AtUri,
    value: &Value,
    returned_cid: &str,
    validator: &dyn RecordValidator,
) -> Result<AdmittedRecord, AdmitError> {
    let len = serde_json::to_vec(value)
        .map_err(|e| AdmitError::Shape(e.to_string()))?
        .len();
    let expect = Expect {
        did: uri.did(),
        collection: uri.collection().as_str(),
        rkey: Some(uri.rkey().as_str()),
    };
    admit_value(
        Some(uri.as_str()),
        Some(returned_cid),
        Some(value),
        len,
        &expect,
        validator,
    )
}

/// Admission of a record block taken from a verified proof (strict DAG-CBOR that hashes to `cid`).
pub fn admit_block(
    uri: &AtUri,
    cid: &Cid,
    block: &[u8],
    validator: &dyn RecordValidator,
) -> Result<AdmittedRecord, AdmitError> {
    if cid.codec() != lessdugin_atp::cid::DAG_CBOR {
        return Err(AdmitError::Cid(format!("{cid} is not dag-cbor")));
    }
    lessdugin_atp::cid::verify(cid, block).map_err(|e| AdmitError::CidMismatch {
        claimed: cid.to_string(),
        computed: e.to_string(),
    })?;
    let ipld = datamodel::record_from_cbor_with(block, &RECORD_LIMITS)
        .map_err(|e| AdmitError::DataModel(e.to_string()))?;
    let value = datamodel::ipld_to_json(&ipld).map_err(|e| AdmitError::DataModel(e.to_string()))?;
    let validity = validity_of(uri, &value, &ipld, block.len(), validator);
    Ok(AdmittedRecord {
        uri: uri.clone(),
        cid: *cid,
        cbor: block.to_vec(),
        value,
        validity,
    })
}

/// One entry of a `listRecords` page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageEntry {
    /// The entry's `uri` string, if it had one.
    pub raw_uri: Option<String>,
    /// The parsed canonical uri (whatever its repo and collection), for the listing checks
    /// (`sync.md` §5.3: a non-canonical uri fails the page).
    pub uri: Option<AtUri>,
    /// The admission result.
    pub result: Result<AdmittedRecord, AdmitError>,
}

impl PageEntry {
    /// The entry's rkey, if its uri is canonical.
    pub fn rkey(&self) -> Option<&str> {
        self.uri.as_ref().map(|u| u.rkey().as_str())
    }
}

/// An admitted `listRecords` page.
#[derive(Clone, Debug, PartialEq)]
pub struct ListPage {
    /// The next-page cursor.
    pub cursor: Option<String>,
    /// Every entry, in page order.
    pub entries: Vec<PageEntry>,
}

/// Admission of a `com.atproto.repo.listRecords` page. Fails only when the page as a whole is
/// unacceptable ([`AdmitError::Malformed`]); each entry carries its own result.
pub fn admit_list_page(
    body: &[u8],
    expect: &Expect<'_>,
    validator: &dyn RecordValidator,
) -> Result<ListPage, AdmitError> {
    let scan = scan_response(body)?;
    if let Some(i) = scan.issues.iter().find(|i| i.path.is_empty()) {
        return Err(AdmitError::Malformed(format!("page envelope: {}", i.kind)));
    }
    let doc: Value =
        serde_json::from_slice(body).map_err(|e| AdmitError::Malformed(e.to_string()))?;
    let Value::Object(m) = &doc else {
        return Err(AdmitError::Malformed(
            "listRecords output is not an object".into(),
        ));
    };
    let cursor = match m.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(AdmitError::Malformed("cursor is not a string".into())),
    };
    let Some(Value::Array(records)) = m.get("records") else {
        return Err(AdmitError::Malformed("records is not an array".into()));
    };
    let mut entries = Vec::with_capacity(records.len());
    for (i, rec) in records.iter().enumerate() {
        let at = [Seg::key("records"), Seg::Index(i)];
        let raw_uri = rec.get("uri").and_then(Value::as_str).map(str::to_owned);
        let uri = raw_uri.as_deref().and_then(|s| AtUri::parse(s).ok());
        let result = match scan.issues_under(&at).next() {
            Some(issue) => Err(issue_error(issue)),
            None => {
                let len = scan
                    .span(&[Seg::key("records"), Seg::Index(i), Seg::key("value")])
                    .map_or(0, |s| s.len());
                admit_value(
                    raw_uri.as_deref(),
                    rec.get("cid").and_then(Value::as_str),
                    rec.get("value"),
                    len,
                    expect,
                    validator,
                )
            }
        };
        entries.push(PageEntry {
            raw_uri,
            uri,
            result,
        });
    }
    Ok(ListPage { cursor, entries })
}

#[cfg(test)]
#[path = "admit_tests.rs"]
mod tests;
