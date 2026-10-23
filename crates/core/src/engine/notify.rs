// SPDX-License-Identifier: AGPL-3.0-or-later
//! Notifications and read state (`engine-api.md` §3.4, §3.6; `social.md` §16): local only,
//! computed from first-seen times, never from `createdAt`.
//!
//! - `notifications(q)` first brings every followed forum's notifications up to date for the
//!   active account ([`crate::notify::compute`] over the forum's model), stores the new ones,
//!   then lists them, newest first. The first computation for an (account, forum) records a
//!   baseline and notifies nothing (a new device does not replay history).
//! - `mark_notifications_read(ids)` / `mark_all_notifications_read()`.
//! - `mark_read(uri)`: a post (thread) — `lastVisited[post]` becomes the greatest `t_obs` among
//!   the post and its listed comments, and the thread's notifications are read; a forum — the
//!   forum's post list is visited (its new-post count restarts).
//! - `unread_counts()`: the notification badge and every followed forum's new posts, unread
//!   comments (in threads visited before) and unread notifications.
//!
//! Notifications are not computed in the background: clients re-query `notifications` or
//! `unread_counts` on `Changed` with a `Forum`, `Thread` or `Notifications` topic. Every
//! change of the stored read state emits `Changed` (`Notifications`, and the thread or forum
//! for read marks).
//!
//! Rows (`state` store): `("loc", "read", post)` and `("loc", "fvisit", F)` (µs marks, per
//! device); `("loc", "nbase", account, F)` ([`Baseline`]); `("loc", "notif", account, id)`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{
    DtoVersion, ForumUnread, KarmaCadence, NotificationList, NotificationReason, NotificationView,
    NotificationsQuery, Subscription, Topic, UnreadCounts,
};
use crate::host::{Host, KvOp, KvStore};
use crate::model::views::bare_actor;
use crate::notify::{self, Baseline, Subs};
use crate::outbox::records;
use crate::store::row;

use super::models::Loaded;
use super::notify_local::encode;
use super::{Engine, EngineError};

const ROW_V: u32 = 1;

/// At most this many notifications are kept per account (the oldest read ones go first).
pub const NOTIFICATIONS_KEPT: usize = 1_000;

/// A stored notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotifRow {
    v: u32,
    id: String,
    reason: NotificationReason,
    at_ms: i64,
    subject: String,
    forum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
    #[serde(default)]
    read: bool,
}

/// A read mark (µs, `t_obs` scale).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct MarkRow {
    v: u32,
    at: i64,
}

fn read_key(post: &str) -> Vec<u8> {
    crate::key!("loc", "read", post)
}

fn visit_key(forum: &str) -> Vec<u8> {
    crate::key!("loc", "fvisit", forum)
}

fn base_key(account: &str, forum: &str) -> Vec<u8> {
    crate::key!("loc", "nbase", account, forum)
}

fn notif_key(account: &str, id: &str) -> Vec<u8> {
    crate::key!("loc", "notif", account, id)
}

fn cadence_ms(c: KarmaCadence) -> i64 {
    match c {
        KarmaCadence::Realtime => 0,
        KarmaCadence::Daily => 24 * 60 * 60 * 1000,
        KarmaCadence::Weekly => 7 * 24 * 60 * 60 * 1000,
    }
}

fn mark(at: i64) -> MarkRow {
    MarkRow { v: ROW_V, at }
}

impl<H: Host> Engine<H> {
    /// `lastVisited[post]` (µs), if the thread was viewed on this device. The views count a
    /// thread's unread comments with it ([`crate::notify::unread_comments`]).
    pub async fn read_mark(&self, post: &str) -> Result<Option<i64>, EngineError> {
        Ok(self.loc_row::<MarkRow>(read_key(post)).await?.map(|m| m.at))
    }

    /// The subscriptions that concern forum `f`.
    fn subs_of(all: &[Subscription], f: &str) -> Subs {
        let mut s = Subs::default();
        for x in all {
            match x {
                Subscription::Forum { uri } if uri == f => s.forum = true,
                Subscription::Post { uri } => {
                    s.posts.insert(uri.clone());
                }
                Subscription::User { did } => {
                    s.users.insert(did.clone());
                }
                Subscription::Tag { forum, slug } if forum == f => {
                    s.tags.insert(slug.clone());
                }
                _ => {}
            }
        }
        s
    }

