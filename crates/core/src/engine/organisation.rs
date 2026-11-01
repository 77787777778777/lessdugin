// SPDX-License-Identifier: AGPL-3.0-or-later
//! M5 organisation queries and writes: wiki tags and library sequences. Records and bodies come
//! from the synced local model.

use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{StrongRef, WikiRevision};
use serde_json::Map;

use crate::dto::{SequenceSummary, SequenceView, TagSummary, TagView, WikiDraft, WriteTicket};
use crate::engine::forums::{excerpt_of, markdown_body};
use crate::engine::queries::fold_pending;
use crate::engine::{Engine, EngineError};
use crate::host::Host;
use crate::outbox::ContentOpts;

impl<H: Host> Engine<H> {
    /// Applicable, existing wiki tags, by descending listed-post count then slug.
    pub async fn tags(&self, forum: &str) -> Result<Vec<TagSummary>, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let wiki = l.model.view.wiki.as_ref().map_err(fold_pending)?;
        let uris: Vec<_> = wiki
            .pages
            .values()
            .filter_map(|p| p.canonical.clone())
            .collect();
        self.with_views(&l, &uris, &uris, |views| {
            let mut tags: Vec<_> = wiki
                .pages
                .keys()
                .filter_map(|slug| views.tag_summary(slug))
                .collect();
            tags.sort_by(|a, b| {
                b.post_count
                    .cmp(&a.post_count)
                    .then_with(|| a.slug.cmp(&b.slug))
            });
            tags
        })
        .await
    }

    /// The canonical wiki page for a tag and its listed relevant posts.
    pub async fn tag(&self, forum: &str, slug: &str) -> Result<TagView, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let wiki = l.model.view.wiki.as_ref().map_err(fold_pending)?;
        let page = wiki
            .pages
            .get(slug)
            .filter(|page| page.exists && page.applicable)
            .ok_or_else(|| EngineError::not_found(format!("tag {slug}")))?;
        let canonical = page
            .canonical
            .as_ref()
            .ok_or_else(|| EngineError::not_found(format!("tag {slug}")))?
            .clone();
        let one = std::slice::from_ref(&canonical);
        self.load_spilled_bodies(&l.model, one, &self.rt.renders)
            .await;
        let mut uris = vec![canonical];
        uris.extend(
            wiki.applies
                .iter()
                .filter(|(_, slugs)| slugs.contains(slug))
                .map(|(uri, _)| uri.clone()),
        );
        self.with_views(&l, &uris, &uris, |views| views.tag_view(slug))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("tag {slug}")))
    }

    /// Listed library sequences in the fold's deterministic order.
    pub async fn sequences(&self, forum: &str) -> Result<Vec<SequenceSummary>, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let library = l.model.view.library.as_ref().map_err(fold_pending)?;
        let listed = library.listed.clone();
        self.with_views(&l, &listed, &listed, |views| {
            listed
                .iter()
                .filter_map(|uri| views.sequence_summary(uri))
                .collect()
        })
        .await
    }

    /// A sequence and its same-forum chapter entries, including redacted unavailable posts.
    pub async fn sequence(&self, uri: &str) -> Result<SequenceView, EngineError> {
        let uri = records_uri(uri)?;
        if uri.collection().as_str() != nsid::SEQUENCE {
            return Err(EngineError::invalid_ref(format!("{uri} is not a sequence")));
        }
        let key = self.forum_of(&uri).await?;
        let l = self.loaded(&key).await?;
        let library = l.model.view.library.as_ref().map_err(fold_pending)?;
        if !library.eligible.contains(uri.as_str()) {
            return Err(EngineError::not_found(format!("sequence {uri}")));
        }
        let mut uris = vec![uri.to_string()];
        uris.extend(
            library
                .entries
                .get(uri.as_str())
                .into_iter()
                .flatten()
                .cloned(),
        );
        self.load_spilled_bodies(&l.model, std::slice::from_ref(&uris[0]), &self.rt.renders)
            .await;
        self.with_views(&l, &uris, &uris, |views| views.sequence_view(uri.as_str()))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("sequence {uri}")))
    }

    /// Create an immutable wiki revision. An edit names exactly the current canonical revision
    /// and its model CID as its parent; a new page has no parent.
    pub async fn wiki_edit(
        &self,
        forum: &str,
        slug: &str,
        draft: WikiDraft,
    ) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let wiki = l.model.view.wiki.as_ref().map_err(fold_pending)?;
        let parents = wiki
            .pages
            .get(slug)
            .and_then(|page| page.canonical.as_ref())
            .map(|canonical| {
                let skel = l.model.skel(canonical).ok_or_else(|| {
                    EngineError::internal(format!("missing canonical wiki skeleton: {canonical}"))
                })?;
                Ok::<StrongRef, EngineError>(StrongRef::new(
                    records_uri(canonical)?,
                    skel.cid.clone(),
                ))
            })
            .transpose()?
            .map(|parent| vec![parent]);
        let mut media = Vec::new();
        let (body, blobs, excerpt) = match draft.body {
            Some(body) => {
                let body = self.with_dids(&body).await?;
                media = body.media.clone();
                let blobs = self.media_blobs(&media).await?;
                let excerpt = excerpt_of(&body.markdown);
                (Some(markdown_body(&body)?), blobs, Some(excerpt))
            }
            None => (None, Vec::new(), None),
        };
        let rev = WikiRevision {
            r#type: nsid::WIKI_REVISION.to_owned(),
            forum: key.uri.clone(),
            slug: slug.to_owned(),
            parents,
            name: draft.name,
            description: draft.description,
            body,
            is_tag: Some(draft.is_tag),
            parent_tags: (!draft.parent_tags.is_empty()).then_some(draft.parent_tags),
            summary: draft.summary,
            created_at: Datetime::from_millis(self.host.now_ms())
                .map_err(|e| EngineError::internal(e.to_string()))?,
            extra: Map::new(),
        };
        let ticket = self
            .outbox
            .create_wiki_revision(&session, &rev, ContentOpts { blobs, excerpt })
            .await?;
        self.sent_prepared_media(&media).await;
        Ok(ticket)
    }
}

fn records_uri(uri: &str) -> Result<AtUri, EngineError> {
    AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(e.to_string()))
}
