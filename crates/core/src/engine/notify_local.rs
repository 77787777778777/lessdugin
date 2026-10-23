// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local lists (`engine-api.md` §3.6, `social.md` §16–§17): subscriptions, bookmarks, mutes,
//! the acknowledged public-action interstitials and the karma-notification cadence. They live
//! in the `state` store only (nothing public is written) and are part of `export_local`.
//!
//! Rows: `("loc", "sub", <subscription JSON>)`, `("loc", "bookmark", uri)`, `("loc", "mute",
//! did)`, `("loc", "ack", account, kind)`, `("loc", "karmaCadence")`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::dto::{
    ActorView, BookmarkEntry, KarmaCadence, PublicActionKind, Subscription, SubscriptionEntry,
    Topic,
};
use crate::host::{Dir, Host, KeyRange, KvOp, KvStore};
use crate::store::key::{Part, decode};
use crate::store::row;

use super::{Engine, EngineError};

const ROW_V: u32 = 1;

/// A row that only records when it was made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AtRow {
    pub(crate) v: u32,
    /// When (ms).
    pub(crate) at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CadenceRow {
    v: u32,
    cadence: KarmaCadence,
}

fn state_error<E: std::fmt::Display>(e: E) -> EngineError {
    EngineError::internal(format!("local state: {e}")).with_retryable(true)
}

pub(crate) fn encode<T: Serialize>(key: Vec<u8>, v: &T) -> Result<KvOp, EngineError> {
    Ok(KvOp::Put(key, row::encode(v).map_err(state_error)?))
}

fn sub_key(s: &Subscription) -> Result<Vec<u8>, EngineError> {
    let j = serde_json::to_string(s).map_err(state_error)?;
    Ok(crate::key!("loc", "sub", j.as_str()))
}

fn bookmark_key(uri: &str) -> Vec<u8> {
    crate::key!("loc", "bookmark", uri)
}

fn mute_key(did: &str) -> Vec<u8> {
    crate::key!("loc", "mute", did)
}

fn ack_key(account: &str, kind: &str) -> Vec<u8> {
    crate::key!("loc", "ack", account, kind)
}

fn kind_name(k: PublicActionKind) -> &'static str {
    match k {
        PublicActionKind::Vote => "vote",
        PublicActionKind::React => "react",
        PublicActionKind::TagVote => "tagVote",
        PublicActionKind::Join => "join",
        PublicActionKind::ModAction => "modAction",
    }
}

const KINDS: [PublicActionKind; 5] = [
    PublicActionKind::Vote,
    PublicActionKind::React,
    PublicActionKind::TagVote,
    PublicActionKind::Join,
    PublicActionKind::ModAction,
];

impl<H: Host> Engine<H> {
    /// Every row under `("loc", kind, …)`: the key parts after `kind`, and the value.
    pub(crate) async fn loc_rows(
        &self,
        prefix: Vec<u8>,
    ) -> Result<Vec<(Vec<String>, Vec<u8>)>, EngineError> {
        let mut range = KeyRange::prefix(&prefix);
        let mut out = Vec::new();
        loop {
            let page = self
                .host
                .state()
                .scan(range.clone(), Dir::Forward, 256)
                .await?;
            let full = page.len() >= 256;
            let last = page.last().map(|(k, _)| k.clone());
            for (k, v) in page {
                let Ok(parts) = decode(&k) else { continue };
                let rest: Vec<String> = parts
                    .iter()
                    .skip(2)
                    .filter_map(|p| match p {
                        Part::Bytes(b) => String::from_utf8(b.clone()).ok(),
                        _ => None,
                    })
                    .collect();
                out.push((rest, v));
            }
            match last {
                Some(k) if full => range.start = std::ops::Bound::Excluded(k),
                _ => return Ok(out),
            }
        }
    }

