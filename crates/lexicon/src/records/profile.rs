// SPDX-License-Identifier: MIT OR Apache-2.0
//! `profile` (§6.10).

use lessdugin_atp::syntax::{AtUri, Datetime};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Blob, Link, Md};
use crate::collection::nsid;

fn profile_type() -> String {
    nsid::PROFILE.to_owned()
}

/// `NS.profile` (rkey `self`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// `NS.profile` of this build.
    #[serde(rename = "$type", default = "profile_type")]
    pub r#type: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Avatar (`image/*`, ≤ 2,000,000 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Blob>,
    /// Biography (inline only, PF2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<Md>,
    /// Profile links (≤ 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<Link>>,
    /// Pinned post, comment or sequence at-uris (≤ 16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Vec<AtUri>>,
    /// Creation time (optional in a profile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Datetime>,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Datetime>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            r#type: profile_type(),
            display_name: None,
            avatar: None,
            bio: None,
            links: None,
            pinned: None,
            created_at: None,
            updated_at: None,
            extra: Map::new(),
        }
    }
}
