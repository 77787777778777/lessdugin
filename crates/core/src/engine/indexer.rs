// SPDX-License-Identifier: AGPL-3.0-or-later
//! Engine entry points for consented assisted indexers. Selection precedes every indexer call;
//! direct mode returns `None` without even constructing a network client.

use std::collections::BTreeSet;

use lessdugin_atp::syntax::{AtUri, Did};

use crate::discovery::forum_ref::ForumKey;
use crate::discovery::state::ForumState;
use crate::host::Host;
use crate::indexer::client::endpoint_allowed;
use crate::indexer::contract::{ForumsPage, OrderingHint, ParticipantsPage};
use crate::indexer::{AuditReport, ChangePollReport, Client, Consent, EndpointPolicy, ProofReport};
use crate::sync::net::Priority;
use crate::sync::queue::DirtyItem;
use crate::sync::{Reason, Session};

use super::{Engine, EngineError, Network, Sources};

fn policy(network: &Network) -> EndpointPolicy {
    match network {
        Network::Production => EndpointPolicy::Production,
        Network::Devnet { .. } => EndpointPolicy::Devnet,
    }
}

impl<H: Host> Engine<H> {
    async fn consents(&self, forum: Option<&ForumKey>) -> Result<Vec<Consent>, EngineError> {
        let mut out = Vec::new();
        for c in &self.config.indexers {
            out.push(Consent {
                url: c.url.clone(),
                did: None,
                forum: None,
            });
        }
        let raw = self.stored_settings().await?;
        if let Some(text) = raw.get("indexers") {
            let stored: Vec<crate::dto::IndexerConsent> = serde_json::from_str(text)
                .map_err(|e| EngineError::invalid_input(format!("stored indexer consent: {e}")))?;
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
        out.retain(|c| match (forum, &c.forum) {
            (_, None) => true,
            (Some(key), Some(consented)) => key.uri.as_str() == consented,
            (None, Some(_)) => false,
        });
        let mut seen = BTreeSet::new();
        out.retain(|c| {
            seen.insert((
                c.url.as_str().trim_end_matches('/').to_owned(),
                c.did.as_ref().map(ToString::to_string),
            ))
        });
        Ok(out)
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
        self.poll_indexer_changes_resolved(&key).await
    }

    pub(crate) async fn poll_indexer_changes_resolved(
        &self,
        key: &ForumKey,
    ) -> Result<Option<ChangePollReport>, EngineError> {
        let Some((client, session)) = self.selected(Some(key), false).await? else {
            return Ok(None);
        };
        if !client.supports("getChanges") {
            return Ok(None);
        }
        let opened = self.open_key(key, false).await?;
        let state = ForumState::load(
            self.host.cache(),
            key,
            &opened.decl,
            self.host.now_ms().saturating_mul(1000),
        )
        .await
        .map_err(super::models::store_error)?;
        let dids = state
            .completeness_set()
            .iter()
            .filter_map(|d| Did::parse(d).ok())
            .collect::<Vec<_>>();
        Ok(Some(
            crate::indexer::changes::poll_changes(
                &self.syncer(),
                &client,
                &session,
                Some(&key.uri),
                &dids,
            )
            .await?,
        ))
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
        let report =
            crate::indexer::proofs::hydrate_visible(&self.syncer(), &client, &session, uris)
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
                if Did::parse(&hint.did).is_err() {
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
