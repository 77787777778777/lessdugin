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
use crate::host::Host;
use crate::limits;
use crate::outbox;
use crate::store::RepoRow;
use crate::store::records::RecordStore;
use crate::store::schema::{GLOBAL_SCOPE, scope_of};
use crate::sync::schedule::{self, ReconcileRole};
use crate::sync::{Action, Priority, RepoOutcome, Scope, Stats};

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
        while running() {
            let wait = match self.live_step().await {
                Ok(r) => r.next_ms,
                Err(e) => {
                    tracing::warn!(error = %e, "live tail round failed");
                    outbox::ERROR_WAIT_MS
                }
            };
            if !running() {
                break;
            }
            let sleep = self.host.sleep(u64::try_from(wait.max(1)).unwrap_or(1));
            futures::pin_mut!(sleep);
            futures::future::select(sleep, woken.next()).await;
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

    /// One `live_tail` round (see the module docs); also for one-shot use (`tail --once`).
    pub async fn live_step(&self) -> Result<LiveRound, EngineError> {
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
        let polls = schedule::due_polls(
            rel.iter()
                .filter_map(|(d, r)| Some((d.as_str(), rows.get(d.as_str())?, r.authority))),
            now,
            |d| jitter(d, rows.get(d)),
        );
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
            if let Some(row) = row {
                let at = schedule::next_poll_at(&row, r.authority, now, jitter(d, Some(&row)));
                // a poll that is still due (it failed) waits a little, not a whole idle time
                next = next.min(at.max(now.saturating_add(1_000)));
            }
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
