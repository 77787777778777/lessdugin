// SPDX-License-Identifier: AGPL-3.0-or-later
//! CORE+ discovery hints from Constellation and relays (`sync.md` §4.4–§4.7).
//!
//! Every value returned here is a discovery hint, never admitted record content. All requests
//! are anonymous GETs with a response cap, a timeout and no redirects or custom headers. Engine
//! helpers additionally re-read `Engine::sources()` immediately before every HTTP request.
//!
//! Every listing is bounded per page (a page with more entries than the requested `limit` is
//! malformed) and in total: Constellation is read newest first up to [`SOURCE_HINTS_MAX`]
//! entries ([`RECENT_POSTS_DISCOVERY`] for recently active authors), relay newcomers up to the
//! 5,000-DID union, the relay directory up to [`DIRECTORY_OWNERS_MAX`] owners. Paging stops at
//! the bound and the rest is dropped with a warning ([`SourceStats::truncated`]).
//!
//! The sources of one sync are independent and best-effort ([`Engine::discover_forum_hints`]):
//! a failing one is logged and reported in `errors`, and the others' hints are still used.
//! Above the 5,000-DID union the relay newcomer source is disabled for the session (§4.4).

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lessdugin_atp::syntax::{Did, Nsid};
use lessdugin_atp::xrpc::query_string;
use lessdugin_lexicon::collection::nsid;
use serde_json::Value;

use crate::discovery::forum::RELAY_SOURCE;
use crate::discovery::{ForumKey, NEWCOMER_BUDGET, SyncOpts};
use crate::engine::{Engine, EngineError, ErrorCode, Sources as SourceMode};
use crate::host::{Host, HostError, HttpRequest, MaybeSendSync, Redirects, Target};
use crate::limits;
use crate::sync::{NetHost, Priority, Stats};

/// Constellation's backlink listing endpoint.
pub const GET_BACKLINKS: &str = "blue.microcosm.links.getBacklinks";
/// Constellation's backlink count endpoint.
pub const GET_BACKLINKS_COUNT: &str = "blue.microcosm.links.getBacklinksCount";
/// The relay's exact-collection repository listing endpoint.
pub const LIST_REPOS_BY_COLLECTION: &str = "com.atproto.sync.listReposByCollection";
/// Constellation's page size (the service may return short pages with a cursor).
pub const CONSTELLATION_PAGE_LIMIT: usize = 100;
/// Relay page size; do not rely on the server to clamp it (ADR 0003).
pub const RELAY_PAGE_LIMIT: usize = 500;
/// Relay newcomer discovery is disabled above this union size for the session.
pub const NEWCOMER_RELAY_UNION_LIMIT: usize = 5_000;
/// Constellation membership hints read per forum, newest first: twice the newcomer budget.
pub const SOURCE_HINTS_MAX: usize = 2 * NEWCOMER_BUDGET;
/// Newest `NS.post` backlinks read to find recently active authors (`limits.md` §11).
pub const RECENT_POSTS_DISCOVERY: usize = 100;
/// Forum owners read from the relays for the directory (`sync.md` §4.7).
pub const DIRECTORY_OWNERS_MAX: usize = 2 * NEWCOMER_RELAY_UNION_LIMIT;
/// A hard safety bound on pages in one listing. Reaching it is an error, not a complete result.
pub const MAX_SOURCE_PAGES: usize = 1_000;
/// Opaque cursors are bounded before they are put into a subsequent URL.
pub const MAX_CURSOR_BYTES: usize = 4_096;

/// Explicit CORE+ service endpoints. No `EngineConfig` field is needed for this service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceEndpoints {
    /// Constellation base URL, if enabled.
    pub constellation: Option<url::Url>,
    /// Relay base URLs, queried in order.
    pub relays: Vec<url::Url>,
}

/// Request and filtering counters for a discovery operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceStats {
    /// HTTP requests sent (or accepted by the Engine's source-mode gate).
    pub requests: u32,
    /// Listing pages returned. A count request is not a page.
    pub pages: u32,
    /// Structurally valid, relevant entries accepted before cross-source deduplication.
    pub returned: usize,
    /// Constellation entries dropped because their collection or rkey did not match exactly.
    pub dropped: usize,
    /// A listing stopped at its bound: older or further entries were dropped.
    pub truncated: bool,
}

