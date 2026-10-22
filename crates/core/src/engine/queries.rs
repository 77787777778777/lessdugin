// SPDX-License-Identifier: AGPL-3.0-or-later
//! Queries (`engine-api.md` §3.4): `list_posts`, `recent_discussion`, `get_post`,
//! `get_thread`, `history`, `raw`.
//! They read the cache only (a forum whose declaration is not cached is opened first) and
//! return `dto/1` objects that are byte-identical for the same cache, local state and clock.
//!
//! `list_posts` pages with an opaque cursor (the offset into the sorted list, as decimal); a
//! page is at most [`limits::LIST_PAGE_MAX`] posts.

use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::values::PostKind as LexPostKind;
use serde_json::Value;

use crate::dto::{
    CommentSort, DiscussionItem, DiscussionList, DtoVersion, HistoryView, ListPostsQuery,
    PostFilter, PostKind, PostList, PostView, RawView, ThreadOpts, ThreadView, Tier, TrustLevel,
};
use crate::host::Host;
use crate::limits;
use crate::model::views::post_kind;
use crate::social::fold::State;
use crate::social::input::SkelData;
use crate::social::rank::{PostKey, order_posts};
use crate::social::threads::Node;
use crate::store::records::{Level, RecordStore};

use super::models::store_error;
use super::{Engine, EngineError};

/// A `recent_discussion` entry: the latest comment `t_rank`, the post, and its latest Visible
/// comments as `(t_rank, uri)`.
type Discussion = (i64, String, Vec<(i64, String)>);

fn parse_uri(uri: &str) -> Result<AtUri, EngineError> {
    AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))
}

impl<H: Host> Engine<H> {
    /// `list_posts(ListPostsQuery)` (`social.md` §12.2–§12.3). The tag filter matches the
    /// author's own tags (TODO(M3): `applies(p, t)` with tag votes, §13.3).
    pub async fn list_posts(&self, q: &ListPostsQuery) -> Result<PostList, EngineError> {
        let key = self.forum_key(&q.forum).await?;
        let author = match q.author.as_deref() {
            None => None,
            Some(a) if a.starts_with("did:") => Some(a.to_owned()),
            Some(a) => Some(self.resolve_actor(a).await?.did),
        };
        let l = self.loaded(&key).await?;
        let m = &l.model;
        let f = &m.fold;
        let p = &f.policy;
        let mut keys = Vec::new();
        for x in m.input.records.values() {
            let SkelData::Post { kind, tags, .. } = &x.data else {
                continue;
            };
            let Some(i) = f.item(&x.uri) else { continue };
            if !i.state.listed() {
                continue;
            }
            let k = post_kind(LexPostKind::effective(kind.as_deref()).value);
            let shown = match q.filter {
                PostFilter::Frontpage => i.tier >= Tier::Frontpage && k != PostKind::QuickTake,
                PostFilter::Curated => i.tier == Tier::Curated && k != PostKind::QuickTake,
                PostFilter::All => true,
            };
            if !shown
                || q.kind.is_some_and(|want| want != k)
                || author.as_ref().is_some_and(|a| *a != x.did)
                || q.tag.as_ref().is_some_and(|t| !tags.contains(t))
            {
                continue;
            }
            let t_rank = i.t_rank.unwrap_or(0);
            let activity = f.threads.get(&x.uri).map_or(t_rank, |th| {
                th.placement
                    .parent
                    .keys()
                    .filter_map(|c| f.item(c))
                    .filter(|c| c.state == State::Visible)
                    .filter_map(|c| c.t_rank)
                    .fold(t_rank, i64::max)
            });
            let score = i.score.score;
            let b = score
                + if i.tier == Tier::Curated {
                    p.curated_bonus
                } else {
                    0
                }
                + if i.tier >= Tier::Frontpage {
                    p.frontpage_bonus
                } else {
                    0
                };
            keys.push(PostKey {
                uri: x.uri.clone(),
                t_rank,
                score,
                b,
                pinned: i.pinned,
                activity,
            });
        }
        // ordered at the query time (the model may be up to MODEL_MAX_AGE_MS old)
        let now_us = self.view_now_ms().saturating_mul(1000);
        let ordered = order_posts(keys, q.sort, now_us, p);
        let offset: usize = match q.cursor.as_deref() {
            None | Some("") => 0,
            Some(c) => c
                .parse()
                .map_err(|_| EngineError::invalid_input(format!("bad cursor {c:?}")))?,
        };
        let limit = match q.limit {
            0 => limits::LIST_PAGE_DEFAULT,
            n => n.min(limits::LIST_PAGE_MAX),
        } as usize;
        let page: Vec<String> = ordered
            .iter()
            .skip(offset)
            .take(limit)
            .map(|k| k.uri.clone())
            .collect();
        let end = offset.saturating_add(page.len());
        let cursor = (end < ordered.len()).then(|| end.to_string());
        let items = self
            .with_views(&l, &page, &page, |v| {
                page.iter().filter_map(|u| v.post_summary(u)).collect()
            })
            .await?;
        Ok(PostList {
            dto: DtoVersion::default(),
            forum: key.uri.to_string(),
            sort: q.sort,
            filter: q.filter,
            items,
            cursor,
            completeness: l.completeness.clone(),
        })
    }

