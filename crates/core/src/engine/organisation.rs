// SPDX-License-Identifier: AGPL-3.0-or-later
//! M5 organisation queries and writes: wiki tags, library sequences, search, user pages and
//! rendered-body backlinks. All records and bodies come from the synced local model.

use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{StrongRef, WikiRevision};
use serde_json::Map;

use crate::dto::{TagSummary, TagView, WikiDraft, WriteTicket};
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
        self.with_views(&l, one, one, |views| views.tag_view(slug))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("tag {slug}")))
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
        let forum = records_uri(forum)?;
        let key = self.forum_key(forum.as_str()).await?;
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
                Ok::<StrongRef, EngineError>(StrongRef::new(records_uri(canonical)?, skel.cid.clone()))
            })
            .transpose()?
            .map(|parent| vec![parent]);
        let (body, blobs, excerpt) = match draft.body {
            Some(body) => {
                let body = self.with_dids(&body).await?;
                let blobs = self.media_blobs(&body.media).await?;
                let excerpt = excerpt_of(&body.markdown);
                (Some(markdown_body(&body)?), blobs, Some(excerpt))
            }
            None => (None, Vec::new(), None),
        };
        let rev = WikiRevision {
            r#type: nsid::WIKI_REVISION.to_owned(),
            forum,
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
        self.outbox
            .create_wiki_revision(&session, &rev, ContentOpts { blobs, excerpt })
            .await
    }
}

fn records_uri(uri: &str) -> Result<AtUri, EngineError> {
    AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(e.to_string()))
}
