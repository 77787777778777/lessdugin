// SPDX-License-Identifier: AGPL-3.0-or-later
//! `Engine<H>`: the public API used by every client (`engine-api.md`).
//!
//! M1 provides identity (`resolve_actor`), verification (`verify`) and the local account list
//! (`accounts`, `add_account` with the login binding, `remove_account`, `set_active_account`,
//! `active_account`). M2 adds forums and membership (`forums`), sync (`syncing`), the queries
//! over the forum model (`queries`), `parse_ref` (`refs`), writes through the outbox
//! (`writes`), the long-lived `live_tail` and `background_verify` (`live`) and events
//! (`events`). The engine keeps no global state: two engines over two hosts are independent.
//! Cloning an engine shares its host, outbox, event subscribers and model cache.

mod accounts;
mod config;
mod error;
mod events;
mod forums;
mod live;
mod models;
mod queries;
mod refs;
mod syncing;
mod writes;

use std::sync::Arc;

use lessdugin_atp::syntax::{AtIdentifier, Did, LooseAtUri};

pub use accounts::{AccountInfo, LoginBinding, PinStatus, is_app_password_shape};
pub use config::{
    EngineConfig, IndexerConsent, Network, Platform, Sources, VerifyMode, default_user_agent,
};
pub use error::{EngineError, ErrorCode};
pub use events::{EventCallback, SubscriptionId};
pub use live::LiveRound;
pub use syncing::ForumSyncOpts;

use crate::dto::{ActorView, VerifyReport};
use crate::host::Host;
use crate::identity::{ResolvedActor, Resolver};
use crate::outbox::Outbox;
use crate::sync::{SyncConfig, SyncShared, Syncer};
use crate::xrpc::Xrpc;

/// The engine.
pub struct Engine<H: Host> {
    host: Arc<H>,
    config: Arc<EngineConfig>,
    resolver: Resolver<H>,
    /// Sync state shared by every clone (scheduler, dirty queue).
    sync: Arc<SyncShared>,
    /// Event bus, forum models, render cache (shared by every clone).
    rt: Arc<models::Runtime>,
    /// The durable outbox, reporting to the event bus.
    outbox: Outbox<H>,
}

impl<H: Host> Clone for Engine<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            config: Arc::clone(&self.config),
            resolver: self.resolver.clone(),
            sync: Arc::clone(&self.sync),
            rt: Arc::clone(&self.rt),
            outbox: self.outbox.clone(),
        }
    }
}

impl<H: Host> Engine<H> {
    /// An engine over `host`.
    pub fn new(host: H, config: EngineConfig) -> Self {
        Self::with_shared_host(Arc::new(host), config)
    }

    /// An engine over a shared host.
    // In wasm the runtime is not `Send` (callbacks are `MaybeSendSync`); the core is
    // single-threaded there.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn with_shared_host(host: Arc<H>, config: EngineConfig) -> Self {
        let resolver = Resolver::new(Arc::clone(&host), config.identity_config());
        let sync = Arc::new(SyncShared::new(SyncConfig::from_engine(&config)));
        let rt = Arc::new(models::Runtime::new());
        let listener = Arc::new(events::EngineListener {
            rt: Arc::clone(&rt),
        });
        let outbox = Outbox::with_listener(Arc::clone(&host), listener)
            .with_syncer(Syncer::with_shared(Arc::clone(&host), Arc::clone(&sync)));
        Self {
            host,
            config: Arc::new(config),
            resolver,
            sync,
            rt,
            outbox,
        }
    }

    /// The host.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// The shared host.
    pub fn shared_host(&self) -> Arc<H> {
        Arc::clone(&self.host)
    }

    /// The configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// The identity resolver.
    pub fn resolver(&self) -> &Resolver<H> {
        &self.resolver
    }

    /// An XRPC client over the host.
    pub fn xrpc(&self) -> Xrpc<'_, H> {
        Xrpc::new(&self.host)
    }

    /// The sync entry point (`sync.md` §4–§6, §9), sharing this engine's scheduler and queue.
    pub fn syncer(&self) -> Syncer<H> {
        Syncer::with_shared(Arc::clone(&self.host), Arc::clone(&self.sync))
    }

    /// The outbox (shared by every clone; its listener feeds the event bus).
    pub fn outbox(&self) -> &Outbox<H> {
        &self.outbox
    }

    /// `subscribe(cb)` (`engine-api.md` §3.8): `cb` receives every event, in order and never
    /// re-entrantly, on the thread (task) that emitted it or another emitter's.
    // In wasm callbacks are not `Send`; the core is single-threaded there.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn subscribe(&self, cb: impl EventCallback + 'static) -> SubscriptionId {
        self.rt.bus.subscribe(Arc::new(cb))
    }

    /// [`Engine::subscribe`] with a boxed callback (bindings).
    pub fn subscribe_boxed(&self, cb: Box<dyn EventCallback>) -> SubscriptionId {
        self.rt.bus.subscribe(Arc::from(cb))
    }

    /// Removes a subscription; `false` if it was not subscribed.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        self.rt.bus.unsubscribe(id)
    }

    /// Resolves a handle, DID, `@handle` or `at://…` actor to an [`ActorView`] (the handle is
    /// present only if verified in both directions).
    pub async fn resolve_actor(&self, input: &str) -> Result<ActorView, EngineError> {
        Ok(self.resolve_identity(input).await?.identity.actor_view())
    }

    /// Like [`Engine::resolve_actor`], with the full identity (PDS, key, resolution method,
    /// `dns-unauthenticated` flag) for `lessdugin resolve`, `doctor` and login code.
    pub async fn resolve_identity(&self, input: &str) -> Result<ResolvedActor, EngineError> {
        Ok(self.resolver.resolve_actor(input).await?)
    }

    /// Like [`Engine::resolve_identity`], bypassing the identity cache: the handle is looked up
    /// now and the DID document is fetched now. Login code uses it before any credential leaves
    /// the machine (`sync.md` §3.6).
    pub async fn resolve_identity_fresh(&self, input: &str) -> Result<ResolvedActor, EngineError> {
        Ok(self.resolver.resolve_actor_fresh(input).await?)
    }

    /// Drops the cached identity of `did` (and its claimed handle).
    pub async fn invalidate_identity(&self, did: &str) -> Result<(), EngineError> {
        let did = Did::parse(did).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
        self.resolver.invalidate_did(&did).await;
        Ok(())
    }

    /// `verify(uri, deep)` (`sync.md` §8.8). Accepts `at://<did|handle>/<collection>/<rkey>`
    /// (with the lenient user-input forms); a handle authority is resolved first.
    pub async fn verify(&self, uri: &str, deep: bool) -> Result<VerifyReport, EngineError> {
        let loose = LooseAtUri::parse_user_input(uri)
            .map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
        let loose = match loose.authority() {
            AtIdentifier::Did(_) => loose,
            AtIdentifier::Handle(h) => {
                let h = self
                    .resolver
                    .check_handle(h.as_str())
                    .map_err(|e| EngineError::invalid_ref(format!("{h}: {e}")))?;
                let (did, _) = self.resolver.resolve_handle_did(&h).await?;
                loose.with_did(did)
            }
        };
        let strict = loose.to_strict().ok_or_else(|| {
            EngineError::invalid_ref(format!(
                "{uri:?}: verify needs a record URI at://<did>/<collection>/<rkey>"
            ))
        })?;
        crate::verify::verify_record(&self.resolver, &strict, deep).await
    }
}
