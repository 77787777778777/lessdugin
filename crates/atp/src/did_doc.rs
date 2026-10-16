// SPDX-License-Identifier: MIT OR Apache-2.0
//! DID documents (`/specs/did`, "DID Documents"; `sync.md` §3.2): the `#atproto` signing key, the
//! `#atproto_pds` service endpoint and the claimed handle (`alsoKnownAs`).
//!
//! Parsing keeps every `verificationMethod` and `service` entry that has a string `id` (missing
//! or non-string `type`, `controller` and endpoints are kept as empty / `None`, so they still
//! count when uniqueness is checked) and ignores unrelated content. The atproto facts follow
//! `sync.md` §3.2 exactly, so that clients agree on the same document:
//!
//! - **Key** ([`DidDocument::atproto_key`]): exactly one `verificationMethod` has id `#atproto` or
//!   `<did>#atproto` and type `Multikey`; its `publicKeyMultibase` must decode to `secp256k1-pub`
//!   or `p256-pub` and a 33-byte compressed point. `controller` is not checked (the reference
//!   implementations do not). Legacy `EcdsaSecp256k1VerificationKey2019` /
//!   `EcdsaSecp256r1VerificationKey2019` methods are accepted only when [`DocPolicy::legacy_keys`]
//!   is set (the devnet PLC still emits them). The result tells missing, ambiguous, unsupported
//!   and undecodable keys apart ([`KeyError`]); each leaves the key unavailable (`UnsupportedKey`).
//! - **PDS** ([`DidDocument::atproto_pds`]): exactly one `service` has id `#atproto_pds` or
//!   `<did>#atproto_pds` and type `AtprotoPersonalDataServer`; its endpoint must pass
//!   [`ServiceEndpoint::parse_with`]: `https` and a DNS name, not an IP literal, `localhost` or a
//!   local-only name, except in devnet mode (§3.7).
//! - **Handle** ([`DidDocument::claimed_handle`]): the first `alsoKnownAs` entry starting with
//!   `at://`, without that prefix, lowercased. Later entries are never used.
//!
//! The document `id` must equal the DID that was resolved ([`DidDocument::parse_for`] /
//! [`DidDocument::check_id`]).

use serde_json::Value;

use crate::crypto::{CryptoError, Curve, PublicKey};
use crate::syntax::{DISALLOWED_TLDS, Did, Handle};

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
    /// No usable `#atproto` signing key.
    #[error("{0}")]
    Key(#[from] KeyError),
    /// No usable service endpoint (`#atproto_pds` for [`DidDocument::pds_endpoint`]).
    #[error("{0}")]
    Service(#[from] ServiceError),
}

/// Why a document has no usable `#atproto` key. Every case makes the key unavailable (proofs fail
/// with `UnsupportedKey`, `sync.md` §3.2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// No `verificationMethod` has the id `#atproto`.
    #[error("no #atproto verification method")]
    Missing,
    /// More than one `#atproto` method of an accepted type.
    #[error("{0} #atproto verification methods (exactly one is allowed)")]
    Ambiguous(usize),
    /// `#atproto` methods exist, but none of an accepted type (`Multikey`, or a legacy type when
    /// [`DocPolicy::legacy_keys`] is set).
    #[error("unsupported #atproto key type {0:?}")]
    UnsupportedType(String),
    /// The method has no string `publicKeyMultibase`.
    #[error("the #atproto key has no publicKeyMultibase")]
    NoMultibase,
    /// The multibase does not decode to a supported, compressed public key.
    #[error("the #atproto key does not decode: {0}")]
    Invalid(CryptoError),
}

/// Why a document has no usable service endpoint.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// No service with that id and type.
    #[error("no such service")]
    Missing,
    /// More than one service with that id and type.
    #[error("{0} services with the same id and type (exactly one is allowed)")]
    Ambiguous(usize),
    /// The one matching service's endpoint is not acceptable.
    #[error("service endpoint: {0}")]
    Invalid(EndpointError),
}