    /// The followed forums' loaded models (a forum that cannot be loaded is skipped).
    async fn followed_models(&self) -> Result<Vec<(ForumKey, Loaded)>, EngineError> {
        let mut out = Vec::new();
        for key in self.followed_keys().await? {
            match self.loaded(&key).await {
                Ok(l) => out.push((key, l)),
                Err(e) => tracing::debug!(forum = %key.uri, error = %e, "notifications: skipped"),
            }
        }
        Ok(out)
    }

    /// Brings `account`'s notifications up to date over `models`; returns `true` if a row was
    /// added.
    async fn refresh_notifications(
        &self,
        account: &str,
        models: &[(ForumKey, Loaded)],
    ) -> Result<bool, EngineError> {
        let subs: Vec<Subscription> = self
            .subscriptions()
            .await?
            .into_iter()
            .map(|e| e.subscription)
            .collect();
        let every = cadence_ms(self.karma_notifications().await?);
        let now = self.host.now_ms();
        let mut ops = Vec::new();
        let mut added = false;
        for (key, l) in models {
            let f = key.uri.as_str();
            let base: Option<Baseline> = self.loc_row(base_key(account, f)).await?;
            let m = &l.model;
            let s = Self::subs_of(&subs, f);
            let new_uris: Vec<String> = match &base {
                Some(b) => notify::new_items(&m.input, &m.view, account, b.obs)
                    .iter()
                    .map(|x| x.uri.clone())
                    .collect(),
                None => Vec::new(),
            };
            let values: BTreeMap<String, Value> = self.values(m, &new_uris).await?;
            let inp = notify::Input {
                me: account,
                input: &m.input,
                view: &m.view,
                subs: &s,
                values: &values,
                now_ms: now,
                karma_every_ms: every,
            };
            let (notes, next) = notify::compute(&inp, base.as_ref());
            if base.as_ref() != Some(&next) {
                ops.push(encode(base_key(account, f), &next)?);
            }
            for n in notes {
                let k = notif_key(account, &n.id);
                if self.loc_row::<NotifRow>(k.clone()).await?.is_some() {
                    continue;
                }
                added = true;
                ops.push(encode(
                    k,
                    &NotifRow {
                        v: ROW_V,
                        id: n.id,
                        reason: n.reason,
                        at_ms: n.at_ms,
                        subject: n.subject,
                        forum: f.to_owned(),
                        actor: n.actor,
                        preview: n.preview,
                        read: false,
                    },
                )?);
            }
        }
        if !ops.is_empty() {
            self.host.state().write(ops).await?;
        }
        if added {
            self.prune_notifications(account).await?;
        }
        Ok(added)
    }

    /// Every stored notification of `account`.
    async fn notif_rows(&self, account: &str) -> Result<Vec<NotifRow>, EngineError> {
        Ok(self
            .loc_rows(crate::key!("loc", "notif", account))
            .await?
            .into_iter()
            .filter_map(|(_, v)| row::decode::<NotifRow>(&v).ok())
            .collect())
    }

    /// Keeps at most [`NOTIFICATIONS_KEPT`] notifications: the oldest read ones go first.
    async fn prune_notifications(&self, account: &str) -> Result<(), EngineError> {
        let mut rows = self.notif_rows(account).await?;
        if rows.len() <= NOTIFICATIONS_KEPT {
            return Ok(());
        }
        rows.sort_by(|a, b| {
            b.read
                .cmp(&a.read)
                .then(a.at_ms.cmp(&b.at_ms))
                .then_with(|| a.id.cmp(&b.id))
        });
        let excess = rows.len() - NOTIFICATIONS_KEPT;
        let ops = rows
            .iter()
            .take(excess)
            .map(|r| KvOp::Delete(notif_key(account, &r.id)))
            .collect();
        self.host.state().write(ops).await?;
        Ok(())
    }

