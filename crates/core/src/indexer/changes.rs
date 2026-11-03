// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::BTreeSet;
use std::sync::Arc;

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::tid::Tid;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys::PROFILE_RKEY;

use crate::discovery::forum::NEWCOMER_BUDGET;
use crate::host::Host;
use crate::live::Operation;
use crate::store::blob::CursorRow;
use crate::store::records::RecordStore;
use crate::store::schema;
use crate::sync::net::Priority;
use crate::sync::queue::{DirtyItem, ready_time};
use crate::sync::schedule::RECONCILE_PARTICIPANT_MS;
use crate::sync::{Reason, Session, Syncer};
use crate::xrpc::XrpcCallError;

use super::client::Client;
use super::contract::{Change, MAX_PROOF_URIS};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangePollReport {
    pub source: String,
    pub from: i64,
    pub cursor: i64,
    pub pages: u32,
    pub changes: usize,
    pub queued: usize,
    pub restarted: bool,
    /// Valid changes outside the forum's scope, never fetched.
    pub dropped: usize,
    /// Newcomer candidates (membership records at the forum's key by DIDs outside its scope),
    /// newest first, for discovery confirmation (`sync.md` §4.4). Nothing was fetched for them.
    pub candidates: Vec<String>,
}

/// What the change feed of one forum may make the client fetch (`sync.md` §15.4): records of
/// that forum by its authorities, granted DIDs, confirmed participants and the viewer's own
/// accounts. An indexer otherwise chooses which DIDs, and so which PDS hosts, every consenting
/// client contacts.
#[derive(Clone, Debug, Default)]
pub struct ChangeScope {
    /// The forum's fk.
    pub fk: String,
    /// The forum declaration, `(owner DID, rkey)`.
    pub declaration: (String, String),
    /// DIDs whose changes are fetched.
    pub participants: BTreeSet<String>,
    /// A: reconciled at P1 after `CursorTooOld`.
    pub authorities: BTreeSet<String>,
    /// C(F): reconciled after `CursorTooOld` (P3 outside A).
    pub reconcile: BTreeSet<String>,
    /// F ∈ F_new (open mode, or the viewer is an authority): newcomer candidates are kept.
    pub newcomers: bool,
}

enum Scoped {
    Fetch,
    Candidate,
    Drop,
}

impl ChangeScope {
    fn classify(&self, uri: &AtUri) -> Scoped {
        let (did, coll, rkey) = (
            uri.did().as_str(),
            uri.collection().as_str(),
            uri.rkey().as_str(),
        );
        let of_forum = schema::scope_of(coll, rkey) == self.fk
            || (coll == nsid::FORUM && (did, rkey) == (&*self.declaration.0, &*self.declaration.1))
            || (coll == nsid::PROFILE && rkey == PROFILE_RKEY);
        if !of_forum {
            Scoped::Drop
        } else if self.participants.contains(did) {
            Scoped::Fetch
        } else if self.newcomers && schema::is_membership(coll) && rkey == self.fk {
            Scoped::Candidate
        } else {
            Scoped::Drop
        }
    }
}

fn invalid(message: impl Into<String>) -> XrpcCallError {
    XrpcCallError::InvalidRequest(message.into())
}

fn validate_change(c: &Change, previous: i64) -> Result<Option<(AtUri, Did)>, XrpcCallError> {
    if c.seq <= previous {
        return Err(invalid(
            "getChanges sequence numbers are not strictly ascending",
        ));
    }
    let uri = AtUri::parse(&c.uri).map_err(|e| invalid(format!("bad change uri: {e}")))?;
    let did = Did::parse(&c.did).map_err(|e| invalid(format!("bad change DID: {e}")))?;
    if uri.did() != &did {
        return Err(invalid("getChanges uri and DID disagree"));
    }
    Tid::parse(&c.rev).map_err(|e| invalid(format!("bad change rev: {e}")))?;
    if let Some(cid) = &c.cid {
        lessdugin_atp::Cid::try_from(cid.as_str())
            .map_err(|e| invalid(format!("bad change CID: {e}")))?;
    }
    // Unknown enum values are ignored entry-by-entry (data-model.md §13.3).
    Ok(c.known_op().then_some((uri, did)))
}

fn source_name(client: &Client, forum: Option<&AtUri>) -> String {
    let fk = forum
        .and_then(|u| lessdugin_lexicon::keys::fk(u.as_str()).ok())
        .unwrap_or_else(|| "global".to_owned());
    format!("indexer:{}:{fk}", client.did())
}

