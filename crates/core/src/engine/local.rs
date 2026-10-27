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
//!
//! **What an import never writes.** An export is a file, and a file can come from someone else
//! ("my settings backup"). So an import writes only the [`IMPORTABLE`] `loc` families: never a
//! key outside `loc` (the outbox would send a planted write with the user's session), never the
//! login state (`account`, `active_account`, the authorization-server pins `as_pin`, `seeded`:
//! `threat-model.md` ID-11, the rows the `web` bridge keeps read-only), never the public-action
//! acknowledgements (`ack`, PRIV-9: the interstitials show again) or prepared media, and a
//! setting only with a value `set_setting` accepts. Those members are counted as `refused`, and
//! the report counts what was written per family, so a client can say what changed.

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

/// The `loc` families an import may write (see the module docs): the lists and marks of
/// `threat-model.md` PRIV-10, follows, drafts and the validated settings.
pub const IMPORTABLE: [&str; 14] = [
    "alias",
    "bookmark",
    "draft",
    "follow",
    "fvisit",
    "karmaCadence",
    "mute",
    "nbase",
    "notif",
    "read",
    "rejected",
    "setting",
    "sub",
    "unfollow",
];

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

/// `import_local`'s result (a DTO).
pub use crate::dto::ImportReport;

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

/// The family of a row an import may write as `v` (see [`IMPORTABLE`]), or `None`.
fn importable<'a>(parts: &'a [Part], v: &[u8]) -> Option<&'a str> {
    if part_str(parts.first()) != Some("loc") {
        return None;
    }
    let tag = part_str(parts.get(1)).filter(|t| IMPORTABLE.contains(t))?;
    if tag == "setting" {
        let k = part_str(parts.get(2))
            .filter(|k| SETTINGS.contains(k) && !NOT_IMPORTED.contains(k) && parts.len() == 3)?;
        let v = std::str::from_utf8(v).ok()?;
        check_setting_value(k, v).ok()?;
    }
    Some(tag)
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
    /// `--mine` export cannot write another account's state, and a member of a family an import
    /// never writes is refused (the module docs). Rows are written in one batch.
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
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
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
            let Some(kind) = importable(&parts, &v) else {
                report.refused += 1;
                continue;
            };
            if !in_scope(&parts, &scope, active.as_deref()) {
                report.out_of_scope += 1;
                continue;
            }
            // a duplicate key: the last one wins, as in any key-value store
            let dup = ops.iter().any(|op| matches!(op, KvOp::Put(prev, _) if prev == &k));
            ops.retain(|op| !matches!(op, KvOp::Put(prev, _) if prev == &k));
            if !dup {
                *kinds.entry(kind.to_owned()).or_default() += 1;
                report.imported += 1;
            }
            ops.push(KvOp::Put(k, v));
        }
        if !ops.is_empty() {
            self.host().state().write(ops).await?;
        }
        report.kinds = kinds.into_iter().collect();
        Ok(report)
    }
}

/// The stored settings, `("loc", "setting", key)` (synthesis-v1.md §1.4, §6.12: "accounts,
/// sources, verification, indexer consent, external images, media autoload hosts"), each with
/// its value as text: the form an export carries.
///
/// These are per device, not per account: they are how this copy of the app behaves, so they
/// live in the same `loc` state the rest of the local lists use, and they travel with
/// [`LocalExport`].
pub const SETTINGS: [&str; 7] = [
    // `direct` or `assisted` (`engine::Sources`).
    "sources",
    // `pds` or `proof` (`engine::VerifyMode`).
    "verify",
    // `off` or `on`: whether media loads at all (3.9's master switch).
    "mediaAutoload",
    // Extra hosts whose media may load without a click, comma separated (3.9's allowlist;
    // `Settings.mediaAutoloadHosts`).
    "mediaHosts",
    // `off` or `on`: whether external link cards fetch a preview.
    "externalImages",
    // `Settings.externalImagesForums`: a JSON array of forum uris.
    "externalImagesForums",
    // `Settings.indexers`: a JSON array of `IndexerConsent`.
    "indexers",
];

