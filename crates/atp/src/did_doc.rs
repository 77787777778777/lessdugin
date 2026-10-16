// SPDX-License-Identifier: MIT OR Apache-2.0
//! DID documents (`/specs/did`, "DID Documents"): the `#atproto` signing key, the
//! `#atproto_pds` service endpoint and the claimed handle (`alsoKnownAs`).
//!
//! Parsing is lenient about unrelated content (unknown fields, foreign verification methods and
//! services, wrongly-typed entries are ignored) and strict about what atproto needs. The document
//! `id` must equal the DID that was resolved ([`DidDocument::parse_for`] / [`DidDocument::check_id`]).

use serde_json::Value;

use crate::crypto::{Curve, PublicKey};
use crate::syntax::{Did, Handle};

/// DID document errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DidDocError {
    /// Not a JSON object, or malformed JSON.
    #[error("DID document is not a JSON object")]
    NotAnObject,
    /// Missing or invalid `id`.
    #[error("DID document has no valid id")]
    InvalidId,
    /// The `id` differs from the DID that was resolved.
    #[error("DID document id {found} does not match {expected}")]
    IdMismatch {
        /// The DID that was resolved.
        expected: String,
        /// The `id` in the document.
        found: String,
    },
    /// No usable `#atproto` verification method.
    #[error("no valid #atproto signing key")]
    NoSigningKey,
    /// No usable `#atproto_pds` service.
    #[error("no valid #atproto_pds service endpoint")]
    NoPds,
}

/// A verification method entry (only string fields are kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationMethod {
    /// `id` (relative `#frag` or `did…#frag`).
    pub id: String,
    /// `type`.
    pub type_: String,
    /// `controller`.
    pub controller: String,
    /// `publicKeyMultibase`, if present.
    pub public_key_multibase: Option<String>,
}

/// A service entry with a string endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// `id` (relative `#frag` or `did…#frag`).
    pub id: String,
    /// `type`.
    pub type_: String,
    /// `serviceEndpoint` (non-string endpoints are skipped).
    pub endpoint: String,
}

/// A parsed DID document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DidDocument {
    /// The document subject.
    pub id: Did,
    /// `alsoKnownAs` (string entries only, in order).
    pub also_known_as: Vec<String>,
    /// `verificationMethod` entries.
    pub verification_methods: Vec<VerificationMethod>,
    /// `service` entries.
    pub services: Vec<Service>,
}

/// The atproto-relevant facts of a DID document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtprotoIdentity {
    /// The DID.
    pub did: Did,
    /// The claimed handle (unverified; must be checked bidirectionally).
    pub handle: Option<Handle>,
    /// The `#atproto` signing key.
    pub signing_key: Option<PublicKey>,
    /// The PDS endpoint (`scheme://host[:port]`).
    pub pds: Option<String>,
}

fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn array<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

