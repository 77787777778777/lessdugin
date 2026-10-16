// SPDX-License-Identifier: MIT OR Apache-2.0
//! `revision` (§6.5).

use lessdugin_atp::syntax::Datetime;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::StrongRef;
use crate::collection::nsid;
use crate::values::RevisionOp;

fn revision_type() -> String {
    nsid::REVISION.to_owned()
}

/// `NS.revision` (rkey `{P}-{tid}`): a snapshot of a superseded or removed version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    /// `NS.revision` of this build.
    #[serde(rename = "$type", default = "revision_type")]
    pub r#type: String,
    /// The superseded version, in the same repo.
    pub subject: StrongRef,
    /// `kv{update, delete}`; an unknown op makes the revision invalid (RV4).
    pub op: String,
    /// The verbatim prior value (`unknown`); its CID must equal `subject.cid` (RV5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<Map<String, Value>>,
    /// Edit summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Creation time.
    pub created_at: Datetime,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Revision {
    /// A snapshot revision (`op` = update or delete) of `record`, whose CID is `subject.cid`.
    pub fn new(
        subject: StrongRef,
        op: RevisionOp,
        record: Map<String, Value>,
        created_at: Datetime,
    ) -> Self {
        Self {
            r#type: revision_type(),
            subject,
            op: op.as_str().to_owned(),
            record: Some(record),
            summary: None,
            created_at,
            extra: Map::new(),
        }
    }

    /// The op, if known.
    pub fn op(&self) -> Option<RevisionOp> {
        RevisionOp::parse(&self.op)
    }
}
