// SPDX-License-Identifier: MIT OR Apache-2.0
//! Decentralized identifiers (`/specs/did`).

use super::{SyntaxError, handle, string_type_impls};

/// Hard length limit for a DID in atproto.
pub const MAX_DID_LEN: usize = 2048;

const KIND: &str = "did";

/// A DID with valid generic atproto syntax (any method).
///
/// Syntax: `^did:[a-z]+:[a-zA-Z0-9._:%-]*[a-zA-Z0-9._-]$`, at most [`MAX_DID_LEN`] bytes. DIDs are
/// case-sensitive and never normalized. Method-specific checks are separate:
/// [`Did::is_valid_plc`], [`Did::web_host`], [`Did::check_lessdugin_policy`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Did(String);

/// The method of a [`Did`], as far as lessdugin cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DidMethod {
    /// `did:plc`
    Plc,
    /// `did:web`
    Web,
    /// Any other (syntactically valid but unsupported) method.
    Other,
}

impl Did {
    /// Parses a DID with generic atproto syntax rules.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        validate(s)?;
        Ok(Self(s.to_owned()))
    }

    /// The full DID string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The method name (`plc`, `web`, …).
    pub fn method(&self) -> &str {
        let rest = &self.0[4..];
        match rest.find(':') {
            Some(i) => &rest[..i],
            None => rest,
        }
    }

    /// The method-specific identifier (everything after `did:<method>:`).
    pub fn identifier(&self) -> &str {
        let m = self.method().len();
        &self.0[4 + m + 1..]
    }

    /// The method kind.
    pub fn method_kind(&self) -> DidMethod {
        match self.method() {
            "plc" => DidMethod::Plc,
            "web" => DidMethod::Web,
            _ => DidMethod::Other,
        }
    }

    /// `true` for a well-formed `did:plc`: `^did:plc:[a-z2-7]{24}$`.
    pub fn is_valid_plc(&self) -> bool {
        self.method() == "plc" && is_plc_identifier(self.identifier())
    }

    /// For `did:web`, the raw method-specific identifier (a hostname, possibly with a
    /// percent-encoded port). `None` for other methods.
    pub fn web_host(&self) -> Option<&str> {
        (self.method() == "web").then(|| self.identifier())
    }

    /// For a `did:web` that passes [`Did::check_lessdugin_policy`] and whose host may be fetched
    /// (see [`Did::web_document_url_with`], outside devnet mode), the DID document URL
    /// `https://<host>/.well-known/did.json`; `None` otherwise.
    pub fn web_document_url(&self) -> Option<String> {
        self.web_document_url_with(false)
    }

    /// Like [`Did::web_document_url`]; the host must also not be under a reserved or local-only
    /// TLD ([`super::DISALLOWED_TLDS`]: `.localhost`, `.local`, `.internal`, `.arpa`, …), which
    /// would make a native client fetch from loopback or the LAN (`threat-model.md` §5.6.2), and
    /// not `.test` unless `devnet` (`sync.md` §3.7). Such DIDs are still syntactically canonical
    /// (`data-model.md` §2.3); they just never resolve.
    pub fn web_document_url_with(&self, devnet: bool) -> Option<String> {
        let host = self.web_host()?;
        self.check_lessdugin_policy().ok()?;
        let tld = host.rsplit('.').next().unwrap_or("");
        if handle::DISALLOWED_TLDS.contains(&tld) || (tld == "test" && !devnet) {
            return None;
        }
        Some(format!("https://{host}/.well-known/did.json"))
    }

    /// lessdugin policy (`data-model.md` §2.3): only `did:plc:[a-z2-7]{24}` and
    /// `did:web:<lowercase hostname>` (no port, no path, no percent-encoding) are accepted.
    pub fn check_lessdugin_policy(&self) -> Result<(), SyntaxError> {
        match self.method_kind() {
            DidMethod::Plc => {
                if is_plc_identifier(self.identifier()) {
                    Ok(())
                } else {
                    Err(SyntaxError::new(
                        KIND,
                        "did:plc identifier must be 24 chars of [a-z2-7]",
                    ))
                }
            }
            DidMethod::Web => {
                let host = self.identifier();
                if host.bytes().any(|b| b == b'%' || b == b':') {
                    return Err(SyntaxError::new(
                        KIND,
                        "did:web with a port or path is not allowed",
                    ));
                }
                if host.bytes().any(|b| b.is_ascii_uppercase()) {
                    return Err(SyntaxError::new(KIND, "did:web hostname must be lowercase"));
                }
                handle::validate_hostname(host)
                    .map_err(|_| SyntaxError::new(KIND, "did:web host is not a valid hostname"))
            }
            DidMethod::Other => Err(SyntaxError::new(KIND, "unsupported DID method")),
        }
    }
}