/// Polls the durable exclusive cursor. Marks are persisted before the in-memory queue and cursor;
/// a crash therefore replays a page or leaves an ordinary persistent dirty mark, never skips it.
/// With `dedup` (`live_tail`'s Jetstream LRU), a commit already handled is skipped and a queued
/// one is recorded there (`sync.md` §6.6).
pub async fn poll_changes<H: Host>(
    syncer: &Syncer<H>,
    client: &Client,
    session: &Session<H>,
    forum: Option<&AtUri>,
    scope: &ChangeScope,
    mut dedup: Option<&mut crate::live::events::Dedup>,
) -> Result<ChangePollReport, XrpcCallError> {
    let source = source_name(client, forum);
    let store = RecordStore::new(session.host().cache());
    let now = session.host().now_ms();
    let stored = store.cursor(&source).await.map_err(store_error)?;
    let since = match stored.and_then(|c| c.seq) {
        Some(seq) => seq,
        // A first poll starts at the indexer's current head, not at its whole log: the PDS
        // sync covers what came before (sync.md §6.6).
        None => {
            let head = client.current_as_of(session).await?.seq.max(0);
            persist_cursor(&store, &source, head, now).await?;
            head
        }
    };
    let mut report = ChangePollReport {
        source: source.clone(),
        from: since,
        cursor: since,
        ..ChangePollReport::default()
    };
    let mut cursor = since;
    let mut restarted = false;
    let mut candidates = Vec::new();
    loop {
        let page = match client.get_changes(session, forum, cursor, 500).await {
            Err(e) if e.error_name() == Some("CursorTooOld") && !restarted => {
                restarted = true;
                report.restarted = true;
                // asOf is a server snapshot, not record evidence. It is only a restart point.
                cursor = client.current_as_of(session).await?.seq.max(0);
                // A full reconcile as for a Jetstream gap (§6.3): authorities at P1, the other
                // participants at P3. A feed that keeps answering CursorTooOld gets it at most
                // once per reconcile period; the daily reconcile (§6.8) covers the rest.
                let now_us = now.saturating_mul(1000);
                let last = store
                    .cursor(&source)
                    .await
                    .map_err(store_error)?
                    .and_then(|c| c.time_us);
                if last.is_none_or(|t| now_us.saturating_sub(t) >= RECONCILE_PARTICIPANT_MS * 1000)
                {
                    for did in &scope.reconcile {
                        let priority = if scope.authorities.contains(did) {
                            Priority::P1
                        } else {
                            Priority::P3
                        };
                        syncer.enqueue(DirtyItem::new(did, priority, Reason::Reconcile, now));
                        report.queued += 1;
                    }
                    stamp_reconcile(&store, &source, now_us, now).await?;
                }
                persist_cursor(&store, &source, cursor, now).await?;
                continue;
            }
            other => other?,
        };
        // A `#change` is handled like a Jetstream commit (sync.md §6.6): its first-seen is when
        // this client received it, never the indexer's `asOf` (§6.4, §5.6.5).
        let received_us = session.host().now_ms().saturating_mul(1000);
        let mut previous = cursor;
        let mut valid = Vec::with_capacity(page.changes.len());
        for change in &page.changes {
            if let Some(v) = validate_change(change, previous)? {
                valid.push((change, v));
            }
            previous = change.seq;
        }
        // The indexer drops blocklisted rows after paging (§15.3): its cursor may pass the last
        // returned row, never fall below it, and never pass its own snapshot.
        let expected = page.changes.last().map_or(cursor, |c| c.seq);
        if page.cursor < expected || page.cursor > page.meta.as_of.seq.max(expected) {
            return Err(invalid("getChanges returned an inconsistent cursor"));
        }
        for (change, (uri, did)) in valid {
            match scope.classify(&uri) {
                Scoped::Fetch => {}
                Scoped::Candidate => {
                    candidates.push(did.to_string());
                    continue;
                }
                Scoped::Drop => {
                    report.dropped += 1;
                    continue;
                }
            }
            let op = match change.op.as_str() {
                "create" => Operation::Create,
                "update" => Operation::Update,
                _ => Operation::Delete,
            };
            let commit = (uri.collection().as_str(), uri.rkey().as_str());
            if dedup
                .as_deref()
                .is_some_and(|d| d.has_commit(did.as_str(), &change.rev, commit.0, commit.1, op))
            {
                report.changes += 1;
                continue;
            }
            syncer
                .mark_hint(session, &uri, &change.rev)
                .await
                .map_err(sync_error)?;
            if let Some(cid) = &change.cid {
                syncer.note_hint_first_seen(uri.as_str(), cid, received_us);
            }
            let at = ready_time(Priority::P1, now, change.seq.unsigned_abs(), None);
            syncer.enqueue(
                DirtyItem::point(did.as_str(), uri.as_str(), Priority::P1, at)
                    .with_min_rev(&change.rev)
                    .with_mark_hint(uri.as_str(), change.cid.clone()),
            );
            if let Some(d) = dedup.as_deref_mut() {
                d.insert_commit(did.as_str(), &change.rev, commit.0, commit.1, op);
            }
            report.queued += 1;
            report.changes += 1;
        }
        cursor = page.cursor;
        persist_cursor(&store, &source, cursor, now).await?;
        report.pages += 1;
        report.cursor = cursor;
        if page.changes.len() < 500 || report.pages >= 20 {
            break;
        }
    }
    let mut seen = BTreeSet::new();
    report.candidates = candidates
        .into_iter()
        .rev()
        .filter(|d| seen.insert(d.clone()))
        .take(NEWCOMER_BUDGET)
        .collect();
    Ok(report)
}

