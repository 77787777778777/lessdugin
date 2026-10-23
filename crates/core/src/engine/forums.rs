// SPDX-License-Identifier: AGPL-3.0-or-later
//! Forums and membership (`engine-api.md` §3.2): `forum_open`, `forum_create`, the local
//! followed list (`forum_follow`, `forum_unfollow`, `followed`, seeded from the accounts' own
//! `membership` records, `sync.md` §4.8), `join`, `leave`, `grant`, `revoke` and `members`.
//!
//! Local rows (`state` store, `sync.md` §10.4): `("loc", "follow", F)`, `("loc", "unfollow", F)`
//! (a local unfollow that seeding respects) and `("loc", "alias", F)`.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_atp::syntax::{AtIdentifier, AtUri, Did, Nsid};
use lessdugin_atp::xrpc::ListRecords;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{AccessAction, Action, Forum, Md, ModAction, Moderator};
use lessdugin_lexicon::values::{AccessLevel, DefaultTier as LexTier, MembershipMode as LexMode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{
    DefaultTier, ForumDraft, ForumSummary, ForumView, MemberView, MembersQuery, MembershipMode,
    ModRole, WriteTicket,
};
use crate::host::{Dir, Host, KeyRange, KvOp, KvStore};
use crate::limits;
use crate::model::views::{Views, bare_actor};
use crate::outbox::{ContentOpts, records};
use crate::social::input::split_uri;
use crate::store::records::RecordStore;
use crate::store::row;
use crate::xrpc::{CallOpts, Service, Xrpc};

use super::models::{Loaded, store_error};
use super::{Engine, EngineError, ErrorCode};

const ROW_V: u32 = 1;

/// `("loc", "follow", F)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FollowRow {
    v: u32,
    /// When followed (ms).
    at: i64,
    /// `true` if added by seeding from a membership record.
    #[serde(default)]
    seeded: bool,
}

/// `("loc", "unfollow", F)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct UnfollowRow {
    v: u32,
    at: i64,
}

/// `("loc", "alias", F)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct AliasRow {
    v: u32,
    alias: String,
}

/// `("loc", "seeded", did)`: the followed list was seeded from the account's memberships since
/// its login (`sync.md` §4.8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SeededRow {
    v: u32,
    at: i64,
}

/// The key of an account's [`SeededRow`] (deleted at login, so the list is seeded again).
pub(crate) fn seeded_key(did: &str) -> Vec<u8> {
    crate::key!("loc", "seeded", did)
}

fn follow_key(f: &str) -> Vec<u8> {
    crate::key!("loc", "follow", f)
}

fn unfollow_key(f: &str) -> Vec<u8> {
    crate::key!("loc", "unfollow", f)
}

fn alias_key(f: &str) -> Vec<u8> {
    crate::key!("loc", "alias", f)
}

fn state_error<E: std::fmt::Display>(e: E) -> EngineError {
    EngineError::internal(format!("local state: {e}")).with_retryable(true)
}

fn encode<T: Serialize>(key: Vec<u8>, v: &T) -> Result<KvOp, EngineError> {
    Ok(KvOp::Put(key, row::encode(v).map_err(state_error)?))
}

/// A markdown body without media (M2; media arrive with M4).
pub(crate) fn markdown_body(md: &crate::dto::BodyDraft) -> Result<Md, EngineError> {
    if !md.media.is_empty() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "media in bodies arrive with M4",
        ));
    }
    let meta = lessdugin_markdown::derive_metadata(&md.markdown, &[]);
    let mut m = lessdugin_lexicon::records::Markdown::inline(md.markdown.clone());
    // W8: derived hints
    if !meta.mentions.is_empty() {
        m.mentions = Some(meta.mentions);
    }
    if !meta.links.is_empty() {
        m.links = Some(meta.links);
    }
    Ok(Md::Known(m))
}

/// The excerpt a spilled body would carry (PO6).
pub(crate) fn excerpt_of(markdown: &str) -> String {
    lessdugin_markdown::derive_metadata(markdown, &[]).excerpt
}

/// A forum summary from the declaration value alone (a forum without a loaded model).
fn summary_from_decl(
    key: &ForumKey,
    decl: Option<&Forum>,
    followed: bool,
    member: bool,
    alias: Option<String>,
    owner: crate::dto::ActorView,
) -> ForumSummary {
    ForumSummary {
        uri: key.uri.to_string(),
        owner,
        name: decl.map_or_else(|| key.slug.clone(), |d| d.name.clone()),
        description: decl.and_then(|d| d.description.clone()),
        membership: match decl.map_or(LexMode::Approval, |d| d.membership_mode().value) {
            LexMode::Open => MembershipMode::Open,
            LexMode::Approval => MembershipMode::Approval,
        },
        followed,
        member,
        alias,
        report_contact: decl.and_then(|d| d.report_contact.clone()),
    }
}

