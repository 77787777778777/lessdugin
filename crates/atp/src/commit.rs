// SPDX-License-Identifier: MIT OR Apache-2.0
//! Repository commit (v3) decoding and signature verification (`/specs/repository`,
//! "Commit Objects"; `sync.md` §8.4).
//!
//! A commit is `{did, version, data, rev, prev, sig}`. The signature is ECDSA (low-S) over
//! `sha256(dag-cbor(commit without "sig"))`. The block is validated as strict DAG-CBOR (nesting
//! ≤ 32, no other caps) without being materialised; the known fields are then read, and the
//! unsigned bytes are the block's own encoding with the `sig` entry cut out. So `prev` presence
//! (`null` vs. absent, both seen in the wild) and any unknown fields, however large, are preserved
//! exactly as signed.
//!
//! [`Commit::decode`] checks only the field types (`BadCommit`). [`Commit::verify`] then checks, in
//! the order of `sync.md` §8.4: `version` = 3, `did` = the repo DID, `rev` is a TID, `rev` not
//! more than [`MAX_FUTURE_SKEW_MICROS`] ahead, and the signature.

use std::collections::BTreeMap;

use ::cid::Cid;

use crate::Ipld;
use crate::crypto::{CryptoError, PublicKey};
use crate::dagcbor::{decode_scalar, map_entries_raw, write_head};
use crate::datamodel::{DataModelError, Limits};
use crate::syntax::{Did, Tid};

/// The only supported repo format version.
pub const VERSION: i64 = 3;
/// Maximum accepted clock skew for `rev` (5 minutes, in microseconds; `FUTURE_SKEW_MS`).
pub const MAX_FUTURE_SKEW_MICROS: i64 = 5 * 60 * 1_000_000;

/// Commit errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// Not strict DAG-CBOR (`NonCanonical`).
    #[error("commit is not valid DAG-CBOR: {0}")]
    Decode(DataModelError),
    /// A field is missing or has the wrong type, or `rev` is not a TID (`BadCommit`).
    #[error("malformed commit: {0}")]
    Malformed(&'static str),
    /// `version` is not 3 (`BadCommit`).
    #[error("unsupported commit version {0}")]
    Version(i64),
    /// `did` differs from the expected repo DID (`DidMismatch`).
    #[error("commit DID {found} does not match {expected}")]
    DidMismatch {
        /// The repo DID.
        expected: String,
        /// The commit's `did`.
        found: String,
    },
    /// `rev` is more than 5 minutes in the future (`FutureRev`).
    #[error("commit rev {0} is in the future")]
    FutureRev(String),
    /// The signature is malformed, high-S, or does not verify (`BadSignature`).
    #[error("commit signature: {0}")]
    Signature(CryptoError),
}

/// A decoded (not yet verified) commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Repo DID, as written (verified against the expected DID by [`Commit::verify`]).
    pub did: String,
    /// Format version.
    pub version: i64,
    /// MST root.
    pub data: Cid,
    /// Revision, as written (see [`Commit::rev_tid`]).
    pub rev: String,
    /// Previous commit (`null` or absent → `None`).
    pub prev: Option<Cid>,
    /// Raw signature bytes.
    pub sig: Vec<u8>,
    unsigned: Vec<u8>,
}

