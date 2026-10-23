// SPDX-License-Identifier: AGPL-3.0-or-later
//! Moderation writes (`engine-api.md` §3.2, §3.5; `data-model.md` §6.7, §8.4, §8.6;
//! `social.md` §4): `mod_action` (assert or remove the acting account's assertion), `mod_adopt`,
//! `mods_set` (`mods rm` adopts first), `forum_update` (the declaration through the edit
//! protocol), and `approve` / `reject` of applicants.
//!
//! Every write is the active account's. Before enqueueing, an assertion is checked against the
//! account's fold of the forum, as UX (readers enforce the rules in the fold, `social.md` §8):
//!
//! | Scope | Checks |
//! |---|---|
//! | forum | the actor is an authority (§4.3 condition 2), the rank rule (3) and the role table (4) |
//! | author | `policy.authorModeration`, the actor is established, the §4.9 table (own post or thread, the author gate by the post's tier, authorities immune) |
//!
//! Positive assertions (keep, tier ≥ frontpage, pinned) are bound to the subject's cached CID
//! (`subjectCid`, §4.5); a negative one on a record carries it too when the record is cached. A
//! lock's allow-list is the thread's comments that are `Visible` or `Collapsed` in the actor's
//! view, sorted by uri; above [`limits::LOCK_ALLOWED_MAX`] the lock is refused and the moderator
//! is told to hide instead (§4.8).
//!
//! Local rows (`state` store): `("loc", "rejected", F, did)`, applicants dismissed by
//! [`Engine::reject`] on this device.

use std::collections::BTreeSet;

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::ns::NS;
use lessdugin_lexicon::records::{
    AccessAction, Action, HideAction, LockAction, ModAction, Moderator, PinAction, RestrictAction,
    StrongRef, TagAction, TierAction,
};
use lessdugin_lexicon::values::{
    AccessLevel as LexAccess, Capability as LexCapability, MembershipMode as LexMode,
    PinScope as LexPinScope, Tier as LexTier,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::discovery::forum_ref::ForumKey;
use crate::dto::{
    AccessLevel, ActionTicket, Capability, DefaultTier, ForumDraft, MembershipMode,
    ModActionIntent, ModActionValue, ModRole, ModScope, ModsChange, NoticeCode, PinScope, Tier,
    Topic, WriteTicket,
};
use crate::host::{Dir, Host, KeyRange, KvOp, KvStore};
use crate::limits;
use crate::outbox::{self, parse_write_id, records};
use crate::social::full::ForumView;
use crate::social::full::view::State;
use crate::social::input::{Skel, split_uri};
use crate::store::records::RecordStore;
use crate::store::row;

use super::forums::markdown_body;
use super::models::{Loaded, store_error};
use super::{Engine, EngineError};

const ROW_V: u32 = 1;

/// `("loc", "rejected", F, did)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RejectedRow {
    v: u32,
    /// When rejected (ms).
    at: i64,
}

fn rejected_key(f: &str, did: &str) -> Vec<u8> {
    crate::key!("loc", "rejected", f, did)
}

/// The retroactivity notice the mod UI must show for a restriction (`social.md` §4.7).
pub const RETROACTIVE_RESTRICT: &str = "this restriction is retroactive: it also hides the user's earlier items of the restricted kinds";
/// The notice for a ban (`social.md` §4.6).
pub const RETROACTIVE_BAN: &str =
    "a ban is retroactive: it hides all of the user's items in this forum, unless they are kept";

/// A modAction subject, normalised (`data-model.md` §6.7: a DID, a canonical at-uri or
/// `tag:` + slug).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Subject {
    Did(String),
    Uri { uri: String, coll: String },
    Tag(String),
}

impl Subject {
    fn as_str(&self) -> &str {
        match self {
            Self::Did(d) => d,
            Self::Uri { uri, .. } => uri,
            Self::Tag(t) => t,
        }
    }

    /// `subjectAuthor(T)` (§4.1).
    fn author(&self) -> Option<&str> {
        match self {
            Self::Did(d) => Some(d),
            Self::Uri { uri, .. } => split_uri(uri).map(|(d, _, _)| d),
            Self::Tag(_) => None,
        }
    }

    fn uri_of(&self, colls: &[&str]) -> Option<&str> {
        match self {
            Self::Uri { uri, coll } if colls.contains(&coll.as_str()) => Some(uri),
            _ => None,
        }
    }
}

fn lex_tier(t: Tier) -> LexTier {
    match t {
        Tier::Personal => LexTier::Personal,
        Tier::Frontpage => LexTier::Frontpage,
        Tier::Curated => LexTier::Curated,
    }
}

