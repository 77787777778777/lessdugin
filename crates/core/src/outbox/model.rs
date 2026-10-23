// SPDX-License-Identifier: AGPL-3.0-or-later
//! The persisted outbox model (`data-model.md` §8.2, `sync.md` §10.4–§10.5): [`PendingWrite`]
//! rows in the durable `state` store, their keys, and the per-account meta, budget and blob rows.
//!
//! | Key (`state` store) | Value |
//! |---|---|
//! | `("out", account, seq)` | [`PendingWrite`] |
//! | `("out.budget", account)` | [`super::budget::BudgetRow`] |
//! | `("out.meta", account)` | [`MetaRow`]: the last TID handed out (W6) |
//! | `("out.blob", account, seq, index, chunk)` | [`ChunkRow`]: ≤ 1 MiB of a blob uploaded when the write is sent (§8.8) |
//!
//! `seq` is the integer value of a TID taken when the write is persisted, so it increases per
//! account (FIFO order) and two processes enqueueing at once do not collide.
//!
//! Record values are kept as their JSON bytes (`serde_bytes`), so a record of any depth, with
//! `$link` and `$bytes` objects, round-trips verbatim whatever the row caps.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dto::{WriteState, WriteTicket};
use crate::engine::{EngineError, ErrorCode};

/// Row version of every outbox row.
pub const OUTBOX_ROW_VERSION: u32 = 1;

/// Key tag of pending writes.
pub const OUT: &str = "out";
/// Key tag of the write budget.
pub const OUT_BUDGET: &str = "out.budget";
/// Key tag of the per-account meta row.
pub const OUT_META: &str = "out.meta";
/// Key tag of blob chunks.
pub const OUT_BLOB: &str = "out.blob";

/// Bytes per blob chunk row (the `KvStore` value cap is 2 MiB, `sync.md` §12.4).
pub const BLOB_CHUNK_BYTES: usize = 1 << 20;

/// `("out", account, seq)`.
pub fn write_key(account: &str, seq: i64) -> Vec<u8> {
    crate::key!(OUT, account, seq)
}

/// The prefix of every write of `account`.
pub fn writes_prefix(account: &str) -> Vec<u8> {
    crate::key!(OUT, account)
}

/// `("out.budget", account)`.
pub fn budget_key(account: &str) -> Vec<u8> {
    crate::key!(OUT_BUDGET, account)
}

/// `("out.meta", account)`.
pub fn meta_key(account: &str) -> Vec<u8> {
    crate::key!(OUT_META, account)
}

/// `("out.blob", account, seq, index, chunk)`.
pub fn blob_chunk_key(account: &str, seq: i64, index: u32, chunk: u32) -> Vec<u8> {
    crate::key!(OUT_BLOB, account, seq, i64::from(index), i64::from(chunk))
}

/// The prefix of every blob chunk of one write.
pub fn blob_prefix(account: &str, seq: i64) -> Vec<u8> {
    crate::key!(OUT_BLOB, account, seq)
}

/// The state of a write (`data-model.md` §8.2), plus `undone` (`sync.md` §7.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteStage {
    /// Waiting for its turn.
    Queued,
    /// Held for the undo / coalescing window until `notBefore` (opinions).
    Held,
    /// Being sent; found at start-up, it is recovered by re-reading its targets.
    Sending,
    /// Applied by the PDS.
    Done,
    /// A precondition failed (another version is current); shown side by side.
    Conflict,
    /// Failed for good (see `error`).
    Failed,
    /// Waiting for the write budget, a rate limit or a back-off until `notBefore`.
    Throttled,
    /// Removed by `undo` before it was sent.
    Undone,
}

impl WriteStage {
    /// `true` for states the pump still has to process.
    pub fn is_pending(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Held | Self::Sending | Self::Throttled
        )
    }
}

/// What an [`Op`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpAction {
    /// Create a record.
    Create,
    /// Update (replace) a record.
    Update,
    /// Delete a record.
    Delete,
}

impl OpAction {
    /// Write-budget points of one op (`data-model.md` §8.2: create 3, update 2, delete 1).
    pub fn cost(self) -> u32 {
        match self {
            Self::Create => crate::limits::WRITE_COST_CREATE,
            Self::Update => crate::limits::WRITE_COST_UPDATE,
            Self::Delete => crate::limits::WRITE_COST_DELETE,
        }
    }
}

