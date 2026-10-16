// SPDX-License-Identifier: MIT OR Apache-2.0
//! AT URIs (`/specs/at-uri-scheme`): a strict canonical record form ([`AtUri`]) for stored
//! references and a lenient form ([`LooseAtUri`]) for Lexicon `at-uri` strings and user input.

use super::{Did, Handle, Nsid, RecordKey, SyntaxError, string_type_impls};

/// Maximum AT URI length (8 KiB).
pub const MAX_AT_URI_LEN: usize = 8192;

const KIND: &str = "at-uri";
const SCHEME: &str = "at://";

/// The authority of an AT URI: a DID or a handle.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AtIdentifier {
    /// A DID authority.
    Did(Did),
    /// A handle authority (normalized lowercase).
    Handle(Handle),
}

impl AtIdentifier {
    /// Parses a DID (if it starts with `did:`) or a handle.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        if s.starts_with("did:") {
            Did::parse(s).map(Self::Did)
        } else {
            Handle::parse(s)
                .map(Self::Handle)
                .map_err(|_| SyntaxError::new("at-identifier", "not a DID or handle"))
        }
    }

    /// The identifier string (handles are lowercase).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Did(d) => d.as_str(),
            Self::Handle(h) => h.as_str(),
        }
    }

    /// The DID, if this is a DID authority.
    pub fn as_did(&self) -> Option<&Did> {
        match self {
            Self::Did(d) => Some(d),
            Self::Handle(_) => None,
        }
    }
}

string_type_impls!(AtIdentifier);

/// A strict, canonical record AT URI: exactly `at://<did>/<nsid>/<rkey>`.
///
/// This is the only form lessdugin stores. It rejects handle authorities, queries, fragments,
/// trailing or doubled slashes, partial paths and NSIDs whose authority is not lowercase. The DID
/// only needs generic syntax here; [`AtUri::check_lessdugin_canonical`] adds the lessdugin DID
/// policy (`did:plc` / host-only `did:web`, `data-model.md` §2.3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AtUri {
    s: String,
    did: Did,
    collection: Nsid,
    rkey: RecordKey,
}

impl AtUri {
    /// Parses a strict canonical record URI.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        if s.len() > MAX_AT_URI_LEN {
            return Err(SyntaxError::new(KIND, "too long"));
        }
        let rest = s
            .strip_prefix(SCHEME)
            .ok_or(SyntaxError::new(KIND, "must start with at://"))?;
        if rest.contains(['?', '#']) {
            return Err(SyntaxError::new(KIND, "query and fragment are not allowed"));
        }
        let mut parts = rest.split('/');
        let (Some(a), Some(c), Some(k), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(SyntaxError::new(
                KIND,
                "must be at://<did>/<collection>/<rkey>",
            ));
        };
        if !a.starts_with("did:") {
            return Err(SyntaxError::new(KIND, "authority must be a DID"));
        }
        let did = Did::parse(a)?;
        let collection = Nsid::parse(c)?;
        if !collection.is_authority_normalized() {
            return Err(SyntaxError::new(
                KIND,
                "collection authority must be lowercase",
            ));
        }
        let rkey = RecordKey::parse(k)?;
        Ok(Self {
            s: s.to_owned(),
            did,
            collection,
            rkey,
        })
    }

    /// Builds a URI from its parts. Fails only if the collection authority is not lowercase.
    pub fn new(did: Did, collection: Nsid, rkey: RecordKey) -> Result<Self, SyntaxError> {
        if !collection.is_authority_normalized() {
            return Err(SyntaxError::new(
                KIND,
                "collection authority must be lowercase",
            ));
        }
        let s = format!(
            "{SCHEME}{}/{}/{}",
            did.as_str(),
            collection.as_str(),
            rkey.as_str()
        );
        Ok(Self {
            s,
            did,
            collection,
            rkey,
        })
    }

    /// The URI string.
    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// The repository DID.
    pub fn did(&self) -> &Did {
        &self.did
    }

    /// The collection NSID.
    pub fn collection(&self) -> &Nsid {
        &self.collection
    }

    /// The record key.
    pub fn rkey(&self) -> &RecordKey {
        &self.rkey
    }

    /// The MST repo path `<collection>/<rkey>`.
    pub fn repo_path(&self) -> String {
        format!("{}/{}", self.collection.as_str(), self.rkey.as_str())
    }

    /// Applies the lessdugin reference policy on top of strict syntax: the DID must pass
    /// [`Did::check_lessdugin_policy`].
    pub fn check_lessdugin_canonical(&self) -> Result<(), SyntaxError> {
        self.did.check_lessdugin_policy()
    }
}

string_type_impls!(AtUri);

/// A lenient AT URI: `at://<did|handle>[/<nsid>[/<rkey>]]` (the Lexicon `at-uri` format).
///
/// [`LooseAtUri::parse`] implements the restricted Lexicon syntax exactly (no query, fragment or
/// trailing slash; handle authorities are lowercased). [`LooseAtUri::parse_user_input`]
/// additionally tolerates surrounding whitespace, a missing or upper-case `at://` scheme, a leading
/// `@` before a handle and one trailing slash. `as_str()` is always the normalized form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LooseAtUri {
    s: String,
    authority: AtIdentifier,
    collection: Option<Nsid>,
    rkey: Option<RecordKey>,
}

impl LooseAtUri {
    /// Parses a Lexicon `at-uri` string.
    pub fn parse(s: &str) -> Result<Self, SyntaxError> {
        if s.len() > MAX_AT_URI_LEN {
            return Err(SyntaxError::new(KIND, "too long"));
        }
        let rest = s
            .strip_prefix(SCHEME)
            .ok_or(SyntaxError::new(KIND, "must start with at://"))?;
        Self::from_path(rest)
    }