fn lex_capability(c: Capability) -> LexCapability {
    match c {
        Capability::Post => LexCapability::Post,
        Capability::Comment => LexCapability::Comment,
        Capability::Vote => LexCapability::Vote,
        Capability::React => LexCapability::React,
        Capability::Tag => LexCapability::Tag,
        Capability::Wiki => LexCapability::Wiki,
        Capability::Sequence => LexCapability::Sequence,
    }
}

fn role_name(rank: u8) -> &'static str {
    match rank {
        3 => "owner",
        2 => "admin",
        1 => "moderator",
        _ => "member",
    }
}

/// The minimal action whose `actionKey` is `key` (for the record key of a removal).
fn action_of_key(key: &str) -> Option<Map<String, Value>> {
    let (ty, scope) = match key {
        "hide" => (nsid::MOD_ACTION_HIDE, None),
        "access" => (nsid::MOD_ACTION_ACCESS, None),
        "restrict" => (nsid::MOD_ACTION_RESTRICT, None),
        "tier" => (nsid::MOD_ACTION_TIER, None),
        "pin:forum" => (nsid::MOD_ACTION_PIN, Some("forum")),
        "pin:thread" => (nsid::MOD_ACTION_PIN, Some("thread")),
        "lock" => (nsid::MOD_ACTION_LOCK, None),
        "tag" => (nsid::MOD_ACTION_TAG, None),
        _ => return None,
    };
    let mut m = Map::new();
    m.insert("$type".into(), Value::String(ty.to_owned()));
    if let Some(s) = scope {
        m.insert("scope".into(), Value::String(s.to_owned()));
    }
    Some(m)
}

/// The record `scope` written for a scope (absent means `forum`).
fn scope_field(scope: ModScope) -> Option<String> {
    match scope {
        ModScope::Forum => None,
        ModScope::Author => Some("author".to_owned()),
    }
}

/// The comments of `post`'s thread that are `Visible` or `Collapsed` in `view` (a lock's
/// allow-list, `data-model.md` §8.6), sorted ascending by uri bytes.
pub(crate) fn lock_allow_list(
    input: &crate::social::input::ForumInput,
    view: &ForumView,
    post: &str,
) -> Vec<String> {
    let states = view.visibility.as_ref().ok().map(|v| &v.states);
    let mut out: BTreeSet<String> = BTreeSet::new();
    for x in input.records.values() {
        let Some((Some(root), _)) = x.comment_refs() else {
            continue;
        };
        if root.uri != post {
            continue;
        }
        let listed = states
            .and_then(|s| s.get(&x.uri))
            .is_some_and(State::listed);
        if listed {
            out.insert(x.uri.clone());
        }
    }
    out.into_iter().collect()
}

/// The root post of comment `c` in the fold input, if it is there.
fn root_of<'a>(input: &'a crate::social::input::ForumInput, c: &str) -> Option<&'a Skel> {
    let (root, _) = input.records.get(c)?.comment_refs()?;
    input.records.get(&root?.uri)
}

/// The forum-scope checks (`social.md` §4.3 conditions 2–4) of `me` asserting `action` on `s`.
fn forum_scope_checks(
    v: &ForumView,
    me: &str,
    s: &Subject,
    action: &ModActionValue,
    author_hint: bool,
) -> Result<(), EngineError> {
    let Some(&rank) = v.authorities.get(me) else {
        let hint = if author_hint {
            "; as the post's author you can act in author scope"
        } else {
            ""
        };
        return Err(EngineError::blocked(format!(
            "only the forum's owner, admins and moderators can moderate it{hint}"
        )));
    };
    let subject_rank = s
        .author()
        .and_then(|a| v.authorities.get(a))
        .copied()
        .unwrap_or(0);
    let owner = me == v.owner;
    if !(owner || s.author() == Some(me) || rank > subject_rank) {
        return Err(EngineError::blocked(format!(
            "a {} cannot moderate {} (the higher rank decides; ask the owner or an admin)",
            role_name(rank),
            match subject_rank {
                3 => "the owner",
                2 => "an admin",
                _ => "another moderator",
            }
        )));
    }
    if let ModActionValue::Tier {
        tier: Tier::Curated,
        ..
    } = action
        && rank < 2
    {
        return Err(EngineError::blocked(
            "only the forum's owner and admins can curate",
        ));
    }
    Ok(())
}