    /// `get_post(uri)`: the post with its rendered body (hidden posts carry an empty body).
    pub async fn get_post(&self, uri: &str) -> Result<PostView, EngineError> {
        let u = parse_uri(uri)?;
        let key = self.forum_of(&u).await?;
        let l = self.loaded(&key).await?;
        let uri = u.to_string();
        let one = std::slice::from_ref(&uri);
        self.with_views(&l, one, one, |v| v.post_view(&uri))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("no post {uri} in the forum")))
    }

    /// `get_thread(uri, ThreadOpts)`: the post and its comments in pre-order with depth
    /// (`social.md` §10.2, §11, §12.4). TODO(M3): `unreadOnly` needs read marks; every comment
    /// is returned.
    pub async fn get_thread(
        &self,
        uri: &str,
        opts: &ThreadOpts,
    ) -> Result<ThreadView, EngineError> {
        let u = parse_uri(uri)?;
        let coll = u.collection().as_str();
        if coll != nsid::POST && coll != nsid::COMMENT {
            return Err(EngineError::invalid_ref(format!(
                "{u} is not a post or a comment"
            )));
        }
        let key = self.forum_of(&u).await?;
        let l = self.loaded(&key).await?;
        let m = &l.model;
        // a comment's uri opens its thread
        let uri = match m.skel(u.as_str()).map(|x| &x.data) {
            Some(SkelData::Comment { root: Some(r), .. }) => r.uri.clone(),
            _ => u.to_string(),
        };
        if m.skel(&uri)
            .is_some_and(|x| !matches!(x.data, SkelData::Post { .. }))
        {
            return Err(EngineError::invalid_ref(format!(
                "{uri} is not a thread root (a post)"
            )));
        }
        let th = m.fold.threads.get(&uri);
        if th.is_none() && m.fold.item(&uri).is_none() {
            return Err(EngineError::not_found(format!(
                "no thread {uri} in the forum"
            )));
        }
        let mut bodies = vec![uri.clone()];
        let mut people = vec![uri.clone()];
        if let Some(th) = th {
            for n in th.placement.top_down() {
                match n {
                    Node::Comment(c) => {
                        if m.fold.item(&c).is_some_and(|i| i.displayed.is_some()) {
                            people.push(c.clone());
                            bodies.push(c);
                        }
                    }
                    Node::Missing(c) => people.push(c),
                    Node::Root => {}
                }
            }
        }
        let sort: CommentSort = opts.sort;
        self.with_views(&l, &bodies, &people, |v| v.thread_view(&uri, sort))
            .await
    }

    /// `recent_discussion(forum, cursor?, limit)` (`engine-api.md` §3.4): the listed posts
    /// with Visible comments, most recently commented first (by the greatest comment
    /// `t_rank`, then uri), each with its latest [`limits::DISCUSSION_COMMENTS`] Visible
    /// comments (newest first). Paged like `list_posts` (`limit` 0: the default page size).
    pub async fn recent_discussion(
        &self,
        forum: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<DiscussionList, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let m = &l.model;
        let f = &m.fold;
        let mut entries: Vec<Discussion> = Vec::new();
        for x in m.input.records.values() {
            if !matches!(x.data, SkelData::Post { .. }) {
                continue;
            }
            if !f.item(&x.uri).is_some_and(|i| i.state.listed()) {
                continue;
            }
            let Some(th) = f.threads.get(&x.uri) else {
                continue;
            };
            let mut comments: Vec<(i64, String)> = th
                .placement
                .parent
                .keys()
                .filter_map(|c| {
                    let i = f.item(c)?;
                    (i.state == State::Visible).then(|| (i.t_rank.unwrap_or(i64::MIN), c.clone()))
                })
                .collect();
            if comments.is_empty() {
                continue;
            }
            comments.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            comments.truncate(limits::DISCUSSION_COMMENTS);
            entries.push((comments[0].0, x.uri.clone(), comments));
        }
        entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let offset: usize = match cursor {
            None | Some("") => 0,
            Some(c) => c
                .parse()
                .map_err(|_| EngineError::invalid_input(format!("bad cursor {c:?}")))?,
        };
        let limit = match limit {
            0 => limits::LIST_PAGE_DEFAULT,
            n => n.min(limits::LIST_PAGE_MAX),
        } as usize;
        let page: Vec<&Discussion> = entries.iter().skip(offset).take(limit).collect();
        let end = offset.saturating_add(page.len());
        let cursor = (end < entries.len()).then(|| end.to_string());
        let uris: Vec<String> = page
            .iter()
            .flat_map(|(_, p, cs)| std::iter::once(p.clone()).chain(cs.iter().map(|c| c.1.clone())))
            .collect();
        let items = self
            .with_views(&l, &uris, &uris, |v| {
                page.iter()
                    .filter_map(|(_, p, cs)| {
                        Some(DiscussionItem {
                            post: v.post_summary(p)?,
                            comments: cs.iter().filter_map(|c| v.comment_node(&c.1, p)).collect(),
                        })
                    })
                    .collect()
            })
            .await?;
        Ok(DiscussionList {
            dto: DtoVersion::default(),
            items,
            cursor,
            completeness: l.completeness.clone(),
        })
    }

    /// `history(uri)` (basic): the versions known from the item's revision records.
    pub async fn history(&self, uri: &str) -> Result<HistoryView, EngineError> {
        let u = parse_uri(uri)?;
        let key = self.forum_of(&u).await?;
        let l = self.loaded(&key).await?;
        let uri = u.to_string();
        self.with_views(&l, &[], std::slice::from_ref(&uri), |v| v.history(&uri))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("no item {uri} in the forum")))
    }

    /// `raw(uri)`: the stored record JSON, its validity violations, the moderation assertions
    /// naming it (TODO(M3): effective/lapsed annotations) and its fold state.
    pub async fn raw(&self, uri: &str) -> Result<RawView, EngineError> {
        let u = parse_uri(uri)?;
        let store = RecordStore::new(self.host.cache());
        let row = store.get(&u).await.map_err(store_error)?;
        let key = self.forum_of(&u).await.ok();
        let loaded = match &key {
            Some(k) => self.loaded(k).await.ok(),
            None => None,
        };
        let uri_s = u.to_string();
        let mut assertions: Vec<Value> = Vec::new();
        let mut state = None;
        let mut root = None;
        if let Some(l) = &loaded {
            let m = &l.model;
            // a comment's thread: its record's root, if the fold placed it there
            if let Some(SkelData::Comment { root: Some(r), .. }) = m.skel(&uri_s).map(|x| &x.data)
                && m.fold
                    .threads
                    .get(&r.uri)
                    .is_some_and(|th| th.placement.parent.contains_key(&uri_s))
            {
                root = Some(r.uri.clone());
            }
            for x in m.input.records.values() {
                if x.coll != Some(Collection::ModAction) {
                    continue;
                }
                let names = x
                    .value()
                    .and_then(|v| v.get("subject")?.get("uri")?.as_str().map(|s| s == uri_s))
                    .unwrap_or(false);
                if names {
                    assertions.push(serde_json::json!({
                        "uri": x.uri,
                        "cid": x.cid,
                        "valid": x.valid,
                        "record": x.value().cloned().unwrap_or(Value::Null),
                    }));
                }
            }
            state = m.fold.item(&uri_s).and_then(|i| i.state.item_state());
        }
        let Some(row) = row else {
            if store.tombstone_of(&u).await.map_err(store_error)?.is_some() {
                return Ok(RawView {
                    dto: DtoVersion::default(),
                    uri: uri_s,
                    cid: None,
                    root,
                    level: TrustLevel::Pds,
                    record: None,
                    validity: Vec::new(),
                    assertions,
                    state,
                });
            }
            return Err(EngineError::not_found(format!(
                "{uri_s} is not in the cache"
            )));
        };
        let record = row.value_json().ok();
        let validity = if row.violations.is_empty() {
            row.rule.iter().cloned().collect()
        } else {
            row.violations.clone()
        };
        Ok(RawView {
            dto: DtoVersion::default(),
            uri: uri_s,
            cid: Some(row.cid.to_string()),
            root,
            level: match row.level {
                Level::Pds => TrustLevel::Pds,
                Level::Proven => TrustLevel::Proven,
            },
            record,
            validity,
            assertions,
            state,
        })
    }
}
