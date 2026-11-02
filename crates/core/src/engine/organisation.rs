// SPDX-License-Identifier: AGPL-3.0-or-later
//! M5 organisation queries and writes: wiki tags and library sequences. Records and bodies come
//! from the synced local model.

use lessdugin_atp::syntax::{AtUri, Datetime};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{Chapter, Md, Sequence, StrongRef, WikiRevision};
use serde_json::{Map, Value};

use crate::dto::{
    SequenceDraft, SequencePosition, SequenceSummary, SequenceView, TagSummary, TagView, WikiDraft,
    WriteTicket,
};
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

    /// Create an immutable wiki revision. An edit names exactly one parent with its model CID:
    /// the draft's `base` (the head the author edited, decision O-2), which must still be the
    /// cached canonical revision (`conflict` otherwise), or without one the current canonical
    /// revision; a new page has no parent.
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
        let head = wiki
            .pages
            .get(slug)
            .and_then(|page| page.canonical.as_ref());
        if let Some(base) = &draft.base {
            let base = records_uri(base)?.to_string();
            if head != Some(&base) {
                let mut e = EngineError::conflict(format!(
                    "the page {slug} has changed since {base} was loaded: reload it and edit the \
                     current head"
                ))
                .with_detail("kind", "wikiHeadMoved")
                .with_detail("base", base);
                if let Some(head) = head {
                    e = e.with_detail("head", head.clone());
                }
                return Err(e);
            }
        }
        let parents = head
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

    /// Rewrite handle mentions and prepare every body in a sequence draft. Media from the
    /// introduction and all chapters share the write's upload set.
    async fn prepare_sequence(
        &self,
        draft: &SequenceDraft,
    ) -> Result<(Option<Md>, Vec<Option<Md>>, Vec<crate::dto::MediaRef>), EngineError> {
        let mut media = Vec::new();
        let body = match &draft.body {
            Some(body) => {
                let body = self.with_dids(body).await?;
                media.extend(body.media.clone());
                Some(markdown_body(&body)?)
            }
            None => None,
        };
        let mut chapters = Vec::with_capacity(draft.chapters.len());
        for chapter in &draft.chapters {
            let body = match &chapter.body {
                Some(body) => {
                    let body = self.with_dids(body).await?;
                    media.extend(body.media.clone());
                    Some(markdown_body(&body)?)
                }
                None => None,
            };
            chapters.push(body);
        }
        Ok((body, chapters, media))
    }

    /// Create a sequence and its chapters through the ordinary content outbox.
    pub async fn seq_create(&self, draft: &SequenceDraft) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(&draft.forum).await?;
        let (body, chapter_bodies, media) = self.prepare_sequence(draft).await?;
        let chapters = draft
            .chapters
            .iter()
            .zip(chapter_bodies)
            .map(|(chapter, body)| {
                Ok(Chapter {
                    title: chapter.title.clone(),
                    body,
                    posts: chapter
                        .posts
                        .iter()
                        .map(|post| {
                            let uri = records_uri(post)?;
                            if uri.collection().as_str() != nsid::POST {
                                return Err(EngineError::invalid_input(format!(
                                    "{uri} is not a post"
                                )));
                            }
                            Ok(uri)
                        })
                        .collect::<Result<Vec<_>, EngineError>>()?,
                    extra: Map::new(),
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let sequence = Sequence {
            r#type: nsid::SEQUENCE.to_owned(),
            forum: key.uri,
            title: draft.title.clone(),
            body,
            cover: None,
            chapters,
            created_at: Datetime::from_millis(self.host.now_ms())
                .map_err(|e| EngineError::internal(e.to_string()))?,
            updated_at: None,
            extra: Map::new(),
        };
        let opts = ContentOpts {
            blobs: self.media_blobs(&media).await?,
            excerpt: None,
        };
        let ticket = self
            .outbox
            .create_sequence(&session, &sequence, opts)
            .await?;
        self.sent_prepared_media(&media).await;
        Ok(ticket)
    }

    /// The cached current value of an own sequence and the outbox session of its author.
    async fn sequence_for_edit(
        &self,
        uri: &str,
    ) -> Result<(crate::outbox::Session, String, String, Value), EngineError> {
        let uri = records_uri(uri)?;
        if uri.collection().as_str() != nsid::SEQUENCE {
            return Err(EngineError::invalid_ref(format!("{uri} is not a sequence")));
        }
        let session = self.session_for(uri.did().as_str()).await?;
        let (base, value) = self.current(&uri).await?;
        Ok((session, uri.to_string(), base, value))
    }

    /// Replace the drafted fields of a sequence. Forum, creation time, cover and unknown fields
    /// are retained; chapter-local unknown fields are retained at their existing positions.
    pub async fn seq_edit(
        &self,
        uri: &str,
        draft: &SequenceDraft,
    ) -> Result<WriteTicket, EngineError> {
        let (session, uri, base, mut value) = self.sequence_for_edit(uri).await?;
        let old_forum = value
            .get("forum")
            .and_then(Value::as_str)
            .ok_or_else(|| EngineError::invalid_input(format!("{uri} names no forum")))?;
        let key = self.forum_key(&draft.forum).await?;
        if key.uri.as_str() != old_forum {
            return Err(EngineError::invalid_input(
                "a sequence's forum is fixed after creation",
            ));
        }
        let (body, chapter_bodies, media) = self.prepare_sequence(draft).await?;
        let cached = value.clone();
        let object = value
            .as_object_mut()
            .ok_or_else(|| EngineError::internal(format!("{uri} is not an object")))?;
        object.insert("title".into(), Value::String(draft.title.clone()));
        match body {
            Some(body) => {
                object.insert(
                    "body".into(),
                    serde_json::to_value(body)
                        .map_err(|e| EngineError::internal(format!("encode: {e}")))?,
                );
            }
            None => {
                object.remove("body");
            }
        }
        let old_chapters = object
            .get("chapters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut chapters = Vec::with_capacity(draft.chapters.len());
        for (i, (chapter, body)) in draft.chapters.iter().zip(chapter_bodies).enumerate() {
            let mut out = old_chapters
                .get(i)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match &chapter.title {
                Some(title) => {
                    out.insert("title".into(), Value::String(title.clone()));
                }
                None => {
                    out.remove("title");
                }
            }
            match body {
                Some(body) => {
                    out.insert(
                        "body".into(),
                        serde_json::to_value(body)
                            .map_err(|e| EngineError::internal(format!("encode: {e}")))?,
                    );
                }
                None => {
                    out.remove("body");
                }
            }
            out.insert(
                "posts".into(),
                Value::Array(chapter.posts.iter().cloned().map(Value::String).collect()),
            );
            chapters.push(Value::Object(out));
        }
        object.insert("chapters".into(), Value::Array(chapters));
        let blobs = self.edit_media_blobs(&cached, &mut value, &media).await?;
        let ticket = self
            .outbox
            .edit(
                &session,
                &uri,
                &value,
                &base,
                crate::outbox::EditOpts {
                    blobs,
                    ..Default::default()
                },
            )
            .await?;
        self.sent_prepared_media(&media).await;
        Ok(ticket)
    }

    /// Add a post at a chapter-local position (`index == len` appends).
    pub async fn seq_add(
        &self,
        uri: &str,
        post: &str,
        at: &SequencePosition,
    ) -> Result<WriteTicket, EngineError> {
        let post = records_uri(post)?;
        if post.collection().as_str() != nsid::POST {
            return Err(EngineError::invalid_input(format!("{post} is not a post")));
        }
        let (session, uri, base, mut value) = self.sequence_for_edit(uri).await?;
        let chapters = value
            .get_mut("chapters")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| EngineError::invalid_input(format!("{uri} has no chapters")))?;
        let chapter = chapters
            .get_mut(at.chapter)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| EngineError::invalid_input("no such sequence chapter"))?;
        let posts = chapter
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| EngineError::invalid_input("the sequence chapter has no posts"))?;
        if at.index > posts.len() {
            return Err(EngineError::invalid_input("no such sequence position"));
        }
        posts.insert(at.index, Value::String(post.to_string()));
        self.outbox
            .edit(
                &session,
                &uri,
                &value,
                &base,
                crate::outbox::EditOpts::default(),
            )
            .await
    }

    /// Remove every occurrence of a post from a sequence's chapters.
    pub async fn seq_remove(&self, uri: &str, post: &str) -> Result<WriteTicket, EngineError> {
        let post = records_uri(post)?;
        if post.collection().as_str() != nsid::POST {
            return Err(EngineError::invalid_input(format!("{post} is not a post")));
        }
        let (session, uri, base, mut value) = self.sequence_for_edit(uri).await?;
        let chapters = value
            .get_mut("chapters")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| EngineError::invalid_input(format!("{uri} has no chapters")))?;
        for chapter in chapters {
            if let Some(posts) = chapter.get_mut("posts").and_then(Value::as_array_mut) {
                posts.retain(|entry| entry.as_str() != Some(post.as_str()));
            }
        }
        self.outbox
            .edit(
                &session,
                &uri,
                &value,
                &base,
                crate::outbox::EditOpts::default(),
            )
            .await
    }

    /// Move the entry at `from` to `to`, removing it before interpreting the destination index.
    pub async fn seq_move(
        &self,
        uri: &str,
        from: &SequencePosition,
        to: &SequencePosition,
    ) -> Result<WriteTicket, EngineError> {
        let (session, uri, base, mut value) = self.sequence_for_edit(uri).await?;
        let chapters = value
            .get_mut("chapters")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| EngineError::invalid_input(format!("{uri} has no chapters")))?;
        if from.chapter >= chapters.len() || to.chapter >= chapters.len() {
            return Err(EngineError::invalid_input("no such sequence chapter"));
        }
        let source_len = chapters[from.chapter]
            .get("posts")
            .and_then(Value::as_array)
            .map(Vec::len)
            .ok_or_else(|| EngineError::invalid_input("the sequence chapter has no posts"))?;
        if from.index >= source_len {
            return Err(EngineError::invalid_input("no such sequence position"));
        }
        let destination_len = chapters[to.chapter]
            .get("posts")
            .and_then(Value::as_array)
            .map(Vec::len)
            .ok_or_else(|| EngineError::invalid_input("the sequence chapter has no posts"))?
            - usize::from(from.chapter == to.chapter);
        if to.index > destination_len {
            return Err(EngineError::invalid_input("no such sequence position"));
        }
        let moved = chapters[from.chapter]
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .expect("source posts checked above")
            .remove(from.index);
        chapters[to.chapter]
            .get_mut("posts")
            .and_then(Value::as_array_mut)
            .expect("destination posts checked above")
            .insert(to.index, moved);
        self.outbox
            .edit(
                &session,
                &uri,
                &value,
                &base,
                crate::outbox::EditOpts::default(),
            )
            .await
    }
}

fn records_uri(uri: &str) -> Result<AtUri, EngineError> {
    AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(e.to_string()))
}
