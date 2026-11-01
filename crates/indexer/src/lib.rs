// SPDX-License-Identifier: AGPL-3.0-or-later
//! Native lessdugin indexer. Content is admitted and folded by `lessdugin-core`; this crate
//! owns only discovery metadata, change/cursor persistence, verified proof caching and HTTP.

pub mod api;
pub mod host;
pub mod ingest;
pub mod live;
pub mod proofs;
pub mod store;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::engine::Engine;
use url::Url;

use crate::host::PoliteHost;
use crate::store::Store;

pub const NS: &str = lessdugin_lexicon::ns::NS;
pub const INDEX_NS: &str = "online.7778777.lessdugin.index";
pub const FEATURES: [&str; 9] = [
    "describeIndexer",
    "listForums",
    "listParticipants",
    "getRepoHeads",
    "getChanges",
    "listPosts",
    "listRecentDiscussion",
    "getThread",
    "getRecordProofs",
];

pub type IndexerEngine = Engine<PoliteHost>;

#[derive(Clone)]
pub struct Indexer {
    pub engine: Arc<IndexerEngine>,
    pub store: Store,
    pub config: Arc<IndexerConfig>,
    pub metrics: Arc<Metrics>,
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone, Debug)]
pub struct IndexerConfig {
    pub did: String,
    pub source_url: String,
    pub plc: Url,
    pub relays: Vec<Url>,
    pub jetstream: Vec<Url>,
    pub poll: Option<Url>,
    pub poll_interval_ms: u64,
    pub blocklist: Arc<std::collections::BTreeSet<String>>,
}

#[derive(Default)]
pub struct Metrics {
    pub requests: AtomicU64,
    pub rate_limited: AtomicU64,
    pub synced_forums: AtomicU64,
    pub sync_errors: AtomicU64,
    pub live_events: AtomicU64,
    pub live_duplicates: AtomicU64,
    pub live_connections: AtomicU64,
    pub proofs_fetched: AtomicU64,
    pub proofs_failed: AtomicU64,
    pub changes_written: AtomicU64,
}

impl Metrics {
    pub fn render(&self, healthy: bool) -> String {
        let values = [
            ("lessdugin_indexer_up", u64::from(healthy)),
            (
                "lessdugin_indexer_http_requests_total",
                self.requests.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_rate_limited_total",
                self.rate_limited.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_forums_synced_total",
                self.synced_forums.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_sync_errors_total",
                self.sync_errors.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_live_events_total",
                self.live_events.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_live_duplicates_total",
                self.live_duplicates.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_live_connections",
                self.live_connections.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_proofs_fetched_total",
                self.proofs_fetched.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_proofs_failed_total",
                self.proofs_failed.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_changes_written_total",
                self.changes_written.load(Ordering::Relaxed),
            ),
        ];
        let mut out = String::new();
        for (name, value) in values {
            out.push_str(&format!("# TYPE {name} counter\n{name} {value}\n"));
        }
        out
    }
}

impl Indexer {
    pub fn new(engine: IndexerEngine, store: Store, config: IndexerConfig) -> Self {
        Self {
            engine: Arc::new(engine),
            store,
            config: Arc::new(config),
            metrics: Arc::new(Metrics::default()),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn blocked(&self, value: &str) -> bool {
        self.config.blocklist.contains(value)
            || lessdugin_atp::syntax::AtUri::parse(value)
                .ok()
                .is_some_and(|u| self.config.blocklist.contains(u.did().as_str()))
    }

    pub(crate) fn blocked_host(&self, url: &str) -> bool {
        Url::parse(url).ok().is_some_and(|u| {
            u.host_str().is_some_and(|host| {
                self.config.blocklist.contains(host)
                    || u.port_or_known_default().is_some_and(|port| {
                        self.config.blocklist.contains(&format!("{host}:{port}"))
                    })
            }) || self
                .config
                .blocklist
                .contains(u.origin().ascii_serialization().as_str())
        })
    }

    pub(crate) fn blocked_record(&self, forum: &str, uri: &str) -> bool {
        self.blocked(uri)
            || self
                .store
                .record_cid(forum, uri)
                .is_some_and(|cid| self.blocked(&cid))
            || self.store.forum(uri).is_some_and(|f| self.blocked(&f.cid))
            || AtUri::parse(uri).ok().is_some_and(|u| {
                self.store
                    .record_pds(u.did().as_str())
                    .is_some_and(|pds| self.blocked_host(&pds))
            })
    }

    fn blocked_change(&self, c: &crate::store::ChangeRow) -> bool {
        self.blocked(&c.uri)
            || c.cid.as_deref().is_some_and(|cid| self.blocked(cid))
            || self
                .store
                .record_pds(&c.did)
                .is_some_and(|pds| self.blocked_host(&pds))
    }

    pub fn now_ms(&self) -> i64 {
        lessdugin_core::host::Host::now_ms(self.engine.host())
    }

    pub fn as_of(&self) -> serde_json::Value {
        let seq = self.store.latest_seq();
        let snapshot_ms = self.store.snapshot_ms();
        let now_ms = self.now_ms();
        let time_us = if snapshot_ms > 0 { snapshot_ms } else { now_ms }.saturating_mul(1000);
        serde_json::json!({"seq": seq, "timeUs": time_us, "lagMs": (now_ms-snapshot_ms).max(0)})
    }

    pub fn info(&self) -> serde_json::Value {
        serde_json::json!({"did": self.config.did, "coreVersion": env!("CARGO_PKG_VERSION")})
    }

    pub fn with_meta(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let Some(map) = value.as_object_mut() {
            map.insert("asOf".into(), self.as_of());
            map.insert("indexer".into(), self.info());
        }
        value
    }
}
