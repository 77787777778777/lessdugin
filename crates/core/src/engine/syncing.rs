// SPDX-License-Identifier: AGPL-3.0-or-later
//! Sync operations (`engine-api.md` §3.3): `sync_forum` and `sync_thread`. Both drop the
//! forum's model and emit `SyncProgress` and `Changed` when done.

use std::collections::BTreeSet;

use futures::future::join_all;
use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::collection::nsid;

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::{self};
use crate::dto::{self, DtoVersion, Event, SyncIssue, SyncReport, Topic};
use crate::host::Host;
use crate::store::records::RecordStore;
use crate::store::schema::{GLOBAL_SCOPE, scope_of};
use crate::sync::{Reason, RepoOutcome, SyncRequest};

use super::models::store_error;
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

impl<H: Host> Engine<H> {
    /// `sync_forum(uri, SyncOpts)` (`engine-api.md` §3.3): opens the forum and syncs its
    /// authorities and confirmed participants in the order of `sync.md` §4.5. Emits
    /// `SyncProgress { done: reposComplete, total: reposTotal }` and `Changed [Forum]`.
    pub async fn sync_forum(
        &self,
        forum: &str,
        opts: &impl ForumSyncOpts,
    ) -> Result<SyncReport, EngineError> {
        let opts = opts.discovery_opts();
        let fs = self.syncer().sync_forum(forum, &opts).await?;
        let f = fs.forum.key.uri.to_string();
        self.rt.register(&fs.forum.key);
        self.rt.invalidate(&f);
        self.rt.bus.emit(Event::SyncProgress {
            forum: f.clone(),
            done: fs.report.repos_complete,
            total: fs.report.repos_total,
        });
        self.rt.bus.changed([Topic::Forum { uri: f }]);
        Ok(fs.report)
    }

    /// The forum of a record uri in a forum's key range: from the registry, else the cached
    /// `("forum", fk)` row, else the record's own `forum` field.
    pub(crate) async fn forum_of(&self, uri: &AtUri) -> Result<ForumKey, EngineError> {
        let coll = uri.collection().as_str();
        let rkey = uri.rkey().as_str();
        if coll == nsid::FORUM {
            return ForumKey::of_uri(uri).map_err(|e| EngineError::invalid_ref(e.to_string()));
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
        if f.is_none() {
            f = store
                .get(uri)
                .await
                .map_err(store_error)?
                .and_then(|r| r.value_json().ok())
                .and_then(|v| v.get("forum")?.as_str().map(str::to_owned));
        }
        let f = f.ok_or_else(|| {
            EngineError::not_found(format!("the forum of {uri} is not known; open it first"))
        })?;
        let key = AtUri::parse(&f)
            .ok()
            .and_then(|u| ForumKey::of_uri(&u).ok())
            .filter(|k| k.fk == fk)
            .ok_or_else(|| EngineError::not_found(format!("the forum of {uri} is not known")))?;
        self.rt.register(&key);
        Ok(key)
    }

    /// `sync_thread(post_uri, SyncOpts)` (`engine-api.md` §3.3): a P0 point fetch of the post,
    /// then the forum ranges of the thread's participants (the post's author is confirmed
    /// first unless already a participant). `budget_ms`, `max_requests` and `full` apply.
    pub async fn sync_thread(
        &self,
        post: &str,
        opts: &dto::SyncOpts,
    ) -> Result<SyncReport, EngineError> {
        let root =
            AtUri::parse(post).map_err(|e| EngineError::invalid_ref(format!("{post:?}: {e}")))?;
        if root.collection().as_str() != nsid::POST {
            return Err(EngineError::invalid_ref(format!("{post} is not a post")));
        }
        let key = self.forum_of(&root).await?;
        let s = self.p0_session(opts.max_requests, opts.budget_ms);
        let sy = self.syncer();
        let mut errors: Vec<SyncIssue> = Vec::new();
        if let Err(e) = sy.point_fetch(&s, &root, None).await {
            errors.push(SyncIssue {
                did: Some(root.did().to_string()),
                host: None,
                code: e.code().to_owned(),
                message: e.to_string(),
            });
        }
        // the model must see what the point fetch stored
        self.rt.invalidate(key.uri.as_str());
        let l = self.loaded(&key).await?;
        let m = &l.model;
        let mut dids: BTreeSet<String> = BTreeSet::new();
        dids.insert(root.did().to_string());
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
        let reqs: Vec<SyncRequest> = dids
            .iter()
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
            })
            .collect();
        let reports = join_all(reqs.iter().map(|r| sy.sync_repo(&s, r))).await;
        let mut complete = 0u32;
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
                errors.push(SyncIssue {
                    did: Some(r.did.clone()),
                    host: r.pds.clone(),
                    code: e.code().to_owned(),
                    message: e.to_string(),
                });
            }
            errors.extend(r.issues.iter().map(|(code, message)| SyncIssue {
                did: Some(r.did.clone()),
                host: r.pds.clone(),
                code: code.clone(),
                message: message.clone(),
            }));
        }
        let total = reports
            .iter()
            .filter(|r| r.outcome != RepoOutcome::Rejected)
            .count() as u32;
        let stats = s.stats();
        let f = key.uri.to_string();
        self.rt.invalidate(&f);
        let report = SyncReport {
            dto: DtoVersion::default(),
            forum: f.clone(),
            repos_total: total,
            repos_complete: complete,
            repos_stale: stale,
            repos_unavailable: unavailable,
            requests: stats.requests(),
            bytes: stats.bytes(),
            changed,
            partial: complete < total || stats.exhausted(),
            degraded: l.completeness.degraded,
            errors,
        };
        self.rt.bus.emit(Event::SyncProgress {
            forum: f.clone(),
            done: complete,
            total,
        });
        self.rt.bus.changed([
            Topic::Forum { uri: f },
            Topic::Thread {
                uri: root.to_string(),
            },
        ]);
        Ok(report)
    }
}
