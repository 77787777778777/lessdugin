// SPDX-License-Identifier: MIT OR Apache-2.0
//! Repository commit (v3) decoding and signature verification (`/specs/repository`,
//! "Commit Objects").
//!
//! A commit is `{did, version, data, rev, prev, sig}`. The signature is ECDSA (low-S) over
//! `sha256(dag-cbor(commit without "sig"))`. The unsigned bytes are produced by removing `sig`
//! from the decoded map and re-encoding it canonically, so `prev` presence (`null` vs. absent,
//! both seen in the wild) and any unknown fields are preserved exactly as signed.

use std::collections::BTreeMap;

use ::cid::Cid;

use crate::Ipld;
use crate::cid::CidError;
use crate::crypto::{CryptoError, PublicKey};
use crate::datamodel::{DataModelError, Limits};
use crate::syntax::{Did, Tid};

/// The only supported repo format version.
pub const VERSION: i64 = 3;
/// Maximum accepted clock skew for `rev` (5 minutes, in microseconds).
pub const MAX_FUTURE_SKEW_MICROS: i64 = 5 * 60 * 1_000_000;
/// Maximum size of a commit block.
pub const MAX_COMMIT_BYTES: usize = 16 * 1024;

/// Commit errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// The block is larger than [`MAX_COMMIT_BYTES`].
    #[error("commit block too large")]
    TooLarge,
    /// Not valid DAG-CBOR.
    #[error("commit is not valid DAG-CBOR: {0}")]
    Decode(DataModelError),
    /// A field is missing or has the wrong type.
    #[error("malformed commit: {0}")]
    Malformed(&'static str),
    /// `data` or `prev` is not a blessed dag-cbor CID.
    #[error("commit link: {0}")]
    Cid(CidError),
    /// `version` is not 3.
    #[error("unsupported commit version {0}")]
    Version(i64),
    /// `did` differs from the expected repo DID.
    #[error("commit DID {found} does not match {expected}")]
    DidMismatch {
        /// The repo DID.
        expected: String,
        /// The commit's `did`.
        found: String,
    },
    /// `rev` is more than 5 minutes in the future.
    #[error("commit rev {0} is in the future")]
    FutureRev(String),
    /// The signature does not verify.
    #[error("commit signature: {0}")]
    Signature(CryptoError),
}

/// A decoded commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Repo DID.
    pub did: Did,
    /// Format version.
    pub version: i64,
    /// MST root.
    pub data: Cid,
    /// Revision (TID).
    pub rev: Tid,
    /// Previous commit (`null` or absent → `None`).
    pub prev: Option<Cid>,
    /// Raw signature bytes.
    pub sig: Vec<u8>,
    unsigned: Vec<u8>,
}

fn commit_limits() -> Limits {
    Limits {
        max_depth: 4,
        max_container_len: 64,
        max_string_bytes: 4096,
        max_bytes_len: 4096,
        max_total_nodes: 256,
    }
}

