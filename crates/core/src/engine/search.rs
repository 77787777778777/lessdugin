// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search over listed posts and comments in synced forum models.

use std::collections::BTreeSet;

use lessdugin_lexicon::Collection;
use lessdugin_markdown::{PlainProjection, plain_projection};
use serde_json::Value;

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{DtoVersion, RenderedBody, SearchHit, SearchItem, SearchQuery, SearchResults};
use crate::host::{Dir, Host, KeyRange, KvStore};
use crate::model::render::{RenderCache, Spilled, text_blob_cid};
use crate::model::views::Views;
use crate::social::input::SkelData;
use crate::store::blob::ForumRow;
use crate::store::{row, schema};

use super::{Engine, EngineError};

const SEARCH_MAX: u32 = 100;

impl<H: Host> Engine<H> {
    /// Canonical keys of forums with local models or sync metadata. This enumerates local state
    /// only; it does not discover or fetch forum content.
    pub(crate) async fn synced_forum_keys(&self) -> Result<Vec<ForumKey>, EngineError> {
        let mut uris: BTreeSet<String> = self.rt.loaded_forums().into_iter().collect();
        let store = self.host.cache();
        let mut range = KeyRange::prefix(&schema::prefix(schema::tag::FORUM, &[]));
        loop {
            let page = store.scan(range.clone(), Dir::Forward, 1_000).await?;
            let count = page.len();
            for (_, bytes) in &page {
                if let Ok(forum) = row::decode::<ForumRow>(bytes) {
                    uris.insert(forum.uri);
                }
            }
            if count < 1_000 {
                break;
            }
            range.start = std::ops::Bound::Excluded(page.last().unwrap().0.clone());
        }
        let mut keys = Vec::new();
        for uri in uris {
            if let Ok(uri) = lessdugin_atp::syntax::AtUri::parse(&uri)
                && let Ok(key) = ForumKey::of_uri(&uri)
            {
                keys.push(key);
            }
        }
        keys.sort_by(|a, b| a.uri.as_str().cmp(b.uri.as_str()));
        keys.dedup_by(|a, b| a.uri == b.uri);
        Ok(keys)
    }

