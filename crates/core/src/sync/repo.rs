// SPDX-License-Identifier: AGPL-3.0-or-later
//! `sync_repo` (`sync.md` §5.2), point fetches (§5.7), own writes (§5.8), head checks (§6.7)
//! and account status (§3.4).
//!
//! ```text
//! sync_repo(did, scopes, reason):
//!   repo lock; identity (document only, level doc, else every row is idHint); PDS
//!   h1 = getLatestCommit                                   headSeen = {h1.rev, h1.cid, sentAt}
//!     RepoNotFound | RepoDeactivated | RepoTakendown | RepoSuspended → re-fetch the document:
//!       another PDS → migration (P1); else getRepoStatus; Failed
//!   h1.rev < repo.rev → Rollback (cache kept, backoff, rollbackSince)  [sync --full accepts it]
//!   h1 = (repo.rev, repo.commit), every scope listed at it, no marks → Clean
//!   confirm (a candidate, §4.2.1): the membership getRecord first; absent or invalid → Rejected
//!   colls: repo.colls if ≤ 24 h old, not dirty, not reconcile, and no stored row lies in a
//!          collection outside it; else describeRepo ∩ (OURS ∪ {app.bsky.actor.profile})
//!          (on failure: OURS)
//!   in parallel: list_range for each (fk scope, c ∈ RANGED ∩ colls); the membership getRecord;
//!                GLOBAL getRecords; OWN full listings
//!   h2 = getLatestCommit; stable ⇔ h2 = h1
//!   one batch: admit every listed row (key (h1.rev, page sentAt), exact ⇔ stable); for stable,
//!   complete ranges, tombstone unlisted rows above the cap boundary and drop those below it;
//!   memberships (RecordNotFound → tombstone); if stable: rev, commit, listed, capped, marks;
//!   colls; host capabilities; repo row — one KvStore::write
//!   ¬stable → queue (minRev h2.rev) at P1, or P3 with the backoff after 3 unstable passes
//! ```

use std::collections::{BTreeMap, BTreeSet};

use futures::future::join_all;
use lessdugin_atp::Cid;
use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, Nsid, RecordKey};
use lessdugin_atp::xrpc::{CommitMeta, DescribeRepo, GetLatestCommit, GetRecord, GetRepoStatus};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::Membership;
use lessdugin_lexicon::{Collection, keys};
use serde_json::Value;

use crate::host::{Dir, Host};
use crate::identity::{DID_DOC_TTL_MS, IdentityLevel};
use crate::limits;
use crate::store::admit::{self, AdmittedRecord, Expect, LexiconValidator};
use crate::store::records::{Admission, Batch, Delete, DeleteEvidence, OrderKey, RecordStore, Slot};
use crate::store::repo::{AccountStatus, HostRow, RangeCursor, RepoRow};
use crate::store::{Observation, schema};
use crate::sync::list::{self, ListError, Lister, RangeListing, RangeVerdict};
use crate::sync::net::{self, Priority, url_origin};
use crate::sync::queue::{DirtyItem, Reason, ready_time};
use crate::sync::schedule;
use crate::sync::{Session, SyncError, Syncer};
use crate::xrpc::{CallOpts, Service, Xrpc, XrpcCallError, decode_json};

/// `describeRepo` results are reused this long (`DESCRIBE_REPO_TTL_MS`).
pub const DESCRIBE_REPO_TTL_MS: i64 = 86_400_000;
/// Consecutive unstable passes after which a repo is re-queued at P3 with the backoff.
pub const UNSTABLE_MAX: u32 = 3;
/// The Bluesky profile collection (the `GLOBAL` profile fallback, `data-model.md` §6.10).
pub const BSKY_PROFILE: &str = "app.bsky.actor.profile";

/// The ten record collections of this build (`OURS`).
pub fn ours() -> Vec<String> {
    Collection::ALL.iter().map(|c| c.nsid().to_owned()).collect()
}

/// The `RANGED` collections of this build.
pub fn ranged() -> Vec<String> {
    schema::RANGED_SHORT
        .iter()
        .map(|s| format!("{}.{s}", lessdugin_lexicon::ns::NS))
        .collect()
}

/// A sync scope (`sync.md` §5.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// A forum key: the `{fk}-` range of each `RANGED` collection plus the membership `{fk}`.
    Forum(String),
    /// `NS.profile/self` (falling back to `app.bsky.actor.profile/self`) and the declarations
    /// with these slugs (forums owned by the DID).
    Global {
        /// Fetch the profile.
        profile: bool,
        /// Declaration slugs.
        decls: Vec<String>,
    },
    /// The user's own repo: every collection in `OURS` listed fully.
    Own,
}

impl Scope {
    /// The `RepoRow.listed` key: the fk, `"global"` or `"own"`.
    pub fn name(&self) -> &str {
        match self {
            Self::Forum(fk) => fk,
            Self::Global { .. } => "global",
            Self::Own => "own",
        }
    }
}

/// A forum whose membership must be confirmed before a candidate is synced (§4.2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    /// The forum's canonical at-uri F.
    pub forum: AtUri,
    /// `fk(F)`.
    pub fk: String,
}

/// One `sync_repo` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRequest {
    /// The repo.
    pub did: Did,
    /// Its scopes.
    pub scopes: Vec<Scope>,
    /// Why (reconcile and `--full` refresh `describeRepo`; `--full` also skips `Clean` and
    /// accepts a rollback).
    pub reason: Reason,
    /// Confirm this membership first, and stop if it is absent or invalid.
    pub confirm: Option<Confirm>,
}

impl SyncRequest {
    /// A sync of `did` for forum `fk`.
    pub fn forum(did: Did, fk: &str, reason: Reason) -> Self {
        Self {
            did,
            scopes: vec![Scope::Forum(fk.to_owned())],
            reason,
            confirm: None,
        }
    }

    /// Confirms the membership in `forum` first.
    pub fn confirming(mut self, forum: AtUri, fk: &str) -> Self {
        self.confirm = Some(Confirm {
            forum,
            fk: fk.to_owned(),
        });
        self
    }
}

/// The state of a DID's membership record for one forum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MembershipCheck {
    /// A valid membership record of the forum.
    Valid,
    /// `RecordNotFound` (or the collection does not exist).
    Absent,
    /// The record fails validity (its first failing rule).
    Invalid(String),
    /// The record names another forum.
    WrongForum,
    /// The fetch failed.
    Unknown(String),
}

impl MembershipCheck {
    /// `true` for a confirmed rejection (absent or invalid, not a transport failure).
    pub fn rejects(&self) -> bool {
        matches!(self, Self::Absent | Self::Invalid(_) | Self::WrongForum)
    }
}

