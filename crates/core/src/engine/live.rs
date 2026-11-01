// SPDX-License-Identifier: AGPL-3.0-or-later
//! Long-lived work (`engine-api.md` §3.3): `live_tail`, `background_verify` and the background
//! verification of shown authors' handles.
//!
//! `live_tail` runs rounds ([`Engine::live_step`]) until [`Engine::live_stop`]. M2 is polling
//! mode (`sync.md` §6.7, §6.8); each round:
//!
//! 1. seeds the followed list of accounts that logged in since their last seeding (§4.8);
//! 2. polls the due heads (`getLatestCommit`) of the followed forums' authorities and
//!    participants and of the accounts' own repos; a moved head queues the repo (§6.7);
//! 3. queues the due reconciles (§6.8; a repo never synced is due at once);
//! 4. serves up to [`limits::LIVE_TAIL_BATCH`] ready dirty-queue items (§6.1): point fetches,
//!    and `sync_repo` over the repo's forum scopes (those it is relevant to, and those it was
//!    listed for), plus `OWN` for an account; the models of the forums that changed are dropped
//!    and `Changed` is emitted; an applied sync of an own repo re-seeds the followed list;
//! 5. verifies the handles of up to [`limits::HANDLE_VERIFY_BATCH`] shown authors (§3.3; the
//!    views queue them), emitting `Changed` when a handle state changed;
//! 6. drops expired tombstones, once per [`limits::TOMBSTONE_GC_EVERY_MS`];
//!
//! then sleeps (with the host) until the next queue item or poll is due, at most
//! [`limits::LIVE_TAIL_IDLE_MS`]. Jetstream (§6.4) and indexer `getChanges` (§6.6) feed the same
//! queue from M5 and M6. Errors are logged and the round is retried; they are never returned.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use futures::StreamExt;
use lessdugin_atp::syntax::{AtUri, Did};

use crate::dto::Topic;
use crate::host::{Dir, Host, KeyRange, KvStore};
use crate::limits;
use crate::live::{
    Action as LiveAction, Context as LiveContext, LiveConfig, LiveSource, SourceStatus,
};
use crate::outbox;
use crate::store::RepoRow;
use crate::store::key::{self, Part};
use crate::store::records::RecordStore;
use crate::store::schema::{self, GLOBAL_SCOPE, scope_of};
use crate::sync::queue::ready_time;
use crate::sync::schedule::{self, ReconcileRole};
use crate::sync::{Action, DirtyItem, Priority, Reason, RepoOutcome, Scope, Stats};

use super::models::store_error;
use super::{Engine, EngineError};

/// What one `live_tail` round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveRound {
    /// Heads polled.
    pub polled: usize,
    /// Reconciles queued.
    pub reconciles: usize,
    /// `sync_repo` passes run.
    pub synced: usize,
    /// Forums whose cached state changed (`Changed` was emitted), by uri.
    pub changed: Vec<String>,
    /// Author handles verified.
    pub verified: usize,
    /// Expired tombstones dropped.
    pub gc: usize,
    /// When the next round is due (ms from now).
    pub next_ms: i64,
}

/// Why a repo is followed by `live_tail`.
#[derive(Clone, Debug, Default)]
struct Relevance {
    /// The forum keys it is synced for.
    fks: BTreeSet<String>,
    /// An authority of a followed forum.
    authority: bool,
    /// A logged-in account.
    own: bool,
}

/// A jitter seed for a repo, stable per DID and check (as `schedule::due_polls` asks).
fn jitter(did: &str, row: Option<&RepoRow>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in did.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h ^ row.map_or(0, |r| r.checked_at as u64)
}

