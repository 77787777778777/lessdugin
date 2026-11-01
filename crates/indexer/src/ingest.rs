// SPDX-License-Identifier: AGPL-3.0-or-later
//! Poll, six-hour backfill and daily reconcile scheduling. Every pass uses the real native core
//! sync engine; relay listings and Jetstream are discovery hints only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, Nsid};
use lessdugin_atp::xrpc::{ListRecords, ListReposByCollection};
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::host::Host;
use lessdugin_core::model::ForumModel;
use lessdugin_core::store::records::RecordStore;
use lessdugin_core::store::repo::AccountStatus;
use lessdugin_core::sync::{Priority, Stats};
use lessdugin_core::xrpc::{CallOpts, Service};
use tracing::{info, warn};

use crate::proofs;
use crate::store::HeadRow;
use crate::{Indexer, NS};

pub fn spawn(indexer: Indexer) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_millis(indexer.config.poll_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now = indexer.now_ms();
            let backfill = indexer.store.schedule_due("backfill", now);
            if backfill {
                if let Err(e) = discover(&indexer).await {
                    warn!(error=%e,"declaration backfill discovery failed");
                }
                indexer
                    .store
                    .schedule_next("backfill", now.saturating_add(6 * 60 * 60 * 1000))
                    .ok();
            }
            if backfill || indexer.config.poll.is_some() {
                let forums = indexer.store.forum_seeds();
                for forum in forums {
                    if indexer.blocked(&forum) {
                        continue;
                    }
                    if let Err(e) = sync_forum(&indexer, &forum).await {
                        indexer.metrics.sync_errors.fetch_add(1, Ordering::Relaxed);
                        warn!(forum=%forum,error=%e,"forum sync failed");
                    }
                }
            }
            if indexer.store.schedule_due("reconcile", now) {
                reconcile(&indexer).await;
                indexer
                    .store
                    .schedule_next("reconcile", now.saturating_add(24 * 60 * 60 * 1000))
                    .ok();
            }
            indexer
                .store
                .prune_changes(now.saturating_sub(30 * 24 * 60 * 60 * 1000))
                .ok();
        }
    })
}

/// Checks every indexed owner/participant repo once, deduplicated across forums. Only forums
/// containing a repo whose synced head moved are refreshed. A failed head or forum refresh is
/// isolated so it cannot prevent the remaining repos from being reconciled.
pub async fn reconcile(indexer: &Indexer) {
    let mut repos: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for forum in indexer.store.forums(None) {
        if indexer.blocked(&forum.uri) {
            continue;
        }
        let participants = indexer.store.participants(&forum.uri, None);
        for did in std::iter::once(forum.did).chain(participants.into_iter().map(|p| p.did)) {
            if !indexer.blocked(&did) {
                repos.entry(did).or_default().insert(forum.uri.clone());
            }
        }
    }

    let session = indexer
        .engine
        .syncer()
        .session(Priority::P3, Arc::new(Stats::new()));
    let mut affected = BTreeSet::new();
    for (did, forums) in repos {
        let Ok(parsed) = Did::parse(&did) else {
            indexer.metrics.sync_errors.fetch_add(1, Ordering::Relaxed);
            warn!(did=%did,"reconcile skipped an invalid indexed DID");
            continue;
        };
        match indexer
            .engine
            .syncer()
            .head_check(&session, &parsed, false)
            .await
        {
            Ok(Some(_)) => affected.extend(forums),
            Ok(None) => {}
            Err(e) => {
                indexer.metrics.sync_errors.fetch_add(1, Ordering::Relaxed);
                warn!(did=%did,error=%e,"reconcile head check failed");
            }
        }
    }
    for forum in affected {
        if let Err(e) = sync_forum(indexer, &forum).await {
            indexer.metrics.sync_errors.fetch_add(1, Ordering::Relaxed);
            warn!(forum=%forum,error=%e,"reconcile forum sync failed");
        }
    }
}

