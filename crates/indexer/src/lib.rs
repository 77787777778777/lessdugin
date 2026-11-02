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
use lessdugin_core::model::ForumModel;
use lessdugin_native::config::NetworkProfile;
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

/// True in builds with devnet mode: debug builds, and release builds with the `devnet` feature.
/// Other builds refuse `--dev` and devnet profiles (`threat-model.md` INFRA-9).
pub const DEVNET_AVAILABLE: bool = cfg!(any(debug_assertions, feature = "devnet"));

/// The network profile the service runs with. Devnet mode is never implicit: `--poll` only
/// selects the polling source, and keeps the configured profile. `dev` (with `--poll` naming
/// the devnet PDS) or a devnet `config.toml` profile selects devnet mode, and only where
/// `devnet_available`. Outside devnet mode a `--poll` URL must be https.
pub fn network_profile(
    configured: NetworkProfile,
    plc: &Url,
    poll: Option<&Url>,
    dev: bool,
    devnet_available: bool,
) -> anyhow::Result<NetworkProfile> {
    let network = if dev {
        let pds = poll
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("--dev needs --poll <devnet PDS URL>"))?;
        let plc = match &configured {
            NetworkProfile::Devnet(d) => d.plc.clone(),
            NetworkProfile::Production => plc.clone(),
        };
        NetworkProfile::Devnet(lessdugin_native::config::DevnetProfile {
            plc,
            pds,
            jetstream: None,
        })
    } else {
        configured
    };
    match &network {
        NetworkProfile::Devnet(_) if !devnet_available => Err(anyhow::anyhow!(
            "devnet mode (--dev, or a devnet [network] profile) is not available in this build; \
             it is for development builds only"
        )),
        NetworkProfile::Production if poll.is_some_and(|u| u.scheme() != "https") => Err(
            anyhow::anyhow!("--poll needs an https PDS URL outside devnet mode"),
        ),
        _ => Ok(network),
    }
}

/// At most this many folded forum models are kept for the query methods.
pub const MODEL_CACHE_ENTRIES: usize = 32;
/// At most this many proofs are fetched for `getRecordProofs` at a time; other uncached items
/// are omitted (the client falls back to its PDS) instead of waiting on the ingest budget.
pub const API_PROOF_FETCHES: usize = 2;

#[derive(Clone)]
pub struct Indexer {
    pub engine: Arc<IndexerEngine>,
    pub store: Store,
    pub config: Arc<IndexerConfig>,
    pub metrics: Arc<Metrics>,
    /// Serialises ordinary (network) forum syncs.
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    /// Serialises projections: a model is built from the cache and applied under this lock, so
    /// an older build can never overwrite a newer projection.
    pub(crate) project_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) models: Arc<std::sync::Mutex<Models>>,
    pub(crate) api_proofs: Arc<tokio::sync::Semaphore>,
    pub(crate) rate_windows:
        Arc<std::sync::Mutex<std::collections::HashMap<String, (tokio::time::Instant, u8)>>>,
}

/// The last projected model of each forum (least recently used first out). Queries read views
/// from it instead of refolding the forum per request.
#[derive(Default)]
pub(crate) struct Models {
    entries: std::collections::HashMap<String, (u64, Arc<ForumModel>)>,
    tick: u64,
}

