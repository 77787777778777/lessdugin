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
//! acknowledgements (`ack`, PRIV-9: the interstitials show again), the notifications and their
//! baselines (`notif`, `nbase`: a planted row would list any text as a notification, or move a
//! forum's watermark or generation and silence it) or prepared media, and a setting only with a
//! value `set_setting` accepts. Those members are counted as `refused`, and the report counts
//! what was written per family, so a client can say what changed.

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
/// `threat-model.md` PRIV-10, follows, drafts and the validated settings. The notifications are
/// exported but never imported: they are computed again on this device.
pub const IMPORTABLE: [&str; 12] = [
    "alias",
    "bookmark",
    "draft",
    "follow",
    "fvisit",
    "karmaCadence",
    "mute",
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

/// The most members an import takes (`too_large` past it, before anything is written).
pub const IMPORT_MAX_MEMBERS: usize = 100_000;

/// The most bytes of decoded keys and values an import takes.
pub const IMPORT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// An import writes at most this many rows per batch.
const IMPORT_BATCH: usize = 1_000;

fn too_large_import(what: String) -> EngineError {
    EngineError::new(
        super::ErrorCode::TooLarge,
        format!("this export is too large to import: {what}"),
    )
    .with_detail("kind", "importTooLarge")
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
        // ("loc","ack",account,kind), ("loc","notif",account[,id]), ("loc","nbase",account,forum),
        // ("loc","draft",account,id)
        "account" | "as_pin" | "seeded" | "ack" | "notif" | "nbase" | "draft" => {
            part_str(parts.get(2))
        }
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
    /// never writes is refused (the module docs). At most [`IMPORT_MAX_MEMBERS`] members and
    /// [`IMPORT_MAX_BYTES`] (`too_large` otherwise, before anything is written); a duplicate key's
    /// last member wins. Rows are written in batches of at most 1,000.
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
        if export.members.len() > IMPORT_MAX_MEMBERS {
            return Err(too_large_import(format!(
                "{} members (at most {IMPORT_MAX_MEMBERS})",
                export.members.len()
            )));
        }
        let mut report = ImportReport::default();
        // by key, the last one winning: a duplicate replaces the row before it
        let mut rows: std::collections::BTreeMap<Vec<u8>, (Vec<u8>, String)> = Default::default();
        let mut bytes = 0usize;
        for m in &export.members {
            let (Ok(k), Ok(v)) = (b64().decode(&m.key), b64().decode(&m.value)) else {
                report.undecodable += 1;
                continue;
            };
            bytes = bytes.saturating_add(k.len() + v.len());
            if bytes > IMPORT_MAX_BYTES {
                return Err(too_large_import(format!(
                    "more than {IMPORT_MAX_BYTES} bytes of keys and values"
                )));
            }
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
            rows.insert(k, (v, kind.to_owned()));
        }
        let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
        for (_, kind) in rows.values() {
            *kinds.entry(kind.clone()).or_default() += 1;
        }
        report.imported = rows.len();
        report.kinds = kinds.into_iter().collect();
        // bounded batches, so one import is never one unbounded transaction
        let mut ops: Vec<KvOp> = Vec::with_capacity(IMPORT_BATCH.min(rows.len()));
        for (k, (v, _)) in rows {
            ops.push(KvOp::Put(k, v));
            if ops.len() == IMPORT_BATCH {
                self.host().state().write(std::mem::take(&mut ops)).await?;
            }
        }
        if !ops.is_empty() {
            self.host().state().write(ops).await?;
        }
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
    ///
    /// `sources` and `verify` are the ones in effect, not merely the stored rows: `sources` is
    /// [`Engine::sources`], and `verify` is `pds`, the only mode there is (`proof` is Reserved).
    pub async fn settings(&self) -> Result<crate::dto::Settings, EngineError> {
        let raw = self.stored_settings().await?;
        Ok(crate::dto::Settings {
            sources: match self.sources().await? {
                super::Sources::Direct => crate::dto::Sources::Direct,
                super::Sources::Assisted => crate::dto::Sources::Assisted,
            },
            verify: crate::dto::VerifyMode::Pds,
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

    /// The `sources` in effect (`sync.md` §2.3, O-3): a flag of this invocation
    /// ([`EngineConfig::sources_flag`]), else the stored setting, else the build default. The
    /// engine reads it at each use, so `set_setting("sources", …)` applies at once: in `direct`
    /// no CORE+ or EXT endpoint (Slingshot, Jetstream, Constellation, a relay, an indexer) is
    /// contacted.
    ///
    /// [`EngineConfig::sources_flag`]: super::EngineConfig::sources_flag
    pub async fn sources(&self) -> Result<super::Sources, EngineError> {
        if let Some(s) = self.config.sources_flag {
            return Ok(s);
        }
        let assisted = self.config.sources == super::Sources::Assisted;
        Ok(if stored_assisted(self.host().state(), assisted).await {
            super::Sources::Assisted
        } else {
            super::Sources::Direct
        })
    }

    /// `media_hosts()`: the allowlist of 3.9, parsed from `mediaHosts`.
    pub async fn media_hosts(&self) -> Result<Vec<String>, EngineError> {
        let raw = self.stored_settings().await?;
        Ok(split_hosts(raw.get("mediaHosts")))
    }
}

/// The stored `sources` setting as "assisted", `default` when none is stored. A store that
/// cannot be read counts as `direct`: no CORE+ endpoint is contacted on a guess.
pub(crate) async fn stored_assisted<K: KvStore>(state: &K, default: bool) -> bool {
    match state
        .get_many(vec![crate::key!("loc", "setting", "sources")])
        .await
        .map(|mut v| v.pop().flatten())
    {
        Ok(Some(v)) => v != b"direct",
        Ok(None) => default,
        Err(e) => {
            tracing::warn!(error = %e, "the sources setting could not be read: direct");
            false
        }
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
            "pds" => Ok(()),
            // verifying every displayed item in the background is Reserved (`sync.md` §2.3):
            // storing it would show a choice that does nothing
            "proof" => Err(EngineError::new(
                super::ErrorCode::Unsupported,
                "verify = \"proof\" (background verification) is not implemented yet",
            )
            .with_detail("milestone", "P2")),
            _ => Err(bad("`pds`")),
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
            Ok(l)
                if l.iter()
                    .all(|u| lessdugin_atp::syntax::AtUri::parse(u).is_ok()) =>
            {
                Ok(())
            }
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

/// How many drafts an account keeps. A composer autosaves every second while it is open, so
/// without a cap a long editing session would accumulate them without limit; the oldest go
/// first.
pub const MAX_DRAFTS: usize = 100;

/// The largest draft a save stores (its row, serialised): the SPA's own store had the same
/// cap, and a value past the host's 2 MiB would fail as a host error instead of `too_large`.
pub const DRAFT_MAX_BYTES: usize = 1_000_000;

/// `("loc", "draft", account[, id])`: drafts are per account (`threat-model.md` AUTH-11,
/// PRIV-8), so another account on the device neither lists, exports nor deletes them.
fn draft_key(account: &str, id: &str) -> Vec<u8> {
    crate::key!("loc", "draft", account, id)
}

impl<H: crate::host::Host> Engine<H> {
    /// The active account, which drafts belong to (`auth_required` without one).
    async fn draft_owner(&self) -> Result<String, EngineError> {
        self.me().await?.ok_or_else(|| {
            EngineError::new(
                super::ErrorCode::AuthRequired,
                "drafts belong to an account; log in first",
            )
        })
    }

    /// `drafts()` (`engine-api.md` §3.6, M4): the active account's saved drafts, newest first
    /// (none without an active account).
    pub async fn drafts(&self) -> Result<Vec<DraftEntryRow>, EngineError> {
        let Some(me) = self.me().await? else {
            return Ok(Vec::new());
        };
        let mut out: Vec<DraftEntryRow> = self
            .draft_rows(&me)
            .await?
            .into_iter()
            .filter_map(|(id, row)| {
                let mut row = row?;
                row.id = id;
                Some(row)
            })
            .collect();
        out.sort_by(|a, b| b.saved_at_ms.cmp(&a.saved_at_ms).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Every stored draft row of `account` by id, `None` for a row that does not decode.
    async fn draft_rows(
        &self,
        account: &str,
    ) -> Result<Vec<(String, Option<DraftEntryRow>)>, EngineError> {
        let rows = self.loc_rows(crate::key!("loc", "draft", account)).await?;
        Ok(rows
            .into_iter()
            .filter_map(|(rest, v)| {
                // rest = [account, id]
                let id = rest.get(1)?.clone();
                Some((id, serde_json::from_slice(&v).ok()))
            })
            .collect())
    }

    /// `saveDraft(draft)` -> its id, for the active account (`auth_required` without one). A
    /// draft that already carries an id is overwritten, which is what the composer's autosave
    /// does with the id a previous save returned. A draft over [`DRAFT_MAX_BYTES`] is refused
    /// (`too_large`).
    pub async fn save_draft(&self, draft: &crate::dto::Draft) -> Result<String, EngineError> {
        let me = self.draft_owner().await?;
        let existing = match draft {
            crate::dto::Draft::Post { id, .. } => id.clone(),
            crate::dto::Draft::Comment { id, .. } => id.clone(),
        };
        let now = self.host.now_ms();
        // a TID-shaped id: sortable by time and valid as an rkey, which is what a draft that is
        // later published wants its id to resemble. The microseconds and the clock id are
        // random, so two drafts started in the same millisecond (a post and a comment composer
        // autosaving together) never share an id.
        let id = existing.unwrap_or_else(|| {
            let mut r = [0u8; 4];
            self.host.fill_random(&mut r);
            let micros = u64::from(u16::from_le_bytes([r[0], r[1]]) % 1000);
            let clock = u16::from_le_bytes([r[2], r[3]]) % 1024;
            lessdugin_atp::tid::Tid::from_parts(
                u64::try_from(now)
                    .unwrap_or(0)
                    .saturating_mul(1000)
                    .saturating_add(micros),
                clock,
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
        if encoded.len() > DRAFT_MAX_BYTES {
            return Err(EngineError::new(
                super::ErrorCode::TooLarge,
                format!(
                    "a draft is at most {DRAFT_MAX_BYTES} bytes; this one is {}",
                    encoded.len()
                ),
            )
            .with_detail("kind", "draftTooLarge"));
        }
        crate::host::KvStore::write(
            self.host().state(),
            vec![crate::host::KvOp::Put(draft_key(&me, &id), encoded)],
        )
        .await?;
        self.rt.bus.changed([crate::dto::Topic::Drafts]);
        self.prune_drafts(&me).await?;
        Ok(id)
    }

    /// `deleteDraft(id)`: removes one of the active account's drafts (`auth_required` without
    /// one). Deleting what is not there is not an error, because the composer deletes on publish
    /// whether or not a save landed.
    pub async fn delete_draft(&self, id: &str) -> Result<(), EngineError> {
        let me = self.draft_owner().await?;
        crate::host::KvStore::write(
            self.host().state(),
            vec![crate::host::KvOp::Delete(draft_key(&me, id))],
        )
        .await?;
        self.rt.bus.changed([crate::dto::Topic::Drafts]);
        Ok(())
    }

    /// Drops `account`'s oldest drafts past [`MAX_DRAFTS`]. It counts the stored rows, so a row
    /// that does not decode (an import, a newer version) counts too, as the oldest.
    async fn prune_drafts(&self, account: &str) -> Result<(), EngineError> {
        let mut all: Vec<(i64, String)> = self
            .draft_rows(account)
            .await?
            .into_iter()
            .map(|(id, row)| (row.map_or(i64::MIN, |r| r.saved_at_ms), id))
            .collect();
        if all.len() <= MAX_DRAFTS {
            return Ok(());
        }
        all.sort_by(|a, b| b.cmp(a));
        let ops: Vec<crate::host::KvOp> = all
            .into_iter()
            .skip(MAX_DRAFTS)
            .map(|(_, id)| crate::host::KvOp::Delete(draft_key(account, &id)))
            .collect();
        crate::host::KvStore::write(self.host().state(), ops).await?;
        Ok(())
    }
}