/// Why a service endpoint URL is not acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    /// Not an origin URL: scheme `http(s)://`, a host, an optional port, an optional `/`, and no
    /// userinfo, path, query or fragment (or not a string at all).
    #[error("not an origin URL ({0})")]
    Syntax(&'static str),
    /// `http` outside devnet mode.
    #[error("the endpoint must use https")]
    Insecure,
    /// An IP literal outside devnet mode.
    #[error("the endpoint host must be a DNS name, not an IP literal")]
    IpLiteral,
    /// `localhost`, a reserved or local-only TLD (`.localhost`, `.local`, `.internal`, `.arpa`,
    /// …), or `.test`, outside devnet mode.
    #[error("the endpoint host is a local or reserved name")]
    LocalName,
}

/// Policy for the atproto facts of a document (`sync.md` §3.2, §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocPolicy {
    /// Devnet mode: `http` endpoints, IP literals, `localhost`, local-only names and `.test` are
    /// allowed.
    pub devnet: bool,
    /// Accept the legacy `EcdsaSecp256k1VerificationKey2019` / `EcdsaSecp256r1VerificationKey2019`
    /// encodings for `#atproto` (not allowed by `sync.md` §3.2; for devnet PLCs that emit them).
    pub legacy_keys: bool,
}

impl DocPolicy {
    /// The normative rules: https DNS-name endpoints, `Multikey` keys only.
    pub const STRICT: Self = Self {
        devnet: false,
        legacy_keys: false,
    };
    /// Devnet mode (`sync.md` §3.7), including legacy key encodings.
    pub const DEVNET: Self = Self {
        devnet: true,
        legacy_keys: true,
    };
}

/// A verification method entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationMethod {
    /// `id` (relative `#frag` or `did…#frag`).
    pub id: String,
    /// `type` (empty if absent or not a string).
    pub type_: String,
    /// `controller`, if a string (not checked by the atproto rules).
    pub controller: Option<String>,
    /// `publicKeyMultibase`, if a string.
    pub public_key_multibase: Option<String>,
}

/// A service entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// `id` (relative `#frag` or `did…#frag`).
    pub id: String,
    /// `type` (empty if absent or not a string).
    pub type_: String,
    /// `serviceEndpoint` (empty if absent or not a string, which no endpoint check accepts).
    pub endpoint: String,
}

/// A parsed DID document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DidDocument {
    /// The document subject.
    pub id: Did,
    /// `alsoKnownAs` (string entries only, in order).
    pub also_known_as: Vec<String>,
    /// `verificationMethod` entries with a string `id`.
    pub verification_methods: Vec<VerificationMethod>,
    /// `service` entries with a string `id`.
    pub services: Vec<Service>,
}

/// The atproto-relevant facts of a DID document under a [`DocPolicy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtprotoIdentity {
    /// The DID.
    pub did: Did,
    /// The claimed handle string (see [`DidDocument::claimed_handle`]; unvalidated).
    pub claimed_handle: Option<String>,
    /// The claimed handle, if it has valid handle syntax (unverified; must be checked
    /// bidirectionally).
    pub handle: Option<Handle>,
    /// The `#atproto` signing key.
    pub signing_key: Result<PublicKey, KeyError>,
    /// The `#atproto_pds` endpoint.
    pub pds: Result<ServiceEndpoint, ServiceError>,
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

