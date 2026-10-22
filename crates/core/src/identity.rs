// SPDX-License-Identifier: AGPL-3.0-or-later
//! Identity resolution (`sync.md` §3): handles via DNS-over-HTTPS (or the system resolver) and
//! `/.well-known/atproto-did`, DIDs via the PLC directory or did:web, bidirectional handle
//! verification, caching with TTLs in the `cache` store, and devnet mode (§3.7).
//!
//! ```text
//! resolve_handle(h) -> Did(d, method) | NoAnswer | Invalid       (§3.1, cached 6 h)
//! resolve_did(d)    -> ResolvedIdentity { level doc|hint, pds, key, claimed handle, state }
//!                                                                 (§3.2, cached 24 h)
//! handle_state(d)   =  verified ⇔ the claimed handle resolves back to d (never via a hint)
//! ```
//!
//! Slingshot (`resolveMiniDoc`) is a hint-only fallback, used only in `assisted` mode: after a
//! `NoAnswer` for a handle, and after a CORS failure on a did:web document. Hint resolutions never
//! verify a handle and give identity level `hint`.
//!
//! Devnet mode is never implicit: it is on only when [`IdentityConfig::devnet_pds`] is set. Then
//! handles resolve only through that PDS's `com.atproto.identity.resolveHandle`, `.test` handles
//! are valid, and `http`/IP-literal PDS endpoints are accepted.
//!
//! Cache failures never fail a resolution (the cache is disposable, I1). A `NoAnswer` reached
//! after a transport failure is not cached, so a network blip does not hide a handle for 6 h. When
//! no resolver could be reached at all, resolving an actor fails with
//! [`IdentityError::HandleUnreachable`] (`network`), not "not found".
//!
//! Cached rows are tagged with the mode that produced them (`HandleRow.method = devnet`,
//! `IdentityRow.src = devnet`): rows from devnet mode are ignored outside it, and production rows
//! are ignored in devnet mode, so a devnet PDS or PLC can never answer for the public network
//! even if both modes share one cache. A Slingshot mini-doc (`hint`) is reused for at most
//! [`HINT_DOC_TTL_MS`], so the real document is retried.

pub mod doc;
pub mod doh;

use std::sync::Arc;

use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::syntax::{Did, DidMethod, Handle};
use lessdugin_atp::xrpc::{ResolveHandle, query_string};
use serde::{Deserialize, Serialize};

use crate::dto::{ActorView, HandleState};
use crate::host::{Host, KvOp, KvStore, Redirects};
use crate::limits;
use crate::store::row::{self, ROW_VERSION};
use crate::xrpc::{CallOpts, Service, Xrpc, XrpcCallError};

/// Cache duration of handle → DID answers (*tunable*).
pub const HANDLE_TTL_MS: i64 = 6 * 60 * 60 * 1000;
/// Cache duration of DID documents (*tunable*).
pub const DID_DOC_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// Third-party proofs need a DID document fetched at most this long ago (`sync.md` §1.1).
pub const THIRD_PARTY_DOC_MAX_AGE_MS: i64 = 60 * 60 * 1000;
/// A cached Slingshot mini-doc (identity level `hint`) is reused for at most this long, so the
/// real DID document is retried (*tunable*).
pub const HINT_DOC_TTL_MS: i64 = 60 * 60 * 1000;
/// Default DoH JSON endpoints, in order.
pub const DEFAULT_DOH: [&str; 2] = [
    "https://cloudflare-dns.com/dns-query",
    "https://dns.google/resolve",
];
/// Default PLC directory.
pub const DEFAULT_PLC: &str = "https://plc.directory";
/// Default Slingshot instance (hint-only).
pub const DEFAULT_SLINGSHOT: &str = "https://slingshot.microcosm.blue";

/// `network.dns` (`sync.md` §2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DnsMode {
    /// DNS over HTTPS (every platform; the default).
    #[default]
    Doh,
    /// The system resolver (native only); verdicts are flagged `dns-unauthenticated`.
    System,
}

/// Identity resolution settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityConfig {
    /// DoH JSON endpoints, tried in order.
    pub doh: Vec<url::Url>,
    /// DoH or the system resolver.
    pub dns: DnsMode,
    /// The PLC directory.
    pub plc: url::Url,
    /// Slingshot base URL (hint-only fallback; used only when `assisted`).
    pub slingshot: Option<url::Url>,
    /// `sources = assisted` (CORE+ allowed).
    pub assisted: bool,
    /// Devnet mode: the PDS whose `resolveHandle` resolves every handle (`sync.md` §3.7).
    pub devnet_pds: Option<url::Url>,
}

fn parse_const(s: &str) -> url::Url {
    url::Url::parse(s).expect("constant URL")
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            doh: DEFAULT_DOH.iter().map(|s| parse_const(s)).collect(),
            dns: DnsMode::Doh,
            plc: parse_const(DEFAULT_PLC),
            slingshot: Some(parse_const(DEFAULT_SLINGSHOT)),
            assisted: true,
            devnet_pds: None,
        }
    }
}

impl IdentityConfig {
    /// Devnet mode with this PLC directory and PDS (no DoH, no Slingshot).
    pub fn devnet(plc: url::Url, pds: url::Url) -> Self {
        Self {
            doh: Vec::new(),
            dns: DnsMode::Doh,
            plc,
            slingshot: None,
            assisted: false,
            devnet_pds: Some(pds),
        }
    }

    /// True in devnet mode.
    pub fn is_devnet(&self) -> bool {
        self.devnet_pds.is_some()
    }
}

