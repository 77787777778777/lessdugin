// SPDX-License-Identifier: MIT OR Apache-2.0
//! CIDv1 computation, parsing and verification (`/specs/data-model`, "Link and CID Formats").
//!
//! atproto's "blessed" CIDs are version 1, sha2-256 (0x12, 32 bytes), with codec dag-cbor (0x71)
//! for data objects or raw (0x55) for blobs, written in base32 lowercase (`b…`) as strings.
//!
//! Strictness, by function:
//! - [`parse`] / [`from_bytes`]: any CIDv1 (codec and hash unchecked), canonical encoding only
//!   (string must be lowercase base32 `b…`; bytes must contain exactly one CID). CIDv0 is always
//!   rejected: atproto requires v1 everywhere.
//! - [`check_blessed`] / [`parse_blessed`]: additionally require sha2-256 and dag-cbor or raw.
//!   Used for commit, MST node and CAR-root links, where the spec demands the blessed format.
//! - [`verify`]: recomputes sha2-256 over bytes; non-sha256 CIDs cannot be verified and fail.
//! - [`is_valid_cid_syntax`]: the loose Lexicon `cid` string-format check (any multibase).

use ::cid::{Cid, Version};
use multihash::Multihash;
use sha2::{Digest, Sha256};

/// Multicodec for DAG-CBOR (DRISL).
pub const DAG_CBOR: u64 = 0x71;
/// Multicodec for raw bytes (blobs).
pub const RAW: u64 = 0x55;
/// Multihash code for sha2-256.
pub const SHA2_256: u64 = 0x12;

/// CID errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CidError {
    /// The string or bytes are not a well-formed CID.
    #[error("malformed CID: {0}")]
    Malformed(&'static str),
    /// CIDv0 is not allowed in atproto.
    #[error("CIDv0 is not allowed")]
    V0,
    /// The string is not in canonical form (lowercase base32 with `b` prefix).
    #[error("CID string is not canonical base32")]
    NotCanonical,
    /// The CID is valid but not one of the blessed atproto formats.
    #[error("CID is not a blessed atproto CID (v1, sha2-256, dag-cbor/raw)")]
    NotBlessed,
    /// The hash function is not sha2-256, so the bytes cannot be verified.
    #[error("unsupported hash function 0x{0:x}")]
    UnsupportedHash(u64),
    /// The bytes do not hash to the CID.
    #[error("bytes do not match CID {0}")]
    Mismatch(Cid),
}

/// sha2-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A CIDv1 with the given codec and a sha2-256 digest.
pub fn from_sha256_digest(codec: u64, digest: [u8; 32]) -> Cid {
    // A 32-byte digest always fits the 64-byte multihash.
    let mh = Multihash::<64>::wrap(SHA2_256, &digest).unwrap_or_default();
    Cid::new_v1(codec, mh)
}

/// The blessed CID of a DAG-CBOR block.
pub fn for_dag_cbor(bytes: &[u8]) -> Cid {
    from_sha256_digest(DAG_CBOR, sha256(bytes))
}

/// The blessed CID of raw bytes (a blob).
pub fn for_raw(bytes: &[u8]) -> Cid {
    from_sha256_digest(RAW, sha256(bytes))
}

/// Parses a canonical CID string: `b` + lowercase base32 (no padding) of a CIDv1.
pub fn parse(s: &str) -> Result<Cid, CidError> {
    let body = s.strip_prefix('b').ok_or(CidError::NotCanonical)?;
    if body.is_empty()
        || !body
            .bytes()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c))
    {
        return Err(CidError::NotCanonical);
    }
    let bytes = multibase::Base::Base32Lower
        .decode(body)
        .map_err(|_| CidError::Malformed("invalid base32"))?;
    let cid = from_bytes(&bytes)?;
    // Reject non-canonical trailing bits by re-encoding.
    if cid.to_string() != s {
        return Err(CidError::NotCanonical);
    }
    Ok(cid)
}

/// Parses a canonical CID string and requires the blessed format.
pub fn parse_blessed(s: &str) -> Result<Cid, CidError> {
    let cid = parse(s)?;
    check_blessed(&cid)?;
    Ok(cid)
}

/// Parses binary CID bytes; the slice must hold exactly one CIDv1.
pub fn from_bytes(bytes: &[u8]) -> Result<Cid, CidError> {
    if bytes.len() >= 2 && bytes[0] == 0x12 && bytes[1] == 0x20 {
        return Err(CidError::V0);
    }
    let cid = Cid::try_from(bytes).map_err(|_| CidError::Malformed("invalid binary CID"))?;
    if cid.version() != Version::V1 {
        return Err(CidError::V0);
    }
    if cid.encoded_len() != bytes.len() {
        return Err(CidError::Malformed("trailing bytes after CID"));
    }
    Ok(cid)
}

/// Reads a CIDv1 from the front of `bytes`, returning it and the number of bytes consumed.
pub(crate) fn read_prefix(bytes: &[u8]) -> Result<(Cid, usize), CidError> {
    if bytes.len() >= 2 && bytes[0] == 0x12 && bytes[1] == 0x20 {
        return Err(CidError::V0);
    }
    let mut r = bytes;
    let cid = Cid::read_bytes(&mut r).map_err(|_| CidError::Malformed("invalid binary CID"))?;
    if cid.version() != Version::V1 {
        return Err(CidError::V0);
    }
    let used = bytes.len() - r.len();
    // Defensive: a non-minimal varint would re-encode differently.
    if cid.encoded_len() != used {
        return Err(CidError::Malformed("non-canonical binary CID"));
    }
    Ok((cid, used))
}