pub async fn sync_forum(indexer: &Indexer, forum: &str) -> anyhow::Result<()> {
    let _one = indexer.sync_lock.lock().await;
    let uri = AtUri::parse(forum)?;
    let key = ForumKey::of_uri(&uri)?;
    indexer
        .engine
        .sync_forum(forum, &SyncOpts::default())
        .await?;
    let now_us = indexer.now_ms().saturating_mul(1000);
    let model = ForumModel::build(indexer.engine.host().cache(), &key, now_us).await?;
    let store = RecordStore::new(indexer.engine.host().cache());
    let mut dids: BTreeSet<String> = model.participants().iter().cloned().collect();
    dids.insert(key.owner.to_string());
    dids.extend(model.input.records.values().map(|s| s.did.clone()));
    let refs: Vec<&str> = dids.iter().map(String::as_str).collect();
    let rows = store.repos(&refs).await?;
    let mut revs = BTreeMap::new();
    for (did, row) in dids.iter().zip(rows) {
        if let Some(row) = row {
            let rev = row
                .head_seen
                .as_ref()
                .map(|h| h.rev.clone())
                .filter(|r| !r.is_empty())
                .unwrap_or(row.rev.clone());
            let commit = row
                .head_seen
                .as_ref()
                .map(|h| h.cid.to_string())
                .or_else(|| row.commit.as_ref().map(|c| c.to_string()));
            if !rev.is_empty() {
                revs.insert(did.clone(), rev.clone());
            }
            let status = match row.status {
                AccountStatus::Active => "active",
                AccountStatus::Temporary => match row.status_detail.as_deref() {
                    Some("throttled") => "throttled",
                    _ => "desynchronized",
                },
                AccountStatus::Inactive => match row.status_detail.as_deref() {
                    Some("takendown") => "takendown",
                    Some("suspended") => "suspended",
                    Some("deleted") => "deleted",
                    _ => "deactivated",
                },
            }
            .to_owned();
            let checked_ms = if row.status_at > 0 {
                row.status_at
            } else {
                indexer.now_ms()
            };
            indexer.store.put_head(&HeadRow {
                did: did.clone(),
                rev: (!rev.is_empty()).then_some(rev),
                commit,
                pds: (!row.pds.is_empty()).then_some(row.pds),
                checked_ms,
                status,
            })?;
        }
    }
    let changes = indexer.store.apply_model(&model, indexer.now_ms(), &revs)?;
    indexer
        .metrics
        .synced_forums
        .fetch_add(1, Ordering::Relaxed);
    indexer
        .metrics
        .changes_written
        .fetch_add(changes as u64, Ordering::Relaxed);
    // Backfill proofs are lazy with respect to HTTP queries but fetched asynchronously after a
    // changed snapshot. Live ingest fetches the event's proof before acknowledging its cursor.
    if changes > 0 {
        let mut proof_uris = vec![model.key.uri.to_string()];
        proof_uris.extend(model.input.records.keys().take(99).cloned());
        for uri in proof_uris {
            if indexer.store.proof(&uri).is_none() {
                let _ = proofs::fetch(indexer, &uri).await;
            }
        }
    }
    info!(forum=%forum,changes,"forum snapshot indexed");
    Ok(())
}

async fn discover(indexer: &Indexer) -> anyhow::Result<()> {
    let relays = indexer.config.relays.clone();
    for relay in relays {
        let mut cursor = None;
        for _ in 0..1000 {
            let params = ListReposByCollection {
                collection: Nsid::parse(&format!("{NS}.forum"))?,
                limit: Some(500),
                cursor: cursor.clone(),
            };
            let page = indexer
                .engine
                .xrpc()
                .query_endpoint(&Service::Anon(relay.clone()), &params, &CallOpts::default())
                .await?;
            for repo in page.repos {
                let Some(did) = repo.did else {
                    continue;
                };
                let Ok(did) = Did::parse(&did) else {
                    continue;
                };
                if indexer.blocked(did.as_str()) {
                    continue;
                }
                if let Err(e) = declarations_of(indexer, &did).await {
                    warn!(did=%did,error=%e,"declaration listing failed");
                }
            }
            match page.cursor {
                Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                _ => break,
            }
        }
    }
    Ok(())
}

async fn declarations_of(indexer: &Indexer, did: &Did) -> anyhow::Result<()> {
    let ident = indexer.engine.resolver().resolve_did_fresh(did, 0).await?;
    let Some(pds) = ident.pds_url() else {
        return Ok(());
    };
    let mut cursor = None;
    for _ in 0..100 {
        let params = ListRecords {
            repo: AtIdentifier::Did(did.clone()),
            collection: Nsid::parse(&format!("{NS}.forum"))?,
            limit: Some(100),
            cursor: cursor.clone(),
            reverse: None,
        };
        let page = indexer
            .engine
            .xrpc()
            .query_endpoint(&Service::Anon(pds.clone()), &params, &CallOpts::default())
            .await?;
        for record in page.records {
            if let Some(uri) = record.uri
                && AtUri::parse(&uri).is_ok()
            {
                indexer.store.add_forum_seed(&uri)?;
            }
        }
        match page.cursor {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => break,
        }
    }
    Ok(())
}