impl SourceStats {
    fn add(&mut self, other: Self) {
        self.requests += other.requests;
        self.pages += other.pages;
        self.returned += other.returned;
        self.dropped += other.dropped;
        self.truncated |= other.truncated;
    }
}

/// DIDs and the work performed to discover them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DidHints {
    /// DIDs in source order (relay duplicates are removed, first occurrence wins).
    pub dids: Vec<String>,
    /// Discovery counters.
    pub stats: SourceStats,
}

/// Newcomer candidates and the work performed to discover them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewcomerHints {
    /// `(did, source)` pairs, newest Constellation hints first, then relay hints.
    pub candidates: Vec<(String, String)>,
    /// Discovery counters.
    pub stats: SourceStats,
    /// The errors of the sources that failed; the others' candidates are still here.
    pub errors: Vec<SourceError>,
}

/// A Constellation count suitable only for a "N members (hint)" badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HintCount {
    /// The unverified count returned by Constellation.
    pub total: u64,
    /// Discovery counters.
    pub stats: SourceStats,
}

/// Both CORE+ inputs already consumed by [`SyncOpts`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CorePlusHints {
    /// Recently active post authors, newest first (`SyncOpts::recently_active`).
    pub recently_active: Vec<String>,
    /// Newcomer candidates (`SyncOpts::newcomers`).
    pub newcomers: Vec<(String, String)>,
    /// Combined discovery counters.
    pub stats: SourceStats,
    /// The errors of the sources that failed; the others' hints are still here.
    pub errors: Vec<SourceError>,
}

impl CorePlusHints {
    /// Append these hints to existing sync options without discarding other sources' hints.
    pub fn apply_to(&self, opts: &mut SyncOpts) {
        opts.recently_active
            .extend(self.recently_active.iter().cloned());
        opts.newcomers.extend(self.newcomers.iter().cloned());
    }
}

/// Why a CORE+ discovery operation did not produce a complete result.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// `sources = direct`; the request was not sent.
    #[error("CORE+ discovery is disabled while sources = direct")]
    DirectMode,
    /// The live sources setting could not be read. Engine treats this as direct.
    #[error("the sources setting could not be read: {0}")]
    SourceMode(String),
    /// An endpoint or exact collection was invalid.
    #[error("invalid discovery request: {0}")]
    InvalidInput(String),
    /// A required service was not configured.
    #[error("no {0} endpoint is configured")]
    MissingEndpoint(&'static str),
    /// The host failed before a complete HTTP response arrived.
    #[error("{service} request failed: {error}")]
    Transport {
        /// Which CORE+ service failed.
        service: &'static str,
        /// The host error (which never contains request credentials).
        error: HostError,
    },
    /// The service returned a non-success status. Redirects are never followed.
    #[error("{service} returned HTTP {status}")]
    Http {
        /// Which CORE+ service failed.
        service: &'static str,
        /// HTTP status.
        status: u16,
    },
    /// A success response had the wrong JSON shape or an invalid DID.
    #[error("malformed {service} response: {detail}")]
    Malformed {
        /// Which CORE+ service failed.
        service: &'static str,
        /// A bounded description of the validation failure.
        detail: String,
    },
    /// The service returned a cursor already seen in this listing.
    #[error("{service} repeated cursor at page {page}")]
    RepeatedCursor {
        /// Which CORE+ service failed.
        service: &'static str,
        /// The page on which the repeated cursor appeared.
        page: usize,
    },
    /// The service did not finish within the hard page bound.
    #[error("{service} exceeded the {MAX_SOURCE_PAGES}-page safety bound")]
    PageLimit {
        /// Which CORE+ service failed.
        service: &'static str,
    },
    /// Relay newcomer enumeration crossed the global relay-union cap. The partial list is not
    /// returned as though it were complete.
    #[error("relay newcomer union exceeded {limit} DIDs (observed at least {observed})")]
    NewcomerUnionTooLarge {
        /// The configured normative cap.
        limit: usize,
        /// The union size at which enumeration stopped.
        observed: usize,
    },
}

impl SourceError {
    /// The source-mode gate refused the request: every CORE+ source is off, not one failing.
    pub fn is_gate(&self) -> bool {
        matches!(self, Self::DirectMode | Self::SourceMode(_))
    }
}