impl Models {
    pub(crate) fn get(&mut self, forum: &str) -> Option<Arc<ForumModel>> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(forum).map(|(used, m)| {
            *used = tick;
            Arc::clone(m)
        })
    }

    pub(crate) fn put(&mut self, forum: &str, model: Arc<ForumModel>) {
        self.tick += 1;
        self.entries.insert(forum.to_owned(), (self.tick, model));
        while self.entries.len() > MODEL_CACHE_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(f, _)| f.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    pub(crate) fn remove(&mut self, forum: &str) {
        self.entries.remove(forum);
    }
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
    pub live_failures: AtomicU64,
    pub live_oversized: AtomicU64,
    pub repo_retries: AtomicU64,
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
            (
                "lessdugin_indexer_live_failures_total",
                self.live_failures.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_live_oversized_total",
                self.live_oversized.load(Ordering::Relaxed),
            ),
            (
                "lessdugin_indexer_repo_retries_total",
                self.repo_retries.load(Ordering::Relaxed),
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
            project_lock: Arc::new(tokio::sync::Mutex::new(())),
            models: Arc::new(std::sync::Mutex::new(Models::default())),
            api_proofs: Arc::new(tokio::sync::Semaphore::new(API_PROOF_FETCHES)),
            rate_windows: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
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

    /// A DID, or its repository's PDS host, is blocklisted.
    pub(crate) fn blocked_did(&self, did: &str) -> bool {
        !self.config.blocklist.is_empty()
            && (self.blocked(did)
                || self
                    .store
                    .record_pds(did)
                    .is_some_and(|pds| self.blocked_host(&pds)))
    }

    /// The forum's URI, owner or declaration CID is blocklisted, or (`host`) its owner's PDS
    /// host. Listings apply the host too; forum-scoped queries still answer for a forum on a
    /// blocked host, with that host's records hidden.
    pub(crate) fn blocked_forum(&self, forum: &crate::store::ForumRow, host: bool) -> bool {
        !self.config.blocklist.is_empty()
            && (self.blocked(&forum.uri)
                || (!forum.cid.is_empty() && self.blocked(&forum.cid))
                || (host && self.blocked_did(&forum.did)))
    }

    /// A record of `forum` is hidden by its URI, its indexed CID, its forum's declaration CID,
    /// its author or its author's PDS host.
    pub(crate) fn blocked_record(&self, forum: &crate::store::ForumRow, uri: &str) -> bool {
        !self.config.blocklist.is_empty()
            && (self.blocked(uri)
                || self
                    .store
                    .record_cid(&forum.uri, uri)
                    .is_some_and(|cid| self.blocked(&cid))
                || (!forum.cid.is_empty() && self.blocked(&forum.cid))
                || AtUri::parse(uri)
                    .ok()
                    .is_some_and(|u| self.blocked_did(u.did().as_str())))
    }

    fn blocked_change(&self, c: &crate::store::ChangeRow) -> bool {
        !self.config.blocklist.is_empty()
            && (self.blocked(&c.uri)
                || c.cid.as_deref().is_some_and(|cid| self.blocked(cid))
                || self.blocked_did(&c.did))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_never_switches_a_production_profile_into_devnet() {
        let plc = Url::parse("https://plc.example.test").unwrap();
        let https = Url::parse("https://pds.example.test").unwrap();
        let plain = Url::parse("http://127.0.0.1:2583").unwrap();
        // --poll alone keeps the configured production profile (and its identity rules).
        let network =
            network_profile(NetworkProfile::Production, &plc, Some(&https), false, true).unwrap();
        assert_eq!(network, NetworkProfile::Production);
        // ...and a plaintext poll source is refused outside devnet mode.
        assert!(
            network_profile(NetworkProfile::Production, &plc, Some(&plain), false, true).is_err()
        );
        // Devnet mode needs the explicit flag, and a build that has devnet mode.
        let NetworkProfile::Devnet(d) =
            network_profile(NetworkProfile::Production, &plc, Some(&plain), true, true).unwrap()
        else {
            panic!("--dev selects devnet mode");
        };
        assert_eq!((d.plc, d.pds), (plc.clone(), plain.clone()));
        assert!(
            network_profile(NetworkProfile::Production, &plc, Some(&plain), true, false).is_err()
        );
        assert!(network_profile(NetworkProfile::Production, &plc, None, true, true).is_err());
        let devnet = NetworkProfile::Devnet(lessdugin_native::config::DevnetProfile {
            plc: plc.clone(),
            pds: plain.clone(),
            jetstream: None,
        });
        assert!(network_profile(devnet.clone(), &plc, None, false, false).is_err());
        assert_eq!(
            network_profile(devnet.clone(), &plc, Some(&plain), false, true).unwrap(),
            devnet
        );
    }
}
