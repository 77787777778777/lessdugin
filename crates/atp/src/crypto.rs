// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public keys (did:key / Multikey, `/specs/cryptography`) and low-S ECDSA verification for
//! secp256k1 (k256) and NIST P-256 (p256).
//!
//! Signatures are 64-byte compact `r ‖ s` over `sha256(message)`. Non-low-S signatures are
//! rejected for both curves, as atproto requires for signatures over CBOR data. Only verification
//! lives here; no secret keys are handled.

use k256::ecdsa as k;
use k256::ecdsa::signature::Verifier as _;
use p256::ecdsa as p;

/// Multicodec varint prefix for a compressed secp256k1 public key (0xe7).
const K256_PREFIX: [u8; 2] = [0xe7, 0x01];
/// Multicodec varint prefix for a compressed P-256 public key (0x1200).
const P256_PREFIX: [u8; 2] = [0x80, 0x24];
const DID_KEY_PREFIX: &str = "did:key:";

/// Crypto errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// Not a `did:key:` identifier.
    #[error("not a did:key")]
    NotDidKey,
    /// The multibase string is not `z` + base58btc.
    #[error("invalid multibase (expected 'z' + base58btc)")]
    Multibase,
    /// Unknown or missing multicodec prefix.
    #[error("unsupported key type (multicodec)")]
    UnsupportedKeyType,
    /// The key bytes are not a valid point on the curve.
    #[error("invalid public key")]
    InvalidKey,
    /// A Multikey / did:key holds an uncompressed point; atproto requires the 33-byte compressed
    /// form (`/specs/cryptography`, `sync.md` §3.2).
    #[error("public key is not a 33-byte compressed point")]
    NotCompressed,
    /// The signature is not 64 bytes of `r ‖ s` with valid scalars.
    #[error("malformed signature (expected 64-byte compact r||s)")]
    MalformedSignature,
    /// The signature uses a high S value (malleable); atproto requires low-S.
    #[error("high-S signature")]
    HighS,
    /// The signature does not verify.
    #[error("signature verification failed")]
    BadSignature,
}

/// Supported curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Curve {
    /// secp256k1 (`k256`, JWT `ES256K`).
    Secp256k1,
    /// NIST P-256 (`p256`, JWT `ES256`).
    P256,
}

/// A verified-parseable atproto public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicKey {
    /// secp256k1 key.
    Secp256k1(k::VerifyingKey),
    /// P-256 key.
    P256(p::VerifyingKey),
}

impl PublicKey {
    /// Parses a `did:key:z…` identifier.
    pub fn from_did_key(s: &str) -> Result<Self, CryptoError> {
        let mb = s
            .strip_prefix(DID_KEY_PREFIX)
            .ok_or(CryptoError::NotDidKey)?;
        Self::from_multikey(mb)
    }

    /// Parses a Multikey `publicKeyMultibase` (`z` + base58btc of multicodec prefix ‖ key): the
    /// multicodec `secp256k1-pub` (0xe7) or `p256-pub` (0x1200) followed by a 33-byte compressed
    /// SEC1 point. Anything else is rejected ([`CryptoError::UnsupportedKeyType`],
    /// [`CryptoError::NotCompressed`], [`CryptoError::InvalidKey`]).
    pub fn from_multikey(s: &str) -> Result<Self, CryptoError> {
        let bytes = decode_z_base58(s)?;
        let (curve, key) = if let Some(key) = bytes.strip_prefix(&K256_PREFIX) {
            (Curve::Secp256k1, key)
        } else if let Some(key) = bytes.strip_prefix(&P256_PREFIX) {
            (Curve::P256, key)
        } else {
            return Err(CryptoError::UnsupportedKeyType);
        };
        if key.len() == 65 {
            return Err(CryptoError::NotCompressed);
        }
        if key.len() != 33 {
            return Err(CryptoError::InvalidKey);
        }
        Self::from_sec1(curve, key)
    }

    /// Parses the legacy DID-document encoding (`EcdsaSecp256k1VerificationKey2019` /
    /// `EcdsaSecp256r1VerificationKey2019`): `z` + base58btc of the raw SEC1 point without a
    /// multicodec prefix; the curve comes from the verification-method type. The spec describes
    /// uncompressed points; compressed points (seen in the interop fixtures) are accepted too.
    pub fn from_legacy_multibase(curve: Curve, s: &str) -> Result<Self, CryptoError> {
        Self::from_sec1(curve, &decode_z_base58(s)?)
    }