/// `true` for a blessed atproto CID: v1, sha2-256 with a 32-byte digest, dag-cbor or raw.
pub fn is_blessed(cid: &Cid) -> bool {
    cid.version() == Version::V1
        && cid.hash().code() == SHA2_256
        && cid.hash().size() == 32
        && (cid.codec() == DAG_CBOR || cid.codec() == RAW)
}

/// Requires a blessed CID (see [`is_blessed`]).
pub fn check_blessed(cid: &Cid) -> Result<(), CidError> {
    if is_blessed(cid) {
        Ok(())
    } else {
        Err(CidError::NotBlessed)
    }
}

/// Requires a blessed dag-cbor CID (commit, MST node).
pub fn check_blessed_dag_cbor(cid: &Cid) -> Result<(), CidError> {
    if is_blessed(cid) && cid.codec() == DAG_CBOR {
        Ok(())
    } else {
        Err(CidError::NotBlessed)
    }
}

/// Verifies that `bytes` hash to `cid` (sha2-256 only; the codec is not checked).
pub fn verify(cid: &Cid, bytes: &[u8]) -> Result<(), CidError> {
    let mh = cid.hash();
    if mh.code() != SHA2_256 || mh.size() != 32 {
        return Err(CidError::UnsupportedHash(mh.code()));
    }
    if sha256(bytes)[..] == mh.digest()[..] {
        Ok(())
    } else {
        Err(CidError::Mismatch(*cid))
    }
}

/// Loose Lexicon `cid` string-format check (as in the reference implementations): 8–256
/// characters of `[A-Za-z0-9+=]`, not a CIDv0 (`Qm…`). It does not decode the CID.
pub fn is_valid_cid_syntax(s: &str) -> bool {
    (8..=256).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'=')
        && !s.starts_with("Qm")
}

/// Serde helpers to (de)serialize a [`Cid`] as its canonical string (`#[serde(with = ...)]`).
pub mod serde_str {
    use ::cid::Cid;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes as a base32 string.
    pub fn serialize<S: Serializer>(cid: &Cid, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(cid)
    }

    /// Deserializes with [`super::parse`].
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Cid, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        super::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Serde helpers for `Option<Cid>` as an optional canonical string.
pub mod serde_str_opt {
    use ::cid::Cid;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes `Some` as a base32 string and `None` as null.
    pub fn serialize<S: Serializer>(cid: &Option<Cid>, s: S) -> Result<S::Ok, S::Error> {
        match cid {
            Some(c) => s.collect_str(c),
            None => s.serialize_none(),
        }
    }

    /// Deserializes null/absent as `None`, strings with [`super::parse`].
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Cid>, D::Error> {
        let s = <Option<std::borrow::Cow<'de, str>>>::deserialize(d)?;
        s.map(|s| super::parse(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "bafyreidfayvfuwqa7qlnopdjiqrxzs6blmoeu4rujcjtnci5beludirz2a";

    #[test]
    fn parse_and_bless() {
        let c = parse(C).unwrap();
        assert_eq!(c.to_string(), C);
        assert!(is_blessed(&c));
        assert_eq!(c.codec(), DAG_CBOR);
        assert_eq!(from_bytes(&c.to_bytes()).unwrap(), c);
        let mut long = c.to_bytes();
        long.push(0);
        assert!(from_bytes(&long).is_err());
        assert_eq!(read_prefix(&long).unwrap(), (c, long.len() - 1));
        assert!(parse(&C.to_uppercase()).is_err());
        assert!(parse("QmbWqxBEKC3P8tqsKc98xmWNzrzDtRLMiMPL8wBuTGsMnR").is_err());
        assert!(parse("zdj7WWeQ43G6JJvLWQWZpyHuAMq6uYWRjkBXFad11vE2LHhQ7").is_err());
        assert!(parse("b").is_err());
        assert!(parse(".").is_err());
        assert!(
            parse_blessed("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi").is_err()
        );
        assert!(parse("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi").is_ok());
    }

    #[test]
    fn compute_and_verify() {
        let c = for_dag_cbor(b"\xa0");
        assert!(verify(&c, b"\xa0").is_ok());
        assert_eq!(verify(&c, b"\xa1"), Err(CidError::Mismatch(c)));
        let r = for_raw(b"hello");
        assert_eq!(r.codec(), RAW);
        assert!(check_blessed_dag_cbor(&r).is_err());
        assert!(check_blessed_dag_cbor(&c).is_ok());
        let v0 = [0x12u8, 0x20]
            .iter()
            .copied()
            .chain([0u8; 32])
            .collect::<Vec<_>>();
        assert_eq!(from_bytes(&v0), Err(CidError::V0));
    }

    #[test]
    fn serde() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct S {
            #[serde(with = "serde_str")]
            a: Cid,
            #[serde(with = "serde_str_opt", default)]
            b: Option<Cid>,
        }
        let s = S {
            a: parse(C).unwrap(),
            b: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(j, format!("{{\"a\":\"{C}\",\"b\":null}}"));
        assert_eq!(serde_json::from_str::<S>(&j).unwrap(), s);
        assert_eq!(
            serde_json::from_str::<S>(&format!("{{\"a\":\"{C}\"}}")).unwrap(),
            s
        );
        assert!(serde_json::from_str::<S>("{\"a\":\"nope\"}").is_err());
    }
}