/// The keys of `set_setting` besides [`SETTINGS`]: the fields of the `Settings` object
/// (`types.ts` `SettingKey`) that are not stored under their own name.
pub const SETTING_FIELDS: [&str; 7] = [
    "sources",
    "verify",
    "indexers",
    "externalImagesForums",
    "mediaAutoloadHosts",
    "karmaNotifications",
    "acknowledgedPublicActions",
];

/// The stored settings an import may not write (sec-9): they name third parties this device
/// would then contact (media hosts, indexers, forums whose external images load). A backup
/// cannot add one; the user does, in the settings, where the list is shown.
const NOT_IMPORTED: [&str; 3] = ["mediaHosts", "externalImagesForums", "indexers"];

impl<H: crate::host::Host> Engine<H> {
    /// `settings()` (`engine-api.md` §3.6, M4): the whole local picture, in the shape the
    /// client declares (`Settings` in `types.ts`). Fields never set report their defaults.
    ///
    /// It is an OBJECT with four list fields, not a flat key/value map. The client does
    /// `settings.indexers.map(...)`, so returning a map here crashed the settings page - which
    /// is what the CI-only e2e failure turned out to be, and why the client's type wins.
    pub async fn settings(&self) -> Result<crate::dto::Settings, EngineError> {
        let raw = self.stored_settings().await?;
        let get = |k: &str, d: &str| raw.get(k).map_or_else(|| d.to_owned(), Clone::clone);
        Ok(crate::dto::Settings {
            sources: match get("sources", "assisted").as_str() {
                "direct" => crate::dto::Sources::Direct,
                _ => crate::dto::Sources::Assisted,
            },
            verify: match get("verify", "pds").as_str() {
                "proof" => crate::dto::VerifyMode::Proof,
                _ => crate::dto::VerifyMode::Pds,
            },
            indexers: stored_list(&raw, "indexers"),
            external_images_forums: stored_list(&raw, "externalImagesForums"),
            media_autoload_hosts: split_hosts(raw.get("mediaHosts")),
            karma_notifications: self.karma_notifications().await?,
            acknowledged_public_actions: self.acknowledged_public_actions().await?,
        })
    }