/// How a handle was resolved (`HandleRow.method`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionMethod {
    /// DNS TXT over HTTPS.
    Doh,
    /// DNS TXT via the system resolver (`dns-unauthenticated`).
    System,
    /// `https://<handle>/.well-known/atproto-did`.
    Wellknown,
    /// Slingshot (never verifies a handle).
    Hint,
    /// The devnet PDS's `resolveHandle`.
    Devnet,
}

/// Identity level of a DID (`sync.md` §1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityLevel {
    /// A DID document from PLC or did:web that passed the checks.
    Doc,
    /// A Slingshot mini-doc.
    Hint,
}

/// Where a DID document came from (`IdentityRow.src`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocSource {
    /// The PLC directory.
    Plc,
    /// `https://<host>/.well-known/did.json`.
    Web,
    /// A Slingshot mini-doc (hint).
    Slingshot,
    /// Fetched in devnet mode (the devnet PLC directory, or a did:web document): never used
    /// outside devnet mode.
    Devnet,
}

/// The outcome of an uncached handle lookup ([`Resolver::lookup_handle`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandleLookup {
    /// The answer.
    pub resolution: HandleResolution,
    /// The answer may be cached: no transport failure was involved, and it is not a hint.
    pub cacheable: bool,
    /// No resolver could be reached: every DNS provider (or the devnet PDS) failed with a
    /// transport error and `/.well-known/atproto-did` gave no definitive answer either. Only
    /// meaningful with [`HandleResolution::NoAnswer`].
    pub unreachable: bool,
}

impl HandleLookup {
    fn new(resolution: HandleResolution, cacheable: bool) -> Self {
        Self {
            resolution,
            cacheable,
            unreachable: false,
        }
    }
}

/// The answer to `resolve_handle` (`sync.md` §3.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HandleResolution {
    /// The handle names this DID.
    Did {
        /// The DID.
        did: Did,
        /// How it was found.
        method: ResolutionMethod,
    },
    /// No answer (no record, or every source failed).
    NoAnswer,
    /// Invalid handle syntax, a disallowed TLD, or DNS named several DIDs.
    Invalid,
}

/// Why a string is not a resolvable handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandleProblem {
    /// Not handle syntax.
    #[error("not a valid handle")]
    Syntax,
    /// A TLD that never resolves (`.alt`, `.arpa`, `.example`, `.internal`, `.invalid`, `.local`,
    /// `.localhost`, `.onion`).
    #[error("handles under this top-level domain never resolve")]
    DisallowedTld,
    /// `.test` outside devnet mode.
    #[error(".test handles are only valid in devnet mode")]
    TestTld,
}

/// Lowercases and checks a handle for resolution (`sync.md` §3.1).
pub fn check_handle(input: &str, devnet: bool) -> Result<Handle, HandleProblem> {
    let h = Handle::parse(input).map_err(|_| HandleProblem::Syntax)?;
    if h.has_disallowed_tld() || h.is_invalid_placeholder() {
        return Err(HandleProblem::DisallowedTld);
    }
    if h.is_test_tld() && !devnet {
        return Err(HandleProblem::TestTld);
    }
    Ok(h)
}

/// `short_did` (`lfm-1.md` §5.4): `did:plc:` + 8 characters + `…`; a did:web up to 40 characters
/// whole, otherwise its first 39 characters + `…`.
pub fn short_did(did: &str) -> String {
    if let Some(id) = did.strip_prefix("did:plc:") {
        format!("did:plc:{}…", id.chars().take(8).collect::<String>())
    } else if did.chars().count() <= 40 {
        did.to_owned()
    } else {
        format!("{}…", did.chars().take(39).collect::<String>())
    }
}

/// A handle-state verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandleVerdict {
    /// The state.
    pub state: HandleState,
    /// How the claimed handle resolved, if it did.
    pub method: Option<ResolutionMethod>,
    /// Verified only through the system resolver.
    pub dns_unauthenticated: bool,
}