    /// Parses user input leniently (see the type docs), normalizing the result.
    pub fn parse_user_input(s: &str) -> Result<Self, SyntaxError> {
        let s = s.trim();
        if s.len() > MAX_AT_URI_LEN {
            return Err(SyntaxError::new(KIND, "too long"));
        }
        let rest = match s.get(..SCHEME.len()) {
            Some(p) if p.eq_ignore_ascii_case(SCHEME) => &s[SCHEME.len()..],
            _ => s,
        };
        let rest = rest.strip_prefix('@').unwrap_or(rest);
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        Self::from_path(rest)
    }

    fn from_path(rest: &str) -> Result<Self, SyntaxError> {
        if rest.contains(['?', '#']) {
            return Err(SyntaxError::new(KIND, "query and fragment are not allowed"));
        }
        let mut parts = rest.split('/');
        let authority = AtIdentifier::parse(parts.next().unwrap_or(""))?;
        let collection = parts.next().map(Nsid::parse).transpose()?;
        let rkey = parts.next().map(RecordKey::parse).transpose()?;
        if parts.next().is_some() {
            return Err(SyntaxError::new(KIND, "too many path segments"));
        }
        let mut s = format!("{SCHEME}{}", authority.as_str());
        if let Some(c) = &collection {
            s.push('/');
            s.push_str(c.as_str());
        }
        if let Some(k) = &rkey {
            s.push('/');
            s.push_str(k.as_str());
        }
        Ok(Self {
            s,
            authority,
            collection,
            rkey,
        })
    }

    /// The normalized URI string.
    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// The authority.
    pub fn authority(&self) -> &AtIdentifier {
        &self.authority
    }

    /// The collection, if present.
    pub fn collection(&self) -> Option<&Nsid> {
        self.collection.as_ref()
    }

    /// The record key, if present.
    pub fn rkey(&self) -> Option<&RecordKey> {
        self.rkey.as_ref()
    }

    /// Converts to a strict [`AtUri`] if the authority is a DID and both path parts are present.
    pub fn to_strict(&self) -> Option<AtUri> {
        let did = self.authority.as_did()?.clone();
        AtUri::new(did, self.collection.clone()?, self.rkey.clone()?).ok()
    }

    /// Replaces a handle authority with a resolved DID (the caller resolves and verifies it).
    pub fn with_did(&self, did: Did) -> Self {
        let mut s = format!("{SCHEME}{}", did.as_str());
        if let Some(c) = &self.collection {
            s.push('/');
            s.push_str(c.as_str());
        }
        if let Some(k) = &self.rkey {
            s.push('/');
            s.push_str(k.as_str());
        }
        Self {
            s,
            authority: AtIdentifier::Did(did),
            collection: self.collection.clone(),
            rkey: self.rkey.clone(),
        }
    }
}

impl From<AtUri> for LooseAtUri {
    fn from(u: AtUri) -> Self {
        Self {
            s: u.s,
            authority: AtIdentifier::Did(u.did),
            collection: Some(u.collection),
            rkey: Some(u.rkey),
        }
    }
}

string_type_impls!(LooseAtUri);

#[cfg(test)]
mod tests {
    use super::*;

    const U: &str = "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2ze2c";

    #[test]
    fn strict() {
        let u = AtUri::parse(U).unwrap();
        assert_eq!(u.did().as_str(), "did:plc:ewvi7nxzyoun6zhxrhs64oiz");
        assert_eq!(u.collection().as_str(), "online.7778777.lessdugin.post");
        assert_eq!(u.rkey().as_str(), "uetjixxr3a33-3l2s5xxv2ze2c");
        assert_eq!(
            u.repo_path(),
            "online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2ze2c"
        );
        assert!(u.check_lessdugin_canonical().is_ok());
        assert_eq!(
            AtUri::new(u.did().clone(), u.collection().clone(), u.rkey().clone()).unwrap(),
            u
        );
        for bad in [
            "at://alice.test/app.bsky.feed.post/3l2s5xxv2ze2c",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/x/",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/x?y=1",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/x#frag",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz//app.bsky.feed.post/x",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/App.Bsky.feed.post/x",
            "AT://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/x",
            " at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/x",
        ] {
            assert!(AtUri::parse(bad).is_err(), "{bad}");
        }
        let web = AtUri::parse("at://did:web:example.com:8080/app.bsky.feed.post/x").unwrap();
        assert!(web.check_lessdugin_canonical().is_err());
    }

    #[test]
    fn lenient() {
        let u = LooseAtUri::parse("at://Alice.Test/app.bsky.feed.post").unwrap();
        assert_eq!(u.as_str(), "at://alice.test/app.bsky.feed.post");
        assert!(u.rkey().is_none());
        assert!(u.to_strict().is_none());
        let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
        assert_eq!(
            u.with_did(did).as_str(),
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post"
        );
        let i = LooseAtUri::parse_user_input("  @Alice.TEST/ ").unwrap();
        assert_eq!(i.as_str(), "at://alice.test");
        let i = LooseAtUri::parse_user_input(
            "At://did:plc:ewvi7nxzyoun6zhxrhs64oiz/app.bsky.feed.post/abc/",
        )
        .unwrap();
        assert!(i.to_strict().is_some());
        assert!(LooseAtUri::parse_user_input("at://alice.test//").is_err());
        assert!(LooseAtUri::parse("at://alice.test/").is_err());
        assert_eq!(LooseAtUri::from(AtUri::parse(U).unwrap()).as_str(), U);
    }
}
