// SPDX-License-Identifier: AGPL-3.0-or-later
//! DID document checks for identity level `doc` (`sync.md` §3.2), through `lessdugin-atp`'s
//! [`DidDocument`] under the [`DocPolicy`] of the mode ([`policy`]).
//!
//! 1. The document is a JSON object whose `id` equals the resolved DID.
//! 2. **PDS:** exactly one `service` entry has id `#atproto_pds` (or `<did>#atproto_pds`) and type
//!    `AtprotoPersonalDataServer`; its endpoint is an origin URL (no userinfo, path, query or
//!    fragment) with scheme `https` and a DNS name of at least two labels that is not an IP
//!    literal, `localhost`, or under a reserved or local-only TLD (`.local`, `.internal`,
//!    `.localhost`, `.arpa` — so `home.arpa` —, `.onion`, `.test`, …; `threat-model.md` §5.6.2).
//!    `http`, IP literals and local names are allowed only in devnet mode (`sync.md` §3.7).
//! 3. **Key:** exactly one `verificationMethod` has id `#atproto` (or `<did>#atproto`) and type
//!    `Multikey`, with a `publicKeyMultibase` for secp256k1 or P-256. The legacy
//!    `EcdsaSecp256k1VerificationKey2019` / `EcdsaSecp256r1VerificationKey2019` encodings are
//!    accepted only in devnet mode (the devnet PLC still emits them), so that every client reaches
//!    the same verdict on production documents. Anything else leaves the key unavailable without
//!    affecting the PDS.
//!
//! The claimed handle is the first `alsoKnownAs` entry starting with `at://`, without that prefix,
//! lowercased — unvalidated here (see `handle_state`).

use lessdugin_atp::crypto::{Curve, PublicKey};
use lessdugin_atp::did_doc::{
    DidDocError, DidDocument, DocPolicy, EndpointError, ServiceEndpoint, ServiceError,
};
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

/// The document policy of a mode: [`DocPolicy::STRICT`] in production, [`DocPolicy::DEVNET`]
/// (http, IP literals, local names, legacy key encodings) in devnet mode.
pub fn policy(devnet: bool) -> DocPolicy {
    if devnet {
        DocPolicy::DEVNET
    } else {
        DocPolicy::STRICT
    }
}

fn endpoint_problem(e: EndpointError) -> String {
    match e {
        EndpointError::Syntax(why) => format!("the PDS endpoint is not an origin URL ({why})"),
        EndpointError::Insecure => "the PDS endpoint must use https".to_owned(),
        EndpointError::IpLiteral => {
            "the PDS endpoint must be a hostname (not an IP literal)".to_owned()
        }
        EndpointError::LocalName => {
            "the PDS endpoint is localhost or a local-only or reserved name".to_owned()
        }
    }
}

/// Checks a service endpoint (check 2) and normalizes it to `scheme://host[:port]`.
pub fn check_service_endpoint(endpoint: &str, devnet: bool) -> Result<String, String> {
    ServiceEndpoint::parse_with(endpoint, &policy(devnet))
        .map(|e| e.as_str().to_owned())
        .map_err(endpoint_problem)
}

/// Validates a DID document for `did`.
pub fn validate(bytes: &[u8], did: &Did, devnet: bool) -> Result<DocFacts, DidDocError> {
    let doc = DidDocument::parse_for(bytes, did)?;
    let policy = policy(devnet);
    let pds = doc
        .atproto_pds(&policy)
        .map(|e| e.as_str().to_owned())
        .map_err(|e| match e {
            ServiceError::Missing => "no #atproto_pds service".to_owned(),
            ServiceError::Ambiguous(n) => {
                format!("{n} #atproto_pds services (exactly one is allowed)")
            }
            ServiceError::Invalid(e) => endpoint_problem(e),
        });
    let key = doc.atproto_key(&policy).map_err(|e| e.to_string());
    Ok(DocFacts {
        pds,
        key,
        claimed_handle: doc.claimed_handle(),
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
        // Local-only and reserved names are refused in production (threat-model §5.6.2): the
        // browser has no DNS guard behind this check.
        for local in [
            "https://pds.local",
            "https://router.internal",
            "https://x.localhost",
            "https://pds.test",
            "https://x.home.arpa",
            "https://foo.onion",
            "https://pds.example",
            "https://router",
            "https://[::1]",
            "https://127.1",
        ] {
            assert!(with_endpoint(local).is_err(), "{local}");
            assert!(check_service_endpoint(local, false).is_err(), "{local}");
        }
        assert!(
            check_service_endpoint("https://pds.local", false)
                .unwrap_err()
                .contains("local-only")
        );
        // devnet allows http, localhost, local names and IP literals
        assert_eq!(
            check_service_endpoint("http://pds.test:2583", true).as_deref(),
            Ok("http://pds.test:2583")
        );
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
        // Legacy key encodings are accepted only in devnet mode (sync.md §3.2: Multikey only).
        let legacy = "zQYEBzXeuTM9UR3rfvNag6L3RNAs5pQZyYPsomTsgQhsxLdEgCrPTLgFna8yqCnxPpNT7DBk6Ym3dgPKNu86vt9GR";
        let mut v = base();
        v["verificationMethod"][0]["type"] = json!("EcdsaSecp256k1VerificationKey2019");
        v["verificationMethod"][0]["publicKeyMultibase"] = json!(legacy);
        let prod = validate(&doc(v.clone()), &did, false).unwrap();
        assert!(prod.key.unwrap_err().contains("unsupported"));
        assert!(prod.pds.is_ok());
        let dev = validate(&doc(v), &did, true).unwrap();
        assert_eq!(curve_name(&dev.key.unwrap()), "k256");
        // A second #atproto method of another type does not hide the Multikey one.
        let mut v = base();
        v["verificationMethod"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "#atproto", "type": "JsonWebKey2020"}));
        assert!(validate(&doc(v), &did, false).unwrap().key.is_ok());
        // no aliases
        let mut v = base();
        v["alsoKnownAs"] = json!([]);
        assert_eq!(validate(&doc(v), &did, false).unwrap().claimed_handle, None);
    }
}
