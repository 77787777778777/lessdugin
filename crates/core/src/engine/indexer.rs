// SPDX-License-Identifier: AGPL-3.0-or-later
//! Engine entry points for consented assisted indexers. Selection precedes every indexer call;
//! direct mode returns `None` without even constructing a network client.

use std::collections::BTreeSet;

use lessdugin_atp::syntax::{AtUri, Did};

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::state::{self, DiscState, ForumState};
use crate::host::Host;
use crate::indexer::changes::ChangeScope;
use crate::indexer::client::endpoint_allowed;
use crate::indexer::contract::{ForumsPage, OrderingHint, ParticipantsPage};
use crate::indexer::{AuditReport, ChangePollReport, Client, Consent, EndpointPolicy, ProofReport};
use crate::sync::net::Priority;
use crate::sync::queue::DirtyItem;
use crate::sync::{Reason, Session};

use super::{Engine, EngineError, IndexerMode, Network, Sources};

fn policy(network: &Network) -> EndpointPolicy {
    match network {
        Network::Production => EndpointPolicy::Production,
        Network::Devnet { .. } => EndpointPolicy::Devnet,
    }
}

impl<H: Host> Engine<H> {
    async fn consents(&self, forum: Option<&ForumKey>) -> Result<Vec<Consent>, EngineError> {
        if self.config.indexer_mode == IndexerMode::None {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for c in &self.config.indexers {
            out.push(Consent {
                url: c.url.clone(),
                did: None,
                forum: None,
            });
        }
        if self.config.stored_indexer_consent {
            let raw = self.stored_settings().await?;
            if let Some(text) = raw.get("indexers") {
                let stored: Vec<crate::dto::IndexerConsent> =
                    serde_json::from_str(text).map_err(|e| {
                        EngineError::invalid_input(format!("stored indexer consent: {e}"))
                    })?;
                for c in stored {
                    let url = url::Url::parse(&c.url)
                        .map_err(|e| EngineError::invalid_input(format!("indexer URL: {e}")))?;
                    let did = c
                        .did
                        .as_deref()
                        .map(Did::parse)
                        .transpose()
                        .map_err(|e| EngineError::invalid_input(format!("indexer DID: {e}")))?;
                    out.push(Consent {
                        url,
                        did,
                        forum: c.forum,
                    });
                }
            }
        }
        out.retain(|c| match (forum, &c.forum) {
            (_, None) => true,
            (Some(key), Some(consented)) => key.uri.as_str() == consented,
            (None, Some(_)) => false,
        });
        // Forum-scoped consent is admitted only for a usable endpoint in that forum's
        // declaration. A stored global consent remains global and never depends on it.
        if let Some(key) = forum
            && out.iter().any(|c| c.forum.is_some())
        {
            let loaded = self.loaded(key).await?;
            let declared = lessdugin_lexicon::validity::entry::usable_indexers(&loaded.model.decl)
                .into_iter()
                .map(|e| (e.url.clone(), e.did.clone()))
                .collect::<Vec<_>>();
            out.retain(|c| {
                c.forum.is_none()
                    || declared.iter().any(|(url, _)| {
                        url::Url::parse(url).is_ok_and(|u| {
                            u.as_str().trim_end_matches('/') == c.url.as_str().trim_end_matches('/')
                        })
                    })
            });
            for c in &mut out {
                if c.forum.is_some()
                    && let Some((_, did)) = declared.iter().find(|(url, _)| {
                        url::Url::parse(url).is_ok_and(|u| {
                            u.as_str().trim_end_matches('/') == c.url.as_str().trim_end_matches('/')
                        })
                    })
                {
                    c.did = did.clone();
                }
            }
        }
        let mut seen = BTreeSet::new();
        out.retain(|c| {
            seen.insert((
                c.url.as_str().trim_end_matches('/').to_owned(),
                c.did.as_ref().map(ToString::to_string),
            ))
        });
        Ok(out)
    }

    /// Validated `discovery.indexers` from the cached declaration and this device's consent.
    /// This reads local state only after resolving the declaration; it never contacts an indexer.
    pub async fn forum_indexers(
        &self,
        forum: &str,
    ) -> Result<Vec<crate::dto::ForumIndexer>, EngineError> {
        let key = self.forum_key(forum).await?;
        let loaded = self.loaded(&key).await?;
        let mut consented = BTreeSet::new();
        if self.config.indexer_mode != IndexerMode::None {
            consented.extend(
                self.config
                    .indexers
                    .iter()
                    .map(|c| c.url.as_str().trim_end_matches('/').to_owned()),
            );
            if self.config.stored_indexer_consent {
                let raw = self.stored_settings().await?;
                let stored = raw
                    .get("indexers")
                    .map(|text| {
                        serde_json::from_str::<Vec<crate::dto::IndexerConsent>>(text).map_err(|e| {
                            EngineError::invalid_input(format!("stored indexer consent: {e}"))
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                consented.extend(
                    stored
                        .into_iter()
                        .filter(|c| c.forum.as_deref().is_none_or(|f| f == key.uri.as_str()))
                        .filter_map(|c| {
                            url::Url::parse(&c.url)
                                .ok()
                                .map(|u| u.as_str().trim_end_matches('/').to_owned())
                        }),
                );
            }
        }
        Ok(
            lessdugin_lexicon::validity::entry::usable_indexers(&loaded.model.decl)
                .into_iter()
                .map(|e| crate::dto::ForumIndexer {
                    consented: url::Url::parse(&e.url)
                        .is_ok_and(|u| consented.contains(u.as_str().trim_end_matches('/'))),
                    url: e.url.clone(),
                    did: e.did.as_ref().map(ToString::to_string),
                    note: e.note.clone(),
                })
                .collect(),
        )
    }

    async fn selected(
        &self,
        forum: Option<&ForumKey>,
        require_one: bool,
    ) -> Result<Option<(Client, Session<H>)>, EngineError> {
        if self.sources().await? == Sources::Direct {
            return Ok(None);
        }
        let consents = self.consents(forum).await?;
        if consents.is_empty() {
            return Ok(None);
        }
        if require_one && consents.len() != 1 {
            return Err(EngineError::invalid_input(format!(
                "audit requires exactly one consented indexer for this forum (found {})",
                consents.len()
            )));
        }
        // Do not silently fall through an explicitly configured but policy-invalid endpoint.
        for c in &consents {
            if !endpoint_allowed(&c.url, policy(&self.config.network)) {
                return Err(EngineError::blocked(format!(
                    "consented indexer endpoint {} violates the endpoint policy",
                    c.url
                )));
            }
        }
        let session = self.p0_session(Some(2_000), Some(120_000));
        let client = Client::connect(&session, &consents[0], policy(&self.config.network)).await?;
        Ok(Some((client, session)))
    }

    /// Polls `getChanges` into the existing dirty queue. No indexer head is observed here.
    pub async fn poll_indexer_changes(
        &self,
        forum: &str,
    ) -> Result<Option<ChangePollReport>, EngineError> {
        let key = self.forum_key(forum).await?;
        self.poll_indexer_changes_resolved(&key, None).await
    }

    pub(crate) async fn poll_indexer_changes_resolved(
        &self,
        key: &ForumKey,
        dedup: Option<&mut crate::live::events::Dedup>,
    ) -> Result<Option<ChangePollReport>, EngineError> {
        let Some((client, session)) = self.selected(Some(key), false).await? else {
            return Ok(None);
        };
        if !client.supports("getChanges") {
            return Ok(None);
        }
        let scope = self.change_scope(key).await?;
        Ok(Some(
            crate::indexer::changes::poll_changes(
                &self.syncer(),
                &client,
                &session,
                Some(&key.uri),
                &scope,
                dedup,
            )
            .await?,
        ))
    }

    /// The DIDs whose records an indexer hint for this forum may make the client fetch
    /// (`sync.md` §15.4): A, G and C(F), confirmed participants and the viewer's own accounts.
    async fn change_scope(&self, key: &ForumKey) -> Result<ChangeScope, EngineError> {
        let opened = self.open_key(key, false).await?;
        let state = ForumState::load(
            self.host.cache(),
            key,
            &opened.decl,
            self.host.now_ms().saturating_mul(1000),
        )
        .await
        .map_err(super::models::store_error)?;
        let reconcile = state.completeness_set();
        let own: BTreeSet<String> = self.accounts().await?.into_iter().map(|a| a.did).collect();
        let authorities: BTreeSet<String> = state.roster.authorities.keys().cloned().collect();
        let mut participants = reconcile.clone();
        participants.extend(state.roster.granted.iter().cloned());
        participants.extend(
            state
                .disc
                .iter()
                .filter(|(_, row)| row.state == DiscState::Confirmed)
                .map(|(did, _)| did.clone()),
        );
        participants.extend(own.iter().cloned());
        Ok(ChangeScope {
            fk: key.fk.clone(),
            declaration: (key.owner.to_string(), key.uri.rkey().to_string()),
            participants,
            newcomers: state.roster.is_open() || own.iter().any(|d| authorities.contains(d)),
            authorities,
            reconcile,
        })
    }

    /// Cold-start/live hydration entry point for native and service hosts. Every URI still has a
    /// fresh PDS head check; stale or invalid proofs are point-fetched from the PDS.
    pub async fn hydrate_indexer_records(
        &self,
        forum: &str,
        uris: &[AtUri],
    ) -> Result<Option<ProofReport>, EngineError> {
        if uris.is_empty() {
            return Ok(Some(ProofReport::default()));
        }
        let key = self.forum_key(forum).await?;
        let Some((client, session)) = self.selected(Some(&key), false).await? else {
            return Ok(None);
        };
        if !client.supports("getRecordProofs") {
            return Ok(None);
        }
        let report =
            crate::indexer::proofs::hydrate_visible(&self.syncer(), &client, &session, uris)
                .await?;
        self.rt.invalidate(key.uri.as_str());
        Ok(Some(report))
    }

    /// Hydrates the first visible post page from URI-only ordering hints. Remote text, tallies
    /// and states never enter this path: after proof/PDS admission the caller uses `list_posts`,
    /// which recomputes order and every displayed value with the local fold. Hosts may ignore an
    /// error and continue their ordinary PDS sync. Direct mode returns before any indexer call.
    pub async fn hydrate_indexer_page(
        &self,
        query: &crate::dto::ListPostsQuery,
    ) -> Result<Option<ProofReport>, EngineError> {
        if query.cursor.is_some() {
            return Ok(None);
        }
        let key = self.forum_key(&query.forum).await?;
        let Some((client, session)) = self.selected(Some(&key), false).await? else {
            return Ok(None);
        };
        if !client.supports("listPosts") || !client.supports("getRecordProofs") {
            return Ok(None);
        }
        let sort = match query.sort {
            crate::dto::PostSort::Magic => "magic",
            crate::dto::PostSort::Top => "top",
            crate::dto::PostSort::New => "new",
            crate::dto::PostSort::Old => "old",
            crate::dto::PostSort::RecentComments => "recentComments",
        };
        let limit = if query.limit == 0 {
            crate::limits::LIST_PAGE_DEFAULT
        } else {
            query.limit.min(100)
        };
        let ordering = client
            .post_ordering(&session, &key.uri, sort, limit.max(1), None)
            .await?;
        // Only posts of this forum (its {fk}- range) are hydrated, and once the forum's
        // participants are known, only theirs: the indexer does not choose whom the client
        // contacts. A cold start (§14.3) cannot tell participants yet; one visible page bounds it.
        let scope = if state::discovery_complete(self.host.cache(), &key.fk)
            .await
            .map_err(super::models::store_error)?
        {
            Some(self.change_scope(&key).await?)
        } else {
            None
        };
        let uris: Vec<AtUri> = ordering
            .uris
            .into_iter()
            .filter(|u| {
                u.collection().as_str() == lessdugin_lexicon::collection::nsid::POST
                    && crate::store::schema::in_range(&key.fk, u.rkey().as_str())
                    && scope
                        .as_ref()
                        .is_none_or(|s| s.participants.contains(u.did().as_str()))
            })
            .collect();
        let report =
            crate::indexer::proofs::hydrate_visible(&self.syncer(), &client, &session, &uris)
                .await?;
        self.rt.invalidate(key.uri.as_str());
        Ok(Some(report))
    }

    /// Ordering/provisional-placeholder input only. Display callers must still use local queries.
    pub async fn indexer_post_ordering(
        &self,
        forum: &str,
        sort: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Option<OrderingHint>, EngineError> {
        let key = self.forum_key(forum).await?;
        let Some((client, session)) = self.selected(Some(&key), false).await? else {
            return Ok(None);
        };
        Ok(Some(
            client
                .post_ordering(&session, &key.uri, sort, limit, cursor)
                .await?,
        ))
    }

    pub async fn indexer_thread_ordering(
        &self,
        forum: &str,
        post: &AtUri,
        sort: &str,
    ) -> Result<Option<OrderingHint>, EngineError> {
        let key = self.forum_key(forum).await?;
        let Some((client, session)) = self.selected(Some(&key), false).await? else {
            return Ok(None);
        };
        Ok(Some(client.thread_ordering(&session, post, sort).await?))
    }

    /// Participant DIDs are returned as discovery hints, not as roster/head observations.
    pub async fn indexer_participants(
        &self,
        forum: &str,
        cursor: Option<&str>,
    ) -> Result<Option<ParticipantsPage>, EngineError> {
        let key = self.forum_key(forum).await?;
        let Some((client, session)) = self.selected(Some(&key), false).await? else {
            return Ok(None);
        };
        Ok(Some(
            client
                .list_participants(&session, &key.uri, 500, cursor)
                .await?,
        ))
    }

    pub async fn indexer_forums(
        &self,
        query: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Option<ForumsPage>, EngineError> {
        let Some((client, session)) = self.selected(None, false).await? else {
            return Ok(None);
        };
        Ok(Some(client.list_forums(&session, query, 50, cursor).await?))
    }

    /// Head hints may queue a normal PDS sync, but never populate `RepoRow.headSeen`.
    pub async fn indexer_head_hints(
        &self,
        forum: &str,
        dids: &[Did],
    ) -> Result<Option<usize>, EngineError> {
        let key = self.forum_key(forum).await?;
        let Some((client, session)) = self.selected(Some(&key), false).await? else {
            return Ok(None);
        };
        let mut queued = 0;
        for chunk in dids.chunks(50) {
            let hints = client.get_repo_heads(&session, chunk).await?;
            for hint in hints.heads {
                // A head for a DID that was not asked about is not a hint about this forum.
                if !chunk.iter().any(|d| d.as_str() == hint.did) {
                    continue;
                }
                self.syncer().enqueue(DirtyItem::new(
                    hint.did,
                    Priority::P1,
                    Reason::Hint,
                    self.host.now_ms(),
                ));
                queued += 1;
            }
        }
        Ok(Some(queued))
    }

    /// Manual A1–A5 audit. Findings never demote an indexer automatically.
    pub async fn audit_indexer(&self, forum: &str) -> Result<Option<AuditReport>, EngineError> {
        let key = self.forum_key(forum).await?;
        let Some((client, session)) = self.selected(Some(&key), true).await? else {
            return Err(EngineError::invalid_input(
                "audit requires assisted mode and a consented indexer",
            ));
        };
        Ok(Some(
            crate::indexer::audit::run(self, &client, &session, &key).await?,
        ))
    }
}