/// Records the error of a best-effort source (logged, kept in `errors`) unless it is the
/// source-mode gate's, which stops the whole discovery.
fn best_effort(
    source: &'static str,
    e: SourceError,
    errors: &mut Vec<SourceError>,
) -> Result<(), SourceError> {
    if e.is_gate() {
        return Err(e);
    }
    tracing::warn!(source, error = %e, "CORE+ hint source unavailable; other sources still used");
    errors.push(e);
    Ok(())
}

impl From<SourceError> for EngineError {
    fn from(e: SourceError) -> Self {
        let message = e.to_string();
        match e {
            SourceError::DirectMode | SourceError::SourceMode(_) => Self::blocked(message),
            SourceError::InvalidInput(_) | SourceError::MissingEndpoint(_) => {
                Self::invalid_input(message)
            }
            SourceError::Transport {
                error: HostError::TooLarge,
                ..
            }
            | SourceError::NewcomerUnionTooLarge { .. }
            | SourceError::PageLimit { .. } => Self::new(ErrorCode::TooLarge, message),
            SourceError::Transport { .. } | SourceError::Http { status: 500.., .. } => {
                Self::new(ErrorCode::Network, message)
            }
            SourceError::Http { status, .. } => Self::new(ErrorCode::Network, message)
                .with_retryable(false)
                .with_detail("status", status),
            SourceError::Malformed { .. } | SourceError::RepeatedCursor { .. } => {
                Self::new(ErrorCode::Network, message).with_retryable(false)
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
type GateFuture = Pin<Box<dyn Future<Output = Result<(), SourceError>> + Send>>;
#[cfg(target_arch = "wasm32")]
type GateFuture = Pin<Box<dyn Future<Output = Result<(), SourceError>>>>;

trait SourceGate: Fn() -> GateFuture + MaybeSendSync {}
impl<T: Fn() -> GateFuture + MaybeSendSync> SourceGate for T {}

/// Host-independent Constellation and relay discovery over explicit endpoints.
pub struct Sources<H: Host> {
    host: Arc<H>,
    endpoints: SourceEndpoints,
    gate: Option<Arc<dyn SourceGate>>,
    /// The relay newcomer source crossed its union cap: disabled for the session (§4.4). Shared
    /// by clones; the engine keeps one per running engine.
    relay_off: Arc<AtomicBool>,
}

impl<H: Host> Clone for Sources<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            endpoints: self.endpoints.clone(),
            gate: self.gate.clone(),
            relay_off: Arc::clone(&self.relay_off),
        }
    }
}

impl<H: Host> Sources<H> {
    /// A service over `host`. For an ungated service the caller decides whether CORE+ is enabled;
    /// the [`Engine`] helpers install a live per-request source-mode gate.
    pub fn new(host: impl Into<Arc<H>>, endpoints: SourceEndpoints) -> Self {
        Self {
            host: host.into(),
            endpoints,
            gate: None,
            relay_off: Arc::default(),
        }
    }

    fn gated(
        host: Arc<H>,
        endpoints: SourceEndpoints,
        gate: Arc<dyn SourceGate>,
        relay_off: Arc<AtomicBool>,
    ) -> Self {
        Self {
            host,
            endpoints,
            gate: Some(gate),
            relay_off,
        }
    }

    /// Exact-filtered Constellation membership hints, in the service's newest-first order.
    /// Paging continues until `cursor: null`, including after short and empty pages, or until
    /// [`SOURCE_HINTS_MAX`] hints were read.
    pub async fn constellation_newcomers(&self, forum: &ForumKey) -> Result<DidHints, SourceError> {
        self.backlinks(
            forum,
            nsid::MEMBERSHIP,
            Some(forum.fk.as_str()),
            SOURCE_HINTS_MAX,
        )
        .await
    }

    /// Recently active post authors from the newest [`RECENT_POSTS_DISCOVERY`] exact
    /// `NS.post:forum` backlinks, newest first.
    pub async fn recently_active(&self, forum: &ForumKey) -> Result<DidHints, SourceError> {
        let Some(_) = &self.endpoints.constellation else {
            return Ok(DidHints::default());
        };
        self.backlinks(forum, nsid::POST, None, RECENT_POSTS_DISCOVERY)
            .await
    }

