// SPDX-License-Identifier: AGPL-3.0-or-later
//! `sync_repo` (`sync.md` §5.2), point fetches (§5.7), own writes (§5.8), head checks (§6.7)
//! and account status (§3.4).
//!
//! ```text
//! sync_repo(did, scopes, reason):
//!   repo lock; identity (document only, level doc, else every row is idHint); PDS
//!     identity hint: rows idHint, keyed ("", sentAt), inexact; no tombstones; no repo state
//!     (rev, commit, listed, capped, colls, headSeen, marks, status); a confirmation fails, so
//!     the candidate stays a candidate
//!   h1 = getLatestCommit                                   headSeen = {h1.rev, h1.cid, sentAt}
//!     RepoNotFound | RepoDeactivated | RepoTakendown | RepoSuspended → re-fetch the document:
//!       another PDS → migration (P1); else getRepoStatus; Failed
//!   h1.rev < repo.rev → Rollback (cache kept, backoff, rollbackSince)  [sync --full accepts it:
//!                        every cached row of the DID, in every scope, is forgotten]
//!   h1 = (repo.rev, repo.commit), every scope listed at it, no marks → Clean
//!   confirm (a candidate, §4.2.1): the membership getRecord first; absent or invalid → Rejected
//!   colls: repo.colls if ≤ 24 h old, not dirty, not reconcile, and no stored row lies in a
//!          collection outside it; else describeRepo ∩ (OURS ∪ {app.bsky.actor.profile})
//!          (on failure or a malformed `collections`: OURS)
//!   in parallel: list_range for each (fk scope, c ∈ RANGED ∩ colls, or c with stored rows in
//!                the range); the membership getRecord; GLOBAL getRecords; OWN full listings
//!   h2 = getLatestCommit; stable ⇔ h2 = h1
//!   one batch: admit every listed row (key (h1.rev, page sentAt), exact ⇔ stable); for stable,
//!   complete ranges, tombstone unlisted rows above the cap boundary and drop those below it
//!   (a collection is emptied only by a listing, never by its absence from describeRepo);
//!   OWN: complete full listings tombstone unlisted rows of the collection in every scope;
//!   memberships (RecordNotFound → tombstone); if stable: rev, commit, listed, capped, marks;
//!   colls; host capabilities (re-read and merged in the batch); repo row — one KvStore::write
//!   strikes: one per bad record (HostRow.strikes), one breaker failure per bad response (R4)
//!   ¬stable → queue (minRev h2.rev) at P1, or P3 with the backoff after 3 unstable passes
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
use crate::store::records::{
    Admission, Batch, Delete, DeleteEvidence, OrderKey, RecordStore, Slot,
};
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
    Collection::ALL
        .iter()
        .map(|c| c.nsid().to_owned())
        .collect()
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
    /// Report the state of this membership in [`RepoReport::membership`] without gating the
    /// pass on it (a granted participant, `social.md` §3.3).
    pub check: Option<Confirm>,
}