/// Writes the cursor, keeping the row's last `CursorTooOld` reconcile time (`timeUs`).
async fn persist_cursor<K: crate::host::KvStore>(
    store: &RecordStore<'_, K>,
    source: &str,
    cursor: i64,
    now: i64,
) -> Result<(), XrpcCallError> {
    let mut row = CursorRow::new(now);
    row.time_us = store
        .cursor(source)
        .await
        .map_err(store_error)?
        .and_then(|c| c.time_us);
    row.seq = Some(cursor);
    let mut batch = store.batch();
    batch.put_cursor(source, &row).map_err(store_error)?;
    batch.commit().await.map_err(store_error)?;
    Ok(())
}

async fn stamp_reconcile<K: crate::host::KvStore>(
    store: &RecordStore<'_, K>,
    source: &str,
    now_us: i64,
    now: i64,
) -> Result<(), XrpcCallError> {
    let mut row = store
        .cursor(source)
        .await
        .map_err(store_error)?
        .unwrap_or_else(|| CursorRow::new(now));
    row.time_us = Some(now_us);
    let mut batch = store.batch();
    batch.put_cursor(source, &row).map_err(store_error)?;
    batch.commit().await.map_err(store_error)?;
    Ok(())
}

fn store_error(e: crate::store::StoreError) -> XrpcCallError {
    invalid(format!("indexer cursor store: {e}"))
}

fn sync_error(e: crate::sync::SyncError) -> XrpcCallError {
    invalid(format!("indexer dirty mark: {e}"))
}

/// Host-driven live batching. The service agent owns the timer/task; core only decides when a
/// batch is due and never spawns. A full batch is ready immediately, otherwise after 1–3 seconds.
#[derive(Clone, Debug)]
pub struct LiveProofBatch {
    uris: BTreeSet<AtUri>,
    flush_at_ms: i64,
    delay_ms: i64,
}

impl LiveProofBatch {
    pub fn new(now_ms: i64, random: u64) -> Self {
        let delay_ms = 1_000 + (random % 2_001) as i64;
        Self {
            uris: BTreeSet::new(),
            flush_at_ms: now_ms.saturating_add(delay_ms),
            delay_ms,
        }
    }

    pub fn push(&mut self, uri: AtUri) {
        self.uris.insert(uri);
    }

    pub fn is_full(&self) -> bool {
        self.uris.len() >= MAX_PROOF_URIS
    }

    pub fn is_due(&self, now_ms: i64) -> bool {
        !self.uris.is_empty() && (self.is_full() || now_ms >= self.flush_at_ms)
    }

    pub fn flush_at_ms(&self) -> i64 {
        self.flush_at_ms
    }

    pub fn take(&mut self, now_ms: i64) -> Vec<AtUri> {
        let out = self
            .uris
            .iter()
            .take(MAX_PROOF_URIS)
            .cloned()
            .collect::<Vec<_>>();
        for uri in &out {
            self.uris.remove(uri);
        }
        self.flush_at_ms = now_ms.saturating_add(self.delay_ms);
        out
    }

    pub fn len(&self) -> usize {
        self.uris.len()
    }

    pub fn is_empty(&self) -> bool {
        self.uris.is_empty()
    }
}

// Keep Arc out of the public batching interface while ensuring native futures remain Send.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Arc<str>>();
};