    /// `notifications(NotificationsQuery)` (`engine-api.md` §3.4, `social.md` §16): the active
    /// account's notifications in its followed forums, unread only unless `all`, newest first
    /// (then by id), at most `limit`; `unread` counts every unread one. Empty without an active
    /// account. Brings them up to date first (over the cached models; it does not sync).
    pub async fn notifications(
        &self,
        q: &NotificationsQuery,
    ) -> Result<NotificationList, EngineError> {
        let Some(me) = self.me().await? else {
            return Ok(NotificationList {
                dto: DtoVersion::default(),
                items: Vec::new(),
                unread: 0,
            });
        };
        let models = self.followed_models().await?;
        self.refresh_notifications(&me, &models).await?;
        let mut rows = self.notif_rows(&me).await?;
        let unread = u32::try_from(rows.iter().filter(|r| !r.read).count()).unwrap_or(u32::MAX);
        rows.retain(|r| q.all || !r.read);
        rows.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| a.id.cmp(&b.id)));
        rows.truncate(usize::try_from(q.limit).unwrap_or(usize::MAX));
        let dids: BTreeSet<&str> = rows.iter().filter_map(|r| r.actor.as_deref()).collect();
        let (actors, _) = self.actors(dids).await;
        let items = rows
            .into_iter()
            .map(|r| NotificationView {
                actor: r
                    .actor
                    .as_deref()
                    .map(|d| actors.get(d).cloned().unwrap_or_else(|| bare_actor(d))),
                id: r.id,
                reason: r.reason,
                at_ms: r.at_ms,
                subject: r.subject,
                forum: Some(r.forum),
                read: r.read,
                preview: r.preview,
            })
            .collect();
        Ok(NotificationList {
            dto: DtoVersion::default(),
            items,
            unread,
        })
    }

    /// Marks the active account's notifications `ids` read (unknown ids are ignored). Emits
    /// `Changed [Notifications]` if one changed.
    pub async fn mark_notifications_read(&self, ids: &[String]) -> Result<(), EngineError> {
        let Some(me) = self.me().await? else {
            return Ok(());
        };
        let want: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        self.set_read(&me, |r| want.contains(r.id.as_str())).await
    }

    /// Marks every notification of the active account read.
    pub async fn mark_all_notifications_read(&self) -> Result<(), EngineError> {
        let Some(me) = self.me().await? else {
            return Ok(());
        };
        self.set_read(&me, |_| true).await
    }

    async fn set_read(
        &self,
        account: &str,
        pick: impl Fn(&NotifRow) -> bool,
    ) -> Result<(), EngineError> {
        let mut ops = Vec::new();
        for mut r in self.notif_rows(account).await? {
            if !r.read && pick(&r) {
                r.read = true;
                ops.push(encode(notif_key(account, &r.id), &r)?);
            }
        }
        if !ops.is_empty() {
            self.host.state().write(ops).await?;
            self.rt.bus.changed([Topic::Notifications]);
        }
        Ok(())
    }

    /// `mark_read(uri)` (`engine-api.md` §3.6, `social.md` §16). For a post: `lastVisited`
    /// becomes the greatest `t_obs` among the post and its listed comments (never lower than
    /// before), and the active account's notifications about the thread are read. For a forum:
    /// its post list is visited (the new-post count of [`Engine::unread_counts`] restarts).
    /// Emits `Changed` for the thread (or forum) and, if notifications changed,
    /// `Notifications`.
    pub async fn mark_read(&self, uri: &str) -> Result<(), EngineError> {
        let u = records::at_uri(uri)?;
        let coll = u.collection().as_str();
        let forum_coll = lessdugin_lexicon::collection::nsid::FORUM;
        let post_coll = lessdugin_lexicon::collection::nsid::POST;
        if coll == forum_coll {
            let key = self.forum_key(uri).await?;
            let l = self.loaded(&key).await?;
            let at = notify::forum_obs(&l.model.input, &l.model.view).unwrap_or(0);
            let before = self
                .loc_row::<MarkRow>(visit_key(key.uri.as_str()))
                .await?
                .map_or(0, |m| m.at);
            self.host
                .state()
                .write(vec![encode(
                    visit_key(key.uri.as_str()),
                    &mark(at.max(before)),
                )?])
                .await?;
            self.rt.bus.changed([Topic::Forum {
                uri: key.uri.to_string(),
            }]);
            return Ok(());
        }
        if coll != post_coll {
            return Err(EngineError::invalid_ref(format!(
                "{u} is not a post or a forum"
            )));
        }
        let (_, value) = self.current(&u).await?;
        let forum = value
            .get("forum")
            .and_then(Value::as_str)
            .ok_or_else(|| EngineError::invalid_input(format!("{u} names no forum")))?;
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let at = notify::thread_obs(&l.model.input, &l.model.view, u.as_str()).unwrap_or(0);
        let before = self.read_mark(u.as_str()).await?.unwrap_or(0);
        self.host
            .state()
            .write(vec![encode(read_key(u.as_str()), &mark(at.max(before)))?])
            .await?;
        let mut topics = vec![
            Topic::Thread {
                uri: u.as_str().to_owned(),
            },
            Topic::Forum {
                uri: key.uri.to_string(),
            },
        ];
        if let Some(me) = self.me().await? {
            let post = u.as_str().to_owned();
            let input = &l.model.input;
            let in_thread = |s: &str| {
                s == post
                    || input
                        .records
                        .get(s)
                        .and_then(|x| x.comment_refs())
                        .and_then(|(r, _)| r)
                        .is_some_and(|r| r.uri == post)
            };
            let before = self.notif_rows(&me).await?;
            let unread_before = before.iter().filter(|r| !r.read).count();
            self.set_read(&me, |r| in_thread(&r.subject)).await?;
            let unread_after = self
                .notif_rows(&me)
                .await?
                .iter()
                .filter(|r| !r.read)
                .count();
            if unread_after != unread_before {
                topics.push(Topic::Notifications);
            }
        }
        self.rt.bus.changed(topics);
        Ok(())
    }

    /// `unread_counts()` (`social.md` §16): brings the active account's notifications up to
    /// date, then counts, per followed forum, the listed posts first seen since the forum's
    /// post list was last visited, the listed comments first seen since each visited thread was
    /// last viewed (threads never viewed are counted as new posts instead), and the unread
    /// notifications; mine are never counted. A forum's first count records the visit mark
    /// (what was synced before is not new). Sorted by forum uri.
    pub async fn unread_counts(&self) -> Result<UnreadCounts, EngineError> {
        let me = self.me().await?;
        let models = self.followed_models().await?;
        let mut per_forum: BTreeMap<String, u32> = BTreeMap::new();
        let mut total = 0;
        if let Some(me) = &me {
            self.refresh_notifications(me, &models).await?;
            for r in self.notif_rows(me).await? {
                if !r.read {
                    total += 1;
                    *per_forum.entry(r.forum).or_default() += 1;
                }
            }
        }
        let mut forums = Vec::new();
        let mut ops = Vec::new();
        for (key, l) in &models {
            let f = key.uri.as_str();
            let (input, view) = (&l.model.input, &l.model.view);
            let visit = match self.loc_row::<MarkRow>(visit_key(f)).await? {
                Some(m) => m.at,
                None => {
                    let at = notify::max_obs(input);
                    ops.push(encode(visit_key(f), &mark(at))?);
                    at
                }
            };
            let posts = notify::unread_posts(input, view, visit, me.as_deref());
            let mut comments = 0u32;
            for x in input.records.values() {
                if x.coll != Some(lessdugin_lexicon::Collection::Post) {
                    continue;
                }
                if let Some(m) = self.read_mark(&x.uri).await? {
                    comments = comments.saturating_add(notify::unread_comments(
                        input,
                        view,
                        &x.uri,
                        m,
                        me.as_deref(),
                    ));
                }
            }
            forums.push(ForumUnread {
                forum: f.to_owned(),
                posts,
                comments,
                notifications: per_forum.get(f).copied().unwrap_or(0),
            });
        }
        if !ops.is_empty() {
            self.host.state().write(ops).await?;
        }
        forums.sort_by(|a, b| a.forum.cmp(&b.forum));
        Ok(UnreadCounts {
            dto: DtoVersion::default(),
            notifications: total,
            forums,
        })
    }
}