impl SyncRequest {
    /// A sync of `did` for forum `fk`.
    pub fn forum(did: Did, fk: &str, reason: Reason) -> Self {
        Self {
            did,
            scopes: vec![Scope::Forum(fk.to_owned())],
            reason,
            confirm: None,
            check: None,
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

    /// Reports the membership in `forum` without gating the pass on it.
    pub fn checking(mut self, forum: AtUri, fk: &str) -> Self {
        self.check = Some(Confirm {
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

    /// A partial problem that is not the repo's failure (a failed `describeRepo` with OURS
    /// listed instead, a safety cap that stopped a listing): reported, but no repo backoff
    /// follows; the affected scope stays below `Fresh` on its own.
    fn soft_issue(&mut self, e: &SyncError) {
        if e.is_refusal() {
            self.refused = true;
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
    Found {
        rec: Box<AdmittedRecord>,
        sent_at: i64,
    },
    NotFound {
        sent_at: i64,
    },
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
    // A missing or malformed `collections` is a failed `describeRepo` (the caller falls back to
    // OURS, §5.2), never "no collections": that would read every range as listed and empty.
    let Some(listed) = v.get("collections").and_then(Value::as_array) else {
        return Err(XrpcCallError::Decode(
            "describeRepo: `collections` is missing or not an array".into(),
        ));
    };
    let mut colls = BTreeSet::new();
    for c in listed {
        let Some(c) = c.as_str() else {
            return Err(XrpcCallError::Decode(
                "describeRepo: `collections` holds a non-string".into(),
            ));
        };
        if wanted.contains(c) {
            colls.insert(c.to_owned());
        }
    }
    Ok((
        colls.into_iter().collect(),
        net::sent_at(&resp).unwrap_or_default(),
    ))
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

/// What one pass learned about its host (see [`Syncer::put_host`]).
#[derive(Debug, Default)]
struct HostDelta {
    verdicts: Vec<RangeVerdict>,
    /// The 7-day re-probe of a `No` ran (§5.3).
    reprobe: bool,
    strikes: u32,
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

    /// Merges what a pass learned about its host into the stored `HostRow`, re-read inside the
    /// pass's batch: concurrent passes on one origin each load the row when they start, so
    /// writing that copy back whole would lose the other passes' verdicts and strikes.
    async fn put_host(
        &self,
        batch: &mut Batch<'_, H::Kv>,
        origin: &str,
        d: &HostDelta,
        now: i64,
    ) -> Result<(), SyncError> {
        let mut row = batch.host_caps(origin).await?.unwrap_or_default();
        if d.reprobe && row.range_cursor == RangeCursor::No && row.use_range(now) {
            row.range_cursor = RangeCursor::Unknown;
        }
        list::apply_verdicts(&mut row, &d.verdicts, now);
        row.strikes = row.strikes.saturating_add(d.strikes);
        self.snapshot_host(origin, &mut row);
        batch.put_host_caps(origin, &row)?;
        Ok(())
    }

    /// Counts admission strikes: each one in `HostRow.strikes`, but at most one breaker failure
    /// per response (`limits.md` R4), so one hostile repo cannot open a shared host's breaker
    /// with a single page of bad records.
    fn strike(&self, d: &mut HostDelta, origin: &str, strikes: u32, responses: u32, now: i64) {
        d.strikes = d.strikes.saturating_add(strikes);
        for _ in 0..responses {
            self.net().strike(origin, now);
        }
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
                    Ok(rec) => Ok(Fetched::Found {
                        rec: Box::new(rec),
                        sent_at,
                    }),
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
                let r = batch.put_version((*rec).into_version(obs)).await?;
                Ok((r.changed, Some(r.admission)))
            }
            Fetched::NotFound { sent_at } => {
                if id_hint {
                    // a tombstone is doc-level evidence: a hint PDS's RecordNotFound is not
                    return Ok((false, None));
                }
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

    async fn write_host_only(
        &self,
        store: &RecordStore<'_, H::Kv>,
        origin: &str,
        d: &HostDelta,
        now: i64,
    ) -> Result<(), SyncError> {
        let mut b = store.batch();
        self.put_host(&mut b, origin, d, now).await?;
        b.commit().await?;
        Ok(())
    }

    /// Writes the repo row (unless the identity is a hint: a PDS that no document names never
    /// changes repo state, §1.5) and the host delta.
    #[allow(clippy::too_many_arguments)]
    async fn write_repo_only(
        &self,
        store: &RecordStore<'_, H::Kv>,
        did: &Did,
        repo: &RepoRow,
        origin: &str,
        d: &HostDelta,
        now: i64,
    ) -> Result<(), SyncError> {
        let mut b = store.batch();
        b.put_repo(did.as_str(), repo)?;
        self.put_host(&mut b, origin, d, now).await?;
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
        if id_hint && req.confirm.is_some() {
            // a PDS that no DID document names can neither confirm nor reject a membership: the
            // candidate stays a candidate (§1.5)
            return Err(SyncError::IdentityHint {
                did: did.to_string(),
            });
        }
        // Under an identity hint (§1.5) the PDS is named by no DID document at level `doc`: its
        // rows are admitted as idHint and inexact, and it writes no tombstones and changes no
        // repo state (rev, commit, listed, capped, colls, headSeen, marks, status), so a wrong
        // or hostile hint cannot pin `repo.rev`, suppress honest data or fake completeness.
        let doc_level = !id_hint;
        let origin = service_origin(&svc);
        let mut repo = store
            .repo(did.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        if doc_level {
            // a migration keeps the cached rows (§3.3)
            repo.pds.clone_from(&pds);
        }
        let host_row = self.load_host_row(&store, &origin).await?;
        let mut host_delta = HostDelta::default();
        let xrpc = s.xrpc();

        // h1
        let h1 = match get_head(&xrpc, &svc, did).await {
            Ok(h) => h,
            Err(e) => {
                if matches!(e, XrpcCallError::Blocked(_)) {
                    // refused locally: not the repo's fault (the host state is kept: earlier
                    // attempts of this request may have opened the breaker)
                    self.write_host_only(&store, &origin, &host_delta, now)
                        .await?;
                    return Err(e.into());
                }
                if doc_level && e.is_repo_status() {
                    self.on_repo_status_error(s, did, &mut repo).await;
                }
                repo.fail(now, self.random());
                self.write_repo_only(&store, did, &repo, &origin, &host_delta, now)
                    .await?;
                return Err(e.into());
            }
        };
        if doc_level {
            repo.observe_head(&h1.rev, h1.cid, h1.sent_at);
            if repo.status != AccountStatus::Active && repo.status_detail.is_some() {
                // the PDS answers for the repo again: it is active (a status check confirms
                // later)
                repo.status = AccountStatus::Active;
                repo.status_detail = None;
            }
        }

        // rollback (§5.9); a hint PDS's head says nothing about the repo's synced rev
        let mut accept_rollback = false;
        if doc_level && !repo.rev.is_empty() && h1.rev < repo.rev {
            if !full {
                repo.rollback_since.get_or_insert(now);
                repo.fail(now, self.random());
                self.write_repo_only(&store, did, &repo, &origin, &host_delta, now)
                    .await?;
                return Ok(RepoOutcome::Rollback);
            }
            // `sync --full` discards the repo's cached state and lists it again, which accepts
            // the PDS state: the PDS is the database.
            repo.rev.clear();
            repo.commit = None;
            repo.rollback_since = None;
            repo.marks.clear();
            repo.marks_overflow = false;
            accept_rollback = true;
        }
        // The scopes whose rows a rollback acceptance forgets: every scope this repo was ever
        // listed for, every known forum and the requested ones, so that no row or tombstone
        // keyed above the new revs survives in a scope this pass does not list (each of them
        // reads `Unavailable` until it is listed again).
        let mut forgotten_fks: BTreeSet<String> = BTreeSet::new();
        if accept_rollback {
            forgotten_fks.extend(repo.listed.keys().cloned());
            forgotten_fks.extend(repo.capped.keys().cloned());
            forgotten_fks.extend(repo.byte_capped.keys().cloned());
            forgotten_fks.extend(store.batch().forum_fks().await?);
            let names: Vec<String> = repo
                .listed
                .keys()
                .chain(repo.capped.keys())
                .chain(repo.byte_capped.keys())
                .cloned()
                .collect();
            for n in names {
                repo.reset_scope(&n);
            }
        }
        if full && doc_level {
            for sc in &req.scopes {
                repo.reset_scope(sc.name());
                if let Scope::Forum(fk) = sc
                    && accept_rollback
                {
                    forgotten_fks.insert(fk.clone());
                }
            }
        }

        // Clean
        let mut clean = !full
            && repo.rev == h1.rev
            && repo.commit == Some(h1.cid)
            && req
                .scopes
                .iter()
                .all(|sc| repo.listed_rev(sc.name()) == repo.rev)
            && repo.marks.is_empty()
            && !repo.marks_overflow;
        // A reconcile always refreshes `describeRepo` (§5.2 rules), even at an unchanged head: a
        // collection created while `repo.colls` was reused was not listed at that head. A new
        // collection makes the pass a full one.
        let mut pre_described: Option<(Vec<String>, i64)> = None;
        if clean && req.reason == Reason::Reconcile {
            match describe(&xrpc, &svc, did).await {
                Ok((cs, at)) => {
                    if cs.iter().any(|c| !repo.colls.contains(c)) {
                        clean = false;
                    }
                    pre_described = Some((cs, at));
                }
                Err(e) => tracing::debug!(did = %did, error = %e, "reconcile: describeRepo failed"),
            }
        }
        if clean {
            if let Some((cs, at)) = pre_described.take() {
                repo.colls = cs;
                repo.colls_at = at;
                repo.colls_dirty = false;
            }
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
            if let Some(c) = &req.check {
                rep.membership
                    .insert(c.fk.clone(), stored_membership(&store, did, c).await?);
            }
            self.write_repo_only(&store, did, &repo, &origin, &host_delta, now)
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
                        self.strike(&mut host_delta, &origin, 1, 1, now);
                        rep.strikes += 1;
                    }
                    rep.membership
                        .insert(c.fk.clone(), MembershipCheck::Unknown(e.to_string()));
                    if !e.is_refusal() {
                        repo.fail(now, self.random());
                    }
                    self.write_repo_only(&store, did, &repo, &origin, &host_delta, now)
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
                    self.note_conflict(
                        did,
                        &format!("at://{did}/{}/{}", nsid::MEMBERSHIP, c.fk),
                        a,
                        now,
                    );
                }
                repo.succeed(now);
                if changed {
                    repo.last_change_at = now;
                }
                rep.changed = batch.changed().len();
                batch.put_repo(did.as_str(), &repo)?;
                self.put_host(&mut batch, &origin, &host_delta, now).await?;
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
                for c in ranged_all
                    .iter()
                    .chain([nsid::MEMBERSHIP.to_owned()].iter())
                {
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
        let colls: Vec<String> = if let Some((cs, at)) = pre_described {
            described = Some(at);
            cs
        } else if need_describe {
            match describe(&xrpc, &svc, did).await {
                Ok((cs, at)) => {
                    described = Some(at);
                    cs
                }
                Err(e) => {
                    // OURS is listed instead: the pass can still complete (no repo backoff)
                    rep.soft_issue(&SyncError::Xrpc(e));
                    ours()
                }
            }
        } else {
            repo.colls.clone()
        };
        let has = |c: &str| colls.iter().any(|x| x == c);

        // listings, memberships, global records, own listings: in parallel
        // the 7-day re-probe: a clean probe may set Yes again
        host_delta.reprobe = host_row.range_cursor == RangeCursor::No && host_row.use_range(now);
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
            keep_values: false,
        };
        // Each RANGED collection in `colls`, and each one outside it that still holds stored
        // rows of this range: a collection is only ever emptied by a listing, never by its
        // absence from `describeRepo` (an empty listing costs one request).
        let mut range_jobs: Vec<(String, String)> = Vec::new();
        for fk in &fk_scopes {
            for c in &ranged_all {
                let listed = has(c)
                    || !store
                        .scan_repo_range(did.as_str(), c, fk, Dir::Forward)
                        .with_page_size(1)
                        .take(1)
                        .await?
                        .is_empty();
                if listed {
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
            } else {
                // the collection does not exist and nothing is stored
                rep.membership
                    .insert((*fk).to_owned(), MembershipCheck::Absent);
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
        let own = req.scopes.contains(&Scope::Own);
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
                if doc_level {
                    repo.observe_head(&h2.rev, h2.cid, h2.sent_at);
                }
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
        if accept_rollback {
            let fk_colls: Vec<String> = ranged_all
                .iter()
                .cloned()
                .chain([nsid::MEMBERSHIP.to_owned()])
                .collect();
            let global_colls: Vec<String> = ours()
                .into_iter()
                .chain([BSKY_PROFILE.to_owned()])
                .collect();
            batch
                .forget_repo(did.as_str(), &forgotten_fks, &fk_colls, &global_colls)
                .await?;
        }
        // rows of a stable pass are exact only under a doc-level identity
        let exact = stable && doc_level;
        let mut strikes = 0u32;
        let mut bad_responses = 0u32;
        let mut conflicts: Vec<(String, Admission)> = Vec::new();
        let any_hints = !self.shared.hints().is_empty();

        for ((fk, c), res) in range_jobs.iter().zip(lists) {
            let st = scopes.get_mut(fk.as_str()).expect("scope of a range job");
            match res {
                Ok(mut l) => {
                    host_delta.verdicts.extend(l.verdict);
                    strikes += l.strikes;
                    bad_responses += l.bad_responses;
                    if let Some(why) = &l.stop {
                        rep.soft_issue(&SyncError::List(ListError::Stopped {
                            collection: c.clone(),
                            why: why.clone(),
                        }));
                    }
                    let keys = l.keys();
                    let boundary = l.boundary().map(str::to_owned);
                    let mut versions = Vec::new();
                    for (rec, sent_at) in l.take_admitted() {
                        let obs = Observation::listing(&h1.rev, sent_at, exact, now)
                            .with_id_hint(id_hint);
                        let obs = if any_hints {
                            self.observation(obs, &rec)
                        } else {
                            obs
                        };
                        versions.push(rec.into_version(obs));
                    }
                    let uris: Vec<String> = versions.iter().map(|v| v.uri.to_string()).collect();
                    for (u, r) in uris.into_iter().zip(batch.put_versions(versions).await?) {
                        if matches!(r.admission, Admission::Conflict(_)) {
                            conflicts.push((u, r.admission));
                        }
                    }
                    if exact && l.complete {
                        let key = OrderKey::new(&h1.rev, l.first_sent_at.unwrap_or(h1.sent_at));
                        batch
                            .reconcile_range(
                                did.as_str(),
                                c,
                                fk,
                                &keys,
                                &key,
                                boundary.as_deref(),
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
                    if let Some(b) = boundary {
                        st.capped.insert(c.clone(), b);
                    }
                }
                Err(e) => {
                    if matches!(
                        e,
                        SyncError::List(ListError::BadPage(_) | ListError::Duplicate(_))
                    ) {
                        strikes += 1;
                        bad_responses += 1;
                    }
                    st.complete = false;
                    rep.issue(&e);
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
            .chain(req.check.iter())
            .map(|c| (c.fk.as_str(), &c.forum))
            .collect();
        for (fk, res) in mem_results {
            match res {
                Ok(f) => {
                    if let Some(forum) = forum_of_fk.get(fk.as_str()) {
                        rep.membership
                            .insert(fk.clone(), membership_check(&f, forum));
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
                        bad_responses += 1;
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
                        bad_responses += 1;
                    }
                    if let Some(st) = scopes.get_mut("global") {
                        st.complete = false;
                    }
                    rep.issue(&e);
                }
            }
        }
        // OWN: admitted; a complete, stable full listing of a collection also tombstones the
        // stored rows it lacks (deletes made on another device), in every fk scope known here:
        // forum rows, scopes this repo was listed for, the requested ones and the fks of every
        // listed own record. (Rows have no per-DID index: rows left in an fk none of these
        // name are cleared by that forum's own listing.)
        let own_fks: BTreeSet<String> = if own_jobs.is_empty() || !exact {
            BTreeSet::new()
        } else {
            let mut f: BTreeSet<String> = batch.forum_fks().await?.into_iter().collect();
            f.extend(repo.listed.keys().cloned());
            f.extend(fk_scopes.iter().map(|s| (*s).to_owned()));
            for l in owns.iter().flatten() {
                f.extend(
                    l.entries
                        .iter()
                        .filter_map(|e| schema::fk_prefix(&e.rkey))
                        .map(str::to_owned),
                );
            }
            f
        };
        for (c, res) in own_jobs.iter().zip(owns) {
            match res {
                Ok(mut l) => {
                    strikes += l.strikes;
                    bad_responses += l.bad_responses;
                    if let Some(why) = &l.stop {
                        rep.soft_issue(&SyncError::List(ListError::Stopped {
                            collection: c.clone(),
                            why: why.clone(),
                        }));
                    }
                    let keys = l.keys();
                    let versions: Vec<_> = l
                        .take_admitted()
                        .into_iter()
                        .map(|(rec, sent_at)| {
                            let obs = Observation::listing(&h1.rev, sent_at, exact, now)
                                .with_id_hint(id_hint);
                            rec.into_version(obs)
                        })
                        .collect();
                    batch.put_versions(versions).await?;
                    if exact && l.complete {
                        let key = OrderKey::new(&h1.rev, l.first_sent_at.unwrap_or(h1.sent_at));
                        batch
                            .reconcile_collection(did.as_str(), c, &own_fks, &keys, &key, now)
                            .await?;
                    }
                    if !l.complete
                        && let Some(st) = scopes.get_mut("own")
                    {
                        st.complete = false;
                        st.byte_capped |= l.byte_capped;
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

        // repo state (never from a hint PDS)
        if exact {
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
        } else if !stable {
            repo.unstable = repo.unstable.saturating_add(1);
        }
        if doc_level {
            if let Some(at) = described {
                repo.colls = colls.clone();
                repo.colls_at = at;
                repo.colls_dirty = false;
            } else if need_describe {
                // describeRepo failed: OURS was listed; it is asked again next time
                repo.colls = colls.clone();
            }
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
        if strikes > 0 {
            rep.strikes += strikes;
            self.strike(&mut host_delta, &origin, strikes, bad_responses, now);
        }
        let changed = batch.changed().len();
        if changed > 0 {
            repo.last_change_at = now;
        }
        batch.put_repo(did.as_str(), &repo)?;
        self.put_host(&mut batch, &origin, &host_delta, now).await?;
        batch.commit().await?;
        rep.changed = changed;
        for (u, a) in &conflicts {
            self.note_conflict(did, u, a, now);
        }

        if !stable {
            let item = if repo.unstable >= UNSTABLE_MAX {
                DirtyItem::new(
                    did.as_str(),
                    Priority::P3,
                    Reason::Unstable,
                    repo.next_check,
                )
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
        // under an identity hint the answer changes no repo state (marks, colls, pds; §1.5)
        let doc_level = !id_hint;
        if doc_level {
            repo.pds.clone_from(&pds);
        }
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
                    self.net().strike(&service_origin(&svc), s.host().now_ms());
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
                if ok_mark && doc_level {
                    repo.marks.remove(uri.as_str());
                }
                if doc_level
                    && ours().iter().any(|c| c == coll)
                    && !repo.colls.iter().any(|c| c == coll)
                {
                    repo.colls_dirty = true;
                }
                Some((rec.cid, rec.validity.is_valid()))
            }
            Fetched::NotFound { .. } => {
                if doc_level && hint.is_some_and(|h| h.cid.is_none()) {
                    repo.marks.remove(uri.as_str());
                }
                None
            }
        };
        let (changed, adm) = self
            .apply_fetched(
                &mut batch, did, coll, rkey, fetched, &head_rev, id_hint, now, true,
            )
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
        let (svc, pds, id_hint) = self.pds_of(s, did).await?;
        let store = RecordStore::new(s.host().cache());
        let mut repo = store
            .repo(did.as_str())
            .await?
            .unwrap_or_else(|| RepoRow::new(&pds));
        let now = s.host().now_ms();
        let h = match get_head(&s.xrpc(), &svc, did).await {
            Ok(h) => h,
            Err(e) => {
                if !id_hint && e.is_repo_status() {
                    self.on_repo_status_error(s, did, &mut repo).await;
                }
                repo.checked_at = now;
                let mut b = store.batch();
                b.put_repo(did.as_str(), &repo)?;
                b.commit().await?;
                return Err(e.into());
            }
        };
        if !id_hint {
            // a hint PDS's head is never `headSeen` (§1.5): it could pin it far ahead
            repo.observe_head(&h.rev, h.cid, h.sent_at);
        }
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

    /// Runs due polls (§6.7, [`schedule::due_polls`]): one `getLatestCommit` each, concurrently
    /// within the scheduler's limits; a head that differs from the synced one queues the repo
    /// (P1, or P2 for `cold`). Returns each DID's result, in the order given.
    pub async fn run_polls(
        &self,
        s: &Session<H>,
        due: &[schedule::PollDue],
    ) -> Vec<(String, Result<Option<Priority>, SyncError>)> {
        let futs = due.iter().map(|p| async move {
            let r = match Did::parse(&p.did) {
                Ok(did) => {
                    self.head_check(s, &did, p.class == crate::store::repo::RepoClass::Authority)
                        .await
                }
                Err(e) => Err(SyncError::Admission(format!("bad DID {:?}: {e}", p.did))),
            };
            (p.did.clone(), r)
        });
        join_all(futs).await
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
        // Without a commit (a no-op write) the rev is a later head's: another device may have
        // changed the record in between, so the key is only a lower bound (not exact) and an
        // exact listing at that rev replaces it rather than conflicting with it.
        let (rev, head_sent_at, exact) = match commit {
            Some(c) => {
                if let Ok(cid) = Cid::try_from(c.cid.as_str()) {
                    repo.observe_head(&c.rev, cid, sent_at);
                }
                (c.rev.clone(), sent_at, true)
            }
            None => {
                let h = get_head(&s.xrpc(), &svc, account).await?;
                repo.observe_head(&h.rev, h.cid, h.sent_at);
                (h.rev, sent_at, false)
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
                            let own = Observation::own_write(&rev, head_sent_at, now);
                            let own = if exact { own } else { own.inexact() };
                            let obs = self.observation(own, &rec);
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

    /// Serves up to `max` ready queue items (§6.1), concurrently within the scheduler's limits
    /// and each at its own priority (counted in `s`'s stats): point fetches for uri-only items,
    /// else `sync_repo` with the scopes `scopes_of(did)` gives (an item without scopes is
    /// dropped). Returns the `sync_repo` reports, in dispatch order.
    pub async fn run_ready(
        &self,
        s: &Session<H>,
        max: usize,
        scopes_of: impl Fn(&str) -> Vec<Scope>,
    ) -> Vec<RepoReport> {
        enum Job {
            Point(Vec<AtUri>),
            Sync(Box<SyncRequest>),
        }
        let now = s.host().now_ms();
        let mut jobs: Vec<(Session<H>, Job)> = Vec::new();
        while jobs.len() < max {
            let Some(item) = self.pop_ready(now) else {
                break;
            };
            let Ok(did) = Did::parse(&item.did) else {
                continue;
            };
            let job = match item.action() {
                crate::sync::queue::Action::PointFetch(uris) => {
                    Job::Point(uris.iter().filter_map(|u| AtUri::parse(u).ok()).collect())
                }
                crate::sync::queue::Action::Sync => {
                    let scopes = scopes_of(&item.did);
                    if scopes.is_empty() {
                        continue;
                    }
                    let reason = [
                        Reason::Full,
                        Reason::Reconcile,
                        Reason::Resync,
                        Reason::Migration,
                    ]
                    .into_iter()
                    .find(|r| item.reasons.contains(r))
                    .unwrap_or_else(|| *item.reasons.iter().next().unwrap_or(&Reason::Hint));
                    Job::Sync(Box::new(SyncRequest {
                        did,
                        scopes,
                        reason,
                        confirm: None,
                        check: None,
                    }))
                }
            };
            jobs.push((self.session(item.priority, Arc::clone(s.stats())), job));
        }
        let futs = jobs.iter().map(|(sess, job)| async move {
            match job {
                Job::Point(uris) => {
                    for uri in uris {
                        if let Err(e) = self.point_fetch(sess, uri, None).await {
                            tracing::debug!(uri = %uri, error = %e, "point fetch failed");
                        }
                    }
                    None
                }
                Job::Sync(req) => Some(self.sync_repo(sess, req).await),
            }
        });
        join_all(futs).await.into_iter().flatten().collect()
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