impl HandleVerdict {
    fn of(state: HandleState) -> Self {
        Self {
            state,
            method: None,
            dns_unauthenticated: false,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A resolved DID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedIdentity {
    /// The DID.
    pub did: Did,
    /// `doc` or `hint`.
    pub level: IdentityLevel,
    /// Where the document came from.
    pub source: DocSource,
    /// The PDS endpoint (`scheme://host[:port]`), if the document has an acceptable one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pds: Option<String>,
    /// Why there is no PDS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pds_problem: Option<String>,
    /// The `#atproto` signing key (Multikey), if usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_key: Option<String>,
    /// Why there is no signing key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_problem: Option<String>,
    /// The claimed handle (lowercased; displayed only when verified).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_handle: Option<String>,
    /// The handle verification state.
    pub handle_state: HandleState,
    /// How the claimed handle resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle_method: Option<ResolutionMethod>,
    /// The handle was verified only through the system resolver.
    #[serde(skip_serializing_if = "is_false")]
    pub dns_unauthenticated: bool,
    /// When the document was fetched (host ms).
    pub doc_at_ms: i64,
    /// When the handle state was computed (host ms).
    pub handle_at_ms: i64,
}

impl ResolvedIdentity {
    /// The verified handle, if any.
    pub fn handle(&self) -> Option<&str> {
        match self.handle_state {
            HandleState::Verified => self.claimed_handle.as_deref(),
            _ => None,
        }
    }

    /// The signing key.
    pub fn public_key(&self) -> Option<PublicKey> {
        PublicKey::from_multikey(self.signing_key.as_deref()?).ok()
    }

    /// The PDS as a URL.
    pub fn pds_url(&self) -> Option<url::Url> {
        url::Url::parse(self.pds.as_deref()?).ok()
    }

    /// The actor DTO.
    pub fn actor_view(&self) -> ActorView {
        ActorView {
            did: self.did.to_string(),
            handle: self.handle().map(str::to_owned),
            handle_state: self.handle_state,
            display_name: None,
            avatar: None,
            short_did: short_did(self.did.as_str()),
        }
    }
}

/// A resolved actor input (handle or DID).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedActor {
    /// The input as given.
    pub input: String,
    /// The input handle (normalized), when the input was a handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_handle: Option<String>,
    /// How the input handle resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<HandleResolution>,
    /// The identity.
    pub identity: ResolvedIdentity,
}

/// Identity failures.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// Not a handle or a DID.
    #[error("not a handle or DID: {0}")]
    InvalidInput(String),
    /// A DID outside lessdugin's policy (`did:plc` and host-only `did:web`).
    #[error("unsupported DID {did}: {reason}")]
    UnsupportedDid {
        /// The DID string.
        did: String,
        /// Why.
        reason: String,
    },
    /// The handle is invalid (syntax, TLD, or several DIDs in DNS).
    #[error("handle {handle} is invalid: {reason}")]
    InvalidHandle {
        /// The handle.
        handle: String,
        /// Why.
        reason: String,
    },
    /// The handle did not resolve.
    #[error("handle {0} did not resolve to a DID")]
    HandleNotFound(String),
    /// No handle resolver (DNS over HTTPS, the system resolver or the devnet PDS, and
    /// `/.well-known/atproto-did`) could be reached.
    #[error("could not reach any resolver for handle {0}")]
    HandleUnreachable(String),
    /// The DID is not registered (PLC 404/410, did:web 404).
    #[error("DID {0} not found")]
    DidNotFound(String),
    /// The DID document failed the checks.
    #[error("DID document for {did} is invalid: {reason}")]
    InvalidDoc {
        /// The DID.
        did: String,
        /// Why.
        reason: String,
    },
    /// The DID document could not be fetched.
    #[error("could not fetch the DID document for {did}: {error}")]
    Fetch {
        /// The DID.
        did: String,
        /// The transport failure.
        error: XrpcCallError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HandleResult {
    Did,
    None,
    Invalid,
}

/// `("id", "handle", handle)` in `cache` (`sync.md` §10.5 `HandleRow`).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HandleRow {
    v: u32,
    result: HandleResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<ResolutionMethod>,
    at: i64,
}

impl HandleRow {
    /// A row for `res`. In devnet mode every row (also `none` and `invalid`) has method `devnet`,
    /// which is how rows of the two modes are told apart.
    fn new(res: &HandleResolution, at: i64, devnet: bool) -> Self {
        let mode = devnet.then_some(ResolutionMethod::Devnet);
        let (result, did, method) = match res {
            HandleResolution::Did { did, method } => {
                (HandleResult::Did, Some(did.to_string()), Some(*method))
            }
            HandleResolution::NoAnswer => (HandleResult::None, None, mode),
            HandleResolution::Invalid => (HandleResult::Invalid, None, mode),
        };
        Self {
            v: ROW_VERSION,
            result,
            did,
            method,
            at,
        }
    }

    /// True if the row was written in the current mode.
    fn matches_mode(&self, devnet: bool) -> bool {
        (self.method == Some(ResolutionMethod::Devnet)) == devnet
    }

    fn resolution(&self) -> Option<HandleResolution> {
        Some(match self.result {
            HandleResult::Did => HandleResolution::Did {
                did: Did::parse(self.did.as_deref()?).ok()?,
                method: self.method?,
            },
            HandleResult::None => HandleResolution::NoAnswer,
            HandleResult::Invalid => HandleResolution::Invalid,
        })
    }
}

/// `("id", "did", did)` in `cache` (`sync.md` §10.5 `IdentityRow`, plus the reasons and the
/// `dns-unauthenticated` flag).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityRow {
    v: u32,
    #[serde(with = "serde_bytes")]
    doc: Vec<u8>,
    level: IdentityLevel,
    src: DocSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pds: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pds_problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_problem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aka: Option<String>,
    hstate: HandleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hmethod: Option<ResolutionMethod>,
    #[serde(default, skip_serializing_if = "is_false")]
    dns_unauth: bool,
    h_at: i64,
    doc_at: i64,
}

impl IdentityRow {
    fn set_verdict(&mut self, v: HandleVerdict, at: i64) {
        self.hstate = v.state;
        self.hmethod = v.method;
        self.dns_unauth = v.dns_unauthenticated;
        self.h_at = at;
    }

    fn identity(&self, did: &Did) -> ResolvedIdentity {
        ResolvedIdentity {
            did: did.clone(),
            level: self.level,
            source: self.src,
            pds: self.pds.clone(),
            pds_problem: self.pds_problem.clone(),
            signing_key: self.key.clone(),
            key_problem: self.key_problem.clone(),
            claimed_handle: self.aka.clone(),
            handle_state: self.hstate,
            handle_method: self.hmethod,
            dns_unauthenticated: self.dns_unauth,
            doc_at_ms: self.doc_at,
            handle_at_ms: self.h_at,
        }
    }
}

/// `true` if a value stamped `at` is younger than `ttl` at `now` (a future stamp is stale).
fn fresh(at: i64, now: i64, ttl: i64) -> bool {
    let age = now.saturating_sub(at);
    age >= 0 && age < ttl
}

fn did_key(did: &Did) -> Vec<u8> {
    crate::key!("id", "did", did.as_str())
}

fn handle_key(handle: &str) -> Vec<u8> {
    crate::key!("id", "handle", handle)
}

fn policy_did(s: &str) -> Option<Did> {
    Did::parse(s)
        .ok()
        .filter(|d| d.check_lessdugin_policy().is_ok())
}

/// The identity resolver. Cloning shares the host.
pub struct Resolver<H: Host> {
    host: Arc<H>,
    config: IdentityConfig,
}

impl<H: Host> Clone for Resolver<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            config: self.config.clone(),
        }
    }
}