impl DidDocument {
    /// Parses a DID document from JSON bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, DidDocError> {
        let v: Value = serde_json::from_slice(bytes).map_err(|_| DidDocError::NotAnObject)?;
        Self::from_json(&v)
    }

    /// Parses a DID document and requires `id == expected`.
    pub fn parse_for(bytes: &[u8], expected: &Did) -> Result<Self, DidDocError> {
        let doc = Self::parse(bytes)?;
        doc.check_id(expected)?;
        Ok(doc)
    }

    /// Parses a DID document from a JSON value.
    pub fn from_json(v: &Value) -> Result<Self, DidDocError> {
        if !v.is_object() {
            return Err(DidDocError::NotAnObject);
        }
        let id = str_field(v, "id")
            .and_then(|s| Did::parse(s).ok())
            .ok_or(DidDocError::InvalidId)?;
        let also_known_as = array(v, "alsoKnownAs")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let verification_methods = array(v, "verificationMethod")
            .iter()
            .filter_map(|m| {
                Some(VerificationMethod {
                    id: str_field(m, "id")?.to_owned(),
                    type_: str_field(m, "type")?.to_owned(),
                    controller: str_field(m, "controller")?.to_owned(),
                    public_key_multibase: str_field(m, "publicKeyMultibase").map(str::to_owned),
                })
            })
            .collect();
        let services = array(v, "service")
            .iter()
            .filter_map(|s| {
                Some(Service {
                    id: str_field(s, "id")?.to_owned(),
                    type_: str_field(s, "type")?.to_owned(),
                    endpoint: str_field(s, "serviceEndpoint")?.to_owned(),
                })
            })
            .collect();
        Ok(Self {
            id,
            also_known_as,
            verification_methods,
            services,
        })
    }

    /// Requires `id == expected`.
    pub fn check_id(&self, expected: &Did) -> Result<(), DidDocError> {
        if &self.id == expected {
            Ok(())
        } else {
            Err(DidDocError::IdMismatch {
                expected: expected.to_string(),
                found: self.id.to_string(),
            })
        }
    }

    fn id_matches(&self, id: &str, fragment: &str) -> bool {
        match id.strip_suffix(fragment) {
            Some("") => true,
            Some(prefix) => prefix == self.id.as_str(),
            None => false,
        }
    }

    /// The claimed handle: the first `at://<handle>` entry of `alsoKnownAs` with valid handle
    /// syntax (nothing else after the handle). Unverified.
    pub fn handle(&self) -> Option<Handle> {
        self.also_known_as
            .iter()
            .find_map(|aka| Handle::parse(aka.strip_prefix("at://")?).ok())
    }

    /// The first valid `#atproto` signing key: `id` `#atproto` or `<did>#atproto`, `controller`
    /// equal to the DID, and either `type: Multikey` or one of the legacy types
    /// `EcdsaSecp256k1VerificationKey2019` / `EcdsaSecp256r1VerificationKey2019`.
    pub fn signing_key(&self) -> Result<PublicKey, DidDocError> {
        self.verification_methods
            .iter()
            .filter(|m| self.id_matches(&m.id, "#atproto") && m.controller == self.id.as_str())
            .find_map(|m| {
                let mb = m.public_key_multibase.as_deref()?;
                match m.type_.as_str() {
                    "Multikey" => PublicKey::from_multikey(mb).ok(),
                    "EcdsaSecp256k1VerificationKey2019" => {
                        PublicKey::from_legacy_multibase(Curve::Secp256k1, mb).ok()
                    }
                    "EcdsaSecp256r1VerificationKey2019" => {
                        PublicKey::from_legacy_multibase(Curve::P256, mb).ok()
                    }
                    _ => None,
                }
            })
            .ok_or(DidDocError::NoSigningKey)
    }

    /// The first valid `#atproto_pds` endpoint (`type: AtprotoPersonalDataServer`), normalized to
    /// `scheme://host[:port]` (see [`normalize_service_endpoint`]).
    pub fn pds_endpoint(&self) -> Result<String, DidDocError> {
        self.service_endpoint("#atproto_pds", "AtprotoPersonalDataServer")
            .ok_or(DidDocError::NoPds)
    }

    /// The first service with the given id fragment (e.g. `#atproto_pds`) and type whose endpoint
    /// is a valid origin URL, normalized.
    pub fn service_endpoint(&self, fragment: &str, type_: &str) -> Option<String> {
        self.services
            .iter()
            .filter(|s| self.id_matches(&s.id, fragment) && s.type_ == type_)
            .find_map(|s| normalize_service_endpoint(&s.endpoint))
    }

    /// The atproto identity facts (missing parts are `None`).
    pub fn atproto(&self) -> AtprotoIdentity {
        AtprotoIdentity {
            did: self.id.clone(),
            handle: self.handle(),
            signing_key: self.signing_key().ok(),
            pds: self.pds_endpoint().ok(),
        }
    }
}