impl Commit {
    /// Decodes a commit block: strict DAG-CBOR (any size, nesting ≤ 32), a map with `did`
    /// (text), `version` (integer), `data` (link), `rev` (text) and `sig` (bytes), and `prev`
    /// (link or null) if present. Other keys are allowed and covered by the signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, CommitError> {
        crate::dagcbor::validate_with(bytes, &Limits::STRICT).map_err(CommitError::Decode)?;
        let entries = map_entries_raw(bytes).ok_or(CommitError::Malformed("not a map"))?;
        let (mut did, mut version, mut data, mut rev, mut prev, mut sig) =
            (None, None, None, None, None, None);
        let mut body = Vec::with_capacity(bytes.len());
        let mut unsigned_len = 0u64;
        for (k, raw) in entries {
            let v = || decode_scalar(raw);
            match k {
                "did" => match v() {
                    Some(Ipld::String(s)) => did = Some(s),
                    _ => return Err(CommitError::Malformed("did must be a string")),
                },
                "version" => match v() {
                    // The decoder only yields integers within i64.
                    Some(Ipld::Integer(i)) => version = i64::try_from(i).ok(),
                    _ => return Err(CommitError::Malformed("version must be an integer")),
                },
                "data" => match v() {
                    Some(Ipld::Link(c)) => data = Some(c),
                    _ => return Err(CommitError::Malformed("data must be a link")),
                },
                "rev" => match v() {
                    Some(Ipld::String(s)) => rev = Some(s),
                    _ => return Err(CommitError::Malformed("rev must be a string")),
                },
                "prev" => match v() {
                    Some(Ipld::Null) => prev = Some(None),
                    Some(Ipld::Link(c)) => prev = Some(Some(c)),
                    _ => return Err(CommitError::Malformed("prev must be a link or null")),
                },
                "sig" => match v() {
                    Some(Ipld::Bytes(b)) => sig = Some(b),
                    _ => return Err(CommitError::Malformed("sig must be bytes")),
                },
                _ => {}
            }
            if k != "sig" {
                // Canonical key head + key + the value's own (canonical) bytes.
                write_head(&mut body, 3, k.len() as u64);
                body.extend_from_slice(k.as_bytes());
                body.extend_from_slice(raw);
                unsigned_len += 1;
            }
        }
        let mut unsigned = Vec::with_capacity(body.len() + 9);
        write_head(&mut unsigned, 5, unsigned_len);
        unsigned.extend_from_slice(&body);
        Ok(Self {
            did: did.ok_or(CommitError::Malformed("did must be a string"))?,
            version: version.ok_or(CommitError::Malformed("version must be an integer"))?,
            data: data.ok_or(CommitError::Malformed("data must be a link"))?,
            rev: rev.ok_or(CommitError::Malformed("rev must be a string"))?,
            prev: prev.flatten(),
            sig: sig.ok_or(CommitError::Malformed("sig must be bytes"))?,
            unsigned,
        })
    }

    /// The canonical DAG-CBOR of the commit without `sig` (the signed bytes).
    pub fn unsigned_bytes(&self) -> &[u8] {
        &self.unsigned
    }

    /// `rev` as a TID, if it is one.
    pub fn rev_tid(&self) -> Option<Tid> {
        Tid::parse(&self.rev).ok()
    }

    /// Verifies the commit, in the order of `sync.md` §8.4: `version == 3`,
    /// `did == expected_did`, `rev` is a TID, `rev ≤ now + 5 min` (`now_micros` since the Unix
    /// epoch), and a 64-byte low-S signature by `key`. Returns the rev.
    pub fn verify(
        &self,
        expected_did: &Did,
        key: &PublicKey,
        now_micros: i64,
    ) -> Result<Tid, CommitError> {
        if self.version != VERSION {
            return Err(CommitError::Version(self.version));
        }
        if self.did != expected_did.as_str() {
            return Err(CommitError::DidMismatch {
                expected: expected_did.to_string(),
                found: self.did.clone(),
            });
        }
        let rev = self
            .rev_tid()
            .ok_or(CommitError::Malformed("rev must be a TID"))?;
        let rev_micros = i64::try_from(rev.timestamp_micros()).unwrap_or(i64::MAX);
        if rev_micros > now_micros.saturating_add(MAX_FUTURE_SKEW_MICROS) {
            return Err(CommitError::FutureRev(self.rev.clone()));
        }
        key.verify(&self.unsigned, &self.sig)
            .map_err(CommitError::Signature)?;
        Ok(rev)
    }
}

