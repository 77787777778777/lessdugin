// SPDX-License-Identifier: AGPL-3.0-or-later
//! Poll, six-hour backfill, per-forum retry, per-repo retry and daily reconcile scheduling. Every
//! network pass uses the real native core sync engine; relay listings and Jetstream are
//! discovery hints only. Projections are rebuilt from the cache under one lock, so live events
//! never need an ordinary forum sync (sync.md §15.1).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, Nsid};
use lessdugin_atp::xrpc::{ListRecords, ListReposByCollection};
use lessdugin_core::discovery::{ForumKey, SyncOpts};
use lessdugin_core::engine::{EngineError, ErrorCode};
use lessdugin_core::host::Host;
use lessdugin_core::model::{ForumModel, ModelError};
use lessdugin_core::store::records::RecordStore;
use lessdugin_core::store::repo::AccountStatus;
use lessdugin_core::sync::{Priority, Reason, Stats, SyncRequest};
use lessdugin_core::xrpc::{CallOpts, Service};
use tracing::{info, warn};

use crate::Indexer;
use crate::proofs;
use crate::store::{HeadRow, LiveChange, RETRY_BASE_MS, backoff_ms};

/// Discovery (relay enumeration) runs every six hours whatever individual forums do.
pub const BACKFILL_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// Due per-repo retries run per scheduler tick, at most this many.
const REPO_RETRIES_PER_TICK: usize = 64;

pub fn spawn(indexer: Indexer) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_millis(indexer.config.poll_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            tick(&indexer).await;
        }
    })
}

/// One scheduler pass.
pub async fn tick(indexer: &Indexer) {
    let now = indexer.now_ms();
    let backfill = indexer.store.schedule_due("backfill", now);
    if backfill {
        // The discovery cadence does not depend on whether individual forums sync: a forum
        // that fails backs off on its own (m6 per-forum retry), never by re-enumerating relays.
        let next = match discover(indexer).await {
            Ok(()) => {
                indexer.store.schedule_next("backfill_failures", 0).ok();
                now.saturating_add(BACKFILL_EVERY_MS)
            }
            Err(e) => {
                warn!(error=%e,"declaration backfill discovery failed");
                let failures = indexer
                    .store
                    .scheduled_at("backfill_failures")
                    .unwrap_or(0)
                    .saturating_add(1);
                indexer
                    .store
                    .schedule_next("backfill_failures", failures)
                    .ok();
                now.saturating_add(backoff_ms(RETRY_BASE_MS, failures).min(BACKFILL_EVERY_MS))
            }
        };
        indexer.store.schedule_next("backfill", next).ok();
    }
    let mut synced = BTreeSet::new();
    if backfill || indexer.config.poll.is_some() {
        for forum in indexer.store.forums_for_pass(now) {
            pass(indexer, &forum).await;
            synced.insert(forum);
        }
    }
    for forum in indexer.store.forums_due(now) {
        if !synced.contains(&forum) {
            pass(indexer, &forum).await;
        }
    }
    for retry in indexer.store.due_repo_retries(now, REPO_RETRIES_PER_TICK) {
        retry_repo(indexer, &retry.did, &retry.forum).await;
    }
    if indexer.store.schedule_due("reconcile", now) {
        reconcile(indexer).await;
        indexer
            .store
            .schedule_next("reconcile", now.saturating_add(24 * 60 * 60 * 1000))
            .ok();
    }
    indexer
        .store
        .prune_changes(now.saturating_sub(30 * 24 * 60 * 60 * 1000))
        .ok();
    if indexer.store.schedule_due("gc", now) {
        gc(indexer, now).await;
        indexer
            .store
            .schedule_next(
                "gc",
                now.saturating_add(lessdugin_core::limits::TOMBSTONE_GC_EVERY_MS),
            )
            .ok();
    }
}

async fn pass(indexer: &Indexer, forum: &str) {
    if indexer.blocked(forum) {
        return;
    }
    if let Err(e) = sync_forum(indexer, forum).await {
        indexer.metrics.sync_errors.fetch_add(1, Ordering::Relaxed);
        warn!(forum=%forum,error=%e,"forum sync failed");
    }
}

