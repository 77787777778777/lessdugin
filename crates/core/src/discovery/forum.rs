// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opening a forum (`sync.md` §4.2) and syncing it in the order of §4.5.
//!
//! ```text
//! open_forum(F):  owner identity (a hint → unavailable, fail closed)
//!                 decl = getRecord (RecordNotFound → tombstone, "forum not found";
//!                        invalid → "forum invalid"; a transport failure falls back to the cache)
//!                 no proven version cached → sync.getRecord + proofs/1 → level proven, else a
//!                 warning "declaration proof failed"
//!                 A = {owner} ∪ moderators (FO2)
//! sync_forum(F):  1. A at P0 (+ 2. authors on screen at P0, confirmed)
//!                 roster (G, B, N, S) from the authorities' records
//!                 4. G ∖ A at P1, not gated by a membership (seeds first)
//!                 open mode: 5. the closure of S at P1, confirmed;
//!                            6. newcomers from hints at P2, confirmed, at most 500
//!                 completeness (C(F), k/n, degraded) → SyncReport
//! ```
//!
//! Step 3 (recently active authors from Constellation / a Jetstream replay) and the newcomer
//! *sources* (Constellation, Jetstream, indexer, relay; §4.4) are CORE+ and arrive with M5: they
//! feed [`SyncOpts::newcomers`] (and later a recently-active list), which this module already
//! confirms and syncs within the budget.
//!
//! Each candidate is confirmed by `sync_repo` itself (the membership `getRecord` comes first and
//! stops the pass if absent or invalid, §4.2.1), so a confirmed candidate costs no extra request.
//! Rejected candidates get a `rejected` disc row and are skipped until a new hint names them (or
//! [`state::REJECTED_RECHECK_MS`] passes).
//!
//! **Grants are not gated.** `sync.md` §4.2.1 confirms every participant outside A, but
//! `social.md` §3.3 keeps a grant counting after its holder leaves (their opinions keep
//! counting), so a granted repo is synced like an authority's: without the membership gate,
//! never held back by a rejection, and always in C(F). Gating it would freeze a leaver's last
//! records on clients that synced them while fresh clients never see them, with both reporting
//! complete (I6). The tension between the two sections is for the spec owner.
//!
//! **Budgets.** `max_requests` and `budget_ms` are enforced per request by the command's
//! [`Stats`]: once spent, nothing new is sent, work in flight ends, the report is `[partial]`
//! and carries one `time_budget` / `request_limit` issue. Nothing here sleeps or spawns.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use futures::future::join_all;
use lessdugin_atp::Cid;
use lessdugin_atp::proof::{ProofCaps, verify_proof};
use lessdugin_atp::syntax::{AtIdentifier, Did};
use lessdugin_atp::xrpc::SyncGetRecord;
use lessdugin_lexicon::records::Forum;

use crate::discovery::forum_ref::{ForumKey, ForumRefError, parse_forum_ref};
use crate::discovery::roster;
use crate::discovery::state::{self, DiscRow, DiscState, ForumState, put_disc_rows};
use crate::dto::{Completeness, DtoVersion, SyncIssue, SyncReport};
use crate::engine::EngineError;
use crate::host::{Host, MaybeSendSync};
use crate::identity::{DID_DOC_TTL_MS, IdentityError, IdentityLevel, ResolvedIdentity};
use crate::limits;
use crate::store::admit::{self, LexiconValidator};
use crate::store::blob::ForumRow;
use crate::store::records::{Level, RecordRow, RecordStore, Source};
use crate::store::{Observation, StoreError};
use crate::sync::net::{self, Priority, REQUEST_LIMIT_REACHED, Stats, TIME_BUDGET_SPENT};
use crate::sync::{
    MembershipCheck, PointOutcome, Reason, RepoOutcome, RepoReport, Session, SyncError,
    SyncRequest, Syncer,
};
use crate::xrpc::{CallOpts, Service};

