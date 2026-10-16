// SPDX-License-Identifier: MIT OR Apache-2.0
//! `modAction` (§6.7) and its action definitions.

use lessdugin_atp::syntax::{AtUri, Datetime};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::{Md, StrongRef};
use crate::collection::nsid;
use crate::values::{AccessLevel, ActionKind, Capability, PinScope, Scope, Tier};

fn mod_action_type() -> String {
    nsid::MOD_ACTION.to_owned()
}

/// `NS.modAction` (rkey `{fk}-{h16("modAction", scope_eff, subject, actionKey)}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModAction {
    /// `NS.modAction` of this build.
    #[serde(rename = "$type", default = "mod_action_type")]
    pub r#type: String,
    /// The forum at-uri.
    pub forum: AtUri,
    /// `kv{forum, author}`, default `forum`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// A did, a canonical at-uri or `"tag:" + slug` (MA1).
    pub subject: String,
    /// The subject version acted on (required for positive assertions, MA5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_cid: Option<String>,
    /// The action (open union).
    pub action: Action,
    /// Public reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The assertion lapses when `until <= now`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Datetime>,
    /// The former moderator's assertion this record re-issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopts: Option<StrongRef>,
    /// Creation time.
    pub created_at: Datetime,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ModAction {
    /// A forum-scope assertion.
    pub fn new(
        forum: AtUri,
        subject: impl Into<String>,
        action: Action,
        created_at: Datetime,
    ) -> Self {
        Self {
            r#type: mod_action_type(),
            forum,
            scope: None,
            subject: subject.into(),
            subject_cid: None,
            action,
            reason: None,
            until: None,
            adopts: None,
            created_at,
            updated_at: None,
            extra: Map::new(),
        }
    }

    /// `scope_eff`: `scope` if present, else `"forum"` (what the key hashes).
    pub fn scope_eff(&self) -> &str {
        Scope::scope_eff(self.scope.as_deref())
    }

    /// The scope, if known (`None` for an unknown scope: no effect, MA7).
    pub fn scope(&self) -> Option<Scope> {
        Scope::parse(self.scope_eff())
    }
}

macro_rules! action_struct {
    ($(#[$m:meta])* $name:ident, $ty:path, $tyfn:ident, $tyfn_s:tt { $($body:tt)* }) => {
        fn $tyfn() -> String {
            $ty.to_owned()
        }

        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            /// The action's `$type` (`NS.modAction#…` of this build).
            #[serde(rename = "$type", default = $tyfn_s)]
            pub r#type: String,
            $($body)*
            /// Unknown fields.
            #[serde(flatten)]
            pub extra: Map<String, Value>,
        }
    };
}

action_struct!(
    /// `modAction#hide`: hide (default) or keep an item; key `"hide"`.
    HideAction, nsid::MOD_ACTION_HIDE, hide_type, "hide_type" {
        /// `true` hides (default); `false` keeps (positive).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub hidden: Option<bool>,
        /// Marks the item as spam (ignored in author scope).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub spam: Option<bool>,
        /// Whether the reason is shown (default `true`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub show_reason: Option<bool>,
    }
);

action_struct!(
    /// `modAction#access`: ban or grant membership; key `"access"`.
    AccessAction, nsid::MOD_ACTION_ACCESS, access_type, "access_type" {
        /// `kv{banned, member}`.
        pub level: String,
    }
);

action_struct!(
    /// `modAction#restrict`: restrict capabilities (retroactive); key `"restrict"`.
    RestrictAction, nsid::MOD_ACTION_RESTRICT, restrict_type, "restrict_type" {
        /// `[kv{post, comment, vote, react, tag, wiki, sequence}]≤16`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub capabilities: Option<Vec<String>>,
        /// Discard all the user's votes in the forum.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub nullify_votes: Option<bool>,
    }
);

action_struct!(
    /// `modAction#tier`: set a post's tier; key `"tier"`.
    TierAction, nsid::MOD_ACTION_TIER, tier_type, "tier_type" {
        /// `kv{personal, frontpage, curated}`.
        pub tier: String,
        /// Moderator notice (inline only, MA12).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub notice: Option<Md>,
    }
);

action_struct!(
    /// `modAction#pin`: pin an item; key `"pin:" + scope`.
    PinAction, nsid::MOD_ACTION_PIN, pin_type, "pin_type" {
        /// `kv{forum, thread}`.
        pub scope: String,
        /// `true` pins (default, positive); `false` unpins.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub pinned: Option<bool>,
    }
);

action_struct!(
    /// `modAction#lock`: lock a post's thread; key `"lock"`.
    LockAction, nsid::MOD_ACTION_LOCK, lock_type, "lock_type" {
        /// `true` locks (default); `false` unlocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub locked: Option<bool>,
        /// Comment at-uris that stay visible (≤ 5000).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub allowed: Option<Vec<AtUri>>,
    }
);

action_struct!(
    /// `modAction#tag`: moderate a tag; key `"tag"`.
    TagAction, nsid::MOD_ACTION_TAG, tag_type, "tag_type" {
        /// Removes the tag from the forum.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub removed: Option<bool>,
        /// The wikiRevision chosen as the tag page head.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub head: Option<StrongRef>,
    }
);

impl HideAction {
    /// A hide (`hidden` absent = `true`).
    pub fn hide() -> Self {
        Self {
            r#type: hide_type(),
            hidden: None,
            spam: None,
            show_reason: None,
            extra: Map::new(),
        }
    }