    /// Constellation's unverified membership count for a future UI badge.
    pub async fn membership_count(&self, forum: &ForumKey) -> Result<HintCount, SourceError> {
        let base = self
            .endpoints
            .constellation
            .as_ref()
            .ok_or(SourceError::MissingEndpoint("Constellation"))?;
        let source = format!("{}:forum", nsid::MEMBERSHIP);
        let params = [("subject", forum.uri.as_str()), ("source", source.as_str())];
        let value = self
            .get_json(base, GET_BACKLINKS_COUNT, &params, "Constellation")
            .await?;
        let total = value
            .as_object()
            .and_then(|o| o.get("total"))
            .and_then(Value::as_u64)
            .ok_or_else(|| malformed("Constellation", "total must be a non-negative integer"))?;
        Ok(HintCount {
            total,
            stats: SourceStats {
                requests: 1,
                ..SourceStats::default()
            },
        })
    }

    /// Exact backlinks, newest first, at most `cap` of them: paging stops there.
    async fn backlinks(
        &self,
        forum: &ForumKey,
        collection: &str,
        exact_rkey: Option<&str>,
        cap: usize,
    ) -> Result<DidHints, SourceError> {
        let base = self
            .endpoints
            .constellation
            .as_ref()
            .ok_or(SourceError::MissingEndpoint("Constellation"))?;
        let source = format!("{collection}:forum");
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        let mut dids = Vec::new();
        let mut stats = SourceStats::default();
        loop {
            if stats.pages as usize >= MAX_SOURCE_PAGES {
                return Err(SourceError::PageLimit {
                    service: "Constellation",
                });
            }
            let mut params = vec![
                ("subject", forum.uri.as_str()),
                ("source", source.as_str()),
                ("limit", "100"),
            ];
            if let Some(c) = &cursor {
                params.push(("cursor", c));
            }
            let value = self
                .get_json(base, GET_BACKLINKS, &params, "Constellation")
                .await?;
            stats.requests += 1;
            stats.pages += 1;
            let object = value
                .as_object()
                .ok_or_else(|| malformed("Constellation", "top level must be an object"))?;
            let records = object
                .get("records")
                .and_then(Value::as_array)
                .ok_or_else(|| malformed("Constellation", "records must be an array"))?;
            if records.len() > CONSTELLATION_PAGE_LIMIT {
                return Err(malformed(
                    "Constellation",
                    "a page has more records than the requested limit",
                ));
            }
            for record in records {
                let entry = parse_backlink(record)?;
                if entry.collection != collection || exact_rkey.is_some_and(|fk| entry.rkey != fk) {
                    stats.dropped += 1;
                    continue;
                }
                if dids.len() >= cap {
                    stats.truncated = true;
                    continue;
                }
                stats.returned += 1;
                dids.push(entry.did);
            }
            cursor = parse_cursor(object.get("cursor"), "Constellation")?;
            if dids.len() >= cap {
                // newest first: what is left is older
                stats.truncated |= cursor.is_some();
                break;
            }
            let Some(next) = &cursor else { break };
            if !seen.insert(next.clone()) {
                return Err(SourceError::RepeatedCursor {
                    service: "Constellation",
                    page: stats.pages as usize,
                });
            }
        }
        if stats.truncated {
            tracing::warn!(
                forum = %forum.uri,
                collection,
                cap,
                "Constellation listing truncated at its bound; older entries dropped"
            );
        }
        Ok(DidHints { dids, stats })
    }

    /// Exact-collection repository DIDs across all configured relays, deduplicated in relay/page
    /// order. This directory primitive has no newcomer-union cap: it stops at
    /// [`DIRECTORY_OWNERS_MAX`] DIDs and drops the rest with a warning.
    pub async fn relay_repositories(&self, collection: &str) -> Result<DidHints, SourceError> {
        self.relay_repositories_inner(collection, RelayBound::Truncate(DIRECTORY_OWNERS_MAX))
            .await
    }

    /// Relay membership hints, refusing enumeration as soon as the cross-relay union exceeds
    /// 5,000 DIDs. A relay entry names a repo with some `NS.membership` record, not a forum, so
    /// each DID is only a candidate, confirmed at its own PDS (§4.2.1) like any other. No partial
    /// result is silently returned.
    pub async fn relay_newcomers(&self) -> Result<DidHints, SourceError> {
        self.relay_repositories_inner(
            nsid::MEMBERSHIP,
            RelayBound::Refuse(NEWCOMER_RELAY_UNION_LIMIT),
        )
        .await
    }

