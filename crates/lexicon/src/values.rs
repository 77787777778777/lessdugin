// SPDX-License-Identifier: MIT OR Apache-2.0
//! Known values (`kv{…}` fields) and their unknown-value fallbacks (`data-model.md` §13.3).
//!
//! Lexicon `knownValues` are open: records keep any string, and the record structs store them as
//! `String`. These enums are the *reading* side: `parse` recognises a known value, and the
//! `effective` constructors apply the default (absent field) and the §13.3 fallback (unknown
//! value), reporting through [`Effective::fallback`] whether a banner or badge is due.

/// A value read with its default and fallback applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Effective<T> {
    /// The value to use.
    pub value: T,
    /// `true` if the field held an unknown value that was replaced (the UI shows the §13.3
    /// banner or badge).
    pub fallback: bool,
}

macro_rules! known_values {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $variant:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($(#[$vm])* $variant),+
        }

        impl $name {
            /// Every known value, in lexicon order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The known value `s`, if it is one.
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($s => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// The value as written in records.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $s),+
                }
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Reads a kv field: absent → `default` (no fallback); unknown → `fallback_value` with
/// `fallback = true`.
fn effective<T: Copy>(
    s: Option<&str>,
    parse: impl Fn(&str) -> Option<T>,
    default: T,
    fallback_value: T,
) -> Effective<T> {
    match s {
        None => Effective {
            value: default,
            fallback: false,
        },
        Some(s) => match parse(s) {
            Some(value) => Effective {
                value,
                fallback: false,
            },
            None => Effective {
                value: fallback_value,
                fallback: true,
            },
        },
    }
}

known_values!(
    /// `post.kind` (§6.3).
    PostKind {
        /// `article` (the default)
        Article = "article",
        /// `link`
        Link = "link",
        /// `question`
        Question = "question",
        /// `quickTake`
        QuickTake = "quickTake",
    }
);

impl PostKind {
    /// The effective kind (PO2): `kind` if known, otherwise `article`.
    pub fn effective(kind: Option<&str>) -> Effective<Self> {
        effective(kind, Self::parse, Self::Article, Self::Article)
    }
}

known_values!(
    /// `forum.membership` (§6.1).
    MembershipMode {
        /// `open`
        Open = "open",
        /// `approval` (the default and the fallback)
        Approval = "approval",
    }
);

impl MembershipMode {
    /// Default `approval`; unknown → `approval` (§13.3).
    pub fn effective(s: Option<&str>) -> Effective<Self> {
        effective(s, Self::parse, Self::Approval, Self::Approval)
    }
}

known_values!(
    /// `forum.defaultTier` (§6.1).
    DefaultTier {
        /// `personal` (the default and the fallback)
        Personal = "personal",
        /// `frontpage`
        Frontpage = "frontpage",
    }
);

impl DefaultTier {
    /// Default `personal`; unknown → `personal` (§13.3).
    pub fn effective(s: Option<&str>) -> Effective<Self> {
        effective(s, Self::parse, Self::Personal, Self::Personal)
    }
}

known_values!(
    /// `forum#moderator.role` (§6.1). An unknown role makes the entry ignored (FO2).
    Role {
        /// `admin` (rank 2)
        Admin = "admin",
        /// `moderator` (rank 1)
        Moderator = "moderator",
    }
);

known_values!(
    /// `forum#policy.moderation` behaviour ID.
    ModerationPolicy {
        /// `mod/1`
        Mod1 = "mod/1",
    }
);

known_values!(
    /// `forum#karmaP.algorithm` behaviour ID.
    KarmaAlgorithm {
        /// `karma/1`
        Karma1 = "karma/1",
    }
);

known_values!(
    /// `forum#rankP.algorithm` behaviour ID.
    RankingAlgorithm {
        /// `magic/1`
        Magic1 = "magic/1",
    }
);

known_values!(
    /// `forum#policy.reactPalette` behaviour ID.
    ReactPalette {
        /// `default/1`
        Default1 = "default/1",
    }
);

known_values!(
    /// `forum#policy.voting`.
    Voting {
        /// `karmaOnly`
        KarmaOnly = "karmaOnly",
        /// `twoAxis`
        TwoAxis = "twoAxis",
        /// `reacts` (the default)
        Reacts = "reacts",
    }
);

known_values!(
    /// `forum#policy.tagCreation`.
    TagCreation {
        /// `open` (the default)
        Open = "open",
        /// `moderators`
        Moderators = "moderators",
    }
);

known_values!(
    /// `forum#display.newUsers`.
    NewUsers {
        /// `show`
        Show = "show",
        /// `collapse` (the default)
        Collapse = "collapse",
        /// `hide`
        Hide = "hide",
    }
);

macro_rules! default_effective {
    ($t:ident, $d:ident) => {
        impl $t {
            /// The field's default when absent; an unknown value takes the default too, with
            /// the "rules newer than your client" banner (§13.3).
            pub fn effective(s: Option<&str>) -> Effective<Self> {
                effective(s, Self::parse, Self::$d, Self::$d)
            }
        }
    };
}

default_effective!(ModerationPolicy, Mod1);
default_effective!(KarmaAlgorithm, Karma1);
default_effective!(RankingAlgorithm, Magic1);
default_effective!(ReactPalette, Default1);
default_effective!(Voting, Reacts);
default_effective!(TagCreation, Open);
default_effective!(NewUsers, Collapse);

known_values!(
    /// `opinion.karma` and `#tagVote.value` (§6.6).
    VoteValue {
        /// `up`
        Up = "up",
        /// `down`
        Down = "down",
        /// `strongUp`
        StrongUp = "strongUp",
        /// `strongDown`
        StrongDown = "strongDown",
    }
);

known_values!(
    /// `opinion.agreement` (§6.6).
    Agreement {
        /// `agree`
        Agree = "agree",
        /// `disagree`
        Disagree = "disagree",
        /// `strongAgree`
        StrongAgree = "strongAgree",
        /// `strongDisagree`
        StrongDisagree = "strongDisagree",
    }
);

known_values!(
    /// `opinion#react.stance` (§6.6).
    Stance {
        /// `for` (the default)
        For = "for",
        /// `against`
        Against = "against",
    }
);

impl Stance {
    /// Absent → `for`; unknown → `None` (the entry is discarded, OP3).
    pub fn effective(s: Option<&str>) -> Option<Self> {
        match s {
            None => Some(Self::For),
            Some(s) => Self::parse(s),
        }
    }
}

known_values!(
    /// `revision.op` (§6.5). An unknown op makes the revision invalid (RV4).
    RevisionOp {
        /// `update`
        Update = "update",
        /// `delete` (modAction subjects only)
        Delete = "delete",
    }
);

known_values!(
    /// `modAction.scope` (§6.7). Absent means `forum`; unknown means no effect (MA7).
    Scope {
        /// `forum`
        Forum = "forum",
        /// `author`
        Author = "author",
    }
);

impl Scope {
    /// `scope_eff` as a string: `scope` if present, else `"forum"` (§6.7). This is what the key
    /// hashes, unknown values included.
    pub fn scope_eff(scope: Option<&str>) -> &str {
        scope.unwrap_or("forum")
    }
}

known_values!(
    /// `modAction#access.level`.
    AccessLevel {
        /// `banned`
        Banned = "banned",
        /// `member`
        Member = "member",
    }
);

known_values!(
    /// `modAction#tier.tier` (and the index `tier` filters).
    Tier {
        /// `personal`
        Personal = "personal",
        /// `frontpage`
        Frontpage = "frontpage",
        /// `curated`
        Curated = "curated",
    }
);

known_values!(
    /// `modAction#pin.scope`.
    PinScope {
        /// `forum`: a post or sequence pinned in the forum
        Forum = "forum",
        /// `thread`: a comment pinned in its thread
        Thread = "thread",
    }
);

known_values!(
    /// `modAction#restrict.capabilities` items. Unknown items are ignored (MA8).
    Capability {
        /// `post`
        Post = "post",
        /// `comment`
        Comment = "comment",
        /// `vote`
        Vote = "vote",
        /// `react`
        React = "react",
        /// `tag`
        Tag = "tag",
        /// `wiki`
        Wiki = "wiki",
        /// `sequence`
        Sequence = "sequence",
    }
);

known_values!(
    /// `markdown.profile` (§5.1). An unknown profile renders as `lfm/1` with a badge.
    MarkdownProfile {
        /// `lfm/1` (the default)
        Lfm1 = "lfm/1",
    }
);

default_effective!(MarkdownProfile, Lfm1);

known_values!(
    /// The modAction action definitions (§6.7), by def name.
    ActionKind {
        /// `#hide`
        Hide = "hide",
        /// `#access`
        Access = "access",
        /// `#restrict`
        Restrict = "restrict",
        /// `#tier`
        Tier = "tier",
        /// `#pin`
        Pin = "pin",
        /// `#lock`
        Lock = "lock",
        /// `#tag`
        Tag = "tag",
    }
);

impl ActionKind {
    /// The kind of an action `$type` under the prefix `ns` (`ns + ".modAction#" + name`);
    /// `None` for an unknown action (MA7), including another prefix's types.
    pub fn from_type_in(ns: &str, t: &str) -> Option<Self> {
        t.strip_prefix(ns)
            .and_then(|r| r.strip_prefix(".modAction#"))
            .and_then(Self::parse)
    }

    /// [`ActionKind::from_type_in`] for this build's prefix.
    pub fn from_type(t: &str) -> Option<Self> {
        Self::from_type_in(crate::ns::NS, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallbacks() {
        assert_eq!(
            PostKind::effective(None),
            Effective {
                value: PostKind::Article,
                fallback: false
            }
        );
        assert_eq!(
            PostKind::effective(Some("quickTake")).value,
            PostKind::QuickTake
        );
        let e = PostKind::effective(Some("poll"));
        assert_eq!(e.value, PostKind::Article);
        assert!(e.fallback);
        assert_eq!(
            MembershipMode::effective(None).value,
            MembershipMode::Approval
        );
        assert_eq!(
            MembershipMode::effective(Some("x")).value,
            MembershipMode::Approval
        );
        assert_eq!(
            DefaultTier::effective(Some("curated")).value,
            DefaultTier::Personal
        );
        assert_eq!(Voting::effective(None).value, Voting::Reacts);
        assert!(Voting::effective(Some("quadratic")).fallback);
        assert_eq!(NewUsers::effective(None).value, NewUsers::Collapse);
        assert_eq!(Stance::effective(None), Some(Stance::For));
        assert_eq!(Stance::effective(Some("meh")), None);
        assert_eq!(Scope::scope_eff(None), "forum");
        assert_eq!(Scope::scope_eff(Some("global")), "global");
        assert_eq!(
            MarkdownProfile::effective(Some("lfm/2")).value,
            MarkdownProfile::Lfm1
        );
        for k in ActionKind::ALL {
            assert_eq!(ActionKind::parse(k.as_str()), Some(*k));
        }
    }

    #[test]
    fn action_types() {
        let ns = crate::ns::NS;
        assert_eq!(
            ActionKind::from_type(&format!("{ns}.modAction#pin")),
            Some(ActionKind::Pin)
        );
        assert_eq!(
            ActionKind::from_type(&format!("{ns}.modAction#feature")),
            None
        );
        assert_eq!(ActionKind::from_type("com.example.mod#hide"), None);
        assert_eq!(
            ActionKind::from_type_in(
                "online.7778777.lessdugin.temp",
                "online.7778777.lessdugin.modAction#hide"
            ),
            None
        );
    }
}
