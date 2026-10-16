// SPDX-License-Identifier: AGPL-3.0-or-later
//! The write protocols (`data-model.md` §8.3–§8.8, ADR 0005), run by the pump for one
//! [`PendingWrite`] in `sending`.
//!
//! Every protocol starts from fresh reads of the own repo (never the cache), so running it again
//! after a crash, an ambiguous failure or a back-off is safe: a write that already took effect is
//! recognised by its targets' CIDs and not sent twice. `InvalidSwap` is the only conflict
//! signal; a 5xx on a write is ambiguous and is resolved by re-reading before anything is sent
//! again. Swap retries wait `uniform(200 ms, 800 ms)` ([`Runner::jitter`]).

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::{
    Action, Opinion, OpinionIntent, StrongRef, TextQuote, qkey,
};
use lessdugin_lexicon::values::{Agreement, RevisionOp, Stance, Tier, VoteValue};
use serde_json::{Map, Value};

use crate::engine::{EngineError, ErrorCode};
use crate::host::{Host, KvOp, KvStore};
use crate::limits;
use crate::outbox::apply::{AppliedRecord, AppliedWrite};
use crate::outbox::blob::{self, BlobData};
use crate::outbox::model::{
    self, ChunkRow, Intent, OnExists, Op, OpAction, OpResult, OpinionStep, PendingWrite,
};
use crate::outbox::pds::{BatchOp, CallError, Current, PdsClient, Session, Swap, Written};
use crate::outbox::records;
use crate::store::admit::LexiconValidator;
use crate::store::records::RecordStore;
use crate::store::row;

/// How a protocol run ended.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Applied (or found already applied); `w.results` is set.
    Done,
    /// A precondition failed: another version is current.
    Conflict(EngineError),
    /// Failed for good.
    Failed(EngineError),
    /// Try again at `at` (rate limit, write budget, ambiguous failure, transient read error).
    Retry {
        /// When (ms).
        at: i64,
        /// Why.
        error: EngineError,
    },
}

/// Protocol runs after which an ambiguous or transient failure becomes final (*tunable*).
pub const MAX_ATTEMPTS: u32 = 10;
/// Back-off base after an ambiguous or transient failure (ms, *tunable*).
pub const BACKOFF_BASE_MS: i64 = 2_000;
/// Back-off cap (ms, *tunable*).
pub const BACKOFF_MAX_MS: i64 = 5 * 60 * 1000;

fn invalid(msg: impl Into<String>) -> CallError {
    CallError::Rejected {
        status: 0,
        name: None,
        error: EngineError::invalid_input(msg),
    }
}

fn rejected(error: EngineError) -> CallError {
    CallError::Rejected {
        status: 0,
        name: None,
        error,
    }
}

fn storage(e: impl std::fmt::Display) -> CallError {
    CallError::Read(
        EngineError::internal(format!("outbox storage: {e}")).with_retryable(true),
    )
}

fn too_many_swaps() -> Outcome {
    Outcome::Failed(
        EngineError::conflict("the repo kept changing; gave up after 3 attempts (TooManySwaps)")
            .with_detail("reason", "tooManySwaps")
            .with_retryable(true),
    )
}

fn conflict_with(message: &str, current: Option<&Current>) -> Outcome {
    let mut e = EngineError::conflict(message);
    if let Some(c) = current {
        e = e.with_detail("currentCid", c.cid.clone());
    }
    Outcome::Conflict(e)
}

/// Strips `createdAt` and `updatedAt` (comparisons "ignoring timestamps").
fn without_timestamps(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(m) = v.as_object_mut() {
        m.remove("createdAt");
        m.remove("updatedAt");
    }
    v
}

/// The runner of one write.
pub(crate) struct Runner<'a, H: Host> {
    host: &'a H,
    session: &'a Session,
    /// The PDS client (holds the account's write budget).
    pub pds: PdsClient<'a, H>,
    /// The write.
    pub w: PendingWrite,
    /// Confirmed write requests, applied to the store when the write is done.
    pub applied: Vec<AppliedWrite>,
}

impl<'a, H: Host> Runner<'a, H> {
    pub(crate) fn new(
        host: &'a H,
        session: &'a Session,
        w: PendingWrite,
        budget: crate::outbox::budget::BudgetRow,
    ) -> Self {
        Self {
            host,
            session,
            pds: PdsClient::new(host, session, budget),
            w,
            applied: Vec::new(),
        }
    }

    fn did(&self) -> &Did {
        &self.session.did
    }

    fn now_ms(&self) -> i64 {
        self.host.now_ms()
    }

    fn now(&self) -> Result<Datetime, CallError> {
        records::datetime_ms(self.now_ms()).map_err(rejected)
    }

    fn random_u64(&self) -> u64 {
        let mut b = [0u8; 8];
        self.host.fill_random(&mut b);
        u64::from_le_bytes(b)
    }

    /// Sleeps `uniform(200 ms, 800 ms)` before a swap retry.
    pub(crate) async fn jitter(&self) {
        let span = limits::SWAP_RETRY_JITTER_MAX_MS - limits::SWAP_RETRY_JITTER_MIN_MS;
        let ms = limits::SWAP_RETRY_JITTER_MIN_MS + self.random_u64() % (span + 1);
        self.host.sleep(ms).await;
    }

    /// Back-off before attempt `attempts + 1`: `min(5 min, 2 s · 2^(attempts − 1))` scaled by
    /// `uniform(0.8, 1.2)`.
    pub(crate) fn backoff_ms(&self, attempts: u32) -> i64 {
        let exp = attempts.saturating_sub(1).min(20);
        let base = BACKOFF_BASE_MS.saturating_mul(1 << exp).min(BACKOFF_MAX_MS);
        let scale = 800 + (self.random_u64() % 401) as i64; // per mille
        base.saturating_mul(scale) / 1000
    }

    /// Persists the write (still `sending`).
    pub(crate) async fn persist(&mut self) -> Result<(), CallError> {
        self.w.updated_at = self.now_ms();
        let bytes = row::encode(&self.w).map_err(storage)?;
        self.host
            .state()
            .write(vec![KvOp::Put(
                model::write_key(&self.w.account, self.w.seq),
                bytes,
            )])
            .await
            .map_err(storage)
    }

    fn op_uri(&self, i: usize) -> String {
        self.w.ops[i].uri(self.did().as_str())
    }

    fn op_value(&self, i: usize) -> Result<Value, CallError> {
        self.w.ops[i]
            .value_json()
            .filter(Value::is_object)
            .ok_or_else(|| invalid("the write has no record value"))
    }

    /// Records a confirmed write request.
    fn confirm(&mut self, wr: &Written, records: Vec<AppliedRecord>) {
        self.applied.push(AppliedWrite {
            account: self.did().clone(),
            pds: self.session.pds.as_str().trim_end_matches('/').to_owned(),
            rev: wr.commit.as_ref().map(|c| c.rev.clone()).unwrap_or_default(),
            commit: wr.commit.as_ref().map(|c| c.cid.clone()),
            sent_at: wr.sent_at,
            records,
        });
    }