/// Daily garbage collection (sync.md §5.6.3): tombstones older than the 30-day retention leave
/// the core record store, and cached proofs of records no forum indexes any more leave
/// `proof_cache` after the same retention.
pub async fn gc(indexer: &Indexer, now_ms: i64) {
    let retention = lessdugin_core::limits::TOMBSTONE_MIN_RETENTION_MS;
    match RecordStore::new(indexer.engine.host().cache())
        .gc_tombstones(now_ms, retention)
        .await
    {
        Ok(n) if n > 0 => info!(tombstones = n, "expired tombstones dropped"),
        Ok(_) => {}
        Err(e) => warn!(error=%e,"tombstone GC failed"),
    }
    match indexer.store.prune_proofs(now_ms.saturating_sub(retention)) {
        Ok(n) if n > 0 => info!(proofs = n, "unindexed cached proofs dropped"),
        Ok(_) => {}
        Err(e) => warn!(error=%e,"proof cache GC failed"),
    }
}

/// Checks every indexed owner/participant repo once, deduplicated across forums. Only forums
/// containing a repo whose synced head moved are refreshed. A failed head or forum refresh is
/// isolated so it cannot prevent the remaining repos from being reconciled.
pub async fn reconcile(indexer: &Indexer) {
    let mut repos: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for forum in indexer.store.indexed_forums(None) {
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

/// The declaration (or its owner's DID) is proven absent, not merely unreachable.
fn declaration_absent(e: &anyhow::Error) -> bool {
    e.downcast_ref::<EngineError>()
        .is_some_and(|e| e.code == ErrorCode::NotFound)
        || e.downcast_ref::<ModelError>()
            .is_some_and(|e| matches!(e, ModelError::NoDeclaration(_)))
}

/// An ordinary forum sync (backfill, poll, reconcile, newcomer confirmation, a changed roster):
/// the core sync of the authorities and participants, then a projection. A declaration proven
/// absent is a successful outcome: the snapshot is emptied (each record logged as deleted) and
/// the seed quarantined. Any other failure backs off this forum only.
pub async fn sync_forum(indexer: &Indexer, forum: &str) -> anyhow::Result<()> {
    match sync_forum_inner(indexer, forum).await {
        Ok(()) => {
            indexer.store.note_forum_synced(forum, indexer.now_ms())?;
            Ok(())
        }
        Err(e) if declaration_absent(&e) => {
            let deletes = indexer.store.note_forum_absent(forum, indexer.now_ms())?;
            lock(&indexer.models).remove(forum);
            info!(forum=%forum,deletes,"forum declaration is absent; seed quarantined");
            Ok(())
        }
        Err(e) => {
            let retry = indexer
                .store
                .note_forum_failed(forum, indexer.now_ms(), &e.to_string())?;
            Err(e.context(format!("forum retry at {retry}")))
        }
    }
}

async fn sync_forum_inner(indexer: &Indexer, forum: &str) -> anyhow::Result<()> {
    let _one = indexer.sync_lock.lock().await;
    let uri = AtUri::parse(forum)?;
    let key = ForumKey::of_uri(&uri)?;
    let pending = indexer.store.candidates(forum, indexer.now_ms());
    let mut confirmations = BTreeMap::new();
    for (did, cid, _) in &pending {
        let membership = lessdugin_lexicon::keys::membership_rkey(key.fk.as_str())?;
        let at = AtUri::parse(&format!(
            "at://{did}/{}/{membership}",
            lessdugin_lexicon::nsid::MEMBERSHIP
        ))?;
        confirmations.insert(did.clone(), (at, cid.clone()));
    }
    // Preserve the durable relay enumeration order, including for authors whose membership has
    // never been cached locally. Do not reconstruct recency from the cache.
    let newcomers = pending
        .into_iter()
        .map(|(did, _, _)| (did, "relay".to_owned()))
        .collect();
    indexer
        .engine
        .sync_forum(
            forum,
            &SyncOpts {
                newcomers,
                background: true,
                ..SyncOpts::default()
            },
        )
        .await?;
    let decl = RecordStore::new(indexer.engine.host().cache())
        .get(&uri)
        .await?
        .and_then(|row| row.value_json().ok())
        .and_then(|value| {
            lessdugin_lexicon::records::Record::from_nsid_value(
                lessdugin_lexicon::nsid::FORUM,
                value,
            )
        })
        .and_then(|record| match record {
            Ok(lessdugin_lexicon::records::Record::Forum(decl)) => Some(decl),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("synced forum declaration is missing"))?;
    let state = lessdugin_core::discovery::ForumState::load(
        indexer.engine.host().cache(),
        &key,
        &decl,
        indexer.now_ms().saturating_mul(1000),
    )
    .await?;
    for (did, (at, expected_cid)) in confirmations {
        let state_name = if matches!(
            state.disc.get(&did).map(|d| &d.state),
            Some(lessdugin_core::discovery::DiscState::Confirmed)
        ) {
            Some("confirmed")
        } else if matches!(
            state.disc.get(&did).map(|d| &d.state),
            Some(lessdugin_core::discovery::DiscState::Rejected)
        ) {
            Some("rejected")
        } else {
            None
        };
        if let Some(name) = state_name {
            let current = RecordStore::new(indexer.engine.host().cache())
                .get(&at)
                .await?;
            // A confirmed membership absence cannot be matched to a relay CID. The PDS check
            // above is authoritative; never cache a superseded membership CID as the rejection.
            if name == "confirmed"
                || current.is_none()
                || current.is_some_and(|row| row.cid.to_string() == expected_cid)
            {
                indexer
                    .store
                    .set_candidate_state(forum, &did, name, indexer.now_ms())?;
            }
        }
    }
    drop(_one);
    let projection = project_forum(indexer, &key, None, &BTreeMap::new()).await?;
    indexer
        .metrics
        .synced_forums
        .fetch_add(1, Ordering::Relaxed);
    // Backfill proofs are lazy: a query may fetch them on demand. Refresh at most two bundles
    // after a changed snapshot so a large forum does not synchronously hydrate its whole history.
    if projection.changes > 0 {
        let mut proof_uris = vec![key.uri.to_string()];
        proof_uris.extend(projection.model.input.records.keys().take(49).cloned());
        for uri in proof_uris {
            let stale = indexer.store.proof(&uri).is_none_or(|proof| {
                proof
                    .cid
                    .as_deref()
                    .is_none_or(|cid| !indexer.store.indexed_record_has_cid(&uri, cid))
            });
            if stale && let Err(e) = proofs::fetch(indexer, &uri).await {
                warn!(uri=%uri,error=%e,"background proof fetch failed");
            }
        }
    }
    info!(forum=%forum,changes=projection.changes,"forum snapshot indexed");
    Ok(())
}

/// A projection applied by [`project_forum`].
pub struct Projection {
    pub model: Arc<ForumModel>,
    pub changes: usize,
}

pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Rebuilds `key`'s projection from the cache alone (no request): the fold, the side tables and
/// the cached query model. Head rows are refreshed for `authors` and for every repository with
/// a changed record (`None`: for every owner, participant and author, after an ordinary sync).
/// Changes matching a `live` commit are logged with that commit's rev.
pub async fn project_forum(
    indexer: &Indexer,
    key: &ForumKey,
    authors: Option<&BTreeSet<String>>,
    live: &BTreeMap<String, LiveChange>,
) -> anyhow::Result<Projection> {
    let _one = indexer.project_lock.lock().await;
    let built_ms = indexer.now_ms();
    let model = ForumModel::build(
        indexer.engine.host().cache(),
        key,
        built_ms.saturating_mul(1000),
    )
    .await?;
    let cost_ms = indexer.now_ms().saturating_sub(built_ms);
    let store = RecordStore::new(indexer.engine.host().cache());
    let mut dids: BTreeSet<String> = match authors {
        Some(authors) => authors.clone(),
        None => {
            let mut all: BTreeSet<String> = model.participants().iter().cloned().collect();
            all.insert(key.owner.to_string());
            all.extend(model.input.records.values().map(|s| s.did.clone()));
            all
        }
    };
    dids.extend(indexer.store.changed_dids(&model)?);
    let refs: Vec<&str> = dids.iter().map(String::as_str).collect();
    let rows = store.repos(&refs).await?;
    let mut revs = BTreeMap::new();
    let mut heads = Vec::new();
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
            heads.push(HeadRow {
                did: did.clone(),
                rev: (!rev.is_empty()).then_some(rev),
                commit,
                pds: (!row.pds.is_empty()).then_some(row.pds),
                checked_ms,
                status,
            });
        }
    }
    indexer.store.put_heads(&heads)?;
    let changes = indexer
        .store
        .apply_model(&model, indexer.now_ms(), &revs, live)?;
    let model = Arc::new(model);
    lock(&indexer.models).project(key.uri.as_str(), Arc::clone(&model), built_ms, cost_ms);
    indexer
        .metrics
        .changes_written
        .fetch_add(changes as u64, Ordering::Relaxed);
    Ok(Projection { model, changes })
}

/// The cached query model of an indexed forum. It is folded from the cache only when it is
/// missing (after a restart or an eviction) or past its age bound ([`crate::MODEL_AGE_FACTOR`]),
/// one fold at a time and never under the projection lock; never per request.
pub async fn model(indexer: &Indexer, key: &ForumKey) -> anyhow::Result<Arc<ForumModel>> {
    let forum = key.uri.as_str();
    if let Some(m) = lock(&indexer.models).get(forum, indexer.now_ms()) {
        return Ok(m);
    }
    let _one = indexer
        .query_folds
        .acquire()
        .await
        .map_err(|_| anyhow::anyhow!("the indexer is shutting down"))?;
    let epoch = {
        let mut models = lock(&indexer.models);
        if let Some(m) = models.get(forum, indexer.now_ms()) {
            return Ok(m);
        }
        models.epoch(forum)
    };
    let built_ms = indexer.now_ms();
    let model = Arc::new(
        ForumModel::build(
            indexer.engine.host().cache(),
            key,
            built_ms.saturating_mul(1000),
        )
        .await?,
    );
    let cost_ms = indexer.now_ms().saturating_sub(built_ms);
    indexer
        .metrics
        .query_model_builds
        .fetch_add(1, Ordering::Relaxed);
    lock(&indexer.models).put_if(forum, epoch, Arc::clone(&model), built_ms, cost_ms);
    Ok(model)
}

/// The per-repo retry of a live event that could not be applied: a targeted resync of the
/// repository's range of `forum`, then a projection from the cache. A failure backs off
/// exponentially; it never holds the Jetstream cursor.
pub async fn retry_repo(indexer: &Indexer, did: &str, forum: &str) {
    indexer.metrics.repo_retries.fetch_add(1, Ordering::Relaxed);
    let result = async {
        let parsed = Did::parse(did)?;
        let key = ForumKey::of_uri(&AtUri::parse(forum)?)?;
        if indexer.store.forum(forum).is_none() {
            // Not indexed (yet): an ordinary sync of the forum covers this repository.
            indexer.store.schedule_forum_sync(forum, indexer.now_ms())?;
            return Ok(());
        }
        resync_repo(indexer, &parsed, &key).await?;
        project_forum(
            indexer,
            &key,
            Some(&BTreeSet::from([did.to_owned()])),
            &BTreeMap::new(),
        )
        .await?;
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            indexer.store.clear_repo_retry(did, forum).ok();
        }
        Err(e) if declaration_absent(&e) => {
            indexer.store.clear_repo_retry(did, forum).ok();
            indexer
                .store
                .schedule_forum_sync(forum, indexer.now_ms())
                .ok();
        }
        Err(e) => {
            let at = indexer
                .store
                .schedule_repo_retry(did, forum, indexer.now_ms(), &e.to_string())
                .unwrap_or(0);
            warn!(did=%did,forum=%forum,error=%e,retry_at=at,"repository retry failed");
        }
    }
}

