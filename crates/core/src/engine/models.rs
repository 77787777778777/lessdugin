// SPDX-License-Identifier: AGPL-3.0-or-later
//! The engine's runtime state shared by clones: the event bus, the cache of forum models (with
//! the forum's completeness), the `fk → F` registry and the render cache; and the helpers the
//! queries share (loading a model, actor views, record values, pending own uris).
//!
//! A model is rebuilt when a `Changed` event names its forum (the listener drops it first), when
//! it is older than [`limits::MODEL_MAX_AGE_MS`], or when it is evicted (at most
//! [`limits::MODEL_CACHE_ENTRIES`], least recently built first). A build that raced an
//! invalidation is used for its own query but not cached.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures::future::join_all;
use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did};
use serde_json::Value;

use crate::discovery::forum_ref::{ForumKey, parse_forum_ref};
use crate::discovery::state::{self, ForumState};
use crate::discovery::{DiscoveryError, OpenedForum};
use crate::dto::{ActorView, Completeness};
use crate::host::Host;
use crate::limits;
use crate::model::render::RenderCache;
use crate::model::views::bare_actor;
use crate::model::{ForumModel, ModelError};
use crate::social::input::SkelData;
use crate::store::records::{RecordStore, StoreError};
use crate::store::schema::RecKey;
use crate::sync::{Priority, Stats};

use super::events::EventBus;
use super::{Engine, EngineError};

impl From<ModelError> for EngineError {
    fn from(e: ModelError) -> Self {
        match e {
            ModelError::Store(e) => store_error(e),
            ModelError::NoDeclaration(uri) => {
                EngineError::not_found(format!("forum {uri} is not in the cache; open it first"))
            }
            ModelError::InvalidDeclaration { uri, rule } => {
                EngineError::invalid_input(format!("forum {uri} is invalid ({rule})"))
            }
            ModelError::Fold(e) => EngineError::internal(format!("fold: {e}")),
        }
    }
}

/// A store failure.
pub(crate) fn store_error(e: StoreError) -> EngineError {
    EngineError::internal(format!("record store: {e}")).with_retryable(true)
}

/// A loaded forum: the model and the forum's completeness at build time.
#[derive(Clone, Debug)]
pub(crate) struct Loaded {
    pub(crate) model: Arc<ForumModel>,
    pub(crate) completeness: Completeness,
    built_ms: i64,
}

#[derive(Default)]
struct Models {
    loaded: BTreeMap<String, Loaded>,
    /// Invalidations per forum, and of everything.
    gens: BTreeMap<String, u64>,
    all_gen: u64,
    /// `fk → F`.
    fks: BTreeMap<String, String>,
}

impl Models {
    fn gen_of(&self, f: &str) -> (u64, u64) {
        (self.gens.get(f).copied().unwrap_or(0), self.all_gen)
    }
}

/// State shared by every clone of an engine.
pub(crate) struct Runtime {
    pub(crate) bus: EventBus,
    models: Mutex<Models>,
    pub(crate) renders: RenderCache,
    /// Accounts whose own memberships were read into the followed list (`sync.md` §4.8).
    pub(crate) seeded: Mutex<BTreeSet<String>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            bus: EventBus::default(),
            models: Mutex::new(Models::default()),
            renders: RenderCache::default(),
            seeded: Mutex::new(BTreeSet::new()),
        }
    }

    /// Records `fk(F) → F`.
    pub(crate) fn register(&self, key: &ForumKey) {
        lock(&self.models)
            .fks
            .insert(key.fk.clone(), key.uri.to_string());
    }

    /// Records `fk(F) → F` for a forum uri string (ignored unless canonical).
    pub(crate) fn register_uri(&self, forum: &str) {
        if let Ok(u) = AtUri::parse(forum)
            && let Ok(k) = ForumKey::of_uri(&u)
        {
            self.register(&k);
        }
    }

    /// F of a registered fk.
    pub(crate) fn forum_of_fk(&self, fk: &str) -> Option<String> {
        lock(&self.models).fks.get(fk).cloned()
    }

    /// The thread root of a comment held by a cached model.
    pub(crate) fn thread_of(&self, comment: &str) -> Option<String> {
        let m = lock(&self.models);
        m.loaded
            .values()
            .find_map(|l| match &l.model.skel(comment)?.data {
                SkelData::Comment { root: Some(r), .. } => Some(r.uri.clone()),
                _ => None,
            })
    }

    /// Drops the model of forum `f`.
    pub(crate) fn invalidate(&self, f: &str) {
        let mut m = lock(&self.models);
        m.loaded.remove(f);
        *m.gens.entry(f.to_owned()).or_insert(0) += 1;
    }

    /// Drops every model.
    pub(crate) fn invalidate_all(&self) {
        let mut m = lock(&self.models);
        m.loaded.clear();
        m.all_gen += 1;
    }

    fn cached(&self, f: &str, now_ms: i64) -> Option<Loaded> {
        let m = lock(&self.models);
        let l = m.loaded.get(f)?;
        (now_ms.saturating_sub(l.built_ms) <= limits::MODEL_MAX_AGE_MS && l.built_ms <= now_ms)
            .then(|| l.clone())
    }

    fn gen_of(&self, f: &str) -> (u64, u64) {
        lock(&self.models).gen_of(f)
    }

    fn put(&self, f: &str, l: Loaded, gen_before: (u64, u64)) {
        let mut m = lock(&self.models);
        if m.gen_of(f) != gen_before {
            return;
        }
        m.loaded.insert(f.to_owned(), l);
        while m.loaded.len() > limits::MODEL_CACHE_ENTRIES {
            let oldest = m
                .loaded
                .iter()
                .min_by_key(|(k, l)| (l.built_ms, (*k).clone()))
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    m.loaded.remove(&k);
                }
                None => break,
            }
        }
    }
}

