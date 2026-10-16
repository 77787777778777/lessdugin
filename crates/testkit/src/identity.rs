// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fake identities: deterministic DIDs and keys, PLC-style DID documents, DoH JSON answers, and
//! helpers that install them as [`MemHost`] routes.

use k256::ecdsa::SigningKey;
use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::syntax::Did;
use serde_json::{Value, json};

use crate::memhost::MemHost;
use crate::repo::TestRepo;
use crate::resp;

/// The default PLC directory used by tests.
pub const PLC: &str = "https://plc.directory";
/// The default first DoH endpoint.
pub const DOH: &str = "https://cloudflare-dns.com/dns-query";
/// The default second DoH endpoint.
pub const DOH2: &str = "https://dns.google/resolve";

/// A deterministic `did:plc` for a seed.
pub fn fake_plc_did(seed: &str) -> Did {
    let digest = lessdugin_atp::cid::sha256(format!("lessdugin-testkit-did:{seed}").as_bytes());
    let b32 = data_encoding::BASE32_NOPAD
        .encode(&digest)
        .to_ascii_lowercase();
    Did::parse(&format!("did:plc:{}", &b32[..24])).expect("valid did:plc")
}

/// A deterministic secp256k1 key for a seed.
pub fn fake_key(seed: &str) -> SigningKey {
    let mut counter = 0u32;
    loop {
        let digest = lessdugin_atp::cid::sha256(
            format!("lessdugin-testkit-key:{seed}:{counter}").as_bytes(),
        );
        if let Ok(k) = SigningKey::from_slice(&digest) {
            return k;
        }
        counter += 1;
    }
}

/// The Multikey of a k256 signing key.
pub fn multikey(key: &SigningKey) -> String {
    PublicKey::Secp256k1(*key.verifying_key()).multikey()
}

/// A DID document in the shape the PLC directory serves.
pub fn did_doc(did: &str, handle: Option<&str>, multikey: &str, pds: &str) -> Value {
    json!({
        "@context": ["https://www.w3.org/ns/did/v1", "https://w3id.org/security/multikey/v1"],
        "id": did,
        "alsoKnownAs": handle.map(|h| vec![format!("at://{h}")]).unwrap_or_default(),
        "verificationMethod": [{
            "id": format!("{did}#atproto"),
            "type": "Multikey",
            "controller": did,
            "publicKeyMultibase": multikey,
        }],
        "service": [{
            "id": "#atproto_pds",
            "type": "AtprotoPersonalDataServer",
            "serviceEndpoint": pds,
        }],
    })
}

/// A DoH JSON answer with TXT records (each string is quoted as DoH providers do).
pub fn doh_txt_answer(qname: &str, txts: &[&str]) -> Value {
    let answers: Vec<Value> = txts
        .iter()
        .map(|t| json!({ "name": format!("{qname}."), "type": 16, "TTL": 300, "data": format!("\"{t}\"") }))
        .collect();
    json!({
        "Status": 0, "TC": false, "RD": true, "RA": true, "AD": false, "CD": false,
        "Question": [{ "name": format!("{qname}."), "type": 16 }],
        "Answer": answers,
    })
}

/// A DoH JSON NXDOMAIN answer.
pub fn doh_nxdomain(qname: &str) -> Value {
    json!({ "Status": 3, "Question": [{ "name": format!("{qname}."), "type": 16 }] })
}

/// The route prefix of a DoH query for `_atproto.<handle>` at `endpoint`.
pub fn doh_prefix(endpoint: &str, handle: &str) -> String {
    format!("{endpoint}?name=_atproto.{handle}&type=TXT")
}

/// The URL of a DID document (PLC or did:web).
pub fn doc_url(plc: &str, did: &Did) -> String {
    match did.web_document_url() {
        Some(u) => u,
        None => format!("{}/{}", plc.trim_end_matches('/'), did),
    }
}

/// Serves `doc` as the DID document of `did`.
pub fn install_doc(host: &MemHost, plc: &str, did: &Did, doc: &Value) {
    host.respond_get(&doc_url(plc, did), resp::ok_json(doc));
}

/// Serves DoH TXT records for `_atproto.<handle>` at `endpoint`.
pub fn install_txt(host: &MemHost, endpoint: &str, handle: &str, txts: &[&str]) {
    let qname = format!("_atproto.{handle}");
    host.respond_get(
        &doh_prefix(endpoint, handle),
        resp::ok_json(&doh_txt_answer(&qname, txts)),
    );
}

