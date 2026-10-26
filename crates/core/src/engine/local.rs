// SPDX-License-Identifier: AGPL-3.0-or-later
//! `export_local()` / `import_local(Json)` (`engine-api.md` §3.6, M4) and the `export` command's
//! local-state half (`synthesis-v1.md` §6.10: `export [--mine|forum] [--format json|md]`).
//!
//! **What is local state.** `social.md` §16 and `synthesis-v1.md` §6.12 list the things that live
//! only on this device: drafts, read marks, subscriptions, followed forums and their aliases,
//! bookmarks, mutes, notification state and snapshots, tag filters, karma history, "continue
//! reading", the public-action acknowledgements, the karma cadence, the account list and the
//! outbox. They are the `loc` keys of the durable `state` store (`Host::state`), and they are
//! exportable as JSON (`threat-model.md` PRIV-10).
//!
//! **Scope of an export.** [`ExportScope::Mine`] (the default, `threat-model.md` PRIV-8) is the
//! active account's own state; [`ExportScope::Everything`] is every account's. The scope is a
//! predicate over the decoded key, so an export can never widen itself by accident.
//!
//! **What is never exported.** Session material: refresh tokens, DPoP keys and app passwords live
//! in the OS keyring, not in this store, but the account and login-binding rows do carry the PDS
//! and the auth kind. The outbox holds the *records of pending writes*, which are already the
//! user's own public data, and its nonces and base CIDs, which are not secrets. Nothing in this
//! module can reach a keyring.
//!
//! **Format.** One JSON object, versioned by [`EXPORT_VERSION`], holding an array of
//! `{key, value}` members where `key` is the base64url of the raw store key and `value` the
//! base64url of the raw value. Values are opaque to the format: a row written by a future version
//! round-trips unchanged, and an import never has to know every row's shape. The key's parts are
//! also recorded as readable tags (`["loc","follow","fk…"]`) so an operator can audit an export
//! without decoding base64.
//!
//! **Import.** [`Engine::import_local`] merges: it writes every member whose key passes the scope
//! predicate and leaves every other local key alone, so a partial export (or an older one) never
//! destroys state it does not mention. A key that fails to decode as a store key is skipped and
//! counted, rather than aborting the whole import.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::dto::{DTO_VERSION, DtoVersion};
use crate::engine::{Engine, EngineError};
use crate::host::{Dir, KeyRange, KvOp, KvStore};
use crate::store::key::{self, Part};

/// The export format version. Bumped only when the envelope changes shape; member values and
/// keys are opaque, so a bump is not needed for a row that grows a field.
pub const EXPORT_VERSION: u32 = 1;

/// The `loc` key prefix, encoded (`key!("loc")`): every durable local-state key starts with this.
fn loc_prefix() -> Vec<u8> {
    crate::key!("loc")
}

/// Which local state an export or import covers.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ExportScope {
    /// The active account's own state, plus the rows that belong to no account (`--mine`, the
    /// default: `threat-model.md` PRIV-8).
    #[default]
    Mine,
    /// Every account's state on this device.
    Everything,
}

/// The keys that carry no account: they are shared by every account on the device.
const ACCOUNT_FREE: [&str; 5] = [
    "active_account",
    "bookmark", // the `("loc","bookmark",uri)` rows are device-wide
    "mute",     // likewise
    "karmaCadence",
    "sub",
];

/// One exported member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportMember {
    /// The raw store key, base64url (no padding).
    pub key: String,
    /// The key's parts, decoded, for auditing (`["loc", "follow", "…"]`). Not used on import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    /// The raw row value, base64url (no padding).
    pub value: String,
}

/// `export_local()`'s result: the envelope `import_local` takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalExport {
    /// The DTO version (`dto/1`).
    pub dto: DtoVersion,
    /// [`EXPORT_VERSION`].
    pub version: u32,
    /// Which scope produced this export.
    pub scope: String,
    /// The account whose state this is, for `--mine`; absent for `Everything`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// When it was made (ms since the epoch), for humans.
    pub exported_ms: i64,
    /// The members, ordered by key so two exports of one store are byte-identical.
    pub members: Vec<ExportMember>,
}

/// `import_local`'s result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Members written.
    pub imported: usize,
    /// Members skipped because they were outside the scope.
    pub out_of_scope: usize,
    /// Members skipped because their key did not decode as a store key.
    pub undecodable: usize,
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// A `Part` as a string, when it is one.
fn part_str(p: Option<&Part>) -> Option<&str> {
    match p? {
        Part::Bytes(b) => std::str::from_utf8(b).ok(),
        _ => None,
    }
}