/// Newcomer candidates confirmed and synced per forum and command (§4.4, *tunable*).
pub const NEWCOMER_BUDGET: usize = 500;

/// A progress callback of [`Syncer::sync_forum`]: `(done, total)`, the repos synced so far and
/// the repos planned so far (the total grows as each step plans its candidates). Called as each
/// repo finishes, on the task that drives the sync.
#[derive(Clone, Default)]
pub struct Progress(Option<Arc<dyn ProgressFn>>);

/// A progress callback (`Fn(done, total)`; `Send + Sync` on native targets).
pub trait ProgressFn: Fn(u32, u32) + MaybeSendSync {}

impl<F: Fn(u32, u32) + MaybeSendSync + ?Sized> ProgressFn for F {}

impl Progress {
    /// A callback.
    // In wasm callbacks are not `Send`; the core is single-threaded there.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(f: impl ProgressFn + 'static) -> Self {
        Self(Some(Arc::new(f)))
    }

    /// Reports `(done, total)`.
    pub fn report(&self, done: u32, total: u32) {
        if let Some(f) = &self.0 {
            f(done, total);
        }
    }
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Progress(callback)"
        } else {
            "Progress(none)"
        })
    }
}

/// Options compare equal whatever their callbacks.
impl PartialEq for Progress {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Progress {}

/// Repos planned and done during one sync (for [`Progress`]).
#[derive(Default)]
struct Tally {
    done: AtomicU32,
    total: AtomicU32,
}

/// Settings of [`Syncer::sync_forum`] (`engine-api.md` §3.3 `SyncOpts`, plus the inputs of the
/// view and CORE+ steps).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncOpts {
    /// Time budget (ms): no request is sent after it.
    pub budget_ms: Option<i64>,
    /// Request cap for the command.
    pub max_requests: Option<u32>,
    /// `sync --full`: cached state discarded, rollbacks accepted, `describeRepo` refreshed.
    pub full: bool,
    /// Step 2: DIDs of the authors on screen (P0, confirmed unless authorities).
    pub on_screen: Vec<String>,
    /// Step 3: recently active authors from hints (assisted mode: Constellation backlinks of
    /// posts, a Jetstream replay; M5), most recent first. Synced at P1 and confirmed: in approval
    /// mode only those in G; in open mode any, those outside A ∪ G counting toward the
    /// newcomer budget.
    pub recently_active: Vec<String>,
    /// Step 6: newcomer candidates from hints, most recent first, as `(did, source)` (§4.4);
    /// used in open mode only, at most [`NEWCOMER_BUDGET`] (with step 3's).
    pub newcomers: Vec<(String, String)>,
    /// Progress reports while the sync runs.
    pub progress: Progress,
}

/// How the declaration's proof went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclProof {
    /// Proven now at this commit rev.
    Proven {
        /// The commit rev.
        rev: String,
    },
    /// A proven version was already cached.
    Cached {
        /// The commit rev of the cached proof.
        rev: Option<String>,
    },
    /// The proof failed; the declaration stays at level `pds` (a header warning).
    Failed(String),
}

/// An opened forum.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenedForum {
    /// F, owner, slug, fk.
    pub key: ForumKey,
    /// The declaration.
    pub decl: Forum,
    /// Its CID.
    pub decl_cid: Cid,
    /// A, with ranks (3 owner, 2 admin, 1 moderator).
    pub authorities: BTreeMap<String, u8>,
    /// The declaration proof.
    pub proof: DeclProof,
    /// Header warnings ("declaration proof failed: …", "declaration from cache: …").
    pub warnings: Vec<String>,
}