    async fn relay_repositories_inner(
        &self,
        collection: &str,
        bound: RelayBound,
    ) -> Result<DidHints, SourceError> {
        Nsid::parse(collection)
            .map_err(|e| SourceError::InvalidInput(format!("collection must be exact: {e}")))?;
        let mut dids = Vec::new();
        let mut union = BTreeSet::new();
        let mut stats = SourceStats::default();
        let relays = &self.endpoints.relays;
        'relays: for (i, base) in relays.iter().enumerate() {
            let mut cursor: Option<String> = None;
            let mut seen = BTreeSet::new();
            loop {
                if stats.pages as usize >= MAX_SOURCE_PAGES {
                    return Err(SourceError::PageLimit { service: "relay" });
                }
                let mut params = vec![("collection", collection), ("limit", "500")];
                if let Some(c) = &cursor {
                    params.push(("cursor", c));
                }
                let value = self
                    .get_json(base, LIST_REPOS_BY_COLLECTION, &params, "relay")
                    .await?;
                stats.requests += 1;
                stats.pages += 1;
                let object = value
                    .as_object()
                    .ok_or_else(|| malformed("relay", "top level must be an object"))?;
                let repos = object
                    .get("repos")
                    .and_then(Value::as_array)
                    .ok_or_else(|| malformed("relay", "repos must be an array"))?;
                if repos.len() > RELAY_PAGE_LIMIT {
                    return Err(malformed(
                        "relay",
                        "a page has more repos than the requested limit",
                    ));
                }
                for repo in repos {
                    let object = repo
                        .as_object()
                        .ok_or_else(|| malformed("relay", "repo entry must be an object"))?;
                    let did = object
                        .get("did")
                        .and_then(Value::as_str)
                        .ok_or_else(|| malformed("relay", "repo did must be a string"))?;
                    let did = strict_did(did, "relay")?;
                    stats.returned += 1;
                    if union.contains(&did) {
                        continue;
                    }
                    match bound {
                        RelayBound::Refuse(limit) if union.len() >= limit => {
                            return Err(SourceError::NewcomerUnionTooLarge {
                                limit,
                                observed: union.len() + 1,
                            });
                        }
                        RelayBound::Truncate(cap) if union.len() >= cap => {
                            stats.truncated = true;
                            continue;
                        }
                        _ => {}
                    }
                    union.insert(did.clone());
                    dids.push(did);
                }
                cursor = parse_cursor(object.get("cursor"), "relay")?;
                if let RelayBound::Truncate(cap) = bound
                    && union.len() >= cap
                {
                    // nothing more is read: further pages and relays could only add DIDs
                    stats.truncated |= cursor.is_some() || i + 1 < relays.len();
                    break 'relays;
                }
                let Some(next) = &cursor else { break };
                if !seen.insert(next.clone()) {
                    return Err(SourceError::RepeatedCursor {
                        service: "relay",
                        page: stats.pages as usize,
                    });
                }
            }
        }
        if stats.truncated {
            tracing::warn!(
                collection,
                ?bound,
                "relay listing truncated at its bound; further repos dropped"
            );
        }
        Ok(DidHints { dids, stats })
    }

    /// Constellation plus relay newcomer hints. Constellation order is retained; relay-only DIDs
    /// follow in relay order, and duplicates across the two sources are removed. Each source is
    /// best-effort: one that fails is logged and reported in `errors`, and the other's hints are
    /// still returned. Once the relay union crosses 5,000 DIDs the relay source is disabled for
    /// the session. Only the source-mode gate's refusal is an error.
    pub async fn newcomer_hints(&self, forum: &ForumKey) -> Result<NewcomerHints, SourceError> {
        let mut out = NewcomerHints::default();
        let mut seen = BTreeSet::new();
        if self.endpoints.constellation.is_some() {
            match self.constellation_newcomers(forum).await {
                Ok(c) => {
                    out.stats.add(c.stats);
                    for did in c.dids {
                        if seen.insert(did.clone()) {
                            out.candidates.push((did, "constellation".to_owned()));
                        }
                    }
                }
                Err(e) => best_effort("Constellation", e, &mut out.errors)?,
            }
        }
        if self.endpoints.relays.is_empty() || self.relay_off.load(Ordering::SeqCst) {
            return Ok(out);
        }
        match self.relay_newcomers().await {
            Ok(r) => {
                out.stats.add(r.stats);
                for did in r.dids {
                    if seen.insert(did.clone()) {
                        out.candidates.push((did, RELAY_SOURCE.to_owned()));
                    }
                }
            }
            Err(e) => {
                if matches!(e, SourceError::NewcomerUnionTooLarge { .. }) {
                    self.relay_off.store(true, Ordering::SeqCst);
                    tracing::warn!(error = %e, "relay newcomer source disabled for the session");
                }
                best_effort("relay", e, &mut out.errors)?;
            }
        }
        Ok(out)
    }

