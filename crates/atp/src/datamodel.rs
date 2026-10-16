// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto data model (`/specs/data-model`): JSON ⇄ IPLD conversion (`$link`, `$bytes`, blobs),
//! data-model validation, record CIDs and blob-reference extraction.
//!
//! JSON conventions: `{"$link": "<cid>"}` is a CID link (exactly one key, canonical base32 CIDv1);
//! `{"$bytes": "<base64>"}` is a byte string (exactly one key; RFC 4648 §4 base64, padding
//! optional on input, never written); blobs are ordinary maps with `$type: "blob"` whose `ref` is a
//! link. Floats are rejected, except JSON numbers with a zero fraction (such as `123.0`) within
//! ±(2^53 − 1), which are integers (as in JavaScript, and per the interop fixtures). JSON numbers
//! are parsed with correct rounding (serde_json's `float_roundtrip`, enabled in this crate's
//! manifest), so the integer is the one JavaScript's `JSON.parse` yields and record CIDs agree with
//! the reference implementation. Integers must fit in `i64`. A `$type` field must be a non-empty
//! string.
//!
//! **CIDs.** Only "blessed" CIDs are part of the data model (`/specs/data-model`): CIDv1, sha2-256
//! with a 32-byte digest, codec dag-cbor or raw. A blob's `ref` must use the raw codec. Conversion
//! and [`validate`] reject anything else; the DAG-CBOR codec itself ([`crate::dagcbor`]) accepts any
//! CIDv1, as strict DAG-CBOR does (`sync.md` §8.3).
//!
//! **Nesting depth** is counted on the data model, identically for JSON and DAG-CBOR: maps and
//! lists are containers; links and byte strings are leaves, including their JSON spellings
//! `{"$link": …}` and `{"$bytes": …}`. So a record has one depth whichever encoding it arrives in.
//!
//! All walkers use explicit depth counters checked before descending ([`Limits`]).

use std::collections::BTreeMap;

use ::cid::Cid;
use data_encoding::{BASE64, BASE64_NOPAD};
use serde_json::{Map, Number, Value};

use crate::Ipld;
use crate::dagcbor;

/// Caps applied by decoders and converters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum nesting of maps and lists (a top-level map is depth 1).
    pub max_depth: usize,
    /// Maximum number of elements in one map or list.
    pub max_container_len: usize,
    /// Maximum length of one string or map key, in UTF-8 bytes.
    pub max_string_bytes: usize,
    /// Maximum length of one byte string.
    pub max_bytes_len: usize,
    /// Maximum number of values (nodes) in the whole document.
    pub max_total_nodes: usize,
}

impl Limits {
    /// Default caps for decoding untrusted records into memory: depth 32 (`NESTING_MAX_RECORD`),
    /// 131,072 elements per container, 1 MiB strings and byte strings, 262,144 values.
    ///
    /// Only the depth is a protocol rule; the other caps are safety (S-class) caps that bound the
    /// memory a decoded tree may take. A value over them is *unreadable* here, not invalid. Data a
    /// program wrote itself (store rows, caches) should be read back with the limits it was
    /// written with, or with [`Limits::uncapped`]: [`crate::dagcbor::encode_with`] enforces the
    /// same caps, so whatever it wrote reads back under the same limits.
    pub const DEFAULT: Self = Self {
        max_depth: 32,
        max_container_len: 131_072,
        max_string_bytes: 1 << 20,
        max_bytes_len: 1 << 20,
        max_total_nodes: 262_144,
    };

    /// Strict decoding for `proofs/1` (`sync.md` §8.3): nesting depth ≤ 32 and nothing else; the
    /// input length bounds everything. Use it with [`crate::dagcbor::validate_with`], which
    /// allocates nothing. Decoding untrusted input into an [`Ipld`] tree with it can amplify memory
    /// (each value takes far more memory than its one encoded byte); prefer [`Limits::DEFAULT`].
    pub const STRICT: Self = Self::uncapped(32);