/// One record a write targets, with its key and intended value fixed when the write is
/// persisted (W6). Revisions created by the edit protocols are built at send time from the
/// current value; their keys are fixed in the [`Intent`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Op {
    /// What it does.
    pub action: OpAction,
    /// Collection NSID.
    pub collection: String,
    /// Record key.
    pub rkey: String,
    /// The intended value as JSON bytes (creates and updates). Blobs are placeholders (the
    /// locally computed blob object) until they are uploaded, then the object `uploadBlob`
    /// returned, verbatim (§8.8).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    pub value: Option<Vec<u8>>,
    /// A compare-and-swap CID (`deleteRecord` with `swapRecord`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap: Option<String>,
    /// For a `post`: its plain-text excerpt, written if the body has to be spilled when the
    /// write is sent (`data-model.md` §5.3 rule 3, §8.9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

impl Op {
    fn with(action: OpAction, collection: &str, rkey: &str, value: Option<&Value>) -> Self {
        Self {
            action,
            collection: collection.to_owned(),
            rkey: rkey.to_owned(),
            value: value.map(|v| serde_json::to_vec(v).unwrap_or_default()),
            swap: None,
            excerpt: None,
        }
    }

    /// A create of `value`.
    pub fn create(collection: &str, rkey: &str, value: &Value) -> Self {
        Self::with(OpAction::Create, collection, rkey, Some(value))
    }

    /// An update to `value`.
    pub fn update(collection: &str, rkey: &str, value: &Value) -> Self {
        Self::with(OpAction::Update, collection, rkey, Some(value))
    }

    /// A delete.
    pub fn delete(collection: &str, rkey: &str) -> Self {
        Self::with(OpAction::Delete, collection, rkey, None)
    }

    /// Sets the compare-and-swap CID.
    pub fn with_swap(mut self, cid: impl Into<String>) -> Self {
        self.swap = Some(cid.into());
        self
    }

    /// Sets the spill excerpt (posts).
    pub fn with_excerpt(mut self, excerpt: impl Into<String>) -> Self {
        self.excerpt = Some(excerpt.into());
        self
    }

    /// The value as JSON.
    pub fn value_json(&self) -> Option<Value> {
        self.value
            .as_deref()
            .and_then(|b| serde_json::from_slice(b).ok())
    }

    /// Replaces the value.
    pub fn set_value(&mut self, v: &Value) {
        self.value = Some(serde_json::to_vec(v).unwrap_or_default());
    }

    /// The record uri in `account`'s repo.
    pub fn uri(&self, account: &str) -> String {
        format!("at://{account}/{}/{}", self.collection, self.rkey)
    }
}

/// What to do when a create-if-absent finds the key taken (`data-model.md` §8.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum OnExists {
    /// An existing record means success (join).
    Succeed,
    /// The write is a conflict (profile created elsewhere).
    Conflict,
    /// Re-read and continue as an update through the edit protocol (modAction create), with the
    /// revision key fixed at persist time.
    Update {
        /// Revision rkey `P(target) + "-" + tid`.
        revision_rkey: String,
    },
}

