// SPDX-License-Identifier: AGPL-3.0-or-later
//! NativeHost: reqwest with an SSRF guard, WebSockets, system clock and randomness.
//!
//! - [`Net`] is the guarded HTTP client ([`guard`] has the policy and the pinned resolver).
//! - `Target::Anon` requests go straight through [`Net`]; `Target::UserPds` requests are handed
//!   to the [`AuthSession`] registered for the account (login code registers it), which adds
//!   DPoP/bearer authentication and sends through the same [`Net`] without redirects.
//! - Blobs are verified while streaming and stored by CID ([`BlobStore`]).

pub mod blob;
pub mod guard;
pub mod net;
pub mod ws;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{
    BlobHandle, Host, HostError, HttpRequest, HttpResponse, KvStore, Redirects, Target,
};

pub use blob::BlobStore;
pub use guard::{DnsBackend, FixedDns, GuardedResolver, NetPolicy, SystemDns, is_blocked};
pub use net::{Net, NetRequest, USER_AGENT, redact_url, user_pds_url};
pub use ws::NativeWs;

use crate::config::{Config, Paths};
use crate::kv::{Durability, SqliteKv};

/// An authenticated session for one account, bound at login to one PDS
/// (`threat-model.md` AUTH-7, AUTH-8; `docs/design/synthesis-v1.md` §6.7).
///
/// Implementations (OAuth with DPoP, app-password sessions) receive `Target::UserPds`
/// requests for their account. They must:
/// - build the URL with [`NetRequest::for_user_pds`] from the PDS bound at login (never from
///   the request, and never following redirects);
/// - add `authorization`/`dpop` headers themselves, refresh tokens under an exclusive lock, and
///   retry once on `use_dpop_nonce`;
/// - send through [`Net::execute`] (the SSRF guard and byte caps apply);
/// - never log tokens, proofs or bodies (`NetRequest`'s `Debug` redacts them).
///
/// Methods return boxed futures so that sessions can be stored as trait objects.
pub trait AuthSession: Send + Sync + 'static {
    /// The account this session authenticates.
    fn account(&self) -> &Did;

    /// Sends one `Target::UserPds` request with authentication.
    fn send(&self, req: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HostError>>;
}

/// The native [`Host`]: SQLite stores (or any [`KvStore`]), the guarded HTTP client, verified
/// blob storage and per-account auth sessions. Cloning shares everything.
#[derive(Clone)]
pub struct NativeHost<K: KvStore = SqliteKv> {
    cache: K,
    state: K,
    net: Net,
    blobs: BlobStore,
    sessions: Arc<RwLock<HashMap<Did, Arc<dyn AuthSession>>>>,
}

impl<K: KvStore> std::fmt::Debug for NativeHost<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeHost")
            .field("net", &self.net)
            .field("blobs", &self.blobs.dir())
            .finish_non_exhaustive()
    }
}

impl NativeHost<SqliteKv> {
    /// Opens the host for `paths` and `config`: creates the directories (0700), opens
    /// `cache.sqlite` (`synchronous=NORMAL`) and `state.sqlite` (`FULL`), and builds the HTTP
    /// client with the configuration's network policy (devnet rules only for a devnet profile).
    pub fn open(paths: &Paths, config: &Config) -> Result<Self, HostError> {
        paths
            .ensure_dirs()
            .map_err(|e| HostError::Storage(e.to_string()))?;
        let cache = SqliteKv::open(&paths.cache_db(), Durability::Normal)?;
        let state = SqliteKv::open(&paths.state_db(), Durability::Full)?;
        let net = Net::new(config.net_policy())?;
        Ok(Self::with_parts(cache, state, paths.blobs_dir(), net))
    }
}

impl<K: KvStore> NativeHost<K> {
    /// A host from its parts (tests, ephemeral sessions with `MemKv`).
    pub fn with_parts(cache: K, state: K, blob_dir: impl Into<PathBuf>, net: Net) -> Self {
        Self {
            cache,
            state,
            net,
            blobs: BlobStore::new(blob_dir),
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// The guarded HTTP client (auth sessions send through it).
    pub fn net(&self) -> &Net {
        &self.net
    }

    /// Verified blob storage (`blob_open`, `blob_read`, `blob_delete`).
    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Registers (or replaces) the session used for `UserPds` requests of its account.
    pub fn set_session(&self, session: Arc<dyn AuthSession>) {
        let did = session.account().clone();
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(did, session);
    }

    /// Removes an account's session (logout, `needs-relogin`).
    pub fn remove_session(&self, account: &Did) -> Option<Arc<dyn AuthSession>> {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(account)
    }

    /// The session of `account`, if one is registered.
    pub fn session(&self, account: &Did) -> Option<Arc<dyn AuthSession>> {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(account)
            .cloned()
    }

    /// Runs a long-lived engine future (live tail, outbox pump…) on the Tokio runtime.
    pub fn spawn<F>(&self, fut: F) -> tokio::task::JoinHandle<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(fut)
    }
}

impl<K: KvStore + 'static> Host for NativeHost<K> {
    type Kv = K;
    type Ws = NativeWs;

    fn cache(&self) -> &K {
        &self.cache
    }

    fn state(&self) -> &K {
        &self.state
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        match &req.target {
            Target::Anon(_) => self.net.execute(NetRequest::from_anon(req)?).await,
            Target::UserPds { account, .. } => {
                let session = self
                    .session(account)
                    .ok_or_else(|| HostError::AuthRequired(account.as_str().to_owned()))?;
                let req = req.with_redirects(Redirects::None);
                session.send(req).await
            }
        }
    }

    async fn fetch_blob(
        &self,
        pds: url::Url,
        did: Did,
        cid: String,
        max_bytes: u64,
    ) -> Result<BlobHandle, HostError> {
        self.blobs
            .fetch(&self.net, &pds, &did, &cid, max_bytes)
            .await
    }

    async fn websocket(
        &self,
        url: url::Url,
        max_message_bytes: u32,
    ) -> Result<NativeWs, HostError> {
        ws::connect(&self.net, url, max_message_bytes).await
    }

    async fn resolve_txt_system(&self, _name: String) -> Result<Option<Vec<String>>, HostError> {
        // No system TXT resolver is in the dependency tree; DNS over HTTPS is the default.
        Err(HostError::Unsupported(
            "system DNS TXT lookups are not available".into(),
        ))
    }

    async fn sleep(&self, ms: u64) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }

    fn now_ms(&self) -> i64 {
        // The native host is the clock of the engine; the ban on clocks applies to the
        // wasm-safe crates only.
        #[allow(clippy::disallowed_methods)]
        let now = std::time::SystemTime::now();
        now.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn fill_random(&self, buf: &mut [u8]) {
        // Without OS randomness nothing secure can be done; fail loudly.
        getrandom::fill(buf).expect("the operating system random number generator failed");
    }
}
