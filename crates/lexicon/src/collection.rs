// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ten record collections and this build's NSIDs (`data-model.md` §1.2, §1.3).
//!
//! Every NSID constant in [`nsid`] already carries this build's prefix ([`crate::ns::NS`]):
//! `online.7778777.lessdugin.temp.*` by default, `online.7778777.lessdugin.*` with the `final-ns`
//! feature. Lexicon files and the spec always use the final prefix; code never does.

use crate::ns::NS;

/// Defines a build-namespace NSID constant from its suffix after the prefix.
macro_rules! build_nsid {
    ($(#[$m:meta])* $name:ident = $suffix:literal) => {
        $(#[$m])*
        #[cfg(feature = "final-ns")]
        pub const $name: &str = concat!("online.7778777.lessdugin", $suffix);
        $(#[$m])*
        #[cfg(not(feature = "final-ns"))]
        pub const $name: &str = concat!("online.7778777.lessdugin.temp", $suffix);
    };
}

/// NSIDs and `$type` values of this build (prefix-mapped, §1.3), plus the reused atproto ones.
pub mod nsid {
    build_nsid!(
        /// `NS.forum`
        FORUM = ".forum"
    );
    build_nsid!(
        /// `NS.membership`
        MEMBERSHIP = ".membership"
    );
    build_nsid!(
        /// `NS.post`
        POST = ".post"
    );
    build_nsid!(
        /// `NS.comment`
        COMMENT = ".comment"
    );
    build_nsid!(
        /// `NS.revision`
        REVISION = ".revision"
    );
    build_nsid!(
        /// `NS.opinion`
        OPINION = ".opinion"
    );
    build_nsid!(
        /// `NS.modAction`
        MOD_ACTION = ".modAction"
    );
    build_nsid!(
        /// `NS.wikiRevision`
        WIKI_REVISION = ".wikiRevision"
    );
    build_nsid!(
        /// `NS.sequence`
        SEQUENCE = ".sequence"
    );
    build_nsid!(
        /// `NS.profile`
        PROFILE = ".profile"
    );
    build_nsid!(
        /// `NS.defs` (shared object definitions, §4)
        DEFS = ".defs"
    );
    build_nsid!(
        /// `NS.markdown`: the only member of every `md` union (§5)
        MARKDOWN = ".markdown"
    );
    build_nsid!(
        /// `NS.authFull` permission set (§10.1; never published under `.temp`)
        AUTH_FULL = ".authFull"
    );
    build_nsid!(
        /// `NS.authInteract` permission set (§10.2; never published under `.temp`)
        AUTH_INTERACT = ".authInteract"
    );
    build_nsid!(
        /// `$type` of the `#hide` action
        MOD_ACTION_HIDE = ".modAction#hide"
    );
    build_nsid!(
        /// `$type` of the `#access` action
        MOD_ACTION_ACCESS = ".modAction#access"
    );
    build_nsid!(
        /// `$type` of the `#restrict` action
        MOD_ACTION_RESTRICT = ".modAction#restrict"
    );
    build_nsid!(
        /// `$type` of the `#tier` action
        MOD_ACTION_TIER = ".modAction#tier"
    );
    build_nsid!(
        /// `$type` of the `#pin` action
        MOD_ACTION_PIN = ".modAction#pin"
    );
    build_nsid!(
        /// `$type` of the `#lock` action
        MOD_ACTION_LOCK = ".modAction#lock"
    );
    build_nsid!(
        /// `$type` of the `#tag` action
        MOD_ACTION_TAG = ".modAction#tag"
    );

    /// `com.atproto.repo.strongRef` (the `sref` of §2.1).
    pub const STRONG_REF: &str = "com.atproto.repo.strongRef";
    /// `com.atproto.label.defs#selfLabels`, the only member of `post.labels`.
    pub const SELF_LABELS: &str = "com.atproto.label.defs#selfLabels";
    /// The `$type` of a typed blob object.
    pub const BLOB: &str = "blob";
}

/// `true` if `t` names the def `reference` as a `$type` value: `nsid#name` matches itself, and a
/// `main` def matches both `nsid` and `nsid#main`.
pub fn type_matches(t: &str, reference: &str) -> bool {
    fn norm(s: &str) -> (&str, &str) {
        match s.split_once('#') {
            Some((n, d)) => (n, d),
            None => (s, "main"),
        }
    }
    norm(t) == norm(reference)
}

/// A lessdugin record collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Collection {
    /// `forum` (§6.1)
    Forum,
    /// `membership` (§6.2)
    Membership,
    /// `post` (§6.3)
    Post,
    /// `comment` (§6.4)
    Comment,
    /// `revision` (§6.5)
    Revision,
    /// `opinion` (§6.6)
    Opinion,
    /// `modAction` (§6.7)
    ModAction,
    /// `wikiRevision` (§6.8)
    WikiRevision,
    /// `sequence` (§6.9)
    Sequence,
    /// `profile` (§6.10)
    Profile,
}

impl Collection {
    /// Every collection, in the order of §1.2.
    pub const ALL: [Collection; 10] = [
        Collection::Forum,
        Collection::Membership,
        Collection::Post,
        Collection::Comment,
        Collection::Revision,
        Collection::Opinion,
        Collection::ModAction,
        Collection::WikiRevision,
        Collection::Sequence,
        Collection::Profile,
    ];

    /// The short name, the NSID suffix after `NS.` (`"post"`, `"modAction"`, …).
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Forum => "forum",
            Self::Membership => "membership",
            Self::Post => "post",
            Self::Comment => "comment",
            Self::Revision => "revision",
            Self::Opinion => "opinion",
            Self::ModAction => "modAction",
            Self::WikiRevision => "wikiRevision",
            Self::Sequence => "sequence",
            Self::Profile => "profile",
        }
    }

    /// Parses a short name.
    pub fn from_short_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.short_name() == s)
    }

    /// This build's collection NSID (also the record's `$type`).
    pub const fn nsid(self) -> &'static str {
        match self {
            Self::Forum => nsid::FORUM,
            Self::Membership => nsid::MEMBERSHIP,
            Self::Post => nsid::POST,
            Self::Comment => nsid::COMMENT,
            Self::Revision => nsid::REVISION,
            Self::Opinion => nsid::OPINION,
            Self::ModAction => nsid::MOD_ACTION,
            Self::WikiRevision => nsid::WIKI_REVISION,
            Self::Sequence => nsid::SEQUENCE,
            Self::Profile => nsid::PROFILE,
        }
    }

    /// The collection NSID under an explicit prefix `ns` (`ns + "." + short name`).
    pub fn nsid_in(self, ns: &str) -> String {
        format!("{ns}.{}", self.short_name())
    }

    /// The collection of a build-namespace NSID (`None` for other prefixes and other names).
    pub fn from_nsid(nsid: &str) -> Option<Self> {
        Self::from_nsid_in(NS, nsid)
    }

    /// The collection of an NSID under the prefix `ns`.
    pub fn from_nsid_in(ns: &str, nsid: &str) -> Option<Self> {
        nsid.strip_prefix(ns)
            .and_then(|rest| rest.strip_prefix('.'))
            .and_then(Self::from_short_name)
    }

    /// `true` for the forum-scoped collections: every collection except `forum` and `profile`.
    /// Their keys start with an fk (§3.2) and, except `revision`, they carry a `forum` field.
    pub const fn is_forum_scoped(self) -> bool {
        !matches!(self, Self::Forum | Self::Profile)
    }

    /// `true` if records of this collection carry a `forum` field (G9 applies).
    pub const fn has_forum_field(self) -> bool {
        !matches!(self, Self::Forum | Self::Profile | Self::Revision)
    }
}

