// SPDX-License-Identifier: AGPL-3.0-or-later
//! `parse_ref(input, RefContext)` (`engine-api.md` §3.1): at-uris, SPA URLs (`#/p/…`, `#/c/…`,
//! `#/f/…`, `#/s/…`, `#/u/…`, `#/at/…`, also inside a full URL), `handle/rkey` (a post in the
//! current forum), a DID and a handle (with or without `@`). Handle authorities are resolved
//! (both directions are not required here: the result names the DID the handle points to).
//! Listing indices (`3`, `c12`) are the CLI's to resolve before calling.

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, LooseAtUri};
use lessdugin_lexicon::collection::nsid;

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{ModScope, Ref, RefContext, RkeyInput, ValidationReport};
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

impl<H: Host> Engine<H> {
    /// `validate_record(collection, json)` (`engine-api.md` §3.7): the validity of `json` as a
    /// record of `collection` (an NSID of this build's namespace, or its last segment such as
    /// `post`), by the rules of `data-model.md` §7, before it is written. The record key is the
    /// canonical one where the value determines it (membership, opinion, modAction), a fresh TID
    /// key where a writer picks the TID (post, comment, revision, wiki revision, sequence),
    /// `self` for a profile and the slug `draft` for a forum (whose key is the slug the writer
    /// chooses); the repo is the active account's. A value whose key cannot be derived fails G5.
    pub async fn validate_record(
        &self,
        collection: &str,
        value: &serde_json::Value,
    ) -> Result<ValidationReport, EngineError> {
        let ns = lessdugin_lexicon::ns::NS;
        let coll = if collection.contains('.') {
            collection.to_owned()
        } else {
            format!("{ns}.{collection}")
        };
        let did = self
            .me()
            .await?
            .unwrap_or_else(|| PLACEHOLDER_DID.to_owned());
        let tid = lessdugin_atp::tid::Tid::from_parts(
            u64::try_from(self.host.now_ms())
                .unwrap_or(0)
                .saturating_mul(1000),
            0,
        )
        .map_err(|e| EngineError::internal(e.to_string()))?
        .to_string();
        let rkey = draft_rkey(ns, &coll, value, &tid).unwrap_or_else(|| "x".to_owned());
        let uri = AtUri::parse(&format!("at://{did}/{coll}/{rkey}"))
            .or_else(|_| AtUri::parse(&format!("at://{did}/{coll}/x")))
            .map_err(|e| EngineError::invalid_input(format!("{collection}: {e}")))?;
        let v = lessdugin_lexicon::validity::check(&coll, &uri, value);
        let mut ids = v.rule_ids();
        ids.sort_unstable();
        ids.dedup();
        Ok(ValidationReport {
            valid: ids.is_empty(),
            errors: ids.iter().map(ToString::to_string).collect(),
        })
    }

    /// `derive_rkey(RkeyInput)` (`engine-api.md` §3.7): the canonical record key
    /// (`data-model.md` §3), by [`lessdugin_lexicon::keys::derive`]. `invalid_input` when the
    /// input names no valid key (a non-canonical uri, a root that is not a post keyed `FK-TID`,
    /// a malformed TID, an `actionKey` no action has).
    pub fn derive_rkey(&self, input: &RkeyInput) -> Result<String, EngineError> {
        use lessdugin_lexicon::keys::{self, KeyError, RkeyInput as K};
        let ns = lessdugin_lexicon::ns::NS;
        let derive = |k: &K<'_>| keys::derive(ns, k).map(|d| d.rkey);
        let r = match input {
            RkeyInput::Membership { forum } => derive(&K::Membership { forum }),
            RkeyInput::Post { forum, tid } => derive(&K::Post { forum, tid }),
            // `keys::derive` with the forum the input does not carry: the comment's fk is its
            // root's, and the root must be a post keyed `FK-TID` (a comment is not a root)
            RkeyInput::Comment { root, tid } => keys::split_uri(root).and_then(|(_, rc, rk)| {
                let (fk, t) = keys::split_fk_tid(rk)
                    .filter(|_| {
                        lessdugin_lexicon::Collection::from_nsid_in(ns, rc)
                            == Some(lessdugin_lexicon::Collection::Post)
                    })
                    .ok_or(KeyError::Undefined)?;
                if !keys::is_tid(tid) {
                    return Err(KeyError::InvalidTid);
                }
                Ok(format!("{fk}-{t}-{tid}"))
            }),
            RkeyInput::Opinion { forum, subject } => derive(&K::Opinion { forum, subject }),
            RkeyInput::ModAction {
                forum,
                scope,
                subject,
                action_key,
            } => action_with_key(ns, action_key)
                .ok_or(KeyError::Undefined)
                .and_then(|action| {
                    derive(&K::ModAction {
                        forum,
                        scope: Some(match scope {
                            ModScope::Forum => "forum",
                            ModScope::Author => "author",
                        }),
                        subject,
                        action: &action,
                    })
                }),
            RkeyInput::Revision { subject, tid } => derive(&K::Revision { subject, tid }),
        };
        r.map_err(|e| EngineError::invalid_input(format!("no record key: {e}")))
    }
}

/// An `action` whose `actionKey` (`data-model.md` §3.2) is `k`, if one has it: the def of the
/// namespace named `k` (`hide`), or `k` as its `$type` (`pin:forum`, a foreign or unknown
/// action). None for a key no action has, such as the full `$type` of a known def.
fn action_with_key(ns: &str, k: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    [format!("{ns}.modAction#{k}"), k.to_owned()]
        .into_iter()
        .map(|t| serde_json::Map::from_iter([("$type".to_owned(), t.into())]))
        .find(|a| lessdugin_lexicon::keys::action_key(ns, a).is_ok_and(|ak| ak == k))
}

/// The repo of a record `validate_record` checks when no account is active.
const PLACEHOLDER_DID: &str = "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa";

/// The key a record value of `coll` would be written under (see [`Engine::validate_record`]).
fn draft_rkey(ns: &str, coll: &str, v: &serde_json::Value, tid: &str) -> Option<String> {
    use lessdugin_lexicon::Collection;
    use lessdugin_lexicon::keys::{self, RkeyInput as K};
    let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
    let r = |k: &str| v.get(k)?.get("uri")?.as_str();
    let input = match Collection::from_nsid_in(ns, coll)? {
        Collection::Forum => return Some("draft".to_owned()),
        Collection::Profile => K::Profile,
        Collection::Membership => K::Membership { forum: s("forum")? },
        Collection::Post => K::Post {
            forum: s("forum")?,
            tid,
        },
        Collection::WikiRevision => K::WikiRevision {
            forum: s("forum")?,
            tid,
        },
        Collection::Sequence => K::Sequence {
            forum: s("forum")?,
            tid,
        },
        Collection::Comment => K::Comment {
            forum: s("forum")?,
            root: r("root")?,
            tid,
        },
        Collection::Revision => K::Revision {
            subject: r("subject")?,
            tid,
        },
        Collection::Opinion => K::Opinion {
            forum: s("forum")?,
            subject: r("subject")?,
        },
        Collection::ModAction => K::ModAction {
            forum: s("forum")?,
            scope: s("scope"),
            subject: s("subject")?,
            action: v.get("action")?.as_object()?,
        },
    };
    keys::derive(ns, &input).ok().map(|d| d.rkey)
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