/// One opinion intent (`data-model.md` §8.5, `engine-api.md` §3.5 `OpinionIntent`), mapped to
/// [`lessdugin_lexicon::records::OpinionIntent`] when the write is sent. An absent `value`
/// clears.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum OpinionStep {
    /// `setKarma(x | clear)`.
    Karma {
        /// `up`, `down`, `strongUp` or `strongDown`; absent clears.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// `setAgreement(x | clear)`.
    Agreement {
        /// `agree`, `disagree`, `strongAgree` or `strongDisagree`; absent clears.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// `react(add | remove, id, quote?, stance)`.
    React {
        /// React id (a palette slug).
        id: String,
        /// `for` or `against`.
        stance: String,
        /// The `defs#textQuote` object, as JSON text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
        /// Remove instead of add.
        #[serde(default)]
        remove: bool,
    },
    /// `tag(set, slug, value | clear)`.
    Tag {
        /// Tag slug.
        slug: String,
        /// `up`, `down`, `strongUp` or `strongDown`; absent clears.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
}

/// The protocol a write runs and its fixed parameters (`data-model.md` §8.3–§8.8). The records
/// it targets are its [`Op`]s; `ops[0]` is the primary target (the ticket's `uri`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Intent {
    /// `createRecord(rkey)` of a post, comment, sequence or wikiRevision: `ops = [create]`.
    Create,
    /// First content in a forum (§8.4): `ops = [create content, create membership]`; the
    /// membership is included only if a fresh `getRecord` shows it absent.
    FirstContent,
    /// Create forum (§8.4): `ops = [create forum, create membership]`, the membership included
    /// only if absent.
    CreateForum,
    /// Create-if-absent (ADR 0005): `ops = [create]`.
    CreateIfAbsent {
        /// The policy when the key exists.
        on_exists: OnExists,
    },
    /// The edit protocol (§8.3): `ops = [update target]`.
    Edit {
        /// The CID the user edited.
        base: String,
        /// Edit summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Revision rkey `P(target) + "-" + tid` (W6).
        revision_rkey: String,
    },
    /// `putRecord(swapRecord = base)`: `--no-history` edits and profile updates; `ops = [update]`.
    Put {
        /// The CID the user edited.
        base: String,
    },
    /// The delete protocol (§8.7): `ops = [delete target]`; its revisions are purged.
    DeleteContent,
    /// `deleteRecord` (leave, wikiRevision, single records): `ops = [delete]`, with the op's
    /// `swap` as `swapRecord`.
    DeleteRecord,
    /// ModAction removal (§8.4): `ops = [delete modAction]`, snapshotted by a `delete` revision.
    RemoveModAction {
        /// Revision rkey `P(target) + "-" + tid` (W6).
        revision_rkey: String,
    },
    /// Redact history: `ops = [delete revision…]`, chunked at ≤ 200.
    Redact,
    /// An opinion write (§8.5): `ops = [update opinion]` without a value (it is computed when
    /// sent from the current value and `steps`).
    Opinion {
        /// Subject at-uri.
        subject: String,
        /// The subject's CID as seen by the user.
        subject_cid: String,
        /// Forum at-uri.
        forum: String,
        /// The intents, in order (coalesced while held).
        steps: Vec<OpinionStep>,
        /// The opinion before this write (JSON text; `None` = absent), once read: the base of
        /// `undo` after sending.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prior: Option<String>,
        /// `true` once `prior` is known.
        #[serde(default)]
        prior_known: bool,
    },
    /// `mod adopt` (§8.6): no ops until sent.
    Adopt {
        /// Forum at-uri.
        forum: String,
        /// The former moderator's DID.
        former: String,
        /// The former moderator's PDS (resolved when the write was made).
        former_pds: String,
        /// The adopting actor's role: `owner`, `admin` or `moderator`.
        role: String,
        /// The TID of every revision this write creates (their `P` differ, W6).
        revision_tid: String,
        /// What was skipped, once sent (`cannot adopt …`, `kept own assertion …`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        report: Vec<String>,
    },
}

impl Intent {
    /// The protocol name (logs, events).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::FirstContent => "firstContent",
            Self::CreateForum => "createForum",
            Self::CreateIfAbsent { .. } => "createIfAbsent",
            Self::Edit { .. } => "edit",
            Self::Put { .. } => "put",
            Self::DeleteContent => "deleteContent",
            Self::DeleteRecord => "deleteRecord",
            Self::RemoveModAction { .. } => "removeModAction",
            Self::Redact => "redact",
            Self::Opinion { .. } => "opinion",
            Self::Adopt { .. } => "adopt",
        }
    }
}

/// A blob the write uploads when it is sent (§8.8); its bytes are in `out.blob` rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingBlob {
    /// The local CID (raw, sha2-256); the returned `ref.$link` must equal it.
    pub cid: String,
    /// The normalised declared MIME type (`data-model.md` §8.8).
    pub mime: String,
    /// Size in bytes.
    pub size: u64,
    /// Number of chunk rows.
    pub chunks: u32,
    /// Set once uploaded (the placeholders were replaced by the returned object).
    #[serde(default)]
    pub uploaded: bool,
    /// When it was last uploaded (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<i64>,
}

impl PendingBlob {
    /// `true` if the blob has to be uploaded (again) before the record write at `now`.
    pub fn needs_upload(&self, now: i64) -> bool {
        !self.uploaded
            || self.uploaded_at.is_none_or(|t| {
                now.saturating_sub(t) >= crate::outbox::protocol::BLOB_REUPLOAD_AFTER_MS
            })
    }
}

/// The result of one target once the write is done.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpResult {
    /// The record uri.
    pub uri: String,
    /// The record CID (absent for deletes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
}

/// A stored error (an [`EngineError`] with its detail kept as JSON text).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredError {
    /// The error code.
    pub code: ErrorCode,
    /// The message.
    pub message: String,
    /// Whether retrying may succeed.
    pub retryable: bool,
    /// The detail object, as JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl From<&EngineError> for StoredError {
    fn from(e: &EngineError) -> Self {
        Self {
            code: e.code,
            message: e.message.clone(),
            retryable: e.retryable,
            detail: e.detail.as_ref().map(ToString::to_string),
        }
    }
}

