// SPDX-License-Identifier: MIT OR Apache-2.0
//! `forum` (§6.1) and `membership` (§6.2).

use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Blob, Md, int};
use crate::collection::nsid;
use crate::values::{
    DefaultTier, Effective, KarmaAlgorithm, MembershipMode, ModerationPolicy, NewUsers,
    RankingAlgorithm, ReactPalette, Role, TagCreation, Voting,
};

fn forum_type() -> String {
    nsid::FORUM.to_owned()
}

fn membership_type() -> String {
    nsid::MEMBERSHIP.to_owned()
}

/// `NS.forum`: a forum declaration in the owner's repo (rkey = slug).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Forum {
    /// `NS.forum` of this build.
    #[serde(rename = "$type", default = "forum_type")]
    pub r#type: String,
    /// Display name.
    pub name: String,
    /// Plain-text tagline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Rules and introduction (MAY spill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<Md>,
    /// Forum icon (`image/*`, ≤ 2,000,000 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Blob>,
    /// Authorities other than the owner (read per FO2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderators: Option<Vec<Moderator>>,
    /// `kv{open, approval}`, default `approval`; writers write it explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership: Option<String>,
    /// `kv{personal, frontpage}`, default `personal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tier: Option<String>,
    /// Core tags offered as filters (slugs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub featured_tags: Option<Vec<String>>,
    /// Behaviour IDs and parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<Policy>,
    /// `mailto:`, `https:` or `at://` contact (`uri` format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_contact: Option<String>,
    /// Discovery services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<Discovery>,
    /// Creation time.
    pub created_at: Datetime,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Forum {
    /// The declaration `forum create` writes (§6.1, "Values written by forum create"):
    /// `membership`, `defaultTier` and every `policy` field explicit, with the preset of `mode`.
    pub fn create(name: impl Into<String>, created_at: Datetime, mode: MembershipMode) -> Self {
        Self {
            r#type: forum_type(),
            name: name.into(),
            description: None,
            about: None,
            icon: None,
            moderators: None,
            membership: Some(mode.as_str().to_owned()),
            default_tier: Some(DefaultTier::Personal.as_str().to_owned()),
            featured_tags: None,
            policy: Some(Policy::preset(mode)),
            report_contact: None,
            discovery: None,
            created_at,
            updated_at: None,
            extra: Map::new(),
        }
    }

    /// The membership mode (default and fallback `approval`).
    pub fn membership_mode(&self) -> Effective<MembershipMode> {
        MembershipMode::effective(self.membership.as_deref())
    }

    /// The default tier (default and fallback `personal`).
    pub fn default_tier(&self) -> Effective<DefaultTier> {
        DefaultTier::effective(self.default_tier.as_deref())
    }
}

/// `forum#moderator`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moderator {
    /// The authority's DID.
    pub did: Did,
    /// `kv{admin, moderator}`; an unknown role makes the entry ignored (FO2).
    pub role: String,
    /// Display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Moderator {
    /// The role, if known.
    pub fn role(&self) -> Option<Role> {
        Role::parse(&self.role)
    }
}