impl Commit {
    /// Decodes a commit block. Validates field types (`did` with generic DID syntax, `rev` as a
    /// TID, `data`/`prev` as blessed dag-cbor CIDs, `sig` as bytes) but not the signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, CommitError> {
        if bytes.len() > MAX_COMMIT_BYTES {
            return Err(CommitError::TooLarge);
        }
        let v =
            crate::dagcbor::decode_with(bytes, &commit_limits()).map_err(CommitError::Decode)?;
        let Ipld::Map(mut map) = v else {
            return Err(CommitError::Malformed("not a map"));
        };
        let did = match map.get("did") {
            Some(Ipld::String(s)) => {
                Did::parse(s).map_err(|_| CommitError::Malformed("invalid did"))?
            }
            _ => return Err(CommitError::Malformed("did must be a string")),
        };
        let version = match map.get("version") {
            Some(Ipld::Integer(v)) => {
                i64::try_from(*v).map_err(|_| CommitError::Malformed("version"))?
            }
            _ => return Err(CommitError::Malformed("version must be an integer")),
        };
        let data = match map.get("data") {
            Some(Ipld::Link(c)) => *c,
            _ => return Err(CommitError::Malformed("data must be a link")),
        };
        crate::cid::check_blessed_dag_cbor(&data).map_err(CommitError::Cid)?;
        let rev = match map.get("rev") {
            Some(Ipld::String(s)) => {
                Tid::parse(s).map_err(|_| CommitError::Malformed("rev must be a TID"))?
            }
            _ => return Err(CommitError::Malformed("rev must be a string")),
        };
        let prev = match map.get("prev") {
            None | Some(Ipld::Null) => None,
            Some(Ipld::Link(c)) => {
                crate::cid::check_blessed_dag_cbor(c).map_err(CommitError::Cid)?;
                Some(*c)
            }
            Some(_) => return Err(CommitError::Malformed("prev must be a link or null")),
        };
        let sig = match map.remove("sig") {
            Some(Ipld::Bytes(b)) => b,
            _ => return Err(CommitError::Malformed("sig must be bytes")),
        };
        let unsigned = crate::dagcbor::encode(&Ipld::Map(map)).map_err(CommitError::Decode)?;
        Ok(Self {
            did,
            version,
            data,
            rev,
            prev,
            sig,
            unsigned,
        })
    }

    /// The canonical DAG-CBOR of the commit without `sig` (the signed bytes).
    pub fn unsigned_bytes(&self) -> &[u8] {
        &self.unsigned
    }

    /// Verifies the commit: `version == 3`, `did == expected_did`,
    /// `rev ≤ now + 5 min` (`now_micros` since the Unix epoch), and a low-S signature by `key`.
    pub fn verify(
        &self,
        expected_did: &Did,
        key: &PublicKey,
        now_micros: i64,
    ) -> Result<(), CommitError> {
        if self.version != VERSION {
            return Err(CommitError::Version(self.version));
        }
        if &self.did != expected_did {
            return Err(CommitError::DidMismatch {
                expected: expected_did.to_string(),
                found: self.did.to_string(),
            });
        }
        let rev_micros = i64::try_from(self.rev.timestamp_micros()).unwrap_or(i64::MAX);
        if rev_micros > now_micros.saturating_add(MAX_FUTURE_SKEW_MICROS) {
            return Err(CommitError::FutureRev(self.rev.to_string()));
        }
        key.verify(&self.unsigned, &self.sig)
            .map_err(CommitError::Signature)
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
        crate::dagcbor::encode(&Ipld::Map(self.map())).unwrap_or_default()
    }

    /// The signed commit block and its CID, given a 64-byte signature over
    /// [`UnsignedCommit::signing_bytes`].
    pub fn signed(&self, sig: &[u8]) -> (Cid, Vec<u8>) {
        let mut m = self.map();
        m.insert("sig".to_owned(), Ipld::Bytes(sig.to_vec()));
        let bytes = crate::dagcbor::encode(&Ipld::Map(m)).unwrap_or_default();
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
        assert!(c.verify(&u.did, &pk, NOW).is_ok());
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
            ("rev", Ipld::String("nope".into())),
            ("data", Ipld::Link(crate::cid::for_raw(b"x"))),
            ("prev", Ipld::Bool(true)),
            ("sig", Ipld::String("x".into())),
            ("version", Ipld::String("3".into())),
        ] {
            let mut m = u.map();
            m.insert("sig".to_owned(), Ipld::Bytes(vec![0; 64]));
            m.insert(k.to_owned(), v);
            assert!(
                Commit::decode(&crate::dagcbor::encode(&Ipld::Map(m)).unwrap()).is_err(),
                "{k}"
            );
        }
        assert!(Commit::decode(&[0xa0]).is_err());
        assert!(Commit::decode(&[0x80]).is_err());
        assert_eq!(
            Commit::decode(&vec![0u8; MAX_COMMIT_BYTES + 1]),
            Err(CommitError::TooLarge)
        );
    }
}