impl<H: Host> Engine<H> {
    /// A fresh P0 sync session with its own counters (`max_requests`, `budget_ms`).
    pub(crate) fn p0_session(
        &self,
        max_requests: Option<u32>,
        budget_ms: Option<i64>,
    ) -> crate::sync::Session<H> {
        let deadline = budget_ms.map(|b| self.host.now_ms().saturating_add(b));
        let stats = Arc::new(Stats::capped(max_requests).with_deadline(deadline));
        self.syncer().session(Priority::P0, stats)
    }

    /// The canonical key of a forum reference (`sync.md` §4.1): an at-uri, `owner/slug` or a
    /// SPA URL; a handle owner is resolved.
    pub(crate) async fn forum_key(&self, input: &str) -> Result<ForumKey, EngineError> {
        let r = parse_forum_ref(input).map_err(DiscoveryError::from)?;
        let key = match &r.owner {
            AtIdentifier::Did(d) => ForumKey::new(d, &r.slug).map_err(DiscoveryError::from)?,
            AtIdentifier::Handle(_) => {
                self.syncer()
                    .resolve_forum_ref(&self.p0_session(None, None), input)
                    .await?
            }
        };
        self.rt.register(&key);
        Ok(key)
    }

    /// `open_forum` (`sync.md` §4.2) at P0.
    pub(crate) async fn open_key(&self, key: &ForumKey) -> Result<OpenedForum, EngineError> {
        let opened = self
            .syncer()
            .open_forum(&self.p0_session(None, None), key)
            .await?;
        self.rt.register(key);
        self.rt.invalidate(key.uri.as_str());
        Ok(opened)
    }

    /// The model of forum `key`, from memory or built from the cache; a forum whose declaration
    /// is not cached yet is opened first.
    pub(crate) async fn loaded(&self, key: &ForumKey) -> Result<Loaded, EngineError> {
        let f = key.uri.as_str();
        let now_ms = self.host.now_ms();
        if let Some(l) = self.rt.cached(f, now_ms) {
            return Ok(l);
        }
        let gen_before = self.rt.gen_of(f);
        let kv = self.host.cache();
        let now_us = now_ms.saturating_mul(1000);
        let model = match ForumModel::build(kv, key, now_us).await {
            Err(ModelError::NoDeclaration(_)) => {
                self.open_key(key).await?;
                ForumModel::build(kv, key, now_us).await?
            }
            r => r?,
        };
        let st = ForumState::load(kv, key, &model.fold.decl, now_us)
            .await
            .map_err(store_error)?;
        let (completeness, _) = state::completeness(kv, key, &st)
            .await
            .map_err(store_error)?;
        let l = Loaded {
            model: Arc::new(model),
            completeness,
            built_ms: now_ms,
        };
        self.rt.register(key);
        self.rt.put(f, l.clone(), gen_before);
        Ok(l)
    }

    /// Actor views and PDS origins of `dids` from the identity cache (no network: a DID
    /// without a cached document gets a bare view).
    pub(crate) async fn actors<'a>(
        &self,
        dids: impl IntoIterator<Item = &'a str>,
    ) -> (BTreeMap<String, ActorView>, BTreeMap<String, String>) {
        let dids: BTreeSet<&str> = dids.into_iter().collect();
        let parsed: Vec<Did> = dids.iter().filter_map(|d| Did::parse(d).ok()).collect();
        let found = join_all(parsed.iter().map(|d| self.resolver.cached(d))).await;
        let mut actors = BTreeMap::new();
        let mut pds = BTreeMap::new();
        for (d, id) in parsed.iter().zip(found) {
            let view = id
                .as_ref()
                .map_or_else(|| bare_actor(d.as_str()), |i| i.actor_view());
            if let Some(p) = id.and_then(|i| i.pds) {
                pds.insert(d.to_string(), p);
            }
            actors.insert(d.to_string(), view);
        }
        (actors, pds)
    }

    /// The stored values of `uris` whose current CID is the model's (bodies are re-read on
    /// demand; the model keeps skeletons only).
    pub(crate) async fn values(
        &self,
        model: &ForumModel,
        uris: &[String],
    ) -> Result<BTreeMap<String, Value>, EngineError> {
        let mut keys = Vec::new();
        let mut want = Vec::new();
        for u in uris {
            if let (Ok(a), Some(x)) = (AtUri::parse(u), model.skel(u)) {
                keys.push(RecKey::of_uri(&a));
                want.push((u, x.cid.as_str()));
            }
        }
        let rows = RecordStore::new(self.host.cache())
            .get_many(&keys)
            .await
            .map_err(store_error)?;
        let mut out = BTreeMap::new();
        for ((u, cid), row) in want.into_iter().zip(rows) {
            if let Some(r) = row
                && r.cid.to_string() == cid
                && let Ok(v) = r.value_json()
            {
                out.insert(u.clone(), v);
            }
        }
        Ok(out)
    }

    /// Own uris (of the active account) with a write in the outbox that is not finished (the
    /// `pending` badge).
    pub(crate) async fn pending_uris(&self) -> Result<BTreeSet<String>, EngineError> {
        let Some(me) = self.active_account().await? else {
            return Ok(BTreeSet::new());
        };
        Ok(self
            .outbox
            .pending(&me.did)
            .await?
            .iter()
            .flat_map(|w| w.ops.iter().map(|o| o.uri(&w.account)))
            .collect())
    }
}