    /// Parses a SEC1-encoded point (33-byte compressed or 65-byte uncompressed).
    pub fn from_sec1(curve: Curve, bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() != 33 && bytes.len() != 65 {
            return Err(CryptoError::InvalidKey);
        }
        match curve {
            Curve::Secp256k1 => k::VerifyingKey::from_sec1_bytes(bytes).map(Self::Secp256k1),
            Curve::P256 => p::VerifyingKey::from_sec1_bytes(bytes).map(Self::P256),
        }
        .map_err(|_| CryptoError::InvalidKey)
    }

    /// The curve.
    pub fn curve(&self) -> Curve {
        match self {
            Self::Secp256k1(_) => Curve::Secp256k1,
            Self::P256(_) => Curve::P256,
        }
    }

    /// The 33-byte compressed SEC1 point.
    pub fn to_compressed(&self) -> Vec<u8> {
        match self {
            Self::Secp256k1(k) => k.to_encoded_point(true).as_bytes().to_vec(),
            Self::P256(p) => p.to_encoded_point(true).as_bytes().to_vec(),
        }
    }

    /// The Multikey string (`z…`), as used in `publicKeyMultibase`.
    pub fn multikey(&self) -> String {
        let prefix = match self.curve() {
            Curve::Secp256k1 => K256_PREFIX,
            Curve::P256 => P256_PREFIX,
        };
        let mut b = prefix.to_vec();
        b.extend_from_slice(&self.to_compressed());
        multibase::encode(multibase::Base::Base58Btc, b)
    }

    /// The `did:key:z…` identifier.
    pub fn did_key(&self) -> String {
        format!("{DID_KEY_PREFIX}{}", self.multikey())
    }

    /// Verifies a 64-byte compact signature over `sha256(msg)`, rejecting high-S signatures.
    pub fn verify(&self, msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        if sig.len() != 64 {
            return Err(CryptoError::MalformedSignature);
        }
        match self {
            Self::Secp256k1(key) => {
                let s =
                    k::Signature::from_slice(sig).map_err(|_| CryptoError::MalformedSignature)?;
                if s.normalize_s().is_some() {
                    return Err(CryptoError::HighS);
                }
                key.verify(msg, &s).map_err(|_| CryptoError::BadSignature)
            }
            Self::P256(key) => {
                let s =
                    p::Signature::from_slice(sig).map_err(|_| CryptoError::MalformedSignature)?;
                if s.normalize_s().is_some() {
                    return Err(CryptoError::HighS);
                }
                key.verify(msg, &s).map_err(|_| CryptoError::BadSignature)
            }
        }
    }
}

impl core::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.did_key())
    }
}