fn is_plc_identifier(id: &str) -> bool {
    id.len() == 24
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

fn validate(s: &str) -> Result<(), SyntaxError> {
    let b = s.as_bytes();
    if b.len() > MAX_DID_LEN {
        return Err(SyntaxError::new(KIND, "too long"));
    }
    let rest = b
        .strip_prefix(b"did:")
        .ok_or(SyntaxError::new(KIND, "must start with 'did:'"))?;
    let colon = rest
        .iter()
        .position(|&c| c == b':')
        .ok_or(SyntaxError::new(KIND, "missing method separator"))?;
    let (method, id) = (&rest[..colon], &rest[colon + 1..]);
    if method.is_empty() || !method.iter().all(u8::is_ascii_lowercase) {
        return Err(SyntaxError::new(KIND, "method must be lowercase letters"));
    }
    let Some(&last) = id.last() else {
        return Err(SyntaxError::new(KIND, "empty identifier"));
    };
    if !id
        .iter()
        .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'%' | b'-'))
    {
        return Err(SyntaxError::new(KIND, "invalid character"));
    }
    if last == b':' || last == b'%' {
        return Err(SyntaxError::new(KIND, "must not end with ':' or '%'"));
    }
    Ok(())
}

string_type_impls!(Did);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts() {
        let d = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
        assert_eq!(d.method(), "plc");
        assert_eq!(d.identifier(), "ewvi7nxzyoun6zhxrhs64oiz");
        assert_eq!(d.method_kind(), DidMethod::Plc);
        assert!(d.is_valid_plc());
        assert!(d.check_lessdugin_policy().is_ok());
        let w = Did::parse("did:web:example.com").unwrap();
        assert_eq!(w.web_host(), Some("example.com"));
        assert_eq!(
            w.web_document_url().unwrap(),
            "https://example.com/.well-known/did.json"
        );
        assert!(d.web_document_url().is_none());
        assert!(
            Did::parse("did:web:localhost%3A2583")
                .unwrap()
                .web_document_url()
                .is_none()
        );
        assert!(w.check_lessdugin_policy().is_ok());
        // reserved and local-only TLDs never resolve; .test only in devnet
        for local in [
            "did:web:evil.localhost",
            "did:web:x.local",
            "did:web:pds.internal",
            "did:web:router.home.arpa",
            "did:web:x.onion",
            "did:web:x.example",
        ] {
            let d = Did::parse(local).unwrap();
            assert!(d.check_lessdugin_policy().is_ok(), "{local}");
            assert!(d.web_document_url().is_none(), "{local}");
            assert!(d.web_document_url_with(true).is_none(), "{local}");
        }
        let t = Did::parse("did:web:alice.test").unwrap();
        assert!(t.web_document_url().is_none());
        assert_eq!(
            t.web_document_url_with(true).as_deref(),
            Some("https://alice.test/.well-known/did.json")
        );
        let m = Did::parse("did:method::::val").unwrap();
        assert_eq!(m.method(), "method");
        assert_eq!(m.identifier(), ":::val");
    }

    #[test]
    fn policy() {
        for bad in [
            "did:plc:asdf123",
            "did:plc:EWVI7NXZYOUN6ZHXRHS64OIZ",
            "did:plc:ewvi7nxzyoun6zhxrhs64oi1",
            "did:web:localhost%3A1234",
            "did:web:example.com:path",
            "did:web:Example.com",
            "did:web:localhost",
            "did:web:127.0.0.1",
            "did:key:zQ3shZc2QzApp2oymGvQbzP8eKheVshBHbU4ZYjeXqwSKEn6N",
        ] {
            let d = Did::parse(bad).unwrap();
            assert!(d.check_lessdugin_policy().is_err(), "{bad}");
        }
    }

    #[test]
    fn invalid() {
        for bad in [
            "",
            "did:",
            "did:m",
            "did:m:",
            "did::x",
            "did:M:x",
            "did:m:x%",
            "did:m:x/y",
        ] {
            assert!(Did::parse(bad).is_err(), "{bad}");
        }
        let long = format!("did:m:{}", "a".repeat(MAX_DID_LEN));
        assert!(Did::parse(&long).is_err());
    }
}
