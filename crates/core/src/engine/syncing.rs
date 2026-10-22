// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sync operations (`engine-api.md` §3.3): `sync_forum`, `sync_thread`, and the authority sync
//! that `forum_open` schedules.
//!
//! Both operations emit `SyncProgress { forum, done, total }` while they run (`done`: repos
//! synced so far, `total`: repos planned so far; the total grows as each step of `sync.md` §4.5
//! plans its candidates) and a last one with `done == total` when they end, then drop the
//! forum's model and emit `Changed`. They do so also when they fail part-way, since the cache
//! may have changed. The report's completeness (`reposComplete` of `reposTotal`) is separate
//! from the progress counts.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use futures::future::join_all;
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::collection::nsid;
use serde_json::Value;

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::{self, OpenedForum, Progress};
use crate::dto::{self, DtoVersion, Event, SyncIssue, SyncReport, Topic};
use crate::host::Host;
use crate::limits;
use crate::store::RepoStatus;
use crate::store::records::RecordStore;
use crate::store::schema::{GLOBAL_SCOPE, scope_of};
use crate::sync::queue::ready_time;
use crate::sync::{
    DirtyItem, PointOutcome, Priority, Reason, RepoOutcome, RepoReport, SyncError, SyncRequest,
};

use super::models::{Runtime, store_error};
use super::{Engine, EngineError};

/// Options accepted by [`Engine::sync_forum`]: the DTO [`dto::SyncOpts`] (clients), or the
/// discovery module's [`discovery::SyncOpts`] (with the view and CORE+ inputs).
pub trait ForumSyncOpts {
    /// As discovery options.
    fn discovery_opts(&self) -> discovery::SyncOpts;
}

impl ForumSyncOpts for discovery::SyncOpts {
    fn discovery_opts(&self) -> discovery::SyncOpts {
        self.clone()
    }
}

impl ForumSyncOpts for dto::SyncOpts {
    fn discovery_opts(&self) -> discovery::SyncOpts {
        discovery::SyncOpts {
            budget_ms: self.budget_ms,
            max_requests: self.max_requests,
            full: self.full,
            ..discovery::SyncOpts::default()
        }
    }
}

/// `SyncProgress` for one forum: forwards discovery progress to the bus and remembers the last
/// total, so the final event can close the count whatever happened.
struct ForumProgress {
    rt: Arc<Runtime>,
    forum: String,
    total: AtomicU32,
}

impl ForumProgress {
    fn report(&self, done: u32, total: u32) {
        self.total.fetch_max(total, Ordering::SeqCst);
        self.rt.bus.emit(Event::SyncProgress {
            forum: self.forum.clone(),
            done,
            total,
        });
    }

    /// The last event: `done == total`.
    fn finish(&self, n: Option<u32>) {
        let n = n.unwrap_or_else(|| self.total.load(Ordering::SeqCst));
        self.rt.bus.emit(Event::SyncProgress {
            forum: self.forum.clone(),
            done: n,
            total: n,
        });
    }
}

fn issue(did: &str, host: Option<&String>, e: &SyncError) -> SyncIssue {
    SyncIssue {
        did: Some(did.to_owned()),
        host: host.cloned(),
        code: e.code().to_owned(),
        message: e.to_string(),
    }
}

/// The thread root a comment value names.
fn root_of(value: &Value) -> Option<AtUri> {
    let r = value.get("root")?.get("uri")?.as_str()?;
    AtUri::parse(r).ok()
}

impl<H: Host> Engine<H> {
    /// `sync_forum(uri, SyncOpts)` (`engine-api.md` §3.3): opens the forum and syncs its
    /// authorities and confirmed participants in the order of `sync.md` §4.5. Emits
    /// `SyncProgress` as repos finish (see the module docs) and `Changed [Forum]`; the forum's
    /// authorities are queued for handle verification.
    // In wasm the runtime is not `Send` (callbacks are `MaybeSendSync`); the core is
    // single-threaded there.
    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn sync_forum(
        &self,
        forum: &str,
        opts: &impl ForumSyncOpts,
    ) -> Result<SyncReport, EngineError> {
        let key = self.forum_key(forum).await?;
        let f = key.uri.to_string();
        let mut opts = opts.discovery_opts();
        // progress, chained after the caller's own callback
        let progress = Arc::new(ForumProgress {
            rt: Arc::clone(&self.rt),
            forum: f.clone(),
            total: AtomicU32::new(0),
        });
        let (outer, p2) = (opts.progress.clone(), Arc::clone(&progress));
        opts.progress = Progress::new(move |done, total| {
            outer.report(done, total);
            p2.report(done, total);
        });
        let res = self.syncer().sync_forum(&f, &opts).await;
        self.rt.invalidate(&f);
        let res = match res {
            Ok(fs) => {
                self.rt.note_opened(&fs.forum, self.host.now_ms());
                self.rt
                    .queue_verify(fs.forum.authorities.keys().map(String::as_str));
                progress.finish(Some(u32::try_from(fs.repos.len()).unwrap_or(u32::MAX)));
                Ok(fs.report)
            }
            Err(e) => {
                progress.finish(None);
                Err(e.into())
            }
        };
        self.rt.bus.changed([Topic::Forum { uri: f }]);
        res
    }

