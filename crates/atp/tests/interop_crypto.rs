// SPDX-License-Identifier: MIT OR Apache-2.0
//! atproto interop crypto fixtures (`testdata/interop/crypto`).

mod common;

use lessdugin_atp::crypto::{CryptoError, Curve, PublicKey};

#[test]
fn signature_fixtures() {
    let fixtures = common::json("crypto/signature-fixtures.json");
    let fixtures = fixtures.as_array().unwrap();
    assert_eq!(fixtures.len(), 6);
    let mut high_s = 0;
    for f in fixtures {
        let msg = common::b64(f["messageBase64"].as_str().unwrap());
        let sig = common::b64(f["signatureBase64"].as_str().unwrap());
        let valid = f["validSignature"].as_bool().unwrap();
        let curve = match f["algorithm"].as_str().unwrap() {
            "ES256" => Curve::P256,
            "ES256K" => Curve::Secp256k1,
            a => panic!("unknown algorithm {a}"),
        };
        let did_key = f["publicKeyDid"].as_str().unwrap();
        let from_did = PublicKey::from_did_key(did_key).unwrap();
        assert_eq!(from_did.curve(), curve);
        assert_eq!(from_did.did_key(), did_key);
        let suite_curve = match f["didDocSuite"].as_str().unwrap() {
            "EcdsaSecp256r1VerificationKey2019" => Curve::P256,
            _ => Curve::Secp256k1,
        };
        let legacy = PublicKey::from_legacy_multibase(
            suite_curve,
            f["publicKeyMultibase"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(legacy, from_did);
        for key in [&from_did, &legacy] {
            let r = key.verify(&msg, &sig);
            assert_eq!(r.is_ok(), valid, "{}: {r:?}", f["comment"]);
        }
        let tags: Vec<&str> = f["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap())
            .collect();
        if tags.contains(&"high-s") {
            assert_eq!(from_did.verify(&msg, &sig), Err(CryptoError::HighS));
            high_s += 1;
        }
        if tags.contains(&"der-encoded") {
            assert_eq!(
                from_did.verify(&msg, &sig),
                Err(CryptoError::MalformedSignature)
            );
        }
    }
    assert_eq!(high_s, 2);
}

#[test]
fn did_key_k256() {
    let fixtures = common::json("crypto/w3c_didkey_K256.json");
    for f in fixtures.as_array().unwrap() {
        let sk_bytes = data_encoding::HEXLOWER
            .decode(f["privateKeyBytesHex"].as_str().unwrap().as_bytes())
            .unwrap();
        let sk = k256::ecdsa::SigningKey::from_slice(&sk_bytes).unwrap();
        let pk = PublicKey::Secp256k1(*sk.verifying_key());
        let expected = f["publicDidKey"].as_str().unwrap();
        assert_eq!(pk.did_key(), expected);
        assert_eq!(PublicKey::from_did_key(expected).unwrap(), pk);
    }
}

#[test]
fn did_key_p256() {
    let fixtures = common::json("crypto/w3c_didkey_P256.json");
    for f in fixtures.as_array().unwrap() {
        let sk_bytes = multibase::Base::Base58Btc
            .decode(f["privateKeyBytesBase58"].as_str().unwrap())
            .unwrap();
        let sk = p256::ecdsa::SigningKey::from_slice(&sk_bytes).unwrap();
        let pk = PublicKey::P256(*sk.verifying_key());
        let expected = f["publicDidKey"].as_str().unwrap();
        assert_eq!(pk.did_key(), expected);
        assert_eq!(PublicKey::from_did_key(expected).unwrap(), pk);
    }
}