impl<H: Host> Engine<H> {
    async fn loc_get<T: serde::de::DeserializeOwned>(
        &self,
        key: Vec<u8>,
    ) -> Result<Option<T>, EngineError> {
        let mut rows = self.host.state().get_many(vec![key]).await?;
        match rows.pop().flatten() {
            Some(b) => Ok(row::decode(&b).ok()),
            None => Ok(None),
        }
    }

    async fn loc_scan(&self, kind: &str) -> Result<Vec<(String, Vec<u8>)>, EngineError> {
        let prefix = crate::key!("loc", kind);
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
                let parts = crate::store::key::decode(&k).map_err(state_error)?;
                if let Some(crate::store::key::Part::Bytes(b)) = parts.get(2)
                    && let Ok(f) = String::from_utf8(b.clone())
                {
                    out.push((f, v));
                }
            }
            match last {
                Some(k) if full => range.start = std::ops::Bound::Excluded(k),
                _ => return Ok(out),
            }
        }
    }

    async fn follow_state(&self, f: &str) -> Result<(bool, Option<String>), EngineError> {
        let followed = self.loc_get::<FollowRow>(follow_key(f)).await?.is_some();
        let alias = self
            .loc_get::<AliasRow>(alias_key(f))
            .await?
            .map(|a| a.alias);
        Ok((followed, alias))
    }

    /// The views over a loaded forum for the active account. `body_uris`: the items whose
    /// bodies are rendered (their values are re-read); `people`: the DIDs, or record uris whose
    /// authors, the views show (the forum's authorities are always included). Actor views come
    /// from the identity cache only.
    pub(crate) async fn with_views<T>(
        &self,
        l: &Loaded,
        body_uris: &[String],
        people: &[String],
        f: impl FnOnce(&Views<'_>) -> T,
    ) -> Result<T, EngineError> {
        let m = &l.model;
        let dids: BTreeSet<&str> = m
            .view
            .authorities
            .keys()
            .map(String::as_str)
            .chain(people.iter().filter_map(|p| {
                if p.starts_with("did:") {
                    Some(p.as_str())
                } else {
                    split_uri(p).map(|(d, _, _)| d)
                }
            }))
            .collect();
        let (actors, pds) = self.actors(dids).await;
        let values = self.values(m, body_uris).await?;
        let pending = self.pending_uris().await?;
        let viewer = self.viewer(m, body_uris).await?;
        let v = Views {
            model: m,
            actors: &actors,
            pds: &pds,
            values: &values,
            renders: &self.rt.renders,
            pending: &pending,
            completeness: &l.completeness,
            viewer: &viewer,
        };
        Ok(f(&v))
    }

    /// The active account's DID.
    pub(crate) async fn me(&self) -> Result<Option<String>, EngineError> {
        Ok(self.active_account().await?.map(|a| a.did))
    }

    /// `forum_open(ref)` (`engine-api.md` §3.2): resolves the reference, fetches the
    /// declaration (with a proof on first open; an open within [`limits::FORUM_REOPEN_MS`] of
    /// the last one reuses it) and returns the header. It schedules the authority sync:
    /// authorities never synced are synced now (P0, within
    /// [`limits::FORUM_OPEN_SYNC_BUDGET_MS`]), so the header is not degraded; stale ones are
    /// queued for `live_tail`. Members are synced by [`Engine::sync_forum`].
    pub async fn forum_open(&self, forum: &str) -> Result<ForumView, EngineError> {
        let key = self.forum_key(forum).await?;
        let opened = self.open_key(&key, false).await?;
        self.sync_authorities(&opened).await;
        let l = self.loaded(&key).await?;
        let me = self.me().await?;
        let (followed, alias) = self.follow_state(key.uri.as_str()).await?;
        self.with_views(&l, &[], &[], |v| {
            v.forum_view(me.as_deref(), followed, alias, &opened.warnings)
        })
        .await
    }

    /// `forum_create(ForumDraft)` (`data-model.md` §8.4): the declaration and the owner's
    /// membership in one `applyWrites`, by the active account. The new forum is followed.
    pub async fn forum_create(&self, draft: &ForumDraft) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let now = records::datetime_ms(self.host.now_ms())?;
        let mode = match draft.membership {
            MembershipMode::Open => LexMode::Open,
            MembershipMode::Approval => LexMode::Approval,
        };
        let mut forum = Forum::create(draft.name.clone(), now, mode);
        forum.description.clone_from(&draft.description);
        forum.about = draft.about.as_ref().map(markdown_body).transpose()?;
        forum.default_tier = Some(
            match draft.default_tier {
                DefaultTier::Personal => LexTier::Personal,
                DefaultTier::Frontpage => LexTier::Frontpage,
            }
            .as_str()
            .to_owned(),
        );
        if !draft.featured_tags.is_empty() {
            forum.featured_tags = Some(draft.featured_tags.clone());
        }
        if !draft.moderators.is_empty() {
            let mut mods = Vec::with_capacity(draft.moderators.len());
            for m in &draft.moderators {
                mods.push(Moderator {
                    did: Did::parse(&m.did)
                        .map_err(|e| EngineError::invalid_input(format!("{:?}: {e}", m.did)))?,
                    role: match m.role {
                        ModRole::Admin => "admin",
                        ModRole::Moderator => "moderator",
                    }
                    .to_owned(),
                    title: m.title.clone(),
                    extra: Map::new(),
                });
            }
            forum.moderators = Some(mods);
        }
        forum.report_contact.clone_from(&draft.report_contact);
        let key = ForumKey::new(&session.did, &draft.slug)
            .map_err(|e| EngineError::invalid_input(e.to_string()))?;
        let t = self
            .outbox
            .create_forum(&session, &draft.slug, &forum, ContentOpts::default())
            .await?;
        self.rt.register(&key);
        self.follow_rows(key.uri.as_str(), None, false).await?;
        Ok(t)
    }

    async fn follow_rows(
        &self,
        f: &str,
        alias: Option<&str>,
        seeded: bool,
    ) -> Result<(), EngineError> {
        let mut ops = vec![
            encode(
                follow_key(f),
                &FollowRow {
                    v: ROW_V,
                    at: self.host.now_ms(),
                    seeded,
                },
            )?,
            KvOp::Delete(unfollow_key(f)),
        ];
        if let Some(a) = alias {
            ops.push(encode(
                alias_key(f),
                &AliasRow {
                    v: ROW_V,
                    alias: a.to_owned(),
                },
            )?);
        }
        self.host.state().write(ops).await?;
        self.rt
            .bus
            .changed([crate::dto::Topic::Forum { uri: f.to_owned() }]);
        Ok(())
    }

    /// `forum_follow(uri, alias?)` (local).
    pub async fn forum_follow(&self, forum: &str, alias: Option<&str>) -> Result<(), EngineError> {
        let key = self.forum_key(forum).await?;
        self.follow_rows(key.uri.as_str(), alias, false).await
    }

    /// `forum_unfollow(uri)` (local; seeding will not add it back).
    pub async fn forum_unfollow(&self, forum: &str) -> Result<(), EngineError> {
        let key = self.forum_key(forum).await?;
        let f = key.uri.as_str();
        self.host
            .state()
            .write(vec![
                KvOp::Delete(follow_key(f)),
                KvOp::Delete(alias_key(f)),
                encode(
                    unfollow_key(f),
                    &UnfollowRow {
                        v: ROW_V,
                        at: self.host.now_ms(),
                    },
                )?,
            ])
            .await?;
        self.rt
            .bus
            .changed([crate::dto::Topic::Forum { uri: f.to_owned() }]);
        Ok(())
    }

    /// Adds the forums of `did`'s valid own `membership` records to the followed list, except
    /// those unfollowed locally (`sync.md` §4.8). Reads the own repo's public `listRecords`
    /// anonymously from the PDS bound at login, as the outbox's fresh reads do: an
    /// authenticated session is for writes only (`threat-model.md` AUTH-4), so every host
    /// behaves the same here.
    async fn seed_follows(&self, did: &Did) -> Result<(), EngineError> {
        let xrpc = Xrpc::new(&*self.host);
        let session = self.session_for(did.as_str()).await?;
        let svc = Service::Anon(session.pds);
        let coll = Nsid::parse(nsid::MEMBERSHIP)
            .map_err(|e| EngineError::internal(format!("membership NSID: {e}")))?;
        let mut cursor: Option<String> = None;
        let mut forums: Vec<String> = Vec::new();
        for _ in 0..limits::FOLLOW_SEED_PAGES {
            let params = ListRecords {
                repo: AtIdentifier::Did(did.clone()),
                collection: coll.clone(),
                limit: Some(100),
                cursor: cursor.take(),
                reverse: None,
            };
            let page = xrpc
                .query_endpoint(
                    &svc,
                    &params,
                    &CallOpts::capped(limits::LIST_PAGE_MAX_BYTES),
                )
                .await?;
            for r in &page.records {
                let (Some(uri), Some(value)) = (r.uri.as_deref(), r.value.as_ref()) else {
                    continue;
                };
                let Ok(uri) = AtUri::parse(uri) else {
                    continue;
                };
                if uri.did() != did || !lessdugin_lexicon::validity::check_uri(&uri, value).valid {
                    continue;
                }
                if let Some(f) = value.get("forum").and_then(Value::as_str) {
                    forums.push(f.to_owned());
                }
            }
            match page.cursor {
                Some(c) if !page.records.is_empty() => cursor = Some(c),
                _ => break,
            }
        }
        for f in forums {
            let Ok(u) = AtUri::parse(&f) else { continue };
            let Ok(key) = ForumKey::of_uri(&u) else {
                continue;
            };
            let f = key.uri.as_str();
            let known = self.loc_get::<FollowRow>(follow_key(f)).await?.is_some()
                || self
                    .loc_get::<UnfollowRow>(unfollow_key(f))
                    .await?
                    .is_some();
            if !known {
                self.rt.register(&key);
                self.follow_rows(f, None, true).await?;
            }
        }
        Ok(())
    }

    /// The keys of the followed forums, by uri.
    pub(crate) async fn followed_keys(&self) -> Result<Vec<ForumKey>, EngineError> {
        Ok(self
            .loc_scan("follow")
            .await?
            .into_iter()
            .filter_map(|(f, _)| ForumKey::of_uri(&AtUri::parse(&f).ok()?).ok())
            .collect())
    }

    /// Seeds the followed list from the memberships of `did` and records that it did.
    pub(crate) async fn seed_account(&self, did: &Did) -> Result<(), EngineError> {
        self.seed_follows(did).await?;
        let row = SeededRow {
            v: ROW_V,
            at: self.host.now_ms(),
        };
        self.host
            .state()
            .write(vec![encode(seeded_key(did.as_str()), &row)?])
            .await?;
        Ok(())
    }

    /// Seeds the followed list of every account not seeded since its login (`sync.md` §4.8:
    /// on login; `live_tail` also re-seeds after each sync of the own repo). A failed read is
    /// logged and retried next time.
    pub(crate) async fn seed_pending(&self) -> Result<(), EngineError> {
        for a in self.accounts().await? {
            if self
                .loc_get::<SeededRow>(seeded_key(&a.did))
                .await?
                .is_some()
            {
                continue;
            }
            let Ok(did) = Did::parse(&a.did) else {
                continue;
            };
            if let Err(e) = self.seed_account(&did).await {
                tracing::warn!(account = %a.did, error = %e, "seeding followed forums");
            }
        }
        Ok(())
    }

    /// `followed()` (`engine-api.md` §3.2): the local followed list, seeded from the accounts'
    /// own membership records once after each login ([`Engine::seed_pending`]). A forum added
    /// by seeding counts as joined until the cache holds the membership (or its tombstone).
    /// Sorted by name, then uri.
    pub async fn followed(&self) -> Result<Vec<ForumSummary>, EngineError> {
        self.seed_pending().await?;
        let me = self.me().await?;
        let aliases: BTreeMap<String, String> = self
            .loc_scan("alias")
            .await?
            .into_iter()
            .filter_map(|(f, b)| Some((f, row::decode::<AliasRow>(&b).ok()?.alias)))
            .collect();
        let store = RecordStore::new(self.host.cache());
        let mut out = Vec::new();
        for (f, follow) in self.loc_scan("follow").await? {
            let seeded = row::decode::<FollowRow>(&follow).is_ok_and(|r| r.seeded);
            let Ok(u) = AtUri::parse(&f) else { continue };
            let Ok(key) = ForumKey::of_uri(&u) else {
                continue;
            };
            let decl: Option<Forum> = store
                .get(&key.uri)
                .await
                .map_err(store_error)?
                .filter(|r| r.valid)
                .and_then(|r| r.value_json().ok())
                .and_then(|v| serde_json::from_value(v).ok());
            let member = match &me {
                Some(me) => {
                    let m = format!("at://{me}/{}/{}", nsid::MEMBERSHIP, key.fk);
                    match AtUri::parse(&m) {
                        Ok(m) => match store.get(&m).await.map_err(store_error)? {
                            Some(r) => r.valid,
                            // seeded from the own PDS listing, not synced yet
                            None => {
                                seeded
                                    && store.tombstone_of(&m).await.map_err(store_error)?.is_none()
                            }
                        },
                        Err(_) => false,
                    }
                }
                None => false,
            };
            let (owner, _) = self.actors([key.owner.as_str()]).await;
            let owner = owner
                .get(key.owner.as_str())
                .cloned()
                .unwrap_or_else(|| bare_actor(key.owner.as_str()));
            out.push(summary_from_decl(
                &key,
                decl.as_ref(),
                true,
                member,
                aliases.get(&f).cloned(),
                owner,
            ));
        }
        out.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.uri.cmp(&b.uri))
        });
        Ok(out)
    }

    /// `join(uri, note?)`: create-if-absent of the membership (approval forums also need a
    /// grant). The forum is followed.
    pub async fn join(&self, forum: &str, note: Option<&str>) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        let t = self.outbox.join(&session, &key.uri, note).await?;
        self.follow_rows(key.uri.as_str(), None, false).await?;
        Ok(t)
    }

    /// `leave(uri)`: deletes the membership (the followed list is unchanged).
    pub async fn leave(&self, forum: &str) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        self.outbox.leave(&session, &key.uri).await
    }

    async fn subject_dids(&self, actors: &[String]) -> Result<Vec<String>, EngineError> {
        let mut out = Vec::with_capacity(actors.len());
        for a in actors {
            out.push(if a.starts_with("did:") {
                Did::parse(a)
                    .map_err(|e| EngineError::invalid_ref(format!("{a:?}: {e}")))?
                    .to_string()
            } else {
                self.resolve_actor(a).await?.did
            });
        }
        Ok(out)
    }

    fn access_action(key: &ForumKey, did: &str, now: lessdugin_atp::syntax::Datetime) -> ModAction {
        ModAction::new(
            key.uri.clone(),
            did,
            Action::Access(AccessAction::new(AccessLevel::Member)),
            now,
        )
    }

    /// `grant(uri, actors)`: an `#access{member}` assertion per actor (DID or handle) by the
    /// active account, which must be an authority of the forum to have an effect (approval
    /// mode, `social.md` §4.7).
    pub async fn grant(
        &self,
        forum: &str,
        actors: &[String],
    ) -> Result<Vec<WriteTicket>, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        let now = records::datetime_ms(self.host.now_ms())?;
        let mut out = Vec::new();
        for d in self.subject_dids(actors).await? {
            let m = Self::access_action(&key, &d, now.clone());
            out.push(self.outbox.mod_assert(&session, &m).await?);
        }
        Ok(out)
    }

    /// `revoke(uri, actors)`: removes the active account's `#access` assertions on the actors
    /// (the removal protocol with its `delete` revision).
    pub async fn revoke(
        &self,
        forum: &str,
        actors: &[String],
    ) -> Result<Vec<WriteTicket>, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        let now = records::datetime_ms(self.host.now_ms())?;
        let mut out = Vec::new();
        for d in self.subject_dids(actors).await? {
            let built = records::mod_action(&Self::access_action(&key, &d, now.clone()))?;
            out.push(
                self.outbox
                    .delete(&session, &built.uri(&session.did))
                    .await?,
            );
        }
        Ok(out)
    }

    /// `members(uri, MembersQuery)` (`social.md` §3): every DID with a valid membership
    /// record, by DID, with its role (owner, admin, moderator, member = a seed, established,
    /// newcomer, none), grant, karma and moderation flags. `pending: true` lists the applicants
    /// (approval mode: a membership without a grant); `status` keeps one role (`newcomer`: the
    /// newcomers).
    pub async fn members(
        &self,
        forum: &str,
        query: &MembersQuery,
    ) -> Result<Vec<MemberView>, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let people: Vec<String> = l.model.view.participation.members.iter().cloned().collect();
        let mut out = self
            .with_views(&l, &[], &people, |v| v.members(query))
            .await?;
        if query.pending == Some(true) {
            // the applicants rejected on this device leave the pending list
            let rejected = self.rejected_applicants(forum).await?;
            out.retain(|m| !rejected.contains(&m.actor.did));
        }
        Ok(out)
    }
}