    /// The forum of a record uri in a forum's key range: from the registry, else the cached
    /// `("forum", fk)` row, else the cached record's own `forum` field, else the record fetched
    /// now (P0).
    pub(crate) async fn forum_of(&self, uri: &AtUri) -> Result<ForumKey, EngineError> {
        Ok(self.forum_of_fetched(uri).await?.0)
    }

    /// [`Engine::forum_of`], and whether the record was fetched to find its forum.
    ///
    /// A link to a record of a forum this device never opened (`#/p/…`, `show <uri>`) names
    /// only the record: its `forum` field is the only way to the forum, so the record is
    /// point-fetched (`sync.md` §5.7) and its forum accepted only if `fk(forum)` is the fk of
    /// the record's own key range.
    pub(crate) async fn forum_of_fetched(
        &self,
        uri: &AtUri,
    ) -> Result<(ForumKey, bool), EngineError> {
        let coll = uri.collection().as_str();
        let rkey = uri.rkey().as_str();
        if coll == nsid::FORUM {
            let key = ForumKey::of_uri(uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
            return Ok((key, false));
        }
        let fk = scope_of(coll, rkey);
        if fk == GLOBAL_SCOPE {
            return Err(EngineError::invalid_ref(format!(
                "{uri} is not a record of a forum"
            )));
        }
        let store = RecordStore::new(self.host.cache());
        let mut f = self.rt.forum_of_fk(fk);
        if f.is_none() {
            f = store.forum(fk).await.map_err(store_error)?.map(|r| r.uri);
        }
        let named = |v: Option<Value>| v.and_then(|v| v.get("forum")?.as_str().map(str::to_owned));
        if f.is_none() {
            f = named(self.cached_value(uri).await?);
        }
        let mut fetched = false;
        if f.is_none() {
            let s = self.p0_session(None, None);
            if let PointOutcome::NotFound { .. } = self.syncer().point_fetch(&s, uri, None).await? {
                return Err(EngineError::not_found(format!("{uri} does not exist")));
            }
            fetched = true;
            f = named(self.cached_value(uri).await?);
        }
        let f = f.ok_or_else(|| {
            EngineError::not_found(format!("the forum of {uri} is not known; open it first"))
        })?;
        let key = AtUri::parse(&f)
            .ok()
            .and_then(|u| ForumKey::of_uri(&u).ok())
            .filter(|k| k.fk == fk)
            .ok_or_else(|| {
                EngineError::not_found(format!(
                    "the forum of {uri} is not known ({f} is not the forum of its key range)"
                ))
            })?;
        self.rt.register(&key);
        Ok((key, fetched))
    }

    /// The cached value of a record, if any.
    async fn cached_value(&self, uri: &AtUri) -> Result<Option<Value>, EngineError> {
        Ok(RecordStore::new(self.host.cache())
            .get(uri)
            .await
            .map_err(store_error)?
            .and_then(|r| r.value_json().ok()))
    }

    /// Sync requests for the authorities of `opened` whose forum range is not fresh: those
    /// never synced (`Unavailable`: the header would be degraded, `sync.md` §4.6) always, the
    /// `Stale` ones too if `stale`; otherwise those are queued at P1 for `live_tail`
    /// (`sync.md` §4.5 step 1).
    async fn authority_jobs(
        &self,
        opened: &OpenedForum,
        stale: bool,
    ) -> Result<Vec<SyncRequest>, EngineError> {
        let key = &opened.key;
        let dids: Vec<&str> = opened.authorities.keys().map(String::as_str).collect();
        let repos = RecordStore::new(self.host.cache())
            .repos(&dids)
            .await
            .map_err(store_error)?;
        let now = self.host.now_ms();
        let mut jobs = Vec::new();
        for (d, r) in dids.iter().zip(repos) {
            let Ok(did) = Did::parse(d) else { continue };
            match r.map_or(RepoStatus::Unavailable, |r| r.repo_status(&key.fk)) {
                RepoStatus::Unavailable => {
                    jobs.push(SyncRequest::forum(did, &key.fk, Reason::Authority));
                }
                RepoStatus::Stale if stale => {
                    jobs.push(SyncRequest::forum(did, &key.fk, Reason::Authority));
                }
                RepoStatus::Stale => {
                    let mut b = [0u8; 8];
                    self.host.fill_random(&mut b);
                    let at = ready_time(Priority::P1, now, u64::from_le_bytes(b), None);
                    self.syncer()
                        .enqueue(DirtyItem::new(*d, Priority::P1, Reason::Authority, at));
                }
                RepoStatus::Fresh => {}
            }
        }
        Ok(jobs)
    }

    /// The authority step `forum_open` schedules (`engine-api.md` §3.2): authorities never
    /// synced are synced now at P0 within [`limits::FORUM_OPEN_SYNC_BUDGET_MS`], stale ones
    /// are queued for `live_tail`. Failures are logged (the header then shows the forum as
    /// degraded). Returns whether the cache changed.
    pub(crate) async fn sync_authorities(&self, opened: &OpenedForum) -> bool {
        let jobs = match self.authority_jobs(opened, false).await {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(forum = %opened.key.uri, error = %e, "authority sync");
                return false;
            }
        };
        if jobs.is_empty() {
            return false;
        }
        let s = self.p0_session(None, Some(limits::FORUM_OPEN_SYNC_BUDGET_MS));
        let sy = self.syncer();
        let reports = join_all(jobs.iter().map(|r| sy.sync_repo(&s, r))).await;
        for r in &reports {
            if let Some(e) = &r.error {
                tracing::warn!(did = %r.did, error = %e, "authority sync");
            }
        }
        let f = opened.key.uri.as_str();
        self.rt.invalidate(f);
        self.rt.bus.changed([Topic::Forum { uri: f.to_owned() }]);
        true
    }

