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

/// The settings a client may change (synthesis-v1.md §1.4, §6.12: "accounts, sources,
/// verification, indexer consent, external images, media autoload hosts").
///
/// These are per device, not per account: they are how this copy of the app behaves, so they
/// live in the same `loc` state the rest of the local lists use, and they travel with
/// [`LocalExport`].
pub const SETTINGS: [&str; 5] = [
    // `direct` or `assisted` (`engine::Sources`).
    "sources",
    // `pds` or `proof` (`engine::VerifyMode`).
    "verify",
    // `off` or `on`: whether media loads at all (3.9's master switch).
    "mediaAutoload",
    // Extra hosts whose media may load without a click, comma separated (3.9's allowlist).
    "mediaHosts",
    // `off` or `on`: whether external link cards fetch a preview.
    "externalImages",
];

impl<H: crate::host::Host> Engine<H> {
    /// `settings()` (`engine-api.md` §3.6, M4): every stored setting, plus the ones that were
    /// never set left out, so a client can tell "default" from "chosen".
    pub async fn settings(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, EngineError> {
        let rows = self.loc_rows(crate::key!("loc", "setting")).await?;
        let mut out = std::collections::BTreeMap::new();
        for (rest, v) in rows {
            let Some(key) = rest.first() else { continue };
            if let Ok(text) = String::from_utf8(v) {
                out.insert(key.clone(), text);
            }
        }
        Ok(out)
    }

    /// `set_setting(key, value)`: `None` removes the key (back to its default).
    ///
    /// An unknown key is refused rather than stored: a typo would otherwise silently do nothing,
    /// and the export would carry junk nobody reads.
    pub async fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), EngineError> {
        if !SETTINGS.contains(&key) {
            return Err(EngineError::invalid_input(format!(
                "unknown setting {key:?} (known: {})",
                SETTINGS.join(", ")
            ))
            .with_detail("kind", "unknownSetting"));
        }
        if let Some(v) = value {
            check_setting_value(key, v)?;
        }
        let k = crate::key!("loc", "setting", key);
        let op = match value {
            Some(v) => KvOp::Put(k, v.as_bytes().to_vec()),
            None => KvOp::Delete(k),
        };
        self.host().state().write(vec![op]).await?;
        self.rt.bus.changed([crate::dto::Topic::Accounts]);
        Ok(())
    }

    /// `media_hosts()`: the allowlist of 3.9, parsed from `mediaHosts`.
    pub async fn media_hosts(&self) -> Result<Vec<String>, EngineError> {
        let all = self.settings().await?;
        Ok(all
            .get("mediaHosts")
            .map(|v| {
                v.split(',')
                    .map(|h| h.trim().to_owned())
                    .filter(|h| !h.is_empty())
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Refuses a value that the setting cannot mean, so a stored setting is always usable.
fn check_setting_value(key: &str, v: &str) -> Result<(), EngineError> {
    let bad = |what: &str| {
        EngineError::invalid_input(format!("{key} = {v:?} is not {what}"))
            .with_detail("kind", "unknownSetting")
    };
    match key {
        "sources" => match v {
            "direct" | "assisted" => Ok(()),
            _ => Err(bad("`direct` or `assisted`")),
        },
        "verify" => match v {
            "pds" | "proof" => Ok(()),
            _ => Err(bad("`pds` or `proof`")),
        },
        "mediaAutoload" | "externalImages" => match v {
            "on" | "off" => Ok(()),
            _ => Err(bad("`on` or `off`")),
        },
        "mediaHosts" => Ok(()),
        _ => Ok(()),
    }
}

/// A saved draft: the `Draft` the composer was working on and when it was last written
/// (`engine-api.md` §3.6, M4).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftEntryRow {
    /// The draft's id (the same one a save returns and a delete takes).
    pub id: String,
    /// When it was last saved (ms).
    pub saved_at_ms: i64,
    /// The draft itself.
    pub draft: crate::dto::Draft,
}

/// How many drafts are kept. A composer autosaves every second while it is open, so without a
/// cap a long editing session would accumulate them without limit; the oldest go first.
pub const MAX_DRAFTS: usize = 100;

impl<H: crate::host::Host> Engine<H> {
    /// `drafts()` (`engine-api.md` §3.6, M4): every saved draft, newest first.
    pub async fn drafts(&self) -> Result<Vec<DraftEntryRow>, EngineError> {
        let rows = self.loc_rows(crate::key!("loc", "draft")).await?;
        let mut out: Vec<DraftEntryRow> = rows
            .into_iter()
            .filter_map(|(rest, v)| {
                let id = rest.first()?.clone();
                let mut row: DraftEntryRow = serde_json::from_slice(&v).ok()?;
                row.id = id;
                Some(row)
            })
            .collect();
        out.sort_by(|a, b| b.saved_at_ms.cmp(&a.saved_at_ms).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// `saveDraft(draft)` -> its id. A draft that already carries an id is overwritten, which is
    /// what the composer's autosave does with the id a previous save returned.
    pub async fn save_draft(&self, draft: &crate::dto::Draft) -> Result<String, EngineError> {
        let existing = match draft {
            crate::dto::Draft::Post { id, .. } => id.clone(),
            crate::dto::Draft::Comment { id, .. } => id.clone(),
        };
        let now = self.host.now_ms();
        // a TID-shaped id: sortable by time and valid as an rkey, which is what a draft that is
        // later published wants its id to resemble
        let id = existing.unwrap_or_else(|| {
            lessdugin_atp::tid::Tid::from_parts(
                u64::try_from(now).unwrap_or(0).saturating_mul(1000),
                0,
            )
            .map(|t| t.to_string())
            .unwrap_or_else(|_| format!("{now:013}"))
        });
        let row = DraftEntryRow {
            id: id.clone(),
            saved_at_ms: now,
            draft: draft.clone(),
        };
        let encoded = serde_json::to_vec(&row)
            .map_err(|e| EngineError::internal(format!("a draft could not be encoded: {e}")))?;
        crate::host::KvStore::write(
            self.host().state(),
            vec![crate::host::KvOp::Put(
                crate::key!("loc", "draft", id.as_str()),
                encoded,
            )],
        )
        .await?;
        self.rt.bus.changed([crate::dto::Topic::Drafts]);
        self.prune_drafts().await?;
        Ok(id)
    }

    /// `deleteDraft(id)`: removes a draft. Deleting what is not there is not an error, because
    /// the composer deletes on publish whether or not a save landed.
    pub async fn delete_draft(&self, id: &str) -> Result<(), EngineError> {
        crate::host::KvStore::write(
            self.host().state(),
            vec![crate::host::KvOp::Delete(crate::key!("loc", "draft", id))],
        )
        .await?;
        self.rt.bus.changed([crate::dto::Topic::Drafts]);
        Ok(())
    }

    /// Drops the oldest drafts past [`MAX_DRAFTS`].
    async fn prune_drafts(&self) -> Result<(), EngineError> {
        let all = self.drafts().await?;
        if all.len() <= MAX_DRAFTS {
            return Ok(());
        }
        let ops: Vec<crate::host::KvOp> = all
            .into_iter()
            .skip(MAX_DRAFTS)
            .map(|d| crate::host::KvOp::Delete(crate::key!("loc", "draft", d.id.as_str())))
            .collect();
        crate::host::KvStore::write(self.host().state(), ops).await?;
        Ok(())
    }
}
