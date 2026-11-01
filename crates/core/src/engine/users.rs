// SPDX-License-Identifier: AGPL-3.0-or-later
//! User pages from cached/synced profiles and listed forum content.

use std::collections::BTreeMap;

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::Profile;
use serde_json::Value;

use crate::dto::{
    CommentNode, DtoVersion, ForumRole, ItemState, KarmaView, LinkView, PostSummary, UserView,
};
use crate::host::Host;
use crate::store::records::RecordStore;

use super::models::store_error;
use super::{Engine, EngineError};

impl<H: Host> Engine<H> {
    /// The synced lessdugin profile, if its current cached record is valid.
    async fn cached_profile(&self, did: &str) -> Result<Option<Profile>, EngineError> {
        let uri = format!("at://{did}/{}/self", nsid::PROFILE);
        let uri = lessdugin_atp::syntax::AtUri::parse(&uri)
            .map_err(|e| EngineError::invalid_ref(e.to_string()))?;
        let row = RecordStore::new(self.host.cache())
            .get(&uri)
            .await
            .map_err(store_error)?;
        Ok(row
            .filter(|row| row.valid)
            .and_then(|row| row.value_json().ok())
            .and_then(|value| serde_json::from_value(value).ok()))
    }

    /// A user page from cached/synced profiles and forum models. No profile or content is fetched
    /// on demand; only actor identity resolution may use the identity resolver.
    pub async fn user(&self, forum: Option<&str>, actor: &str) -> Result<UserView, EngineError> {
        let actor = self.resolve_actor(actor).await?;
        let did = actor.did.clone();
        let profile = self.cached_profile(&did).await?;
        let context = match forum {
            Some(forum) => Some(self.forum_key(forum).await?),
            None => None,
        };
        let mut keys = self.synced_forum_keys().await?;
        if let Some(key) = &context
            && !keys.iter().any(|k| k.uri == key.uri)
        {
            keys.push(key.clone());
        }
        keys.sort_by(|a, b| a.uri.as_str().cmp(b.uri.as_str()));
        keys.dedup_by(|a, b| a.uri == b.uri);
        let pins: Vec<String> = profile
            .as_ref()
            .and_then(|p| p.pinned.as_ref())
            .into_iter()
            .flatten()
            .map(ToString::to_string)
            .collect();
        let mut pinned = BTreeMap::<String, PostSummary>::new();
        let mut posts = BTreeMap::<String, PostSummary>::new();
        let mut comments = BTreeMap::<String, CommentNode>::new();
        let mut status = ForumRole::None;
        let mut karma = None;
        for key in keys {
            let is_context = context
                .as_ref()
                .is_some_and(|context| context.uri == key.uri);
            let loaded = match self.loaded(&key).await {
                Ok(loaded) => loaded,
                Err(e) if is_context => return Err(e),
                Err(_) => continue,
            };
            let model = &loaded.model;
            let uris: Vec<String> = model
                .input
                .records
                .iter()
                .filter(|(uri, record)| {
                    record.valid
                        && (pins.contains(uri)
                            || (record.did == did
                                && matches!(
                                    record.coll,
                                    Some(Collection::Post | Collection::Comment)
                                )))
                })
                .map(|(uri, _)| uri.clone())
                .collect();
            let (local_pinned, local_posts, local_comments, role) = self
                .with_views(&loaded, &uris, &uris, |views| {
                    let listed = |state: &ItemState| {
                        matches!(state, ItemState::Visible | ItemState::Collapsed { .. })
                    };
                    let mut local_pinned = Vec::new();
                    let mut local_posts = Vec::new();
                    let mut local_comments = Vec::new();
                    for uri in &uris {
                        if let Some(post) = views.post_summary(uri)
                            && listed(&post.state)
                        {
                            if pins.contains(uri) {
                                local_pinned.push(post.clone());
                            }
                            if post.author.did == did {
                                local_posts.push(post);
                            }
                            continue;
                        }
                        if let Some(root) = model.comment_root(uri)
                            && let Some(comment) = views.comment_node(uri, root)
                            && listed(&comment.state)
                            && comment.author.did == did
                        {
                            local_comments.push(comment);
                        }
                    }
                    (
                        local_pinned,
                        local_posts,
                        local_comments,
                        views.role_of(&did),
                    )
                })
                .await?;
            pinned.extend(
                local_pinned
                    .into_iter()
                    .map(|post| (post.uri.clone(), post)),
            );
            posts.extend(local_posts.into_iter().map(|post| (post.uri.clone(), post)));
            comments.extend(
                local_comments
                    .into_iter()
                    .map(|comment| (comment.uri.clone(), comment)),
            );
            if is_context {
                status = role;
                karma = loaded.model.view.karma.as_ref().ok().map(|k| KarmaView {
                    k1: k.k1_of(&did),
                    kp: k.kp_of(&did),
                    k: k.k_of(&did),
                });
            }
        }
        let mut recent_posts: Vec<_> = posts.into_values().collect();
        recent_posts.sort_by(|a, b| {
            b.t_rank_ms
                .cmp(&a.t_rank_ms)
                .then_with(|| a.uri.cmp(&b.uri))
        });
        recent_posts.truncate(10);
        let mut recent_comments: Vec<_> = comments.into_values().collect();
        recent_comments.sort_by(|a, b| {
            b.t_rank_ms
                .cmp(&a.t_rank_ms)
                .then_with(|| a.uri.cmp(&b.uri))
        });
        recent_comments.truncate(10);
        let bio = profile.as_ref().and_then(|profile| {
            let bio = profile.bio.as_ref()?;
            let value = serde_json::to_value(bio).ok()?;
            let text = value.get("text")?.as_str()?;
            let media = value.get("media").cloned().unwrap_or(Value::Null);
            let profile = value.get("profile").and_then(Value::as_str);
            Some(crate::model::render::render_markdown(text, &media, profile))
        });
        let links = profile
            .as_ref()
            .and_then(|profile| profile.links.as_ref())
            .into_iter()
            .flatten()
            .map(|link| LinkView {
                host: url::Url::parse(&link.uri)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default(),
                uri: link.uri.clone(),
                label: link.label.clone(),
            })
            .collect();
        let mut actor = actor;
        if actor.display_name.is_none() {
            actor.display_name = profile.as_ref().and_then(|p| p.display_name.clone());
        }
        Ok(UserView {
            dto: DtoVersion::default(),
            actor,
            forum: context.map(|key| key.uri.to_string()),
            karma,
            status,
            bio,
            links,
            pinned: pins
                .iter()
                .filter_map(|uri| pinned.get(uri).cloned())
                .collect(),
            recent_posts,
            recent_comments,
        })
    }
}