    /// `sync_thread(uri, SyncOpts)` (`engine-api.md` §3.3) for a post or a comment (its
    /// thread): a P0 point fetch of the item (and of a comment's root), then the forum ranges
    /// of the forum's authorities that are not fresh and of the thread's participants (an
    /// author outside the authorities and confirmed participants is confirmed first).
    /// `budget_ms`, `max_requests` and `full` apply. A forum never opened is opened from the
    /// record's `forum` field. `degraded` is the forum's state after the sync.
    pub async fn sync_thread(
        &self,
        uri: &str,
        opts: &dto::SyncOpts,
    ) -> Result<SyncReport, EngineError> {
        let target =
            AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
        let coll = target.collection().as_str();
        if coll != nsid::POST && coll != nsid::COMMENT {
            return Err(EngineError::invalid_ref(format!(
                "{uri} is not a post or a comment"
            )));
        }
        let (key, fetched) = self.forum_of_fetched(&target).await?;
        let f = key.uri.to_string();
        let progress = ForumProgress {
            rt: Arc::clone(&self.rt),
            forum: f.clone(),
            total: AtomicU32::new(0),
        };
        let res = self
            .sync_thread_in(&key, &target, fetched, opts, &progress)
            .await;
        self.rt.invalidate(&f);
        let thread = match &res {
            Ok((_, root, n)) => {
                progress.finish(Some(*n));
                root.to_string()
            }
            Err(_) => {
                progress.finish(None);
                target.to_string()
            }
        };
        self.rt
            .bus
            .changed([Topic::Forum { uri: f }, Topic::Thread { uri: thread }]);
        res.map(|(report, _, _)| report)
    }

