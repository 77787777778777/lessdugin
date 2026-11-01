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

use crate::Indexer;
use crate::proofs;
use crate::store::HeadRow;

pub fn spawn(indexer: Indexer) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_millis(indexer.config.poll_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now = indexer.now_ms();
            let backfill = indexer.store.schedule_due("backfill", now);
            let mut discovery = Ok(());
            if backfill {
                discovery = discover(&indexer).await;
                if let Err(e) = &discovery {
                    warn!(error=%e,"declaration backfill discovery failed");
                }
            }
            let mut synced = true;
            if backfill || indexer.config.poll.is_some() {
                let forums = indexer.store.forum_seeds();
                for forum in forums {
                    if indexer.blocked(&forum) {
                        continue;
                    }
                    if let Err(e) = sync_forum(&indexer, &forum).await {
                        synced = false;
                        indexer.metrics.sync_errors.fetch_add(1, Ordering::Relaxed);
                        warn!(forum=%forum,error=%e,"forum sync failed");
                    }
                }
            }
            if backfill {
                indexer
                    .store
                    .schedule_next(
                        "backfill",
                        now.saturating_add(if discovery.is_ok() && synced {
                            6 * 60 * 60 * 1000
                        } else {
                            60 * 1000
                        }),
                    )
                    .ok();
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
    // Backfill proofs are lazy: a query may fetch them on demand. Refresh at most two bundles
    // after a changed snapshot so a large forum does not synchronously hydrate its whole history.
    if changes > 0 {
        let mut proof_uris = vec![model.key.uri.to_string()];
        proof_uris.extend(model.input.records.keys().take(49).cloned());
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
    info!(forum=%forum,changes,"forum snapshot indexed");
    Ok(())
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
                indexer.store.add_forum_seed(forum.as_str())?;
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
                    indexer
                        .store
                        .add_candidate(forum.as_str(), did.as_str(), &cid, created_ms)?;
                }
            }
        }
        match page.cursor {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => break,
        }
    }
    Ok(())
}