/// `forum#policy`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// `kv{"mod/1"}`, default `mod/1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation: Option<String>,
    /// `karma/1` parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub karma: Option<KarmaParams>,
    /// `magic/1` parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking: Option<RankParams>,
    /// `kv{karmaOnly, twoAxis, reacts}`, default `reacts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voting: Option<String>,
    /// `kv{"default/1"}`, default `default/1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_palette: Option<String>,
    /// Karma thresholds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gates: Option<Gates>,
    /// Display parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplayParams>,
    /// Default `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_moderation: Option<bool>,
    /// `kv{open, moderators}`, default `open`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_creation: Option<String>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Policy {
    /// The §6.1 preset for `mode`, with every field explicit.
    pub fn preset(mode: MembershipMode) -> Self {
        let open = mode == MembershipMode::Open;
        let pick = |o: i64, a: i64| Some(if open { o } else { a });
        Self {
            moderation: Some(ModerationPolicy::Mod1.as_str().to_owned()),
            karma: Some(KarmaParams {
                algorithm: Some(KarmaAlgorithm::Karma1.as_str().to_owned()),
                pair_cap: Some(25),
                establish: Some(1),
                establish_seeds: Some(2),
                extra: Map::new(),
            }),
            ranking: Some(RankParams {
                algorithm: Some(RankingAlgorithm::Magic1.as_str().to_owned()),
                offset_hours: Some(2),
                exponent_milli: Some(1150),
                curated_bonus: Some(10),
                frontpage_bonus: Some(0),
                cutoff_days: Some(90),
                extra: Map::new(),
            }),
            voting: Some(Voting::Reacts.as_str().to_owned()),
            react_palette: Some(ReactPalette::Default1.as_str().to_owned()),
            gates: Some(Gates {
                react_second: pick(5, 0),
                react_create: pick(10, 0),
                react_anti: pick(20, 5),
                author_mod_personal: pick(50, 10),
                author_mod_frontpage: pick(2000, 100),
                create_tag: pick(10, 0),
                wiki_edit: pick(5, 0),
                sequence: pick(1000, 0),
                extra: Map::new(),
            }),
            display: Some(DisplayParams {
                collapse_below: Some(-5),
                new_users: Some(NewUsers::Collapse.as_str().to_owned()),
                extra: Map::new(),
            }),
            author_moderation: Some(true),
            tag_creation: Some(TagCreation::Open.as_str().to_owned()),
            extra: Map::new(),
        }
    }

    /// `voting` with its default and fallback (§13.3).
    pub fn voting(&self) -> Effective<Voting> {
        Voting::effective(self.voting.as_deref())
    }

    /// `tagCreation` with its default and fallback.
    pub fn tag_creation(&self) -> Effective<TagCreation> {
        TagCreation::effective(self.tag_creation.as_deref())
    }

    /// `moderation` with its default and fallback.
    pub fn moderation(&self) -> Effective<ModerationPolicy> {
        ModerationPolicy::effective(self.moderation.as_deref())
    }

    /// `reactPalette` with its default and fallback.
    pub fn react_palette(&self) -> Effective<ReactPalette> {
        ReactPalette::effective(self.react_palette.as_deref())
    }

    /// `authorModeration` (default `true`).
    pub fn author_moderation(&self) -> bool {
        self.author_moderation.unwrap_or(true)
    }
}

/// `forum#karmaP` (`karma/1` parameters).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KarmaParams {
    /// `kv{"karma/1"}`, default `karma/1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
    /// 1..=1,000,000, default 25.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub pair_cap: Option<i64>,
    /// 0..=1,000,000, default 1.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub establish: Option<i64>,
    /// 1..=1,000, default 2.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub establish_seeds: Option<i64>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `forum#rankP` (`magic/1` parameters).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankParams {
    /// `kv{"magic/1"}`, default `magic/1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
    /// 1..=10,000, default 2.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub offset_hours: Option<i64>,
    /// 0..=10,000, default 1150.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub exponent_milli: Option<i64>,
    /// −1,000,000..=1,000,000, default 10.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub curated_bonus: Option<i64>,
    /// −1,000,000..=1,000,000, default 0.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub frontpage_bonus: Option<i64>,
    /// 1..=36,500, default 90.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub cutoff_days: Option<i64>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `forum#gates`: karma thresholds; an absent gate is the `karma/1` default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gates {
    /// Karma to add an existing react.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub react_second: Option<i64>,
    /// Karma to be first with a react.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub react_create: Option<i64>,
    /// Karma to react against.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub react_anti: Option<i64>,
    /// Karma to moderate one's own personal posts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub author_mod_personal: Option<i64>,
    /// Karma to moderate one's own frontpage posts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub author_mod_frontpage: Option<i64>,
    /// Karma to create a tag.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub create_tag: Option<i64>,
    /// Karma to edit a wiki page.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub wiki_edit: Option<i64>,
    /// Karma to have a sequence listed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub sequence: Option<i64>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `forum#display`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayParams {
    /// −1,000,000..=1,000,000, default −5.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "int::de_opt"
    )]
    pub collapse_below: Option<i64>,
    /// `kv{show, collapse, hide}`, default `collapse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_users: Option<String>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `forum#discovery`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Discovery {
    /// Indexers (≤ 8), used after one-time consent and only if usable (FO3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexers: Option<Vec<Endpoint>>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `forum#endpoint`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    /// Base URL (`uri` format).
    pub url: String,
    /// The service's DID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub did: Option<Did>,
    /// Operator note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `NS.membership` (rkey `{fk}`, in the member's repo).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Membership {
    /// `NS.membership` of this build.
    #[serde(rename = "$type", default = "membership_type")]
    pub r#type: String,
    /// The forum at-uri.
    pub forum: AtUri,
    /// Application text (public).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Creation time.
    pub created_at: Datetime,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Membership {
    /// A membership of `forum`.
    pub fn new(forum: AtUri, created_at: Datetime) -> Self {
        Self {
            r#type: membership_type(),
            forum,
            note: None,
            created_at,
            extra: Map::new(),
        }
    }
}