    /// The body of [`Engine::sync_thread`]: the report, the thread root and the number of
    /// repos synced.
    async fn sync_thread_in(
        &self,
        key: &ForumKey,
        target: &AtUri,
        fetched: bool,
        opts: &dto::SyncOpts,
        progress: &ForumProgress,
    ) -> Result<(SyncReport, AtUri, u32), EngineError> {
        let s = self.p0_session(opts.max_requests, opts.budget_ms);
        let sy = self.syncer();
        let mut errors: Vec<SyncIssue> = Vec::new();
        if !fetched && let Err(e) = sy.point_fetch(&s, target, None).await {
            errors.push(issue(target.did().as_str(), None, &e));
        }
        // a comment's thread is its root's
        let root = if target.collection().as_str() == nsid::COMMENT {
            let root = self
                .cached_value(target)
                .await?
                .as_ref()
                .and_then(root_of)
                .filter(|r| r.collection().as_str() == nsid::POST)
                .ok_or_else(|| {
                    EngineError::not_found(format!("the thread of {target} is not known"))
                })?;
            if let Err(e) = sy.point_fetch(&s, &root, None).await {
                errors.push(issue(root.did().as_str(), None, &e));
            }
            root
        } else {
            target.clone()
        };
        let opened = self.open_key(key, false).await?;
        // the model must see what the point fetches stored
        self.rt.invalidate(key.uri.as_str());
        let l = self.loaded(key).await?;
        let m = &l.model;
        let mut reqs = self.authority_jobs(&opened, true).await?;
        let mut dids: BTreeSet<String> = BTreeSet::new();
        dids.insert(root.did().to_string());
        dids.insert(target.did().to_string());
        if let Some(th) = m.fold.threads.get(root.as_str()) {
            dids.extend(
                th.placement
                    .parent
                    .keys()
                    .filter_map(|c| m.skel(c))
                    .map(|x| x.did.clone()),
            );
        }
        let reason = if opts.full {
            Reason::Full
        } else {
            Reason::View
        };
        let planned: BTreeSet<String> = reqs.iter().map(|r| r.did.to_string()).collect();
        let fresh_authority = |d: &str| m.fold.is_authority(d) && !planned.contains(d);
        reqs.extend(
            dids.iter()
                .filter(|d| !planned.contains(*d) && !fresh_authority(d))
                .filter_map(|d| Did::parse(d).ok())
                .map(|d| {
                    let known =
                        m.fold.is_authority(d.as_str()) || m.fold.participants.contains(d.as_str());
                    let r = SyncRequest::forum(d, &key.fk, reason);
                    if known {
                        r
                    } else {
                        r.confirming(key.uri.clone(), &key.fk)
                    }
                }),
        );
        // authors who are fresh authorities are counted as synced without a request
        let skipped: Vec<&String> = dids.iter().filter(|d| fresh_authority(d)).collect();
        let total = u32::try_from(reqs.len() + skipped.len()).unwrap_or(u32::MAX);
        let done = AtomicU32::new(u32::try_from(skipped.len()).unwrap_or(u32::MAX));
        progress.report(done.load(Ordering::SeqCst), total);
        // user-initiated: stale collection lists are refreshed (`sync.md` §5.2)
        let reqs: Vec<SyncRequest> = reqs.into_iter().map(SyncRequest::explicit).collect();
        let reports: Vec<RepoReport> = join_all(reqs.iter().map(|r| async {
            let rep = sy.sync_repo(&s, r).await;
            progress.report(done.fetch_add(1, Ordering::SeqCst) + 1, total);
            rep
        }))
        .await;
        let mut complete = u32::try_from(skipped.len()).unwrap_or(u32::MAX);
        let mut stale = 0u32;
        let mut unavailable = 0u32;
        let mut changed = 0u32;
        for r in &reports {
            changed += r.changed as u32;
            match r.outcome {
                RepoOutcome::Clean | RepoOutcome::Applied => complete += 1,
                RepoOutcome::Unstable | RepoOutcome::Rollback => stale += 1,
                RepoOutcome::Rejected => {}
                RepoOutcome::Failed => unavailable += 1,
            }
            if let Some(e) = &r.error {
                errors.push(issue(&r.did, r.pds.as_ref(), e));
            }
            errors.extend(r.issues.iter().map(|(code, message)| SyncIssue {
                did: Some(r.did.clone()),
                host: r.pds.clone(),
                code: code.clone(),
                message: message.clone(),
            }));
        }
        let synced = reports
            .iter()
            .filter(|r| r.outcome != RepoOutcome::Rejected)
            .count();
        let total = u32::try_from(synced + skipped.len()).unwrap_or(u32::MAX);
        let stats = s.stats();
        // the forum's state after the sync
        self.rt.invalidate(key.uri.as_str());
        let degraded = match self.loaded(key).await {
            Ok(after) => after.completeness.degraded,
            Err(_) => l.completeness.degraded,
        };
        self.rt
            .queue_verify(opened.authorities.keys().map(String::as_str));
        let report = SyncReport {
            dto: DtoVersion::default(),
            forum: key.uri.to_string(),
            repos_total: total,
            repos_complete: complete,
            repos_stale: stale,
            repos_unavailable: unavailable,
            requests: stats.requests(),
            bytes: stats.bytes(),
            changed,
            partial: complete < total || stats.exhausted(),
            degraded,
            errors,
        };
        let n = u32::try_from(reports.len() + skipped.len()).unwrap_or(u32::MAX);
        Ok((report, root, n))
    }
}
