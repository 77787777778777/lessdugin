// SPDX-License-Identifier: AGPL-3.0-or-later
//! `parse_ref(input, RefContext)` (`engine-api.md` §3.1): at-uris, SPA URLs (`#/p/…`, `#/c/…`,
//! `#/f/…`, `#/s/…`, `#/u/…`, `#/at/…`, also inside a full URL), `handle/rkey` (a post in the
//! current forum), a DID and a handle (with or without `@`). Handle authorities are resolved
//! (both directions are not required here: the result names the DID the handle points to).
//! Listing indices (`3`, `c12`) are the CLI's to resolve before calling.

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, LooseAtUri};
use lessdugin_lexicon::collection::nsid;

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{Ref, RefContext};
use crate::host::Host;
use crate::store::schema::scope_of;

use super::{Engine, EngineError};

/// Percent-decodes one URL path segment (invalid escapes are kept as written).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The SPA route of `input` (`#/kind/rest`, possibly inside a full URL): `(kind, segments)`.
fn spa_route(input: &str) -> Option<(&str, Vec<String>)> {
    let route = &input[input.find("#/")? + 2..];
    let route = route.split(['?', '#']).next().unwrap_or("");
    let (kind, rest) = route.split_once('/').unwrap_or((route, ""));
    let parts = rest
        .split('/')
        .filter(|p| !p.is_empty())
        .map(percent_decode)
        .collect();
    Some((kind, parts))
}

fn bad(input: &str, why: &str) -> EngineError {
    EngineError::invalid_ref(format!("{input:?}: {why}"))
}

/// The kind of reference a record uri is.
fn ref_of(uri: AtUri) -> Ref {
    let uri_s = uri.to_string();
    match uri.collection().as_str() {
        nsid::FORUM => Ref::Forum { uri: uri_s },
        nsid::POST => Ref::Post { uri: uri_s },
        nsid::COMMENT => Ref::Comment { uri: uri_s },
        nsid::SEQUENCE => Ref::Sequence { uri: uri_s },
        _ => Ref::Record { uri: uri_s },
    }
}

impl<H: Host> Engine<H> {
    /// The DID of an authority (a DID, or a handle resolved now).
    async fn authority_did(&self, input: &str, a: &str) -> Result<Did, EngineError> {
        let a = a.strip_prefix('@').unwrap_or(a);
        if a.starts_with("did:") {
            return Did::parse(a).map_err(|e| bad(input, &e.to_string()));
        }
        let h = self
            .resolver
            .check_handle(a)
            .map_err(|e| bad(input, &format!("{a}: {e}")))?;
        Ok(self.resolver.resolve_handle_did(&h).await?.0)
    }

    /// The reference of `at://authority[/collection/rkey]`.
    async fn at_ref(&self, input: &str, at: &str) -> Result<Ref, EngineError> {
        let loose = LooseAtUri::parse_user_input(at).map_err(|e| bad(input, &e.to_string()))?;
        let did = match loose.authority() {
            AtIdentifier::Did(d) => d.clone(),
            AtIdentifier::Handle(h) => self.authority_did(input, h.as_str()).await?,
        };
        if loose.collection().is_none() && loose.rkey().is_none() {
            return Ok(Ref::Actor {
                did: did.to_string(),
            });
        }
        let strict = loose
            .with_did(did)
            .to_strict()
            .ok_or_else(|| bad(input, "a record uri is at://<did>/<collection>/<rkey>"))?;
        Ok(ref_of(strict))
    }

    /// `parse_ref(input, RefContext)` (`engine-api.md` §3.1): see the module docs. Fails with
    /// `invalid_ref` when the input is none of the accepted forms, and with the resolver's
    /// error when a handle does not resolve.
    pub async fn parse_ref(&self, input: &str, ctx: &RefContext) -> Result<Ref, EngineError> {
        let raw = input.trim();
        if let Some((kind, parts)) = spa_route(raw) {
            let record = |coll: &str, did: Did, rkey: &str| -> Result<Ref, EngineError> {
                let uri = AtUri::parse(&format!("at://{did}/{coll}/{rkey}"))
                    .map_err(|e| bad(input, &e.to_string()))?;
                Ok(ref_of(uri))
            };
            return match (kind, parts.as_slice()) {
                ("at", [_, ..]) => {
                    self.at_ref(input, &format!("at://{}", parts.join("/")))
                        .await
                }
                ("u", [a]) => Ok(Ref::Actor {
                    did: self.authority_did(input, a).await?.to_string(),
                }),
                ("f" | "p" | "c" | "s", [a, b]) => {
                    let coll = match kind {
                        "f" => nsid::FORUM,
                        "p" => nsid::POST,
                        "c" => nsid::COMMENT,
                        _ => nsid::SEQUENCE,
                    };
                    let did = self.authority_did(input, a).await?;
                    record(coll, did, b)
                }
                _ => Err(bad(input, "not a lessdugin link")),
            };
        }
        if raw.starts_with("at://") {
            return self.at_ref(input, raw).await;
        }
        let bare = raw.strip_prefix('@').unwrap_or(raw);
        if bare.starts_with("did:") {
            let did = Did::parse(bare).map_err(|e| bad(input, &e.to_string()))?;
            return Ok(Ref::Actor {
                did: did.to_string(),
            });
        }
        if let Some((a, rkey)) = bare.split_once('/') {
            if rkey.contains('/') || a.is_empty() || rkey.is_empty() {
                return Err(bad(input, "not a reference"));
            }
            // `handle/rkey`: a post of the current forum
            let forum = ctx
                .forum
                .as_deref()
                .ok_or_else(|| bad(input, "a handle/rkey reference needs a current forum"))?;
            let key = AtUri::parse(forum)
                .ok()
                .and_then(|u| ForumKey::of_uri(&u).ok())
                .ok_or_else(|| bad(forum, "the current forum is not a forum uri"))?;
            if scope_of(nsid::POST, rkey) != key.fk {
                return Err(bad(input, "the rkey is not a post of the current forum"));
            }
            let did = self.authority_did(input, a).await?;
            let uri = AtUri::parse(&format!("at://{did}/{}/{rkey}", nsid::POST))
                .map_err(|e| bad(input, &e.to_string()))?;
            return Ok(ref_of(uri));
        }
        if bare.contains('.') && !bare.contains(char::is_whitespace) {
            return Ok(Ref::Actor {
                did: self.authority_did(input, bare).await?.to_string(),
            });
        }
        Err(bad(input, "not a reference"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        assert_eq!(
            spa_route("https://x.example/#/p/alice.example.com/abc?x=1"),
            Some(("p", vec!["alice.example.com".to_owned(), "abc".to_owned()]))
        );
        assert_eq!(
            spa_route("#/u/did%3Aplc%3Aabc"),
            Some(("u", vec!["did:plc:abc".to_owned()]))
        );
        assert_eq!(spa_route("at://x"), None);
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%zz%41"), "%zzA");
    }
}
