// SPDX-License-Identifier: AGPL-3.0-or-later
//! The durable outbox (`data-model.md` §8, `sync.md` §7): every write is a [`PendingWrite`]
//! persisted in the `state` store before any network call, then sent by a pump that keeps at
//! most one write in flight per account, in `seq` order.
//!
//! - **Enqueue.** The write operations ([`Outbox::create_post`], [`Outbox::edit`],
//!   [`Outbox::opinion`], …, or [`Outbox::enqueue_with`]) build lexicon-valid values with their
//!   canonical keys ([`records`]), spill bodies over 64 KiB to `text/markdown` blobs, check the
//!   single-record validity rules and the reserved names, store the blob bytes, and persist the
//!   write in one atomic batch. They return a [`WriteTicket`] at once.
//! - **Pump.** [`Outbox::pump`] (long-lived, driven by the host), [`Outbox::run_due`] and
//!   [`Outbox::run_until_idle`] run the write protocols (`protocol.rs`): fresh swap heads, swap
//!   retries with jitter, CID-based recovery of a write found in `sending`, the write budget and
//!   rate-limit back-off. Confirmed writes are applied to the record store at once (`sync.md`
//!   §5.8): through the sync module when the outbox has a [`Syncer`] ([`Outbox::with_syncer`],
//!   which also queues the own repo at P1), else directly ([`apply::apply_own_write`]); either
//!   way they are reported to the [`OutboxListener`].
//! - **Wiring (engine).** `Outbox::with_listener(host, listener).with_syncer(engine.syncer())`,
//!   one per engine (clones share the pump locks, wakers and TID clock); the listener maps
//!   [`OutboxListener::on_write`] to `Event::Write { id, state }`.
//! - **Undo.** Opinion intents are held 4 s and coalesced per subject; [`Outbox::undo`] removes a
//!   write that was not sent, or issues the inverse opinion write once it was.
//!
//! Exclusion between processes sharing one `state` store (CLI and TUI, browser tabs) is the
//! host's `outbox:<account>` lock (`sync.md` §10.6), which the [`Host`] trait does not expose
//! yet: within one process the outbox serialises its own pumps and row updates.
// TODO(host-lock): take `outbox:<account>` around `step` once `Host` has `lock()` (§12.3).

pub mod apply;
pub mod blob;
pub mod budget;
pub mod model;
pub mod pds;
mod protocol;
pub mod records;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use futures::StreamExt;
use futures::channel::mpsc;
use futures::lock::Mutex as AsyncMutex;
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_atp::tid::{Tid, TidGenerator};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{
    Action, Comment, Forum, ModAction, Opinion, Post, Profile, Sequence, WikiRevision,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dto::{WriteState, WriteTicket};
use crate::engine::{EngineError, ErrorCode};
use crate::host::{Dir, Host, KeyRange, KvOp, KvStore, MaybeSendSync};
use crate::limits;
use crate::store::records::RecordStore;
use crate::store::row;
use crate::sync::Syncer;

pub use apply::{AppliedRecord, AppliedWrite, ApplyReport, apply_own_write, apply_with_syncer};
pub use blob::BlobData;
pub use budget::BudgetRow;
pub use model::{
    Intent, OnExists, Op, OpAction, OpResult, OpinionStep, PendingBlob, PendingWrite, StoredError,
    WriteStage, parse_write_id, write_id,
};
pub use pds::Session;
pub use protocol::{BACKOFF_BASE_MS, BACKOFF_MAX_MS, BLOB_REUPLOAD_AFTER_MS, MAX_ATTEMPTS};

use protocol::{Outcome, Runner};

/// How long the pump sleeps when it has nothing to do and is not woken (ms, *tunable*).
pub const IDLE_POLL_MS: i64 = 60_000;
/// How long the pump waits after a storage error (ms, *tunable*).
pub const ERROR_WAIT_MS: i64 = 5_000;
/// Done and undone writes are kept this long, then pruned by the idle pump (ms, *tunable*).
pub const DONE_KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The waker key of the all-account waiters (no DID is empty).
const WAKE_ALL: &str = "";
/// Rows per scan page.
const SCAN_PAGE: u32 = 256;
/// The largest estimate charged against the budget before a write starts (a write that
/// computes its ops when sent is charged per request).
const PRECHECK_COST_MAX: u32 = 600;

/// Receives outbox events. Called synchronously, in order, never re-entrantly.
pub trait OutboxListener: MaybeSendSync {
    /// A write's state changed (`Write { id, state }`, `engine-api.md` §3.8).
    fn on_write(&self, _ticket: &WriteTicket) {}

    /// The PDS (or the host) refused `account`'s session: its writes wait for a new login
    /// (`Auth { did, state: Expired }`, `engine-api.md` §3.8).
    fn on_session_refused(&self, _account: &str) {}

    /// A confirmed write was applied to the record store (`sync.md` §5.8). Called after the
    /// write's resulting state is saved and reported through [`OutboxListener::on_write`]. With a
    /// [`Syncer`] ([`Outbox::with_syncer`]) the own repo is already marked
    /// dirty at P1 and `report.mismatched` queued for P0 point fetches; without one, scheduling
    /// them is the listener's job.
    fn on_applied(&self, _write: &AppliedWrite, _report: &ApplyReport) {}
}

struct NoListener;

impl OutboxListener for NoListener {}

/// Builds a write while the outbox holds its row lock: TIDs come from one strictly increasing
/// generator per account (W6), seeded from the persisted last TID.
pub struct WriteCtx<'a> {
    session: &'a Session,
    now_ms: i64,
    /// The TID clock (µs): never behind the last persisted TID, so a clock set back cannot
    /// reuse a `seq` or an rkey.
    clock_us: i64,
    tids: TidGenerator,
}

impl WriteCtx<'_> {
    /// The account.
    pub fn account(&self) -> &Did {
        &self.session.did
    }

    /// The writer's clock (ms).
    pub fn now_ms(&self) -> i64 {
        self.now_ms
    }

    /// The writer's clock as a datetime.
    pub fn now(&self) -> Result<Datetime, EngineError> {
        records::datetime_ms(self.now_ms)
    }

    /// A new TID, greater than every TID handed out for this account before.
    pub fn tid(&mut self) -> Result<String, EngineError> {
        Ok(self.next()?.as_str().to_owned())
    }

    fn next(&mut self) -> Result<Tid, EngineError> {
        self.tids
            .next(self.clock_us)
            .map_err(|e| EngineError::internal(format!("TID: {e}")))
    }
}

/// A write to persist.
#[derive(Clone, Debug)]
pub struct NewWrite {
    /// The protocol.
    pub intent: Intent,
    /// The targets (`ops[0]` is the primary one).
    pub ops: Vec<Op>,
    /// Blobs referenced (as placeholders) by the op values, uploaded when the write is sent.
    pub blobs: Vec<BlobData>,
    /// Hold the write this long before sending (opinions: 4 s).
    pub hold_ms: Option<i64>,
    /// For an inverse write made by `undo`: the undone write's id.
    pub undo_of: Option<String>,
    /// Sent only after the write with this `seq` (same account) is done
    /// ([`PendingWrite::after`]).
    pub after: Option<i64>,
}

impl NewWrite {
    /// A write of `intent` over `ops`.
    pub fn new(intent: Intent, ops: Vec<Op>) -> Self {
        Self {
            intent,
            ops,
            blobs: Vec::new(),
            hold_ms: None,
            undo_of: None,
            after: None,
        }
    }

    /// Sent only once the earlier write `seq` of the same account is done; failed unsent if it
    /// failed, conflicted or was undone ([`PendingWrite::after`]).
    pub fn after(mut self, seq: i64) -> Self {
        self.after = Some(seq);
        self
    }

    /// With blobs.
    pub fn with_blobs(mut self, blobs: Vec<BlobData>) -> Self {
        self.blobs = blobs;
        self
    }