/// Serves NXDOMAIN for `_atproto.<handle>` at `endpoint`.
pub fn install_nxdomain(host: &MemHost, endpoint: &str, handle: &str) {
    let qname = format!("_atproto.{handle}");
    host.respond_get(
        &doh_prefix(endpoint, handle),
        resp::ok_json(&doh_nxdomain(&qname)),
    );
}

/// Serves `https://<handle>/.well-known/atproto-did`.
pub fn install_wellknown(host: &MemHost, handle: &str, body: &str) {
    host.respond_get(
        &format!("https://{handle}/.well-known/atproto-did"),
        resp::text(200, body),
    );
}

/// A fake account: DID, handle, signing key and PDS.
#[derive(Clone)]
pub struct FakeIdentity {
    /// The DID.
    pub did: Did,
    /// The handle (claimed in the document and published in DNS by [`FakeIdentity::install`]).
    pub handle: Option<String>,
    /// The repo signing key.
    pub key: SigningKey,
    /// The PDS endpoint.
    pub pds: String,
}

impl std::fmt::Debug for FakeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeIdentity")
            .field("did", &self.did)
            .field("handle", &self.handle)
            .field("pds", &self.pds)
            .finish_non_exhaustive()
    }
}

impl FakeIdentity {
    /// A `did:plc` identity derived from `seed`.
    pub fn plc(seed: &str, handle: &str, pds: &str) -> Self {
        Self {
            did: fake_plc_did(seed),
            handle: Some(handle.to_owned()),
            key: fake_key(seed),
            pds: pds.to_owned(),
        }
    }

    /// A `did:web:<host>` identity.
    pub fn web(host: &str, handle: &str, pds: &str) -> Self {
        Self {
            did: Did::parse(&format!("did:web:{host}")).expect("valid did:web"),
            handle: Some(handle.to_owned()),
            key: fake_key(host),
            pds: pds.to_owned(),
        }
    }

    /// The public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::Secp256k1(*self.key.verifying_key())
    }

    /// The Multikey.
    pub fn multikey(&self) -> String {
        multikey(&self.key)
    }

    /// The DID document.
    pub fn doc(&self) -> Value {
        did_doc(
            self.did.as_str(),
            self.handle.as_deref(),
            &self.multikey(),
            &self.pds,
        )
    }

    /// An empty repository signed by this identity's key.
    pub fn repo(&self) -> TestRepo {
        TestRepo::new(self.did.clone(), self.key.clone())
    }

    /// Serves the DID document (at `plc` for did:plc) and the handle's DNS TXT record at
    /// `doh` (`did=<did>`).
    pub fn install(&self, host: &MemHost, plc: &str, doh: &str) {
        install_doc(host, plc, &self.did, &self.doc());
        if let Some(h) = &self.handle {
            install_txt(host, doh, h, &[&format!("did={}", self.did)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        assert_eq!(fake_plc_did("alice"), fake_plc_did("alice"));
        assert_ne!(fake_plc_did("alice"), fake_plc_did("bob"));
        assert!(fake_plc_did("alice").is_valid_plc());
        assert_eq!(fake_key("a").to_bytes(), fake_key("a").to_bytes());
        let id = FakeIdentity::plc("alice", "alice.example.com", "https://pds.example.com");
        let doc = id.doc();
        let parsed = lessdugin_atp::did_doc::DidDocument::from_json(&doc).unwrap();
        assert_eq!(parsed.signing_key().unwrap(), id.public_key());
        assert_eq!(parsed.pds_endpoint().unwrap(), "https://pds.example.com");
        assert_eq!(parsed.handle().unwrap().as_str(), "alice.example.com");
        assert_eq!(
            doh_prefix(DOH, "alice.example.com"),
            "https://cloudflare-dns.com/dns-query?name=_atproto.alice.example.com&type=TXT"
        );
        let web = FakeIdentity::web("example.com", "example.com", "https://pds.example.com");
        assert_eq!(
            doc_url(PLC, &web.did),
            "https://example.com/.well-known/did.json"
        );
        assert_eq!(
            doc_url(PLC, &id.did),
            format!("https://plc.directory/{}", id.did)
        );
    }
}