    async fn get_json(
        &self,
        base: &url::Url,
        nsid: &str,
        params: &[(&str, &str)],
        service: &'static str,
    ) -> Result<Value, SourceError> {
        if !base.username().is_empty() || base.password().is_some() {
            return Err(SourceError::InvalidInput(
                "service endpoint must not contain userinfo".to_owned(),
            ));
        }
        let query = query_string(params);
        let url = format!(
            "{}/xrpc/{nsid}{}",
            base.as_str().trim_end_matches('/'),
            if query.is_empty() {
                String::new()
            } else {
                format!("?{query}")
            }
        );
        let url = url::Url::parse(&url)
            .map_err(|e| SourceError::InvalidInput(format!("bad service URL: {e}")))?;
        let req = HttpRequest::get(Target::Anon(url))
            .with_max_bytes(limits::DEFAULT_RESPONSE_MAX_BYTES)
            .with_timeout_ms(limits::REQUEST_TIMEOUT_MS)
            .with_redirects(Redirects::None);
        // Keep this gate immediately adjacent to Host::http: setting changes between pages (or
        // before a count request) therefore stop the very next CORE+ request.
        if let Some(gate) = &self.gate {
            gate().await?;
        }
        let response = self
            .host
            .http(req)
            .await
            .map_err(|error| SourceError::Transport { service, error })?;
        if !(200..300).contains(&response.status) {
            return Err(SourceError::Http {
                service,
                status: response.status,
            });
        }
        serde_json::from_slice(&response.body).map_err(|e| {
            malformed(
                service,
                &format!(
                    "invalid JSON: {}",
                    e.to_string().chars().take(256).collect::<String>()
                ),
            )
        })
    }
}

/// How a relay listing is bounded in total.
#[derive(Clone, Copy, Debug)]
enum RelayBound {
    /// More distinct DIDs than this is an error (the newcomer union, §4.4).
    Refuse(usize),
    /// Stop at this many distinct DIDs and drop the rest with a warning (the directory, §4.7).
    Truncate(usize),
}

struct Backlink {
    did: String,
    collection: String,
    rkey: String,
}

fn parse_backlink(value: &Value) -> Result<Backlink, SourceError> {
    let object = value
        .as_object()
        .ok_or_else(|| malformed("Constellation", "record must be an object"))?;
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("Constellation", &format!("record {name} must be a string")))
    };
    let did = strict_did(field("did")?, "Constellation")?;
    let collection = field("collection")?.to_owned();
    Nsid::parse(&collection)
        .map_err(|_| malformed("Constellation", "record collection must be an NSID"))?;
    let rkey = field("rkey")?.to_owned();
    if rkey.is_empty() || rkey.len() > 1_024 {
        return Err(malformed(
            "Constellation",
            "record rkey has an invalid length",
        ));
    }
    Ok(Backlink {
        did,
        collection,
        rkey,
    })
}

fn strict_did(raw: &str, service: &'static str) -> Result<String, SourceError> {
    let did = Did::parse(raw)
        .and_then(|did| {
            did.check_lessdugin_policy()?;
            Ok(did)
        })
        .map_err(|_| malformed(service, "record did is not a supported canonical DID"))?;
    Ok(did.to_string())
}

fn parse_cursor(
    value: Option<&Value>,
    service: &'static str,
) -> Result<Option<String>, SourceError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.is_empty() && s.len() <= MAX_CURSOR_BYTES => {
            Ok(Some(s.clone()))
        }
        _ => Err(malformed(
            service,
            "cursor must be null or a non-empty string of at most 4096 bytes",
        )),
    }
}

fn malformed(service: &'static str, detail: &str) -> SourceError {
    SourceError::Malformed {
        service,
        detail: detail.chars().take(256).collect(),
    }
}