/// The author-scope checks (`social.md` §4.9) of `me` asserting `action` on `s`.
fn author_scope_checks(
    l: &Loaded,
    me: &str,
    s: &Subject,
    action: &ModActionValue,
) -> Result<(), EngineError> {
    use crate::social::full::gates::{Gate, passes};
    let v = &l.model.view;
    let input = &l.model.input;
    if !v.policy.author_moderation {
        return Err(EngineError::blocked(
            "this forum does not allow author moderation",
        ));
    }
    let karma = v.karma.as_ref().ok();
    if karma.is_some_and(|k| !k.e.contains(me)) {
        return Err(EngineError::blocked(
            "author moderation needs you to be established in this forum",
        ));
    }
    // authorGate(X, p) ⇔ X ∈ A ∨ K[X] ≥ the gate of p's tier
    let gate = |p: &str| -> Result<(), EngineError> {
        let g = match v.tiers.get(p).copied().unwrap_or(v.moderation.default_tier) {
            LexTier::Personal => Gate::AuthorModPersonal,
            _ => Gate::AuthorModFrontpage,
        };
        match karma {
            Some(k) if !passes(&v.policy, &v.authorities, k, me, g) => {
                Err(EngineError::blocked(format!(
                    "author moderation on this post needs {} karma in this forum (you have {})",
                    g.value(&v.policy),
                    k.k_of(me)
                ))
                .with_detail("gate", g.name()))
            }
            _ => Ok(()),
        }
    };
    let own_thread = |c: &str| -> Result<String, EngineError> {
        let root = root_of(input, c).ok_or_else(|| {
            EngineError::not_found(format!("the thread of {c} is not in the cache; sync it"))
        })?;
        if root.did != me {
            return Err(EngineError::blocked(
                "author moderation acts only in the threads of your own posts",
            ));
        }
        Ok(root.uri.clone())
    };
    let immune = |d: &str| -> Result<(), EngineError> {
        if v.authorities.contains_key(d) {
            Err(EngineError::blocked(
                "the forum's authorities are immune to author moderation",
            ))
        } else {
            Ok(())
        }
    };
    match (action, s) {
        (
            ModActionValue::Hide {
                hidden: true,
                spam: false,
                ..
            },
            Subject::Uri { uri, coll },
        ) if coll == nsid::COMMENT => {
            let p = own_thread(uri)?;
            immune(s.author().unwrap_or_default())?;
            gate(&p)
        }
        (
            ModActionValue::Access {
                level: AccessLevel::Banned,
            },
            Subject::Did(d),
        ) => immune(d),
        (
            ModActionValue::Pin {
                scope: PinScope::Thread,
                ..
            },
            Subject::Uri { uri, coll },
        ) if coll == nsid::COMMENT => {
            let p = own_thread(uri)?;
            gate(&p)
        }
        (ModActionValue::Lock { .. }, Subject::Uri { uri, coll }) if coll == nsid::POST => {
            if s.author() != Some(me) {
                return Err(EngineError::blocked(
                    "author moderation can lock only your own posts",
                ));
            }
            gate(uri)
        }
        (ModActionValue::Hide { spam: true, .. }, _) => Err(EngineError::invalid_input(
            "spam does not apply in author scope; ask a moderator",
        )),
        _ => Err(EngineError::invalid_input(
            "author scope allows only hiding comments in your threads, banning from your threads, \
             pinning comments in your threads and locking your posts (MA6)",
        )),
    }
}

impl<H: Host> Engine<H> {
    /// A modAction subject from user input: `tag:<slug>`, an at-uri (with a DID authority), a
    /// DID, or a handle (resolved).
    async fn mod_subject(&self, input: &str) -> Result<Subject, EngineError> {
        if let Some(slug) = input.strip_prefix("tag:") {
            if !keys::is_slug(slug) {
                return Err(EngineError::invalid_input(format!(
                    "{slug:?} is not a tag slug"
                )));
            }
            return Ok(Subject::Tag(input.to_owned()));
        }
        if input.starts_with("at://") {
            let u = records::at_uri(input)?;
            return Ok(Subject::Uri {
                uri: u.as_str().to_owned(),
                coll: u.collection().as_str().to_owned(),
            });
        }
        if input.starts_with("did:") {
            let d = Did::parse(input)
                .map_err(|e| EngineError::invalid_ref(format!("{input:?}: {e}")))?;
            return Ok(Subject::Did(d.to_string()));
        }
        Ok(Subject::Did(self.resolve_actor(input).await?.did))
    }

    /// The cached CID of `uri`, if cached.
    async fn cached_cid(&self, uri: &str) -> Result<Option<String>, EngineError> {
        let u = records::at_uri(uri)?;
        Ok(RecordStore::new(self.host.cache())
            .get(&u)
            .await
            .map_err(store_error)?
            .map(|r| r.cid.to_string()))
    }