    /// Held for `ms` before sending.
    pub fn held(mut self, ms: i64) -> Self {
        self.hold_ms = Some(ms);
        self
    }
}

/// Media and excerpt of a content write.
#[derive(Clone, Debug, Default)]
pub struct ContentOpts {
    /// Blobs referenced by placeholders in the value ([`BlobData::placeholder`]).
    pub blobs: Vec<BlobData>,
    /// The excerpt written if a post body is spilled (PO6).
    pub excerpt: Option<String>,
}

/// Options of [`Outbox::edit`] (`engine-api.md` §3.5 `EditOpts`).
#[derive(Clone, Debug)]
pub struct EditOpts {
    /// Snapshot the old version in a revision (the edit protocol); `false` is `--no-history`.
    pub history: bool,
    /// Edit summary.
    pub summary: Option<String>,
    /// Blobs referenced by placeholders in the new value.
    pub blobs: Vec<BlobData>,
    /// The excerpt written if a post body is spilled.
    pub excerpt: Option<String>,
}

impl Default for EditOpts {
    fn default() -> Self {
        Self {
            history: true,
            summary: None,
            blobs: Vec::new(),
            excerpt: None,
        }
    }
}

/// One pump step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// No pending write.
    Idle,
    /// The first pending write waits until this time (ms): held, throttled or backing off.
    Wait(i64),
    /// The first pending write waits until the account logs in again (its session was
    /// refused); nothing of the account is sent before that.
    NeedsLogin(WriteTicket),
    /// A write was run; its new state.
    Processed(WriteTicket),
}

/// What a pump run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PumpReport {
    /// Protocol runs.
    pub processed: u32,
    /// Writes done.
    pub done: u32,
    /// Writes failed.
    pub failed: u32,
    /// Writes in conflict.
    pub conflicts: u32,
    /// Runs that ended throttled or backing off.
    pub throttled: u32,
    /// Accounts whose writes wait for a new login (their session was refused).
    pub needs_login: u32,
}

impl PumpReport {
    fn count(&mut self, t: &WriteTicket) {
        self.processed += 1;
        match &t.state {
            WriteState::Done { .. } => self.done += 1,
            WriteState::Failed { error } if error.code == ErrorCode::Conflict => {
                self.conflicts += 1;
            }
            WriteState::Failed { .. } => self.failed += 1,
            WriteState::Pending { .. } => self.throttled += 1,
            WriteState::Sending | WriteState::Undone => {}
        }
    }
}

struct Shared {
    listener: Arc<dyn OutboxListener>,
    /// Serialises read-modify-write of outbox rows within this process.
    rows: AsyncMutex<()>,
    /// One pump lock per account.
    pumps: StdMutex<BTreeMap<String, Arc<AsyncMutex<()>>>>,
    /// Wakers of running pumps, per account ([`WAKE_ALL`]: woken for every account).
    wakers: StdMutex<BTreeMap<String, Vec<mpsc::UnboundedSender<()>>>>,
    /// This process's TID clock id.
    clock_id: u16,
    /// Incremented by `stop`: a pump runs while the generation it started in is current.
    generation: AtomicU64,
}

/// The outbox over a host. Cloning shares it.
pub struct Outbox<H: Host> {
    host: Arc<H>,
    shared: Arc<Shared>,
    /// Applies confirmed writes through the sync module (repo lock, dirty marks), if set.
    sync: Option<Syncer<H>>,
}

impl<H: Host> Clone for Outbox<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            shared: Arc::clone(&self.shared),
            sync: self.sync.clone(),
        }
    }
}

fn storage<E: std::fmt::Display>(e: E) -> EngineError {
    EngineError::internal(format!("outbox storage: {e}")).with_retryable(true)
}