const LEGACY_K256: &str = "EcdsaSecp256k1VerificationKey2019";
const LEGACY_P256: &str = "EcdsaSecp256r1VerificationKey2019";

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
        let owned = |m: &Value, k: &str| str_field(m, k).map(str::to_owned);
        let verification_methods = array(v, "verificationMethod")
            .iter()
            .filter_map(|m| {
                Some(VerificationMethod {
                    id: owned(m, "id")?,
                    type_: owned(m, "type").unwrap_or_default(),
                    controller: owned(m, "controller"),
                    public_key_multibase: owned(m, "publicKeyMultibase"),
                })
            })
            .collect();
        let services = array(v, "service")
            .iter()
            .filter_map(|s| {
                Some(Service {
                    id: owned(s, "id")?,
                    type_: owned(s, "type").unwrap_or_default(),
                    endpoint: owned(s, "serviceEndpoint").unwrap_or_default(),
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

    /// `true` if `id` is `fragment` or `<did>fragment`.
    fn id_matches(&self, id: &str, fragment: &str) -> bool {
        match id.strip_suffix(fragment) {
            Some("") => true,
            Some(prefix) => prefix == self.id.as_str(),
            None => false,
        }
    }

    /// The claimed handle: the first `alsoKnownAs` entry that starts with `at://`, without that
    /// prefix, lowercased. Unvalidated and unverified (`sync.md` §3.1 decides its state).
    pub fn claimed_handle(&self) -> Option<String> {
        self.also_known_as
            .iter()
            .find_map(|a| a.strip_prefix("at://"))
            .map(str::to_ascii_lowercase)
    }

    /// The claimed handle if it has valid handle syntax. Only the first `at://` entry counts: if
    /// it is invalid, there is no handle (a later entry is never used).
    pub fn handle(&self) -> Option<Handle> {
        Handle::parse(&self.claimed_handle()?).ok()
    }

    /// The `#atproto` signing key under `policy` (see the module docs).
    pub fn atproto_key(&self, policy: &DocPolicy) -> Result<PublicKey, KeyError> {
        let accepted = |t: &str| {
            t == "Multikey" || (policy.legacy_keys && (t == LEGACY_K256 || t == LEGACY_P256))
        };
        let with_id: Vec<&VerificationMethod> = self
            .verification_methods
            .iter()
            .filter(|m| self.id_matches(&m.id, "#atproto"))
            .collect();
        let found: Vec<&&VerificationMethod> =
            with_id.iter().filter(|m| accepted(&m.type_)).collect();
        let m = match found.as_slice() {
            [m] => m,
            [] => {
                return Err(match with_id.first() {
                    Some(m) => KeyError::UnsupportedType(m.type_.clone()),
                    None => KeyError::Missing,
                });
            }
            many => return Err(KeyError::Ambiguous(many.len())),
        };
        let mb = m
            .public_key_multibase
            .as_deref()
            .ok_or(KeyError::NoMultibase)?;
        match m.type_.as_str() {
            LEGACY_K256 => PublicKey::from_legacy_multibase(Curve::Secp256k1, mb),
            LEGACY_P256 => PublicKey::from_legacy_multibase(Curve::P256, mb),
            _ => PublicKey::from_multikey(mb),
        }
        .map_err(KeyError::Invalid)
    }

    /// The `#atproto_pds` endpoint under `policy` (see the module docs).
    pub fn atproto_pds(&self, policy: &DocPolicy) -> Result<ServiceEndpoint, ServiceError> {
        self.service("#atproto_pds", "AtprotoPersonalDataServer", policy)
    }

    /// The endpoint of the one service with id `fragment` (or `<did>fragment`) and type `type_`,
    /// checked under `policy`.
    pub fn service(
        &self,
        fragment: &str,
        type_: &str,
        policy: &DocPolicy,
    ) -> Result<ServiceEndpoint, ServiceError> {
        let found: Vec<&Service> = self
            .services
            .iter()
            .filter(|s| self.id_matches(&s.id, fragment) && s.type_ == type_)
            .collect();
        match found.as_slice() {
            [s] => ServiceEndpoint::parse_with(&s.endpoint, policy).map_err(ServiceError::Invalid),
            [] => Err(ServiceError::Missing),
            many => Err(ServiceError::Ambiguous(many.len())),
        }
    }

    /// [`DidDocument::atproto_key`] under [`DocPolicy::STRICT`].
    pub fn signing_key(&self) -> Result<PublicKey, DidDocError> {
        Ok(self.atproto_key(&DocPolicy::STRICT)?)
    }

    /// [`DidDocument::atproto_pds`] under [`DocPolicy::STRICT`], as `https://host[:port]`.
    pub fn pds_endpoint(&self) -> Result<String, DidDocError> {
        Ok(self.atproto_pds(&DocPolicy::STRICT)?.as_str().to_owned())
    }

    /// [`DidDocument::service`] under [`DocPolicy::STRICT`], as a string.
    pub fn service_endpoint(&self, fragment: &str, type_: &str) -> Option<String> {
        self.service(fragment, type_, &DocPolicy::STRICT)
            .ok()
            .map(|e| e.as_str().to_owned())
    }

    /// The atproto identity facts under [`DocPolicy::STRICT`].
    pub fn atproto(&self) -> AtprotoIdentity {
        self.atproto_with(&DocPolicy::STRICT)
    }

    /// The atproto identity facts under `policy`.
    pub fn atproto_with(&self, policy: &DocPolicy) -> AtprotoIdentity {
        AtprotoIdentity {
            did: self.id.clone(),
            claimed_handle: self.claimed_handle(),
            handle: self.handle(),
            signing_key: self.atproto_key(policy),
            pds: self.atproto_pds(policy),
        }
    }
}

/// What kind of host a [`ServiceEndpoint`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostKind {
    /// A DNS name (handle syntax: at least two labels, a TLD starting with a letter).
    Name,
    /// An IPv4 literal (dotted decimal, no leading zeros).
    Ipv4,
    /// `localhost`.
    Localhost,
}

/// A service endpoint origin: `scheme://host[:port]`, host lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceEndpoint {
    s: String,
    https: bool,
    host_len: usize,
    port: Option<u16>,
    kind: HostKind,
}

impl ServiceEndpoint {
    /// Parses an origin URL (syntax only, no policy): `http://` or `https://` (lowercase), a DNS
    /// name, `localhost` or an IPv4 literal, an optional port 1–65535, an optional single
    /// trailing `/`; no userinfo, path, query or fragment.
    pub fn parse(url: &str) -> Result<Self, EndpointError> {
        let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(EndpointError::Syntax("scheme must be http:// or https://"));
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.is_empty() {
            return Err(EndpointError::Syntax("no host"));
        }
        if rest.contains(['/', '?', '#', '@', '\\']) {
            return Err(EndpointError::Syntax(
                "userinfo, path, query and fragment are not allowed",
            ));
        }
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => {
                if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(EndpointError::Syntax("invalid port"));
                }
                match p.parse::<u16>() {
                    Ok(n) if n > 0 => (h, Some(n)),
                    _ => return Err(EndpointError::Syntax("invalid port")),
                }
            }
            None => (rest, None),
        };
        let host = host.to_ascii_lowercase();
        let kind = if host == "localhost" {
            HostKind::Localhost
        } else if is_ipv4(&host) {
            HostKind::Ipv4
        } else if Handle::parse(&host).is_ok() {
            HostKind::Name
        } else {
            return Err(EndpointError::Syntax("invalid host"));
        };
        let scheme = if https { "https" } else { "http" };
        let s = match port {
            Some(p) => format!("{scheme}://{host}:{p}"),
            None => format!("{scheme}://{host}"),
        };
        Ok(Self {
            s,
            https,
            host_len: host.len(),
            port,
            kind,
        })
    }

    /// Parses and checks an endpoint under `policy` ([`ServiceEndpoint::check`]).
    pub fn parse_with(url: &str, policy: &DocPolicy) -> Result<Self, EndpointError> {
        let e = Self::parse(url)?;
        e.check(policy)?;
        Ok(e)
    }

    /// Applies `sync.md` §3.2 / §3.7 and the browser SSRF subset of `threat-model.md` §5.6.2:
    /// outside devnet mode, the scheme must be `https` and the host a DNS name that is not
    /// `localhost`, not under a reserved or local-only TLD ([`DISALLOWED_TLDS`]) and not `.test`.
    pub fn check(&self, policy: &DocPolicy) -> Result<(), EndpointError> {
        if policy.devnet {
            return Ok(());
        }
        if !self.https {
            return Err(EndpointError::Insecure);
        }
        match self.kind {
            HostKind::Ipv4 => Err(EndpointError::IpLiteral),
            HostKind::Localhost => Err(EndpointError::LocalName),
            HostKind::Name if self.is_local_name() => Err(EndpointError::LocalName),
            HostKind::Name => Ok(()),
        }
    }

    /// `true` for a DNS name under a reserved or local-only TLD, or `.test`.
    pub fn is_local_name(&self) -> bool {
        self.kind == HostKind::Name && {
            let tld = self.host().rsplit('.').next().unwrap_or("");
            tld == "test" || DISALLOWED_TLDS.contains(&tld)
        }
    }

    /// `scheme://host[:port]`.
    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// `true` for `https`.
    pub fn is_https(&self) -> bool {
        self.https
    }

    /// The lowercased host.
    pub fn host(&self) -> &str {
        let start = if self.https { 8 } else { 7 };
        &self.s[start..start + self.host_len]
    }

    /// The explicit port, if any.
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// The kind of host.
    pub fn host_kind(&self) -> HostKind {
        self.kind
    }
}