    /// One `state` row, decoded (`None`: absent or unreadable).
    pub(crate) async fn loc_row<T: serde::de::DeserializeOwned>(
        &self,
        key: Vec<u8>,
    ) -> Result<Option<T>, EngineError> {
        let mut rows = self.host.state().get_many(vec![key]).await?;
        Ok(rows.pop().flatten().and_then(|b| row::decode(&b).ok()))
    }

    /// `subscribe(Subscription, on)` (`engine-api.md` §3.6; `Engine::subscribe` is the event
    /// subscription): a local subscription to new posts in a forum, by a user or with a tag, new
    /// comments on a post, or new entries of a sequence (`social.md` §16). Emits
    /// `Changed [Notifications]`.
    pub async fn set_subscription(&self, s: &Subscription, on: bool) -> Result<(), EngineError> {
        let key = sub_key(s)?;
        let op = if on {
            encode(
                key,
                &AtRow {
                    v: ROW_V,
                    at: self.host.now_ms(),
                },
            )?
        } else {
            KvOp::Delete(key)
        };
        self.host.state().write(vec![op]).await?;
        self.rt.bus.changed([Topic::Notifications]);
        Ok(())
    }

    /// `subscriptions()`: every local subscription, oldest first.
    pub async fn subscriptions(&self) -> Result<Vec<SubscriptionEntry>, EngineError> {
        let mut out: Vec<SubscriptionEntry> = self
            .loc_rows(crate::key!("loc", "sub"))
            .await?
            .into_iter()
            .filter_map(|(k, v)| {
                let subscription: Subscription = serde_json::from_str(k.first()?).ok()?;
                let at: AtRow = row::decode(&v).ok()?;
                Some(SubscriptionEntry {
                    subscription,
                    since_ms: at.at,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.since_ms
                .cmp(&b.since_ms)
                .then_with(|| a.subscription.cmp(&b.subscription))
        });
        Ok(out)
    }

    /// `bookmark(uri, on)` (local). Emits `Changed [Thread]` for the item.
    pub async fn bookmark(&self, uri: &str, on: bool) -> Result<(), EngineError> {
        let u = crate::outbox::records::at_uri(uri)?;
        let key = bookmark_key(u.as_str());
        let op = if on {
            encode(
                key,
                &AtRow {
                    v: ROW_V,
                    at: self.host.now_ms(),
                },
            )?
        } else {
            KvOp::Delete(key)
        };
        self.host.state().write(vec![op]).await?;
        self.rt.bus.changed([Topic::Thread {
            uri: u.as_str().to_owned(),
        }]);
        Ok(())
    }

    /// `bookmarks()`: newest first, with the post's summary when it can be built from the
    /// cache.
    pub async fn bookmarks(&self) -> Result<Vec<BookmarkEntry>, EngineError> {
        let mut rows: Vec<(String, i64)> = self
            .loc_rows(crate::key!("loc", "bookmark"))
            .await?
            .into_iter()
            .filter_map(|(k, v)| Some((k.first()?.clone(), row::decode::<AtRow>(&v).ok()?.at)))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut out = Vec::with_capacity(rows.len());
        for (uri, at) in rows {
            let post = if uri.contains(lessdugin_lexicon::collection::nsid::POST) {
                self.get_post(&uri).await.ok().map(|p| p.post)
            } else {
                None
            };
            out.push(BookmarkEntry {
                uri,
                saved_at_ms: at,
                post,
            });
        }
        Ok(out)
    }

    /// `true` if `uri` is bookmarked.
    pub async fn bookmarked(&self, uri: &str) -> Result<bool, EngineError> {
        Ok(self.loc_row::<AtRow>(bookmark_key(uri)).await?.is_some())
    }

    /// `mute(did, on)` (local; the lens collapses a muted author's listed items,
    /// `social.md` §10.3). Emits `Changed` for the user and every cached forum.
    pub async fn mute(&self, actor: &str, on: bool) -> Result<(), EngineError> {
        let did = if actor.starts_with("did:") {
            lessdugin_atp::syntax::Did::parse(actor)
                .map_err(|e| EngineError::invalid_ref(format!("{actor:?}: {e}")))?
                .to_string()
        } else {
            self.resolve_actor(actor).await?.did
        };
        let key = mute_key(&did);
        let op = if on {
            encode(
                key,
                &AtRow {
                    v: ROW_V,
                    at: self.host.now_ms(),
                },
            )?
        } else {
            KvOp::Delete(key)
        };
        self.host.state().write(vec![op]).await?;
        let mut topics = vec![Topic::User { did }];
        topics.extend(
            self.rt
                .loaded_forums()
                .into_iter()
                .map(|uri| Topic::Forum { uri }),
        );
        self.rt.bus.changed(topics);
        Ok(())
    }

    /// The muted DIDs (the lens, `social.md` §10.3).
    pub async fn muted(&self) -> Result<BTreeSet<String>, EngineError> {
        Ok(self
            .loc_rows(crate::key!("loc", "mute"))
            .await?
            .into_iter()
            .filter_map(|(k, _)| k.first().cloned())
            .collect())
    }

    /// `mutes()` (`engine-api.md` §3.4): the muted actors, by DID, with cached actor views.
    pub async fn mutes(&self) -> Result<Vec<ActorView>, EngineError> {
        let dids = self.muted().await?;
        let (actors, _) = self.actors(dids.iter().map(String::as_str)).await;
        Ok(actors.into_values().collect())
    }

    /// The public actions whose first-use interstitial the active account has acknowledged
    /// (`social.md` §17, `threat-model.md` PRIV-9), in the order of [`PublicActionKind`]. Empty
    /// without an active account. Clients show the interstitial before the first action of a
    /// kind not listed here, then call [`Engine::acknowledge_public_action`].
    pub async fn acknowledged_public_actions(&self) -> Result<Vec<PublicActionKind>, EngineError> {
        let Some(me) = self.me().await? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for k in KINDS {
            if self
                .loc_row::<AtRow>(ack_key(&me, kind_name(k)))
                .await?
                .is_some()
            {
                out.push(k);
            }
        }
        Ok(out)
    }

    /// `true` if the active account acknowledged the interstitial of `kind`.
    pub async fn public_action_acknowledged(
        &self,
        kind: PublicActionKind,
    ) -> Result<bool, EngineError> {
        Ok(self.acknowledged_public_actions().await?.contains(&kind))
    }

    /// Records that the active account saw the "this is public" interstitial of `kind`
    /// (persisted per account; shared by every client on this device's `state` store).
    pub async fn acknowledge_public_action(
        &self,
        kind: PublicActionKind,
    ) -> Result<(), EngineError> {
        let me = self.me().await?.ok_or_else(|| {
            EngineError::new(
                super::ErrorCode::AuthRequired,
                "no active account; log in first",
            )
        })?;
        let row = AtRow {
            v: ROW_V,
            at: self.host.now_ms(),
        };
        self.host
            .state()
            .write(vec![encode(ack_key(&me, kind_name(kind)), &row)?])
            .await?;
        self.rt.bus.changed([Topic::Accounts]);
        Ok(())
    }

    /// How often karma changes are notified (`social.md` §16; default daily).
    pub async fn karma_notifications(&self) -> Result<KarmaCadence, EngineError> {
        Ok(self
            .loc_row::<CadenceRow>(crate::key!("loc", "karmaCadence"))
            .await?
            .map_or(KarmaCadence::Daily, |r| r.cadence))
    }

    /// Sets the karma-notification cadence.
    pub async fn set_karma_notifications(&self, cadence: KarmaCadence) -> Result<(), EngineError> {
        let row = CadenceRow { v: ROW_V, cadence };
        self.host
            .state()
            .write(vec![encode(crate::key!("loc", "karmaCadence"), &row)?])
            .await?;
        self.rt.bus.changed([Topic::Notifications]);
        Ok(())
    }
}