/// Validates and normalizes a service endpoint origin: `http` or `https` scheme (lowercase), a
/// hostname (handle syntax), `localhost` or an IPv4 literal, an optional port 1–65535, an
/// optional single trailing `/`; no userinfo, path, query or fragment. Returns
/// `scheme://host[:port]` with the host lowercased.
pub fn normalize_service_endpoint(url: &str) -> Option<String> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else {
        ("http", url.strip_prefix("http://")?)
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '\\']) {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: u32 = p.parse().ok()?;
            if n == 0 || n > 65_535 {
                return None;
            }
            (h, Some(n))
        }
        None => (rest, None),
    };
    let host = host.to_ascii_lowercase();
    let host_ok = host == "localhost" || is_ipv4(&host) || Handle::parse(&host).is_ok();
    if !host_ok {
        return None;
    }
    Some(match port {
        Some(p) => format!("{scheme}://{host}:{p}"),
        None => format!("{scheme}://{host}"),
    })
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r##"{"@context":["https://www.w3.org/ns/did/v1"],"id":"did:plc:z72i7hdynmk6r22z27h6tvur","alsoKnownAs":["https://x","at://Bsky.App","at://other.test"],"verificationMethod":[{"id":"#atproto","type":"Multikey","controller":"did:plc:other","publicKeyMultibase":"zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA"},{"id":"did:plc:z72i7hdynmk6r22z27h6tvur#atproto","type":"Multikey","controller":"did:plc:z72i7hdynmk6r22z27h6tvur","publicKeyMultibase":"zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA"}],"service":[{"id":"#atproto_pds","type":"Other","serviceEndpoint":"https://a.example"},{"id":"#atproto_pds","type":"AtprotoPersonalDataServer","serviceEndpoint":{"x":1}},{"id":"#atproto_pds","type":"AtprotoPersonalDataServer","serviceEndpoint":"https://Puffball.us-east.host.bsky.network/"}]}"##;

    #[test]
    fn parses() {
        let did = Did::parse("did:plc:z72i7hdynmk6r22z27h6tvur").unwrap();
        let doc = DidDocument::parse_for(DOC.as_bytes(), &did).unwrap();
        assert_eq!(doc.handle().unwrap().as_str(), "bsky.app");
        assert_eq!(
            doc.signing_key().unwrap().multikey(),
            "zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA"
        );
        assert_eq!(
            doc.pds_endpoint().unwrap(),
            "https://puffball.us-east.host.bsky.network"
        );
        let a = doc.atproto();
        assert!(a.signing_key.is_some() && a.pds.is_some() && a.handle.is_some());
        let other = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(matches!(
            DidDocument::parse_for(DOC.as_bytes(), &other),
            Err(DidDocError::IdMismatch { .. })
        ));
    }

    #[test]
    fn missing_parts() {
        let doc = DidDocument::parse(br#"{"id":"did:web:example.com","service":"nope"}"#).unwrap();
        assert_eq!(doc.signing_key(), Err(DidDocError::NoSigningKey));
        assert_eq!(doc.pds_endpoint(), Err(DidDocError::NoPds));
        assert!(doc.handle().is_none());
        assert_eq!(DidDocument::parse(b"[]"), Err(DidDocError::NotAnObject));
        assert_eq!(
            DidDocument::parse(b"{\"id\":\"nope\"}"),
            Err(DidDocError::InvalidId)
        );
        assert_eq!(DidDocument::parse(b"{"), Err(DidDocError::NotAnObject));
    }

    #[test]
    fn legacy_key() {
        let doc = DidDocument::parse(br##"{"id":"did:web:example.com","verificationMethod":[{"id":"#atproto","type":"EcdsaSecp256k1VerificationKey2019","controller":"did:web:example.com","publicKeyMultibase":"zQYEBzXeuTM9UR3rfvNag6L3RNAs5pQZyYPsomTsgQhsxLdEgCrPTLgFna8yqCnxPpNT7DBk6Ym3dgPKNu86vt9GR"}]}"##).unwrap();
        assert_eq!(
            doc.signing_key().unwrap().multikey(),
            "zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF"
        );
    }

    #[test]
    fn endpoints() {
        assert_eq!(
            normalize_service_endpoint("http://localhost:2583").as_deref(),
            Some("http://localhost:2583")
        );
        assert_eq!(
            normalize_service_endpoint("https://10.0.0.1").as_deref(),
            Some("https://10.0.0.1")
        );
        for bad in [
            "ftp://x.com",
            "https://",
            "https://x.com/path",
            "https://u@x.com",
            "https://x.com?q",
            "https://x.com:0",
            "https://x.com:99999",
            "https://x.com:",
            "https://single",
            "https://x.com//",
            "https://01.0.0.1",
            "HTTPS://x.com",
        ] {
            assert!(normalize_service_endpoint(bad).is_none(), "{bad}");
        }
    }
}