impl core::fmt::Display for ServiceEndpoint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.s)
    }
}

/// Validates and normalizes a service endpoint origin, syntax only (see
/// [`ServiceEndpoint::parse`]); `http`, IP literals and `localhost` pass. Returns
/// `scheme://host[:port]` with the host lowercased. Prefer [`ServiceEndpoint::parse_with`], which
/// applies the `sync.md` §3.2 policy.
pub fn normalize_service_endpoint(url: &str) -> Option<String> {
    ServiceEndpoint::parse(url).ok().map(|e| e.s)
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
    use serde_json::json;

    const DID: &str = "did:plc:z72i7hdynmk6r22z27h6tvur";
    const KEY: &str = "zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA";
    /// The legacy (uncompressed, no multicodec) encoding of a secp256k1 key, from `/specs/did`.
    const LEGACY: &str = "zQYEBzXeuTM9UR3rfvNag6L3RNAs5pQZyYPsomTsgQhsxLdEgCrPTLgFna8yqCnxPpNT7DBk6Ym3dgPKNu86vt9GR";

    fn base() -> Value {
        json!({
            "@context": ["https://www.w3.org/ns/did/v1"],
            "id": DID,
            "alsoKnownAs": ["https://x", "at://Bsky.App", "at://other.test"],
            "verificationMethod": [
                {"id": "#other", "type": "Multikey", "controller": DID, "publicKeyMultibase": "zBAD"},
                {"id": format!("{DID}#atproto"), "type": "Multikey", "controller": DID, "publicKeyMultibase": KEY}
            ],
            "service": [
                {"id": "#atproto_pds", "type": "Other", "serviceEndpoint": "https://a.example"},
                {"id": "#other", "type": "AtprotoPersonalDataServer", "serviceEndpoint": "https://b.example.com"},
                {"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": "https://Puffball.us-east.host.bsky.network/"}
            ]
        })
    }

    fn doc(v: &Value) -> DidDocument {
        DidDocument::from_json(v).unwrap()
    }

    #[test]
    fn parses() {
        let did = Did::parse(DID).unwrap();
        let bytes = serde_json::to_vec(&base()).unwrap();
        let d = DidDocument::parse_for(&bytes, &did).unwrap();
        assert_eq!(d.handle().unwrap().as_str(), "bsky.app");
        assert_eq!(d.claimed_handle().as_deref(), Some("bsky.app"));
        assert_eq!(d.signing_key().unwrap().multikey(), KEY);
        assert_eq!(
            d.pds_endpoint().unwrap(),
            "https://puffball.us-east.host.bsky.network"
        );
        let a = d.atproto();
        assert!(a.signing_key.is_ok() && a.pds.is_ok() && a.handle.is_some());
        let other = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert!(matches!(
            DidDocument::parse_for(&bytes, &other),
            Err(DidDocError::IdMismatch { .. })
        ));
    }

    #[test]
    fn key_rules() {
        let key = |v: &Value, p: &DocPolicy| doc(v).atproto_key(p);
        let strict = DocPolicy::STRICT;
        // no controller (the reference implementations do not require one), or another one
        let mut v = base();
        v["verificationMethod"][1]
            .as_object_mut()
            .unwrap()
            .remove("controller");
        assert_eq!(key(&v, &strict).unwrap().multikey(), KEY);
        v["verificationMethod"][1]["controller"] = json!("did:plc:someoneelse");
        assert!(key(&v, &strict).is_ok());
        // exactly one: a broken first entry does not make way for a good second one
        let mut v = base();
        v["verificationMethod"] = json!([
            {"id": "#atproto", "type": "Multikey", "publicKeyMultibase": "zBROKEN"},
            {"id": "#atproto", "type": "Multikey", "publicKeyMultibase": KEY}
        ]);
        assert_eq!(key(&v, &strict), Err(KeyError::Ambiguous(2)));
        v["verificationMethod"][0]["id"] = json!(format!("{DID}#atproto"));
        assert_eq!(key(&v, &strict), Err(KeyError::Ambiguous(2)));
        // missing, unsupported type, no multibase, undecodable
        let mut v = base();
        v["verificationMethod"] = json!([]);
        assert_eq!(key(&v, &strict), Err(KeyError::Missing));
        assert_eq!(
            doc(&v).signing_key(),
            Err(DidDocError::Key(KeyError::Missing))
        );
        v["verificationMethod"] = json!([{"id": "#atproto", "type": "JsonWebKey2020"}]);
        assert_eq!(
            key(&v, &strict),
            Err(KeyError::UnsupportedType("JsonWebKey2020".into()))
        );
        v["verificationMethod"] = json!([{"id": "#atproto"}]);
        assert_eq!(key(&v, &strict), Err(KeyError::UnsupportedType(String::new())));
        v["verificationMethod"] = json!([{"id": "#atproto", "type": "Multikey"}]);
        assert_eq!(key(&v, &strict), Err(KeyError::NoMultibase));
        v["verificationMethod"] =
            json!([{"id": "#atproto", "type": "Multikey", "publicKeyMultibase": "zBROKEN"}]);
        assert!(matches!(key(&v, &strict), Err(KeyError::Invalid(_))));
        // another DID's #atproto does not count
        v["verificationMethod"] = json!([{"id": "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa#atproto", "type": "Multikey", "publicKeyMultibase": KEY}]);
        assert_eq!(key(&v, &strict), Err(KeyError::Missing));
    }

    #[test]
    fn legacy_and_uncompressed_keys() {
        let v = json!({"id": "did:web:example.com", "verificationMethod": [
            {"id": "#atproto", "type": "EcdsaSecp256k1VerificationKey2019", "controller": "did:web:example.com", "publicKeyMultibase": LEGACY}
        ]});
        let d = doc(&v);
        assert_eq!(
            d.atproto_key(&DocPolicy::STRICT),
            Err(KeyError::UnsupportedType(
                "EcdsaSecp256k1VerificationKey2019".into()
            ))
        );
        assert!(d.signing_key().is_err());
        let legacy = DocPolicy {
            legacy_keys: true,
            ..DocPolicy::STRICT
        };
        assert_eq!(
            d.atproto_key(&legacy).unwrap().multikey(),
            "zQ3shXjHeiBuRCKmM36cuYnm7YEMzhGnCmCyW92sRJ9pribSF"
        );
        assert!(d.atproto_key(&DocPolicy::DEVNET).is_ok());
        // a Multikey holding an uncompressed point is not accepted
        let pk = PublicKey::from_multikey(KEY).unwrap();
        let PublicKey::Secp256k1(vk) = &pk else {
            unreachable!()
        };
        let mut b = vec![0xe7, 0x01];
        b.extend_from_slice(vk.to_encoded_point(false).as_bytes());
        let mb = multibase::encode(multibase::Base::Base58Btc, b);
        let v = json!({"id": DID, "verificationMethod": [{"id": "#atproto", "type": "Multikey", "publicKeyMultibase": mb}]});
        assert_eq!(
            doc(&v).atproto_key(&DocPolicy::DEVNET),
            Err(KeyError::Invalid(CryptoError::NotCompressed))
        );
    }

    #[test]
    fn handle_rules() {
        let mut v = base();
        // only the first at:// entry counts, even if it is invalid
        v["alsoKnownAs"] = json!(["at://bad_handle", "at://good.example.com"]);
        let d = doc(&v);
        assert_eq!(d.claimed_handle().as_deref(), Some("bad_handle"));
        assert!(d.handle().is_none());
        v["alsoKnownAs"] = json!(["https://x", 1, "at://Good.Example.com"]);
        assert_eq!(doc(&v).handle().unwrap().as_str(), "good.example.com");
        v["alsoKnownAs"] = json!("at://not-an-array.com");
        assert!(doc(&v).claimed_handle().is_none());
    }

    #[test]
    fn pds_rules() {
        let pds = |eps: Value, p: &DocPolicy| {
            let mut v = base();
            v["service"] = eps;
            doc(&v).atproto_pds(p)
        };
        let svc = |ep: Value| json!([{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": ep}]);
        let strict = DocPolicy::STRICT;
        let ok = pds(svc(json!("https://pds.example.com:8443/")), &strict).unwrap();
        assert_eq!(ok.as_str(), "https://pds.example.com:8443");
        assert_eq!(ok.host(), "pds.example.com");
        assert_eq!(ok.port(), Some(8443));
        assert_eq!(ok.host_kind(), HostKind::Name);
        assert!(ok.is_https());
        // exactly one matching entry; a non-string endpoint still counts
        let two = json!([
            {"id": "#atproto_pds", "type": "AtprotoPersonalDataServer", "serviceEndpoint": {"x": 1}},
            {"id": format!("{DID}#atproto_pds"), "type": "AtprotoPersonalDataServer", "serviceEndpoint": "https://pds.example.com"}
        ]);
        assert_eq!(pds(two, &strict), Err(ServiceError::Ambiguous(2)));
        assert_eq!(
            pds(svc(json!({"x": 1})), &strict),
            Err(ServiceError::Invalid(EndpointError::Syntax(
                "scheme must be http:// or https://"
            )))
        );
        assert_eq!(pds(json!([]), &strict), Err(ServiceError::Missing));
        assert_eq!(
            doc(&json!({"id": DID})).pds_endpoint(),
            Err(DidDocError::Service(ServiceError::Missing))
        );
        // http, IP literals, localhost and local names only in devnet mode
        for (ep, err) in [
            ("http://pds.example.com", EndpointError::Insecure),
            ("https://10.0.0.1", EndpointError::IpLiteral),
            ("https://127.0.0.1:2583", EndpointError::IpLiteral),
            ("https://localhost", EndpointError::LocalName),
            ("https://pds.localhost", EndpointError::LocalName),
            ("https://nas.local", EndpointError::LocalName),
            ("https://pds.internal", EndpointError::LocalName),
            ("https://router.home.arpa", EndpointError::LocalName),
            ("https://pds.test", EndpointError::LocalName),
        ] {
            assert_eq!(
                pds(svc(json!(ep)), &strict),
                Err(ServiceError::Invalid(err)),
                "{ep}"
            );
            let dev = pds(svc(json!(ep)), &DocPolicy::DEVNET).unwrap();
            assert_eq!(dev.as_str(), ep, "{ep}");
        }
        assert_eq!(
            ServiceEndpoint::parse("http://127.0.0.1:2583").unwrap().host_kind(),
            HostKind::Ipv4
        );
        assert_eq!(
            ServiceEndpoint::parse("http://localhost").unwrap().host_kind(),
            HostKind::Localhost
        );
    }

    #[test]
    fn missing_parts() {
        let d = DidDocument::parse(br#"{"id":"did:web:example.com","service":"nope"}"#).unwrap();
        assert_eq!(d.signing_key(), Err(DidDocError::Key(KeyError::Missing)));
        assert_eq!(
            d.pds_endpoint(),
            Err(DidDocError::Service(ServiceError::Missing))
        );
        assert!(d.handle().is_none());
        assert_eq!(DidDocument::parse(b"[]"), Err(DidDocError::NotAnObject));
        assert_eq!(
            DidDocument::parse(b"{\"id\":\"nope\"}"),
            Err(DidDocError::InvalidId)
        );
        assert_eq!(DidDocument::parse(b"{"), Err(DidDocError::NotAnObject));
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
        assert_eq!(
            normalize_service_endpoint("https://X.com:080/").as_deref(),
            Some("https://x.com:80")
        );
        for bad in [
            "ftp://x.com",
            "https://",
            "https://x.com/path",
            "https://u@x.com",
            "https://x.com?q",
            "https://x.com#f",
            "https://x.com:0",
            "https://x.com:99999",
            "https://x.com:65536",
            "https://x.com:",
            "https://single",
            "https://x.com//",
            "https://01.0.0.1",
            "https://[::1]:443",
            "HTTPS://x.com",
            "",
        ] {
            assert!(normalize_service_endpoint(bad).is_none(), "{bad}");
            assert!(ServiceEndpoint::parse(bad).is_err(), "{bad}");
        }
    }
}