fn lock<T>(m: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The own record `uri` of `session` (W9: never another repo).
fn own_uri(session: &Session, uri: &str) -> Result<AtUri, EngineError> {
    let u = records::at_uri(uri)?;
    if u.did() != &session.did {
        return Err(EngineError::invalid_input(format!(
            "{uri} is not in the repo of {} (W9)",
            session.did
        )));
    }
    Ok(u)
}

impl<H: Host> Outbox<H> {
    /// An outbox over `host` without a listener.
    pub fn new(host: Arc<H>) -> Self {
        Self::with_listener(host, Arc::new(NoListener))
    }

    /// An outbox over `host` reporting to `listener`.
    // In wasm the listener is not `Send` (`MaybeSendSync`); the core is single-threaded there.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn with_listener(host: Arc<H>, listener: Arc<dyn OutboxListener>) -> Self {
        let mut b = [0u8; 2];
        host.fill_random(&mut b);
        let clock_id = u16::from_le_bytes(b) & lessdugin_atp::tid::MAX_CLOCK_ID;
        Self {
            host,
            shared: Arc::new(Shared {
                listener,
                rows: AsyncMutex::new(()),
                pumps: StdMutex::new(BTreeMap::new()),
                wakers: StdMutex::new(BTreeMap::new()),
                clock_id,
                generation: AtomicU64::new(0),
            }),
            sync: None,
        }
    }

    /// Applies confirmed writes through `syncer` ([`apply_with_syncer`], `sync.md` §5.8): under
    /// the repo lock, with the own repo marked dirty at P1 and mismatched uris queued for P0
    /// point fetches in the engine's dirty queue. Set it before cloning the outbox.
    pub fn with_syncer(mut self, syncer: Syncer<H>) -> Self {
        self.sync = Some(syncer);
        self
    }

    /// The host.
    pub fn host(&self) -> &H {
        &self.host
    }

    // -----------------------------------------------------------------------------------------
    // Rows

    async fn get_row<T: DeserializeOwned>(&self, key: Vec<u8>) -> Result<Option<T>, EngineError> {
        let mut rows = self.host.state().get_many(vec![key]).await?;
        match rows.pop().flatten() {
            Some(b) => Ok(Some(row::decode(&b).map_err(storage)?)),
            None => Ok(None),
        }
    }

    fn put_op<T: serde::Serialize>(key: Vec<u8>, value: &T) -> Result<KvOp, EngineError> {
        Ok(KvOp::Put(key, row::encode(value).map_err(storage)?))
    }

    /// The writes of `account` in `seq` order; with `first_pending`, stops at the first pending
    /// one (and returns only it).
    async fn scan_writes(
        &self,
        account: &str,
        first_pending: bool,
    ) -> Result<Vec<PendingWrite>, EngineError> {
        let prefix = model::writes_prefix(account);
        let mut range = KeyRange::prefix(&prefix);
        let mut out = Vec::new();
        loop {
            let page = self
                .host
                .state()
                .scan(range.clone(), Dir::Forward, SCAN_PAGE)
                .await?;
            let full = page.len() >= SCAN_PAGE as usize;
            let last = page.last().map(|(k, _)| k.clone());
            for (k, v) in page {
                match row::decode::<PendingWrite>(&v) {
                    Ok(w) if first_pending => {
                        if w.state.is_pending() {
                            return Ok(vec![w]);
                        }
                    }
                    Ok(w) => out.push(w),
                    Err(e) => tracing::warn!(key = ?k, error = %e, "unreadable outbox row"),
                }
            }
            match last {
                Some(k) if full => range.start = Bound::Excluded(k),
                _ => return Ok(out),
            }
        }
    }

    fn emit(&self, w: &PendingWrite) {
        self.shared.listener.on_write(&w.ticket());
    }

    /// Wakes the pumps of `account` and every all-account waiter ([`Outbox::waker`]).
    fn wake(&self, account: &str) {
        let mut wakers = lock(&self.shared.wakers);
        for key in [account, WAKE_ALL] {
            if let Some(list) = wakers.get_mut(key) {
                list.retain(|tx| tx.unbounded_send(()).is_ok());
            }
        }
        wakers.retain(|_, list| !list.is_empty());
    }

    /// Wakes every running pump and waiter (an account was added or removed).
    pub fn wake_all(&self) {
        let mut wakers = lock(&self.shared.wakers);
        for list in wakers.values_mut() {
            list.retain(|tx| tx.unbounded_send(()).is_ok());
        }
        wakers.retain(|_, list| !list.is_empty());
    }

    /// A receiver woken by every new or re-queued write of any account, by
    /// [`Outbox::wake_all`] and by [`Outbox::stop`] (the engine's all-account pump).
    pub fn waker(&self) -> mpsc::UnboundedReceiver<()> {
        let (tx, rx) = mpsc::unbounded();
        lock(&self.shared.wakers)
            .entry(WAKE_ALL.to_owned())
            .or_default()
            .push(tx);
        rx
    }

    /// The pump generation: a pump started now runs until [`Outbox::stop`] changes it.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::SeqCst)
    }

    /// The host lock `outbox:<account>` (`sync.md` §7.1, §10.6), waiting for it.
    async fn account_lock(&self, account: &str) -> Result<H::Lock, EngineError> {
        let name = format!("outbox:{account}");
        self.host.lock(&name, true).await?.ok_or_else(|| {
            EngineError::internal(format!("the host did not grant the lock {name}"))
                .with_retryable(true)
        })
    }

    fn pump_lock(&self, account: &str) -> Arc<AsyncMutex<()>> {
        Arc::clone(
            lock(&self.shared.pumps)
                .entry(account.to_owned())
                .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
        )
    }

    // -----------------------------------------------------------------------------------------
    // Enqueue

    /// Persists the write `build` returns (built under the row lock with a [`WriteCtx`]).
    pub async fn enqueue_with<F>(
        &self,
        session: &Session,
        build: F,
    ) -> Result<WriteTicket, EngineError>
    where
        F: FnOnce(&mut WriteCtx<'_>) -> Result<NewWrite, EngineError> + crate::host::MaybeSend,
    {
        let _g = self.shared.rows.lock().await;
        let w = self.enqueue_locked(session, build).await?;
        drop(_g);
        self.emit(&w);
        self.wake(&w.account);
        Ok(w.ticket())
    }

    /// Persists `w`.
    pub async fn enqueue(
        &self,
        session: &Session,
        w: NewWrite,
    ) -> Result<WriteTicket, EngineError> {
        self.enqueue_with(session, move |_| Ok(w)).await
    }

    async fn enqueue_locked<F>(
        &self,
        session: &Session,
        build: F,
    ) -> Result<PendingWrite, EngineError>
    where
        F: FnOnce(&mut WriteCtx<'_>) -> Result<NewWrite, EngineError>,
    {
        let account = session.did.as_str();
        let now = self.host.now_ms();
        let mut meta: model::MetaRow = self
            .get_row(model::meta_key(account))
            .await?
            .unwrap_or_default();
        let last = Tid::from_u64(meta.last_tid);
        let clock_us = now
            .saturating_mul(1000)
            .max(i64::try_from(last.timestamp_micros()).unwrap_or(i64::MAX));
        let mut tids = TidGenerator::new(self.shared.clock_id);
        tids.observe(&last, clock_us);
        let mut ctx = WriteCtx {
            session,
            now_ms: now,
            clock_us,
            tids,
        };
        let seq_tid = ctx.next()?;
        let seq = i64::try_from(seq_tid.to_u64()).map_err(storage)?;
        let new = build(&mut ctx)?;
        let last = ctx.next()?; // past every TID the builder took
        meta.v = model::OUTBOX_ROW_VERSION;
        meta.last_tid = last.to_u64();
        let (w, chunks) = prepare(session, seq, now, new)?;
        let mut batch = vec![
            Self::put_op(model::meta_key(account), &meta)?,
            Self::put_op(model::write_key(account, seq), &w)?,
        ];
        batch.extend(chunks);
        self.host.state().write(batch).await?;
        Ok(w)
    }

    // -----------------------------------------------------------------------------------------
    // Write operations

    async fn has_own_membership(&self, session: &Session, forum: &AtUri) -> bool {
        let Ok(rkey) = lessdugin_lexicon::keys::membership_rkey(forum.as_str()) else {
            return false;
        };
        let Ok(uri) = AtUri::parse(&format!("at://{}/{}/{rkey}", session.did, nsid::MEMBERSHIP))
        else {
            return false;
        };
        matches!(
            RecordStore::new(self.host.cache()).get(&uri).await,
            Ok(Some(_))
        )
    }

    /// Content in a forum: `createRecord`, or the first-content batch with the membership when
    /// the record store does not show the own membership (the protocol re-checks it fresh).
    async fn create_content<F>(
        &self,
        session: &Session,
        forum: &AtUri,
        first_content: bool,
        opts: ContentOpts,
        build: F,
    ) -> Result<WriteTicket, EngineError>
    where
        F: FnOnce(&str) -> Result<records::Built, EngineError> + crate::host::MaybeSend,
    {
        let first = first_content && !self.has_own_membership(session, forum).await;
        let forum = forum.clone();
        self.enqueue_with(session, move |ctx| {
            let tid = ctx.tid()?;
            let built = build(&tid)?;
            let mut op = built.create_op();
            if let Some(e) = opts.excerpt {
                op = op.with_excerpt(e);
            }
            let mut ops = vec![op];
            let intent = if first {
                ops.push(records::membership(&forum, None, &ctx.now()?)?.create_op());
                Intent::FirstContent
            } else {
                Intent::Create
            };
            Ok(NewWrite::new(intent, ops).with_blobs(opts.blobs))
        })
        .await
    }

    /// Publishes a post (`createRecord`; with the membership on first content, §8.4).
    pub async fn create_post(
        &self,
        session: &Session,
        post: &Post,
        opts: ContentOpts,
    ) -> Result<WriteTicket, EngineError> {
        let p = post.clone();
        self.create_content(session, &post.forum, true, opts, move |tid| {
            records::post(&p, tid)
        })
        .await
    }

    /// Publishes a comment (`createRecord`; with the membership on first content, §8.4).
    pub async fn create_comment(
        &self,
        session: &Session,
        comment: &Comment,
        opts: ContentOpts,
    ) -> Result<WriteTicket, EngineError> {
        let c = comment.clone();
        self.create_content(session, &comment.forum, true, opts, move |tid| {
            records::comment(&c, tid)
        })
        .await
    }

    /// Creates a sequence.
    pub async fn create_sequence(
        &self,
        session: &Session,
        sequence: &Sequence,
        opts: ContentOpts,
    ) -> Result<WriteTicket, EngineError> {
        let s = sequence.clone();
        self.create_content(session, &sequence.forum, false, opts, move |tid| {
            records::sequence(&s, tid)
        })
        .await
    }

    /// Creates a wiki revision.
    pub async fn create_wiki_revision(
        &self,
        session: &Session,
        rev: &WikiRevision,
        opts: ContentOpts,
    ) -> Result<WriteTicket, EngineError> {
        let w = rev.clone();
        self.create_content(session, &rev.forum, false, opts, move |tid| {
            records::wiki_revision(&w, tid)
        })
        .await
    }

    /// Creates a forum and the owner's membership in one `applyWrites` (§8.4).
    pub async fn create_forum(
        &self,
        session: &Session,
        slug: &str,
        forum: &Forum,
        opts: ContentOpts,
    ) -> Result<WriteTicket, EngineError> {
        let built = records::forum(slug, forum)?;
        let uri = records::at_uri(&built.uri(&session.did))?;
        self.enqueue_with(session, move |ctx| {
            let mut op = built.create_op();
            if let Some(e) = opts.excerpt {
                op = op.with_excerpt(e);
            }
            let membership = records::membership(&uri, None, &ctx.now()?)?;
            Ok(
                NewWrite::new(Intent::CreateForum, vec![op, membership.create_op()])
                    .with_blobs(opts.blobs),
            )
        })
        .await
    }

    /// Joins a forum (create-if-absent of the membership; an existing one is success).
    pub async fn join(
        &self,
        session: &Session,
        forum: &AtUri,
        note: Option<&str>,
    ) -> Result<WriteTicket, EngineError> {
        let forum = forum.clone();
        let note = note.map(str::to_owned);
        self.enqueue_with(session, move |ctx| {
            let m = records::membership(&forum, note.as_deref(), &ctx.now()?)?;
            Ok(NewWrite::new(
                Intent::CreateIfAbsent {
                    on_exists: OnExists::Succeed,
                },
                vec![m.create_op()],
            ))
        })
        .await
    }

    /// Leaves a forum: `deleteRecord(membership, swapRecord = cid)` with the CID the record
    /// store holds.
    pub async fn leave(
        &self,
        session: &Session,
        forum: &AtUri,
    ) -> Result<WriteTicket, EngineError> {
        let rkey = lessdugin_lexicon::keys::membership_rkey(forum.as_str())
            .map_err(|e| EngineError::invalid_ref(format!("{forum}: {e}")))?;
        let uri = records::at_uri(&format!("at://{}/{}/{rkey}", session.did, nsid::MEMBERSHIP))?;
        let cid = RecordStore::new(self.host.cache())
            .get(&uri)
            .await
            .ok()
            .flatten()
            .map(|r| r.cid.to_string());
        let mut op = Op::delete(nsid::MEMBERSHIP, &rkey);
        if let Some(c) = cid {
            op = op.with_swap(c);
        }
        self.enqueue(session, NewWrite::new(Intent::DeleteRecord, vec![op]))
            .await
    }

    /// Edits an own record (§8.3): the edit protocol with a revision snapshot, or with
    /// `history = false` (`--no-history`, not for modActions) `putRecord(swapRecord = base)`.
    /// `value` is the complete new value (unknown fields preserved by the caller); `base` is the
    /// CID the user edited.
    pub async fn edit(
        &self,
        session: &Session,
        uri: &str,
        value: &Value,
        base: &str,
        opts: EditOpts,
    ) -> Result<WriteTicket, EngineError> {
        self.edit_after(session, uri, value, base, opts, None).await
    }

    /// [`Outbox::edit`], sent only once the earlier write `after` (a `seq` of the same account)
    /// is done, and failed unsent if it did not complete ([`PendingWrite::after`]).
    pub async fn edit_after(
        &self,
        session: &Session,
        uri: &str,
        value: &Value,
        base: &str,
        opts: EditOpts,
        after: Option<i64>,
    ) -> Result<WriteTicket, EngineError> {
        let u = own_uri(session, uri)?;
        let collection = u.collection().as_str().to_owned();
        let rkey = u.rkey().as_str().to_owned();
        let content =
            [nsid::POST, nsid::COMMENT, nsid::SEQUENCE, nsid::FORUM].contains(&collection.as_str());
        if !content && collection != nsid::MOD_ACTION {
            return Err(EngineError::invalid_input(format!(
                "{collection} records are not edited with the edit protocol"
            )));
        }
        if !opts.history && !content {
            return Err(EngineError::invalid_input(
                "--no-history applies to posts, comments, sequences and forums only",
            ));
        }
        let value = value.clone();
        let base = base.to_owned();
        let uri = uri.to_owned();
        self.enqueue_with(session, move |ctx| {
            // W5: updatedAt is set on every update
            let mut value = value;
            if let Some(m) = value.as_object_mut() {
                m.insert(
                    "updatedAt".into(),
                    Value::String(ctx.now()?.as_str().to_owned()),
                );
            }
            let mut op = Op::update(&collection, &rkey, &value);
            if let Some(e) = opts.excerpt {
                op = op.with_excerpt(e);
            }
            let intent = if opts.history {
                Intent::Edit {
                    base,
                    summary: opts.summary,
                    revision_rkey: records::revision_rkey(&uri, &ctx.tid()?)?,
                }
            } else {
                Intent::Put { base }
            };
            let mut w = NewWrite::new(intent, vec![op]).with_blobs(opts.blobs);
            w.after = after;
            Ok(w)
        })
        .await
    }

    /// Deletes an own record: content (post, comment, sequence, forum) by the delete protocol
    /// (§8.7), a modAction by the removal protocol with its `delete` revision, a revision by
    /// redaction, anything else with `deleteRecord`.
    pub async fn delete(&self, session: &Session, uri: &str) -> Result<WriteTicket, EngineError> {
        let u = own_uri(session, uri)?;
        let collection = u.collection().as_str().to_owned();
        let rkey = u.rkey().as_str().to_owned();
        let uri = uri.to_owned();
        self.enqueue_with(session, move |ctx| {
            let op = Op::delete(&collection, &rkey);
            let intent = match collection.as_str() {
                c if [nsid::POST, nsid::COMMENT, nsid::SEQUENCE, nsid::FORUM].contains(&c) => {
                    Intent::DeleteContent
                }
                c if c == nsid::MOD_ACTION => Intent::RemoveModAction {
                    revision_rkey: records::revision_rkey(&uri, &ctx.tid()?)?,
                },
                c if c == nsid::REVISION => Intent::Redact,
                _ => Intent::DeleteRecord,
            };
            Ok(NewWrite::new(intent, vec![op]))
        })
        .await
    }

    /// Redacts own revision records (`applyWrites[#delete revision …]`, chunked at ≤ 200).
    pub async fn redact(
        &self,
        session: &Session,
        uris: &[String],
    ) -> Result<WriteTicket, EngineError> {
        let mut ops = Vec::with_capacity(uris.len());
        for uri in uris {
            let u = own_uri(session, uri)?;
            if u.collection().as_str() != nsid::REVISION {
                return Err(EngineError::invalid_input(format!(
                    "{uri} is not a revision"
                )));
            }
            ops.push(Op::delete(nsid::REVISION, u.rkey().as_str()));
        }
        if ops.is_empty() {
            return Err(EngineError::invalid_input("nothing to redact"));
        }
        self.enqueue(session, NewWrite::new(Intent::Redact, ops))
            .await
    }

    /// Writes the own profile: create-if-absent without `base` (an existing profile is a
    /// conflict), otherwise `putRecord(self, swapRecord = base)`.
    pub async fn put_profile(
        &self,
        session: &Session,
        profile: &Profile,
        base: Option<&str>,
        blobs: Vec<BlobData>,
    ) -> Result<WriteTicket, EngineError> {
        let built = records::profile(profile)?;
        let base = base.map(str::to_owned);
        self.enqueue_with(session, move |ctx| {
            let w = match base {
                None => NewWrite::new(
                    Intent::CreateIfAbsent {
                        on_exists: OnExists::Conflict,
                    },
                    vec![built.create_op()],
                ),
                Some(base) => {
                    // W5: updatedAt is set on every update
                    let mut v = built.value;
                    if let Some(m) = v.as_object_mut() {
                        m.insert(
                            "updatedAt".into(),
                            Value::String(ctx.now()?.as_str().to_owned()),
                        );
                    }
                    NewWrite::new(
                        Intent::Put { base },
                        vec![Op::update(built.collection, &built.rkey, &v)],
                    )
                }
            };
            Ok(w.with_blobs(blobs))
        })
        .await
    }

    /// Creates a moderation assertion (§8.4: create-if-absent; an existing record continues as
    /// an update through the edit protocol). A lock whose allow-list has more than 5,000
    /// comments is refused (§8.6: use hide instead).
    pub async fn mod_assert(
        &self,
        session: &Session,
        action: &ModAction,
    ) -> Result<WriteTicket, EngineError> {
        if let Action::Lock(l) = &action.action
            && l.locked()
            && l.allowed
                .as_ref()
                .is_some_and(|a| a.len() > limits::LOCK_ALLOWED_MAX)
        {
            return Err(EngineError::invalid_input(
                "a lock may keep at most 5,000 comments visible; hide the thread instead",
            )
            .with_detail("hint", "useHide"));
        }
        let built = records::mod_action(action)?;
        let uri = built.uri(&session.did);
        self.enqueue_with(session, move |ctx| {
            Ok(NewWrite::new(
                Intent::CreateIfAbsent {
                    on_exists: OnExists::Update {
                        revision_rkey: records::revision_rkey(&uri, &ctx.tid()?)?,
                    },
                },
                vec![built.create_op()],
            ))
        })
        .await
    }

    /// `mod adopt` (§8.6): re-issues the former moderator's forum-scope assertions. `role` is
    /// the actor's role in the forum (`owner`, `admin` or `moderator`); `former_pds` is the
    /// former moderator's resolved PDS.
    pub async fn mod_adopt(
        &self,
        session: &Session,
        forum: &AtUri,
        former: &Did,
        former_pds: &str,
        role: &str,
    ) -> Result<WriteTicket, EngineError> {
        if !["owner", "admin", "moderator"].contains(&role) {
            return Err(EngineError::invalid_input(format!(
                "{role:?} may not adopt"
            )));
        }
        Session::parse(former.as_str(), former_pds)?;
        let forum = forum.as_str().to_owned();
        let former = former.as_str().to_owned();
        let former_pds = former_pds.to_owned();
        let role = role.to_owned();
        self.enqueue_with(session, move |ctx| {
            Ok(NewWrite::new(
                Intent::Adopt {
                    forum,
                    former,
                    former_pds,
                    role,
                    revision_tid: ctx.tid()?,
                    report: Vec::new(),
                },
                Vec::new(),
            ))
        })
        .await
    }

    /// An opinion intent on `subject` (§8.5), held 4 s. An intent on a subject whose opinion
    /// write is still held joins that write (applied in order) and restarts its hold; the
    /// ticket is that write's.
    pub async fn opinion(
        &self,
        session: &Session,
        forum: &str,
        subject: &str,
        subject_cid: &str,
        step: OpinionStep,
    ) -> Result<WriteTicket, EngineError> {
        protocol::lexicon_intent(&step)?;
        let rkey = records::opinion_rkey(forum, subject)?;
        records::at_uri(subject)?;
        lessdugin_atp::Cid::try_from(subject_cid)
            .map_err(|e| EngineError::invalid_input(format!("subject CID {subject_cid:?}: {e}")))?;
        let account = session.did.as_str();
        let _g = self.shared.rows.lock().await;
        let now = self.host.now_ms();
        let held = self
            .scan_writes(account, false)
            .await?
            .into_iter()
            .rev()
            .find(|w| {
                w.state == WriteStage::Held
                    && !w.sent
                    && w.undo_of.is_none()
                    && matches!(&w.intent, Intent::Opinion { subject: s, forum: f, .. }
                        if s == subject && f == forum)
            });
        let w = match held {
            Some(mut w) => {
                if let Intent::Opinion {
                    steps,
                    subject_cid: cid,
                    ..
                } = &mut w.intent
                {
                    steps.push(step);
                    subject_cid.clone_into(cid);
                }
                w.not_before = Some(now + limits::OPINION_HOLD_MS);
                w.updated_at = now;
                self.host
                    .state()
                    .write(vec![Self::put_op(model::write_key(account, w.seq), &w)?])
                    .await?;
                w
            }
            None => {
                let intent = Intent::Opinion {
                    subject: subject.to_owned(),
                    subject_cid: subject_cid.to_owned(),
                    forum: forum.to_owned(),
                    steps: vec![step],
                    prior: None,
                    prior_known: false,
                };
                let op = Op {
                    action: OpAction::Update,
                    collection: nsid::OPINION.to_owned(),
                    rkey,
                    value: None,
                    swap: None,
                    excerpt: None,
                };
                self.enqueue_locked(session, move |_| {
                    Ok(NewWrite::new(intent, vec![op]).held(limits::OPINION_HOLD_MS))
                })
                .await?
            }
        };
        drop(_g);
        self.emit(&w);
        self.wake(account);
        Ok(w.ticket())
    }

    // -----------------------------------------------------------------------------------------
    // Undo, retry, queries

    fn own_write_id(session: &Session, id: &str) -> Result<i64, EngineError> {
        let (account, seq) = parse_write_id(id)
            .ok_or_else(|| EngineError::invalid_input(format!("bad write id {id:?}")))?;
        if account != session.did.as_str() {
            return Err(EngineError::invalid_input(format!(
                "write {id} belongs to another account (W9)"
            )));
        }
        Ok(seq)
    }

    /// Undoes a write (`sync.md` §7.4): a write not yet sent (held, queued, throttled) is
    /// removed; an opinion write that was sent issues the inverse intents as a new write, whose
    /// ticket is returned. A failed or conflicting write is dismissed. Other sent writes cannot
    /// be undone (edit or delete instead).
    pub async fn undo(&self, session: &Session, id: &str) -> Result<WriteTicket, EngineError> {
        let seq = Self::own_write_id(session, id)?;
        let account = session.did.as_str();
        // another process may be picking this write to send it (`sync.md` §7.1); a write being
        // sent finishes first. Lock order: host lock, then the row lock.
        let _held = self.account_lock(account).await?;
        let _g = self.shared.rows.lock().await;
        let mut w: PendingWrite = self
            .get_row(model::write_key(account, seq))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("no write {id}")))?;
        let now = self.host.now_ms();
        let removable = matches!(
            w.state,
            WriteStage::Queued | WriteStage::Held | WriteStage::Throttled
        ) && !w.sent
            || matches!(w.state, WriteStage::Failed | WriteStage::Conflict);
        let out = if w.state == WriteStage::Undone {
            w
        } else if removable {
            w.state = WriteStage::Undone;
            w.not_before = None;
            w.updated_at = now;
            self.host
                .state()
                .write(vec![
                    Self::put_op(model::write_key(account, seq), &w)?,
                    KvOp::DeleteRange(KeyRange::prefix(&model::blob_prefix(account, seq))),
                ])
                .await?;
            w
        } else if let Intent::Opinion {
            subject,
            subject_cid,
            forum,
            steps,
            prior,
            prior_known,
        } = &w.intent
        {
            if !*prior_known {
                return Err(EngineError::conflict(
                    "the opinion is being sent; undo again in a moment",
                )
                .with_retryable(true));
            }
            let prior: Option<Opinion> = match prior {
                Some(j) => Some(serde_json::from_str(j).map_err(storage)?),
                None => None,
            };
            let inverse = protocol::inverse_steps(steps, prior.as_ref());
            let intent = Intent::Opinion {
                subject: subject.clone(),
                subject_cid: subject_cid.clone(),
                forum: forum.clone(),
                steps: inverse,
                prior: None,
                prior_known: false,
            };
            let ops = w.ops.iter().take(1).map(|o| Op {
                value: None,
                ..o.clone()
            });
            let ops: Vec<Op> = ops.collect();
            let undone = w.id();
            self.enqueue_locked(session, move |_| {
                let mut n = NewWrite::new(intent, ops);
                n.undo_of = Some(undone);
                Ok(n)
            })
            .await?
        } else if w.state == WriteStage::Sending {
            return Err(EngineError::conflict("the write is being sent").with_retryable(true));
        } else {
            return Err(EngineError::invalid_input(
                "this write was sent and cannot be undone; edit or delete the record instead",
            ));
        };
        drop(_g);
        self.emit(&out);
        self.wake(account);
        Ok(out.ticket())
    }

    /// Queues a failed or conflicting write again.
    pub async fn retry(&self, session: &Session, id: &str) -> Result<WriteTicket, EngineError> {
        let seq = Self::own_write_id(session, id)?;
        let account = session.did.as_str();
        let _g = self.shared.rows.lock().await;
        let mut w: PendingWrite = self
            .get_row(model::write_key(account, seq))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("no write {id}")))?;
        let now = self.host.now_ms();
        let waiting = is_back_off(&w, now) || (w.needs_login && w.state == WriteStage::Throttled);
        if !matches!(w.state, WriteStage::Failed | WriteStage::Conflict) && !waiting {
            return Err(EngineError::invalid_input(format!(
                "write {id} is neither failed nor backing off"
            )));
        }
        if waiting {
            // "send now": the attempts so far still count towards MAX_ATTEMPTS
            w.state = WriteStage::Queued;
            w.needs_login = false;
            w.not_before = None;
            w.updated_at = now;
            self.host
                .state()
                .write(vec![Self::put_op(model::write_key(account, seq), &w)?])
                .await?;
            drop(_g);
            self.emit(&w);
            self.wake(account);
            return Ok(w.ticket());
        }
        w.state = WriteStage::Queued;
        w.attempts = 0;
        w.error = None;
        w.not_before = None;
        w.updated_at = self.host.now_ms();
        self.host
            .state()
            .write(vec![Self::put_op(model::write_key(account, seq), &w)?])
            .await?;
        drop(_g);
        self.emit(&w);
        self.wake(account);
        Ok(w.ticket())
    }

    /// A write by id.
    pub async fn get(&self, id: &str) -> Result<Option<PendingWrite>, EngineError> {
        let Some((account, seq)) = parse_write_id(id) else {
            return Ok(None);
        };
        self.get_row(model::write_key(&account, seq)).await
    }

    /// A write's ticket.
    pub async fn ticket(&self, id: &str) -> Result<Option<WriteTicket>, EngineError> {
        Ok(self.get(id).await?.map(|w| w.ticket()))
    }

    /// Every write of `account`, in `seq` order.
    pub async fn writes(&self, account: &str) -> Result<Vec<PendingWrite>, EngineError> {
        self.scan_writes(account, false).await
    }

    /// The pending writes of `account` (queued, held, sending, throttled), in `seq` order.
    pub async fn pending(&self, account: &str) -> Result<Vec<PendingWrite>, EngineError> {
        Ok(self
            .scan_writes(account, false)
            .await?
            .into_iter()
            .filter(|w| w.state.is_pending())
            .collect())
    }

    /// The write budget of `account`.
    pub async fn budget(&self, account: &str) -> Result<BudgetRow, EngineError> {
        Ok(self
            .get_row(model::budget_key(account))
            .await?
            .unwrap_or_default())
    }

    /// Removes the done and undone writes of `account` last changed before `before_ms`;
    /// returns how many.
    pub async fn prune(&self, account: &str, before_ms: i64) -> Result<usize, EngineError> {
        let _g = self.shared.rows.lock().await;
        let ops: Vec<KvOp> = self
            .scan_writes(account, false)
            .await?
            .into_iter()
            .filter(|w| {
                matches!(w.state, WriteStage::Done | WriteStage::Undone) && w.updated_at < before_ms
            })
            .flat_map(|w| {
                [
                    KvOp::Delete(model::write_key(account, w.seq)),
                    KvOp::DeleteRange(KeyRange::prefix(&model::blob_prefix(account, w.seq))),
                ]
            })
            .collect();
        let n = ops.len() / 2;
        if !ops.is_empty() {
            self.host.state().write(ops).await?;
        }
        Ok(n)
    }

    // -----------------------------------------------------------------------------------------
    // The pump

    /// Runs at most one write of `session`'s account (`sync.md` §7.2): the first pending write
    /// by `seq`, unless it waits (held, throttled) or the write budget is spent.
    pub async fn step(&self, session: &Session) -> Result<Step, EngineError> {
        let account = session.did.as_str().to_owned();
        let pump = self.pump_lock(&account);
        let _p = pump.lock().await;
        // One write in flight per account across engines, processes and tabs (`sync.md` §7.1):
        // the host lock is held from picking the write to saving its outcome, so the pick sees
        // what another holder saved. Lock order: host lock, then the row lock.
        let _held = self.account_lock(&account).await?;
        let (w, budget, recovering) = {
            let _g = self.shared.rows.lock().await;
            let Some(mut w) = self.scan_writes(&account, true).await?.pop() else {
                return Ok(Step::Idle);
            };
            let now = self.host.now_ms();
            if w.needs_login && w.state == WriteStage::Throttled {
                return Ok(Step::NeedsLogin(w.ticket()));
            }
            if let (WriteStage::Held | WriteStage::Throttled, Some(t)) = (w.state, w.not_before)
                && t > now
            {
                return Ok(Step::Wait(t));
            }
            if w.account != account {
                // W9: a write stamped with another account is never sent with this session.
                w.state = WriteStage::Failed;
                w.error = Some(StoredError::from(&EngineError::invalid_input(format!(
                    "the write is stamped for {} but the session is {account} (W9)",
                    w.account
                ))));
                w.updated_at = now;
                self.save(&account, &w, None).await?;
                drop(_g);
                self.emit(&w);
                return Ok(Step::Processed(w.ticket()));
            }
            if let Some(dep) = w.after {
                let d: Option<PendingWrite> = self.get_row(model::write_key(&account, dep)).await?;
                if let Some(error) = unmet_dependency(&account, dep, d.as_ref()) {
                    // FIFO: an earlier write is picked first, so a dependency still pending
                    // here cannot happen; one that did not complete fails this write unsent
                    w.state = WriteStage::Failed;
                    w.error = Some(StoredError::from(&error));
                    w.not_before = None;
                    w.updated_at = now;
                    self.save(&account, &w, None).await?;
                    drop(_g);
                    self.emit(&w);
                    return Ok(Step::Processed(w.ticket()));
                }
            }
            let budget: BudgetRow = self
                .get_row(model::budget_key(&account))
                .await?
                .unwrap_or_default();
            let cost = w.estimated_cost().min(PRECHECK_COST_MAX);
            if !budget.fits(now, cost) {
                let until = budget.next_fit(now, cost);
                // a row left `sending` (a crash mid-send) may have been sent: keep that through
                // the wait, so undo never "removes" a write that is live
                w.sent |= w.state == WriteStage::Sending;
                w.state = WriteStage::Throttled;
                w.not_before = Some(until);
                w.error = Some(StoredError::from(
                    &EngineError::new(ErrorCode::RateLimited, "write budget or rate limit")
                        .with_detail("untilMs", until),
                ));
                w.updated_at = now;
                self.save(&account, &w, None).await?;
                drop(_g);
                self.emit(&w);
                return Ok(Step::Wait(until));
            }
            let recovering = w.state == WriteStage::Sending || w.sent;
            w.state = WriteStage::Sending;
            w.attempts += 1;
            w.not_before = None;
            w.updated_at = now;
            self.save(&account, &w, None).await?;
            (w, budget, recovering)
        };
        self.emit(&w);
        let ticket = self.run_write(session, w, budget, recovering).await?;
        Ok(Step::Processed(ticket))
    }

    async fn save(
        &self,
        account: &str,
        w: &PendingWrite,
        budget: Option<&BudgetRow>,
    ) -> Result<(), EngineError> {
        let mut ops = vec![Self::put_op(model::write_key(account, w.seq), w)?];
        if let Some(b) = budget {
            ops.push(Self::put_op(model::budget_key(account), b)?);
        }
        if matches!(w.state, WriteStage::Done | WriteStage::Undone) {
            ops.push(KvOp::DeleteRange(KeyRange::prefix(&model::blob_prefix(
                account, w.seq,
            ))));
        }
        self.host.state().write(ops).await?;
        Ok(())
    }

    async fn run_write(
        &self,
        session: &Session,
        w: PendingWrite,
        budget: BudgetRow,
        recovering: bool,
    ) -> Result<WriteTicket, EngineError> {
        let host: &H = &self.host;
        let loaded = budget.clone();
        let mut r = Runner::new(host, session, w, budget);
        // A recovery whose check fails (offline, a bad answer) leaves the earlier attempt
        // possibly sent; a check that finds the write absent clears that doubt.
        let mut unknown = false;
        let res = if recovering {
            match r.already_applied().await {
                Ok(true) => Ok(Outcome::Done),
                Ok(false) => r.run().await,
                Err(e) => {
                    unknown = true;
                    Err(e)
                }
            }
        } else {
            r.run().await
        };
        let now = host.now_ms();
        let outcome = match res {
            Ok(o) => o,
            Err(e) => classify(&r, e, now),
        };
        // A confirmed request without a commit (a no-op putRecord): read the head (§8.2).
        if r.applied.iter().any(|a| a.rev.is_empty())
            && let Ok(h) = r.pds.head().await
        {
            for a in r.applied.iter_mut().filter(|a| a.rev.is_empty()) {
                a.rev.clone_from(&h.rev);
                a.commit = Some(h.cid.clone());
            }
        }
        let sent = r.pds.sent_any;
        let applied = std::mem::take(&mut r.applied);
        let budget = r.pds.budget.clone();
        let mut w = r.w;
        w.sent |= sent || unknown;
        match outcome {
            Outcome::Done => {
                w.state = WriteStage::Done;
                w.error = None;
                w.not_before = None;
            }
            Outcome::Conflict(e) => {
                w.state = WriteStage::Conflict;
                w.error = Some(StoredError::from(&e));
            }
            Outcome::Failed(e) => {
                w.state = WriteStage::Failed;
                w.error = Some(StoredError::from(&e));
            }
            Outcome::Retry { at, error } => {
                w.state = WriteStage::Throttled;
                w.not_before = Some(at);
                w.error = Some(StoredError::from(&error));
            }
            Outcome::NeedsLogin(error) => {
                // not an attempt: the write was never judged, only the session
                w.state = WriteStage::Throttled;
                w.needs_login = true;
                w.not_before = None;
                w.attempts = w.attempts.saturating_sub(1);
                w.error = Some(StoredError::from(&error));
            }
        }
        w.updated_at = now;
        // Confirmed requests are facts whatever the outcome (a partial delete): apply them
        // before the write is saved as finished (a crash in between is recovered by
        // `already_applied`), and report them after it is saved and its state emitted, so a
        // view re-queried on `Changed` no longer shows the write as pending.
        let mut reports = Vec::with_capacity(applied.len());
        for a in &applied {
            let report = match &self.sync {
                Some(syncer) => apply_with_syncer(syncer, a).await,
                None => apply_own_write(host.cache(), a, now)
                    .await
                    .map_err(|e| EngineError::internal(e.to_string())),
            };
            match report {
                Ok(report) => reports.push((a, report)),
                Err(e) => tracing::warn!(error = %e, "could not apply an own write to the store"),
            }
        }
        {
            let _g = self.shared.rows.lock().await;
            let account = session.did.as_str();
            let mut merged: BudgetRow = self
                .get_row(model::budget_key(account))
                .await?
                .unwrap_or_default();
            merged.merge_run(&loaded, &budget, now);
            self.save(account, &w, Some(&merged)).await?;
        }
        self.emit(&w);
        if w.needs_login {
            self.shared.listener.on_session_refused(account_of(session));
        }
        for (a, report) in &reports {
            self.shared.listener.on_applied(a, report);
        }
        Ok(w.ticket())
    }

    /// Releases the writes of `account` that wait for a new login (`Step::NeedsLogin`): they
    /// are queued again and the pump is woken. Called when the account logs in. Returns how
    /// many were released.
    pub async fn release_login_holds(&self, account: &str) -> Result<u32, EngineError> {
        let _g = self.shared.rows.lock().await;
        let now = self.host.now_ms();
        let mut released = Vec::new();
        for mut w in self.scan_writes(account, true).await? {
            if !(w.needs_login && w.state == WriteStage::Throttled) {
                continue;
            }
            w.needs_login = false;
            w.state = WriteStage::Queued;
            w.not_before = None;
            w.error = None;
            w.updated_at = now;
            self.save(account, &w, None).await?;
            released.push(w);
        }
        drop(_g);
        for w in &released {
            self.emit(w);
        }
        if !released.is_empty() {
            self.wake(account);
        }
        Ok(u32::try_from(released.len()).unwrap_or(u32::MAX))
    }

    /// "Send now" (`lessdugin outbox flush`, `retry`): ends the back-off of `account`'s writes
    /// that wait after an ambiguous or transient failure, so the next step tries them at once.
    /// Holds that are not ours to shorten stay: the undo window (`held`), the write budget and
    /// a PDS rate limit (`rate_limited`), and a refused session (`needsLogin`). Returns how
    /// many were expedited.
    pub async fn expedite(&self, account: &str) -> Result<u32, EngineError> {
        let _g = self.shared.rows.lock().await;
        let now = self.host.now_ms();
        let mut expedited = Vec::new();
        for mut w in self.scan_writes(account, true).await? {
            if !is_back_off(&w, now) {
                continue;
            }
            w.not_before = None;
            w.updated_at = now;
            self.save(account, &w, None).await?;
            expedited.push(w);
        }
        drop(_g);
        for w in &expedited {
            self.emit(w);
        }
        if !expedited.is_empty() {
            self.wake(account);
        }
        Ok(u32::try_from(expedited.len()).unwrap_or(u32::MAX))
    }

    /// Runs the account's due writes; returns when none is due (or the first waits).
    pub async fn run_due(&self, session: &Session) -> Result<PumpReport, EngineError> {
        let mut report = PumpReport::default();
        loop {
            match self.step(session).await? {
                Step::Processed(t) => report.count(&t),
                Step::Idle | Step::Wait(_) => return Ok(report),
                Step::NeedsLogin(_) => {
                    report.needs_login += 1;
                    return Ok(report);
                }
            }
        }
    }

    /// Runs the account's writes until none is pending, sleeping (with the host) through holds
    /// and back-offs.
    pub async fn run_until_idle(&self, session: &Session) -> Result<PumpReport, EngineError> {
        let mut report = PumpReport::default();
        loop {
            match self.step(session).await? {
                Step::Processed(t) => report.count(&t),
                Step::Idle => return Ok(report),
                Step::NeedsLogin(_) => {
                    report.needs_login += 1;
                    return Ok(report);
                }
                Step::Wait(t) => {
                    let ms = (t - self.host.now_ms()).max(1);
                    self.host.sleep(u64::try_from(ms).unwrap_or(1)).await;
                }
            }
        }
    }

    /// The long-lived pump of `session`'s account (`engine-api.md` §3.3 `outbox_pump`): drains
    /// the outbox, sleeping through holds and back-offs, woken by new writes. Returns after
    /// [`Outbox::stop`].
    pub async fn pump(&self, session: &Session) {
        let account = session.did.as_str().to_owned();
        let (tx, mut rx) = mpsc::unbounded();
        lock(&self.shared.wakers)
            .entry(account.clone())
            .or_default()
            .push(tx);
        // a later `stop` ends this pump; a pump started after a `stop` runs (generations)
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let stopped = || self.shared.generation.load(Ordering::SeqCst) != generation;
        while !stopped() {
            let wait = match self.step(session).await {
                Ok(Step::Processed(_)) => continue,
                Ok(Step::Wait(t)) => (t - self.host.now_ms()).clamp(1, IDLE_POLL_MS),
                // woken by the login (`release_login_holds`, account changes)
                Ok(Step::NeedsLogin(_)) => IDLE_POLL_MS,
                Ok(Step::Idle) => {
                    let before = self.host.now_ms().saturating_sub(DONE_KEEP_MS);
                    if let Err(e) = self.prune(&account, before).await {
                        tracing::warn!(account = %account, error = %e, "outbox prune failed");
                    }
                    IDLE_POLL_MS
                }
                Err(e) => {
                    tracing::warn!(account = %account, error = %e, "outbox pump step failed");
                    ERROR_WAIT_MS
                }
            };
            if stopped() {
                break;
            }
            let sleep = self.host.sleep(u64::try_from(wait).unwrap_or(1));
            futures::pin_mut!(sleep);
            let woken = rx.next();
            futures::future::select(sleep, woken).await;
            while rx.try_recv().is_ok() {}
        }
    }

    /// Stops every running pump of this outbox (they return after their current step). A pump
    /// started afterwards runs normally.
    pub fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.wake_all();
    }
}