    fn put_record(&self, i: usize, value: Value, cid: Option<String>) -> AppliedRecord {
        AppliedRecord::Put {
            uri: self.op_uri(i),
            cid: cid.unwrap_or_default(),
            value,
        }
    }

    fn set_result(&mut self, uri: String, cid: Option<String>) {
        self.w.results = vec![OpResult { uri, cid }];
    }

    // -----------------------------------------------------------------------------------------
    // Host capabilities (cache)

    async fn null_swap(&self) -> Option<bool> {
        RecordStore::new(self.host.cache())
            .host_caps(&self.session.origin())
            .await
            .ok()
            .flatten()
            .and_then(|h| h.null_swap)
    }

    async fn set_null_swap(&self, supported: bool) {
        let store = RecordStore::new(self.host.cache());
        let mut b = store.batch();
        let origin = self.session.origin();
        let mut h = b.host_caps(&origin).await.ok().flatten().unwrap_or_default();
        if h.null_swap == Some(supported) {
            return;
        }
        h.null_swap = Some(supported);
        if b.put_host_caps(&origin, &h).is_ok()
            && let Err(e) = b.commit().await
        {
            tracing::warn!(error = %e, "could not record the host's null-swap support");
        }
    }

    // -----------------------------------------------------------------------------------------
    // Blobs and size

    async fn chunks(&self, index: usize) -> Result<Vec<u8>, CallError> {
        let b = &self.w.blobs[index];
        let keys: Vec<Vec<u8>> = (0..b.chunks)
            .map(|c| model::blob_chunk_key(&self.w.account, self.w.seq, index as u32, c))
            .collect();
        let rows = self.host.state().get_many(keys).await.map_err(storage)?;
        let mut out = Vec::with_capacity(usize::try_from(b.size).unwrap_or(0));
        for r in rows {
            let r = r.ok_or_else(|| rejected(EngineError::internal("a blob chunk is missing")))?;
            let c: ChunkRow = row::decode(&r).map_err(storage)?;
            out.extend_from_slice(&c.b);
        }
        if out.len() as u64 != b.size || lessdugin_atp::cid::for_raw(&out).to_string() != b.cid {
            return Err(rejected(EngineError::internal(
                "a stored blob does not match its CID",
            )));
        }
        Ok(out)
    }

    /// Uploads `bytes` and replaces its placeholders in the op values with the returned object,
    /// verbatim (`data-model.md` §8.8). The returned `ref.$link` must equal the local CID.
    async fn upload(&mut self, data: &BlobData) -> Result<(), CallError> {
        let object = self.pds.upload_blob(data.bytes.clone(), &data.mime).await?;
        if blob::blob_link(&object) != Some(data.cid.as_str()) {
            return Err(rejected(
                EngineError::invalid_input("the PDS stored the blob under another CID")
                    .with_detail("kind", "mediaRejected")
                    .with_detail("cid", data.cid.clone()),
            ));
        }
        for op in &mut self.w.ops {
            if let Some(mut v) = op.value_json()
                && blob::substitute_blob(&mut v, &data.cid, &object) > 0
            {
                op.set_value(&v);
            }
        }
        Ok(())
    }

    /// Uploads the write's pending blobs (just before its record write, §8.8), then persists
    /// the substituted values so that recovery compares the right CIDs.
    pub(crate) async fn upload_blobs(&mut self) -> Result<(), CallError> {
        if self.w.blobs.iter().all(|b| b.uploaded) {
            return Ok(());
        }
        for i in 0..self.w.blobs.len() {
            if self.w.blobs[i].uploaded {
                continue;
            }
            let bytes = self.chunks(i).await?;
            let data = BlobData {
                cid: self.w.blobs[i].cid.clone(),
                mime: self.w.blobs[i].mime.clone(),
                bytes,
            };
            self.upload(&data).await?;
            self.w.blobs[i].uploaded = true;
        }
        self.persist().await
    }

    /// The size check (§8.9): while `measure()` exceeds 900,000 bytes, spills the largest inline
    /// spillable field of the ops `indices` (uploading it at once) and persists. `Ok(false)` if
    /// nothing is left to spill.
    async fn fit<F>(&mut self, indices: &[usize], measure: F) -> Result<bool, CallError>
    where
        F: Fn(&Self) -> Result<usize, CallError>,
    {
        loop {
            if measure(self)? <= limits::WRITE_BODY_MAX_BYTES {
                return Ok(true);
            }
            let values: Vec<(String, Value)> = indices
                .iter()
                .map(|&i| Ok((self.w.ops[i].collection.clone(), self.op_value(i)?)))
                .collect::<Result<_, CallError>>()?;
            let refs: Vec<(&str, &Value)> = values.iter().map(|(c, v)| (c.as_str(), v)).collect();
            let Some((k, field, _)) = blob::largest_inline(&refs) else {
                return Ok(false);
            };
            let i = indices[k];
            let mut v = values[k].1.clone();
            let excerpt = self.w.ops[i].excerpt.clone();
            let Some(data) =
                blob::spill(&mut v, &self.w.ops[i].collection, field, excerpt.as_deref())
                    .map_err(rejected)?
            else {
                return Ok(false);
            };
            self.w.ops[i].set_value(&v);
            self.upload(&data).await?;
            self.persist().await?;
        }
    }

    fn too_large(edit: bool) -> Outcome {
        let mut e = EngineError::new(
            ErrorCode::TooLarge,
            "the write is larger than 900,000 bytes even with every long body spilled",
        );
        if edit {
            e = e.with_detail("offerNoHistory", true);
        }
        Outcome::Failed(e)
    }

    // -----------------------------------------------------------------------------------------
    // Recovery

    /// `data-model.md` §8.2: a write found in `sending` (or one that may have reached the PDS)
    /// is done if each target's current CID equals the CID of its intended value. Protocols
    /// that compute their ops when sent (opinions, deletes with revisions, redaction, adoption)
    /// are idempotent and simply run again.
    pub(crate) async fn already_applied(&mut self) -> Result<bool, CallError> {
        let targets: Vec<usize> = match &self.w.intent {
            Intent::Create
            | Intent::FirstContent
            | Intent::CreateForum
            | Intent::CreateIfAbsent { .. }
            | Intent::Edit { .. }
            | Intent::Put { .. }
            | Intent::DeleteRecord => vec![0],
            _ => return Ok(false),
        };
        if self.w.ops.is_empty() || self.w.blobs.iter().any(|b| !b.uploaded) {
            return Ok(false);
        }
        let mut results = Vec::new();
        for i in targets {
            let op = self.w.ops[i].clone();
            let cur = self.pds.get(&op.collection, &op.rkey).await?;
            match (op.action, cur) {
                (OpAction::Delete, None) => results.push(OpResult {
                    uri: self.op_uri(i),
                    cid: None,
                }),
                (OpAction::Create | OpAction::Update, Some(c))
                    if Some(c.cid.as_str())
                        == records::record_cid(&self.op_value(i)?).ok().as_deref() =>
                {
                    results.push(OpResult {
                        uri: self.op_uri(i),
                        cid: Some(c.cid),
                    });
                }
                _ => return Ok(false),
            }
        }
        self.w.results = results;
        Ok(true)
    }