impl<H: Host> Engine<H> {
    fn core_plus_service(&self, priority: Priority) -> Sources<NetHost<H>> {
        let stats = Arc::new(Stats::new());
        let syncer = self.syncer();
        let host = Arc::new(NetHost::new(
            self.shared_host(),
            Arc::clone(syncer.shared().net()),
            priority,
            stats,
        ));
        let endpoints = SourceEndpoints {
            constellation: self.config().constellation.clone(),
            relays: self.config().relays.clone(),
        };
        // In wasm an Engine's callback runtime is not Send; the core is single-threaded there.
        #[allow(clippy::arc_with_non_send_sync)]
        let gate: Arc<dyn SourceGate> = {
            let engine = self.clone();
            Arc::new(move || {
                let engine = engine.clone();
                Box::pin(async move {
                    match engine.sources().await {
                        Ok(SourceMode::Assisted) => Ok(()),
                        Ok(SourceMode::Direct) => Err(SourceError::DirectMode),
                        Err(e) => Err(SourceError::SourceMode(e.to_string())),
                    }
                }) as GateFuture
            })
        };
        Sources::gated(
            host,
            endpoints,
            gate,
            Arc::clone(self.relay_newcomers_off()),
        )
    }

    /// Discover recently active post authors. The live sources setting is checked before every
    /// Constellation request and requests use the engine's P1 scheduler and budgets.
    pub async fn discover_recently_active(
        &self,
        forum: &ForumKey,
    ) -> Result<DidHints, SourceError> {
        self.core_plus_service(Priority::P1)
            .recently_active(forum)
            .await
    }

    /// Discover Constellation and relay newcomers, feeding the exact shape of
    /// [`SyncOpts::newcomers`]. Relay enumeration has the 5,000-DID union cap (above it the
    /// relay source is off for the session); every candidate is confirmed at its own PDS by the
    /// sync (§4.2.1). A failing source is reported in `errors`, not as an error.
    pub async fn discover_newcomers(&self, forum: &ForumKey) -> Result<NewcomerHints, SourceError> {
        self.core_plus_service(Priority::P2)
            .newcomer_hints(forum)
            .await
    }

    /// Discover both CORE+ confirmation sinks consumed by [`SyncOpts`]. Recently active authors,
    /// Constellation newcomers and relay newcomers are independent and best-effort: a source that
    /// fails is logged and reported in `errors`, and the others' hints are returned. Only the
    /// source-mode gate (`sources = direct`) makes this an error.
    pub async fn discover_forum_hints(
        &self,
        forum: &ForumKey,
    ) -> Result<CorePlusHints, SourceError> {
        let mut hints = CorePlusHints::default();
        match self.discover_recently_active(forum).await {
            Ok(recent) => {
                hints.stats.add(recent.stats);
                hints.recently_active = recent.dids;
            }
            Err(e) => best_effort("Constellation", e, &mut hints.errors)?,
        }
        let newcomers = self.discover_newcomers(forum).await?;
        hints.stats.add(newcomers.stats);
        hints.newcomers = newcomers.candidates;
        hints.errors.extend(newcomers.errors);
        Ok(hints)
    }

    /// Constellation's membership count hint, gated and scheduled like the other CORE+ calls.
    pub async fn discover_membership_count(
        &self,
        forum: &ForumKey,
    ) -> Result<HintCount, SourceError> {
        self.core_plus_service(Priority::P2)
            .membership_count(forum)
            .await
    }

    /// Relay forum-owner hints for the directory. Unlike newcomer enumeration this is not capped
    /// at 5,000 DIDs; it stops at [`DIRECTORY_OWNERS_MAX`].
    pub async fn discover_forum_owners(&self) -> Result<DidHints, SourceError> {
        self.core_plus_service(Priority::P2)
            .relay_repositories(nsid::FORUM)
            .await
    }

    /// Discover CORE+ hints, feed them to the existing confirmation sinks, and run `sync_forum`.
    /// Candidate confirmation and all subsequent PDS traffic remain the existing syncer's work.
    pub async fn sync_forum_discovered(
        &self,
        forum: &str,
        opts: &SyncOpts,
    ) -> Result<crate::dto::SyncReport, EngineError> {
        let key = self.forum_key(forum).await?;
        let hints = self.discover_forum_hints(&key).await?;
        let mut opts = opts.clone();
        hints.apply_to(&mut opts);
        self.sync_forum_resolved(&key, opts).await
    }
}