/// An unsigned v3 commit, for building commits in tests, fixtures and fake servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsignedCommit {
    /// Repo DID.
    pub did: Did,
    /// MST root.
    pub data: Cid,
    /// Revision.
    pub rev: Tid,
    /// Previous commit (written as `null` when `None`, as v3 requires).
    pub prev: Option<Cid>,
}

impl UnsignedCommit {
    fn map(&self) -> BTreeMap<String, Ipld> {
        let mut m = BTreeMap::new();
        m.insert("did".to_owned(), Ipld::String(self.did.to_string()));
        m.insert("version".to_owned(), Ipld::Integer(i128::from(VERSION)));
        m.insert("data".to_owned(), Ipld::Link(self.data));
        m.insert("rev".to_owned(), Ipld::String(self.rev.to_string()));
        m.insert("prev".to_owned(), self.prev.map_or(Ipld::Null, Ipld::Link));
        m
    }

    /// The bytes to sign: canonical DAG-CBOR without `sig`.
    pub fn signing_bytes(&self) -> Vec<u8> {
        crate::dagcbor::encode_with(&Ipld::Map(self.map()), &Limits::uncapped(8))
            .unwrap_or_default()
    }

    /// The signed commit block and its CID, given a 64-byte signature over
    /// [`UnsignedCommit::signing_bytes`].
    pub fn signed(&self, sig: &[u8]) -> (Cid, Vec<u8>) {
        let mut m = self.map();
        m.insert("sig".to_owned(), Ipld::Bytes(sig.to_vec()));
        let bytes =
            crate::dagcbor::encode_with(&Ipld::Map(m), &Limits::uncapped(8)).unwrap_or_default();
        (crate::cid::for_dag_cbor(&bytes), bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::signature::Signer as _;
    use k256::ecdsa::{Signature, SigningKey};

    const NOW: i64 = 1_760_000_000_000_000;

    fn setup() -> (SigningKey, PublicKey, UnsignedCommit) {
        let sk = SigningKey::from_slice(&[42u8; 32]).unwrap();
        let pk = PublicKey::Secp256k1(*sk.verifying_key());
        let u = UnsignedCommit {
            did: Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap(),
            data: crate::cid::for_dag_cbor(b"\xa0"),
            rev: Tid::from_parts(NOW as u64, 0).unwrap(),
            prev: None,
        };
        (sk, pk, u)
    }

    fn sign(sk: &SigningKey, b: &[u8]) -> Vec<u8> {
        let s: Signature = sk.sign(b);
        s.to_bytes().to_vec()
    }

    #[test]
    fn verify_ok_and_failures() {
        let (sk, pk, u) = setup();
        let (_, bytes) = u.signed(&sign(&sk, &u.signing_bytes()));
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(c.unsigned_bytes(), u.signing_bytes().as_slice());
        assert_eq!(c.verify(&u.did, &pk, NOW).unwrap(), u.rev);
        assert_eq!(c.rev_tid(), Some(u.rev));
        // rev at the edge of the skew window
        assert!(c.verify(&u.did, &pk, NOW - MAX_FUTURE_SKEW_MICROS).is_ok());
        assert!(matches!(
            c.verify(&u.did, &pk, NOW - MAX_FUTURE_SKEW_MICROS - 1),
            Err(CommitError::FutureRev(_))
        ));
        let other = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(matches!(
            c.verify(&other, &pk, NOW),
            Err(CommitError::DidMismatch { .. })
        ));
        let wrong_key =
            PublicKey::Secp256k1(*SigningKey::from_slice(&[1u8; 32]).unwrap().verifying_key());
        assert!(matches!(
            c.verify(&u.did, &wrong_key, NOW),
            Err(CommitError::Signature(CryptoError::BadSignature))
        ));
    }

    #[test]
    fn high_s_rejected() {
        let (sk, pk, u) = setup();
        let s: Signature = sk.sign(&u.signing_bytes());
        let (r, sc) = s.split_scalars();
        let high = Signature::from_scalars(r, -*sc).unwrap();
        let (_, bytes) = u.signed(&high.to_bytes());
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(
            c.verify(&u.did, &pk, NOW),
            Err(CommitError::Signature(CryptoError::HighS))
        );
    }

    #[test]
    fn prev_absent_is_preserved() {
        let (sk, pk, u) = setup();
        let mut m = u.map();
        m.remove("prev");
        let unsigned = crate::dagcbor::encode(&Ipld::Map(m.clone())).unwrap();
        m.insert("sig".to_owned(), Ipld::Bytes(sign(&sk, &unsigned)));
        let bytes = crate::dagcbor::encode(&Ipld::Map(m)).unwrap();
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(c.prev, None);
        assert!(c.verify(&u.did, &pk, NOW).is_ok());
    }

    #[test]
    fn version_and_shape() {
        let (sk, pk, u) = setup();
        let mut m = u.map();
        m.insert("version".to_owned(), Ipld::Integer(2));
        let unsigned = crate::dagcbor::encode(&Ipld::Map(m.clone())).unwrap();
        m.insert("sig".to_owned(), Ipld::Bytes(sign(&sk, &unsigned)));
        let c = Commit::decode(&crate::dagcbor::encode(&Ipld::Map(m)).unwrap()).unwrap();
        assert_eq!(c.verify(&u.did, &pk, NOW), Err(CommitError::Version(2)));
        for (k, v) in [
            ("did", Ipld::Integer(1)),
            ("rev", Ipld::Integer(1)),
            ("data", Ipld::String("x".into())),
            ("data", Ipld::Null),
            ("prev", Ipld::Bool(true)),
            ("sig", Ipld::String("x".into())),
            ("version", Ipld::String("3".into())),
            ("version", Ipld::List(vec![])),
        ] {
            let mut m = u.map();
            m.insert("sig".to_owned(), Ipld::Bytes(vec![0; 64]));
            m.insert(k.to_owned(), v);
            assert!(
                matches!(
                    Commit::decode(&crate::dagcbor::encode(&Ipld::Map(m)).unwrap()),
                    Err(CommitError::Malformed(_))
                ),
                "{k}"
            );
        }
        for missing in ["did", "version", "data", "rev", "sig"] {
            let mut m = u.map();
            m.insert("sig".to_owned(), Ipld::Bytes(vec![0; 64]));
            m.remove(missing);
            assert!(
                matches!(
                    Commit::decode(&crate::dagcbor::encode(&Ipld::Map(m)).unwrap()),
                    Err(CommitError::Malformed(_))
                ),
                "{missing}"
            );
        }
        assert_eq!(
            Commit::decode(&[0xa0]),
            Err(CommitError::Malformed("did must be a string"))
        );
        assert_eq!(
            Commit::decode(&[0x80]),
            Err(CommitError::Malformed("not a map"))
        );
        // non-canonical CBOR (unsorted keys) is NonCanonical, not BadCommit
        assert!(matches!(
            Commit::decode(&[0xa2, 0x61, b'b', 0x01, 0x61, b'a', 0x02]),
            Err(CommitError::Decode(_))
        ));
    }

    /// Signs a commit map with `extra` fields merged in.
    fn signed_with(sk: &SigningKey, u: &UnsignedCommit, extra: Vec<(&str, Ipld)>) -> Vec<u8> {
        let mut m = u.map();
        for (k, v) in extra {
            m.insert(k.to_owned(), v);
        }
        let lim = Limits::uncapped(64);
        let unsigned = crate::dagcbor::encode_with(&Ipld::Map(m.clone()), &lim).unwrap();
        m.insert("sig".to_owned(), Ipld::Bytes(sign(sk, &unsigned)));
        crate::dagcbor::encode_with(&Ipld::Map(m), &lim).unwrap()
    }

    #[test]
    fn unknown_fields_are_signed_and_unbounded() {
        let (sk, pk, u) = setup();
        // nested 31 deep under the commit map (32 containers in all): accepted and covered by
        // the signature
        let mut deep = Ipld::List(vec![]);
        for _ in 0..30 {
            deep = Ipld::List(vec![deep]);
        }
        let big = Ipld::List((0..200_000).map(|_| Ipld::Null).collect());
        let bytes = signed_with(
            &sk,
            &u,
            vec![
                ("ext", deep.clone()),
                ("big", big),
                ("long", Ipld::String("x".repeat(70_000))),
                ("a", Ipld::Integer(-1)),
            ],
        );
        assert!(bytes.len() > 200_000);
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(c.verify(&u.did, &pk, NOW).unwrap(), u.rev);
        // tampering with an unknown field breaks the signature
        let tampered = signed_with(&sk, &u, vec![("ext", Ipld::Integer(1))]);
        let mut t = tampered.clone();
        let at = t.windows(4).position(|w| w == b"ext\x01").unwrap();
        t[at + 3] = 0x02;
        let c = Commit::decode(&t).unwrap();
        assert!(matches!(
            c.verify(&u.did, &pk, NOW),
            Err(CommitError::Signature(_))
        ));
        // 33 containers is too deep
        let bytes = signed_with(&sk, &u, vec![("ext", Ipld::List(vec![deep]))]);
        assert!(matches!(
            Commit::decode(&bytes),
            Err(CommitError::Decode(DataModelError::DepthExceeded {
                max: 32
            }))
        ));
        // `data` and `prev` may be any CID (the walk decides what a non-node `data` means)
        let mut odd = u.clone();
        odd.data = crate::cid::for_raw(b"x");
        odd.prev = Some(crate::cid::for_raw(b"y"));
        let (_, bytes) = odd.signed(&sign(&sk, &odd.signing_bytes()));
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(c.prev, odd.prev);
        assert!(c.verify(&u.did, &pk, NOW).is_ok());
    }

    #[test]
    fn checks_follow_spec_order() {
        // sync.md §8.4: version, then did, then rev syntax, then future rev, then signature
        let (sk, pk, u) = setup();
        let other = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let bytes = signed_with(
            &sk,
            &u,
            vec![
                ("version", Ipld::Integer(2)),
                ("rev", Ipld::String("nope".into())),
            ],
        );
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(c.verify(&other, &pk, NOW), Err(CommitError::Version(2)));
        let bytes = signed_with(
            &sk,
            &u,
            vec![
                ("did", Ipld::String("not a did".into())),
                ("rev", Ipld::String("nope".into())),
            ],
        );
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(c.rev_tid(), None);
        assert!(matches!(
            c.verify(&u.did, &pk, NOW),
            Err(CommitError::DidMismatch { .. })
        ));
        let bytes = signed_with(&sk, &u, vec![("rev", Ipld::String("nope".into()))]);
        let c = Commit::decode(&bytes).unwrap();
        assert_eq!(
            c.verify(&u.did, &pk, NOW),
            Err(CommitError::Malformed("rev must be a TID"))
        );
        // a future rev is reported before a bad signature
        let future = Tid::from_parts((NOW + MAX_FUTURE_SKEW_MICROS + 1) as u64, 0).unwrap();
        let bytes = signed_with(&sk, &u, vec![("rev", Ipld::String(future.to_string()))]);
        let c = Commit::decode(&bytes).unwrap();
        let wrong =
            PublicKey::Secp256k1(*SigningKey::from_slice(&[1u8; 32]).unwrap().verifying_key());
        assert!(matches!(
            c.verify(&u.did, &wrong, NOW),
            Err(CommitError::FutureRev(_))
        ));
        // signature length
        let mut m = u.map();
        m.insert("sig".to_owned(), Ipld::Bytes(vec![1; 63]));
        let c = Commit::decode(&crate::dagcbor::encode(&Ipld::Map(m)).unwrap()).unwrap();
        assert_eq!(
            c.verify(&u.did, &pk, NOW),
            Err(CommitError::Signature(CryptoError::MalformedSignature))
        );
    }
}