impl<H: Host> Resolver<H> {
    /// A resolver over `host`.
    pub fn new(host: Arc<H>, config: IdentityConfig) -> Self {
        Self { host, config }
    }

    /// The host.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// The settings.
    pub fn config(&self) -> &IdentityConfig {
        &self.config
    }

    fn xrpc(&self) -> Xrpc<'_, H> {
        Xrpc::new(&self.host)
    }

    fn devnet(&self) -> bool {
        self.config.is_devnet()
    }

    fn slingshot(&self) -> Option<&url::Url> {
        if self.config.assisted && !self.devnet() {
            self.config.slingshot.as_ref()
        } else {
            None
        }
    }

    /// Lowercases and checks a handle (`.test` is valid only in devnet mode).
    pub fn check_handle(&self, input: &str) -> Result<Handle, HandleProblem> {
        check_handle(input, self.devnet())
    }

    // -----------------------------------------------------------------------------------------
    // Cache

    async fn read_row<T: serde::de::DeserializeOwned>(&self, key: &[u8]) -> Option<T> {
        match self.host.cache().get_many(vec![key.to_vec()]).await {
            Ok(mut v) => row::decode(&v.pop().flatten()?).ok(),
            Err(e) => {
                tracing::debug!(error = %e, "identity cache read failed");
                None
            }
        }
    }

    async fn write_row<T: Serialize>(&self, key: Vec<u8>, value: &T) {
        let Ok(bytes) = row::encode(value) else {
            return;
        };
        if let Err(e) = self.host.cache().write(vec![KvOp::Put(key, bytes)]).await {
            tracing::debug!(error = %e, "identity cache write failed");
        }
    }

    /// Drops the cached document of `did` and the cached answer for its claimed handle
    /// (Jetstream `identity` events, proof signature failures, `RepoNotFound`… — `sync.md` §3.3).
    pub async fn invalidate_did(&self, did: &Did) {
        let mut ops = vec![KvOp::Delete(did_key(did))];
        if let Some(h) = self
            .read_row::<IdentityRow>(&did_key(did))
            .await
            .and_then(|r| r.aka)
        {
            ops.push(KvOp::Delete(handle_key(&h)));
        }
        if let Err(e) = self.host.cache().write(ops).await {
            tracing::debug!(error = %e, "identity cache invalidation failed");
        }
    }

    /// Drops the cached answer for `handle`.
    pub async fn invalidate_handle(&self, handle: &Handle) {
        if let Err(e) = self
            .host
            .cache()
            .write(vec![KvOp::Delete(handle_key(handle.as_str()))])
            .await
        {
            tracing::debug!(error = %e, "identity cache invalidation failed");
        }
    }

    // -----------------------------------------------------------------------------------------
    // Handles

    /// `resolve_handle` (`sync.md` §3.1), cached for [`HANDLE_TTL_MS`].
    pub async fn resolve_handle(&self, handle: &Handle) -> HandleResolution {
        self.resolve_handle_outcome(handle, true).await.resolution
    }

    /// Resolves a handle to a DID (cached): [`IdentityError::HandleNotFound`] for no answer,
    /// [`IdentityError::HandleUnreachable`] when no resolver could be reached, and
    /// [`IdentityError::InvalidHandle`] for invalid handles and several DIDs in DNS.
    pub async fn resolve_handle_did(
        &self,
        handle: &Handle,
    ) -> Result<(Did, ResolutionMethod), IdentityError> {
        self.handle_did(handle, true).await
    }

    async fn handle_did(
        &self,
        handle: &Handle,
        use_cache: bool,
    ) -> Result<(Did, ResolutionMethod), IdentityError> {
        let out = self.resolve_handle_outcome(handle, use_cache).await;
        match out.resolution {
            HandleResolution::Did { did, method } => Ok((did, method)),
            HandleResolution::NoAnswer if out.unreachable => {
                Err(IdentityError::HandleUnreachable(handle.to_string()))
            }
            HandleResolution::NoAnswer => Err(IdentityError::HandleNotFound(handle.to_string())),
            HandleResolution::Invalid => Err(IdentityError::InvalidHandle {
                handle: handle.to_string(),
                reason: match self.check_handle(handle.as_str()) {
                    Err(p) => p.to_string(),
                    Ok(_) => "DNS names more than one DID".to_owned(),
                },
            }),
        }
    }

    /// `resolve_handle` with the cache read only when `use_cache` (the answer is cached either
    /// way when it may be).
    async fn resolve_handle_outcome(&self, handle: &Handle, use_cache: bool) -> HandleLookup {
        if self.check_handle(handle.as_str()).is_err() {
            return HandleLookup::new(HandleResolution::Invalid, false);
        }
        let key = handle_key(handle.as_str());
        let now = self.host.now_ms();
        if use_cache {
            let devnet = self.devnet();
            let cached = self.read_row::<HandleRow>(&key).await.filter(|r| {
                r.v == ROW_VERSION && fresh(r.at, now, HANDLE_TTL_MS) && r.matches_mode(devnet)
            });
            if let Some(res) = cached.and_then(|r| r.resolution()) {
                return HandleLookup::new(res, true);
            }
        }
        let out = self.lookup_handle(handle).await;
        if out.cacheable {
            self.write_row(key, &HandleRow::new(&out.resolution, now, self.devnet()))
                .await;
        }
        out
    }

    /// `resolve_handle` without the cache.
    pub async fn lookup_handle(&self, handle: &Handle) -> HandleLookup {
        if self.check_handle(handle.as_str()).is_err() {
            return HandleLookup::new(HandleResolution::Invalid, true);
        }
        if let Some(pds) = &self.config.devnet_pds {
            return self.lookup_devnet(pds, handle).await;
        }
        let qname = format!("_atproto.{}", handle.as_str());
        let mut transport_failure = false;
        let answer = match self.config.dns {
            DnsMode::Doh => {
                let mut answer = None;
                for endpoint in &self.config.doh {
                    match self.doh_txt(endpoint, &qname).await {
                        Some(txts) => {
                            answer = Some((doh::dids_from_txt(&txts), ResolutionMethod::Doh));
                            break;
                        }
                        None => transport_failure = true,
                    }
                }
                answer
            }
            DnsMode::System => match self.host.resolve_txt_system(qname).await {
                Ok(txts) => Some((
                    doh::dids_from_txt(&txts.unwrap_or_default()),
                    ResolutionMethod::System,
                )),
                Err(e) => {
                    tracing::debug!(error = %e, "system DNS failed");
                    transport_failure = true;
                    None
                }
            },
        };
        let dns_answered = answer.is_some();
        match answer {
            Some((doh::TxtDids::One(did), method)) => {
                return HandleLookup::new(HandleResolution::Did { did, method }, true);
            }
            Some((doh::TxtDids::Many, _)) => {
                return HandleLookup::new(HandleResolution::Invalid, true);
            }
            Some((doh::TxtDids::None, _)) | None => {}
        }
        let mut wellknown_failed = false;
        match self.wellknown(handle).await {
            Ok(Some(did)) => {
                return HandleLookup::new(
                    HandleResolution::Did {
                        did,
                        method: ResolutionMethod::Wellknown,
                    },
                    true,
                );
            }
            Ok(None) => {}
            Err(()) => {
                transport_failure = true;
                wellknown_failed = true;
            }
        }
        if let Some(did) = self.slingshot_handle(handle).await {
            return HandleLookup::new(
                HandleResolution::Did {
                    did,
                    method: ResolutionMethod::Hint,
                },
                false,
            );
        }
        HandleLookup {
            resolution: HandleResolution::NoAnswer,
            cacheable: !transport_failure,
            unreachable: !dns_answered && wellknown_failed,
        }
    }

    /// One DoH provider: `Some(txt strings)` for an answer (NXDOMAIN is empty), `None` for a
    /// transport failure.
    async fn doh_txt(&self, endpoint: &url::Url, qname: &str) -> Option<Vec<String>> {
        let url = doh::query_url(endpoint, qname).ok()?;
        let opts = CallOpts::capped(limits::DOH_RESPONSE_MAX_BYTES).with_accept(doh::DNS_JSON);
        let resp = match self.xrpc().get_url(url, &opts).await {
            Ok(r) if r.status == 200 => r,
            Ok(r) => {
                tracing::debug!(status = r.status, "DoH: unexpected status");
                return None;
            }
            Err(e) => {
                tracing::debug!(error = %e, "DoH request failed");
                return None;
            }
        };
        doh::parse_txt_answer(&resp.body)
            .map_err(|e| tracing::debug!(error = %e, "DoH: bad answer"))
            .ok()
    }

    /// `https://<handle>/.well-known/atproto-did`: `Ok(Some)` for a DID, `Ok(None)` for a
    /// definitive no, `Err` for a transport failure (including CORS).
    async fn wellknown(&self, handle: &Handle) -> Result<Option<Did>, ()> {
        let url = url::Url::parse(&format!(
            "https://{}/.well-known/atproto-did",
            handle.as_str()
        ))
        .map_err(|_| ())?;
        let opts = CallOpts::capped(limits::WELLKNOWN_DID_MAX_BYTES)
            .with_redirects(Redirects::Follow(limits::IDENTITY_REDIRECTS));
        match self.xrpc().get_url(url, &opts).await {
            Ok(resp) if resp.status == 200 => Ok(std::str::from_utf8(&resp.body)
                .ok()
                .and_then(|s| policy_did(s.trim_ascii()))),
            Ok(_) => Ok(None),
            Err(e) if e.is_transient() || e == XrpcCallError::Cors => Err(()),
            Err(_) => Ok(None),
        }
    }

    async fn lookup_devnet(&self, pds: &url::Url, handle: &Handle) -> HandleLookup {
        let params = ResolveHandle {
            handle: handle.clone(),
        };
        let opts = CallOpts::capped(limits::SMALL_RESPONSE_MAX_BYTES);
        match self
            .xrpc()
            .query_endpoint(&Service::Anon(pds.clone()), &params, &opts)
            .await
        {
            Ok(out) => match policy_did(&out.did) {
                Some(did) => HandleLookup::new(
                    HandleResolution::Did {
                        did,
                        method: ResolutionMethod::Devnet,
                    },
                    true,
                ),
                None => HandleLookup::new(HandleResolution::NoAnswer, true),
            },
            Err(e) => HandleLookup {
                resolution: HandleResolution::NoAnswer,
                cacheable: !e.is_transient(),
                unreachable: e.is_transient(),
            },
        }
    }

    async fn minidoc(&self, identifier: &str) -> Option<(doc::MiniDoc, Vec<u8>)> {
        let base = self.slingshot()?;
        let path = format!(
            "/xrpc/com.bad-example.identity.resolveMiniDoc?{}",
            query_string(&[("identifier", identifier)])
        );
        let opts = CallOpts::capped(limits::SMALL_RESPONSE_MAX_BYTES);
        let resp = self
            .xrpc()
            .get(&Service::Anon(base.clone()), &path, &opts)
            .await
            .ok()?;
        let md: doc::MiniDoc = serde_json::from_slice(&resp.body).ok()?;
        Some((md, resp.body))
    }

    async fn slingshot_handle(&self, handle: &Handle) -> Option<Did> {
        let (md, _) = self.minidoc(handle.as_str()).await?;
        policy_did(&md.did)
    }

    /// The handle state of `did` given its claimed handle (`sync.md` §3.1).
    pub async fn handle_state(&self, did: &Did, claimed: Option<&str>) -> HandleVerdict {
        let Some(claimed) = claimed else {
            return HandleVerdict::of(HandleState::Unverified);
        };
        let Ok(handle) = self.check_handle(claimed) else {
            return HandleVerdict::of(HandleState::Invalid);
        };
        match self.resolve_handle(&handle).await {
            HandleResolution::Invalid => HandleVerdict::of(HandleState::Invalid),
            HandleResolution::Did { did: d, method }
                if d == *did && method != ResolutionMethod::Hint =>
            {
                HandleVerdict {
                    state: HandleState::Verified,
                    method: Some(method),
                    dns_unauthenticated: method == ResolutionMethod::System,
                }
            }
            HandleResolution::Did { method, .. } => HandleVerdict {
                state: HandleState::Unverified,
                method: Some(method),
                dns_unauthenticated: false,
            },
            HandleResolution::NoAnswer => HandleVerdict::of(HandleState::Unverified),
        }
    }

    // -----------------------------------------------------------------------------------------
    // DIDs

    /// The cached identity of `did`, whatever its age, without any request (views label authors
    /// from the cache; the handle shows only if its cached state is `verified`).
    pub async fn cached(&self, did: &Did) -> Option<ResolvedIdentity> {
        let r = self.read_row::<IdentityRow>(&did_key(did)).await?;
        (r.v == ROW_VERSION && (r.src == DocSource::Devnet) == self.devnet()).then(|| r.identity(did))
    }

    /// The identity of `did`, from a document at most [`DID_DOC_TTL_MS`] old.
    pub async fn resolve_did(&self, did: &Did) -> Result<ResolvedIdentity, IdentityError> {
        self.resolve_did_fresh(did, DID_DOC_TTL_MS).await
    }

    /// The identity of `did`, from a document at most `max_age_ms` old (0: fetch now). The
    /// handle state is recomputed when it is older than [`HANDLE_TTL_MS`].
    pub async fn resolve_did_fresh(
        &self,
        did: &Did,
        max_age_ms: i64,
    ) -> Result<ResolvedIdentity, IdentityError> {
        did.check_lessdugin_policy()
            .map_err(|e| IdentityError::UnsupportedDid {
                did: did.to_string(),
                reason: e.reason.to_owned(),
            })?;
        let key = did_key(did);
        let now = self.host.now_ms();
        if let Some(mut r) = self.read_row::<IdentityRow>(&key).await {
            let usable = r.v == ROW_VERSION
                && (r.src == DocSource::Devnet) == self.devnet()
                && match r.level {
                    IdentityLevel::Doc => fresh(r.doc_at, now, max_age_ms),
                    IdentityLevel::Hint => {
                        self.slingshot().is_some()
                            && fresh(r.doc_at, now, max_age_ms.min(HINT_DOC_TTL_MS))
                    }
                };
            if usable {
                if !fresh(r.h_at, now, HANDLE_TTL_MS) {
                    let verdict = self.handle_state(did, r.aka.as_deref()).await;
                    r.set_verdict(verdict, self.host.now_ms());
                    self.write_row(key, &r).await;
                }
                return Ok(r.identity(did));
            }
        }
        let mut r = self.fetch_row(did).await?;
        let verdict = self.handle_state(did, r.aka.as_deref()).await;
        r.set_verdict(verdict, self.host.now_ms());
        self.write_row(key, &r).await;
        Ok(r.identity(did))
    }

    /// The identity of `did` from a document at most `max_age_ms` old, **without** computing
    /// the handle state: sync resolves documents only (PDS and key), and DoH lookups happen
    /// only for authors on screen (`sync.md` §3.3, §14.1). A cached row is returned whatever
    /// the age of its handle state; a fetched document is cached with a handle state that is
    /// due at once (stamped 0, `unverified`), so the next [`Resolver::resolve_did`] computes
    /// it.
    pub async fn resolve_did_doc(
        &self,
        did: &Did,
        max_age_ms: i64,
    ) -> Result<ResolvedIdentity, IdentityError> {
        did.check_lessdugin_policy()
            .map_err(|e| IdentityError::UnsupportedDid {
                did: did.to_string(),
                reason: e.reason.to_owned(),
            })?;
        let key = did_key(did);
        let now = self.host.now_ms();
        if let Some(r) = self.read_row::<IdentityRow>(&key).await {
            let usable = r.v == ROW_VERSION
                && (r.src == DocSource::Devnet) == self.devnet()
                && match r.level {
                    IdentityLevel::Doc => fresh(r.doc_at, now, max_age_ms),
                    IdentityLevel::Hint => {
                        self.slingshot().is_some()
                            && fresh(r.doc_at, now, max_age_ms.min(HINT_DOC_TTL_MS))
                    }
                };
            if usable {
                return Ok(r.identity(did));
            }
        }
        let mut r = self.fetch_row(did).await?;
        r.set_verdict(HandleVerdict::of(HandleState::Unverified), 0);
        self.write_row(key, &r).await;
        Ok(r.identity(did))
    }

    fn plc_doc_url(&self, did: &Did) -> Result<url::Url, IdentityError> {
        let s = format!(
            "{}/{}",
            self.config.plc.as_str().trim_end_matches('/'),
            did.as_str()
        );
        url::Url::parse(&s).map_err(|e| IdentityError::Fetch {
            did: did.to_string(),
            error: XrpcCallError::InvalidRequest(e.to_string()),
        })
    }

    async fn fetch_row(&self, did: &Did) -> Result<IdentityRow, IdentityError> {
        let (url, src) = match did.method_kind() {
            DidMethod::Plc => (self.plc_doc_url(did)?, DocSource::Plc),
            DidMethod::Web => {
                // The policy (no port, no path) was checked by the caller; what is left is the TLD.
                let s = did.web_document_url_with(self.devnet()).ok_or_else(|| {
                    IdentityError::UnsupportedDid {
                        did: did.to_string(),
                        reason: if self.devnet() {
                            "did:web under a reserved or local-only top-level domain".to_owned()
                        } else {
                            "did:web under a reserved or local-only top-level domain (or .test \
                             outside devnet mode)"
                                .to_owned()
                        },
                    }
                })?;
                let url = url::Url::parse(&s).map_err(|e| IdentityError::UnsupportedDid {
                    did: did.to_string(),
                    reason: e.to_string(),
                })?;
                (url, DocSource::Web)
            }
            DidMethod::Other => {
                return Err(IdentityError::UnsupportedDid {
                    did: did.to_string(),
                    reason: "unsupported DID method".to_owned(),
                });
            }
        };
        let opts = CallOpts::capped(limits::DID_DOC_MAX_BYTES)
            .with_redirects(Redirects::Follow(limits::IDENTITY_REDIRECTS));
        let fetched_at = self.host.now_ms();
        let resp = match self.xrpc().get_url(url, &opts).await {
            Ok(r) => r,
            Err(e) if matches!(e.status(), Some(404 | 410)) => {
                return Err(IdentityError::DidNotFound(did.to_string()));
            }
            Err(XrpcCallError::Cors) if src == DocSource::Web && self.slingshot().is_some() => {
                return self.minidoc_row(did, fetched_at).await;
            }
            Err(error) => {
                return Err(IdentityError::Fetch {
                    did: did.to_string(),
                    error,
                });
            }
        };
        let facts = doc::validate(&resp.body, did, self.devnet()).map_err(|e| {
            IdentityError::InvalidDoc {
                did: did.to_string(),
                reason: e.to_string(),
            }
        })?;
        let (pds, pds_problem) = split(facts.pds);
        let (key, key_problem) = split(facts.key.map(|k| k.multikey()));
        Ok(IdentityRow {
            v: ROW_VERSION,
            doc: resp.body,
            level: IdentityLevel::Doc,
            src: if self.devnet() {
                DocSource::Devnet
            } else {
                src
            },
            pds,
            pds_problem,
            key,
            key_problem,
            aka: facts.claimed_handle,
            hstate: HandleState::Unverified,
            hmethod: None,
            dns_unauth: false,
            h_at: fetched_at,
            doc_at: fetched_at,
        })
    }

    /// The Slingshot mini-doc of a did:web whose document the browser could not read (hint).
    async fn minidoc_row(&self, did: &Did, at: i64) -> Result<IdentityRow, IdentityError> {
        let fail = || IdentityError::Fetch {
            did: did.to_string(),
            error: XrpcCallError::Cors,
        };
        let (md, body) = self.minidoc(did.as_str()).await.ok_or_else(fail)?;
        if md.did != did.as_str() {
            return Err(fail());
        }
        let pds = md
            .pds
            .as_deref()
            .ok_or_else(|| "the mini-doc has no PDS".to_owned())
            .and_then(|p| doc::check_service_endpoint(p, self.devnet()));
        let key = md
            .signing_key
            .as_deref()
            .ok_or_else(|| "the mini-doc has no signing key".to_owned())
            .and_then(|k| PublicKey::from_multikey(k).map_err(|e| e.to_string()))
            .map(|k| k.multikey());
        let (pds, pds_problem) = split(pds);
        let (key, key_problem) = split(key);
        Ok(IdentityRow {
            v: ROW_VERSION,
            doc: body,
            level: IdentityLevel::Hint,
            src: DocSource::Slingshot,
            pds,
            pds_problem,
            key,
            key_problem,
            aka: md.handle.map(|h| h.to_ascii_lowercase()),
            hstate: HandleState::Unverified,
            hmethod: None,
            dns_unauth: false,
            h_at: at,
            doc_at: at,
        })
    }

    // -----------------------------------------------------------------------------------------
    // Actors

    /// Resolves user input naming an actor: a DID, a handle, `@handle` or `at://<handle|did>`.
    pub async fn resolve_actor(&self, input: &str) -> Result<ResolvedActor, IdentityError> {
        self.resolve_actor_with(input, false).await
    }

    /// Like [`Resolver::resolve_actor`], but bypassing the cache: the handle is looked up now and
    /// the DID document fetched now (login, before any credential leaves the machine). The fresh
    /// answers are cached.
    pub async fn resolve_actor_fresh(&self, input: &str) -> Result<ResolvedActor, IdentityError> {
        self.resolve_actor_with(input, true).await
    }

    async fn resolve_actor_with(
        &self,
        input: &str,
        fresh: bool,
    ) -> Result<ResolvedActor, IdentityError> {
        let max_age_ms = if fresh { 0 } else { DID_DOC_TTL_MS };
        let s = input.trim();
        let s = match s.get(..5) {
            Some(p) if p.eq_ignore_ascii_case("at://") => &s[5..],
            _ => s,
        };
        let s = s.strip_prefix('@').unwrap_or(s);
        let s = s.strip_suffix('/').unwrap_or(s);
        if s.is_empty() || s.contains(['/', '?', '#', ' ']) {
            return Err(IdentityError::InvalidInput(input.to_owned()));
        }
        if s.starts_with("did:") {
            let did = Did::parse(s).map_err(|_| IdentityError::InvalidInput(input.to_owned()))?;
            let identity = self.resolve_did_fresh(&did, max_age_ms).await?;
            return Ok(ResolvedActor {
                input: input.to_owned(),
                input_handle: None,
                resolution: None,
                identity,
            });
        }
        let handle = match self.check_handle(s) {
            Ok(h) => h,
            Err(HandleProblem::Syntax) => {
                return Err(IdentityError::InvalidInput(input.to_owned()));
            }
            Err(p) => {
                return Err(IdentityError::InvalidHandle {
                    handle: s.to_ascii_lowercase(),
                    reason: p.to_string(),
                });
            }
        };
        let (did, method) = self.handle_did(&handle, !fresh).await?;
        let identity = self.resolve_did_fresh(&did, max_age_ms).await?;
        Ok(ResolvedActor {
            input: input.to_owned(),
            input_handle: Some(handle.to_string()),
            resolution: Some(HandleResolution::Did { did, method }),
            identity,
        })
    }
}