/// The account a `loc` key belongs to, if it names one. The `loc` keys put the account in a fixed
/// position per family; this reads the families the engine writes.
fn account_of(parts: &[Part]) -> Option<&str> {
    match part_str(parts.get(1))? {
        // ("loc","account",did), ("loc","as_pin",did), ("loc","seeded",did),
        // ("loc","ack",account,kind), ("loc","notif",account[,id]), ("loc","nbase",account,forum)
        "account" | "as_pin" | "seeded" | "ack" | "notif" | "nbase" => part_str(parts.get(2)),
        // ("loc","rejected",forum,did) is per forum, not per account
        _ => None,
    }
}

/// True when `parts` is inside `scope`.
fn in_scope(parts: &[Part], scope: &ExportScope, active: Option<&str>) -> bool {
    let ExportScope::Mine = scope else {
        return true;
    };
    let Some(tag) = part_str(parts.get(1)) else {
        // a `loc` key with no tag: keep it, it is not account-scoped
        return true;
    };
    if ACCOUNT_FREE.contains(&tag) {
        return true;
    }
    match account_of(parts) {
        Some(did) => Some(did) == active,
        // account-scoped by the engine's own knowledge but not one of the families above:
        // keep it rather than silently dropping state the rule does not understand.
        None => true,
    }
}

impl<H: crate::host::Host> Engine<H> {
    /// `export_local()` (`engine-api.md` §3.6): this device's local state as one JSON object.
    ///
    /// `--mine` (the default) covers the active account's state and the rows that belong to no
    /// account; [`ExportScope::Everything`] covers every account. Rows are emitted in key order,
    /// so exporting one unchanged store twice gives byte-identical output.
    pub async fn export_local(&self, scope: ExportScope) -> Result<LocalExport, EngineError> {
        let active = self
            .active_account()
            .await?
            .map(|a| a.did.clone())
            .filter(|d| !d.is_empty());
        let mut rows = Vec::new();
        let mut range = KeyRange::prefix(&loc_prefix());
        loop {
            let page = self
                .host()
                .state()
                .scan(range.clone(), Dir::Forward, 256)
                .await?;
            let full = page.len() >= 256;
            let last = page.last().map(|(k, _)| k.clone());
            for (k, v) in page {
                let Ok(parts) = key::decode(&k) else { continue };
                if !in_scope(&parts, &scope, active.as_deref()) {
                    continue;
                }
                rows.push(ExportMember {
                    key: b64().encode(&k),
                    parts: parts
                        .iter()
                        .map(|p| match p {
                            Part::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                            Part::Int(i) => i.to_string(),
                            Part::Max => "max".to_owned(),
                        })
                        .collect(),
                    value: b64().encode(&v),
                });
            }
            match last {
                Some(k) if full => range.start = std::ops::Bound::Excluded(k),
                _ => break,
            }
        }
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(LocalExport {
            dto: DtoVersion(DTO_VERSION),
            version: EXPORT_VERSION,
            scope: match scope {
                ExportScope::Mine => "mine".to_owned(),
                ExportScope::Everything => "everything".to_owned(),
            },
            account: match scope {
                ExportScope::Mine => active,
                ExportScope::Everything => None,
            },
            exported_ms: self.host().now_ms(),
            members: rows,
        })
    }

    /// `import_local(Json)` (`engine-api.md` §3.6): merge an export back into this device.
    ///
    /// Merging, not replacing: an imported member overwrites the same key, and every local key the
    /// import does not mention is left alone. A member outside `scope` is skipped, so importing a
    /// `--mine` export cannot write another account's state. Rows are written in one batch.
    pub async fn import_local(
        &self,
        export: &LocalExport,
        scope: ExportScope,
    ) -> Result<ImportReport, EngineError> {
        if export.version != EXPORT_VERSION {
            return Err(EngineError::blocked(format!(
                "this export is version {}, not {EXPORT_VERSION}",
                export.version
            ))
            .with_detail("kind", "exportVersion"));
        }
        let active = self
            .active_account()
            .await?
            .map(|a| a.did.clone())
            .filter(|d| !d.is_empty());
        let mut report = ImportReport::default();
        let mut ops: Vec<KvOp> = Vec::new();
        for m in &export.members {
            let (Ok(k), Ok(v)) = (b64().decode(&m.key), b64().decode(&m.value)) else {
                report.undecodable += 1;
                continue;
            };
            let Ok(parts) = key::decode(&k) else {
                report.undecodable += 1;
                continue;
            };
            if !in_scope(&parts, &scope, active.as_deref()) {
                report.out_of_scope += 1;
                continue;
            }
            // a duplicate key: the last one wins, as in any key-value store
            ops.retain(|op| !matches!(op, KvOp::Put(prev, _) if prev == &k));
            ops.push(KvOp::Put(k, v));
            report.imported += 1;
        }
        if !ops.is_empty() {
            self.host().state().write(ops).await?;
        }
        Ok(report)
    }
}