    /// `mod_action(ModActionIntent)` (`engine-api.md` §3.5, `data-model.md` §6.7, §8.4): the
    /// active account asserts (create-if-absent, else an update through the edit protocol) or
    /// removes (the removal protocol, with its `delete` revision) its own assertion for
    /// (scope, subject, action key).
    ///
    /// Refused before enqueueing (`blocked`) when the account's fold of the forum shows it would
    /// have no effect: forum scope needs an authority with the rank and role (`social.md`
    /// §4.3); author scope needs the §4.9 conditions. `invalid_input`: a subject of the wrong
    /// kind for the action, an action author scope does not allow, a lock that would keep more
    /// than 5,000 comments (detail `hint: "useHide"`: hide the thread instead). `not_found`: a
    /// positive assertion on an uncached subject, or a removal of an assertion the account does
    /// not have. Restrictions and bans carry a [`NoticeCode::Retroactive`] notice.
    pub async fn mod_action(&self, intent: &ModActionIntent) -> Result<ActionTicket, EngineError> {
        let session = self.active_session().await?;
        let me = session.did.as_str().to_owned();
        match intent {
            ModActionIntent::Remove {
                forum,
                scope,
                subject,
                action_key,
            } => {
                let key = self.forum_key(forum).await?;
                let s = self.mod_subject(subject).await?;
                let action = action_of_key(action_key).ok_or_else(|| {
                    EngineError::invalid_input(format!("unknown action key {action_key:?}"))
                })?;
                let rkey = keys::mod_action_rkey(
                    NS,
                    key.uri.as_str(),
                    scope_field(*scope).as_deref(),
                    s.as_str(),
                    &action,
                )
                .map_err(|e| EngineError::invalid_input(e.to_string()))?;
                let uri = format!("at://{me}/{}/{rkey}", nsid::MOD_ACTION);
                if self.cached_cid(&uri).await?.is_none() {
                    return Err(EngineError::not_found(format!(
                        "you have no {action_key} assertion on {} in this forum (to override \
                         another moderator, assert the opposite value)",
                        s.as_str()
                    )));
                }
                Ok(ActionTicket::new(self.outbox.delete(&session, &uri).await?))
            }
            ModActionIntent::Assert {
                forum,
                scope,
                subject,
                action,
                reason,
                until_ms,
            } => {
                let key = self.forum_key(forum).await?;
                let l = self.loaded(&key).await?;
                let s = self.mod_subject(subject).await?;
                let m = self
                    .assertion(&key, &l, &me, *scope, &s, action, reason, *until_ms)
                    .await?;
                let mut t = ActionTicket::new(self.outbox.mod_assert(&session, &m).await?);
                match action {
                    ModActionValue::Restrict { capabilities, .. } if !capabilities.is_empty() => {
                        t = t.with_notice(NoticeCode::Retroactive, RETROACTIVE_RESTRICT);
                    }
                    ModActionValue::Access {
                        level: AccessLevel::Banned,
                    } => t = t.with_notice(NoticeCode::Retroactive, RETROACTIVE_BAN),
                    _ => {}
                }
                Ok(t)
            }
        }
    }