impl<H: Host> Engine<H> {
    /// `live_tail()` (long-lived, `engine-api.md` §3.3): runs [`Engine::live_step`] rounds,
    /// sleeping between them, until [`Engine::live_stop`]. Start it once (before or after
    /// login); the host drives the future. A `live_tail` started after a stop runs normally.
    pub async fn live_tail(&self) -> Result<(), EngineError> {
        let generation = self.rt.live_gen.load(Ordering::SeqCst);
        let mut woken = self.rt.live_waker();
        let running = || self.rt.live_gen.load(Ordering::SeqCst) == generation;
        let mut source: Option<LiveSource<H>> = None;
        let mut proof_batch: Option<crate::indexer::LiveProofBatch> = None;
        let mut pending_actions: Vec<LiveAction> = Vec::new();
        let mut wait = 1;
        while running() {
            let assisted = self.sources().await? == super::Sources::Assisted;
            if !assisted && let Some(mut old) = source.take() {
                old.stop().await;
            }
            let mut actions = Vec::new();
            if assisted {
                let live = source.get_or_insert_with(|| {
                    LiveSource::new(
                        self.shared_host(),
                        LiveConfig::new(
                            self.config.jetstream.clone(),
                            self.config.platform == super::Platform::Browser,
                        ),
                    )
                });
                // An unacknowledged proof frame is retained locally until its timer fires. Do
                // not poll `next` in a busy loop: it intentionally replays that frame at once.
                if pending_actions.is_empty() {
                    let cx = self.live_context().await?;
                    let retry = live.retry_in_ms();
                    let next = async {
                        if retry > 0 {
                            // Do not let an immediately-ready Waiting result starve the stop waker
                            // while a fake host clock is parked during reconnect backoff.
                            futures::future::pending().await
                        } else {
                            live.next(&cx).await
                        }
                    };
                    futures::pin_mut!(next);
                    let sleep = self.host.sleep(u64::try_from(wait.max(1)).unwrap_or(1));
                    futures::pin_mut!(sleep);
                    let wake = woken.next();
                    futures::pin_mut!(wake);
                    let timeout = futures::future::select(sleep, wake);
                    futures::pin_mut!(timeout);
                    if let futures::future::Either::Left((result, _)) =
                        futures::future::select(next, timeout).await
                    {
                        match result {
                            Ok(SourceStatus::Actions(a)) => actions = a,
                            Ok(SourceStatus::Stopped) => break,
                            Ok(SourceStatus::Waiting | SourceStatus::LeaderUnavailable) => {}
                            Err(e) => tracing::warn!(error = %e, "live source round failed"),
                        }
                    }
                } else {
                    let sleep = self.host.sleep(u64::try_from(wait.max(1)).unwrap_or(1));
                    futures::pin_mut!(sleep);
                    futures::future::select(sleep, woken.next()).await;
                }
            } else {
                let sleep = self.host.sleep(u64::try_from(wait.max(1)).unwrap_or(1));
                futures::pin_mut!(sleep);
                futures::future::select(sleep, woken.next()).await;
            }
            if !running() {
                break;
            }
            if !actions.is_empty() {
                pending_actions = actions;
            }
            if !pending_actions.is_empty() {
                let has_proofs = pending_actions
                    .iter()
                    .any(|a| matches!(a, LiveAction::PointFetch { .. }));
                if has_proofs && proof_batch.as_ref().is_none_or(|b| b.is_empty()) {
                    let mut batch =
                        crate::indexer::LiveProofBatch::new(self.host.now_ms(), self.live_random());
                    for action in &pending_actions {
                        if let LiveAction::PointFetch { uri, .. } = action
                            && let Ok(uri) = AtUri::parse(uri)
                        {
                            batch.push(uri);
                        }
                    }
                    proof_batch = Some(batch);
                }
                let due = proof_batch
                    .as_ref()
                    .is_none_or(|b| b.is_empty() || b.is_due(self.host.now_ms()));
                if due {
                    let result = if has_proofs {
                        self.apply_batched_live_actions(
                            pending_actions.clone(),
                            proof_batch.as_mut().expect("proof batch created"),
                        )
                        .await
                    } else {
                        self.apply_live_actions(pending_actions.clone()).await
                    };
                    match result {
                        Ok(()) => {
                            let acknowledged = if let Some(live) = &mut source {
                                match live.acknowledge().await {
                                    Ok(()) => true,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "acknowledging live source actions failed");
                                        false
                                    }
                                }
                            } else {
                                true
                            };
                            if acknowledged {
                                pending_actions.clear();
                                proof_batch = None;
                            }
                        }
                        Err(e) => {
                            // The actionable frame stays unacknowledged. Its old cursor and all
                            // live actions are replayed; no failed proof batch can advance it.
                            tracing::warn!(error = %e, "applying live source actions failed");
                        }
                    }
                }
            }
            let polling = !assisted || source.as_ref().is_none_or(LiveSource::polling_enabled);
            wait = match self.live_step_with_polling(polling).await {
                Ok(r) => r.next_ms,
                Err(e) => {
                    tracing::warn!(error = %e, "live tail round failed");
                    outbox::ERROR_WAIT_MS
                }
            };
            if let Some(batch) = &proof_batch
                && !pending_actions.is_empty()
            {
                wait = wait.min(
                    batch
                        .flush_at_ms()
                        .saturating_sub(self.host.now_ms())
                        .max(1),
                );
            }
            if let Some(live) = &source {
                let retry = live.retry_in_ms();
                if retry > 0 {
                    wait = wait.min(retry);
                }
            }
        }
        if let Some(mut live) = source {
            live.stop().await;
        }
        Ok(())
    }

    /// Stops the running `live_tail`s (they return after their current round).
    pub fn live_stop(&self) {
        self.rt.live_stop();
    }

    /// `background_verify()` (`engine-api.md` §3.3): `verify = Proof` is Reserved (P2,
    /// `sync.md` §2.3), so in v1 it has nothing to do and returns at once.
    pub async fn background_verify(&self) -> Result<(), EngineError> {
        Ok(())
    }

    /// The repos `live_tail` keeps fresh: the authorities and participants of the followed
    /// forums, and the accounts' own repos.
    async fn relevant_repos(&self) -> Result<BTreeMap<String, Relevance>, EngineError> {
        let mut rel: BTreeMap<String, Relevance> = BTreeMap::new();
        for key in self.followed_keys().await? {
            for did in crate::discovery::state::disc_rows(self.host.cache(), &key.fk)
                .await
                .map_err(store_error)?
                .into_keys()
            {
                rel.entry(did).or_default().fks.insert(key.fk.clone());
            }
            let l = match self.loaded(&key).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::debug!(forum = %key.uri, error = %e, "live tail: forum not loaded");
                    continue;
                }
            };
            let f = &l.model.view;
            for a in f.authorities.keys() {
                let r = rel.entry(a.clone()).or_default();
                r.fks.insert(key.fk.clone());
                r.authority = true;
            }
            for p in &f.participation.p {
                rel.entry(p.clone()).or_default().fks.insert(key.fk.clone());
            }
        }
        for a in self.accounts().await? {
            rel.entry(a.did).or_default().own = true;
        }
        Ok(rel)
    }

    /// Every actual DID-document cache row, including repos unrelated to the current fold.
    async fn known_identity_dids(&self) -> Result<BTreeSet<String>, EngineError> {
        let mut out = BTreeSet::new();
        let prefix = schema::prefix("id", &["did"]);
        let mut range = Some(KeyRange::prefix(&prefix));
        while let Some(r) = range.take() {
            let page = self
                .host
                .cache()
                .scan(r.clone(), Dir::Forward, 1_000)
                .await?;
            let full = page.len() == 1_000;
            for (k, _) in &page {
                let Ok(parts) = key::decode(k) else { continue };
                let Some(Part::Bytes(did)) = parts.get(2) else {
                    continue;
                };
                let Ok(did) = std::str::from_utf8(did) else {
                    continue;
                };
                if Did::parse(did).is_ok() {
                    out.insert(did.to_owned());
                }
            }
            if full && let Some((last, _)) = page.last() {
                range = Some(KeyRange {
                    start: std::ops::Bound::Excluded(last.clone()),
                    end: r.end,
                });
            }
        }
        Ok(out)
    }

    /// Builds live filtering state from all identity/discovery rows. Fold completeness is not a
    /// substitute for having finished discovery: an incomplete fold may still have a known D set.
    async fn live_context(&self) -> Result<LiveContext, EngineError> {
        let rel = self.relevant_repos().await?;
        let followed = self.followed_keys().await?;
        let mut cx = LiveContext {
            known_dids: self.known_identity_dids().await?,
            ..LiveContext::default()
        };
        cx.known_dids.extend(rel.keys().cloned());
        cx.filter_dids.extend(rel.keys().cloned());
        for (did, relevance) in &rel {
            if relevance.own {
                cx.own_dids.insert(did.clone());
            }
            if relevance.authority {
                cx.authorities.insert(did.clone());
            }
            cx.forums.extend(relevance.fks.iter().cloned());
        }
        let mut complete = !followed.is_empty();
        for key in followed {
            cx.declarations
                .insert((key.owner.to_string(), key.slug.clone()));
            let disc = crate::discovery::state::disc_rows(self.host.cache(), &key.fk)
                .await
                .map_err(store_error)?;
            cx.filter_dids.extend(disc.keys().cloned());
            cx.newcomer_dids.extend(disc.keys().cloned());
            complete &= crate::discovery::state::discovery_complete(self.host.cache(), &key.fk)
                .await
                .map_err(store_error)?;
            if let Ok(loaded) = self.loaded(&key).await {
                let viewer_authority = cx
                    .own_dids
                    .iter()
                    .any(|did| loaded.model.view.authorities.contains_key(did));
                if loaded.model.view.policy.membership
                    == lessdugin_lexicon::values::MembershipMode::Open
                    || viewer_authority
                {
                    cx.newcomer_forums.insert(key.fk.clone());
                }
            }
        }
        cx.filter_dids.sort();
        cx.filter_dids.dedup();
        cx.participants_known = complete;
        Ok(cx)
    }

    fn live_random(&self) -> u64 {
        let mut b = [0u8; 8];
        self.host.fill_random(&mut b);
        u64::from_le_bytes(b)
    }

    fn live_item(&self, did: &str, priority: Priority, reason: Reason) -> DirtyItem {
        let now = self.host.now_ms();
        DirtyItem::new(
            did,
            priority,
            reason,
            ready_time(priority, now, self.live_random(), None),
        )
    }

    /// Applies one actionable frame after its live proof delay. Proof-backed point actions are
    /// grouped by forum and hydrated in <=25-URI requests. If no consented indexer advertises
    /// proofs, the exact original actions go through the ordinary PDS dirty queue. Any failed
    /// hydration or PDS fallback returns an error, so the caller cannot acknowledge the frame.
    async fn apply_batched_live_actions(
        &self,
        actions: Vec<LiveAction>,
        batch: &mut crate::indexer::LiveProofBatch,
    ) -> Result<(), EngineError> {
        let now = self.host.now_ms();
        while !batch.is_empty() {
            let _ = batch.take(now);
        }

        let mut ordinary = Vec::new();
        let mut grouped: BTreeMap<String, Vec<LiveAction>> = BTreeMap::new();
        for action in actions {
            let LiveAction::PointFetch { uri, .. } = &action else {
                ordinary.push(action);
                continue;
            };
            let parsed = AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
            let fk = scope_of(parsed.collection().as_str(), parsed.rkey().as_str());
            if let Some(forum) = self.rt.forum_of_fk(fk) {
                grouped.entry(forum).or_default().push(action);
            } else {
                ordinary.push(action);
            }
        }

        for (forum, actions) in grouped {
            let uris = actions
                .iter()
                .filter_map(|a| match a {
                    LiveAction::PointFetch { uri, .. } => AtUri::parse(uri).ok(),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            match self.hydrate_indexer_records(&forum, &uris).await? {
                Some(report)
                    if report.items.len() == uris.len()
                        && report.items.iter().all(|i| {
                            matches!(
                                i.status,
                                crate::indexer::ProofStatus::Admitted
                                    | crate::indexer::ProofStatus::Absent
                            ) || i.point_fetched
                        }) =>
                {
                    self.rt.bus.changed([Topic::Forum { uri: forum.clone() }]);
                }
                Some(report) => {
                    return Err(EngineError::internal(format!(
                        "live proof hydration and PDS fallback did not apply every action: {report:?}"
                    )));
                }
                None => ordinary.extend(actions),
            }
        }
        self.apply_live_actions(ordinary).await
    }

    /// Applies live hints to the existing dirty queue and identity/account APIs. Candidate rows
    /// remain candidates until the normal forum sync confirms their membership at the PDS.
    async fn apply_live_actions(&self, actions: Vec<LiveAction>) -> Result<(), EngineError> {
        if actions.is_empty() {
            return Ok(());
        }
        let sy = self.syncer();
        for action in actions {
            match action {
                LiveAction::PointFetch {
                    did,
                    uri,
                    rev,
                    cid,
                    priority,
                    first_seen_us,
                } => {
                    let parsed =
                        AtUri::parse(&uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
                    sy.note_observed_change(crate::sync::ObservedChange {
                        fk: crate::store::schema::scope_of(
                            parsed.collection().as_str(),
                            parsed.rkey().as_str(),
                        )
                        .to_owned(),
                        uri: uri.clone(),
                        did: did.clone(),
                        rev: rev.clone(),
                        cid: cid.clone(),
                        observed_us: self.host.now_ms().saturating_mul(1000),
                    });
                    if let Some(cid) = &cid {
                        sy.note_hint_first_seen(&uri, cid, first_seen_us);
                    }
                    let stats = Arc::new(Stats::capped(Some(8)));
                    let session = sy.session(priority, stats);
                    sy.mark_hint(&session, &parsed, &rev).await?;
                    let at = ready_time(priority, self.host.now_ms(), self.live_random(), None);
                    sy.enqueue(
                        DirtyItem::point(did, &uri, priority, at)
                            .with_min_rev(rev)
                            .with_mark_hint(&uri, cid),
                    );
                }
                LiveAction::Candidate {
                    did,
                    fk,
                    uri,
                    rev,
                    cid,
                    first_seen_us,
                } => {
                    sy.note_observed_change(crate::sync::ObservedChange {
                        fk: fk.clone(),
                        uri: uri.clone(),
                        did: did.clone(),
                        rev: rev.clone(),
                        cid: cid.clone(),
                        observed_us: self.host.now_ms().saturating_mul(1000),
                    });
                    let mut rows = crate::discovery::state::disc_rows(self.host.cache(), &fk)
                        .await
                        .map_err(store_error)?;
                    let now = self.host.now_ms();
                    rows.entry(did.clone())
                        .or_insert_with(|| crate::discovery::DiscRow::new("hint:jetstream", now))
                        .add_src("hint:jetstream");
                    crate::discovery::state::put_disc_rows(self.host.cache(), &fk, &rows)
                        .await
                        .map_err(store_error)?;
                    if let Some(cid) = &cid {
                        sy.note_hint_first_seen(&uri, cid, first_seen_us);
                    }
                    let parsed =
                        AtUri::parse(&uri).map_err(|e| EngineError::invalid_ref(e.to_string()))?;
                    let stats = Arc::new(Stats::capped(Some(8)));
                    let session = sy.session(Priority::P1, stats);
                    sy.mark_hint(&session, &parsed, &rev).await?;
                    let at = ready_time(Priority::P1, now, self.live_random(), None);
                    sy.enqueue(
                        DirtyItem::point(did.clone(), &uri, Priority::P1, at)
                            .with_min_rev(rev)
                            .with_mark_hint(&uri, cid),
                    );
                    // A point fetch admits the membership record but does not confirm the
                    // candidate. Only this scoped discovery pass may change its DiscState.
                    if let Some(forum) = self.rt.forum_of_fk(&fk) {
                        let opts = crate::discovery::SyncOpts {
                            budget_ms: Some(30_000),
                            max_requests: Some(64),
                            newcomers: vec![(did, "jetstream".into())],
                            background: true,
                            ..crate::discovery::SyncOpts::default()
                        };
                        self.sync_forum(&forum, &opts).await?;
                    }
                }
                LiveAction::InvalidateIdentity { did, authority } => {
                    self.invalidate_identity(&did).await?;
                    if authority {
                        sy.enqueue(self.live_item(&did, Priority::P1, Reason::Hint));
                    }
                }
                LiveAction::AccountStatus { did, .. } => {
                    let Ok(did) = Did::parse(&did) else { continue };
                    let stats = Arc::new(Stats::capped(None));
                    let session = sy.session(Priority::P1, stats);
                    sy.check_account_status(&session, &did).await?;
                }
                LiveAction::Resync { did } => {
                    sy.enqueue(self.live_item(&did, Priority::P1, Reason::Resync));
                }
                LiveAction::CatchUp { did } => {
                    sy.enqueue(self.live_item(&did, Priority::P2, Reason::Hint));
                }
                LiveAction::Reconcile(gap) => {
                    tracing::warn!(?gap, "Jetstream cursor gap: scheduling reconcile");
                    for (did, relevance) in self.relevant_repos().await? {
                        if relevance.authority {
                            sy.enqueue(self.live_item(&did, Priority::P1, Reason::Reconcile));
                        } else if !relevance.own {
                            let mut item = self.live_item(&did, Priority::P3, Reason::Reconcile);
                            item.ready_at = self.host.now_ms();
                            sy.enqueue(item);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// One `live_tail` round (see the module docs); also for one-shot use (`tail --once`).
    pub async fn live_step(&self) -> Result<LiveRound, EngineError> {
        self.live_step_with_polling(true).await
    }

    async fn live_step_with_polling(&self, polling: bool) -> Result<LiveRound, EngineError> {
        let mut out = LiveRound::default();
        self.seed_pending().await?;
        let rel = self.relevant_repos().await?;
        let sy = self.syncer();
        let store = RecordStore::new(self.host.cache());
        let now = self.host.now_ms();

        // the rows of the relevant and the queued repos
        let queued = sy.queue_snapshot();
        let mut dids: BTreeSet<&str> = rel.keys().map(String::as_str).collect();
        dids.extend(queued.ordered().iter().map(|i| i.did.as_str()));
        let dids: Vec<&str> = dids.into_iter().collect();
        let rows: BTreeMap<&str, RepoRow> = dids
            .iter()
            .copied()
            .zip(store.repos(&dids).await.map_err(store_error)?)
            .filter_map(|(d, r)| Some((d, r?)))
            .collect();

        // polls and reconciles
        let stats = Arc::new(Stats::capped(None));
        let s = sy.session(Priority::P1, Arc::clone(&stats));
        let polls = if polling {
            schedule::due_polls(
                rel.iter()
                    .filter_map(|(d, r)| Some((d.as_str(), rows.get(d.as_str())?, r.authority))),
                now,
                |d| jitter(d, rows.get(d)),
            )
        } else {
            Vec::new()
        };
        out.polled = polls.len();
        for (did, res) in sy.run_polls(&s, &polls).await {
            if let Err(e) = res {
                tracing::debug!(did = %did, error = %e, "live tail: poll failed");
            }
        }
        let reconciles = schedule::due_reconciles(
            rel.iter().map(|(d, r)| {
                let role = if r.authority {
                    ReconcileRole::Authority
                } else {
                    ReconcileRole::Participant
                };
                (d.as_str(), rows.get(d.as_str()), role)
            }),
            now,
            |d| jitter(d, rows.get(d)),
        );
        out.reconciles = reconciles.len();
        for item in reconciles {
            sy.enqueue(item);
        }

        // the dirty queue: each repo's scopes, and the forums its point fetches touch
        let queued = sy.queue_snapshot();
        let mut scopes: BTreeMap<String, Vec<Scope>> = BTreeMap::new();
        let mut point_fks: BTreeSet<String> = BTreeSet::new();
        for item in queued.ordered() {
            if item.ready_at > now {
                continue;
            }
            if let Action::PointFetch(uris) = item.action() {
                for u in uris {
                    if let Ok(u) = AtUri::parse(&u) {
                        let fk = scope_of(u.collection().as_str(), u.rkey().as_str());
                        if fk != GLOBAL_SCOPE {
                            point_fks.insert(fk.to_owned());
                        }
                    }
                }
                continue;
            }
            let r = rel.get(&item.did);
            let mut fks: BTreeSet<String> = r.map(|r| r.fks.clone()).unwrap_or_default();
            if let Some(row) = rows.get(item.did.as_str()) {
                fks.extend(
                    row.listed
                        .keys()
                        .filter(|k| *k != "global" && *k != "own")
                        .cloned(),
                );
            }
            let mut sc: Vec<Scope> = fks.into_iter().map(Scope::Forum).collect();
            if r.is_some_and(|r| r.own) {
                sc.push(Scope::Own);
            }
            scopes.insert(item.did.clone(), sc);
        }
        let reports = sy
            .run_ready(&s, limits::LIVE_TAIL_BATCH, |d| {
                scopes.get(d).cloned().unwrap_or_default()
            })
            .await;
        out.synced = reports.len();
        let mut changed_fks = point_fks;
        let mut topics: Vec<Topic> = Vec::new();
        for r in &reports {
            if let Some(e) = &r.error {
                tracing::debug!(did = %r.did, error = %e, "live tail: sync failed");
            }
            if r.changed > 0 {
                topics.push(Topic::User { did: r.did.clone() });
                for sc in scopes.get(&r.did).into_iter().flatten() {
                    if let Scope::Forum(fk) = sc {
                        changed_fks.insert(fk.clone());
                    }
                }
            }
            let own = rel.get(&r.did).is_some_and(|x| x.own);
            if own
                && r.outcome == RepoOutcome::Applied
                && let Ok(did) = Did::parse(&r.did)
                && let Err(e) = self.seed_account(&did).await
            {
                tracing::warn!(account = %r.did, error = %e, "re-seeding followed forums");
            }
        }
        for fk in &changed_fks {
            if let Some(f) = self.rt.forum_of_fk(fk) {
                self.rt.invalidate(&f);
                out.changed.push(f.clone());
                topics.push(Topic::Forum { uri: f });
            }
        }
        self.rt.bus.changed(topics);

        // spilled bodies views left pending (lfm-1.md §1.3): one that arrives is announced, so a
        // client shows it without being asked again
        let arrived = self.retry_spilled_bodies(&self.rt.renders).await;
        if !arrived.is_empty() {
            self.rt.bus.changed(arrived);
        }

        // handles of shown authors
        out.verified = self.verify_handles(limits::HANDLE_VERIFY_BATCH).await;

        // expired tombstones, daily
        let gc_due = super::models::lock(&self.rt.gc_at)
            .is_none_or(|t| now.saturating_sub(t) >= limits::TOMBSTONE_GC_EVERY_MS);
        if gc_due {
            *super::models::lock(&self.rt.gc_at) = Some(now);
            match store
                .gc_tombstones(now, limits::TOMBSTONE_MIN_RETENTION_MS)
                .await
            {
                Ok(n) => out.gc = n,
                Err(e) => tracing::warn!(error = %e, "tombstone GC failed"),
            }
        }

        // the next round: the next ready item or due poll (from the rows as they are now), at
        // most the idle time
        let now = self.host.now_ms();
        let mut next = now.saturating_add(limits::LIVE_TAIL_IDLE_MS);
        if let Some(t) = sy.queue_snapshot().next_ready_at() {
            next = next.min(t);
        }
        let rel_dids: Vec<&str> = rel.keys().map(String::as_str).collect();
        let rows = store.repos(&rel_dids).await.map_err(store_error)?;
        for ((d, r), row) in rel.iter().zip(rows) {
            if polling && let Some(row) = row {
                let at = schedule::next_poll_at(&row, r.authority, now, jitter(d, Some(&row)));
                // a poll that is still due (it failed) waits a little, not a whole idle time
                next = next.min(at.max(now.saturating_add(1_000)));
            }
        }
        // a pending spilled body is asked for again when its retry time comes
        for (_, p) in self.rt.renders.spilled_pending() {
            next = next.min(p.at_ms.max(now.saturating_add(1_000)));
        }
        out.next_ms = next.saturating_sub(now).max(1);
        Ok(out)
    }

    /// Verifies the handles of up to `n` queued authors (`sync.md` §3.3; a cached state younger
    /// than its TTL is kept) and emits `Changed` for them and every cached forum if any handle
    /// state changed. Returns how many were checked.
    pub async fn verify_handles(&self, n: usize) -> usize {
        let dids = self.rt.take_verify(n);
        let mut changed = Vec::new();
        for d in &dids {
            let Ok(did) = Did::parse(d) else { continue };
            let before = self.resolver.cached(&did).await.map(|i| i.actor_view());
            match self.resolver.resolve_did(&did).await {
                Ok(id) => {
                    if before.as_ref() != Some(&id.actor_view()) {
                        changed.push(Topic::User { did: d.clone() });
                    }
                }
                Err(e) => tracing::debug!(did = %d, error = %e, "handle verification failed"),
            }
        }
        if !changed.is_empty() {
            changed.extend(
                self.rt
                    .loaded_forums()
                    .into_iter()
                    .map(|uri| Topic::Forum { uri }),
            );
            self.rt.bus.changed(changed);
        }
        dids.len()
    }
}

#[cfg(test)]
mod context_tests {
    use serde::Serialize;

    use super::*;
    use crate::discovery::{DiscRow, ForumKey};
    use crate::engine::EngineConfig;
    use crate::host::{
        BlobHandle, HostError, HttpRequest, HttpResponse, KvOp, WebSocket, WsMessage,
    };
    use crate::store::{mem::MemKv, row};

    #[derive(Clone, Default)]
    struct TestHost {
        cache: MemKv,
        state: MemKv,
    }

    struct TestWs;

    impl WebSocket for TestWs {
        async fn next(&mut self) -> Option<Result<WsMessage, HostError>> {
            None
        }

        async fn send_text(&mut self, _text: String) -> Result<(), HostError> {
            Ok(())
        }

        async fn close(&mut self) {}
    }

    impl Host for TestHost {
        type Kv = MemKv;
        type Ws = TestWs;
        type Lock = ();

        fn cache(&self) -> &MemKv {
            &self.cache
        }

        fn state(&self) -> &MemKv {
            &self.state
        }

        async fn http(&self, _req: HttpRequest) -> Result<HttpResponse, HostError> {
            Err(HostError::Unsupported("test".into()))
        }

        async fn fetch_blob(
            &self,
            _pds: url::Url,
            _did: Did,
            _cid: String,
            _max_bytes: u64,
        ) -> Result<BlobHandle, HostError> {
            Err(HostError::Unsupported("test".into()))
        }

        async fn websocket(&self, _url: url::Url, _max: u32) -> Result<TestWs, HostError> {
            Err(HostError::Unsupported("test".into()))
        }

        async fn resolve_txt_system(
            &self,
            _name: String,
        ) -> Result<Option<Vec<String>>, HostError> {
            Ok(None)
        }

        async fn lock(&self, _name: &str, _wait: bool) -> Result<Option<()>, HostError> {
            Ok(Some(()))
        }

        async fn sleep(&self, _ms: u64) {}

        fn now_ms(&self) -> i64 {
            1
        }

        fn fill_random(&self, buf: &mut [u8]) {
            buf.fill(0);
        }
    }

    #[derive(Serialize)]
    struct FollowRow {
        v: u32,
        at: i64,
        seeded: bool,
    }

    #[test]
    fn context_uses_all_identity_rows_and_discovery_not_fold_completeness() {
        futures::executor::block_on(async {
            let host = TestHost::default();
            let engine = Engine::new(host.clone(), EngineConfig::default());
            let owner = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
            let key = ForumKey::new(&owner, "meta").unwrap();
            let follow = FollowRow {
                v: 1,
                at: 1,
                seeded: false,
            };
            host.state()
                .write(vec![KvOp::Put(
                    crate::key!("loc", "follow", key.uri.as_str()),
                    row::encode(&follow).unwrap(),
                )])
                .await
                .unwrap();
            let candidate = "did:plc:candidate".to_owned();
            let rows =
                BTreeMap::from([(candidate.clone(), DiscRow::new("hint:test", host.now_ms()))]);
            crate::discovery::state::put_disc_rows(host.cache(), &key.fk, &rows)
                .await
                .unwrap();
            let unrelated = "did:plc:unrelated";
            host.cache()
                .write(vec![KvOp::Put(
                    schema::prefix("id", &["did", unrelated]),
                    vec![1],
                )])
                .await
                .unwrap();

            let cx = engine.live_context().await.unwrap();
            assert!(cx.known_dids.contains(unrelated));
            assert!(cx.filter_dids.contains(&candidate));
            assert!(!cx.participants_known);

            crate::discovery::state::mark_discovery_complete(host.cache(), &key.fk, host.now_ms())
                .await
                .unwrap();
            let cx = engine.live_context().await.unwrap();
            assert!(cx.participants_known, "no complete fold was loaded");
            assert!(cx.filter_dids.contains(&candidate));
        });
    }
}