fn split<T>(r: Result<T, String>) -> (Option<T>, Option<String>) {
    match r {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_checks() {
        assert_eq!(
            check_handle("Alice.Example.COM", false).unwrap().as_str(),
            "alice.example.com"
        );
        assert_eq!(check_handle("alice", false), Err(HandleProblem::Syntax));
        assert_eq!(
            check_handle("alice.local", false),
            Err(HandleProblem::DisallowedTld)
        );
        assert_eq!(
            check_handle("handle.invalid", false),
            Err(HandleProblem::DisallowedTld)
        );
        assert_eq!(
            check_handle("alice.onion", true),
            Err(HandleProblem::DisallowedTld)
        );
        assert_eq!(
            check_handle("alice.test", false),
            Err(HandleProblem::TestTld)
        );
        assert!(check_handle("alice.test", true).is_ok());
    }

    #[test]
    fn short_dids() {
        assert_eq!(
            short_did("did:plc:ewvi7nxzyoun6zhxrhs64oiz"),
            "did:plc:ewvi7nxz…"
        );
        assert_eq!(short_did("did:web:example.com"), "did:web:example.com");
        let long = "did:web:a-very-long-subdomain.of.example.com";
        assert_eq!(long.len(), 44);
        assert_eq!(short_did(long), "did:web:a-very-long-subdomain.of.exampl…");
    }

    #[test]
    fn rows_roundtrip() {
        let did = Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap();
        let res = HandleResolution::Did {
            did: did.clone(),
            method: ResolutionMethod::Wellknown,
        };
        let r = HandleRow::new(&res, 5, false);
        let back: HandleRow = row::decode(&row::encode(&r).unwrap()).unwrap();
        assert_eq!(back.resolution(), Some(res));
        assert!(back.matches_mode(false) && !back.matches_mode(true));
        let none = HandleRow::new(&HandleResolution::NoAnswer, 1, false);
        assert_eq!(none.resolution(), Some(HandleResolution::NoAnswer));
        assert!(none.matches_mode(false) && !none.matches_mode(true));
        // Devnet rows are tagged, whatever the result.
        let dev_none = HandleRow::new(&HandleResolution::NoAnswer, 1, true);
        assert_eq!(dev_none.method, Some(ResolutionMethod::Devnet));
        assert!(dev_none.matches_mode(true) && !dev_none.matches_mode(false));
        let dev_did = HandleRow::new(
            &HandleResolution::Did {
                did: did.clone(),
                method: ResolutionMethod::Devnet,
            },
            1,
            true,
        );
        assert!(dev_did.matches_mode(true) && !dev_did.matches_mode(false));
        let ir = IdentityRow {
            v: ROW_VERSION,
            doc: b"{}".to_vec(),
            level: IdentityLevel::Doc,
            src: DocSource::Plc,
            pds: Some("https://pds.example.com".into()),
            pds_problem: None,
            key: None,
            key_problem: Some("no key".into()),
            aka: Some("alice.example.com".into()),
            hstate: HandleState::Verified,
            hmethod: Some(ResolutionMethod::Doh),
            dns_unauth: false,
            h_at: 1,
            doc_at: 2,
        };
        let bytes = row::encode(&ir).unwrap();
        let back: IdentityRow = row::decode(&bytes).unwrap();
        let id = back.identity(&did);
        assert_eq!(id.handle(), Some("alice.example.com"));
        assert_eq!(id.actor_view().short_did, "did:plc:ewvi7nxz…");
        // field names follow sync.md §10.5
        let ipld = lessdugin_atp::dagcbor::decode(&bytes).unwrap();
        let lessdugin_atp::Ipld::Map(m) = ipld else {
            panic!()
        };
        for k in [
            "v", "doc", "level", "src", "pds", "aka", "hstate", "hAt", "docAt",
        ] {
            assert!(m.contains_key(k), "{k}");
        }
    }

    #[test]
    fn freshness() {
        assert!(fresh(100, 100, 1));
        assert!(!fresh(100, 101, 1));
        assert!(!fresh(100, 99, 10), "future stamps are stale");
        assert!(!fresh(100, 100, 0));
    }
}