impl StoredError {
    /// Back to an [`EngineError`].
    pub fn to_engine_error(&self) -> EngineError {
        EngineError {
            code: self.code,
            message: self.message.clone(),
            retryable: self.retryable,
            detail: self
                .detail
                .as_deref()
                .and_then(|d| serde_json::from_str(d).ok()),
        }
    }
}

/// `PendingWrite` (`data-model.md` §8.2): one durable write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWrite {
    /// Row version.
    pub v: u32,
    /// The account DID the write is stamped with (W9).
    pub account: String,
    /// FIFO position per account.
    pub seq: i64,
    /// The protocol.
    pub intent: Intent,
    /// The targets.
    pub ops: Vec<Op>,
    /// Blobs uploaded when sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<PendingBlob>,
    /// The state.
    pub state: WriteStage,
    /// Protocol runs so far.
    pub attempts: u32,
    /// When it was persisted (ms).
    pub created_at: i64,
    /// Last state change (ms).
    pub updated_at: i64,
    /// End of the hold (`held`) or of the back-off (`throttled`), ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
    /// `throttled` because the account's session was refused (`error` says how): the write is
    /// not tried again until the account logs in again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_login: bool,
    /// A write request of this write may have reached the PDS (undo is no longer a removal).
    #[serde(default)]
    pub sent: bool,
    /// The results, once done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<OpResult>,
    /// The last error (failed, conflict, or the cause of a back-off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StoredError>,
    /// For an inverse write made by `undo` after sending: the undone write's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_of: Option<String>,
    /// The `seq` of an earlier write of the same account this one depends on: it is sent only
    /// once that write is done, and fails without being sent if that write failed, conflicted
    /// or was undone (`mods rm` runs only after its `mod adopt` completed, `data-model.md`
    /// §8.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
}

impl PendingWrite {
    /// The write id: `<account>/<seq>`.
    pub fn id(&self) -> String {
        write_id(&self.account, self.seq)
    }

    /// The primary target's uri.
    pub fn primary_uri(&self) -> Option<String> {
        self.ops.first().map(|o| o.uri(&self.account))
    }

    /// The ticket state (`engine-api.md` §3.5).
    pub fn status(&self) -> WriteState {
        match self.state {
            WriteStage::Queued => WriteState::Pending {
                not_before_ms: None,
                held: false,
                throttled: false,
                needs_login: false,
                error: None,
            },
            WriteStage::Held => WriteState::Pending {
                not_before_ms: self.not_before,
                held: true,
                throttled: false,
                needs_login: false,
                error: None,
            },
            WriteStage::Throttled => WriteState::Pending {
                not_before_ms: self.not_before,
                held: false,
                throttled: !self.needs_login,
                needs_login: self.needs_login,
                error: self.error.as_ref().map(StoredError::to_engine_error),
            },
            WriteStage::Sending => WriteState::Sending,
            WriteStage::Done => {
                let first = self.results.first();
                WriteState::Done {
                    uri: first
                        .map(|r| r.uri.clone())
                        .or_else(|| self.primary_uri())
                        .unwrap_or_default(),
                    cid: first.and_then(|r| r.cid.clone()),
                }
            }
            WriteStage::Conflict | WriteStage::Failed => WriteState::Failed {
                error: self.error.as_ref().map_or_else(
                    || {
                        if self.state == WriteStage::Conflict {
                            EngineError::conflict("the record changed")
                        } else {
                            EngineError::internal("the write failed")
                        }
                    },
                    StoredError::to_engine_error,
                ),
            },
            WriteStage::Undone => WriteState::Undone,
        }
    }

    /// The ticket.
    pub fn ticket(&self) -> WriteTicket {
        WriteTicket {
            id: self.id(),
            state: self.status(),
        }
    }

    /// Write-budget points of the ops known before sending (an estimate for protocols that
    /// compute their ops when sent).
    pub fn estimated_cost(&self) -> u32 {
        let ops: u32 = self.ops.iter().map(|o| o.action.cost()).sum();
        match &self.intent {
            // + the revision create
            Intent::Edit { .. } | Intent::RemoveModAction { .. } => ops + OpAction::Create.cost(),
            // the opinion op is a create, an update or a delete
            Intent::Opinion { .. } => OpAction::Create.cost(),
            _ => ops.max(1),
        }
    }
}