    /// `hidden`, default `true`.
    pub fn hidden(&self) -> bool {
        self.hidden.unwrap_or(true)
    }
}

impl AccessAction {
    /// An access action with `level`.
    pub fn new(level: AccessLevel) -> Self {
        Self {
            r#type: access_type(),
            level: level.as_str().to_owned(),
            extra: Map::new(),
        }
    }

    /// The level, if known.
    pub fn level(&self) -> Option<AccessLevel> {
        AccessLevel::parse(&self.level)
    }
}

impl RestrictAction {
    /// A restriction of `capabilities`.
    pub fn new(capabilities: &[Capability], nullify_votes: bool) -> Self {
        Self {
            r#type: restrict_type(),
            capabilities: Some(capabilities.iter().map(|c| c.as_str().to_owned()).collect()),
            nullify_votes: nullify_votes.then_some(true),
            extra: Map::new(),
        }
    }
}

impl TierAction {
    /// A tier action.
    pub fn new(tier: Tier) -> Self {
        Self {
            r#type: tier_type(),
            tier: tier.as_str().to_owned(),
            notice: None,
            extra: Map::new(),
        }
    }

    /// The tier, if known.
    pub fn tier(&self) -> Option<Tier> {
        Tier::parse(&self.tier)
    }
}

impl PinAction {
    /// A pin (`pinned` absent = `true`).
    pub fn new(scope: PinScope) -> Self {
        Self {
            r#type: pin_type(),
            scope: scope.as_str().to_owned(),
            pinned: None,
            extra: Map::new(),
        }
    }

    /// The pin scope, if known.
    pub fn scope(&self) -> Option<PinScope> {
        PinScope::parse(&self.scope)
    }

    /// `pinned`, default `true`.
    pub fn pinned(&self) -> bool {
        self.pinned.unwrap_or(true)
    }
}

impl LockAction {
    /// A lock (`locked` absent = `true`) with an allow-list.
    pub fn new(allowed: Vec<AtUri>) -> Self {
        Self {
            r#type: lock_type(),
            locked: None,
            allowed: Some(allowed),
            extra: Map::new(),
        }
    }

    /// `locked`, default `true`.
    pub fn locked(&self) -> bool {
        self.locked.unwrap_or(true)
    }
}

impl TagAction {
    /// A tag action.
    pub fn new(removed: Option<bool>, head: Option<StrongRef>) -> Self {
        Self {
            r#type: tag_type(),
            removed,
            head,
            extra: Map::new(),
        }
    }
}

/// `modAction.action`: an open union of the seven action definitions. An unknown `$type`
/// (including another prefix's types) is kept verbatim as [`Action::Unknown`]: such a record has
/// no effect and is shown as "unknown action" (MA7).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `#hide`
    Hide(HideAction),
    /// `#access`
    Access(AccessAction),
    /// `#restrict`
    Restrict(RestrictAction),
    /// `#tier`
    Tier(TierAction),
    /// `#pin`
    Pin(PinAction),
    /// `#lock`
    Lock(LockAction),
    /// `#tag`
    Tag(TagAction),
    /// Any other member, verbatim.
    Unknown(Map<String, Value>),
}

impl Action {
    /// The action kind (`None` for an unknown action).
    pub fn kind(&self) -> Option<ActionKind> {
        Some(match self {
            Self::Hide(_) => ActionKind::Hide,
            Self::Access(_) => ActionKind::Access,
            Self::Restrict(_) => ActionKind::Restrict,
            Self::Tier(_) => ActionKind::Tier,
            Self::Pin(_) => ActionKind::Pin,
            Self::Lock(_) => ActionKind::Lock,
            Self::Tag(_) => ActionKind::Tag,
            Self::Unknown(_) => return None,
        })
    }

    /// `actionKey(action)` (§3.1): the def name, `"pin:" + scope`, or the `$type` of an unknown
    /// action.
    pub fn action_key(&self) -> String {
        match self {
            Self::Pin(p) => format!("pin:{}", p.scope),
            Self::Unknown(m) => m
                .get("$type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            other => other
                .kind()
                .map(|k| k.as_str().to_owned())
                .unwrap_or_default(),
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Hide(a) => a.serialize(s),
            Self::Access(a) => a.serialize(s),
            Self::Restrict(a) => a.serialize(s),
            Self::Tier(a) => a.serialize(s),
            Self::Pin(a) => a.serialize(s),
            Self::Lock(a) => a.serialize(s),
            Self::Tag(a) => a.serialize(s),
            Self::Unknown(m) => m.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m = Map::<String, Value>::deserialize(d)?;
        let kind = m
            .get("$type")
            .and_then(Value::as_str)
            .and_then(ActionKind::from_type);
        let Some(kind) = kind else {
            return Ok(Self::Unknown(m));
        };
        let v = Value::Object(m);
        let r = match kind {
            ActionKind::Hide => serde_json::from_value(v).map(Self::Hide),
            ActionKind::Access => serde_json::from_value(v).map(Self::Access),
            ActionKind::Restrict => serde_json::from_value(v).map(Self::Restrict),
            ActionKind::Tier => serde_json::from_value(v).map(Self::Tier),
            ActionKind::Pin => serde_json::from_value(v).map(Self::Pin),
            ActionKind::Lock => serde_json::from_value(v).map(Self::Lock),
            ActionKind::Tag => serde_json::from_value(v).map(Self::Tag),
        };
        r.map_err(D::Error::custom)
    }
}