    /// No caps except `max_depth`: for validation passes, and for reading back trusted data.
    pub const fn uncapped(max_depth: usize) -> Self {
        Self {
            max_depth,
            max_container_len: usize::MAX,
            max_string_bytes: usize::MAX,
            max_bytes_len: usize::MAX,
            max_total_nodes: usize::MAX,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Data-model and DAG-CBOR errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataModelError {
    /// Nesting deeper than the limit.
    #[error("nesting exceeds depth {max}")]
    DepthExceeded {
        /// The depth limit.
        max: usize,
    },
    /// A map or list has too many elements.
    #[error("container has {len} elements (max {max})")]
    ContainerTooLarge {
        /// Declared length.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// Too many values in the document.
    #[error("document exceeds {max} values")]
    TooManyNodes {
        /// The limit.
        max: usize,
    },
    /// A string is too long.
    #[error("string of {len} bytes (max {max})")]
    StringTooLong {
        /// Declared length.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// A byte string is too long.
    #[error("byte string of {len} bytes (max {max})")]
    BytesTooLong {
        /// Declared length.
        len: usize,
        /// The limit.
        max: usize,
    },
    /// Floats are not allowed in atproto.
    #[error("floats are not allowed")]
    Float,
    /// An integer outside the signed 64-bit range.
    #[error("integer out of range")]
    IntegerOutOfRange,
    /// A malformed `$link` / CID.
    #[error("invalid link: {0}")]
    InvalidLink(String),
    /// A malformed `$bytes` object.
    #[error("invalid $bytes: {0}")]
    InvalidBytes(&'static str),
    /// A malformed blob object.
    #[error("invalid blob: {0}")]
    InvalidBlob(&'static str),
    /// `$type` is present but not a non-empty string.
    #[error("$type must be a non-empty string")]
    InvalidType,
    /// A map key reserved by the JSON encoding (`$link`, `$bytes`) in a plain map.
    #[error("reserved key {0:?} in a map")]
    ReservedKey(String),
    /// The top-level value is not an object/map.
    #[error("record must be an object")]
    NotAnObject,
    /// Malformed or non-canonical CBOR.
    #[error("invalid DAG-CBOR at byte {offset}: {reason}")]
    Cbor {
        /// Byte offset of the problem.
        offset: usize,
        /// What is wrong.
        reason: &'static str,
    },
    /// Malformed JSON text.
    #[error("invalid JSON: {0}")]
    Json(String),
}

type Result<T> = core::result::Result<T, DataModelError>;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

// ---------------------------------------------------------------------------------------------
// JSON -> IPLD

/// Converts atproto JSON to IPLD with [`Limits::default`]. Any top-level value is accepted.
pub fn json_to_ipld(v: &Value) -> Result<Ipld> {
    json_to_ipld_with(v, &Limits::default())
}

/// Converts atproto JSON to IPLD with explicit limits.
pub fn json_to_ipld_with(v: &Value, limits: &Limits) -> Result<Ipld> {
    let mut nodes = 0usize;
    from_json(v, 0, limits, &mut nodes)
}

/// Converts a record (a top-level JSON object) to IPLD.
pub fn record_json_to_ipld(v: &Value) -> Result<Ipld> {
    if !v.is_object() {
        return Err(DataModelError::NotAnObject);
    }
    json_to_ipld(v)
}

/// Parses JSON text (bytes) as a record and converts it to IPLD.
pub fn record_json_bytes_to_ipld(bytes: &[u8]) -> Result<Ipld> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|e| DataModelError::Json(e.to_string()))?;
    record_json_to_ipld(&v)
}

fn enter(depth: usize, limits: &Limits) -> Result<usize> {
    let d = depth + 1;
    if d > limits.max_depth {
        Err(DataModelError::DepthExceeded {
            max: limits.max_depth,
        })
    } else {
        Ok(d)
    }
}

fn json_number(n: &Number) -> Result<Ipld> {
    if let Some(i) = n.as_i64() {
        return Ok(Ipld::Integer(i128::from(i)));
    }
    if n.as_u64().is_some() {
        return Err(DataModelError::IntegerOutOfRange);
    }
    match n.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER => {
            Ok(Ipld::Integer(f as i128))
        }
        _ => Err(DataModelError::Float),
    }
}

/// Decodes `$bytes` base64 (standard alphabet, padding optional).
pub fn decode_base64(s: &str) -> Result<Vec<u8>> {
    let r = if s.ends_with('=') {
        BASE64.decode(s.as_bytes())
    } else {
        BASE64_NOPAD.decode(s.as_bytes())
    };
    r.map_err(|_| DataModelError::InvalidBytes("invalid base64"))
}

/// Encodes bytes as `$bytes` base64 (standard alphabet, no padding).
pub fn encode_base64(b: &[u8]) -> String {
    BASE64_NOPAD.encode(b)
}

fn from_json(v: &Value, depth: usize, limits: &Limits, nodes: &mut usize) -> Result<Ipld> {
    *nodes += 1;
    if *nodes > limits.max_total_nodes {
        return Err(DataModelError::TooManyNodes {
            max: limits.max_total_nodes,
        });
    }
    Ok(match v {
        Value::Null => Ipld::Null,
        Value::Bool(b) => Ipld::Bool(*b),
        Value::Number(n) => json_number(n)?,
        Value::String(s) => Ipld::String(check_string(s, limits)?.to_owned()),
        Value::Array(items) => {
            let d = enter(depth, limits)?;
            check_len(items.len(), limits)?;
            let mut out = Vec::new();
            for item in items {
                out.push(from_json(item, d, limits, nodes)?);
            }
            Ipld::List(out)
        }
        Value::Object(obj) => {
            if let Some(link) = obj.get("$link") {
                if obj.len() != 1 {
                    return Err(DataModelError::InvalidLink(
                        "$link object must have exactly one key".into(),
                    ));
                }
                let s = link
                    .as_str()
                    .ok_or_else(|| DataModelError::InvalidLink("$link must be a string".into()))?;
                let cid = crate::cid::parse_blessed(s)
                    .map_err(|e| DataModelError::InvalidLink(e.to_string()))?;
                return Ok(Ipld::Link(cid));
            }
            if let Some(bytes) = obj.get("$bytes") {
                if obj.len() != 1 {
                    return Err(DataModelError::InvalidBytes(
                        "$bytes object must have exactly one key",
                    ));
                }
                let s = bytes
                    .as_str()
                    .ok_or(DataModelError::InvalidBytes("$bytes must be a string"))?;
                // base64 expands 3 -> 4; reject before decoding.
                if s.len() / 4 * 3 > limits.max_bytes_len.saturating_add(3) {
                    return Err(DataModelError::BytesTooLong {
                        len: s.len() / 4 * 3,
                        max: limits.max_bytes_len,
                    });
                }
                let b = decode_base64(s)?;
                if b.len() > limits.max_bytes_len {
                    return Err(DataModelError::BytesTooLong {
                        len: b.len(),
                        max: limits.max_bytes_len,
                    });
                }
                return Ok(Ipld::Bytes(b));
            }
            let d = enter(depth, limits)?;
            check_len(obj.len(), limits)?;
            let mut map = BTreeMap::new();
            for (k, val) in obj {
                check_string(k, limits)?;
                map.insert(k.clone(), from_json(val, d, limits, nodes)?);
            }
            check_special_map(&map)?;
            Ipld::Map(map)
        }
    })
}

/// A blessed CID with the raw codec (a blob `ref`).
fn is_blob_cid(c: &Cid) -> bool {
    crate::cid::is_blessed(c) && c.codec() == crate::cid::RAW
}

fn check_link(c: &Cid) -> Result<()> {
    if crate::cid::is_blessed(c) {
        Ok(())
    } else {
        Err(DataModelError::InvalidLink(
            "not a blessed CID (v1, sha2-256, dag-cbor or raw)".into(),
        ))
    }
}

fn check_string<'s>(s: &'s str, limits: &Limits) -> Result<&'s str> {
    if s.len() > limits.max_string_bytes {
        return Err(DataModelError::StringTooLong {
            len: s.len(),
            max: limits.max_string_bytes,
        });
    }
    Ok(s)
}

fn check_len(len: usize, limits: &Limits) -> Result<()> {
    if len > limits.max_container_len {
        return Err(DataModelError::ContainerTooLarge {
            len,
            max: limits.max_container_len,
        });
    }
    Ok(())
}

/// Checks `$type` and blob structure of one map (non-recursive).
fn check_special_map(map: &BTreeMap<String, Ipld>) -> Result<()> {
    if map.contains_key("$link") || map.contains_key("$bytes") {
        let k = if map.contains_key("$link") {
            "$link"
        } else {
            "$bytes"
        };
        return Err(DataModelError::ReservedKey(k.to_owned()));
    }
    match map.get("$type") {
        None => Ok(()),
        Some(Ipld::String(t)) if t == "blob" => check_blob(map),
        Some(Ipld::String(t)) if !t.is_empty() => Ok(()),
        Some(_) => Err(DataModelError::InvalidType),
    }
}

fn check_blob(map: &BTreeMap<String, Ipld>) -> Result<()> {
    match map.get("ref") {
        Some(Ipld::Link(c)) if is_blob_cid(c) => {}
        Some(Ipld::Link(_)) => {
            return Err(DataModelError::InvalidBlob(
                "ref must be a blessed CID with the raw codec",
            ));
        }
        _ => return Err(DataModelError::InvalidBlob("ref must be a link")),
    }
    match map.get("mimeType") {
        Some(Ipld::String(m)) if !m.is_empty() => {}
        _ => {
            return Err(DataModelError::InvalidBlob(
                "mimeType must be a non-empty string",
            ));
        }
    }
    match map.get("size") {
        Some(Ipld::Integer(n)) if *n >= 0 => Ok(()),
        _ => Err(DataModelError::InvalidBlob(
            "size must be a non-negative integer",
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// IPLD -> JSON and validation

/// Converts IPLD to atproto JSON (links as `$link`, bytes as unpadded `$bytes`), validating the
/// data model on the way (see [`validate`]).
pub fn ipld_to_json(v: &Ipld) -> Result<Value> {
    to_json(v, 0, &Limits::default())
}

fn to_json(v: &Ipld, depth: usize, limits: &Limits) -> Result<Value> {
    Ok(match v {
        Ipld::Null => Value::Null,
        Ipld::Bool(b) => Value::Bool(*b),
        Ipld::Integer(i) => Value::Number(
            i64::try_from(*i)
                .map_err(|_| DataModelError::IntegerOutOfRange)?
                .into(),
        ),
        Ipld::Float(_) => return Err(DataModelError::Float),
        Ipld::String(s) => Value::String(s.clone()),
        Ipld::Bytes(b) => {
            let mut m = Map::new();
            m.insert("$bytes".into(), Value::String(encode_base64(b)));
            Value::Object(m)
        }
        Ipld::Link(c) => {
            check_link(c)?;
            let mut m = Map::new();
            m.insert("$link".into(), Value::String(c.to_string()));
            Value::Object(m)
        }
        Ipld::List(items) => {
            let d = enter(depth, limits)?;
            Value::Array(
                items
                    .iter()
                    .map(|i| to_json(i, d, limits))
                    .collect::<Result<_>>()?,
            )
        }
        Ipld::Map(map) => {
            let d = enter(depth, limits)?;
            check_special_map(map)?;
            let mut m = Map::new();
            for (k, val) in map {
                m.insert(k.clone(), to_json(val, d, limits)?);
            }
            Value::Object(m)
        }
    })
}

/// Validates an IPLD value against the abstract atproto data model: no floats, integers in
/// `i64`, blessed CIDs only, `$type` non-empty strings, well-formed blobs (with a raw-codec
/// `ref`), no reserved `$link`/`$bytes` keys in plain maps, depth within [`Limits::default`].
pub fn validate(v: &Ipld) -> Result<()> {
    validate_inner(v, 0, &Limits::default())
}

/// [`validate`] with an explicit depth limit (`limits.max_depth`).
pub fn validate_with(v: &Ipld, limits: &Limits) -> Result<()> {
    validate_inner(v, 0, limits)
}

/// The nesting depth of a value as `limits.md` §1 counts it, on the data model: the number of
/// maps and lists on the path from the root to the deepest value (a root map is depth 1, a scalar
/// root 0). Links and byte strings are leaves, so a `{"$link": …}` or `{"$bytes": …}` object adds
/// nothing, and a record has the same depth in JSON and in DAG-CBOR. Iterative.
pub fn nesting_depth(v: &Ipld) -> usize {
    let mut max = 0;
    let mut stack = vec![(v, 0usize)];
    while let Some((v, d)) = stack.pop() {
        match v {
            Ipld::List(items) => {
                max = max.max(d + 1);
                stack.extend(items.iter().map(|i| (i, d + 1)));
            }
            Ipld::Map(map) => {
                max = max.max(d + 1);
                stack.extend(map.values().map(|i| (i, d + 1)));
            }
            _ => {}
        }
    }
    max
}

fn validate_inner(v: &Ipld, depth: usize, limits: &Limits) -> Result<()> {
    match v {
        Ipld::Float(_) => Err(DataModelError::Float),
        Ipld::Integer(i) => i64::try_from(*i)
            .map(|_| ())
            .map_err(|_| DataModelError::IntegerOutOfRange),
        Ipld::Link(c) => check_link(c),
        Ipld::List(items) => {
            let d = enter(depth, limits)?;
            items.iter().try_for_each(|i| validate_inner(i, d, limits))
        }
        Ipld::Map(map) => {
            let d = enter(depth, limits)?;
            check_special_map(map)?;
            map.values().try_for_each(|i| validate_inner(i, d, limits))
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Records

/// Encodes a JSON record as canonical DAG-CBOR.
pub fn record_to_cbor(v: &Value) -> Result<Vec<u8>> {
    record_to_cbor_with(v, &Limits::default())
}

/// Encodes a JSON record as canonical DAG-CBOR under explicit limits, used for both the
/// conversion and the encoding. For example, admission (`sync.md` §5.5) converts and CID-checks
/// records up to `NESTING_MAX_DOCUMENT` (36) deep, and validity rule G10 then rejects a depth
/// over 32 ([`nesting_depth`]).
pub fn record_to_cbor_with(v: &Value, limits: &Limits) -> Result<Vec<u8>> {
    if !v.is_object() {
        return Err(DataModelError::NotAnObject);
    }
    dagcbor::encode_with(&json_to_ipld_with(v, limits)?, limits)
}

/// The CID of a JSON record: sha2-256 CIDv1 (dag-cbor) of its canonical DAG-CBOR encoding.
pub fn record_cid(v: &Value) -> Result<Cid> {
    Ok(crate::cid::for_dag_cbor(&record_to_cbor(v)?))
}

/// [`record_cid`] under explicit limits (see [`record_to_cbor_with`]).
pub fn record_cid_with(v: &Value, limits: &Limits) -> Result<Cid> {
    Ok(crate::cid::for_dag_cbor(&record_to_cbor_with(v, limits)?))
}

/// Decodes a DAG-CBOR record block: strict decode, top-level map, data-model validation.
pub fn record_from_cbor(bytes: &[u8]) -> Result<Ipld> {
    record_from_cbor_with(bytes, &Limits::default())
}

/// [`record_from_cbor`] under explicit limits (decoding and validation).
pub fn record_from_cbor_with(bytes: &[u8], limits: &Limits) -> Result<Ipld> {
    let v = dagcbor::decode_with(bytes, limits)?;
    if !matches!(v, Ipld::Map(_)) {
        return Err(DataModelError::NotAnObject);
    }
    validate_with(&v, limits)?;
    Ok(v)
}

/// Decodes a DAG-CBOR record block to atproto JSON.
pub fn record_cbor_to_json(bytes: &[u8]) -> Result<Value> {
    ipld_to_json(&record_from_cbor(bytes)?)
}

// ---------------------------------------------------------------------------------------------
// Blob references

/// A blob reference found in a record (`{$type: "blob", ref, mimeType, size}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    /// The blob CID (`ref`).
    pub cid: Cid,
    /// The declared MIME type.
    pub mime_type: String,
    /// The declared size in bytes.
    pub size: i64,
}

/// Collects every well-formed blob reference in a value, anywhere (including unknown and nested
/// fields), depth-first with map keys in sorted order. Malformed blob objects (including a `ref`
/// that is not a blessed raw-codec CID) and legacy `{cid, mimeType}` blobs are skipped. Iterative;
/// never recurses.
pub fn blob_refs(v: &Ipld) -> Vec<BlobRef> {
    let mut out = Vec::new();
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Ipld::Map(map) => {
                if let (
                    Some(Ipld::String(t)),
                    Some(Ipld::Link(cid)),
                    Some(Ipld::String(m)),
                    Some(Ipld::Integer(n)),
                ) = (
                    map.get("$type"),
                    map.get("ref"),
                    map.get("mimeType"),
                    map.get("size"),
                ) && t == "blob"
                    && is_blob_cid(cid)
                    && !m.is_empty()
                    && let Ok(size) = i64::try_from(*n)
                    && size >= 0
                {
                    out.push(BlobRef {
                        cid: *cid,
                        mime_type: m.clone(),
                        size,
                    });
                }
                stack.extend(map.values().rev());
            }
            Ipld::List(items) => stack.extend(items.iter().rev()),
            _ => {}
        }
    }
    out
}

/// The distinct blob CIDs of a value, in first-seen order (see [`blob_refs`]).
pub fn blob_cids(v: &Ipld) -> Vec<Cid> {
    let mut seen = Vec::new();
    for b in blob_refs(v) {
        if !seen.contains(&b.cid) {
            seen.push(b.cid);
        }
    }
    seen
}

/// Like [`blob_refs`] for atproto JSON; values that fail conversion yield no refs.
pub fn blob_refs_json(v: &Value) -> Vec<BlobRef> {
    json_to_ipld(v).map(|i| blob_refs(&i)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const C: &str = "bafkreiccldh766hwcnuxnf2wh6jgzepf2nlu2lvcllt63eww5p6chi4ity";

    #[test]
    fn json_roundtrip() {
        let v = json!({
            "$type": "com.example.thing",
            "i": -5, "big": 9007199254740993i64, "s": "héllo", "n": null, "t": true,
            "l": [{"$link": C}, {"$bytes": "AAEC"}],
            "blob": {"$type": "blob", "ref": {"$link": C}, "mimeType": "image/png", "size": 3}
        });
        let ipld = record_json_to_ipld(&v).unwrap();
        let back = ipld_to_json(&ipld).unwrap();
        assert_eq!(back, v);
        let cbor = dagcbor::encode(&ipld).unwrap();
        assert_eq!(record_cbor_to_json(&cbor).unwrap(), v);
        assert_eq!(record_cid(&v).unwrap(), crate::cid::for_dag_cbor(&cbor));
    }

    #[test]
    fn numbers() {
        assert_eq!(json_to_ipld(&json!(123.0)).unwrap(), Ipld::Integer(123));
        assert_eq!(json_to_ipld(&json!(-0.0)).unwrap(), Ipld::Integer(0));
        assert_eq!(json_to_ipld(&json!(1.5)), Err(DataModelError::Float));
        assert_eq!(json_to_ipld(&json!(1e300)), Err(DataModelError::Float));
        assert_eq!(
            json_to_ipld(&json!(u64::MAX)),
            Err(DataModelError::IntegerOutOfRange)
        );
        assert_eq!(
            json_to_ipld(&json!(i64::MIN)).unwrap(),
            Ipld::Integer(i128::from(i64::MIN))
        );
        assert!(ipld_to_json(&Ipld::Integer(i128::from(i64::MAX) + 1)).is_err());
        assert!(ipld_to_json(&Ipld::Float(1.0)).is_err());
    }

    #[test]
    fn bytes_padding() {
        assert_eq!(
            json_to_ipld(&json!({"$bytes": "AAE="})).unwrap(),
            Ipld::Bytes(vec![0, 1])
        );
        assert_eq!(
            json_to_ipld(&json!({"$bytes": "AAE"})).unwrap(),
            Ipld::Bytes(vec![0, 1])
        );
        assert!(json_to_ipld(&json!({"$bytes": "A"})).is_err());
        assert!(json_to_ipld(&json!({"$bytes": "AA-_"})).is_err());
        assert_eq!(
            ipld_to_json(&Ipld::Bytes(vec![0, 1])).unwrap(),
            json!({"$bytes": "AAE"})
        );
    }

    #[test]
    fn reserved_keys_and_types() {
        let mut m = BTreeMap::new();
        m.insert("$link".to_owned(), Ipld::String("x".into()));
        assert!(matches!(
            ipld_to_json(&Ipld::Map(m.clone())),
            Err(DataModelError::ReservedKey(_))
        ));
        assert!(validate(&Ipld::Map(m)).is_err());
        assert_eq!(
            json_to_ipld(&json!({"$type": ""})),
            Err(DataModelError::InvalidType)
        );
        assert_eq!(
            json_to_ipld(&json!({"$type": 1})),
            Err(DataModelError::InvalidType)
        );
        assert!(
            json_to_ipld(&json!({"$type": "blob", "ref": {"$link": C}, "mimeType": "", "size": 1}))
                .is_err()
        );
        assert!(
            json_to_ipld(
                &json!({"$type": "blob", "ref": {"$link": C}, "mimeType": "a/b", "size": -1})
            )
            .is_err()
        );
        assert!(record_json_to_ipld(&json!([1])).is_err());
    }

    #[test]
    fn depth_limit() {
        let mut v = json!(1);
        for _ in 0..32 {
            v = json!([v]);
        }
        assert!(json_to_ipld(&v).is_ok());
        let v = json!([v]);
        assert!(matches!(
            json_to_ipld(&v),
            Err(DataModelError::DepthExceeded { max: 32 })
        ));
        let lim = Limits {
            max_container_len: 1,
            ..Limits::default()
        };
        assert!(json_to_ipld_with(&json!([1, 2]), &lim).is_err());
        let lim = Limits {
            max_string_bytes: 1,
            ..Limits::default()
        };
        assert!(json_to_ipld_with(&json!({"ab": 1}), &lim).is_err());
        let lim = Limits {
            max_total_nodes: 2,
            ..Limits::default()
        };
        assert!(json_to_ipld_with(&json!([1, 2]), &lim).is_err());
        let lim = Limits {
            max_bytes_len: 1,
            ..Limits::default()
        };
        assert!(json_to_ipld_with(&json!({"$bytes": "AAE"}), &lim).is_err());
    }

    #[test]
    fn blobs() {
        let other = "bafkreihwihm6kpd6zuwhhlro75p5qks5qtrcu55jp3gddbfjsieiv7wuka";
        let v = json!({
            "a": {"$type": "blob", "ref": {"$link": C}, "mimeType": "image/png", "size": 3},
            "z": [{"unknown": {"deep": {"$type": "blob", "ref": {"$link": other}, "mimeType": "video/mp4", "size": 9}}}],
            "dup": {"$type": "blob", "ref": {"$link": C}, "mimeType": "image/png", "size": 3},
            "legacy": {"cid": C, "mimeType": "image/png"},
        });
        let refs = blob_refs_json(&v);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].cid.to_string(), C);
        assert_eq!(refs[2].mime_type, "video/mp4");
        let cids = blob_cids(&json_to_ipld(&v).unwrap());
        assert_eq!(
            cids.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            vec![C, other]
        );
    }

    #[test]
    fn whole_number_floats_are_correctly_rounded() {
        for (s, want) in [
            ("9007199254740991.0", 9_007_199_254_740_991i64),
            ("-9007199254740991.0", -9_007_199_254_740_991),
            ("9007199254665365.000000", 9_007_199_254_665_365),
            ("123.000000000000000000000", 123),
            ("1.23e2", 123),
            ("100000000000000000000e-18", 100),
            ("-0.0", 0),
        ] {
            let v: Value = serde_json::from_str(s).unwrap();
            assert_eq!(
                json_to_ipld(&v).unwrap(),
                Ipld::Integer(i128::from(want)),
                "{s}"
            );
        }
        // agrees with the correctly rounded `str::parse::<f64>` (what JavaScript does) on the
        // inputs that the default serde_json parser got wrong
        for i in 0..20_000u64 {
            let n = 9_007_199_254_740_991 - i * 37_813;
            for suffix in [".0", ".00", ".000000"] {
                let s = format!("{n}{suffix}");
                let v: Value = serde_json::from_str(&s).unwrap();
                assert_eq!(v.as_f64(), Some(s.parse::<f64>().unwrap()), "{s}");
                assert_eq!(
                    json_to_ipld(&v).unwrap(),
                    Ipld::Integer(i128::from(n)),
                    "{s}"
                );
            }
        }
        // 2^53 and above are not safe integers
        for s in ["9007199254740992.0", "9007199254740993.0", "1e16"] {
            let v: Value = serde_json::from_str(s).unwrap();
            assert_eq!(json_to_ipld(&v), Err(DataModelError::Float), "{s}");
        }
    }

    #[test]
    fn only_blessed_cids() {
        // dag-pb (sha2-256) is a valid CIDv1 but not part of the atproto data model
        let pb = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
        assert!(crate::cid::parse(pb).is_ok());
        let v = json!({"$type": "a.b.c", "l": {"$link": pb}});
        assert!(matches!(
            record_json_to_ipld(&v),
            Err(DataModelError::InvalidLink(_))
        ));
        assert!(record_cid(&v).is_err());
        // an identity-hash link inside CBOR: strict DAG-CBOR, but not a valid record
        let mh = multihash::Multihash::<64>::wrap(0x00, b"hello").unwrap();
        let id = Cid::new_v1(crate::cid::DAG_CBOR, mh);
        let m = Ipld::Map([("x".to_owned(), Ipld::Link(id))].into());
        let b = dagcbor::encode(&m).unwrap();
        assert_eq!(dagcbor::decode(&b).unwrap(), m);
        assert!(matches!(
            record_from_cbor(&b),
            Err(DataModelError::InvalidLink(_))
        ));
        assert!(validate(&m).is_err());
        assert!(ipld_to_json(&m).is_err());
        // a blob ref must be a raw-codec CID
        let cbor_ref = "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454";
        let blob = json!({"$type": "blob", "ref": {"$link": cbor_ref}, "mimeType": "image/png", "size": 1});
        assert_eq!(
            json_to_ipld(&json!({"b": blob})),
            Err(DataModelError::InvalidBlob(
                "ref must be a blessed CID with the raw codec"
            ))
        );
        let mut raw_map = BTreeMap::new();
        raw_map.insert("$type".to_owned(), Ipld::String("blob".into()));
        raw_map.insert(
            "ref".to_owned(),
            Ipld::Link(crate::cid::parse(cbor_ref).unwrap()),
        );
        raw_map.insert("mimeType".to_owned(), Ipld::String("image/png".into()));
        raw_map.insert("size".to_owned(), Ipld::Integer(1));
        let raw_map = Ipld::Map(raw_map);
        assert!(blob_refs(&raw_map).is_empty());
        assert!(validate(&raw_map).is_err());
        // raw and dag-cbor links elsewhere are fine
        assert!(json_to_ipld(&json!({"a": {"$link": cbor_ref}, "b": {"$link": C}})).is_ok());
    }

    #[test]
    fn bytes_cap_does_not_overflow() {
        let lim = Limits {
            max_bytes_len: usize::MAX,
            ..Limits::default()
        };
        assert_eq!(
            json_to_ipld_with(&json!({"$bytes": "AAAA"}), &lim).unwrap(),
            Ipld::Bytes(vec![0, 0, 0])
        );
    }

    #[test]
    fn depth_counts_data_model_containers() {
        let objects = |n: usize| {
            let mut v = json!(1);
            for _ in 0..n {
                v = json!({"a": v});
            }
            v
        };
        assert!(json_to_ipld(&objects(32)).is_ok());
        assert_eq!(
            json_to_ipld(&objects(33)),
            Err(DataModelError::DepthExceeded { max: 32 })
        );
        // alternating maps and lists
        let mut v = json!(null);
        for i in 0..32 {
            v = if i % 2 == 0 {
                json!([v])
            } else {
                json!({"k": v})
            };
        }
        let ipld = json_to_ipld(&v).unwrap();
        let cbor = dagcbor::encode(&ipld).unwrap();
        assert_eq!(record_from_cbor(&cbor).unwrap(), ipld);
        assert!(json_to_ipld(&json!({"top": v})).is_err());
        // `$link` / `$bytes` objects are leaves, as their DAG-CBOR forms are: 32 containers
        // around one is the limit in both encodings
        for leaf in [json!({"$link": C}), json!({"$bytes": "AAE"})] {
            let mut v = leaf;
            for _ in 0..31 {
                v = json!([v]);
            }
            let v = json!({"x": v});
            let ipld = json_to_ipld(&v).unwrap();
            let cbor = dagcbor::encode(&ipld).unwrap();
            assert_eq!(record_from_cbor(&cbor).unwrap(), ipld);
            assert_eq!(record_cbor_to_json(&cbor).unwrap(), v);
            let deeper = json!([v]);
            assert!(json_to_ipld(&deeper).is_err());
            let ipld = Ipld::List(vec![ipld]);
            assert_eq!(nesting_depth(&ipld), 33);
            assert!(dagcbor::encode(&ipld).is_err());
        }
        assert_eq!(nesting_depth(&Ipld::Integer(1)), 0);
        assert_eq!(
            nesting_depth(&json_to_ipld(&json!({"a": [], "b": [[{}]]})).unwrap()),
            4
        );
        // admission converts deeper records under the document limit; G10 then judges the depth
        let doc = Limits {
            max_depth: 36,
            ..Limits::default()
        };
        let deep = objects(34);
        assert!(record_to_cbor(&deep).is_err());
        let cbor = record_to_cbor_with(&deep, &doc).unwrap();
        assert_eq!(
            record_cid_with(&deep, &doc).unwrap(),
            crate::cid::for_dag_cbor(&cbor)
        );
        assert!(record_from_cbor(&cbor).is_err());
        let back = record_from_cbor_with(&cbor, &doc).unwrap();
        assert_eq!(nesting_depth(&back), 34);
        assert_eq!(dagcbor::encode_with(&back, &doc).unwrap(), cbor);
        assert!(record_to_cbor_with(&objects(37), &doc).is_err());
        assert_eq!(
            record_to_cbor_with(&json!([1]), &doc),
            Err(DataModelError::NotAnObject)
        );
    }
}