/// The write id of `(account, seq)`.
pub fn write_id(account: &str, seq: i64) -> String {
    format!("{account}/{seq}")
}

/// Parses a write id into `(account, seq)`.
pub fn parse_write_id(id: &str) -> Option<(String, i64)> {
    let (account, seq) = id.rsplit_once('/')?;
    if !account.starts_with("did:") {
        return None;
    }
    Some((account.to_owned(), seq.parse().ok()?))
}

/// `("out.meta", account)`: TIDs strictly increase per account (W6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaRow {
    /// Row version.
    pub v: u32,
    /// The last TID handed out (its integer value).
    pub last_tid: u64,
}

/// A blob chunk row value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRow {
    /// Row version.
    pub v: u32,
    /// The bytes.
    #[serde(with = "serde_bytes")]
    pub b: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::row;
    use serde_json::json;

    fn sample() -> PendingWrite {
        PendingWrite {
            v: OUTBOX_ROW_VERSION,
            account: "did:plc:ewvi7nxzyoun6zhxrhs64oiz".into(),
            seq: 1_792_151_363_000_123,
            intent: Intent::Opinion {
                subject: "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.y.post/a".into(),
                subject_cid: "bafyrei".into(),
                forum: "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x.y.forum/f".into(),
                steps: vec![
                    OpinionStep::Karma {
                        value: Some("up".into()),
                    },
                    OpinionStep::React {
                        id: "insightful".into(),
                        stance: "for".into(),
                        quote: Some(r#"{"exact":"q"}"#.into()),
                        remove: false,
                    },
                    OpinionStep::Tag {
                        slug: "x".into(),
                        value: None,
                    },
                ],
                prior: None,
                prior_known: true,
            },
            ops: vec![
                Op::create(
                    "x.y.post",
                    "k",
                    &json!({"a": {"$link": "bafyrei"}, "n": -3, "deep": [[[[{"b": true}]]]]}),
                )
                .with_excerpt("e"),
                Op::delete("x.y.post", "k2").with_swap("bafy"),
            ],
            blobs: vec![PendingBlob {
                cid: "bafkrei".into(),
                mime: "image/png".into(),
                size: 69,
                chunks: 1,
                uploaded: false,
                uploaded_at: None,
            }],
            state: WriteStage::Held,
            attempts: 2,
            created_at: 5,
            updated_at: 6,
            not_before: Some(4_005),
            needs_login: false,
            sent: false,
            results: vec![OpResult {
                uri: "at://x/y/z".into(),
                cid: None,
            }],
            error: Some(StoredError::from(
                &EngineError::conflict("x").with_detail("current", "bafy"),
            )),
            undo_of: None,
            after: Some(1_792_151_363_000_122),
        }
    }

    #[test]
    fn rows_roundtrip() {
        let w = sample();
        let bytes = row::encode(&w).unwrap();
        assert_eq!(row::decode::<PendingWrite>(&bytes).unwrap(), w);
        for intent in [
            Intent::Create,
            Intent::CreateIfAbsent {
                on_exists: OnExists::Update {
                    revision_rkey: "r".into(),
                },
            },
            Intent::Edit {
                base: "b".into(),
                summary: None,
                revision_rkey: "r".into(),
            },
            Intent::Adopt {
                forum: "f".into(),
                former: "d".into(),
                former_pds: "p".into(),
                role: "owner".into(),
                revision_tid: "t".into(),
                report: vec!["x".into()],
            },
        ] {
            let w = PendingWrite {
                intent,
                ..w.clone()
            };
            let bytes = row::encode(&w).unwrap();
            assert_eq!(row::decode::<PendingWrite>(&bytes).unwrap(), w);
        }
        let m = MetaRow { v: 1, last_tid: 7 };
        assert_eq!(
            row::decode::<MetaRow>(&row::encode(&m).unwrap()).unwrap(),
            m
        );
    }

    #[test]
    fn ids_and_status() {
        let w = sample();
        assert_eq!(parse_write_id(&w.id()), Some((w.account.clone(), w.seq)));
        assert_eq!(parse_write_id("x/1"), None);
        assert!(matches!(
            w.status(),
            WriteState::Pending {
                held: true,
                not_before_ms: Some(4_005),
                ..
            }
        ));
        let mut d = w.clone();
        d.state = WriteStage::Conflict;
        let WriteState::Failed { error } = d.status() else {
            panic!()
        };
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.detail.unwrap()["current"], "bafy");
        assert_eq!(w.estimated_cost(), 3);
    }
}
