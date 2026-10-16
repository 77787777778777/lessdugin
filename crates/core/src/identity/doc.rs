// SPDX-License-Identifier: AGPL-3.0-or-later
//! DID document checks for identity level `doc` (`sync.md` §3.2).
//!
//! 1. The document is a JSON object whose `id` equals the resolved DID.
//! 2. **PDS:** exactly one `service` entry has id `#atproto_pds` (or `<did>#atproto_pds`) and type
//!    `AtprotoPersonalDataServer`; its endpoint is an origin URL (no userinfo, path, query or
//!    fragment) with scheme `https` and a hostname that is not an IP literal or `localhost` —
//!    `http`, IP literals and `localhost` are allowed only in devnet mode (`sync.md` §3.7).
//! 3. **Key:** exactly one `verificationMethod` has id `#atproto` (or `<did>#atproto`) and a
//!    decodable `publicKeyMultibase` for secp256k1 or P-256. `Multikey` is the normal type; the
//!    legacy `EcdsaSecp256k1VerificationKey2019` / `EcdsaSecp256r1VerificationKey2019` encodings
//!    are accepted too (the devnet PLC still emits them). Anything else leaves the key unavailable
//!    without affecting the PDS.
//!
//! The claimed handle is the first `alsoKnownAs` entry starting with `at://`, without that prefix,
//! lowercased — unvalidated here (see `handle_state`).

use lessdugin_atp::crypto::{Curve, PublicKey};
use lessdugin_atp::did_doc::{DidDocError, DidDocument, normalize_service_endpoint};
use lessdugin_atp::syntax::Did;
use serde::Deserialize;

/// The atproto facts of a DID document that passed check 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocFacts {
    /// The PDS endpoint, normalized to `scheme://host[:port]`.
    pub pds: Result<String, String>,
    /// The `#atproto` signing key.
    pub key: Result<PublicKey, String>,
    /// The claimed handle (unvalidated, lowercased).
    pub claimed_handle: Option<String>,
}

fn id_matches(id: &str, did: &Did, fragment: &str) -> bool {
    id == fragment || id.strip_suffix(fragment) == Some(did.as_str())
}

/// Checks a service endpoint (check 2) and normalizes it.
pub fn check_service_endpoint(endpoint: &str, devnet: bool) -> Result<String, String> {
    let n = normalize_service_endpoint(endpoint)
        .ok_or_else(|| "the PDS endpoint is not an origin URL".to_owned())?;
    if devnet {
        return Ok(n);
    }
    let Some(rest) = n.strip_prefix("https://") else {
        return Err("the PDS endpoint must use https".to_owned());
    };
    let host = rest.rsplit_once(':').map_or(rest, |(h, _)| h);
    let is_ip = host
        .split('.')
        .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_digit()));
    if host == "localhost" || is_ip {
        return Err(
            "the PDS endpoint must be a hostname (not an IP literal or localhost)".to_owned(),
        );
    }
    Ok(n)
}

fn decode_key(type_: &str, multibase: Option<&str>) -> Result<PublicKey, String> {
    let mb = multibase.ok_or_else(|| "the #atproto key has no publicKeyMultibase".to_owned())?;
    let r = match type_ {
        "Multikey" => PublicKey::from_multikey(mb),
        "EcdsaSecp256k1VerificationKey2019" => {
            PublicKey::from_legacy_multibase(Curve::Secp256k1, mb)
        }
        "EcdsaSecp256r1VerificationKey2019" => PublicKey::from_legacy_multibase(Curve::P256, mb),
        other => return Err(format!("unsupported #atproto key type {other:?}")),
    };
    r.map_err(|e| format!("the #atproto key does not decode: {e}"))
}

/// Validates a DID document for `did`.
pub fn validate(bytes: &[u8], did: &Did, devnet: bool) -> Result<DocFacts, DidDocError> {
    let doc = DidDocument::parse_for(bytes, did)?;
    let pds = {
        let found: Vec<_> = doc
            .services
            .iter()
            .filter(|s| {
                id_matches(&s.id, did, "#atproto_pds") && s.type_ == "AtprotoPersonalDataServer"
            })
            .collect();
        match found.as_slice() {
            [] => Err("no #atproto_pds service".to_owned()),
            [s] => check_service_endpoint(&s.endpoint, devnet),
            _ => Err("more than one #atproto_pds service".to_owned()),
        }
    };
    let key = {
        let found: Vec<_> = doc
            .verification_methods
            .iter()
            .filter(|m| id_matches(&m.id, did, "#atproto"))
            .collect();
        match found.as_slice() {
            [] => Err("no #atproto verification method".to_owned()),
            [m] => decode_key(&m.type_, m.public_key_multibase.as_deref()),
            _ => Err("more than one #atproto verification method".to_owned()),
        }
    };
    let claimed_handle = doc
        .also_known_as
        .iter()
        .find_map(|a| a.strip_prefix("at://"))
        .map(str::to_ascii_lowercase);
    Ok(DocFacts {
        pds,
        key,
        claimed_handle,
    })
}

