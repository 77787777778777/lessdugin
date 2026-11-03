// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search over listed posts and comments in synced forum models.

use std::collections::BTreeSet;
use std::sync::Arc;

use lessdugin_lexicon::Collection;
use lessdugin_markdown::{PlainProjection, plain_projection};
use serde_json::Value;

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{
    DtoVersion, ItemState, RenderedBody, SearchHit, SearchItem, SearchQuery, SearchResults,
};
use crate::host::{Dir, Host, KeyRange, KvStore};
use crate::model::ForumModel;
use crate::model::render::{RenderCache, Spilled, text_blob_cid};
use crate::model::views::Views;
use crate::search::index::{self, ForumIndex};
use crate::social::full::view::State;
use crate::social::input::{SkelData, split_uri};
use crate::social::time::ms;
use crate::store::blob::ForumRow;
use crate::store::{row, schema};

use super::models::Loaded;
use super::{Engine, EngineError};

const SEARCH_MAX: u32 = 100;
/// Bodies read at a time while an index is built.
const INDEX_BATCH: usize = 256;

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

    /// The search index of a loaded forum (`sync.md` §13): built on first use from the title and
    /// P of every listed post and comment (bodies are read in batches, never rendered, and
    /// released), and kept until the model is replaced.
    async fn search_index(&self, l: &Loaded) -> Result<Arc<ForumIndex>, EngineError> {
        let model = &l.model;
        let f = model.key.uri.as_str();
        if let Some(ix) = self.rt.search.get(f, model) {
            return Ok(ix);
        }
        let docs: Vec<String> = model
            .input
            .records
            .iter()
            .filter(|(uri, record)| {
                record.valid
                    && matches!(record.coll, Some(Collection::Post | Collection::Comment))
                    && model.displayed(uri).is_some_and(State::listed)
                    && searchable(model, uri)
            })
            .map(|(uri, _)| uri.clone())
            .collect();
        let mut builder = index::Builder::default();
        for batch in docs.chunks(INDEX_BATCH) {
            let values = self.values(model, batch).await?;
            for uri in batch {
                let title = match model.skel(uri).map(|x| &x.data) {
                    Some(SkelData::Post { title, .. }) => title.as_deref(),
                    _ => None,
                };
                let p = match values.get(uri).and_then(|v| v.get("body")) {
                    Some(md) => projection(md, &model.input.ns, &self.rt.renders).map(|p| p.text),
                    None => Some(String::new()),
                };
                builder.add(uri, title, p.as_deref());
            }
        }
        let ix = Arc::new(builder.finish(model));
        self.rt.search.put(f, Arc::clone(&ix));
        Ok(ix)
    }

    /// Search title and rendered body text of listed, locally synced posts and comments. Hidden
    /// and tombstoned content is never inspected. Each forum's in-memory index selects the
    /// candidates; only those are rendered and checked. Terms match the title and P; snippets
    /// and highlights are cut from the title and P_x, so they never show spoiler content
    /// (`lfm-1.md` §12). Results are newest first, then URI, and capped at 100 hits.
    pub async fn search(&self, q: &SearchQuery) -> Result<SearchResults, EngineError> {
        let empty = || SearchResults {
            dto: DtoVersion::default(),
            hits: Vec::new(),
        };
        let terms = crate::search::terms(&q.q);
        if terms.is_empty() || q.limit == 0 {
            return Ok(empty());
        }
        let want = q.limit.min(SEARCH_MAX) as usize;
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
            let index = self.search_index(&loaded).await?;
            let rank = |uri: &str| model.view.times.get(uri).map_or(0, |t| ms(t.t_rank));
            let mut candidates: Vec<(i64, &str)> = index
                .candidates(&terms)
                .into_iter()
                .filter(|uri| {
                    author.as_deref().is_none_or(|did| {
                        split_uri(uri).is_some_and(|(author, _, _)| author == did)
                    })
                })
                .map(|uri| (rank(uri), uri))
                .collect();
            // the order of the results: the first `want` hits of the forum are all it can give
            candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            let mut found = 0;
            for batch in candidates.chunks(want.max(16)) {
                let uris: Vec<String> = batch.iter().map(|(_, u)| (*u).to_owned()).collect();
                let batch = self
                    .with_views(&loaded, &uris, &uris, |views| {
                        uris.iter()
                            .filter_map(|uri| search_hit(views, uri, &q.q))
                            .collect::<Vec<_>>()
                    })
                    .await?;
                found += batch.len();
                hits.extend(batch);
                if found >= want {
                    break;
                }
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        hits.truncate(want);
        Ok(SearchResults {
            dto: DtoVersion::default(),
            hits: hits.into_iter().map(|(_, _, hit)| hit).collect(),
        })
    }
}

/// `false` for a comment under a Missing root (or none): like lists, search leaves it out
/// (`social.md` §11).
fn searchable(model: &ForumModel, uri: &str) -> bool {
    match &model.skel(uri).map(|x| &x.data) {
        Some(SkelData::Comment { .. }) => model.comment_root(uri).is_some_and(|root| {
            model
                .view
                .threads
                .get(root)
                .is_some_and(|pl| !pl.root_missing)
        }),
        _ => true,
    }
}

/// The hit of the listed post or comment `uri`, with its `t_rank` (ms) and uri for ordering.
fn search_hit(views: &Views<'_>, uri: &str, q: &str) -> Option<(i64, String, SearchHit)> {
    let model = views.model;
    let skel = model.skel(uri)?;
    let listed =
        |state: &ItemState| matches!(state, ItemState::Visible | ItemState::Collapsed { .. });
    match &skel.data {
        SkelData::Post { .. } => {
            let post = views.post_summary(uri)?;
            if !listed(&post.state) {
                return None;
            }
            let body = views.body(skel, views.listed(uri));
            let title = post.title.as_deref().unwrap_or_default();
            let (snippet, highlights) = hit(views, uri, title, &body, q)?;
            Some((
                post.t_rank_ms,
                uri.to_owned(),
                SearchHit {
                    item: SearchItem::Post(post),
                    snippet,
                    highlights,
                },
            ))
        }
        SkelData::Comment { .. } => {
            if !searchable(model, uri) {
                return None;
            }
            let comment = views.comment_node(uri, model.comment_root(uri)?)?;
            if !listed(&comment.state) {
                return None;
            }
            let (snippet, highlights) = hit(views, uri, "", &comment.body, q)?;
            Some((
                comment.t_rank_ms,
                uri.to_owned(),
                SearchHit {
                    item: SearchItem::Comment(Box::new(comment)),
                    snippet,
                    highlights,
                },
            ))
        }
        _ => None,
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
/// snippet cut from the title and P_x of the body's source. A spilled body whose source is not
/// in memory (search fetches no blob) renders pending, with the record's own excerpt as its
/// text: that excerpt is matched and shown, as lists show it.
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