fn account_of(session: &Session) -> &str {
    session.did.as_str()
}

/// `true` for a write that backs off after an ambiguous or transient failure (not the undo
/// window, the write budget, a rate limit or a refused session).
fn is_back_off(w: &PendingWrite, now: i64) -> bool {
    w.state == WriteStage::Throttled
        && !w.needs_login
        && w.not_before.is_some_and(|t| t > now)
        && w.error
            .as_ref()
            .is_some_and(|e| e.code != ErrorCode::RateLimited)
}

/// Maps a protocol error to an outcome (`data-model.md` §8.2): `InvalidSwap` is a conflict, an
/// ambiguous or transient failure backs off (and fails after [`MAX_ATTEMPTS`]), a rate limit
/// waits until it lifts, a refusal fails.
fn classify<H: Host>(r: &Runner<'_, H>, e: pds::CallError, now: i64) -> Outcome {
    use pds::CallError;
    let attempts = r.w.attempts;
    let backoff = |error: EngineError| {
        if attempts >= MAX_ATTEMPTS {
            Outcome::Failed(error.with_retryable(true))
        } else {
            Outcome::Retry {
                at: now + r.backoff_ms(attempts),
                error,
            }
        }
    };
    match e {
        CallError::InvalidSwap => Outcome::Conflict(CallError::InvalidSwap.engine_error()),
        CallError::Ambiguous(error) => backoff(error),
        // A reset that is already past (a fast client clock, `Retry-After: 0`, a past HTTP
        // date) still waits: at least the default rate-limit wait, growing with the attempts
        // (consecutive 429s), never a retry every millisecond.
        CallError::RateLimited { until, error } => Outcome::Retry {
            at: if until > now {
                until
            } else {
                now + r
                    .backoff_ms(attempts)
                    .max(budget::DEFAULT_RATE_LIMIT_WAIT_MS)
            },
            error,
        },
        // The session was refused (expired, revoked, logged out): retrying cannot help until
        // the account logs in again, and backing off would hide why nothing is sent.
        CallError::Rejected { error, .. } if error.code == ErrorCode::AuthRequired => {
            Outcome::NeedsLogin(error)
        }
        CallError::Rejected { error, .. } => Outcome::Failed(error),
        // a read that failed in transport (offline) before anything was sent never makes the
        // write fail: nothing can have landed, so it waits for the network and tries again
        CallError::Read(error)
            if error.retryable && pds::is_offline(&error) && !r.pds.sent_any && !r.w.sent =>
        {
            Outcome::Retry {
                at: now + r.backoff_ms(attempts),
                error,
            }
        }
        CallError::Read(error) if error.retryable => backoff(error),
        CallError::Read(error) => Outcome::Failed(error),
    }
}

