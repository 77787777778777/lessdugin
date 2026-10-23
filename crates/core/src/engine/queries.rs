// SPDX-License-Identifier: AGPL-3.0-or-later
//! Queries (`engine-api.md` §3.4): `list_posts`, `recent_discussion`, `get_post`,
//! `get_thread`, `history`, `raw`, `mod_queue`, `mod_log`.
//! They read the cache only (a forum whose declaration is not cached is opened first) and
//! return `dto/1` objects that are byte-identical for the same cache, local state and clock.
//!
//! Every view comes from the forum model's full fold (`social.md` §1); a sort, filter or thread
//! that the model's base fold did not compute is re-ranked from its cached layers
//! ([`crate::model::ForumModel::orderings`]), never refolded. `list_posts`, `recent_discussion`
//! and `mod_log` page with an opaque cursor (the offset into the sorted list, as decimal); a
//! page is at most [`limits::LIST_PAGE_MAX`] items.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::collection::nsid;
use serde_json::Value;

use crate::dto::{
    CommentSort, DiscussionItem, DiscussionList, DtoVersion, HistoryView, ListPostsQuery, ModLog,
    ModQueue, PostList, PostView, RawView, ThreadOpts, ThreadView, TrustLevel,
};
use crate::host::Host;
use crate::limits;
use crate::model::ForumModel;
use crate::model::views::{Viewer, post_filter, post_sort};

use crate::social::full::query::{PostQuery, Query};
use crate::social::input::{SkelData, split_uri};
use crate::social::threads::Node;
use crate::store::records::{Level, RecordStore};

use super::models::store_error;
use super::{Engine, EngineError, ErrorCode};

fn parse_uri(uri: &str) -> Result<AtUri, EngineError> {
    AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))
}

/// `(offset, limit)` of a page: `cursor` is the decimal offset, `limit` 0 the default size.
fn page_of(cursor: Option<&str>, limit: u32) -> Result<(usize, usize), EngineError> {
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
    Ok((offset, limit))
}

/// The page of `items` at `offset` and the next cursor.
fn page<T: Clone>(items: &[T], offset: usize, limit: usize) -> (Vec<T>, Option<String>) {
    let page: Vec<T> = items.iter().skip(offset).take(limit).cloned().collect();
    let end = offset.saturating_add(page.len());
    let cursor = (end < items.len()).then(|| end.to_string());
    (page, cursor)
}

fn fold_pending(p: impl std::fmt::Display) -> EngineError {
    EngineError::internal(format!("the fold layer is pending: {p}"))
}

impl<H: Host> Engine<H> {
    /// The viewer of the views over `m`: the active account, its mutes (the lens,
    /// `social.md` §10.3) and the read marks of the posts among `uris` (§16).
    pub(crate) async fn viewer(
        &self,
        _m: &ForumModel,
        uris: &[String],
    ) -> Result<Viewer, EngineError> {
        let mut read = BTreeMap::new();
        for u in uris {
            if split_uri(u).is_some_and(|(_, c, _)| c == nsid::POST)
                && !read.contains_key(u)
                && let Some(at) = self.read_mark(u).await?
            {
                read.insert(u.clone(), at);
            }
        }
        Ok(Viewer {
            did: self.me().await?,
            muted: self.muted().await?,
            read,
        })
    }

    /// `list_posts(ListPostsQuery)` (`social.md` §12.2–§12.3): the listed posts passing the
    /// filter (`frontpage`, the default: tier ≥ frontpage and not a quick take; `all`;
    /// `curated`), the tag (`applies(p, t)`, §13.3), kind and author filters, in the sort's
    /// order (`magic`, the default: pins first, then `hot` with the cutoff; `top`, `new`, `old`,
    /// `recentComments`).
    pub async fn list_posts(&self, q: &ListPostsQuery) -> Result<PostList, EngineError> {
        let key = self.forum_key(&q.forum).await?;
        let author = match q.author.as_deref() {
            None => None,
            Some(a) if a.starts_with("did:") => Some(a.to_owned()),
            Some(a) => Some(self.resolve_actor(a).await?.did),
        };
        let l = self.loaded(&key).await?;
        let query = Query {
            thread: None,
            posts: Some(PostQuery {
                sort: post_sort(q.sort),
                filter: post_filter(q.filter),
                tag: q.tag.clone(),
                kind: q.kind.map(|k| {
                    serde_json::to_value(k)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default()
                }),
                author,
            }),
            comments: None,
        };
        let order = l
            .model
            .orderings(&query)
            .map_err(fold_pending)?
            .post_order
            .unwrap_or_default();
        let (offset, limit) = page_of(q.cursor.as_deref(), q.limit)?;
        let (page, cursor) = page(&order, offset, limit);
        let (items, banners) = self
            .with_views(&l, &page, &page, |v| {
                (
                    page.iter().filter_map(|u| v.post_summary(u)).collect(),
                    v.banners(),
                )
            })
            .await?;
        let completeness = self.with_views(&l, &[], &[], |v| v.completeness()).await?;
        Ok(PostList {
            dto: DtoVersion::default(),
            forum: key.uri.to_string(),
            sort: q.sort,
            filter: q.filter,
            items,
            cursor,
            completeness,
            banners,
        })
    }

