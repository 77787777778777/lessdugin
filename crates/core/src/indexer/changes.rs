// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::BTreeSet;
use std::sync::Arc;

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_atp::tid::Tid;

use crate::host::Host;
use crate::store::blob::CursorRow;
use crate::store::records::RecordStore;
use crate::sync::net::Priority;
use crate::sync::queue::{DirtyItem, ready_time};
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
pub async fn poll_changes<H: Host>(
    syncer: &Syncer<H>,
    client: &Client,
    session: &Session<H>,
    forum: Option<&AtUri>,
    reconcile_dids: &[Did],
) -> Result<ChangePollReport, XrpcCallError> {
    let source = source_name(client, forum);
    let store = RecordStore::new(session.host().cache());
    let now = session.host().now_ms();
    let stored = store.cursor(&source).await.map_err(store_error)?;
    let since = stored.and_then(|c| c.seq).unwrap_or(0);
    let mut report = ChangePollReport {
        source: source.clone(),
        from: since,
        cursor: since,
        ..ChangePollReport::default()
    };
    let mut cursor = since;
    let mut restarted = false;
    loop {
        let page = match client.get_changes(session, forum, cursor, 500).await {
            Err(e) if e.error_name() == Some("CursorTooOld") && !restarted => {
                restarted = true;
                report.restarted = true;
                // asOf is a server snapshot, not record evidence. It is only a restart point.
                cursor = client.current_as_of(session).await?.seq.max(0);
                for did in reconcile_dids {
                    syncer.enqueue(DirtyItem::new(
                        did.as_str(),
                        Priority::P0,
                        Reason::Reconcile,
                        now,
                    ));
                    report.queued += 1;
                }
                persist_cursor(&store, &source, cursor, now).await?;
                continue;
            }
            other => other?,
        };
        let mut previous = cursor;
        let mut valid = Vec::with_capacity(page.changes.len());
        for change in &page.changes {
            if let Some(v) = validate_change(change, previous)? {
                valid.push((change, v));
            }
            previous = change.seq;
        }
        let expected = page.changes.last().map_or(cursor, |c| c.seq);
        if page.cursor != expected || page.cursor < cursor {
            return Err(invalid("getChanges returned an inconsistent cursor"));
        }
        for (change, (uri, did)) in valid {
            syncer
                .mark_hint(session, &uri, &change.rev)
                .await
                .map_err(sync_error)?;
            if let Some(cid) = &change.cid {
                syncer.note_hint_first_seen(uri.as_str(), cid, page.meta.as_of.time_us);
            }
            let at = ready_time(Priority::P1, now, change.seq.unsigned_abs(), None);
            syncer.enqueue(
                DirtyItem::point(did.as_str(), uri.as_str(), Priority::P1, at)
                    .with_min_rev(&change.rev)
                    .with_mark_hint(uri.as_str(), change.cid.clone()),
            );
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
    Ok(report)
}

async fn persist_cursor<K: crate::host::KvStore>(
    store: &RecordStore<'_, K>,
    source: &str,
    cursor: i64,
    now: i64,
) -> Result<(), XrpcCallError> {
    let mut row = CursorRow::new(now);
    row.seq = Some(cursor);
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
