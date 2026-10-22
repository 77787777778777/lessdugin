// SPDX-License-Identifier: AGPL-3.0-or-later
//! Forum references (`sync.md` §4.1) and the canonical forum key.
//!
//! A reference is one of
//! - a forum at-uri `at://<did|handle>/NS.forum/<slug>`;
//! - an SPA URL `…#/f/<owner>/<slug>` (`<owner>` a handle or DID, percent-encoded or not;
//!   anything after the slug — a sub-route, a query — is ignored);
//! - `<handle|did>/<slug>` (an optional leading `@` is dropped).
//!
//! Parsing is pure. A handle owner is resolved to a DID by the caller (§3.1); then
//! [`ForumKey::new`] gives the canonical uri F and `fk(F)` (`data-model.md` §3.1).

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;

/// A parsed forum reference: the owner (not yet resolved) and the slug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForumRef {
    /// The owner, as written.
    pub owner: AtIdentifier,
    /// The declaration rkey.
    pub slug: String,
}

/// Why a forum reference was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a forum reference: {0}")]
pub struct ForumRefError(pub String);

fn err(input: &str, why: &str) -> ForumRefError {
    ForumRefError(format!("{input:?}: {why}"))
}

/// Percent-decodes `s` (UTF-8); `None` on a bad escape or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn owner_of(input: &str, s: &str) -> Result<AtIdentifier, ForumRefError> {
    let s = s.strip_prefix('@').unwrap_or(s);
    // handles are case-insensitive; DIDs are not touched
    let s = if s.starts_with("did:") {
        s.to_owned()
    } else {
        s.to_ascii_lowercase()
    };
    AtIdentifier::parse(&s).map_err(|e| err(input, &format!("bad owner {s:?}: {e}")))
}

fn slug_of(input: &str, s: &str) -> Result<String, ForumRefError> {
    if keys::is_slug(s) {
        Ok(s.to_owned())
    } else {
        Err(err(input, &format!("bad forum slug {s:?}")))
    }
}

/// Parses a forum reference (§4.1).
pub fn parse_forum_ref(input: &str) -> Result<ForumRef, ForumRefError> {
    let t = input.trim();
    if let Some(rest) = t.strip_prefix("at://") {
        let mut parts = rest.split('/');
        let (Some(owner), Some(coll), Some(slug), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(err(input, "expected at://<owner>/<forum collection>/<slug>"));
        };
        if coll != nsid::FORUM {
            return Err(err(input, &format!("{coll} is not {}", nsid::FORUM)));
        }
        return Ok(ForumRef {
            owner: owner_of(input, owner)?,
            slug: slug_of(input, slug)?,
        });
    }
    if let Some(i) = t.find("#/f/") {
        let rest = &t[i + 4..];
        let rest = rest.split(['?', '#']).next().unwrap_or_default();
        let mut parts = rest.split('/');
        let (Some(owner), Some(slug)) = (parts.next(), parts.next()) else {
            return Err(err(input, "expected …#/f/<owner>/<slug>"));
        };
        let owner = percent_decode(owner).ok_or_else(|| err(input, "bad percent-encoding"))?;
        let slug = percent_decode(slug).ok_or_else(|| err(input, "bad percent-encoding"))?;
        return Ok(ForumRef {
            owner: owner_of(input, &owner)?,
            slug: slug_of(input, &slug)?,
        });
    }
    match t.split_once('/') {
        Some((owner, slug)) if !owner.is_empty() && !slug.contains('/') => Ok(ForumRef {
            owner: owner_of(input, owner)?,
            slug: slug_of(input, slug)?,
        }),
        _ => Err(err(
            input,
            "expected an at-uri, an SPA URL …#/f/<owner>/<slug> or <handle|did>/<slug>",
        )),
    }
}

/// A forum's canonical identity: F, its owner, slug and `fk(F)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ForumKey {
    /// The canonical forum at-uri F.
    pub uri: AtUri,
    /// `did(F)`.
    pub owner: Did,
    /// The declaration rkey.
    pub slug: String,
    /// `fk(F)`.
    pub fk: String,
}

impl ForumKey {
    /// The key of forum `slug` owned by `owner`.
    pub fn new(owner: &Did, slug: &str) -> Result<Self, ForumRefError> {
        if !keys::is_slug(slug) {
            return Err(err(slug, "bad forum slug"));
        }
        let s = format!("at://{owner}/{}/{slug}", nsid::FORUM);
        let uri = AtUri::parse(&s).map_err(|e| err(&s, &e.to_string()))?;
        let fk = keys::fk(uri.as_str()).map_err(|e| err(&s, &e.to_string()))?;
        Ok(Self {
            uri,
            owner: owner.clone(),
            slug: slug.to_owned(),
            fk,
        })
    }

    /// The key of a canonical forum at-uri (a DID authority).
    pub fn of_uri(uri: &AtUri) -> Result<Self, ForumRefError> {
        if uri.collection().as_str() != nsid::FORUM {
            return Err(err(uri.as_str(), "not a forum uri"));
        }
        Self::new(uri.did(), uri.rkey().as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DID: &str = "did:plc:abcdefghijklmnopqrstuvwx";

    #[test]
    fn reference_forms() {
        let f = nsid::FORUM;
        let want = ForumRef {
            owner: AtIdentifier::parse(DID).unwrap(),
            slug: "rationality".into(),
        };
        for s in [
            format!("at://{DID}/{f}/rationality"),
            format!("https://lessdugin.example/#/f/{DID}/rationality"),
            format!("https://x/#/f/{}/rationality/p/3l2a?sort=new", DID.replace(':', "%3A")),
            format!("{DID}/rationality"),
            format!("  @{DID}/rationality "),
        ] {
            assert_eq!(parse_forum_ref(&s).unwrap(), want, "{s}");
        }
        let h = parse_forum_ref("Alice.Example.com/meta").unwrap();
        assert_eq!(h.owner.as_str(), "alice.example.com");
        assert_eq!(h.slug, "meta");
        let h = parse_forum_ref(&format!("at://alice.example.com/{f}/meta")).unwrap();
        assert!(matches!(h.owner, AtIdentifier::Handle(_)));
        for bad in [
            format!("at://{DID}/app.bsky.feed.post/meta"),
            format!("at://{DID}/{f}"),
            format!("{DID}/Not_A_Slug"),
            format!("{DID}/a/b"),
            "meta".to_owned(),
            "/meta".to_owned(),
            "https://x/#/f/%ZZ/meta".to_owned(),
        ] {
            assert!(parse_forum_ref(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn canonical_key() {
        let did = Did::parse(DID).unwrap();
        let k = ForumKey::new(&did, "rationality").unwrap();
        assert_eq!(
            k.uri.as_str(),
            format!("at://{DID}/{}/rationality", nsid::FORUM)
        );
        assert_eq!(k.fk, keys::fk(k.uri.as_str()).unwrap());
        assert_eq!(k.fk.len(), keys::FK_LEN);
        assert_eq!(ForumKey::of_uri(&k.uri).unwrap(), k);
        assert!(ForumKey::new(&did, "Bad").is_err());
    }
}