    /// The stored key/value rows, the raw form `set_setting` writes.
    async fn stored_settings(
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

    /// `set_setting(key, value)` (`engine-api.md` §3.6): `key` is a field of `Settings`
    /// ([`SETTING_FIELDS`]) with a value of that field's type, or a stored setting
    /// ([`SETTINGS`]) with its text. `None` (JSON `null`) returns it to its default.
    /// `karmaNotifications` and `acknowledgedPublicActions` are kept where their own operations
    /// keep them (the latter needs an active account: `auth_required`).
    ///
    /// An unknown key, or a value the setting cannot mean, is refused (`invalid_input`, `detail.kind
    /// = "unknownSetting"`) rather than stored: a typo would otherwise silently do nothing, and
    /// the export would carry junk nobody reads.
    pub async fn set_setting(
        &self,
        key: &str,
        value: Option<&serde_json::Value>,
    ) -> Result<(), EngineError> {
        use serde_json::Value;
        let bad = |what: &str| {
            EngineError::invalid_input(format!("{key} = {} is not {what}", show(value)))
                .with_detail("kind", "unknownSetting")
        };
        // `null` is the default, like an absent value
        let value = value.filter(|v| !v.is_null());
        let (name, text): (&str, Option<String>) = match key {
            "karmaNotifications" => {
                let c = typed(value, || bad("a karma cadence"))?
                    .unwrap_or(crate::dto::KarmaCadence::Daily);
                return self.set_karma_notifications(c).await;
            }
            "acknowledgedPublicActions" => {
                let kinds: Vec<crate::dto::PublicActionKind> =
                    typed(value, || bad("a list of public actions"))?.unwrap_or_default();
                return self.set_acknowledged_public_actions(&kinds).await;
            }
            "mediaAutoloadHosts" => {
                let hosts: Vec<String> =
                    typed(value, || bad("a list of host names"))?.unwrap_or_default();
                let joined = hosts.join(",");
                if hosts.iter().any(|h| !is_host(&h.to_ascii_lowercase())) {
                    return Err(bad("a list of host names"));
                }
                ("mediaHosts", (!hosts.is_empty()).then_some(joined))
            }
            k if SETTINGS.contains(&k) => match value {
                None => (k, None),
                Some(Value::String(v)) if !matches!(k, "externalImagesForums" | "indexers") => {
                    check_setting_value(k, v)?;
                    (k, Some(v.clone()))
                }
                // the two lists are stored as their JSON text
                Some(v) if matches!(k, "externalImagesForums" | "indexers") => {
                    let t = v.to_string();
                    check_setting_value(k, &t)?;
                    (k, Some(t))
                }
                Some(_) => return Err(bad("a string")),
            },
            _ => {
                return Err(EngineError::invalid_input(format!(
                    "unknown setting {key:?} (known: {})",
                    SETTING_FIELDS.join(", ")
                ))
                .with_detail("kind", "unknownSetting"));
            }
        };
        let k = crate::key!("loc", "setting", name);
        let op = match text {
            Some(v) => KvOp::Put(k, v.into_bytes()),
            None => KvOp::Delete(k),
        };
        self.host().state().write(vec![op]).await?;
        self.rt.bus.changed([crate::dto::Topic::Accounts]);
        Ok(())
    }

    /// `media_hosts()`: the allowlist of 3.9, parsed from `mediaHosts`.
    pub async fn media_hosts(&self) -> Result<Vec<String>, EngineError> {
        let raw = self.stored_settings().await?;
        Ok(split_hosts(raw.get("mediaHosts")))
    }
}

/// `v` as a `T`, or `bad()`.
fn typed<T: serde::de::DeserializeOwned>(
    v: Option<&serde_json::Value>,
    bad: impl FnOnce() -> EngineError,
) -> Result<Option<T>, EngineError> {
    v.map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|_| bad())
}

/// A stored setting that holds a JSON list (empty if unset or unreadable).
fn stored_list<T: serde::de::DeserializeOwned>(
    raw: &std::collections::BTreeMap<String, String>,
    k: &str,
) -> Vec<T> {
    raw.get(k)
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default()
}

/// A setting's value as an error message shows it.
fn show(v: Option<&serde_json::Value>) -> String {
    v.map_or_else(|| "null".to_owned(), serde_json::Value::to_string)
}

/// Splits a stored `mediaHosts` value into hosts: comma separated, trimmed, empties dropped.
fn split_hosts(v: Option<&String>) -> Vec<String> {
    v.map(|v| {
        v.split(',')
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// `true` for a host name (`a.example`, `cdn-2.example.com`): no scheme, path, port or space.
fn is_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

/// Refuses a stored value that the setting cannot mean, so a stored setting is always usable.
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
        "mediaHosts" => {
            if split_hosts(Some(&v.to_owned()))
                .iter()
                .all(|h| is_host(&h.to_ascii_lowercase()))
            {
                Ok(())
            } else {
                Err(bad("a comma-separated list of host names"))
            }
        }
        "externalImagesForums" => match serde_json::from_str::<Vec<String>>(v) {
            Ok(l) if l.iter().all(|u| lessdugin_atp::syntax::AtUri::parse(u).is_ok()) => Ok(()),
            _ => Err(bad("a list of forum uris")),
        },
        "indexers" => match serde_json::from_str::<Vec<crate::dto::IndexerConsent>>(v) {
            Ok(l) if l.iter().all(|i| is_https_endpoint(&i.url)) => Ok(()),
            _ => Err(bad("a list of indexer consents with https URLs")),
        },
        _ => Ok(()),
    }
}

/// An indexer URL a consent may name (`threat-model.md` §5): `https`, a host name, the default
/// port, no user info.
fn is_https_endpoint(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|u| {
        u.scheme() == "https"
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str().is_some_and(is_host)
    })
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