/// Builds the persisted write: rule-1 spills (§8.9), the validity and reserved-name checks, and
/// the blob chunk rows.
fn prepare(
    session: &Session,
    seq: i64,
    now: i64,
    new: NewWrite,
) -> Result<(PendingWrite, Vec<KvOp>), EngineError> {
    let did = &session.did;
    let account = did.as_str();
    let mut blobs = new.blobs;
    let mut ops = new.ops;
    for op in &mut ops {
        if op.action == OpAction::Delete {
            continue;
        }
        let Some(mut v) = op.value_json() else {
            if matches!(new.intent, Intent::Opinion { .. }) {
                continue;
            }
            return Err(EngineError::invalid_input(
                "a create or update needs a value",
            ));
        };
        if !v.is_object() {
            return Err(EngineError::invalid_input(
                "a record value is a JSON object",
            ));
        }
        let spilled = blob::spill_oversized(&mut v, &op.collection, op.excerpt.as_deref())?;
        if !spilled.is_empty() {
            op.set_value(&v);
            blobs.extend(spilled);
        }
        records::check(
            did,
            &op.collection,
            &op.rkey,
            &v,
            op.action == OpAction::Create,
        )?;
    }
    let referenced: BTreeSet<String> = ops
        .iter()
        .filter_map(Op::value_json)
        .flat_map(|v| blob::blob_links(&v))
        .collect();
    let mut pending = Vec::new();
    let mut chunks = Vec::new();
    let mut seen = BTreeSet::new();
    for b in blobs {
        if !referenced.contains(&b.cid) || !seen.insert(b.cid.clone()) {
            continue;
        }
        let index = u32::try_from(pending.len()).map_err(storage)?;
        let rows = protocol::chunk_rows(account, seq, index, &b.bytes).map_err(storage)?;
        pending.push(PendingBlob {
            cid: b.cid,
            mime: b.mime,
            size: b.bytes.len() as u64,
            chunks: u32::try_from(rows.len()).map_err(storage)?,
            uploaded: false,
            uploaded_at: None,
        });
        chunks.extend(rows);
    }
    let (state, not_before) = match new.hold_ms {
        Some(h) if h > 0 => (WriteStage::Held, Some(now + h)),
        _ => (WriteStage::Queued, None),
    };
    let w = PendingWrite {
        v: model::OUTBOX_ROW_VERSION,
        account: account.to_owned(),
        seq,
        intent: new.intent,
        ops,
        blobs: pending,
        state,
        attempts: 0,
        created_at: now,
        updated_at: now,
        not_before,
        needs_login: false,
        sent: false,
        results: Vec::new(),
        error: None,
        undo_of: new.undo_of,
        after: new.after,
    };
    Ok((w, chunks))
}