/// Why a forum cannot be opened (fail closed, §4.6).
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum DiscoveryError {
    /// The reference does not parse.
    #[error(transparent)]
    Ref(#[from] ForumRefError),
    /// The owner (or the handle in the reference) did not resolve.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// The owner identity is only a hint.
    #[error("the identity of forum owner {0} is only a hint: the forum is unavailable")]
    OwnerHint(String),
    /// The declaration does not exist.
    #[error("forum not found: {0}")]
    NotFound(String),
    /// The declaration is invalid (`data-model.md` §7).
    #[error("forum {uri} is invalid ({rule})")]
    Invalid {
        /// F.
        uri: String,
        /// The first failing rule.
        rule: String,
    },
    /// Fetching the declaration failed and nothing is cached.
    #[error(transparent)]
    Sync(#[from] SyncError),
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<SyncError> for EngineError {
    fn from(e: SyncError) -> Self {
        match e {
            SyncError::Identity(e) => e.into(),
            SyncError::Xrpc(e) | SyncError::List(crate::sync::list::ListError::Xrpc(e)) => e.into(),
            SyncError::Store(e) => EngineError::internal(e.to_string()),
            e => EngineError::new(crate::engine::ErrorCode::Network, e.to_string())
                .with_retryable(true),
        }
    }
}

impl From<DiscoveryError> for EngineError {
    fn from(e: DiscoveryError) -> Self {
        match e {
            DiscoveryError::Ref(e) => EngineError::invalid_ref(e.to_string()),
            DiscoveryError::Identity(e) => e.into(),
            DiscoveryError::OwnerHint(_) => {
                EngineError::new(crate::engine::ErrorCode::Network, e.to_string())
                    .with_retryable(true)
            }
            DiscoveryError::NotFound(_) => EngineError::not_found(e.to_string()),
            DiscoveryError::Invalid { .. } => EngineError::invalid_input(e.to_string()),
            DiscoveryError::Sync(e) => e.into(),
            DiscoveryError::Store(e) => EngineError::internal(e.to_string()),
        }
    }
}

/// The result of [`Syncer::sync_forum`].
#[derive(Clone, Debug, PartialEq)]
pub struct ForumSync {
    /// The opened forum.
    pub forum: OpenedForum,
    /// Roster, closure and discovery rows after the sync.
    pub state: ForumState,
    /// Completeness after the sync.
    pub completeness: Completeness,
    /// The report.
    pub report: SyncReport,
    /// One report per `sync_repo` call, in call order.
    pub repos: Vec<RepoReport>,
}

/// A candidate to confirm and sync.
struct Candidate {
    did: Did,
    src: String,
    /// Named by a new hint: a rejected disc row does not hold it back.
    hinted: bool,
    /// In G: synced like an authority, without the membership gate, whatever its disc row says
    /// (`social.md` §3.3: a grant still counts after its holder leaves).
    granted: bool,
}

fn parse_dids<'a>(it: impl IntoIterator<Item = &'a String>) -> Vec<Did> {
    it.into_iter().filter_map(|d| Did::parse(d).ok()).collect()
}

impl<H: Host> Syncer<H> {
    /// Resolves a forum reference (§4.1) to its canonical key (a handle owner is resolved).
    pub async fn resolve_forum_ref(
        &self,
        s: &Session<H>,
        input: &str,
    ) -> Result<ForumKey, DiscoveryError> {
        let r = parse_forum_ref(input)?;
        let owner = match &r.owner {
            AtIdentifier::Did(d) => d.clone(),
            AtIdentifier::Handle(h) => {
                let h = s.resolver().check_handle(h.as_str()).map_err(|e| {
                    IdentityError::InvalidHandle {
                        handle: h.to_string(),
                        reason: e.to_string(),
                    }
                })?;
                s.resolver().resolve_handle_did(&h).await?.0
            }
        };
        Ok(ForumKey::new(&owner, &r.slug)?)
    }

    /// `open_forum` (§4.2): the declaration from the owner's PDS, its proof on first open, and
    /// the authorities. Fails closed (§4.6).
    pub async fn open_forum(
        &self,
        s: &Session<H>,
        key: &ForumKey,
    ) -> Result<OpenedForum, DiscoveryError> {
        let owner = &key.owner;
        let id = s.resolver().resolve_did_doc(owner, DID_DOC_TTL_MS).await?;
        if id.level == IdentityLevel::Hint {
            return Err(DiscoveryError::OwnerHint(owner.to_string()));
        }
        let mut warnings = Vec::new();
        let mut fetch_error = None;
        match self.point_fetch(s, &key.uri, None).await {
            Ok(PointOutcome::Admitted { .. }) => {}
            Ok(PointOutcome::NotFound { .. }) => {
                return Err(DiscoveryError::NotFound(key.uri.to_string()));
            }
            Err(e) if e.is_transient() || e.is_refusal() => {
                warnings.push(format!("declaration from cache: {e}"));
                fetch_error = Some(e);
            }
            Err(e) => return Err(e.into()),
        }
        let store = RecordStore::new(s.host().cache());
        let Some(row) = store.get(&key.uri).await? else {
            return Err(match fetch_error {
                Some(e) => e.into(),
                None => DiscoveryError::NotFound(key.uri.to_string()),
            });
        };
        let invalid = |rule: String| DiscoveryError::Invalid {
            uri: key.uri.to_string(),
            rule,
        };
        if !row.valid {
            return Err(invalid(row.rule.clone().unwrap_or_default()));
        }
        let decl: Forum = row
            .value_json()
            .map_err(|e| invalid(e.to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(|e| invalid(e.to_string())))?;
        let proof = if row.level == Level::Proven {
            DeclProof::Cached {
                rev: row.rev.clone(),
            }
        } else {
            self.prove_declaration(s, key, &id, &row).await
        };
        if let DeclProof::Failed(why) = &proof {
            warnings.push(format!("declaration proof failed: {why}"));
        }
        let authorities = roster::authorities(owner, &decl);
        let mut b = store.batch();
        b.put_forum(&key.fk, &ForumRow::new(key.uri.as_str(), s.host().now_ms()))?;
        b.commit().await?;
        Ok(OpenedForum {
            key: key.clone(),
            decl,
            decl_cid: row.cid,
            authorities,
            proof,
            warnings,
        })
    }

    /// `sync.getRecord` + proofs/1 for the declaration; a proof of the fetched version raises it
    /// to level `proven`.
    async fn prove_declaration(
        &self,
        s: &Session<H>,
        key: &ForumKey,
        id: &ResolvedIdentity,
        row: &RecordRow,
    ) -> DeclProof {
        let fail = |m: String| DeclProof::Failed(m);
        let Some(pk) = id.public_key() else {
            return fail(
                id.key_problem
                    .clone()
                    .unwrap_or_else(|| "no usable signing key".into()),
            );
        };
        let Some(pds) = id.pds_url() else {
            return fail("no PDS".into());
        };
        let params = SyncGetRecord {
            did: key.owner.clone(),
            collection: key.uri.collection().clone(),
            rkey: key.uri.rkey().clone(),
        };
        let opts =
            CallOpts::capped(limits::PROOF_CAR_MAX_BYTES).with_timeout_ms(limits::CAR_TIMEOUT_MS);
        let before = s.host().now_ms();
        let resp = match s
            .xrpc()
            .query_raw(&Service::Anon(pds), &params, &opts)
            .await
        {
            Ok(r) => r,
            Err(e) => return fail(e.to_string()),
        };
        let sent_at = net::sent_at(&resp).unwrap_or(before);
        let now = s.host().now_ms();
        let outcome = match verify_proof(
            &resp.body,
            &key.owner,
            key.uri.collection(),
            key.uri.rkey(),
            &pk,
            now.saturating_mul(1000),
            &ProofCaps::default(),
        ) {
            Ok(o) => o,
            Err(f) => return fail(format!("{:?}: {}", f.error.kind(), f.error)),
        };
        let Some(rec) = &outcome.record else {
            return fail("the proof shows no declaration".into());
        };
        if rec.cid != row.cid {
            return fail(format!(
                "the proof is of version {}, the fetched declaration is {}",
                rec.cid, row.cid
            ));
        }
        let admitted = match admit::admit_block(&key.uri, &rec.cid, &rec.bytes, &LexiconValidator) {
            Ok(a) => a,
            Err(e) => return fail(e.to_string()),
        };
        let rev = outcome.rev.to_string();
        let obs = Observation::proof(&rev, sent_at, Source::Proof, now);
        let _guard = self.net().repo_lock(&key.owner).await;
        let store = RecordStore::new(s.host().cache());
        let mut b = store.batch();
        if let Err(e) = b.put_version(admitted.into_version(obs)).await {
            return fail(e.to_string());
        }
        match b.commit().await {
            Ok(_) => DeclProof::Proven { rev },
            Err(e) => fail(e.to_string()),
        }
    }

    /// `sync_forum` (§4.2, §4.5): opens the forum, syncs its authorities, then the confirmed
    /// candidates, and reports completeness.
    pub async fn sync_forum(
        &self,
        input: &str,
        opts: &SyncOpts,
    ) -> Result<ForumSync, DiscoveryError> {
        let deadline = opts
            .budget_ms
            .map(|b| self.host().now_ms().saturating_add(b));
        let stats = Arc::new(Stats::capped(opts.max_requests).with_deadline(deadline));
        let p0 = self.session(Priority::P0, Arc::clone(&stats));
        let key = self.resolve_forum_ref(&p0, input).await?;
        let forum = self.open_forum(&p0, &key).await?;
        self.sync_opened_forum(forum, opts, stats).await
    }

    /// [`Syncer::sync_forum`] for a forum already opened, counting into `stats`.
    pub async fn sync_opened_forum(
        &self,
        forum: OpenedForum,
        opts: &SyncOpts,
        stats: Arc<Stats>,
    ) -> Result<ForumSync, DiscoveryError> {
        let key = forum.key.clone();
        let fk = key.fk.as_str();
        let kv = self.host().cache();
        let p0 = self.session(Priority::P0, Arc::clone(&stats));
        let p1 = self.session(Priority::P1, Arc::clone(&stats));
        let p2 = self.session(Priority::P2, Arc::clone(&stats));
        let reason = |r: Reason| if opts.full { Reason::Full } else { r };
        let mut repos: Vec<RepoReport> = Vec::new();
        let mut done: BTreeSet<String> = BTreeSet::new();
        let tally = Tally::default();

        // 1. authorities at P0; 2. authors on screen at P0
        let a = &forum.authorities;
        let mut jobs: Vec<SyncRequest> = parse_dids(a.keys())
            .into_iter()
            .map(|d| SyncRequest::forum(d, fk, reason(Reason::Authority)))
            .collect();
        done.extend(a.keys().cloned());
        let mut disc = state::disc_rows(kv, fk).await?;
        let mut touched: BTreeSet<String> = BTreeSet::new();
        let view: Vec<Candidate> =
            parse_dids(opts.on_screen.iter().filter(|d| !a.contains_key(*d)))
                .into_iter()
                .map(|did| Candidate {
                    did,
                    src: "view".into(),
                    hinted: true,
                    granted: false,
                })
                .collect();
        jobs.extend(self.candidate_jobs(&key, &view, &mut done, &disc, reason(Reason::View)));
        let reports = self.run_jobs(&p0, &jobs, &tally, &opts.progress).await;
        self.note_candidates(&key, &view, &reports, &mut disc, &mut touched);
        repos.extend(reports);

        // the roster from the authorities' records
        let now_us = self.host().now_ms().saturating_mul(1000);
        let st = ForumState::load(kv, &key, &forum.decl, now_us).await?;
        let r = &st.roster;
        let hinted: BTreeSet<&str> = opts
            .newcomers
            .iter()
            .map(|(d, _)| d.as_str())
            .chain(opts.recently_active.iter().map(String::as_str))
            .collect();
        let mut budget = NEWCOMER_BUDGET;
        // a granted author on screen was gated by its membership at step 2 (the roster was not
        // known yet): a rejection there does not stop the ungated grant sync below
        let gated_out: BTreeSet<String> = repos
            .iter()
            .filter(|r| r.outcome == RepoOutcome::Rejected)
            .map(|r| r.did.clone())
            .collect();
        for d in &gated_out {
            if r.granted.contains(d) {
                done.remove(d);
            }
        }

        // 3. recently active authors at P1: approval mode, those in G; open mode, any (outside
        //    A ∪ G they are newcomers and count toward the budget)
        let mut active: Vec<Candidate> = Vec::new();
        for did in parse_dids(&opts.recently_active) {
            let d = did.as_str();
            if a.contains_key(d) || done.contains(d) || active.iter().any(|c| c.did == did) {
                continue;
            }
            let granted = r.granted.contains(d);
            if !granted && !r.is_open() {
                continue;
            }
            if !granted {
                if budget == 0 {
                    continue;
                }
                budget -= 1;
            }
            active.push(Candidate {
                did,
                src: if granted { "grant" } else { "hint:active" }.into(),
                hinted: true,
                granted,
            });
        }
        let jobs =
            self.candidate_jobs(&key, &active, &mut done, &disc, reason(Reason::Participant));
        let reports = self.run_jobs(&p1, &jobs, &tally, &opts.progress).await;
        self.note_candidates(&key, &active, &reports, &mut disc, &mut touched);
        repos.extend(reports);

        // 4. G ∖ A at P1 (seeds first, then banned or nullified grants)
        let mut granted: Vec<&String> = r.granted.iter().filter(|d| r.seeds.contains(*d)).collect();
        granted.extend(r.granted.iter().filter(|d| !r.seeds.contains(*d)));
        let grants: Vec<Candidate> = parse_dids(granted)
            .into_iter()
            .map(|did| Candidate {
                hinted: hinted.contains(did.as_str()),
                did,
                src: "grant".into(),
                granted: true,
            })
            .collect();
        let jobs =
            self.candidate_jobs(&key, &grants, &mut done, &disc, reason(Reason::Participant));
        let reports = self.run_jobs(&p1, &jobs, &tally, &opts.progress).await;
        self.note_candidates(&key, &grants, &reports, &mut disc, &mut touched);
        repos.extend(reports);

        if r.is_open() {
            // 5. the closure of S at P1 (S's records are synced now)
            let st = ForumState::load(kv, &key, &forum.decl, now_us).await?;
            let closure: Vec<Candidate> = parse_dids(st.closure.iter())
                .into_iter()
                .map(|did| Candidate {
                    hinted: hinted.contains(did.as_str()),
                    did,
                    src: "closure".into(),
                    granted: false,
                })
                .collect();
            let jobs = self.candidate_jobs(
                &key,
                &closure,
                &mut done,
                &disc,
                reason(Reason::Participant),
            );
            let reports = self.run_jobs(&p1, &jobs, &tally, &opts.progress).await;
            self.note_candidates(&key, &closure, &reports, &mut disc, &mut touched);
            repos.extend(reports);

            // 6. newcomers from hints at P2, most recent first, within the budget
            let mut seen = BTreeSet::new();
            let newcomers: Vec<Candidate> = opts
                .newcomers
                .iter()
                .filter(|(d, _)| !done.contains(d) && seen.insert(d.clone()))
                .filter_map(|(d, src)| {
                    Some(Candidate {
                        did: Did::parse(d).ok()?,
                        src: format!("hint:{src}"),
                        hinted: true,
                        granted: false,
                    })
                })
                .collect();
            let (now_list, later) = newcomers.split_at(newcomers.len().min(budget));
            let jobs = self.candidate_jobs(
                &key,
                now_list,
                &mut done,
                &disc,
                reason(Reason::Participant),
            );
            let reports = self.run_jobs(&p2, &jobs, &tally, &opts.progress).await;
            self.note_candidates(&key, now_list, &reports, &mut disc, &mut touched);
            repos.extend(reports);
            // the rest stay candidates
            let now = self.host().now_ms();
            for c in later {
                let row = disc
                    .entry(c.did.to_string())
                    .or_insert_with(|| DiscRow::new(&c.src, now));
                row.add_src(&c.src);
                touched.insert(c.did.to_string());
            }
        }

        let changed: BTreeMap<String, DiscRow> = touched
            .iter()
            .filter_map(|d| Some((d.clone(), disc.get(d)?.clone())))
            .collect();
        put_disc_rows(kv, fk, &changed).await?;

        // completeness
        let now_us = self.host().now_ms().saturating_mul(1000);
        let st = ForumState::load(kv, &key, &forum.decl, now_us).await?;
        let (completeness, counts) = state::completeness(kv, &key, &st).await?;
        let report = self.report(&forum, &completeness, &counts, &repos, &stats);
        Ok(ForumSync {
            forum,
            state: st,
            completeness,
            report,
            repos,
        })
    }

    /// Sync requests for the candidates not yet handled and not held back by a rejection. A
    /// granted candidate is synced without the membership gate and never held back (H1 of the
    /// M2 review: `social.md` §3.3 keeps a grant counting after its holder leaves, so its repo
    /// must stay synced, like an authority's).
    fn candidate_jobs(
        &self,
        key: &ForumKey,
        cands: &[Candidate],
        done: &mut BTreeSet<String>,
        disc: &BTreeMap<String, DiscRow>,
        reason: Reason,
    ) -> Vec<SyncRequest> {
        let now = self.host().now_ms();
        let mut out = Vec::new();
        for c in cands {
            let d = c.did.to_string();
            if done.contains(&d) {
                continue;
            }
            if c.granted {
                done.insert(d);
                out.push(
                    SyncRequest::forum(c.did.clone(), &key.fk, reason)
                        .checking(key.uri.clone(), &key.fk),
                );
                continue;
            }
            let held = reason != Reason::Full
                && !c.hinted
                && disc.get(&d).is_some_and(|r| r.skip_rejected(now));
            if held {
                continue;
            }
            done.insert(d);
            out.push(
                SyncRequest::forum(c.did.clone(), &key.fk, reason)
                    .confirming(key.uri.clone(), &key.fk),
            );
        }
        out
    }

    /// Updates discovery rows from the reports of a candidate phase.
    fn note_candidates(
        &self,
        key: &ForumKey,
        cands: &[Candidate],
        reports: &[RepoReport],
        disc: &mut BTreeMap<String, DiscRow>,
        touched: &mut BTreeSet<String>,
    ) {
        let now = self.host().now_ms();
        let by_did: BTreeMap<&str, &RepoReport> =
            reports.iter().map(|r| (r.did.as_str(), r)).collect();
        for c in cands {
            let d = c.did.to_string();
            let row = disc
                .entry(d.clone())
                .or_insert_with(|| DiscRow::new(&c.src, now));
            row.add_src(&c.src);
            touched.insert(d.clone());
            let Some(rep) = by_did.get(d.as_str()) else {
                continue;
            };
            match rep.membership.get(&key.fk) {
                Some(MembershipCheck::Valid) => row.set(DiscState::Confirmed, now),
                // a grant is never rejected: its repo is synced without the gate
                _ if c.granted => {}
                Some(m) if m.rejects() => row.set(DiscState::Rejected, now),
                _ if rep.outcome == RepoOutcome::Rejected => row.set(DiscState::Rejected, now),
                _ => {}
            }
        }
    }

    /// Runs `sync_repo` for each request, concurrently (bounded by the scheduler), reporting
    /// progress as each one finishes.
    async fn run_jobs(
        &self,
        s: &Session<H>,
        jobs: &[SyncRequest],
        tally: &Tally,
        progress: &Progress,
    ) -> Vec<RepoReport> {
        if jobs.is_empty() {
            return Vec::new();
        }
        let n = u32::try_from(jobs.len()).unwrap_or(u32::MAX);
        let total = tally.total.fetch_add(n, Ordering::SeqCst).saturating_add(n);
        progress.report(tally.done.load(Ordering::SeqCst), total);
        join_all(jobs.iter().map(|r| async move {
            let report = self.sync_repo(s, r).await;
            let done = tally.done.fetch_add(1, Ordering::SeqCst) + 1;
            progress.report(done, tally.total.load(Ordering::SeqCst));
            report
        }))
        .await
    }

    fn report(
        &self,
        forum: &OpenedForum,
        c: &Completeness,
        counts: &state::StatusCounts,
        repos: &[RepoReport],
        stats: &Stats,
    ) -> SyncReport {
        let mut errors = Vec::new();
        let mut refused: BTreeSet<&'static str> = BTreeSet::new();
        let mut push = |did: &str, host: Option<&String>, code: &str, msg: String| {
            for (needle, kind) in [
                (TIME_BUDGET_SPENT, "time_budget"),
                (REQUEST_LIMIT_REACHED, "request_limit"),
            ] {
                if msg.contains(needle) {
                    refused.insert(kind);
                    return;
                }
            }
            errors.push(SyncIssue {
                did: Some(did.to_owned()),
                host: host.cloned(),
                code: code.to_owned(),
                message: msg,
            });
        };
        for w in &forum.warnings {
            push(
                forum.key.owner.as_str(),
                None,
                if w.starts_with("declaration proof") {
                    "declaration_proof"
                } else {
                    "declaration"
                },
                w.clone(),
            );
        }
        for r in repos {
            if let Some(e) = &r.error {
                push(&r.did, r.pds.as_ref(), e.code(), e.to_string());
            }
            for (code, msg) in &r.issues {
                push(&r.did, r.pds.as_ref(), code, msg.clone());
            }
        }
        for kind in refused {
            errors.push(SyncIssue {
                did: None,
                host: None,
                code: kind.to_owned(),
                message: match kind {
                    "time_budget" => "the time budget was spent before the sync completed",
                    _ => "the request limit was reached before the sync completed",
                }
                .to_owned(),
            });
        }
        SyncReport {
            dto: DtoVersion::default(),
            forum: forum.key.uri.to_string(),
            repos_total: c.repos_total,
            repos_complete: c.repos_complete,
            repos_stale: counts.stale.len() as u32,
            repos_unavailable: counts.unavailable.len() as u32,
            requests: stats.requests(),
            bytes: stats.bytes(),
            changed: repos.iter().map(|r| r.changed as u32).sum(),
            partial: c.partial,
            degraded: c.degraded,
            errors,
        }
    }

    /// Completeness of an opened forum from the cache (for views).
    pub async fn forum_completeness(
        &self,
        forum: &OpenedForum,
    ) -> Result<(Completeness, ForumState), StoreError> {
        let kv = self.host().cache();
        let now_us = self.host().now_ms().saturating_mul(1000);
        let st = ForumState::load(kv, &forum.key, &forum.decl, now_us).await?;
        let (c, _) = state::completeness(kv, &forum.key, &st).await?;
        Ok((c, st))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xrpc::XrpcCallError;

    #[test]
    fn errors_map_to_engine_codes() {
        use crate::engine::ErrorCode;
        let e: EngineError = DiscoveryError::NotFound("at://x".into()).into();
        assert_eq!(e.code, ErrorCode::NotFound);
        let e: EngineError = DiscoveryError::Ref(ForumRefError("x".into())).into();
        assert_eq!(e.code, ErrorCode::InvalidRef);
        let e: EngineError = DiscoveryError::Invalid {
            uri: "u".into(),
            rule: "FO1".into(),
        }
        .into();
        assert_eq!(e.code, ErrorCode::InvalidInput);
        let e: EngineError = DiscoveryError::OwnerHint("did:x".into()).into();
        assert!(e.retryable);
        let e: EngineError = SyncError::Xrpc(XrpcCallError::RateLimited {
            retry_after_ms: None,
        })
        .into();
        assert_eq!(e.code, ErrorCode::RateLimited);
    }
}