    /// `get_post(uri)`: the post with its rendered body and reacts (hidden posts carry an
    /// empty body and no reacts).
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

    /// `get_thread(uri, ThreadOpts)`: the post and its displayed comments in pre-order with
    /// depth (`social.md` §10.2, §11, §12.4), siblings in the `sort` order (thread pins first).
    /// With `unreadOnly`, only the unread comments and the nodes on their paths are kept (a
    /// thread without a read mark keeps everything).
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
        let uri = match m.comment_root(u.as_str()) {
            Some(r) => r.to_owned(),
            None => u.to_string(),
        };
        if m.skel(&uri)
            .is_some_and(|x| !matches!(x.data, SkelData::Post { .. }))
        {
            return Err(EngineError::invalid_ref(format!(
                "{uri} is not a thread root (a post)"
            )));
        }
        let pl = m.view.threads.get(&uri);
        if pl.is_none() && m.skel(&uri).is_none() {
            return Err(EngineError::not_found(format!(
                "no thread {uri} in the forum"
            )));
        }
        let mut bodies = vec![uri.clone()];
        let mut people = vec![uri.clone()];
        if let Some(pl) = pl {
            for n in pl.top_down() {
                match n {
                    Node::Comment(c) => {
                        if m.displayed(&c).is_some_and(|s| s.listed()) {
                            bodies.push(c.clone());
                        }
                        people.push(c);
                    }
                    Node::Missing(c) => people.push(c),
                    Node::Root => {}
                }
            }
        }
        let sort: CommentSort = opts.sort;
        let mut view = self
            .with_views(&l, &bodies, &people, |v| v.thread_view(&uri, sort))
            .await?;
        if opts.unread_only && view.comments.iter().any(|c| c.unread) {
            // keep the unread comments and their ancestors (pre-order: parents come first)
            let parents: BTreeMap<String, Option<String>> = view
                .comments
                .iter()
                .map(|c| (c.uri.clone(), c.parent.clone()))
                .collect();
            let mut keep: BTreeSet<String> = BTreeSet::new();
            for c in view.comments.iter().filter(|c| c.unread) {
                let mut cur = Some(c.uri.clone());
                while let Some(u) = cur {
                    if !keep.insert(u.clone()) {
                        break;
                    }
                    cur = parents.get(&u).cloned().flatten();
                }
            }
            view.comments.retain(|c| keep.contains(&c.uri));
        }
        Ok(view)
    }

    /// `recent_discussion(forum, cursor?, limit)` (`social.md` §12.5): the listed posts with
    /// `Visible` comments, by the newest `Visible` comment's `t_rank` desc then uri, each with
    /// its [`limits::DISCUSSION_COMMENTS`] newest `Visible` comments (newest first). Paged
    /// like `list_posts` (`limit` 0: the default page size).
    pub async fn recent_discussion(
        &self,
        forum: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<DiscussionList, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let entries = l
            .model
            .view
            .orderings
            .as_ref()
            .map_err(|p| fold_pending(*p))?
            .recent_discussion
            .clone();
        let (offset, limit) = page_of(cursor, limit)?;
        let (page, cursor) = page(&entries, offset, limit);
        let uris: Vec<String> = page
            .iter()
            .flat_map(|r| {
                std::iter::once(r.post.clone())
                    .chain(r.comments.iter().take(limits::DISCUSSION_COMMENTS).cloned())
            })
            .collect();
        let (items, completeness, banners) = self
            .with_views(&l, &uris, &uris, |v| {
                let items = page
                    .iter()
                    .filter_map(|r| {
                        Some(DiscussionItem {
                            post: v.post_summary(&r.post)?,
                            comments: r
                                .comments
                                .iter()
                                .take(limits::DISCUSSION_COMMENTS)
                                .filter_map(|c| v.comment_node(c, &r.post))
                                .collect(),
                        })
                    })
                    .collect();
                (items, v.completeness(), v.banners())
            })
            .await?;
        Ok(DiscussionList {
            dto: DtoVersion::default(),
            items,
            cursor,
            completeness,
            banners,
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

    /// `mod_queue(forum)` (`social.md` §4.12), for the forum's authorities: applicants,
    /// top-rank tag-head conflicts, stale positive assertions, and (open mode) the newest
    /// newcomer items. `blocked` for anyone else.
    pub async fn mod_queue(&self, forum: &str) -> Result<ModQueue, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let me = self.me().await?;
        if !me.as_deref().is_some_and(|d| l.model.is_authority(d)) {
            return Err(EngineError::new(
                ErrorCode::Blocked,
                "the mod queue is for the forum's moderators",
            ));
        }
        let v = &l.model.view;
        let mut people: Vec<String> = v.participation.applicants.iter().cloned().collect();
        people.extend(v.moderation.records.values().map(|r| r.actor.clone()));
        if let Ok(vis) = &v.visibility {
            people.extend(vis.states.keys().cloned());
        }
        self.with_views(&l, &[], &people, |v| v.mod_queue()).await
    }

    /// `mod_log(forum, cursor?, limit)` (`social.md` §4.12): every listed modAction of the forum
    /// (any scope, any actor) and every revision that edited or removed one, with its status,
    /// by time desc then uri. Public; the reason of a hide with `showReason: false` is shown to
    /// the forum's authorities only.
    pub async fn mod_log(
        &self,
        forum: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<ModLog, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let (offset, limit) = page_of(cursor, limit)?;
        let people: Vec<String> = self
            .with_views(&l, &[], &[], |v| v.mod_log_actors().into_iter().collect())
            .await?;
        let entries = self
            .with_views(&l, &[], &people, |v| v.mod_log_entries())
            .await?;
        let (entries, cursor) = page(&entries, offset, limit);
        Ok(ModLog {
            dto: DtoVersion::default(),
            entries,
            cursor,
        })
    }

    /// `raw(uri)` (`social.md` §4.12): the stored record JSON, its validity violations, every
    /// modAction naming it (valid or not, with its mod-log status), its displayed state and the
    /// fold's score. The raw view shows everything, hidden content included (§10.3).
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
        let mut score = None;
        let mut root = None;
        if let Some(l) = &loaded {
            let m = &l.model;
            // a comment's thread: its record's root, if the fold placed it there
            if let Some(r) = m.comment_root(&uri_s)
                && m.view
                    .threads
                    .get(r)
                    .is_some_and(|pl| pl.parent.contains_key(&uri_s))
            {
                root = Some(r.to_owned());
            }
            let naming: Vec<(String, String, bool, Option<String>, Value)> = m
                .input
                .records
                .values()
                .filter(|x| x.coll == Some(Collection::ModAction))
                .filter_map(|x| {
                    let v = x.value()?;
                    (v.get("subject")?.as_str()? == uri_s).then(|| {
                        (
                            x.uri.clone(),
                            x.cid.clone(),
                            x.valid,
                            x.rule.clone(),
                            v.clone(),
                        )
                    })
                })
                .collect();
            let people = [uri_s.clone()];
            let (st, sc, statuses) = self
                .with_views(l, &[], &people, |v| {
                    let statuses: Vec<Option<String>> = naming
                        .iter()
                        .map(|(u, ..)| {
                            v.mod_status(u).and_then(|s| {
                                serde_json::to_value(s)
                                    .ok()
                                    .and_then(|j| j.as_str().map(str::to_owned))
                            })
                        })
                        .collect();
                    (v.item_state(&uri_s), v.raw_score(&uri_s), statuses)
                })
                .await?;
            state = st;
            score = sc;
            for ((auri, cid, valid, rule, record), status) in naming.into_iter().zip(statuses) {
                let mut a = serde_json::json!({
                    "uri": auri, "cid": cid, "valid": valid, "record": record,
                });
                if let Some(r) = rule.filter(|_| !valid) {
                    a["rule"] = Value::String(r);
                }
                if let Some(s) = status {
                    a["status"] = Value::String(s);
                }
                assertions.push(a);
            }
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
                    score,
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
            score,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages() {
        let items: Vec<u32> = (0..5).collect();
        assert_eq!(page(&items, 0, 2), (vec![0, 1], Some("2".into())));
        assert_eq!(page(&items, 4, 2), (vec![4], None));
        assert_eq!(page(&items, 9, 2), (vec![], None));
        assert_eq!(
            page_of(None, 0).unwrap().1,
            limits::LIST_PAGE_DEFAULT as usize
        );
        assert!(page_of(Some("x"), 1).is_err());
    }
}