/// One `sync_repo` of `did`'s range of `key` (no forum-wide sync).
pub async fn resync_repo(indexer: &Indexer, did: &Did, key: &ForumKey) -> anyhow::Result<()> {
    let session = indexer
        .engine
        .syncer()
        .session(Priority::P1, Arc::new(Stats::new()));
    let report = indexer
        .engine
        .syncer()
        .sync_repo(
            &session,
            &SyncRequest::forum(did.clone(), &key.fk, Reason::Resync),
        )
        .await;
    match report.error {
        Some(error) => Err(anyhow::anyhow!(
            "targeted repository resync failed: {error}"
        )),
        None => Ok(()),
    }
}

pub async fn discover(indexer: &Indexer) -> anyhow::Result<()> {
    let mut failures = Vec::new();
    let relays = indexer.config.relays.clone();
    for relay in relays {
        for collection in [
            lessdugin_lexicon::nsid::FORUM,
            lessdugin_lexicon::nsid::MEMBERSHIP,
        ] {
            let mut cursor = None;
            for _ in 0..1000 {
                let params = ListReposByCollection {
                    collection: Nsid::parse(collection)?,
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
                    if let Err(e) = records_of(indexer, &did, collection).await {
                        warn!(did=%did,collection,error=%e,"backfill record listing failed");
                        failures.push(format!("{did}/{collection}: {e}"));
                    }
                }
                match page.cursor {
                    Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                    _ => break,
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{} repository discovery passes failed: {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}

async fn records_of(indexer: &Indexer, did: &Did, collection: &str) -> anyhow::Result<()> {
    let ident = indexer.engine.resolver().resolve_did_fresh(did, 0).await?;
    let Some(pds) = ident.pds_url() else {
        return Ok(());
    };
    // Quotas are per DID and per PDS host (threat-model.md INFRA-12).
    let host = pds.host_str().unwrap_or_default().to_owned();
    if indexer.blocked_host(pds.as_str()) {
        return Ok(());
    }
    let mut refused = 0usize;
    let mut cursor = None;
    for _ in 0..100 {
        let params = ListRecords {
            repo: AtIdentifier::Did(did.clone()),
            collection: Nsid::parse(collection)?,
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
            let (Some(uri), Some(cid), Some(value)) = (record.uri, record.cid, record.value) else {
                continue;
            };
            if cid.is_empty() {
                continue;
            }
            let Ok(uri) = AtUri::parse(&uri) else {
                continue;
            };
            if uri.did() != did
                || uri.collection().as_str() != collection
                || indexer.blocked(uri.as_str())
                || lessdugin_lexicon::validity::check_uri(&uri, &value)
                    .first_rule()
                    .is_some()
            {
                continue;
            }
            let forum = if collection == lessdugin_lexicon::nsid::FORUM {
                Some(uri)
            } else {
                match lessdugin_lexicon::records::Record::from_nsid_value(collection, value.clone())
                {
                    Some(Ok(lessdugin_lexicon::records::Record::Membership(m))) => Some(m.forum),
                    _ => None,
                }
            };
            if let Some(forum) = forum
                && !indexer.blocked(forum.as_str())
            {
                if !indexer
                    .store
                    .add_forum_hint(forum.as_str(), did.as_str(), &host)?
                {
                    refused += 1;
                    continue;
                }
                if collection == lessdugin_lexicon::nsid::MEMBERSHIP {
                    let created_ms =
                        lessdugin_lexicon::records::Record::from_nsid_value(collection, value)
                            .and_then(|r| match r {
                                Ok(lessdugin_lexicon::records::Record::Membership(m)) => {
                                    Some(m.created_at.timestamp_micros().div_euclid(1000))
                                }
                                _ => None,
                            })
                            .unwrap_or(0);
                    if !indexer.store.add_candidate_from(
                        forum.as_str(),
                        did.as_str(),
                        &cid,
                        created_ms,
                        &host,
                    )? {
                        refused += 1;
                    }
                }
            }
        }
        match page.cursor {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => break,
        }
    }
    if refused > 0 {
        warn!(did=%did,host=%host,collection,refused,"forum hints beyond the per-DID or per-host quota were ignored");
    }
    Ok(())
}