/// Why a write that depends on write `dep` of `account` ([`PendingWrite::after`]) may not be
/// sent: `None` once `dep` is done.
fn unmet_dependency(account: &str, dep: i64, d: Option<&PendingWrite>) -> Option<EngineError> {
    let id = model::write_id(account, dep);
    let why = match d.map(|d| (d.state, d.intent.name())) {
        Some((WriteStage::Done, _)) => return None,
        Some((WriteStage::Failed | WriteStage::Conflict, name)) => {
            format!("the {name} write {id} it depends on did not complete")
        }
        Some((WriteStage::Undone, name)) => {
            format!("the {name} write {id} it depends on was undone")
        }
        Some((_, name)) => format!("the {name} write {id} it depends on has not been sent"),
        None => format!("the write {id} it depends on is no longer in the outbox"),
    };
    Some(EngineError::conflict(format!("{why}; this write was not sent")).with_detail("after", id))
}

/// The `state` rows of `account`'s outbox, for purging an account: writes, blob chunks, the
/// budget and the meta row.
pub fn purge_ops(account: &str) -> Vec<KvOp> {
    vec![
        KvOp::DeleteRange(KeyRange::prefix(&model::writes_prefix(account))),
        KvOp::DeleteRange(KeyRange::prefix(&crate::key!(model::OUT_BLOB, account))),
        KvOp::Delete(model::budget_key(account)),
        KvOp::Delete(model::meta_key(account)),
    ]
}
