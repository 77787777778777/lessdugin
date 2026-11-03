// SPDX-License-Identifier: AGPL-3.0-or-later
//! Two concurrent wildcard Jetstream consumers. Both feed one core-derived relevance filter and
//! one shared 10,000-entry deduplicator. A cursor is acknowledged only after point admission,
//! proof acquisition and a real forum fold have completed.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::host::{Host, WebSocket, WsMessage};
use lessdugin_core::live::events::{Action, Context, EventFilter};
use lessdugin_core::live::source::{Cursor, DEDUP_CAPACITY, MAX_MESSAGE_BYTES};
use lessdugin_core::live::wire::{self, Wire};
use lessdugin_core::sync::repo::MarkHint;
use lessdugin_core::sync::{Priority, Stats};
use lessdugin_lexicon::collection::nsid;
use tokio::sync::Mutex;
use tracing::{debug, warn};
use url::Url;

use crate::{Indexer, NS, ingest, proofs};

const REWIND_US: i64 = 10_000_000;

pub fn spawn(indexer: Indexer) {
    if indexer.config.jetstream.is_empty() {
        return;
    }
    let filter = Arc::new(Mutex::new(EventFilter::with_dedup_capacity(DEDUP_CAPACITY)));
    let endpoints = indexer
        .config
        .jetstream
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>();
    for endpoint in endpoints {
        let indexer = indexer.clone();
        let filter = Arc::clone(&filter);
        tokio::spawn(async move {
            let mut backoff = Duration::from_millis(250);
            loop {
                match connection(&indexer, &endpoint, Arc::clone(&filter)).await {
                    Ok(()) => backoff = Duration::from_millis(250),
                    Err(e) => {
                        warn!(endpoint=%endpoint,error=%e,"Jetstream connection ended");
                    }
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        });
    }
}

fn source_name(endpoint: &Url) -> String {
    format!("jetstream:{}", endpoint.origin().ascii_serialization())
}

fn subscribe(endpoint: &Url, cursor: Option<i64>, now_us: i64) -> Url {
    let mut url = endpoint.clone();
    url.set_path("/xrpc/network.bsky.jetstream.subscribeEvents");
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut()
        .append_pair("collections", &format!("{NS}.*"))
        .append_pair(
            "cursor",
            &cursor
                .map_or_else(
                    || now_us.saturating_sub(REWIND_US),
                    |c| c.saturating_sub(REWIND_US),
                )
                .to_string(),
        );
    url
}

async fn connection(
    indexer: &Indexer,
    endpoint: &Url,
    filter: Arc<Mutex<EventFilter>>,
) -> anyhow::Result<()> {
    let name = source_name(endpoint);
    let previous = indexer
        .store
        .cursor(&name)
        .and_then(|v| v.parse::<i64>().ok());
    let url = subscribe(endpoint, previous, indexer.now_ms().saturating_mul(1000));
    let mut socket = indexer
        .engine
        .host()
        .websocket(url, MAX_MESSAGE_BYTES)
        .await?;
    indexer
        .metrics
        .live_connections
        .fetch_add(1, Ordering::Relaxed);
    let result = async {
        loop {
            let pending = filter.lock().await.retry();
            let Some((_, origin, _, _, out)) = pending else {
                break;
            };
            for action in out.actions {
                apply(indexer, action).await?;
            }
            if !filter.lock().await.acknowledge_for(Wire::V2, &origin) {
                continue;
            }
            if let Some(Cursor { time_us, .. }) = out.cursor {
                indexer
                    .store
                    .set_cursor(&format!("jetstream:{origin}"), &time_us.to_string())?;
            }
        }
        loop {
            if filter.lock().await.has_pending() {
                break;
            }
            let Some(msg) = socket.next().await else {
                break;
            };
            let text = match msg? {
                WsMessage::Text(t) => t,
                WsMessage::Binary(b) => String::from_utf8(b)?,
            };
            let event = match wire::parse(Wire::V2, &text) {
                Ok(e) => e,
                Err(e) => {
                    debug!(error=%e,"ignored malformed Jetstream event");
                    continue;
                }
            };
            let Some(event) = event else {
                continue;
            };
            indexer.metrics.live_events.fetch_add(1, Ordering::Relaxed);
            let cx = context(indexer);
            let out = filter.lock().await.apply(
                Wire::V2,
                &endpoint.origin().ascii_serialization(),
                event,
                &cx,
                // first-seen is when this indexer received the frame, never the frame's time
                indexer.now_ms().saturating_mul(1_000),
            );
            if out.duplicate {
                indexer
                    .metrics
                    .live_duplicates
                    .fetch_add(1, Ordering::Relaxed);
            }
            for action in out.actions {
                apply(indexer, action).await?;
            }
            filter
                .lock()
                .await
                .acknowledge_for(Wire::V2, &endpoint.origin().ascii_serialization());
            if let Some(Cursor { time_us, .. }) = out.cursor {
                indexer.store.set_cursor(&name, &time_us.to_string())?;
            }
            if out.restart_cursor {
                indexer
                    .store
                    .set_cursor(&name, &indexer.now_ms().saturating_mul(1000).to_string())?;
            }
        }
        Ok(())
    }
    .await;
    indexer
        .metrics
        .live_connections
        .fetch_sub(1, Ordering::Relaxed);
    socket.close().await;
    result
}

fn context(indexer: &Indexer) -> Context {
    let mut cx = Context::default();
    for forum in indexer.store.forums(None) {
        if let Ok(uri) = AtUri::parse(&forum.uri)
            && let Ok(key) = ForumKey::of_uri(&uri)
        {
            cx.forums.insert(key.fk.clone());
            cx.newcomer_forums.insert(key.fk);
            cx.declarations
                .insert((forum.did.clone(), uri.rkey().to_string()));
            cx.known_dids.insert(forum.did.clone());
            cx.authorities.insert(forum.did);
            cx.participants_known |= forum.synced_ms > 0;
        }
        for p in indexer.store.participants(&forum.uri, None) {
            cx.known_dids.insert(p.did.clone());
            cx.newcomer_dids.insert(p.did);
        }
    }
    cx
}

async fn apply(indexer: &Indexer, action: Action) -> anyhow::Result<()> {
    match action {
        Action::PointFetch {
            uri,
            rev,
            cid,
            priority,
            ..
        } => {
            point(indexer, &uri, &rev, cid, priority).await?;
        }
        Action::Candidate { uri, rev, cid, .. } => {
            point(indexer, &uri, &rev, cid, Priority::P1).await?
        }
        Action::InvalidateIdentity { did, authority } => {
            indexer.engine.invalidate_identity(&did).await?;
            if authority {
                refresh_forums(indexer, &did).await?;
            }
        }
        Action::AccountStatus { did, .. } => {
            let parsed = Did::parse(&did)?;
            let session = indexer
                .engine
                .syncer()
                .session(Priority::P1, Arc::new(Stats::new()));
            indexer
                .engine
                .syncer()
                .check_account_status(&session, &parsed)
                .await?;
            refresh_forums(indexer, &did).await?;
        }
        Action::Resync { did } | Action::CatchUp { did } => {
            resync(indexer, &did).await?;
        }
        Action::Reconcile(_) => {
            for forum in indexer.store.forum_seeds() {
                ingest::sync_forum(indexer, &forum).await?;
            }
        }
    }
    Ok(())
}

fn did_forums(indexer: &Indexer, did: &str) -> Vec<String> {
    indexer
        .store
        .forum_seeds()
        .into_iter()
        .filter(|forum| {
            indexer.store.forum(forum).is_some_and(|row| {
                row.did == did
                    || indexer
                        .store
                        .participants(forum, None)
                        .iter()
                        .any(|p| p.did == did)
            })
        })
        .collect()
}

async fn refresh_forums(indexer: &Indexer, did: &str) -> anyhow::Result<()> {
    for forum in did_forums(indexer, did) {
        ingest::sync_forum(indexer, &forum).await?;
    }
    Ok(())
}

async fn resync(indexer: &Indexer, did: &str) -> anyhow::Result<()> {
    let parsed = Did::parse(did)?;
    let session = indexer
        .engine
        .syncer()
        .session(Priority::P1, Arc::new(Stats::new()));
    let _one = indexer.sync_lock.lock().await;
    let forums = did_forums(indexer, did);
    for forum in &forums {
        let key = ForumKey::of_uri(&AtUri::parse(forum)?)?;
        let report = indexer
            .engine
            .syncer()
            .sync_repo(
                &session,
                &lessdugin_core::sync::SyncRequest::forum(
                    parsed.clone(),
                    &key.fk,
                    lessdugin_core::sync::Reason::Resync,
                ),
            )
            .await;
        if let Some(error) = report.error {
            return Err(anyhow::anyhow!(
                "targeted repository resync failed: {error}"
            ));
        }
    }
    drop(_one);
    for forum in forums {
        ingest::sync_forum(indexer, &forum).await?;
    }
    Ok(())
}

async fn point(
    indexer: &Indexer,
    uri: &str,
    rev: &str,
    cid: Option<String>,
    priority: Priority,
) -> anyhow::Result<()> {
    if indexer.blocked(uri) {
        return Ok(());
    }
    let uri = AtUri::parse(uri)?;
    let hint = MarkHint {
        cid: cid.as_deref().map(lessdugin_atp::cid::parse).transpose()?,
    };
    let session = indexer
        .engine
        .syncer()
        .session(priority, Arc::new(Stats::new()));
    indexer
        .engine
        .syncer()
        .point_fetch(&session, &uri, Some(&hint))
        .await?;
    // A live event is not acknowledged until its independent proof is cached (including a
    // proven-absence proof for a deletion).
    proofs::fetch(indexer, uri.as_str()).await?;
    let mut forums = BTreeSet::new();
    for forum in indexer.store.forum_seeds() {
        let f = AtUri::parse(&forum)?;
        if uri.collection().as_str() == nsid::FORUM && uri == f {
            forums.insert(forum);
            continue;
        }
        if let Ok(key) = ForumKey::of_uri(&f)
            && uri.rkey().as_str().starts_with(&key.fk)
        {
            forums.insert(forum);
        }
    }
    for forum in forums {
        ingest::sync_forum(indexer, &forum).await?;
    }
    let _ = rev; // the claimed rev is a hint; core stores only the PDS response's actual head.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_restart_rewinds_ten_seconds() {
        let endpoint = Url::parse("wss://jetstream.example.test").unwrap();
        let url = subscribe(&endpoint, Some(50_000_000), 100_000_000);
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["collections"], format!("{NS}.*"));
        assert_eq!(q["cursor"], "40000000");
        let fresh = subscribe(&endpoint, None, 100_000_000);
        assert_eq!(
            fresh.query_pairs().find(|(k, _)| k == "cursor").unwrap().1,
            "90000000"
        );
    }

    use std::collections::HashMap;
}