    // -----------------------------------------------------------------------------------------
    // Dispatch

    /// Runs the write's protocol.
    pub(crate) async fn run(&mut self) -> Result<Outcome, CallError> {
        if !matches!(self.w.intent, Intent::Opinion { .. } | Intent::Adopt { .. })
            && self.w.ops.is_empty()
        {
            return Err(invalid("the write has no ops"));
        }
        self.upload_blobs().await?;
        match self.w.intent.clone() {
            Intent::Create => self.create().await,
            Intent::FirstContent => self.first_content(false).await,
            Intent::CreateForum => self.first_content(true).await,
            Intent::CreateIfAbsent { on_exists } => self.create_if_absent(on_exists).await,
            Intent::Edit {
                base,
                summary,
                revision_rkey,
            } => self.edit(&base, summary.as_deref(), &revision_rkey).await,
            Intent::Put { base } => self.put(&base).await,
            Intent::DeleteContent => self.delete_content().await,
            Intent::DeleteRecord => self.delete_record().await,
            Intent::RemoveModAction { revision_rkey } => {
                self.remove_mod_action(&revision_rkey).await
            }
            Intent::Redact => self.redact().await,
            Intent::Opinion { .. } => self.opinion().await,
            Intent::Adopt { .. } => self.adopt().await,
        }
    }

    // -----------------------------------------------------------------------------------------
    // createRecord(rkey): posts, comments, sequences, wikiRevisions