    /// The checked record of an assertion.
    #[allow(clippy::too_many_arguments)]
    async fn assertion(
        &self,
        key: &ForumKey,
        l: &Loaded,
        me: &str,
        scope: ModScope,
        s: &Subject,
        action: &ModActionValue,
        reason: &Option<String>,
        until_ms: Option<i64>,
    ) -> Result<ModAction, EngineError> {
        use nsid::{COMMENT, POST, SEQUENCE, WIKI_REVISION};
        let v = &l.model.view;
        let wrong = |what: &str| {
            Err(EngineError::invalid_input(format!(
                "{} applies to {what}, not {}",
                action.action_key(),
                s.as_str()
            )))
        };
        // the subject kinds of data-model.md §6.7
        let (lex, positive) = match action {
            ModActionValue::Hide {
                hidden,
                spam,
                show_reason,
            } => {
                if s.uri_of(&[POST, COMMENT, SEQUENCE, WIKI_REVISION])
                    .is_none()
                {
                    return wrong("a post, comment, sequence or wiki revision");
                }
                let mut h = HideAction::hide();
                h.hidden = (!hidden).then_some(false);
                h.spam = (*spam && *hidden).then_some(true);
                h.show_reason = (!show_reason).then_some(false);
                (Action::Hide(h), !hidden)
            }
            ModActionValue::Access { level } => {
                if !matches!(s, Subject::Did(_)) {
                    return wrong("a user (DID)");
                }
                let level = match level {
                    AccessLevel::Banned => LexAccess::Banned,
                    AccessLevel::Member => LexAccess::Member,
                };
                (Action::Access(AccessAction::new(level)), false)
            }
            ModActionValue::Restrict {
                capabilities,
                nullify_votes,
            } => {
                if !matches!(s, Subject::Did(_)) {
                    return wrong("a user (DID)");
                }
                let caps: BTreeSet<LexCapability> =
                    capabilities.iter().copied().map(lex_capability).collect();
                let caps: Vec<LexCapability> = caps.into_iter().collect();
                (
                    Action::Restrict(RestrictAction::new(&caps, *nullify_votes)),
                    false,
                )
            }
            ModActionValue::Tier { tier, notice } => {
                if s.uri_of(&[POST]).is_none() {
                    return wrong("a post");
                }
                let mut t = TierAction::new(lex_tier(*tier));
                t.notice = notice.as_ref().map(markdown_body).transpose()?;
                (Action::Tier(t), *tier != Tier::Personal)
            }
            ModActionValue::Pin {
                scope: pin_scope,
                pinned,
            } => {
                let (kinds, what, lex_scope) = match pin_scope {
                    PinScope::Forum => (
                        &[POST, SEQUENCE][..],
                        "a post or sequence",
                        LexPinScope::Forum,
                    ),
                    PinScope::Thread => (&[COMMENT][..], "a comment", LexPinScope::Thread),
                };
                if s.uri_of(kinds).is_none() {
                    return wrong(what);
                }
                let mut p = PinAction::new(lex_scope);
                p.pinned = (!pinned).then_some(false);
                (Action::Pin(p), *pinned)
            }
            ModActionValue::Lock { locked } => {
                let Some(post) = s.uri_of(&[POST]) else {
                    // a forum uri is reserved (forum-wide locks are P2, MA9)
                    return wrong("a post");
                };
                let lock = if *locked {
                    let allowed = lock_allow_list(&l.model.input, v, post);
                    if allowed.len() > limits::LOCK_ALLOWED_MAX {
                        return Err(EngineError::invalid_input(format!(
                            "this thread has {} visible comments; a lock may keep at most {} \
                             visible, so hide the thread instead",
                            allowed.len(),
                            limits::LOCK_ALLOWED_MAX
                        ))
                        .with_detail("hint", "useHide")
                        .with_detail("allowed", allowed.len()));
                    }
                    let allowed = allowed
                        .iter()
                        .map(|u| records::at_uri(u))
                        .collect::<Result<Vec<AtUri>, _>>()?;
                    LockAction::new(allowed)
                } else {
                    let mut l = LockAction::new(Vec::new());
                    l.locked = Some(false);
                    l.allowed = None;
                    l
                };
                (Action::Lock(lock), false)
            }
            ModActionValue::Tag { removed, head } => {
                if !matches!(s, Subject::Tag(_)) {
                    return wrong("a tag (tag:<slug>)");
                }
                let head = match head {
                    Some(h) => {
                        let u = records::at_uri(h)?;
                        if u.collection().as_str() != WIKI_REVISION {
                            return Err(EngineError::invalid_input(format!(
                                "a tag head is a wiki revision, not {u}"
                            )));
                        }
                        let cid = self.cached_cid(h).await?.ok_or_else(|| {
                            EngineError::not_found(format!("{h} is not in the cache; sync it"))
                        })?;
                        Some(StrongRef::new(u, cid))
                    }
                    None => None,
                };
                (
                    Action::Tag(TagAction::new(removed.then_some(true), head)),
                    false,
                )
            }
        };
        match scope {
            ModScope::Forum => {
                let author_hint = s.author() == Some(me)
                    || matches!(s, Subject::Uri { uri, .. }
                        if root_of(&l.model.input, uri).is_some_and(|r| r.did == me));
                forum_scope_checks(v, me, s, action, author_hint)?;
            }
            ModScope::Author => author_scope_checks(l, me, s, action)?,
        }
        let now = records::datetime_ms(self.host.now_ms())?;
        let mut m = ModAction::new(key.uri.clone(), s.as_str(), lex, now);
        m.scope = scope_field(scope);
        if let Subject::Uri { uri, .. } = s {
            m.subject_cid = self.cached_cid(uri).await?;
            if positive && m.subject_cid.is_none() {
                return Err(EngineError::not_found(format!(
                    "{uri} is not in the cache; sync it first (the assertion is bound to the \
                     version you see)"
                )));
            }
        }
        m.reason = reason.clone().filter(|r| !r.trim().is_empty());
        m.until = until_ms.map(records::datetime_ms).transpose()?;
        Ok(m)
    }