    /// Search title and rendered body text of listed, locally synced posts and comments. Hidden
    /// and tombstoned content is never inspected. Terms match the title and P; snippets and
    /// highlights are cut from the title and P_x, so they never show spoiler content
    /// (`lfm-1.md` §12). Results are newest first, then URI, and capped at 100 hits.
    pub async fn search(&self, q: &SearchQuery) -> Result<SearchResults, EngineError> {
        let empty = || SearchResults {
            dto: DtoVersion::default(),
            hits: Vec::new(),
        };
        if q.q.trim().is_empty() || q.limit == 0 {
            return Ok(empty());
        }
        let author = match q.author.as_deref() {
            Some(a) if a.starts_with("did:") => Some(a.to_owned()),
            Some(a) => Some(self.resolve_actor(a).await?.did),
            None => None,
        };
        let keys = match q.forum.as_deref() {
            Some(forum) => vec![self.forum_key(forum).await?],
            None => self.synced_forum_keys().await?,
        };
        let scoped = q.forum.is_some();
        let mut hits = Vec::new();
        for key in keys {
            let loaded = match self.loaded(&key).await {
                Ok(loaded) => loaded,
                Err(_) if !scoped => continue,
                Err(e) => return Err(e),
            };
            let model = &loaded.model;
            let uris: Vec<String> = model
                .input
                .records
                .iter()
                .filter(|(uri, record)| {
                    record.valid
                        && matches!(record.coll, Some(Collection::Post | Collection::Comment))
                        && author.as_deref().is_none_or(|did| {
                            lessdugin_atp::syntax::AtUri::parse(uri)
                                .is_ok_and(|uri| uri.did().as_str() == did)
                        })
                })
                .map(|(uri, _)| uri.clone())
                .collect();
            let found = self
                .with_views(&loaded, &uris, &uris, |views| {
                    let mut found = Vec::new();
                    for uri in &uris {
                        let Some(skel) = model.skel(uri) else {
                            continue;
                        };
                        match &skel.data {
                            SkelData::Post { .. } => {
                                let Some(post) = views.post_summary(uri) else {
                                    continue;
                                };
                                if !matches!(
                                    post.state,
                                    crate::dto::ItemState::Visible
                                        | crate::dto::ItemState::Collapsed { .. }
                                ) {
                                    continue;
                                }
                                let body = views.body(skel, views.listed(uri));
                                let title = post.title.as_deref().unwrap_or_default();
                                let Some((snippet, highlights)) =
                                    hit(views, uri, title, &body, &q.q)
                                else {
                                    continue;
                                };
                                found.push((
                                    post.t_rank_ms,
                                    uri.clone(),
                                    SearchHit {
                                        item: SearchItem::Post(post),
                                        snippet,
                                        highlights,
                                    },
                                ));
                            }
                            SkelData::Comment { .. } => {
                                let Some(root) = model.comment_root(uri) else {
                                    continue;
                                };
                                let Some(comment) = views.comment_node(uri, root) else {
                                    continue;
                                };
                                if !matches!(
                                    comment.state,
                                    crate::dto::ItemState::Visible
                                        | crate::dto::ItemState::Collapsed { .. }
                                ) {
                                    continue;
                                }
                                let Some((snippet, highlights)) =
                                    hit(views, uri, "", &comment.body, &q.q)
                                else {
                                    continue;
                                };
                                found.push((
                                    comment.t_rank_ms,
                                    uri.clone(),
                                    SearchHit {
                                        item: SearchItem::Comment(Box::new(comment)),
                                        snippet,
                                        highlights,
                                    },
                                ));
                            }
                            _ => {}
                        }
                    }
                    found
                })
                .await?;
            hits.extend(found);
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        hits.truncate(q.limit.min(SEARCH_MAX) as usize);
        Ok(SearchResults {
            dto: DtoVersion::default(),
            hits: hits.into_iter().map(|(_, _, hit)| hit).collect(),
        })
    }
}

/// P and P_x of the body `md` as the views render it, from its source (`None`: a spilled body
/// whose source is not in memory, rendered from the LRU or still pending).
fn projection(md: &Value, ns: &str, renders: &RenderCache) -> Option<PlainProjection> {
    let markdown = format!("{ns}.markdown");
    let known = md
        .get("$type")
        .and_then(Value::as_str)
        .is_some_and(|t| t == markdown || t == format!("{markdown}#main"));
    let empty = || PlainProjection {
        text: String::new(),
        text_x: String::new(),
    };
    if !known {
        return Some(empty());
    }
    if let Some(text) = md.get("text").and_then(Value::as_str) {
        return Some(plain_projection(text));
    }
    match text_blob_cid(md)
        .and_then(|blob| renders.spilled(blob))
        .as_deref()
    {
        Some(Spilled::Text(src)) => Some(plain_projection(src)),
        Some(_) => Some(empty()),
        None => None,
    }
}

/// `title` and `text` joined by a space when both are present.
fn joined(title: &str, text: &str) -> String {
    match (title.is_empty(), text.is_empty()) {
        (false, false) => format!("{title} {text}"),
        (false, true) => title.to_owned(),
        _ => text.to_owned(),
    }
}

/// The hit of item `uri` (title `title`, rendered `body`): matched over the title and P, its
/// snippet cut from the title and P_x of the body's source. Without the source in memory (a
/// spilled body), the rendered excerpt stands in for P_x: it is cut from P_x too.
fn hit(
    views: &Views<'_>,
    uri: &str,
    title: &str,
    body: &RenderedBody,
    q: &str,
) -> Option<(String, Vec<(u32, u32)>)> {
    let matched = joined(title, &body.text);
    let px = views
        .values
        .get(uri)
        .and_then(|v| v.get("body"))
        .and_then(|md| projection(md, &views.model.input.ns, views.renders))
        .map_or_else(|| body.excerpt.clone(), |p| p.text_x);
    crate::search::hit(&matched, &joined(title, &px), q)
}