    async fn create(&mut self) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        let did = self.did().clone();
        let fits = self
            .fit(&[0], |s| {
                let v = s.op_value(0)?;
                Ok(create_body_len(&did, &s.w.ops[0], &v))
            })
            .await?;
        if !fits {
            return Ok(Self::too_large(false));
        }
        let value = self.op_value(0)?;
        let intended = records::record_cid(&value).map_err(rejected)?;
        match self
            .pds
            .create_record(&op.collection, &op.rkey, &value)
            .await
        {
            Ok(wr) => {
                let rec = self.put_record(0, value, wr.cid.clone());
                self.confirm(&wr, vec![rec]);
                self.set_result(self.op_uri(0), wr.cid);
                Ok(Outcome::Done)
            }
            Err(CallError::Ambiguous(e)) => {
                // The key may have been written: re-read before deciding.
                match self.pds.get(&op.collection, &op.rkey).await? {
                    Some(c) if c.cid == intended => {
                        self.set_result(self.op_uri(0), Some(c.cid));
                        Ok(Outcome::Done)
                    }
                    Some(c) => Ok(conflict_with("the record key is taken", Some(&c))),
                    None => Err(CallError::Ambiguous(e)),
                }
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------------------------
    // First content in a forum / create forum (§8.4)

    async fn first_content(&mut self, forum: bool) -> Result<Outcome, CallError> {
        if self.w.ops.len() < 2 {
            return Err(invalid("first content needs the membership op"));
        }
        let content = self.w.ops[0].clone();
        let membership = self.w.ops[1].clone();
        let mem_value = self.op_value(1)?;
        let mut swaps = 0;
        let mut retried_ambiguous = false;
        loop {
            let value = self.op_value(0)?;
            let intended = records::record_cid(&value).map_err(rejected)?;
            // fresh reads: the content (recovery, taken slug) and the membership
            match self.pds.get(&content.collection, &content.rkey).await? {
                Some(c) if c.cid == intended => {
                    self.set_result(self.op_uri(0), Some(c.cid));
                    return Ok(Outcome::Done);
                }
                Some(c) if forum => {
                    return Ok(conflict_with("a forum with this slug exists", Some(&c)));
                }
                Some(c) => return Ok(conflict_with("the record key is taken", Some(&c))),
                None => {}
            }
            let have_membership = self
                .pds
                .get(&membership.collection, &membership.rkey)
                .await?
                .is_some();
            let head = self.pds.head().await?;
            let build = |s: &Self| -> Result<Vec<BatchOp>, CallError> {
                let mut ops = Vec::new();
                if !have_membership {
                    ops.push(BatchOp::Create {
                        collection: membership.collection.clone(),
                        rkey: membership.rkey.clone(),
                        value: mem_value.clone(),
                    });
                }
                ops.push(BatchOp::Create {
                    collection: content.collection.clone(),
                    rkey: content.rkey.clone(),
                    value: s.op_value(0)?,
                });
                Ok(ops)
            };
            let head_cid = head.cid.clone();
            let fits = self
                .fit(&[0], |s| {
                    let ops = build(s)?;
                    Ok(body_len(&s.pds.apply_writes_body(&ops, Some(&head_cid))))
                })
                .await?;
            if !fits {
                return Ok(Self::too_large(false));
            }
            let ops = build(self)?;
            match self.pds.apply_writes(&ops, Some(&head.cid)).await {
                Ok(wr) => {
                    let recs = self.batch_records(&ops, &wr);
                    let cid = wr.results.last().cloned().flatten();
                    self.confirm(&wr, recs);
                    self.set_result(self.op_uri(0), cid);
                    return Ok(Outcome::Done);
                }
                Err(CallError::InvalidSwap) => {
                    swaps += 1;
                    if swaps >= limits::SWAP_RETRY_MAX {
                        return Ok(too_many_swaps());
                    }
                    self.jitter().await;
                }
                Err(CallError::Ambiguous(e)) => {
                    // An HTTP 500 is ambiguous (the membership may have existed): re-read the
                    // membership and the head, and retry once without it if it now exists.
                    if retried_ambiguous {
                        return Err(CallError::Ambiguous(e));
                    }
                    retried_ambiguous = true;
                    let now_has = self
                        .pds
                        .get(&membership.collection, &membership.rkey)
                        .await?
                        .is_some();
                    let applied = self
                        .pds
                        .get(&content.collection, &content.rkey)
                        .await?
                        .is_some();
                    if !(now_has && !have_membership) && !applied && !forum {
                        return Err(CallError::Ambiguous(e));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// The applied records of a confirmed `applyWrites`.
    fn batch_records(&self, ops: &[BatchOp], wr: &Written) -> Vec<AppliedRecord> {
        let did = self.did().as_str();
        ops.iter()
            .enumerate()
            .map(|(i, op)| {
                let (coll, rkey) = op.target();
                let uri = format!("at://{did}/{coll}/{rkey}");
                match op.value() {
                    Some(v) => AppliedRecord::Put {
                        uri,
                        cid: wr.results.get(i).cloned().flatten().unwrap_or_default(),
                        value: v.clone(),
                    },
                    None => AppliedRecord::Delete { uri },
                }
            })
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // Create-if-absent (ADR 0005)

    async fn create_if_absent(&mut self, on_exists: OnExists) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        let mut null_swap = self.null_swap().await;
        let did = self.did().clone();
        let fits = self
            .fit(&[0], |s| {
                let v = s.op_value(0)?;
                Ok(create_body_len(&did, &s.w.ops[0], &v))
            })
            .await?;
        if !fits {
            return Ok(Self::too_large(false));
        }
        let value = self.op_value(0)?;
        let intended = records::record_cid(&value).map_err(rejected)?;
        let mut tries = 0;
        while tries < limits::SWAP_RETRY_MAX {
            tries += 1;
            let use_put = null_swap != Some(false);
            let r = if use_put {
                self.pds
                    .put_record(&op.collection, &op.rkey, &value, Swap::Null)
                    .await
            } else {
                self.pds
                    .create_record(&op.collection, &op.rkey, &value)
                    .await
            };
            match r {
                Ok(wr) => {
                    if use_put && null_swap.is_none() {
                        self.set_null_swap(true).await;
                    }
                    let rec = self.put_record(0, value.clone(), wr.cid.clone());
                    self.confirm(&wr, vec![rec]);
                    self.set_result(self.op_uri(0), wr.cid);
                    return Ok(Outcome::Done);
                }
                // The typed "exists" signal.
                Err(CallError::InvalidSwap) if use_put => {
                    match self.pds.get(&op.collection, &op.rkey).await? {
                        None => self.jitter().await, // deleted meanwhile: try again
                        Some(c) => return self.exists(on_exists, &intended, c).await,
                    }
                }
                // A host that answers a null swap with another 400 does not support it.
                Err(CallError::Rejected { status: 400, .. }) if use_put && null_swap.is_none() => {
                    null_swap = Some(false);
                    self.set_null_swap(false).await;
                    tries -= 1;
                }
                Err(CallError::Ambiguous(e)) => {
                    match self.pds.get(&op.collection, &op.rkey).await? {
                        None => return Err(CallError::Ambiguous(e)),
                        Some(c) => return self.exists(on_exists, &intended, c).await,
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(too_many_swaps())
    }

    /// The key of a create-if-absent exists with `cur`.
    async fn exists(
        &mut self,
        on_exists: OnExists,
        intended: &str,
        cur: Current,
    ) -> Result<Outcome, CallError> {
        if cur.cid == intended {
            self.set_result(self.op_uri(0), Some(cur.cid));
            return Ok(Outcome::Done);
        }
        match on_exists {
            OnExists::Succeed => {
                self.set_result(self.op_uri(0), Some(cur.cid));
                Ok(Outcome::Done)
            }
            OnExists::Conflict => Ok(conflict_with("the record exists", Some(&cur))),
            OnExists::Update { revision_rkey } => {
                // Continue as an update through the edit protocol (modAction create, §8.4):
                // the existing record keeps its createdAt.
                let mut v = self.op_value(0)?;
                if let (Some(m), Some(created)) = (v.as_object_mut(), cur.value.get("createdAt"))
                {
                    m.insert("createdAt".into(), created.clone());
                    m.insert(
                        "updatedAt".into(),
                        Value::String(self.now()?.as_str().to_owned()),
                    );
                }
                let op = &self.w.ops[0];
                self.w.ops[0] = Op::update(&op.collection.clone(), &op.rkey.clone(), &v);
                self.w.intent = Intent::Edit {
                    base: cur.cid.clone(),
                    summary: None,
                    revision_rkey: revision_rkey.clone(),
                };
                self.persist().await?;
                self.edit(&cur.cid, None, &revision_rkey).await
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // The edit protocol (§8.3)

    async fn edit(
        &mut self,
        base: &str,
        summary: Option<&str>,
        revision_rkey: &str,
    ) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        let target = self.op_uri(0);
        let target_uri = records::at_uri(&target).map_err(rejected)?;
        let mut last_ambiguous: Option<EngineError> = None;
        for _ in 0..limits::SWAP_RETRY_MAX {
            let head = self.pds.head().await?;
            let Some(cur) = self.pds.get(&op.collection, &op.rkey).await? else {
                return Ok(conflict_with("the record no longer exists", None));
            };
            let new_cid = records::record_cid(&self.op_value(0)?).map_err(rejected)?;
            if cur.cid == new_cid {
                self.set_result(target.clone(), Some(cur.cid));
                return Ok(Outcome::Done);
            }
            if cur.cid != base {
                return Ok(conflict_with(
                    "the record changed since it was edited",
                    Some(&cur),
                ));
            }
            let record = cur
                .value
                .as_object()
                .cloned()
                .ok_or_else(|| invalid("the current record is not an object"))?;
            let rev = records::revision(
                StrongRef::new(target_uri.clone(), cur.cid.clone()),
                RevisionOp::Update,
                record,
                summary,
                &self.now()?,
                revision_rkey,
            )
            .map_err(rejected)?;
            records::check(self.did(), rev.collection, &rev.rkey, &rev.value, true)
                .map_err(rejected)?;
            let build = |s: &Self| -> Result<Vec<BatchOp>, CallError> {
                Ok(vec![
                    BatchOp::Create {
                        collection: rev.collection.to_owned(),
                        rkey: rev.rkey.clone(),
                        value: rev.value.clone(),
                    },
                    BatchOp::Update {
                        collection: op.collection.clone(),
                        rkey: op.rkey.clone(),
                        value: s.op_value(0)?,
                    },
                ])
            };
            let head_cid = head.cid.clone();
            let fits = self
                .fit(&[0], |s| {
                    let ops = build(s)?;
                    Ok(body_len(&s.pds.apply_writes_body(&ops, Some(&head_cid))))
                })
                .await?;
            if !fits {
                return Ok(Self::too_large(true));
            }
            let ops = build(self)?;
            match self.pds.apply_writes(&ops, Some(&head.cid)).await {
                Ok(wr) => {
                    let recs = self.batch_records(&ops, &wr);
                    let cid = wr.results.get(1).cloned().flatten();
                    self.confirm(&wr, recs);
                    self.set_result(target, cid);
                    return Ok(Outcome::Done);
                }
                Err(CallError::InvalidSwap) => {
                    last_ambiguous = None;
                    self.jitter().await;
                }
                Err(CallError::Ambiguous(e)) => {
                    // re-read (head and target) before anything is sent again
                    last_ambiguous = Some(e);
                    self.jitter().await;
                }
                Err(e) => return Err(e),
            }
        }
        match last_ambiguous {
            Some(e) => Err(CallError::Ambiguous(e)),
            None => Ok(too_many_swaps()),
        }
    }

    // -----------------------------------------------------------------------------------------
    // putRecord(swapRecord = base): --no-history, profile updates

    async fn put(&mut self, base: &str) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        let did = self.did().clone();
        let fits = self
            .fit(&[0], |s| {
                let v = s.op_value(0)?;
                Ok(create_body_len(&did, &s.w.ops[0], &v) + base.len() + 16)
            })
            .await?;
        if !fits {
            return Ok(Self::too_large(false));
        }
        let value = self.op_value(0)?;
        let new_cid = records::record_cid(&value).map_err(rejected)?;
        if new_cid == base {
            // a no-op update is skipped (ADR 0005)
            self.set_result(self.op_uri(0), Some(new_cid));
            return Ok(Outcome::Done);
        }
        match self
            .pds
            .put_record(&op.collection, &op.rkey, &value, Swap::Cid(base))
            .await
        {
            Ok(wr) => {
                let rec = self.put_record(0, value, wr.cid.clone());
                self.confirm(&wr, vec![rec]);
                self.set_result(self.op_uri(0), wr.cid);
                Ok(Outcome::Done)
            }
            Err(CallError::InvalidSwap) => {
                match self.pds.get(&op.collection, &op.rkey).await? {
                    Some(c) if c.cid == new_cid => {
                        self.set_result(self.op_uri(0), Some(c.cid));
                        Ok(Outcome::Done)
                    }
                    c => Ok(conflict_with(
                        "the record changed since it was edited",
                        c.as_ref(),
                    )),
                }
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------------------------
    // deleteRecord: leave, wikiRevision, single records

    async fn delete_record(&mut self) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        match self
            .pds
            .delete_record(&op.collection, &op.rkey, op.swap.as_deref())
            .await
        {
            Ok(wr) => {
                let uri = self.op_uri(0);
                self.confirm(&wr, vec![AppliedRecord::Delete { uri: uri.clone() }]);
                self.set_result(uri, None);
                Ok(Outcome::Done)
            }
            Err(CallError::InvalidSwap) => {
                let cur = self.pds.get(&op.collection, &op.rkey).await?;
                match cur {
                    None => {
                        self.set_result(self.op_uri(0), None);
                        Ok(Outcome::Done)
                    }
                    c => Ok(conflict_with("the record changed", c.as_ref())),
                }
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------------------------
    // The delete protocol (§8.7) and redaction

    /// Sends `ops` in chunks of ≤ 200 with `applyWrites` (no `swapCommit`), in order.
    /// `Ok(false)` on an ambiguous failure (the caller re-lists and recomputes).
    async fn send_chunks(&mut self, ops: &[BatchOp]) -> Result<Option<EngineError>, CallError> {
        for chunk in ops.chunks(limits::APPLYWRITES_MAX_OPS) {
            match self.pds.apply_writes(chunk, None).await {
                Ok(wr) => {
                    let recs = self.batch_records(chunk, &wr);
                    self.confirm(&wr, recs);
                }
                Err(CallError::Ambiguous(e)) => return Ok(Some(e)),
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    async fn delete_content(&mut self) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        let target = self.op_uri(0);
        let p = keys::revision_prefix(NS, &target)
            .map_err(|e| invalid(format!("{target} has no revisions to purge: {e}")))?;
        let mut ambiguous = 0;
        loop {
            let present = self.pds.get(&op.collection, &op.rkey).await?.is_some();
            let revs = self.own_revisions_of(&p, &target).await?;
            let mut ops = Vec::with_capacity(revs.len() + 1);
            if present {
                ops.push(BatchOp::Delete {
                    collection: op.collection.clone(),
                    rkey: op.rkey.clone(),
                });
            }
            ops.extend(revs.into_iter().map(|rkey| BatchOp::Delete {
                collection: nsid::REVISION.to_owned(),
                rkey,
            }));
            match self.send_chunks(&ops).await? {
                None => {
                    self.set_result(target, None);
                    return Ok(Outcome::Done);
                }
                Some(e) => {
                    ambiguous += 1;
                    if ambiguous >= limits::SWAP_RETRY_MAX {
                        return Err(CallError::Ambiguous(e));
                    }
                    self.jitter().await;
                }
            }
        }
    }

    /// The own revisions of `subject`: rkey exactly `P + "-" + TID` and `subject.uri` equal (the
    /// prefix alone is not enough, §3.2), ascending.
    async fn own_revisions_of(&mut self, p: &str, subject: &str) -> Result<Vec<String>, CallError> {
        let prefix = format!("{p}-");
        let rows = self
            .pds
            .list_prefix(nsid::REVISION, &prefix, None)
            .await?;
        let mut out: Vec<String> = rows
            .into_iter()
            .filter(|r| {
                r.rkey
                    .strip_prefix(&prefix)
                    .is_some_and(keys::is_tid)
                    && r.value
                        .get("subject")
                        .and_then(|s| s.get("uri"))
                        .and_then(Value::as_str)
                        == Some(subject)
            })
            .map(|r| r.rkey)
            .collect();
        out.sort();
        Ok(out)
    }

    async fn redact(&mut self) -> Result<Outcome, CallError> {
        let wanted: BTreeSet<String> = self
            .w
            .ops
            .iter()
            .filter(|o| o.action == OpAction::Delete && o.collection == nsid::REVISION)
            .map(|o| o.rkey.clone())
            .collect();
        let prefixes: BTreeSet<String> = wanted
            .iter()
            .filter_map(|k| k.rsplit_once('-').map(|(p, _)| format!("{p}-")))
            .collect();
        let mut ambiguous = 0;
        loop {
            let mut present = BTreeSet::new();
            for p in &prefixes {
                for r in self.pds.list_prefix(nsid::REVISION, p, None).await? {
                    present.insert(r.rkey);
                }
            }
            let ops: Vec<BatchOp> = wanted
                .iter()
                .filter(|k| present.contains(*k))
                .map(|k| BatchOp::Delete {
                    collection: nsid::REVISION.to_owned(),
                    rkey: k.clone(),
                })
                .collect();
            match self.send_chunks(&ops).await? {
                None => {
                    self.w.results = self
                        .w
                        .ops
                        .iter()
                        .map(|o| OpResult {
                            uri: o.uri(self.did().as_str()),
                            cid: None,
                        })
                        .collect();
                    return Ok(Outcome::Done);
                }
                Some(e) => {
                    ambiguous += 1;
                    if ambiguous >= limits::SWAP_RETRY_MAX {
                        return Err(CallError::Ambiguous(e));
                    }
                    self.jitter().await;
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // ModAction removal (§8.4)

    async fn remove_mod_action(&mut self, revision_rkey: &str) -> Result<Outcome, CallError> {
        let op = self.w.ops[0].clone();
        let target = self.op_uri(0);
        let target_uri = records::at_uri(&target).map_err(rejected)?;
        let mut last_ambiguous = None;
        for _ in 0..limits::SWAP_RETRY_MAX {
            let head = self.pds.head().await?;
            let Some(cur) = self.pds.get(&op.collection, &op.rkey).await? else {
                self.set_result(target, None);
                return Ok(Outcome::Done);
            };
            let record = cur
                .value
                .as_object()
                .cloned()
                .ok_or_else(|| invalid("the current record is not an object"))?;
            let rev = records::revision(
                StrongRef::new(target_uri.clone(), cur.cid.clone()),
                RevisionOp::Delete,
                record,
                None,
                &self.now()?,
                revision_rkey,
            )
            .map_err(rejected)?;
            let ops = vec![
                BatchOp::Create {
                    collection: rev.collection.to_owned(),
                    rkey: rev.rkey.clone(),
                    value: rev.value.clone(),
                },
                BatchOp::Delete {
                    collection: op.collection.clone(),
                    rkey: op.rkey.clone(),
                },
            ];
            match self.pds.apply_writes(&ops, Some(&head.cid)).await {
                Ok(wr) => {
                    let recs = self.batch_records(&ops, &wr);
                    self.confirm(&wr, recs);
                    self.set_result(target, None);
                    return Ok(Outcome::Done);
                }
                Err(CallError::InvalidSwap) => {
                    last_ambiguous = None;
                    self.jitter().await;
                }
                Err(CallError::Ambiguous(e)) => {
                    last_ambiguous = Some(e);
                    self.jitter().await;
                }
                Err(e) => return Err(e),
            }
        }
        match last_ambiguous {
            Some(e) => Err(CallError::Ambiguous(e)),
            None => Ok(too_many_swaps()),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Opinion writes (§8.5)

    async fn opinion(&mut self) -> Result<Outcome, CallError> {
        let Intent::Opinion {
            subject,
            subject_cid,
            forum,
            steps,
            ..
        } = self.w.intent.clone()
        else {
            return Err(invalid("not an opinion write"));
        };
        let subject_uri = records::at_uri(&subject).map_err(rejected)?;
        let forum_uri = records::at_uri(&forum).map_err(rejected)?;
        let intents = steps
            .iter()
            .map(lexicon_intent)
            .collect::<Result<Vec<_>, _>>()
            .map_err(rejected)?;
        let rkey = records::opinion_rkey(&forum, &subject).map_err(rejected)?;
        if self.w.ops.is_empty() {
            self.w.ops = vec![Op {
                action: OpAction::Update,
                collection: nsid::OPINION.to_owned(),
                rkey: rkey.clone(),
                value: None,
                swap: None,
                excerpt: None,
            }];
        }
        let uri = self.op_uri(0);
        let mut null_swap = self.null_swap().await;
        let mut tries = 0;
        while tries < limits::SWAP_RETRY_MAX {
            tries += 1;
            let cur = self.pds.get(nsid::OPINION, &rkey).await?;
            self.note_prior(cur.as_ref()).await?;
            let cur_op: Option<Opinion> = match &cur {
                None => None,
                Some(c) => Some(serde_json::from_value(c.value.clone()).map_err(|e| {
                    rejected(EngineError::conflict(format!(
                        "the current opinion record cannot be read: {e}"
                    )))
                })?),
            };
            let now = self.now()?;
            let new = Opinion::apply(
                cur_op.as_ref(),
                &subject_uri,
                &forum_uri,
                &subject_cid,
                &intents,
                &now,
            );
            match (&cur, &cur_op) {
                (Some(c), Some(co)) if new.eq_ignoring_updated_at(co) => {
                    self.set_result(uri, Some(c.cid.clone()));
                    return Ok(Outcome::Done);
                }
                (None, _) if new.is_empty() => {
                    self.set_result(uri, None);
                    return Ok(Outcome::Done);
                }
                _ => {}
            }
            let built = records::opinion(&new).map_err(rejected)?;
            records::check(self.did(), built.collection, &built.rkey, &built.value, false)
                .map_err(rejected)?;
            let (r, fallback) = match &cur {
                None if null_swap != Some(false) => (
                    self.pds
                        .put_record(nsid::OPINION, &rkey, &built.value, Swap::Null)
                        .await,
                    false,
                ),
                None => (
                    self.pds
                        .create_record(nsid::OPINION, &rkey, &built.value)
                        .await,
                    true,
                ),
                Some(c) if new.is_empty() => (
                    self.pds
                        .delete_record(nsid::OPINION, &rkey, Some(&c.cid))
                        .await,
                    false,
                ),
                Some(c) => (
                    self.pds
                        .put_record(nsid::OPINION, &rkey, &built.value, Swap::Cid(&c.cid))
                        .await,
                    false,
                ),
            };
            match r {
                Ok(wr) => {
                    let deleted = cur.is_some() && new.is_empty();
                    let rec = if deleted {
                        AppliedRecord::Delete { uri: uri.clone() }
                    } else {
                        AppliedRecord::Put {
                            uri: uri.clone(),
                            cid: wr.cid.clone().unwrap_or_default(),
                            value: built.value.clone(),
                        }
                    };
                    if cur.is_none() && !fallback && null_swap.is_none() {
                        self.set_null_swap(true).await;
                    }
                    self.confirm(&wr, vec![rec]);
                    self.set_result(uri, if deleted { None } else { wr.cid });
                    return Ok(Outcome::Done);
                }
                Err(CallError::InvalidSwap) => self.jitter().await,
                Err(CallError::Rejected { status: 400, .. })
                    if cur.is_none() && !fallback && null_swap.is_none() =>
                {
                    null_swap = Some(false);
                    self.set_null_swap(false).await;
                    tries -= 1;
                }
                // createRecord fallback: a 5xx after which the key exists means "exists"
                Err(CallError::Ambiguous(e)) if fallback => {
                    if self.pds.get(nsid::OPINION, &rkey).await?.is_none() {
                        return Err(CallError::Ambiguous(e));
                    }
                    self.jitter().await;
                }
                // re-read and re-apply (the function is idempotent)
                Err(CallError::Ambiguous(_)) => self.jitter().await,
                Err(e) => return Err(e),
            }
        }
        Ok(too_many_swaps())
    }

    /// Records the opinion before this write (the base of `undo` after sending), once.
    async fn note_prior(&mut self, cur: Option<&Current>) -> Result<(), CallError> {
        if let Intent::Opinion {
            prior, prior_known, ..
        } = &mut self.w.intent
            && !*prior_known
        {
            *prior = cur.map(|c| c.value.to_string());
            *prior_known = true;
            self.persist().await?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // mod adopt (§8.6)

    async fn adopt(&mut self) -> Result<Outcome, CallError> {
        let Intent::Adopt {
            forum,
            former,
            former_pds,
            role,
            revision_tid,
            ..
        } = self.w.intent.clone()
        else {
            return Err(invalid("not an adopt write"));
        };
        let fk = keys::fk(&forum).map_err(|e| invalid(format!("forum {forum}: {e}")))?;
        let prefix = format!("{fk}-");
        let former_session = Session::parse(&former, &former_pds).map_err(rejected)?;
        let mut last_ambiguous = None;
        for _ in 0..limits::SWAP_RETRY_MAX {
            let mut report = Vec::new();
            // fresh, complete listings
            let originals = {
                let mut reader = PdsClient::new(self.host, &former_session, Default::default());
                let rows = reader
                    .list_prefix(nsid::MOD_ACTION, &prefix, Some(&LexiconValidator))
                    .await?;
                self.pds.requests += reader.requests;
                rows
            };
            let own: BTreeMap<String, Current> = self
                .pds
                .list_prefix(nsid::MOD_ACTION, &prefix, None)
                .await?
                .into_iter()
                .map(|c| (c.rkey.clone(), c))
                .collect();
            let now = self.now()?;
            let now_us = now.timestamp_micros();
            let mut groups: Vec<Vec<BatchOp>> = Vec::new();
            let mut originals: Vec<Current> = originals
                .into_iter()
                .filter(|o| o.valid && is_adoptable(&o.rkey, &o.value, now_us))
                .collect();
            originals.sort_by(|a, b| a.rkey.cmp(&b.rkey));
            for o in originals {
                let o_uri = format!("at://{former}/{}/{}", nsid::MOD_ACTION, o.rkey);
                if !role_may_issue(&role, &o.value) {
                    report.push(format!("cannot adopt {o_uri}: not allowed for a {role}"));
                    continue;
                }
                let mut v = o.value.clone();
                let Some(m) = v.as_object_mut() else { continue };
                m.insert(
                    "adopts".into(),
                    serde_json::json!({"uri": o_uri, "cid": o.cid}),
                );
                m.insert("createdAt".into(), Value::String(now.as_str().to_owned()));
                m.remove("updatedAt");
                let k = o.rkey.clone();
                match own.get(&k) {
                    None => {
                        records::check(self.did(), nsid::MOD_ACTION, &k, &v, true)
                            .map_err(rejected)?;
                        groups.push(vec![BatchOp::Create {
                            collection: nsid::MOD_ACTION.to_owned(),
                            rkey: k,
                            value: v,
                        }]);
                    }
                    Some(mine)
                        if mine
                            .value
                            .get("adopts")
                            .and_then(|a| a.get("uri"))
                            .and_then(Value::as_str)
                            == Some(o_uri.as_str()) =>
                    {
                        if without_timestamps(&mine.value) == without_timestamps(&v) {
                            continue;
                        }
                        if let (Some(m), Some(created)) =
                            (v.as_object_mut(), mine.value.get("createdAt"))
                        {
                            m.insert("createdAt".into(), created.clone());
                            m.insert("updatedAt".into(), Value::String(now.as_str().to_owned()));
                        }
                        let mine_uri = format!("at://{}/{}/{}", self.did(), nsid::MOD_ACTION, k);
                        let rk = records::revision_rkey(&mine_uri, &revision_tid).map_err(rejected)?;
                        let rev = records::revision(
                            StrongRef::new(
                                records::at_uri(&mine_uri).map_err(rejected)?,
                                mine.cid.clone(),
                            ),
                            RevisionOp::Update,
                            mine.value.as_object().cloned().unwrap_or_default(),
                            None,
                            &now,
                            &rk,
                        )
                        .map_err(rejected)?;
                        records::check(self.did(), nsid::MOD_ACTION, &k, &v, false)
                            .map_err(rejected)?;
                        groups.push(vec![
                            BatchOp::Create {
                                collection: rev.collection.to_owned(),
                                rkey: rev.rkey,
                                value: rev.value,
                            },
                            BatchOp::Update {
                                collection: nsid::MOD_ACTION.to_owned(),
                                rkey: k,
                                value: v,
                            },
                        ]);
                    }
                    Some(_) => report.push(format!("kept own assertion for {o_uri}")),
                }
            }
            // batches of ≤ 200 ops; a revision and its update are never split
            let mut batches: Vec<Vec<BatchOp>> = Vec::new();
            for g in groups {
                match batches.last_mut() {
                    Some(b) if b.len() + g.len() <= limits::APPLYWRITES_MAX_OPS => b.extend(g),
                    _ => batches.push(g),
                }
            }
            let mut retry = false;
            for b in &batches {
                let head = self.pds.head().await?;
                match self.pds.apply_writes(b, Some(&head.cid)).await {
                    Ok(wr) => {
                        let recs = self.batch_records(b, &wr);
                        self.confirm(&wr, recs);
                    }
                    Err(CallError::InvalidSwap) => {
                        last_ambiguous = None;
                        retry = true;
                        break;
                    }
                    Err(CallError::Ambiguous(e)) => {
                        last_ambiguous = Some(e);
                        retry = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
            if retry {
                // re-list the own range and recompute the remaining batches
                self.jitter().await;
                continue;
            }
            if let Intent::Adopt { report: r, .. } = &mut self.w.intent {
                *r = report;
            }
            self.w.results = vec![OpResult {
                uri: forum.clone(),
                cid: None,
            }];
            return Ok(Outcome::Done);
        }
        match last_ambiguous {
            Some(e) => Err(CallError::Ambiguous(e)),
            None => Ok(too_many_swaps()),
        }
    }
}

/// The JSON body length of a `createRecord` / `putRecord` of `value` (for the size check).
fn create_body_len(did: &Did, op: &Op, value: &Value) -> usize {
    body_len(&serde_json::json!({
        "repo": did.as_str(),
        "collection": op.collection,
        "rkey": op.rkey,
        "record": value,
        "swapRecord": Value::Null,
    }))
}

fn body_len(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

/// Maps a persisted step to the lexicon intent (unknown values are refused).
pub(crate) fn lexicon_intent(s: &OpinionStep) -> Result<OpinionIntent, EngineError> {
    fn known<T>(v: Option<&str>, parse: fn(&str) -> Option<T>, what: &str) -> Result<Option<T>, EngineError> {
        v.map(|x| {
            parse(x).ok_or_else(|| EngineError::invalid_input(format!("unknown {what} value {x:?}")))
        })
        .transpose()
    }
    let quote = |q: &Option<String>| -> Result<Option<TextQuote>, EngineError> {
        q.as_deref()
            .map(|j| {
                serde_json::from_str(j)
                    .map_err(|e| EngineError::invalid_input(format!("react quote: {e}")))
            })
            .transpose()
    };
    Ok(match s {
        OpinionStep::Karma { value } => {
            OpinionIntent::Karma(known(value.as_deref(), VoteValue::parse, "karma")?)
        }
        OpinionStep::Agreement { value } => {
            OpinionIntent::Agreement(known(value.as_deref(), Agreement::parse, "agreement")?)
        }
        OpinionStep::React {
            id,
            stance,
            quote: q,
            remove,
        } => {
            if !keys::is_slug(id) {
                return Err(EngineError::invalid_input(format!("react id {id:?} is not a slug")));
            }
            if *remove {
                OpinionIntent::RemoveReact {
                    react: id.clone(),
                    quote: quote(q)?,
                }
            } else {
                OpinionIntent::AddReact {
                    react: id.clone(),
                    quote: quote(q)?,
                    stance: Stance::parse(stance).ok_or_else(|| {
                        EngineError::invalid_input(format!("unknown stance {stance:?}"))
                    })?,
                }
            }
        }
        OpinionStep::Tag { slug, value } => {
            if !keys::is_slug(slug) {
                return Err(EngineError::invalid_input(format!("tag {slug:?} is not a slug")));
            }
            OpinionIntent::Tag {
                tag: slug.clone(),
                value: known(value.as_deref(), VoteValue::parse, "tag vote")?,
            }
        }
    })
}

/// The inverse of `steps` relative to the opinion before they were applied (`undo` after
/// sending, `sync.md` §7.4): each touched axis, react or tag vote goes back to its prior state.
pub(crate) fn inverse_steps(steps: &[OpinionStep], prior: Option<&Opinion>) -> Vec<OpinionStep> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for s in steps {
        let key = match s {
            OpinionStep::Karma { .. } => "karma".to_owned(),
            OpinionStep::Agreement { .. } => "agreement".to_owned(),
            OpinionStep::React { id, quote, .. } => {
                let q: Option<TextQuote> = quote.as_deref().and_then(|j| serde_json::from_str(j).ok());
                format!("react\u{1f}{id}\u{1f}{}", qkey(q.as_ref()))
            }
            OpinionStep::Tag { slug, .. } => format!("tag\u{1f}{slug}"),
        };
        if !seen.insert(key) {
            continue;
        }
        out.push(match s {
            OpinionStep::Karma { .. } => OpinionStep::Karma {
                value: prior.and_then(|p| p.karma.clone()),
            },
            OpinionStep::Agreement { .. } => OpinionStep::Agreement {
                value: prior.and_then(|p| p.agreement.clone()),
            },
            OpinionStep::React {
                id, quote, stance, ..
            } => {
                let q: Option<TextQuote> = quote.as_deref().and_then(|j| serde_json::from_str(j).ok());
                let k = qkey(q.as_ref());
                let before = prior
                    .and_then(|p| p.reacts.as_ref())
                    .and_then(|rs| rs.iter().find(|r| r.react == *id && qkey(r.quote.as_ref()) == k));
                match before {
                    Some(r) => OpinionStep::React {
                        id: id.clone(),
                        stance: r.stance.clone().unwrap_or_else(|| "for".to_owned()),
                        quote: r
                            .quote
                            .as_ref()
                            .and_then(|q| serde_json::to_string(q).ok()),
                        remove: false,
                    },
                    None => OpinionStep::React {
                        id: id.clone(),
                        stance: stance.clone(),
                        quote: quote.clone(),
                        remove: true,
                    },
                }
            }
            OpinionStep::Tag { slug, .. } => OpinionStep::Tag {
                slug: slug.clone(),
                value: prior
                    .and_then(|p| p.tags.as_ref())
                    .and_then(|ts| ts.iter().find(|t| t.tag == *slug))
                    .map(|t| t.value.clone()),
            },
        });
    }
    out
}

/// An original assertion `mod adopt` re-issues: the assertion key form `^FK-H16$`, forum scope,
/// not lapsed (`until > now`).
fn is_adoptable(rkey: &str, v: &Value, now_us: i64) -> bool {
    let assertion = matches!(
        keys::split_mod_action_key(rkey),
        Some(keys::ModActionKey::Assertion { .. })
    );
    let scope = v.get("scope").and_then(Value::as_str).unwrap_or("forum");
    let lapsed = v
        .get("until")
        .and_then(Value::as_str)
        .and_then(|u| Datetime::parse(u).ok())
        .is_some_and(|u| u.timestamp_micros() <= now_us);
    assertion && scope == "forum" && !lapsed
}

/// The `mod/1` role table (`social.md` §4.3): owners and admins may issue every known action;
/// moderators every one except `#tier` `curated`. Unknown actions are never adopted.
fn role_may_issue(role: &str, v: &Value) -> bool {
    let action: Option<Action> = v
        .get("action")
        .and_then(|a| serde_json::from_value(a.clone()).ok());
    let Some(action) = action else { return false };
    if action.kind().is_none() {
        return false;
    }
    match role {
        "owner" | "admin" => true,
        "moderator" => !matches!(&action, Action::Tier(t) if t.tier() == Some(Tier::Curated)),
        _ => false,
    }
}

/// Splits a blob into chunk rows of ≤ 1 MiB.
pub(crate) fn chunk_rows(account: &str, seq: i64, index: u32, bytes: &[u8]) -> Vec<KvOp> {
    bytes
        .chunks(model::BLOB_CHUNK_BYTES)
        .enumerate()
        .filter_map(|(c, part)| {
            let row = ChunkRow {
                v: model::OUTBOX_ROW_VERSION,
                b: part.to_vec(),
            };
            row::encode(&row)
                .ok()
                .map(|b| KvOp::Put(model::blob_chunk_key(account, seq, index, c as u32), b))
        })
        .collect()
}

/// Is `rkey` a revision key of exactly `p`?
#[allow(dead_code)]
fn is_revision_of(rkey: &str, p: &str) -> bool {
    rkey.strip_prefix(p)
        .and_then(|r| r.strip_prefix('-'))
        .is_some_and(keys::is_tid)
}

#[allow(dead_code)]
fn _unused(_: &Map<String, Value>, _: &AtUri) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_restores_prior() {
        let prior: Opinion = serde_json::from_value(serde_json::json!({
            "$type": nsid::OPINION,
            "subject": {"uri": "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x/y", "cid": "c"},
            "forum": "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/x/f",
            "karma": "down",
            "reacts": [{"react": "funny", "stance": "against"}],
            "createdAt": "2026-01-01T00:00:00.000Z",
        }))
        .unwrap();
        let steps = vec![
            OpinionStep::Karma {
                value: Some("up".into()),
            },
            OpinionStep::React {
                id: "funny".into(),
                stance: "for".into(),
                quote: None,
                remove: false,
            },
            OpinionStep::React {
                id: "wise".into(),
                stance: "for".into(),
                quote: None,
                remove: false,
            },
            OpinionStep::Tag {
                slug: "x".into(),
                value: Some("up".into()),
            },
            OpinionStep::Karma { value: None },
        ];
        let inv = inverse_steps(&steps, Some(&prior));
        assert_eq!(
            inv,
            vec![
                OpinionStep::Karma {
                    value: Some("down".into())
                },
                OpinionStep::React {
                    id: "funny".into(),
                    stance: "against".into(),
                    quote: None,
                    remove: false
                },
                OpinionStep::React {
                    id: "wise".into(),
                    stance: "for".into(),
                    quote: None,
                    remove: true
                },
                OpinionStep::Tag {
                    slug: "x".into(),
                    value: None
                },
            ]
        );
        assert_eq!(
            inverse_steps(&steps[..1], None),
            vec![OpinionStep::Karma { value: None }]
        );
        assert!(lexicon_intent(&OpinionStep::Karma {
            value: Some("sideways".into())
        })
        .is_err());
        assert!(lexicon_intent(&OpinionStep::Tag {
            slug: "Bad Slug".into(),
            value: None
        })
        .is_err());
    }

    #[test]
    fn adopt_rules() {
        let hide = serde_json::json!({"action": {"$type": nsid::MOD_ACTION_HIDE}});
        let curated = serde_json::json!({"action": {"$type": nsid::MOD_ACTION_TIER, "tier": "curated"}});
        let unknown = serde_json::json!({"action": {"$type": "x.y#z"}});
        assert!(role_may_issue("moderator", &hide));
        assert!(!role_may_issue("moderator", &curated));
        assert!(role_may_issue("admin", &curated));
        assert!(!role_may_issue("owner", &unknown));
        let k = "uetjixxr3a33-ckjbz7a2pjeplbao";
        assert!(is_adoptable(k, &serde_json::json!({}), 0));
        assert!(!is_adoptable(k, &serde_json::json!({"scope": "author"}), 0));
        assert!(!is_adoptable("uetjixxr3a33-3l2s5xxv2ze2c", &serde_json::json!({}), 0));
        let lapsed = serde_json::json!({"until": "2020-01-01T00:00:00.000Z"});
        assert!(!is_adoptable(k, &lapsed, 1_700_000_000_000_000));
        assert!(is_revision_of("abc-3l2s5xxv2ze2c", "abc"));
        assert!(!is_revision_of("abc-3l2s5xxv2ze2c-3l2s5xxv2ze2c", "abc"));
    }
}