fn decode_z_base58(s: &str) -> Result<Vec<u8>, CryptoError> {
    let body = s.strip_prefix('z').ok_or(CryptoError::Multibase)?;
    // A compressed or uncompressed key with prefix is < 100 base58 chars; bound the work.
    if body.is_empty() || body.len() > 128 {
        return Err(CryptoError::Multibase);
    }
    multibase::Base::Base58Btc
        .decode(body)
        .map_err(|_| CryptoError::Multibase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::signature::Signer as _;

    #[test]
    fn spec_examples() {
        let p =
            PublicKey::from_did_key("did:key:zDnaembgSGUhZULN2Caob4HLJPaxBh92N7rtH21TErzqf8HQo")
                .unwrap();
        assert_eq!(p.curve(), Curve::P256);
        assert_eq!(
            p.did_key(),
            "did:key:zDnaembgSGUhZULN2Caob4HLJPaxBh92N7rtH21TErzqf8HQo"
        );
        let k =
            PublicKey::from_multikey("zQ3shqwJEJyMBsBXCWyCBpUBMqxcon9oHB7mCvx4sSpMdLJwc").unwrap();
        assert_eq!(k.curve(), Curve::Secp256k1);
        assert_eq!(
            k.multikey(),
            "zQ3shqwJEJyMBsBXCWyCBpUBMqxcon9oHB7mCvx4sSpMdLJwc"
        );
        // legacy uncompressed example from /specs/did
        let legacy = PublicKey::from_legacy_multibase(
            Curve::Secp256k1,
            "zQYEBzXeuTM9UR3rfvNag6L3RNAs5pQZyYPsomTsgQhsxLdEgCrPTLgFna8yqCnxPpNT7DBk6Ym3dgPKNu86vt9GR",
        )
        .unwrap();
        assert_eq!(
            legacy.multikey(),
            "zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(
            PublicKey::from_did_key("did:web:x"),
            Err(CryptoError::NotDidKey)
        );
        assert_eq!(
            PublicKey::from_multikey("Q3sh"),
            Err(CryptoError::Multibase)
        );
        assert_eq!(
            PublicKey::from_multikey("z0OIl"),
            Err(CryptoError::Multibase)
        );
        assert_eq!(
            PublicKey::from_multikey("z111"),
            Err(CryptoError::UnsupportedKeyType)
        );
        let mut b = K256_PREFIX.to_vec();
        b.extend_from_slice(&[2u8; 8]);
        let s = multibase::encode(multibase::Base::Base58Btc, &b);
        assert_eq!(PublicKey::from_multikey(&s), Err(CryptoError::InvalidKey));
        // an uncompressed point under a Multikey prefix is not accepted (both curves)
        let k = k::SigningKey::from_slice(&[7u8; 32]).unwrap();
        let mut b = K256_PREFIX.to_vec();
        b.extend_from_slice(k.verifying_key().to_encoded_point(false).as_bytes());
        let s = multibase::encode(multibase::Base::Base58Btc, &b);
        assert_eq!(
            PublicKey::from_multikey(&s),
            Err(CryptoError::NotCompressed)
        );
        assert_eq!(
            PublicKey::from_did_key(&format!("did:key:{s}")),
            Err(CryptoError::NotCompressed)
        );
        let pk = p::SigningKey::from_slice(&[9u8; 32]).unwrap();
        let mut b = P256_PREFIX.to_vec();
        b.extend_from_slice(pk.verifying_key().to_encoded_point(false).as_bytes());
        let s = multibase::encode(multibase::Base::Base58Btc, &b);
        assert_eq!(
            PublicKey::from_multikey(&s),
            Err(CryptoError::NotCompressed)
        );
        // the compressed form of the same keys is fine
        let ok = PublicKey::Secp256k1(*k.verifying_key()).multikey();
        assert!(PublicKey::from_multikey(&ok).is_ok());
    }

    #[test]
    fn sign_verify_low_s() {
        let sk = k::SigningKey::from_slice(&[7u8; 32]).unwrap();
        let pk = PublicKey::Secp256k1(*sk.verifying_key());
        let sig: k::Signature = sk.sign(b"hello");
        let bytes = sig.to_bytes();
        assert!(pk.verify(b"hello", &bytes).is_ok());
        assert_eq!(pk.verify(b"hellO", &bytes), Err(CryptoError::BadSignature));
        assert_eq!(
            pk.verify(b"hello", &bytes[..63]),
            Err(CryptoError::MalformedSignature)
        );
        assert_eq!(
            pk.verify(b"hello", &[0u8; 64]),
            Err(CryptoError::MalformedSignature)
        );
        // flip to high-S: s' = n - s
        let (r, s) = sig.split_scalars();
        let high = k::Signature::from_scalars(r, -*s).unwrap();
        assert_eq!(
            pk.verify(b"hello", &high.to_bytes()),
            Err(CryptoError::HighS)
        );
        assert_eq!(PublicKey::from_did_key(&pk.did_key()).unwrap(), pk);

        let psk = p::SigningKey::from_slice(&[9u8; 32]).unwrap();
        let ppk = PublicKey::P256(*psk.verifying_key());
        let psig: p::Signature = psk.sign(b"msg");
        let low = psig.normalize_s().unwrap_or(psig);
        assert!(ppk.verify(b"msg", &low.to_bytes()).is_ok());
        let (r, s) = low.split_scalars();
        let high = p::Signature::from_scalars(r, -*s).unwrap();
        assert_eq!(
            ppk.verify(b"msg", &high.to_bytes()),
            Err(CryptoError::HighS)
        );
        assert_eq!(PublicKey::from_did_key(&ppk.did_key()).unwrap(), ppk);
        assert_eq!(ppk.to_string(), ppk.did_key());
    }
}