/// What `sync_repo` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoOutcome {
    /// The head and every scope were already synced.
    Clean,
    /// A stable pass was applied.
    Applied,
    /// The head moved during the listing: rows admitted, no tombstones, nothing marked listed.
    Unstable,
    /// The head is below the synced rev: nothing applied.
    Rollback,
    /// A candidate's membership is absent or invalid: not synced (§4.2.1).
    Rejected,
    /// The pass failed (see `error`).
    Failed,
}

/// The report of one `sync_repo` call.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoReport {
    /// The repo.
    pub did: String,
    /// What happened.
    pub outcome: RepoOutcome,
    /// Why it failed.
    pub error: Option<SyncError>,
    /// Partial failures (a range or fetch failed; the rest was applied): `(code, message)`.
    pub issues: Vec<(String, String)>,
    /// Stored uris whose state changed.
    pub changed: usize,
    /// The author's identity was only a hint.
    pub id_hint: bool,
    /// The PDS synced from.
    pub pds: Option<String>,
    /// Membership checks made, per fk.
    pub membership: BTreeMap<String, MembershipCheck>,
    /// Admission and listing strikes against the PDS.
    pub strikes: u32,
    /// Some request was refused locally (request cap, time budget, open breaker): the pass is
    /// incomplete but no repo backoff was applied.
    pub refused: bool,
    hard_issues: usize,
}

impl RepoReport {
    fn new(did: &Did) -> Self {
        Self {
            did: did.to_string(),
            outcome: RepoOutcome::Failed,
            error: None,
            issues: Vec::new(),
            changed: 0,
            id_hint: false,
            pds: None,
            membership: BTreeMap::new(),
            strikes: 0,
            refused: false,
            hard_issues: 0,
        }
    }

    fn issue(&mut self, e: &SyncError) {
        if e.is_refusal() {
            self.refused = true;
        } else {
            self.hard_issues += 1;
        }
        self.issues.push((e.code().to_owned(), e.to_string()));
    }
}

/// A head observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Head {
    pub(crate) rev: String,
    pub(crate) cid: Cid,
    pub(crate) sent_at: i64,
}

/// A `getRecord` answer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fetched {
    Found { rec: AdmittedRecord, sent_at: i64 },
    NotFound { sent_at: i64 },
}

/// What a point fetch did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointOutcome {
    /// A version was admitted.
    Admitted {
        /// Its CID.
        cid: Cid,
        /// It passed validity.
        valid: bool,
        /// The stored state changed.
        changed: bool,
    },
    /// `RecordNotFound`.
    NotFound {
        /// A tombstone was written or raised.
        tombstoned: bool,
    },
}

/// A hint that a point fetch may satisfy (a dirty mark, §6.5): the CID the hint reported, or
/// `None` for a delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkHint {
    /// The hinted CID (`None`: a delete).
    pub cid: Option<Cid>,
}

/// One confirmed own write (§5.8): `value` is the exact record sent (`None` for a delete), `cid`
/// the CID the PDS returned for it.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnWrite {
    /// The record uri.
    pub uri: AtUri,
    /// The value sent; `None` for a delete.
    pub value: Option<Value>,
    /// The CID the PDS returned (creates and updates).
    pub cid: Option<String>,
}

/// What [`Syncer::apply_own_writes`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnWritesApplied {
    /// The rev used as `revLo`.
    pub rev: String,
    /// Uris admitted at level `pds`, source `own`.
    pub stored: Vec<String>,
    /// Uris whose local CID differed from the returned one: queued for a P0 point fetch.
    pub refetch: Vec<String>,
    /// Uris tombstoned.
    pub tombstoned: Vec<String>,
}

fn nsid_of(s: &str) -> Result<Nsid, SyncError> {
    Nsid::parse(s).map_err(|e| SyncError::Admission(format!("bad collection {s:?}: {e}")))
}

/// `getLatestCommit` at `svc`.
pub(crate) async fn get_head<H: Host>(
    xrpc: &Xrpc<'_, H>,
    svc: &Service,
    did: &Did,
) -> Result<Head, XrpcCallError> {
    let opts = CallOpts::capped(limits::SMALL_RESPONSE_MAX_BYTES);
    let resp = xrpc
        .query_raw(svc, &GetLatestCommit { did: did.clone() }, &opts)
        .await?;
    let meta: CommitMeta = decode_json(&resp.body)?;
    if lessdugin_atp::tid::Tid::parse(&meta.rev).is_err() {
        return Err(XrpcCallError::Decode(format!(
            "getLatestCommit rev {:?} is not a TID",
            meta.rev
        )));
    }
    let cid = Cid::try_from(meta.cid.as_str())
        .map_err(|e| XrpcCallError::Decode(format!("getLatestCommit cid: {e}")))?;
    Ok(Head {
        rev: meta.rev,
        cid,
        sent_at: net::sent_at(&resp).unwrap_or_default(),
    })
}