impl core::fmt::Display for Collection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.short_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nsids_follow_the_build_prefix() {
        for c in Collection::ALL {
            assert_eq!(c.nsid(), format!("{NS}.{}", c.short_name()));
            assert_eq!(c.nsid_in(NS), c.nsid());
            assert_eq!(Collection::from_nsid(c.nsid()), Some(c));
            assert_eq!(Collection::from_short_name(c.short_name()), Some(c));
        }
        assert_eq!(nsid::MARKDOWN, format!("{NS}.markdown"));
        assert_eq!(nsid::DEFS, format!("{NS}.defs"));
        assert_eq!(nsid::MOD_ACTION_PIN, format!("{NS}.modAction#pin"));
        assert_eq!(nsid::AUTH_FULL, format!("{NS}.authFull"));
        assert_eq!(Collection::from_nsid("app.bsky.feed.post"), None);
        assert_eq!(Collection::from_nsid(&format!("{NS}post")), None);
        assert_eq!(Collection::from_nsid(&format!("{NS}.index")), None);
    }

    #[test]
    fn other_prefix_is_not_ours() {
        let other = if NS == crate::ns::NS_TEMP {
            crate::ns::NS_FINAL
        } else {
            crate::ns::NS_TEMP
        };
        for c in Collection::ALL {
            assert_eq!(Collection::from_nsid(&c.nsid_in(other)), None);
            assert_eq!(Collection::from_nsid_in(other, &c.nsid_in(other)), Some(c));
        }
    }

    #[test]
    fn type_matching() {
        assert!(type_matches("a.b.c", "a.b.c#main"));
        assert!(type_matches("a.b.c#main", "a.b.c"));
        assert!(type_matches("a.b.c#x", "a.b.c#x"));
        assert!(!type_matches("a.b.c#x", "a.b.c"));
        assert!(!type_matches("a.b.d", "a.b.c"));
    }
}