    /// The active account's role in a loaded forum, if it is an authority.
    fn role_in(v: &ForumView, me: &str) -> Option<&'static str> {
        v.authorities.get(me).map(|r| role_name(*r))
    }

    /// `mod_adopt(forum, former)` (`data-model.md` §8.6, `social.md` §4.10): the active account,
    /// an authority of the forum, re-issues the former (or departing) moderator's non-lapsed
    /// forum-scope assertions with `adopts` pointing at each original. `former` is a DID or a
    /// handle; its PDS is resolved now. What could not be adopted is reported in the write's
    /// result (`cannot adopt …`, `kept own assertion …`).
    pub async fn mod_adopt(&self, forum: &str, former: &str) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let role = Self::role_in(&l.model.view, session.did.as_str()).ok_or_else(|| {
            EngineError::blocked("only the forum's owner, admins and moderators can adopt")
        })?;
        let (did, pds) = self.did_and_pds(former).await?;
        self.outbox
            .mod_adopt(&session, &key.uri, &did, &pds, role)
            .await
    }

    /// The DID and PDS of an actor.
    async fn did_and_pds(&self, actor: &str) -> Result<(Did, String), EngineError> {
        let r = self.resolve_identity(actor).await?;
        let pds = r.identity.pds.clone().ok_or_else(|| {
            EngineError::not_found(format!(
                "{actor}: the DID document names no PDS ({})",
                r.identity.pds_problem.clone().unwrap_or_default()
            ))
        })?;
        Ok((r.identity.did, pds))
    }

    /// The cached declaration of a forum the active account owns: `(session, cid, value)`.
    async fn own_declaration(
        &self,
        key: &ForumKey,
    ) -> Result<(outbox::Session, String, Value), EngineError> {
        let session = self.active_session().await?;
        if session.did != key.owner {
            return Err(EngineError::blocked(
                "only the forum's owner can change it (the declaration is in the owner's repo)",
            ));
        }
        let (cid, value) = self.current(&key.uri).await?;
        Ok((session, cid, value))
    }

    /// `mods_set(forum, ModsChange)` (`data-model.md` §8.4, §8.6; `social.md` §4.10), by the
    /// owner: edits `moderators` in the declaration through the edit protocol. `Add` adds the
    /// entry or replaces the DID's entry (a role change). `Remove` first enqueues `mod adopt`
    /// of the moderator's assertions, then the declaration edit, which is sent only once the
    /// adoption is done (it fails unsent if the adoption failed: run `mods rm` again); the
    /// result carries a [`NoticeCode::AdoptedFirst`] notice naming the adoption's write. With
    /// `skipAdopt` the moderator's assertions lapse at once (clients confirm first).
    pub async fn mods_set(
        &self,
        forum: &str,
        change: &ModsChange,
    ) -> Result<ActionTicket, EngineError> {
        let key = self.forum_key(forum).await?;
        let (session, base, mut value) = self.own_declaration(&key).await?;
        let mods: Vec<Value> = value
            .get("moderators")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let did_of = |m: &Value| m.get("did").and_then(Value::as_str).map(str::to_owned);
        let (ModsChange::Add { did, .. } | ModsChange::Remove { did, .. }) = change;
        let Subject::Did(target) = self.mod_subject(did).await? else {
            return Err(EngineError::invalid_input(format!("{did:?} is not a user")));
        };
        let summary = format!(
            "mods {} {target}",
            if matches!(change, ModsChange::Add { .. }) {
                "add"
            } else {
                "rm"
            }
        );
        if target == key.owner.as_str() {
            return Err(EngineError::invalid_input(
                "the owner is not listed as a moderator (they hold every right)",
            ));
        }
        let mut kept: Vec<Value> = mods
            .iter()
            .filter(|m| did_of(m).as_deref() != Some(target.as_str()))
            .cloned()
            .collect();
        let mut after = None;
        let mut notices = Vec::new();
        match change {
            ModsChange::Add { role, title, .. } => {
                let m = Moderator {
                    did: Did::parse(&target).map_err(|e| EngineError::internal(e.to_string()))?,
                    role: match role {
                        ModRole::Admin => "admin",
                        ModRole::Moderator => "moderator",
                    }
                    .to_owned(),
                    title: title.clone(),
                    extra: Map::new(),
                };
                // the DID's entry keeps its position on a role change
                let mv = serde_json::to_value(&m)
                    .map_err(|e| EngineError::internal(format!("encode: {e}")))?;
                match mods
                    .iter()
                    .position(|m| did_of(m).as_deref() == Some(target.as_str()))
                {
                    Some(i) => {
                        kept = mods.clone();
                        kept[i] = mv;
                    }
                    None => kept.push(mv),
                }
            }
            ModsChange::Remove { skip_adopt, .. } => {
                if kept.len() == mods.len() {
                    return Err(EngineError::not_found(format!(
                        "{target} is not a moderator of this forum"
                    )));
                }
                if !skip_adopt {
                    let pds = self.did_and_pds(&target).await?.1;
                    let former =
                        Did::parse(&target).map_err(|e| EngineError::internal(e.to_string()))?;
                    let t = self
                        .outbox
                        .mod_adopt(&session, &key.uri, &former, &pds, "owner")
                        .await?;
                    after = parse_write_id(&t.id).map(|(_, seq)| seq);
                    notices.push(format!(
                        "mod adopt runs first (write {}); the moderator is removed only once it \
                         completes",
                        t.id
                    ));
                }
            }
        }
        if let Some(o) = value.as_object_mut() {
            if kept.is_empty() {
                o.remove("moderators");
            } else {
                o.insert("moderators".into(), Value::Array(kept));
            }
        }
        let opts = outbox::EditOpts {
            summary: Some(summary),
            ..outbox::EditOpts::default()
        };
        let t = self
            .outbox
            .edit_after(&session, key.uri.as_str(), &value, &base, opts, after)
            .await?;
        Ok(notices.into_iter().fold(ActionTicket::new(t), |t, m| {
            t.with_notice(NoticeCode::AdoptedFirst, m)
        }))
    }

    /// `forum_update(uri, ForumDraft)` (`engine-api.md` §3.2, `data-model.md` §8.4), by the
    /// owner: the declaration's name, description, about, membership mode, default tier,
    /// featured tags and report contact, through the edit protocol (a revision of the old
    /// version; unknown fields and the policy are kept). The slug is fixed; `moderators` is
    /// changed only with [`Engine::mods_set`] (an empty list leaves them as they are; a
    /// different one is refused, since removing a moderator adopts first).
    pub async fn forum_update(
        &self,
        forum: &str,
        draft: &ForumDraft,
    ) -> Result<WriteTicket, EngineError> {
        let key = self.forum_key(forum).await?;
        if !draft.slug.is_empty() && draft.slug != key.slug {
            return Err(EngineError::invalid_input(
                "a forum's slug is fixed after creation",
            ));
        }
        let (session, base, mut value) = self.own_declaration(&key).await?;
        if !draft.moderators.is_empty() {
            let current: BTreeSet<(String, String, Option<String>)> = value
                .get("moderators")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    Some((
                        m.get("did")?.as_str()?.to_owned(),
                        m.get("role")?.as_str()?.to_owned(),
                        m.get("title").and_then(Value::as_str).map(str::to_owned),
                    ))
                })
                .collect();
            let wanted: BTreeSet<(String, String, Option<String>)> = draft
                .moderators
                .iter()
                .map(|m| {
                    (
                        m.did.clone(),
                        match m.role {
                            ModRole::Admin => "admin",
                            ModRole::Moderator => "moderator",
                        }
                        .to_owned(),
                        m.title.clone(),
                    )
                })
                .collect();
            if current != wanted {
                return Err(EngineError::invalid_input(
                    "change moderators with mods_set (removing a moderator adopts their \
                     assertions first)",
                ));
            }
        }
        let o = value
            .as_object_mut()
            .ok_or_else(|| EngineError::internal("the declaration is not an object"))?;
        let set = |o: &mut Map<String, Value>, k: &str, v: Option<Value>| match v {
            Some(v) => {
                o.insert(k.to_owned(), v);
            }
            None => {
                o.remove(k);
            }
        };
        set(o, "name", Some(Value::String(draft.name.clone())));
        set(
            o,
            "description",
            draft.description.clone().map(Value::String),
        );
        let about = draft.about.as_ref().map(markdown_body).transpose()?;
        set(
            o,
            "about",
            about
                .map(|a| serde_json::to_value(&a))
                .transpose()
                .map_err(|e| EngineError::internal(format!("encode: {e}")))?,
        );
        let mode = match draft.membership {
            MembershipMode::Open => LexMode::Open,
            MembershipMode::Approval => LexMode::Approval,
        };
        set(
            o,
            "membership",
            Some(Value::String(mode.as_str().to_owned())),
        );
        let tier = match draft.default_tier {
            DefaultTier::Personal => "personal",
            DefaultTier::Frontpage => "frontpage",
        };
        set(o, "defaultTier", Some(Value::String(tier.to_owned())));
        set(
            o,
            "featuredTags",
            (!draft.featured_tags.is_empty()).then(|| {
                Value::Array(
                    draft
                        .featured_tags
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                )
            }),
        );
        set(
            o,
            "reportContact",
            draft.report_contact.clone().map(Value::String),
        );
        self.outbox
            .edit(
                &session,
                key.uri.as_str(),
                &value,
                &base,
                outbox::EditOpts::default(),
            )
            .await
    }

    /// `approve(forum, actors)` (`social.md` §3.3, approval mode): grants each actor (DID or
    /// handle) membership with an `#access{member}` assertion by the active account, which
    /// must be an authority of the forum, and drops them from this device's rejected list.
    pub async fn approve(
        &self,
        forum: &str,
        actors: &[String],
    ) -> Result<Vec<WriteTicket>, EngineError> {
        let key = self.forum_key(forum).await?;
        let l = self.loaded(&key).await?;
        let me = self.active_session().await?.did.to_string();
        if Self::role_in(&l.model.view, &me).is_none() {
            return Err(EngineError::blocked(
                "only the forum's owner, admins and moderators can approve members",
            ));
        }
        let tickets = self.grant(key.uri.as_str(), actors).await?;
        let mut ops = Vec::new();
        for a in actors {
            let d = self.mod_subject(a).await?;
            ops.push(KvOp::Delete(rejected_key(key.uri.as_str(), d.as_str())));
        }
        self.host.state().write(ops).await?;
        self.rt.bus.changed([Topic::Forum {
            uri: key.uri.to_string(),
        }]);
        Ok(tickets)
    }

    /// `reject(forum, actors)`: dismisses applicants on this device (they leave the pending
    /// list; nothing public is written for them), and removes the active account's own
    /// `#access{member}` grant of an actor if it has one (the removal protocol). Returns the
    /// removals' tickets.
    pub async fn reject(
        &self,
        forum: &str,
        actors: &[String],
    ) -> Result<Vec<WriteTicket>, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(forum).await?;
        let now = self.host.now_ms();
        let mut ops = Vec::new();
        let mut tickets = Vec::new();
        for a in actors {
            let Subject::Did(d) = self.mod_subject(a).await? else {
                return Err(EngineError::invalid_input(format!("{a:?} is not a user")));
            };
            let row = RejectedRow { v: ROW_V, at: now };
            ops.push(KvOp::Put(
                rejected_key(key.uri.as_str(), &d),
                row::encode(&row).map_err(|e| EngineError::internal(e.to_string()))?,
            ));
            let grant = ModAction::new(
                key.uri.clone(),
                d.as_str(),
                Action::Access(AccessAction::new(LexAccess::Member)),
                records::datetime_ms(now)?,
            );
            let uri = records::mod_action(&grant)?.uri(&session.did);
            let own_grant = RecordStore::new(self.host.cache())
                .get(&records::at_uri(&uri)?)
                .await
                .map_err(store_error)?
                .and_then(|r| r.value_json().ok())
                .is_some_and(|v| {
                    v.get("action")
                        .and_then(|a| a.get("level"))
                        .and_then(Value::as_str)
                        == Some("member")
                });
            if own_grant {
                tickets.push(self.outbox.delete(&session, &uri).await?);
            }
        }
        self.host.state().write(ops).await?;
        self.rt.bus.changed([Topic::Forum {
            uri: key.uri.to_string(),
        }]);
        Ok(tickets)
    }

    /// The applicants of forum `forum` rejected on this device ([`Engine::reject`]), by DID.
    /// The members view drops them from the pending list.
    pub async fn rejected_applicants(&self, forum: &str) -> Result<BTreeSet<String>, EngineError> {
        let key = self.forum_key(forum).await?;
        let prefix = crate::key!("loc", "rejected", key.uri.as_str());
        let mut range = KeyRange::prefix(&prefix);
        let mut out = BTreeSet::new();
        loop {
            let page = self
                .host
                .state()
                .scan(range.clone(), Dir::Forward, 256)
                .await?;
            let full = page.len() >= 256;
            let last = page.last().map(|(k, _)| k.clone());
            for (k, _) in page {
                if let Ok(parts) = crate::store::key::decode(&k)
                    && let Some(crate::store::key::Part::Bytes(b)) = parts.get(3)
                    && let Ok(d) = String::from_utf8(b.clone())
                {
                    out.insert(d);
                }
            }
            match last {
                Some(k) if full => range.start = std::ops::Bound::Excluded(k),
                _ => return Ok(out),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_keys_match_the_asserted_records() {
        let forum = AtUri::parse(
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.forum/meta",
        )
        .unwrap();
        let now = records::datetime_ms(0).unwrap();
        let subject = "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.post/x";
        let cases = [
            ("hide", Action::Hide(HideAction::hide())),
            (
                "access",
                Action::Access(AccessAction::new(LexAccess::Banned)),
            ),
            ("restrict", Action::Restrict(RestrictAction::new(&[], true))),
            ("tier", Action::Tier(TierAction::new(LexTier::Curated))),
            ("pin:forum", Action::Pin(PinAction::new(LexPinScope::Forum))),
            (
                "pin:thread",
                Action::Pin(PinAction::new(LexPinScope::Thread)),
            ),
            ("lock", Action::Lock(LockAction::new(Vec::new()))),
            ("tag", Action::Tag(TagAction::new(Some(true), None))),
        ];
        for scope in [ModScope::Forum, ModScope::Author] {
            for (k, a) in &cases {
                let mut m = ModAction::new(forum.clone(), subject, a.clone(), now.clone());
                m.scope = scope_field(scope);
                let built = records::mod_action(&m).unwrap();
                let removal = keys::mod_action_rkey(
                    NS,
                    forum.as_str(),
                    scope_field(scope).as_deref(),
                    subject,
                    &action_of_key(k).unwrap(),
                )
                .unwrap();
                assert_eq!(built.rkey, removal, "{k} {scope:?}");
            }
        }
        assert!(action_of_key("pin").is_none());
    }
}