/// `describeRepo` collections ∩ (OURS ∪ {app.bsky.actor.profile}), sorted, with the send time.
async fn describe<H: Host>(
    xrpc: &Xrpc<'_, H>,
    svc: &Service,
    did: &Did,
) -> Result<(Vec<String>, i64), XrpcCallError> {
    let opts = CallOpts::capped(limits::DEFAULT_RESPONSE_MAX_BYTES);
    let params = DescribeRepo {
        repo: AtIdentifier::Did(did.clone()),
    };
    let resp = xrpc.query_raw(svc, &params, &opts).await?;
    let v: Value = decode_json(&resp.body)?;
    let wanted: BTreeSet<String> = ours()
        .into_iter()
        .chain([BSKY_PROFILE.to_owned()])
        .collect();
    let colls: BTreeSet<String> = v
        .get("collections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|c| wanted.contains(*c))
        .map(str::to_owned)
        .collect();
    Ok((colls.into_iter().collect(), net::sent_at(&resp).unwrap_or_default()))
}

/// The membership state a fetch shows for forum `forum`.
pub(crate) fn membership_check(f: &Fetched, forum: &AtUri) -> MembershipCheck {
    match f {
        Fetched::NotFound { .. } => MembershipCheck::Absent,
        Fetched::Found { rec, .. } => {
            if let Some(rule) = &rec.validity.rule {
                return MembershipCheck::Invalid(rule.clone());
            }
            match serde_json::from_value::<Membership>(rec.value.clone()) {
                Ok(m) if m.forum == *forum => MembershipCheck::Valid,
                Ok(_) => MembershipCheck::WrongForum,
                Err(e) => MembershipCheck::Invalid(format!("membership: {e}")),
            }
        }
    }
}

/// The membership state the store shows for `c` (exact for a repo listed at its head).
pub(crate) async fn stored_membership<K: crate::host::KvStore>(
    store: &RecordStore<'_, K>,
    did: &Did,
    c: &Confirm,
) -> Result<MembershipCheck, SyncError> {
    let k = schema::RecKey::new(nsid::MEMBERSHIP, did.as_str(), &c.fk);
    Ok(match store.slot(&k).await? {
        Slot::Rec(row) => {
            if !row.valid {
                return Ok(MembershipCheck::Invalid(row.rule.unwrap_or_default()));
            }
            match row
                .value_json()
                .ok()
                .and_then(|v| serde_json::from_value::<Membership>(v).ok())
            {
                Some(m) if m.forum == c.forum => MembershipCheck::Valid,
                Some(_) => MembershipCheck::WrongForum,
                None => MembershipCheck::Invalid("membership: undecodable".into()),
            }
        }
        _ => MembershipCheck::Absent,
    })
}

fn account_status_of(active: bool, status: Option<&str>) -> AccountStatus {
    if active {
        return AccountStatus::Active;
    }
    match status {
        None | Some("takendown" | "suspended" | "deleted" | "deactivated") => {
            AccountStatus::Inactive
        }
        // desynchronized, throttled and anything unknown: failing, not inactive
        Some(_) => AccountStatus::Temporary,
    }
}

struct ScopeState {
    complete: bool,
    byte_capped: bool,
    capped: BTreeMap<String, String>,
}

impl<H: Host> Syncer<H> {
    /// `sync_repo` (§5.2). Never panics; failures are reported in the [`RepoReport`].
    pub async fn sync_repo(&self, s: &Session<H>, req: &SyncRequest) -> RepoReport {
        let _guard = self.net().repo_lock(&req.did).await;
        let mut rep = RepoReport::new(&req.did);
        match self.sync_repo_locked(s, req, &mut rep).await {
            Ok(outcome) => rep.outcome = outcome,
            Err(e) => {
                rep.outcome = RepoOutcome::Failed;
                rep.refused |= e.is_refusal();
                rep.error = Some(e);
            }
        }
        rep
    }

    /// The PDS service of `did` from its document (level `doc`, or a hint).
    pub(crate) async fn pds_of(
        &self,
        s: &Session<H>,
        did: &Did,
    ) -> Result<(Service, String, bool), SyncError> {
        let id = s.resolver().resolve_did_doc(did, DID_DOC_TTL_MS).await?;
        let pds = id.pds_url().ok_or_else(|| SyncError::NoPds {
            did: did.to_string(),
            why: id.pds_problem.clone().unwrap_or_else(|| "no PDS".into()),
        })?;
        let id_hint = id.level == IdentityLevel::Hint;
        Ok((Service::Anon(pds), id.pds.unwrap_or_default(), id_hint))
    }

    async fn load_host_row(
        &self,
        store: &RecordStore<'_, H::Kv>,
        origin: &str,
    ) -> Result<HostRow, SyncError> {
        let row = store.host_caps(origin).await?.unwrap_or_default();
        self.net()
            .seed_host(origin, &row.breaker, row.budget.as_ref());
        Ok(row)
    }

    fn snapshot_host(&self, origin: &str, row: &mut HostRow) {
        let (breaker, budget) = self.net().host_snapshot(origin);
        row.breaker = breaker;
        row.budget = budget;
    }

    /// `getRecord(did, coll, rkey)` at `svc`, admitted (§5.5). An admission failure is a strike
    /// against the host.
    pub(crate) async fn fetch(
        &self,
        s: &Session<H>,
        svc: &Service,
        did: &Did,
        coll: &str,
        rkey: &str,
    ) -> Result<Fetched, SyncError> {
        let params = GetRecord {
            repo: AtIdentifier::Did(did.clone()),
            collection: nsid_of(coll)?,
            rkey: RecordKey::parse(rkey)
                .map_err(|e| SyncError::Admission(format!("bad rkey {rkey:?}: {e}")))?,
            cid: None,
        };
        let opts = CallOpts::capped(limits::RECORD_RESPONSE_MAX_BYTES);
        let before = s.host().now_ms();
        match s.xrpc().query_raw(svc, &params, &opts).await {
            Ok(resp) => {
                let sent_at = net::sent_at(&resp).unwrap_or(before);
                let expect = Expect {
                    did,
                    collection: coll,
                    rkey: Some(rkey),
                };
                match admit::admit_get_record(&resp.body, &expect, &LexiconValidator) {
                    Ok(rec) => Ok(Fetched::Found { rec, sent_at }),
                    // a strike against the host: counted by the caller
                    Err(e) => Err(SyncError::Admission(e.to_string())),
                }
            }
            Err(XrpcCallError::RecordNotFound) => Ok(Fetched::NotFound { sent_at: before }),
            Err(e) => Err(e.into()),
        }
    }

    /// Applies a point-fetched answer (key `(head_rev, sentAt)`, not exact). A `RecordNotFound`
    /// tombstones the uri only if `tomb_always` or a version is stored. Returns whether the
    /// stored state changed and the admission (for conflicts).
    #[allow(clippy::too_many_arguments)]
    async fn apply_fetched(
        &self,
        batch: &mut Batch<'_, H::Kv>,
        did: &Did,
        coll: &str,
        rkey: &str,
        f: Fetched,
        head_rev: &str,
        id_hint: bool,
        now: i64,
        tomb_always: bool,
    ) -> Result<(bool, Option<Admission>), SyncError> {
        match f {
            Fetched::Found { rec, sent_at } => {
                let obs = self.observation(
                    Observation::point_fetch(head_rev, sent_at, now).with_id_hint(id_hint),
                    &rec,
                );
                let r = batch.put_version(rec.into_version(obs)).await?;
                Ok((r.changed, Some(r.admission)))
            }
            Fetched::NotFound { sent_at } => {
                let k = schema::RecKey::new(coll, did.as_str(), rkey);
                if !tomb_always && !matches!(batch.slot(&k).await?, Slot::Rec(_)) {
                    return Ok((false, None));
                }
                let d = Delete {
                    key: OrderKey::new(head_rev, sent_at),
                    evidence: DeleteEvidence::RecordNotFound,
                    at_ms: now,
                };
                Ok((batch.tombstone_key(&k, d).await?.changed, None))
            }
        }
    }

    /// An observation with any hint first-seen time of the version applied (§5.6.5).
    fn observation(&self, obs: Observation, rec: &AdmittedRecord) -> Observation {
        match self.hint_first_seen(rec.uri.as_str(), &rec.cid.to_string()) {
            Some(t) => obs.with_first_seen(t),
            None => obs,
        }
    }

    fn note_conflict(&self, did: &Did, uri: &str, a: &Admission, now: i64) {
        if let Admission::Conflict(c) = a {
            tracing::warn!(
                did = %did, uri, rev = %c.rev, stored = %c.stored.0, observed = %c.observed.0,
                stored_src = %c.stored.1.as_string(), observed_src = %c.observed.1.as_string(),
                "same exact rev with two CIDs (I3)"
            );
            let at = ready_time(Priority::P1, now, self.random(), None);
            self.enqueue(DirtyItem::new(did.as_str(), Priority::P1, Reason::Hint, at));
        }
    }

    async fn write_repo_only(
        &self,
        store: &RecordStore<'_, H::Kv>,
        did: &Did,
        repo: &RepoRow,
        origin: &str,
        host_row: &mut HostRow,
    ) -> Result<(), SyncError> {
        self.snapshot_host(origin, host_row);
        let mut b = store.batch();
        b.put_repo(did.as_str(), repo)?;
        b.put_host_caps(origin, host_row)?;
        b.commit().await?;
        Ok(())
    }

    /// Handles `RepoNotFound`, `RepoDeactivated`, `RepoTakendown`, `RepoSuspended` (§3.3): the
    /// document is fetched again; a new PDS is a migration (queued at P1), otherwise the account
    /// status is checked.
    async fn on_repo_status_error(&self, s: &Session<H>, did: &Did, repo: &mut RepoRow) {
        s.resolver().invalidate_did(did).await;
        let now = s.host().now_ms();
        match s.resolver().resolve_did_doc(did, 0).await {
            Ok(id) => match id.pds.as_deref() {
                Some(p) if p != repo.pds => {
                    repo.pds = p.to_owned();
                    let at = ready_time(Priority::P1, now, self.random(), None);
                    self.enqueue(DirtyItem::new(
                        did.as_str(),
                        Priority::P1,
                        Reason::Migration,
                        at,
                    ));
                }
                Some(p) => {
                    let svc = match url::Url::parse(p) {
                        Ok(u) => Service::Anon(u),
                        Err(_) => return,
                    };
                    if let Ok((status, detail)) = self.repo_status_at(s, &svc, did).await {
                        repo.status = status;
                        repo.status_detail = detail;
                        repo.status_at = now;
                    }
                }
                None => {}
            },
            Err(e) => tracing::debug!(did = %did, error = %e, "identity re-fetch failed"),
        }
    }

    async fn repo_status_at(
        &self,
        s: &Session<H>,
        svc: &Service,
        did: &Did,
    ) -> Result<(AccountStatus, Option<String>), SyncError> {
        let opts = CallOpts::capped(limits::SMALL_RESPONSE_MAX_BYTES);
        match s
            .xrpc()
            .query_endpoint(svc, &GetRepoStatus { did: did.clone() }, &opts)
            .await
        {
            Ok(out) => {
                let st = account_status_of(out.active, out.status.as_deref());
                let detail = (!out.active).then(|| out.status.unwrap_or_default());
                Ok((st, detail))
            }
            Err(XrpcCallError::RepoNotFound) => {
                Ok((AccountStatus::Inactive, Some("deleted".to_owned())))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Checks the account status of `did` at its PDS (`getRepoStatus`, §3.4) and stores it.
    pub async fn check_account_status(
        &self,
        s: &Session<H>,
        did: &Did,
    ) -> Result<AccountStatus, SyncError> {
        let _guard = self.net().repo_lock(did).await;
        let (svc, pds, _) = self.pds_of(s, did).await?;
        let (status, detail) = self.repo_status_at(s, &svc, did).await?;
        let store = RecordStore::new(s.host().cache());
        let mut repo = store
            .repo(did.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        repo.status = status;
        repo.status_detail = detail;
        repo.status_at = s.host().now_ms();
        let mut b = store.batch();
        b.put_repo(did.as_str(), &repo)?;
        b.commit().await?;
        Ok(status)
    }

    #[allow(clippy::too_many_lines)]
    async fn sync_repo_locked(
        &self,
        s: &Session<H>,
        req: &SyncRequest,
        rep: &mut RepoReport,
    ) -> Result<RepoOutcome, SyncError> {
        let host = s.host();
        let now = host.now_ms();
        let did = &req.did;
        let store = RecordStore::new(host.cache());
        let full = req.reason == Reason::Full;

        // identity and PDS
        let (svc, pds, id_hint) = self.pds_of(s, did).await?;
        rep.id_hint = id_hint;
        rep.pds = Some(pds.clone());
        let origin = service_origin(&svc);
        let mut repo = store
            .repo(did.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        // a migration keeps the cached rows (§3.3)
        repo.pds.clone_from(&pds);
        let mut host_row = self.load_host_row(&store, &origin).await?;
        let xrpc = s.xrpc();

        // h1
        let h1 = match get_head(&xrpc, &svc, did).await {
            Ok(h) => h,
            Err(e) => {
                if matches!(e, XrpcCallError::Blocked(_)) {
                    // refused locally: not the repo's fault
                    return Err(e.into());
                }
                if e.is_repo_status() {
                    self.on_repo_status_error(s, did, &mut repo).await;
                }
                repo.fail(now, self.random());
                self.write_repo_only(&store, did, &repo, &origin, &mut host_row)
                    .await?;
                return Err(e.into());
            }
        };
        repo.observe_head(&h1.rev, h1.cid, h1.sent_at);
        if repo.status != AccountStatus::Active && repo.status_detail.is_some() {
            // the PDS answers for the repo again: it is active (a status check confirms later)
            repo.status = AccountStatus::Active;
            repo.status_detail = None;
        }

        // rollback (§5.9)
        if !repo.rev.is_empty() && h1.rev < repo.rev {
            if !full {
                repo.rollback_since.get_or_insert(now);
                repo.fail(now, self.random());
                self.write_repo_only(&store, did, &repo, &origin, &mut host_row)
                    .await?;
                return Ok(RepoOutcome::Rollback);
            }
            // `sync --full` accepts the PDS state: the PDS is the database
            repo.rev.clear();
            repo.commit = None;
            repo.rollback_since = None;
        }
        if full {
            for sc in &req.scopes {
                repo.reset_scope(sc.name());
            }
        }

        // Clean
        let clean = !full
            && repo.rev == h1.rev
            && repo.commit == Some(h1.cid)
            && req
                .scopes
                .iter()
                .all(|sc| repo.listed_rev(sc.name()) == repo.rev)
            && repo.marks.is_empty()
            && !repo.marks_overflow;
        if clean {
            // the listing at this rev covered the membership: a confirmation is served from the
            // cache (§4.2.1)
            let mut outcome = RepoOutcome::Clean;
            if let Some(c) = &req.confirm {
                let check = stored_membership(&store, did, c).await?;
                if check.rejects() {
                    outcome = RepoOutcome::Rejected;
                }
                rep.membership.insert(c.fk.clone(), check);
            }
            repo.succeed(now);
            if req.reason == Reason::Reconcile {
                repo.last_reconcile = now;
            }
            self.write_repo_only(&store, did, &repo, &origin, &mut host_row)
                .await?;
            return Ok(outcome);
        }

        // confirm a candidate (§4.2.1): the membership first
        let mut pre_member: Option<(String, Fetched)> = None;
        if let Some(c) = &req.confirm {
            let fetched = match self.fetch(s, &svc, did, nsid::MEMBERSHIP, &c.fk).await {
                Ok(f) => f,
                Err(e) => {
                    if matches!(e, SyncError::Admission(_)) {
                        host_row.strike();
                        self.net().strike(&origin, now);
                        rep.strikes += 1;
                    }
                    rep.membership
                        .insert(c.fk.clone(), MembershipCheck::Unknown(e.to_string()));
                    if !e.is_refusal() {
                        repo.fail(now, self.random());
                    }
                    self.write_repo_only(&store, did, &repo, &origin, &mut host_row)
                        .await?;
                    return Err(e);
                }
            };
            let check = membership_check(&fetched, &c.forum);
            rep.membership.insert(c.fk.clone(), check.clone());
            if check.rejects() {
                let mut batch = store.batch();
                let (changed, adm) = self
                    .apply_fetched(
                        &mut batch,
                        did,
                        nsid::MEMBERSHIP,
                        &c.fk,
                        fetched,
                        &h1.rev,
                        id_hint,
                        now,
                        false,
                    )
                    .await?;
                if let Some(a) = &adm {
                    self.note_conflict(did, &format!("at://{did}/{}/{}", nsid::MEMBERSHIP, c.fk), a, now);
                }
                repo.succeed(now);
                if changed {
                    repo.last_change_at = now;
                }
                rep.changed = batch.changed().len();
                self.snapshot_host(&origin, &mut host_row);
                batch.put_repo(did.as_str(), &repo)?;
                batch.put_host_caps(&origin, &host_row)?;
                batch.commit().await?;
                return Ok(RepoOutcome::Rejected);
            }
            pre_member = Some((c.fk.clone(), fetched));
        }

        // collections
        let fk_scopes: Vec<&str> = req
            .scopes
            .iter()
            .filter_map(|sc| match sc {
                Scope::Forum(fk) => Some(fk.as_str()),
                _ => None,
            })
            .collect();
        let ranged_all = ranged();
        let mut need_describe = matches!(
            req.reason,
            Reason::Reconcile | Reason::Full | Reason::Resync | Reason::Migration
        ) || repo.colls_at <= 0
            || now.saturating_sub(repo.colls_at) > DESCRIBE_REPO_TTL_MS
            || repo.colls_dirty;
        if !need_describe {
            'outer: for fk in &fk_scopes {
                for c in ranged_all.iter().chain([nsid::MEMBERSHIP.to_owned()].iter()) {
                    if repo.colls.contains(c) {
                        continue;
                    }
                    let found = store
                        .scan_repo_range(did.as_str(), c, fk, Dir::Forward)
                        .with_page_size(1)
                        .take(1)
                        .await?;
                    if !found.is_empty() {
                        need_describe = true;
                        break 'outer;
                    }
                }
            }
        }
        let mut described: Option<i64> = None;
        let colls: Vec<String> = if need_describe {
            match describe(&xrpc, &svc, did).await {
                Ok((cs, at)) => {
                    described = Some(at);
                    cs
                }
                Err(e) => {
                    rep.issue(&SyncError::Xrpc(e));
                    ours()
                }
            }
        } else {
            repo.colls.clone()
        };
        let has = |c: &str| colls.iter().any(|x| x == c);

        // listings, memberships, global records, own listings: in parallel
        if host_row.range_cursor == RangeCursor::No && host_row.use_range(now) {
            // the 7-day re-probe: a clean probe may set Yes again
            host_row.range_cursor = RangeCursor::Unknown;
        }
        let use_range = host_row.use_range(now);
        let short_page_ends = host_row.short_page_ends;
        let tuning = self.config().tuning.clone();
        let lister = Lister {
            xrpc: &xrpc,
            service: &svc,
            origin: &origin,
            did,
            validator: &LexiconValidator,
            tuning: &tuning,
        };
        let mut range_jobs: Vec<(String, String)> = Vec::new();
        for fk in &fk_scopes {
            for c in &ranged_all {
                if has(c) {
                    range_jobs.push(((*fk).to_owned(), c.clone()));
                }
            }
        }
        let list_futs = range_jobs.iter().map(|(fk, c)| {
            let lister = &lister;
            async move {
                let coll = nsid_of(c)?;
                Ok::<RangeListing, SyncError>(
                    lister
                        .list_range(&coll, fk, use_range, short_page_ends)
                        .await?,
                )
            }
        });
        // memberships of fk scopes (not fetched by the confirmation)
        let mut mem_jobs: Vec<String> = Vec::new();
        for fk in &fk_scopes {
            if pre_member.as_ref().is_some_and(|(f, _)| f == fk) {
                continue;
            }
            let stored = !matches!(
                store
                    .slot(&schema::RecKey::new(nsid::MEMBERSHIP, did.as_str(), fk))
                    .await?,
                Slot::Empty
            );
            if has(nsid::MEMBERSHIP) || stored {
                mem_jobs.push((*fk).to_owned());
            }
        }
        let mem_futs = mem_jobs
            .iter()
            .map(|fk| self.fetch(s, &svc, did, nsid::MEMBERSHIP, fk));
        // GLOBAL
        let mut global_jobs: Vec<(String, String)> = Vec::new();
        for sc in &req.scopes {
            if let Scope::Global { profile, decls } = sc {
                if *profile {
                    if has(nsid::PROFILE) {
                        global_jobs.push((nsid::PROFILE.to_owned(), keys::PROFILE_RKEY.into()));
                    } else if has(BSKY_PROFILE) {
                        global_jobs.push((BSKY_PROFILE.to_owned(), keys::PROFILE_RKEY.into()));
                    }
                }
                for slug in decls {
                    global_jobs.push((nsid::FORUM.to_owned(), slug.clone()));
                }
            }
        }
        let global_futs = global_jobs
            .iter()
            .map(|(c, k)| self.fetch(s, &svc, did, c, k));
        // OWN
        let own = req.scopes.iter().any(|sc| *sc == Scope::Own);
        let own_jobs: Vec<String> = if own {
            ours().into_iter().filter(|c| has(c)).collect()
        } else {
            Vec::new()
        };
        let own_futs = own_jobs.iter().map(|c| {
            let lister = &lister;
            async move {
                let coll = nsid_of(c)?;
                Ok::<RangeListing, SyncError>(lister.list_all(&coll).await?)
            }
        });
        let (lists, mems, globals, owns) = futures::join!(
            join_all(list_futs),
            join_all(mem_futs),
            join_all(global_futs),
            join_all(own_futs)
        );

        // h2: the head re-check before the atomic write
        let (stable, h2) = match get_head(&xrpc, &svc, did).await {
            Ok(h2) => {
                repo.observe_head(&h2.rev, h2.cid, h2.sent_at);
                (h2.rev == h1.rev && h2.cid == h1.cid, Some(h2))
            }
            Err(e) => {
                rep.issue(&SyncError::Xrpc(e));
                (false, None)
            }
        };

        // one batch
        let now = host.now_ms();
        let mut batch = store.batch();
        let mut scopes: BTreeMap<String, ScopeState> = req
            .scopes
            .iter()
            .map(|sc| {
                (
                    sc.name().to_owned(),
                    ScopeState {
                        complete: true,
                        byte_capped: false,
                        capped: BTreeMap::new(),
                    },
                )
            })
            .collect();
        let mut verdicts: Vec<RangeVerdict> = Vec::new();
        let mut strikes = 0u32;
        let mut conflicts: Vec<(String, Admission)> = Vec::new();
        let any_hints = !self.shared.hints().is_empty();

        for ((fk, c), res) in range_jobs.iter().zip(lists) {
            let st = scopes.get_mut(fk.as_str()).expect("scope of a range job");
            match res {
                Ok(l) => {
                    verdicts.extend(l.verdict);
                    strikes += l.strikes;
                    let mut versions = Vec::new();
                    for (rec, sent_at) in l.admitted() {
                        let obs = Observation::listing(&h1.rev, sent_at, stable, now)
                            .with_id_hint(id_hint);
                        let obs = if any_hints {
                            self.observation(obs, rec)
                        } else {
                            obs
                        };
                        versions.push(rec.clone().into_version(obs));
                    }
                    let uris: Vec<String> =
                        versions.iter().map(|v| v.uri.to_string()).collect();
                    for (u, r) in uris.into_iter().zip(batch.put_versions(versions).await?) {
                        if matches!(r.admission, Admission::Conflict(_)) {
                            conflicts.push((u, r.admission));
                        }
                    }
                    if stable && l.complete {
                        let key = OrderKey::new(&h1.rev, l.first_sent_at.unwrap_or(h1.sent_at));
                        batch
                            .reconcile_range(
                                did.as_str(),
                                c,
                                fk,
                                &l.keys(),
                                &key,
                                l.boundary(),
                                now,
                            )
                            .await?;
                    }
                    if !l.complete {
                        st.complete = false;
                    }
                    if l.byte_capped {
                        st.byte_capped = true;
                    }
                    if let Some(b) = l.boundary() {
                        st.capped.insert(c.clone(), b.to_owned());
                    }
                }
                Err(e) => {
                    if matches!(e, SyncError::List(ListError::BadPage(_) | ListError::Duplicate(_))) {
                        strikes += 1;
                    }
                    st.complete = false;
                    rep.issue(&e);
                }
            }
        }
        // collections that do not exist (a fresh describeRepo): their stored rows are gone
        if stable && let Some(at) = described {
            let key = OrderKey::new(&h1.rev, at);
            for fk in &fk_scopes {
                for c in &ranged_all {
                    if !has(c) {
                        batch
                            .reconcile_range(
                                did.as_str(),
                                c,
                                fk,
                                &BTreeSet::new(),
                                &key,
                                None,
                                now,
                            )
                            .await?;
                    }
                }
            }
        }
        // memberships
        let mut mem_results: Vec<(String, Result<Fetched, SyncError>)> =
            mem_jobs.into_iter().zip(mems).collect();
        if let Some((fk, f)) = pre_member {
            mem_results.push((fk, Ok(f)));
        }
        let forum_of_fk: BTreeMap<&str, &AtUri> = req
            .confirm
            .iter()
            .map(|c| (c.fk.as_str(), &c.forum))
            .collect();
        for (fk, res) in mem_results {
            match res {
                Ok(f) => {
                    if let Some(forum) = forum_of_fk.get(fk.as_str()) {
                        rep.membership.insert(fk.clone(), membership_check(&f, forum));
                    } else if matches!(f, Fetched::NotFound { .. }) {
                        rep.membership.insert(fk.clone(), MembershipCheck::Absent);
                    }
                    let (_, adm) = self
                        .apply_fetched(
                            &mut batch,
                            did,
                            nsid::MEMBERSHIP,
                            &fk,
                            f,
                            &h1.rev,
                            id_hint,
                            now,
                            false,
                        )
                        .await?;
                    if let Some(a) = adm {
                        conflicts.push((format!("at://{did}/{}/{fk}", nsid::MEMBERSHIP), a));
                    }
                }
                Err(e) => {
                    if matches!(e, SyncError::Admission(_)) {
                        strikes += 1;
                    }
                    rep.membership
                        .insert(fk.clone(), MembershipCheck::Unknown(e.to_string()));
                    if let Some(st) = scopes.get_mut(fk.as_str()) {
                        st.complete = false;
                    }
                    rep.issue(&e);
                }
            }
        }
        // GLOBAL
        for ((c, k), res) in global_jobs.iter().zip(globals) {
            match res {
                Ok(f) => {
                    let (_, adm) = self
                        .apply_fetched(&mut batch, did, c, k, f, &h1.rev, id_hint, now, false)
                        .await?;
                    if let Some(a) = adm {
                        conflicts.push((format!("at://{did}/{c}/{k}"), a));
                    }
                }
                Err(e) => {
                    if matches!(e, SyncError::Admission(_)) {
                        strikes += 1;
                    }
                    if let Some(st) = scopes.get_mut("global") {
                        st.complete = false;
                    }
                    rep.issue(&e);
                }
            }
        }
        // OWN: admitted; deletes reach the store through own-write tombstones and the forum
        // scopes' listings
        for (_, res) in own_jobs.iter().zip(owns) {
            match res {
                Ok(l) => {
                    strikes += l.strikes;
                    let versions: Vec<_> = l
                        .admitted()
                        .map(|(rec, sent_at)| {
                            let obs = Observation::listing(&h1.rev, sent_at, stable, now)
                                .with_id_hint(id_hint);
                            rec.clone().into_version(obs)
                        })
                        .collect();
                    batch.put_versions(versions).await?;
                    if !l.complete {
                        if let Some(st) = scopes.get_mut("own") {
                            st.complete = false;
                            st.byte_capped |= l.byte_capped;
                        }
                    }
                }
                Err(e) => {
                    if let Some(st) = scopes.get_mut("own") {
                        st.complete = false;
                    }
                    rep.issue(&e);
                }
            }
        }

        // repo state
        if stable {
            let done: Vec<(&str, BTreeMap<String, String>)> = scopes
                .iter()
                .filter(|(_, st)| st.complete && !st.byte_capped)
                .map(|(name, st)| (name.as_str(), st.capped.clone()))
                .collect();
            repo.apply_stable_pass(&h1.rev, h1.cid, done);
            for (name, st) in &scopes {
                if st.byte_capped {
                    repo.set_byte_capped(name, true);
                }
            }
        } else {
            repo.unstable = repo.unstable.saturating_add(1);
        }
        if let Some(at) = described {
            repo.colls = colls.clone();
            repo.colls_at = at;
            repo.colls_dirty = false;
        } else if need_describe {
            // describeRepo failed: OURS was listed; it is asked again next time
            repo.colls = colls.clone();
        }
        let failed = rep.hard_issues > 0;
        if failed || (!stable && repo.unstable >= UNSTABLE_MAX) {
            repo.fail(now, self.random());
        } else if rep.refused {
            // incomplete through a local refusal: neither a failure nor a success
            repo.checked_at = now;
        } else {
            repo.succeed(now);
            if req.reason == Reason::Reconcile {
                repo.last_reconcile = now;
            }
        }
        list::apply_verdicts(&mut host_row, &verdicts, now);
        if strikes > 0 {
            rep.strikes += strikes;
            for _ in 0..strikes {
                host_row.strike();
                self.net().strike(&origin, now);
            }
        }
        self.snapshot_host(&origin, &mut host_row);
        let changed = batch.changed().len();
        if changed > 0 {
            repo.last_change_at = now;
        }
        batch.put_repo(did.as_str(), &repo)?;
        batch.put_host_caps(&origin, &host_row)?;
        batch.commit().await?;
        rep.changed = changed;
        for (u, a) in &conflicts {
            self.note_conflict(did, u, a, now);
        }

        if !stable {
            let item = if repo.unstable >= UNSTABLE_MAX {
                DirtyItem::new(did.as_str(), Priority::P3, Reason::Unstable, repo.next_check)
            } else {
                let at = ready_time(Priority::P1, now, self.random(), None);
                DirtyItem::new(did.as_str(), Priority::P1, Reason::Unstable, at)
            };
            let item = match &h2 {
                Some(h) => item.with_min_rev(h.rev.clone()),
                None => item,
            };
            self.enqueue(item);
            return Ok(RepoOutcome::Unstable);
        }
        Ok(RepoOutcome::Applied)
    }

    /// A point fetch (§5.7): `getRecord(uri)` at the author's PDS, admitted at level `pds` with
    /// key `(repo.headSeen.rev, sentAt)`; `RecordNotFound` tombstones it. A matching `hint`
    /// satisfies the uri's dirty mark (§6.5).
    pub async fn point_fetch(
        &self,
        s: &Session<H>,
        uri: &AtUri,
        hint: Option<&MarkHint>,
    ) -> Result<PointOutcome, SyncError> {
        let did = uri.did();
        let _guard = self.net().repo_lock(did).await;
        let (svc, pds, id_hint) = self.pds_of(s, did).await?;
        let store = RecordStore::new(s.host().cache());
        let mut repo = store
            .repo(did.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        repo.pds.clone_from(&pds);
        let head_rev = repo
            .head_seen
            .as_ref()
            .map(|h| h.rev.clone())
            .unwrap_or_default();
        let coll = uri.collection().as_str();
        let rkey = uri.rkey().as_str();
        let fetched = match self.fetch(s, &svc, did, coll, rkey).await {
            Ok(f) => f,
            Err(e) => {
                if matches!(e, SyncError::Admission(_)) {
                    self.net()
                        .strike(&service_origin(&svc), s.host().now_ms());
                }
                if let SyncError::Xrpc(x) = &e
                    && x.is_repo_status()
                {
                    self.on_repo_status_error(s, did, &mut repo).await;
                    let mut b = store.batch();
                    b.put_repo(did.as_str(), &repo)?;
                    b.commit().await?;
                }
                return Err(e);
            }
        };
        let now = s.host().now_ms();
        let mut batch = store.batch();
        let found = match &fetched {
            Fetched::Found { rec, .. } => {
                let ok_mark = hint.is_some_and(|h| h.cid.as_ref() == Some(&rec.cid));
                if ok_mark {
                    repo.marks.remove(uri.as_str());
                }
                if ours().iter().any(|c| c == coll) && !repo.colls.iter().any(|c| c == coll) {
                    repo.colls_dirty = true;
                }
                Some((rec.cid, rec.validity.is_valid()))
            }
            Fetched::NotFound { .. } => {
                if hint.is_some_and(|h| h.cid.is_none()) {
                    repo.marks.remove(uri.as_str());
                }
                None
            }
        };
        let (changed, adm) = self
            .apply_fetched(&mut batch, did, coll, rkey, fetched, &head_rev, id_hint, now, true)
            .await?;
        if changed {
            repo.last_change_at = now;
        }
        batch.put_repo(did.as_str(), &repo)?;
        batch.commit().await?;
        if let Some(a) = &adm {
            self.note_conflict(did, uri.as_str(), a, now);
        }
        Ok(match found {
            Some((cid, valid)) => PointOutcome::Admitted {
                cid,
                valid,
                changed,
            },
            None => PointOutcome::NotFound {
                tombstoned: changed,
            },
        })
    }

    /// A head check (a poll, §6.7): `getLatestCommit`, stored as `headSeen`. Returns the priority
    /// at which the repo was queued if its head differs from the synced one.
    pub async fn head_check(
        &self,
        s: &Session<H>,
        did: &Did,
        authority: bool,
    ) -> Result<Option<Priority>, SyncError> {
        let _guard = self.net().repo_lock(did).await;
        let (svc, pds, _) = self.pds_of(s, did).await?;
        let store = RecordStore::new(s.host().cache());
        let mut repo = store
            .repo(did.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        let now = s.host().now_ms();
        let h = match get_head(&s.xrpc(), &svc, did).await {
            Ok(h) => h,
            Err(e) => {
                if e.is_repo_status() {
                    self.on_repo_status_error(s, did, &mut repo).await;
                }
                repo.checked_at = now;
                let mut b = store.batch();
                b.put_repo(did.as_str(), &repo)?;
                b.commit().await?;
                return Err(e.into());
            }
        };
        repo.observe_head(&h.rev, h.cid, h.sent_at);
        repo.checked_at = now;
        let class = repo.class_at(now, authority);
        let follow = schedule::poll_followup(&repo, class);
        let mut b = store.batch();
        b.put_repo(did.as_str(), &repo)?;
        b.commit().await?;
        if let Some(p) = follow {
            let at = ready_time(p, now, self.random(), None);
            self.enqueue(DirtyItem::new(did.as_str(), p, Reason::Poll, at).with_min_rev(h.rev));
        }
        Ok(follow)
    }

    /// Applies a successful write's results (§5.8): each created or updated record is admitted
    /// with the exact value sent (level `pds`, source `own`, key `(commit.rev, sentAt)`) if its
    /// CID equals the returned one (otherwise it gets a P0 point fetch); each delete gets a
    /// tombstone. Without a `commit` (a no-op `putRecord`), a `getLatestCommit` gives the rev.
    /// `headSeen` is updated, `rev` and `listed` are not; the repo is queued at P1 so a listing
    /// confirms the write; a write into a collection outside `repo.colls` sets `collsDirty`.
    pub async fn apply_own_writes(
        &self,
        s: &Session<H>,
        account: &Did,
        sent_at: i64,
        commit: Option<&CommitMeta>,
        writes: &[OwnWrite],
    ) -> Result<OwnWritesApplied, SyncError> {
        let _guard = self.net().repo_lock(account).await;
        let (svc, pds, _) = self.pds_of(s, account).await?;
        let store = RecordStore::new(s.host().cache());
        let mut repo = store
            .repo(account.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        let (rev, head_sent_at) = match commit {
            Some(c) => {
                if let Ok(cid) = Cid::try_from(c.cid.as_str()) {
                    repo.observe_head(&c.rev, cid, sent_at);
                }
                (c.rev.clone(), sent_at)
            }
            None => {
                let h = get_head(&s.xrpc(), &svc, account).await?;
                repo.observe_head(&h.rev, h.cid, h.sent_at);
                (h.rev, sent_at)
            }
        };
        let now = s.host().now_ms();
        let mut out = OwnWritesApplied {
            rev: rev.clone(),
            ..OwnWritesApplied::default()
        };
        let mut batch = store.batch();
        for w in writes {
            if w.uri.did() != account {
                continue;
            }
            let coll = w.uri.collection().as_str();
            if ours().iter().any(|c| c == coll) && !repo.colls.iter().any(|c| c == coll) {
                repo.colls_dirty = true;
            }
            match (&w.value, &w.cid) {
                (Some(v), Some(cid)) => {
                    match admit::admit_own_write(&w.uri, v, cid, &LexiconValidator) {
                        Ok(rec) => {
                            let obs = self.observation(
                                Observation::own_write(&rev, head_sent_at, now),
                                &rec,
                            );
                            batch.put_version(rec.into_version(obs)).await?;
                            out.stored.push(w.uri.to_string());
                        }
                        Err(e) => {
                            tracing::debug!(uri = %w.uri, error = %e, "own write not stored");
                            out.refetch.push(w.uri.to_string());
                        }
                    }
                }
                (Some(_), None) => out.refetch.push(w.uri.to_string()),
                (None, _) => {
                    let d = Delete {
                        key: OrderKey::new(&rev, head_sent_at),
                        evidence: DeleteEvidence::OwnDelete,
                        at_ms: now,
                    };
                    batch.tombstone(&w.uri, d).await?;
                    out.tombstoned.push(w.uri.to_string());
                }
            }
        }
        if !batch.changed().is_empty() {
            repo.last_change_at = now;
        }
        batch.put_repo(account.as_str(), &repo)?;
        batch.commit().await?;
        for u in &out.refetch {
            let at = ready_time(Priority::P0, now, self.random(), None);
            self.enqueue(DirtyItem::point(account.as_str(), u, Priority::P0, at));
        }
        let at = ready_time(Priority::P1, now, self.random(), None);
        self.enqueue(
            DirtyItem::new(account.as_str(), Priority::P1, Reason::OwnWrite, at).with_min_rev(rev),
        );
        Ok(out)
    }

    /// Serves up to `max` ready queue items (§6.1): point fetches for uri-only items, else
    /// `sync_repo` with the scopes `scopes_of(did)` gives. Returns the `sync_repo` reports.
    pub async fn run_ready(
        &self,
        s: &Session<H>,
        max: usize,
        scopes_of: impl Fn(&str) -> Vec<Scope>,
    ) -> Vec<RepoReport> {
        let mut out = Vec::new();
        for _ in 0..max {
            let now = s.host().now_ms();
            let Some(item) = self.pop_ready(now) else {
                break;
            };
            let Ok(did) = Did::parse(&item.did) else {
                continue;
            };
            match item.action() {
                crate::sync::queue::Action::PointFetch(uris) => {
                    for u in uris {
                        if let Ok(uri) = AtUri::parse(&u)
                            && let Err(e) = self.point_fetch(s, &uri, None).await
                        {
                            tracing::debug!(uri = %u, error = %e, "point fetch failed");
                        }
                    }
                }
                crate::sync::queue::Action::Sync => {
                    let scopes = scopes_of(&item.did);
                    if scopes.is_empty() {
                        continue;
                    }
                    let reason = [Reason::Full, Reason::Reconcile, Reason::Resync, Reason::Migration]
                        .into_iter()
                        .find(|r| item.reasons.contains(r))
                        .unwrap_or_else(|| *item.reasons.iter().next().unwrap_or(&Reason::Hint));
                    let req = SyncRequest {
                        did,
                        scopes,
                        reason,
                        confirm: None,
                    };
                    out.push(self.sync_repo(s, &req).await);
                }
            }
        }
        out
    }
}

/// The scheduler origin of a service.
fn service_origin(svc: &Service) -> String {
    match svc {
        Service::Anon(u) => url_origin(u),
        Service::UserPds(d) => format!("userpds:{d}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses() {
        assert_eq!(account_status_of(true, None), AccountStatus::Active);
        assert_eq!(account_status_of(false, None), AccountStatus::Inactive);
        for s in ["takendown", "suspended", "deleted", "deactivated"] {
            assert_eq!(account_status_of(false, Some(s)), AccountStatus::Inactive);
        }
        assert_eq!(
            account_status_of(false, Some("throttled")),
            AccountStatus::Temporary
        );
        assert_eq!(
            account_status_of(false, Some("desynchronized")),
            AccountStatus::Temporary
        );
    }

    #[test]
    fn collection_sets() {
        assert_eq!(ours().len(), 10);
        let r = ranged();
        assert_eq!(r.len(), 7);
        assert!(r.iter().all(|c| schema::is_ranged(c)));
        assert!(!r.iter().any(|c| c == nsid::MEMBERSHIP));
        assert_eq!(Scope::Forum("abc".into()).name(), "abc");
        assert_eq!(Scope::Own.name(), "own");
    }
}