/// A Slingshot `com.bad-example.identity.resolveMiniDoc` answer (a hint, `sync.md` §3.2).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MiniDoc {
    /// The DID.
    pub did: String,
    /// The handle (Slingshot's own verdict; unverified for us).
    #[serde(default)]
    pub handle: Option<String>,
    /// The PDS endpoint.
    #[serde(default)]
    pub pds: Option<String>,
    /// The signing key (Multikey).
    #[serde(default)]
    pub signing_key: Option<String>,
}

/// `k256` or `p256`.
pub fn curve_name(key: &PublicKey) -> &'static str {
    match key.curve() {
        Curve::Secp256k1 => "k256",
        Curve::P256 => "p256",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";
    // secp256k1 Multikey from the atproto DID spec examples.
    const MK: &str = "zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF";

    fn doc(v: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&v).unwrap()
    }

    fn base() -> serde_json::Value {
        json!({
            "id": DID,
            "alsoKnownAs": ["https://x", "at://Alice.Example.COM", "at://second.example.com"],
            "verificationMethod": [{"id": "#atproto", "type": "Multikey", "controller": DID,
                                    "publicKeyMultibase": MK}],
            "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer",
                         "serviceEndpoint": "https://PDS.example.com/"}]
        })
    }

    #[test]
    fn good_doc() {
        let did = Did::parse(DID).unwrap();
        let f = validate(&doc(base()), &did, false).unwrap();
        assert_eq!(f.pds.as_deref(), Ok("https://pds.example.com"));
        assert_eq!(f.key.as_ref().map(PublicKey::multikey).as_deref(), Ok(MK));
        assert_eq!(curve_name(f.key.as_ref().unwrap()), "k256");
        assert_eq!(f.claimed_handle.as_deref(), Some("alice.example.com"));
        // absolute ids
        let mut v = base();
        v["service"][0]["id"] = json!(format!("{DID}#atproto_pds"));
        v["verificationMethod"][0]["id"] = json!(format!("{DID}#atproto"));
        let f = validate(&doc(v), &did, false).unwrap();
        assert!(f.pds.is_ok() && f.key.is_ok());
    }

    #[test]
    fn id_must_match() {
        let other = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(matches!(
            validate(&doc(base()), &other, false),
            Err(DidDocError::IdMismatch { .. })
        ));
        assert!(validate(b"[]", &other, false).is_err());
        assert!(validate(b"not json", &other, false).is_err());
    }

    #[test]
    fn pds_rules() {
        let did = Did::parse(DID).unwrap();
        let with_endpoint = |e: &str| {
            let mut v = base();
            v["service"][0]["serviceEndpoint"] = json!(e);
            validate(&doc(v), &did, false).unwrap().pds
        };
        assert!(with_endpoint("http://pds.example.com").is_err());
        assert!(with_endpoint("https://127.0.0.1").is_err());
        assert!(with_endpoint("https://localhost:2583").is_err());
        assert!(with_endpoint("https://pds.example.com/path").is_err());
        assert!(with_endpoint("https://user@pds.example.com").is_err());
        assert!(with_endpoint("https://pds.example.com?x").is_err());
        assert_eq!(
            with_endpoint("https://pds.example.com:8443").as_deref(),
            Ok("https://pds.example.com:8443")
        );
        // devnet allows http, localhost and IP literals
        assert_eq!(
            check_service_endpoint("http://localhost:2583", true).as_deref(),
            Ok("http://localhost:2583")
        );
        assert_eq!(
            check_service_endpoint("http://127.0.0.1:1", true).as_deref(),
            Ok("http://127.0.0.1:1")
        );
        // exactly one
        let mut v = base();
        let s = v["service"][0].clone();
        v["service"].as_array_mut().unwrap().push(s);
        assert!(validate(&doc(v), &did, false).unwrap().pds.is_err());
        let mut v = base();
        v["service"][0]["type"] = json!("Other");
        assert!(validate(&doc(v), &did, false).unwrap().pds.is_err());
    }

    #[test]
    fn key_rules() {
        let did = Did::parse(DID).unwrap();
        let mut v = base();
        v["verificationMethod"][0]["type"] = json!("JsonWebKey2020");
        let f = validate(&doc(v), &did, false).unwrap();
        assert!(f.key.is_err());
        assert!(f.pds.is_ok(), "an unusable key leaves the PDS usable");
        let mut v = base();
        let m = v["verificationMethod"][0].clone();
        v["verificationMethod"].as_array_mut().unwrap().push(m);
        assert!(validate(&doc(v), &did, false).unwrap().key.is_err());
        let mut v = base();
        v["verificationMethod"][0]["publicKeyMultibase"] = json!("zNotAKey");
        assert!(validate(&doc(v), &did, false).unwrap().key.is_err());
        // no aliases
        let mut v = base();
        v["alsoKnownAs"] = json!([]);
        assert_eq!(validate(&doc(v), &did, false).unwrap().claimed_handle, None);
    }
}
