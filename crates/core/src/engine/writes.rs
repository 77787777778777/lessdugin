// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writes (`engine-api.md` §3.5) through the durable outbox: `publish`, `comment`, `edit`,
//! `delete`, `retract`, `opinion`, `undo`, the write status, and the pump (`outbox_pump`,
//! long-lived; `outbox_flush` runs every account's outbox until idle). Moderation writes are in
//! `moderation.rs`.
//!
//! **Opinions** (`data-model.md` §8.5): one record per (voter, subject), computed when sent as a
//! pure function of the current value and the intents, held 4 s so it can be undone and
//! coalesced with later intents on the same subject, merged across devices (a swap failure
//! re-reads and re-applies the intents per axis), deleted when empty. Before enqueueing, the
//! engine checks the viewer's standing in its fold of the forum (`social.md` §5, §8, §9.2): what
//! could never count is refused (`blocked` or `invalid_input`), and a vote that will count only
//! once the voter is established is recorded with a notice.
//!
//! New content is written by the active account; edits, deletes and undos by the account that
//! owns the record or write (W9). Each returns a [`WriteTicket`] once the write is persisted;
//! progress arrives as `Write` events, and a confirmed write as `Changed` for its forum and
//! thread. Bodies are LFM/1 markdown with the derived `mentions`/`links` (W8); media are
//! refused until M4 (`unsupported`). A comment's or react's quote gets its derived
//! `prefix`/`suffix`/`start` from P of the target's cached version (W8). Handle-authority
//! `at://` destinations are rewritten to verified DIDs first (W7, `Engine::with_dids`).

use futures::StreamExt;
use futures::future::join_all;
use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{
    Comment, ExternalLink, Labels, Post, SelfLabels, StrongRef, TextQuote as LexQuote,
};
use serde_json::{Map, Value};

use crate::dto::{
    ActionNotice, ActionTicket, AgreementValue, CommentDraft, Draft, EditOpts, KarmaValue,
    NoticeCode, OpinionIntent, PostDraft, PostKind, Stance, TextQuote, WriteTicket,
};
use crate::host::Host;
use crate::outbox::{
    self, ContentOpts, OpinionStep, PumpReport, Session, Step, parse_write_id, records,
};
use crate::social::full::ForumView;
use crate::social::full::gates::{Gate, passes};
use crate::social::full::reacts::palette;
use crate::social::full::view::ModStatus;
use crate::social::input::split_uri;
use crate::store::records::RecordStore;

use super::forums::{excerpt_of, markdown_body};
use super::models::store_error;
use super::{Engine, EngineError, ErrorCode};

fn kind_str(k: PostKind) -> &'static str {
    match k {
        PostKind::Article => "article",
        PostKind::Link => "link",
        PostKind::Question => "question",
        PostKind::QuickTake => "quickTake",
    }
}

fn lex_quote(q: &TextQuote) -> LexQuote {
    let mut l = LexQuote::new(q.exact.clone());
    l.prefix.clone_from(&q.prefix);
    l.suffix.clone_from(&q.suffix);
    l.start = q.start.map(i64::from);
    l
}

/// The record value of a karma (or tag vote) value.
pub(crate) fn karma_str(v: KarmaValue) -> &'static str {
    match v {
        KarmaValue::Up => "up",
        KarmaValue::Down => "down",
        KarmaValue::StrongUp => "strongUp",
        KarmaValue::StrongDown => "strongDown",
    }
}

/// The record value of an agreement value.
pub(crate) fn agreement_str(v: AgreementValue) -> &'static str {
    match v {
        AgreementValue::Agree => "agree",
        AgreementValue::Disagree => "disagree",
        AgreementValue::StrongAgree => "strongAgree",
        AgreementValue::StrongDisagree => "strongDisagree",
    }
}

/// P (`lfm-1.md` §10) of a record's inline body; `None` for a spilled or absent body.
pub(crate) fn body_text(value: &Value) -> Option<String> {
    let src = value.get("body")?.get("text")?.as_str()?;
    Some(lessdugin_markdown::plain_projection(src).text)
}

/// A quote with its derived `prefix`, `suffix` and `start` (W8): a quote with only `exact` is
/// located in P of the item's current version (`lfm-1.md` §11, the CLI's `--quote`); one with a
/// selector already (built by a client from a selection) is kept. A spilled body keeps `exact`
/// alone.
fn derived_quote(q: &TextQuote, value: &Value) -> Result<LexQuote, EngineError> {
    if q.prefix.is_some() || q.suffix.is_some() || q.start.is_some() {
        return Ok(lex_quote(q));
    }
    let Some(p) = body_text(value) else {
        return Ok(lex_quote(q));
    };
    let (start, end) = lessdugin_markdown::quote::find_first(&q.exact, &p).ok_or_else(|| {
        EngineError::invalid_input(format!("the quoted text {:?} is not in the item", q.exact))
    })?;
    let (start, end) = (
        usize::try_from(start).unwrap_or(usize::MAX),
        usize::try_from(end).unwrap_or(usize::MAX),
    );
    let b = lessdugin_markdown::build_quote(&p, start, end)
        .map_err(|e| EngineError::invalid_input(format!("quote: {e}")))?;
    let mut l = LexQuote::new(b.exact);
    l.prefix = b.prefix;
    l.suffix = b.suffix;
    l.start = b.start.and_then(|s| i64::try_from(s).ok());
    Ok(l)
}

/// The notice of an opinion that is recorded but does not count yet.
pub const NOT_ESTABLISHED: &str = "recorded; will count once you're established";

/// `true` if `intent` only clears or removes (always allowed: it can only stop something
/// from counting).
fn clears(intent: &OpinionIntent) -> bool {
    match intent {
        OpinionIntent::Karma { value } => value.is_none(),
        OpinionIntent::Agreement { value } => value.is_none(),
        OpinionIntent::React { remove, .. } => *remove,
        OpinionIntent::Tag { value, .. } => value.is_none(),
    }
}

/// The checks before an opinion is enqueued (`social.md` §5, §8, §9.2), over the viewer's
/// fold of the forum: an opinion that could never count is refused, and one that will count
/// only once the voter is established gets a [`NoticeCode::NotEstablished`] notice. These are
/// UX checks (readers enforce the rules in the fold); clearing and removing are always allowed.
/// `qkey` is the react quote's key (`""` for the whole item).
pub(crate) fn opinion_checks(
    v: &ForumView,
    me: &str,
    subject: &str,
    intent: &OpinionIntent,
    qkey: &str,
) -> Result<Vec<(NoticeCode, String)>, EngineError> {
    use lessdugin_lexicon::values::{Capability, Voting};
    if clears(intent) {
        return Ok(Vec::new());
    }
    let (author, coll, _) = split_uri(subject)
        .ok_or_else(|| EngineError::invalid_ref(format!("{subject:?} is not a record uri")))?;
    if author == me {
        return Err(EngineError::invalid_input(
            "opinions on your own items are ignored (O3)",
        ));
    }
    if v.participation.b.contains(me) {
        return Err(EngineError::blocked(
            "you are banned in this forum, so your opinions do not count",
        ));
    }
    if v.participation.n.contains(me) {
        return Err(EngineError::blocked(
            "your votes in this forum are nullified, so your opinions do not count",
        ));
    }
    if v.moderation.spam(subject) {
        return Err(EngineError::blocked(
            "this item is marked as spam, so opinions on it do not count",
        ));
    }
    let karma = v.karma.as_ref().ok();
    let established = karma.is_none_or(|k| k.e.contains(me));
    let restricted = |c: Capability, what: &str| {
        if v.moderation.restricted(me, c) {
            Err(EngineError::blocked(format!(
                "you are restricted from {what} in this forum"
            )))
        } else {
            Ok(())
        }
    };
    match intent {
        OpinionIntent::Karma { .. } => restricted(Capability::Vote, "voting")?,
        OpinionIntent::Agreement { .. } => {
            restricted(Capability::Vote, "voting")?;
            if v.policy.voting == Voting::KarmaOnly {
                return Err(EngineError::invalid_input(
                    "this forum does not use agreement voting",
                ));
            }
        }
        OpinionIntent::React { id, stance, .. } => {
            if v.policy.voting != Voting::Reacts {
                return Err(EngineError::invalid_input("this forum does not use reacts"));
            }
            if !palette(v.policy.react_palette).contains(&id.as_str()) {
                return Err(EngineError::invalid_input(format!(
                    "{id:?} is not a react of this forum's palette"
                )));
            }
            restricted(Capability::React, "reacting")?;
            let existing = v
                .reacts
                .as_ref()
                .ok()
                .and_then(|r| r.get(subject))
                .and_then(|ts| ts.iter().find(|t| t.react == *id && t.qkey == qkey));
            let gate = match (stance, existing) {
                (Stance::For, Some(t)) if t.supporters.contains(me) => None,
                (Stance::For, Some(_)) => Some((Gate::ReactSecond, "seconding a react")),
                (Stance::For, None) => Some((Gate::ReactCreate, "starting a new react")),
                (Stance::Against, Some(t)) if t.opposers.contains(me) => None,
                (Stance::Against, Some(_)) => Some((Gate::ReactAnti, "reacting against")),
                (Stance::Against, None) => {
                    return Err(EngineError::invalid_input(format!(
                        "nobody has reacted {id:?} here yet, so there is nothing to react against"
                    )));
                }
            };
            if let (Some((g, what)), Some(k)) = (gate, karma)
                && !passes(&v.policy, &v.authorities, k, me, g)
            {
                let need = g.value(&v.policy);
                let have = k.k_of(me);
                return Err(EngineError::blocked(format!(
                    "{what} needs {need} karma in this forum (you have {have})"
                ))
                .with_detail("gate", g.name())
                .with_detail("need", need)
                .with_detail("have", have));
            }
        }
        OpinionIntent::Tag { .. } => {
            if coll != lessdugin_lexicon::collection::nsid::POST {
                return Err(EngineError::invalid_input("tag votes apply to posts only"));
            }
            restricted(Capability::Tag, "tag voting")?;
        }
    }
    Ok(if established {
        Vec::new()
    } else {
        vec![(NoticeCode::NotEstablished, NOT_ESTABLISHED.to_owned())]
    })
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, EngineError> {
    serde_json::to_value(v).map_err(|e| EngineError::internal(format!("encode: {e}")))
}

/// Sets `k` to `v`, or removes it for `None`.
fn set(m: &mut Map<String, Value>, k: &str, v: Option<Value>) {
    match v {
        Some(v) => {
            m.insert(k.to_owned(), v);
        }
        None => {
            m.remove(k);
        }
    }
}

impl<H: Host> Engine<H> {
    /// The outbox session of the active account (`auth_required` without one).
    pub(crate) async fn active_session(&self) -> Result<Session, EngineError> {
        let a = self.active_account().await?.ok_or_else(|| {
            EngineError::new(ErrorCode::AuthRequired, "no active account; log in first")
        })?;
        Session::parse(&a.did, &a.pds)
    }

    /// The outbox session of account `did` (`auth_required` if it is not logged in).
    pub(crate) async fn session_for(&self, did: &str) -> Result<Session, EngineError> {
        let a = self.account(did).await?.ok_or_else(|| {
            EngineError::new(
                ErrorCode::AuthRequired,
                format!("{did} is not logged in on this device"),
            )
        })?;
        Session::parse(&a.did, &a.pds)
    }

    async fn sessions(&self) -> Result<Vec<Session>, EngineError> {
        self.accounts()
            .await?
            .iter()
            .map(|a| Session::parse(&a.did, &a.pds))
            .collect()
    }

    fn post_record(&self, d: &PostDraft, forum: AtUri) -> Result<Post, EngineError> {
        let mut p = Post::article(forum, "", "", records::datetime_ms(self.host.now_ms())?);
        p.kind = (d.kind != PostKind::Article).then(|| kind_str(d.kind).to_owned());
        p.title.clone_from(&d.title);
        p.body = Some(markdown_body(&d.body)?);
        p.link = d.link.as_ref().map(|l| ExternalLink {
            uri: l.uri.clone(),
            title: l.title.clone(),
            description: l.description.clone(),
            thumb: None,
            extra: Map::new(),
        });
        p.tags = (!d.tags.is_empty()).then(|| d.tags.clone());
        p.labels = (!d.labels.is_empty()).then(|| Labels::Known(SelfLabels::new(d.labels.clone())));
        Ok(p)
    }

    /// `publish(PostDraft)`: a post in `draft.forum` by the active account (with the
    /// membership on first content, `data-model.md` §8.4).
    pub async fn publish(&self, draft: &PostDraft) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let key = self.forum_key(&draft.forum).await?;
        let draft = &PostDraft {
            body: self.with_dids(&draft.body).await?,
            ..draft.clone()
        };
        let post = self.post_record(draft, key.uri.clone())?;
        let opts = ContentOpts {
            blobs: self.media_blobs(&draft.body.media).await?,
            excerpt: Some(excerpt_of(&draft.body.markdown)),
        };
        let t = self.outbox.create_post(&session, &post, opts).await?;
        self.sent_prepared_media(&draft.body.media).await;
        Ok(t)
    }

    /// The current `(cid, value)` of a cached record.
    pub(crate) async fn current(&self, uri: &AtUri) -> Result<(String, Value), EngineError> {
        let row = RecordStore::new(self.host.cache())
            .get(uri)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                EngineError::not_found(format!("{uri} is not in the cache; sync it first"))
            })?;
        let v = row
            .value_json()
            .map_err(|e| EngineError::internal(format!("{uri}: {e}")))?;
        Ok((row.cid.to_string(), v))
    }

    /// `comment(CommentDraft)`: a comment in the thread of `root` (replying to `parent` if
    /// given) by the active account. The root and parent must be cached, and the parent must be
    /// a comment of the same thread (`invalid_ref` otherwise); the comment refers to their
    /// cached versions.
    pub async fn comment(&self, draft: &CommentDraft) -> Result<WriteTicket, EngineError> {
        let session = self.active_session().await?;
        let root = records::at_uri(&draft.root)?;
        if root.collection().as_str() != nsid::POST {
            return Err(EngineError::invalid_ref(format!("{root} is not a post")));
        }
        let (root_cid, root_value) = self.current(&root).await?;
        let forum = root_value
            .get("forum")
            .and_then(Value::as_str)
            .ok_or_else(|| EngineError::invalid_input(format!("{root} names no forum")))
            .and_then(records::at_uri)?;
        self.refuse_locked(&forum, &root, session.did.as_str())
            .await?;
        let mut quoted = root_value.clone();
        let parent = match &draft.parent {
            Some(p) => {
                let p = records::at_uri(p)?;
                if p.collection().as_str() != nsid::COMMENT {
                    return Err(EngineError::invalid_ref(format!("{p} is not a comment")));
                }
                let (cid, value) = self.current(&p).await?;
                // a parent in another thread would put the comment under a Missing placeholder
                let parent_root = value
                    .get("root")
                    .and_then(|r| r.get("uri"))
                    .and_then(Value::as_str);
                if parent_root != Some(root.as_str()) {
                    return Err(EngineError::invalid_ref(format!(
                        "{p} is not a comment in the thread of {root}"
                    )));
                }
                quoted = value;
                Some(StrongRef::new(p, cid))
            }
            None => None,
        };
        let mut c = Comment::new(
            forum,
            StrongRef::new(root, root_cid),
            parent,
            "",
            records::datetime_ms(self.host.now_ms())?,
        );
        c.body = markdown_body(&self.with_dids(&draft.body).await?)?;
        // the quote's target is the parent if present, else the root (`social.md` §16)
        c.quote = draft
            .quote
            .as_ref()
            .map(|q| derived_quote(q, &quoted))
            .transpose()?;
        // the comment's media are uploaded with it (data-model.md §8.8)
        let opts = ContentOpts {
            blobs: self.media_blobs(&draft.body.media).await?,
            excerpt: None,
        };
        let t = self.outbox.create_comment(&session, &c, opts).await?;
        self.sent_prepared_media(&draft.body.media).await;
        Ok(t)
    }

    /// `social.md` §4.8 "Composer": a comment on a locked post is refused (`blocked`,
    /// `detail.kind = "locked"`) unless the writer is in A, or, for an author lock, is the post's
    /// author. It would be published and at once be `Hidden(Locked)` for everyone (V7): a new
    /// comment is never on the allow-list and has no keep. Without a cached model of the forum
    /// there is nothing to go by, and the comment is written.
    async fn refuse_locked(
        &self,
        forum: &AtUri,
        root: &AtUri,
        writer: &str,
    ) -> Result<(), EngineError> {
        let Ok(key) = self.forum_key(forum.as_str()).await else {
            return Ok(());
        };
        let Ok(l) = self.loaded(&key).await else {
            return Ok(());
        };
        let v = &l.model.view;
        if v.authorities.contains_key(writer) {
            return Ok(());
        }
        let forum_lock = v.moderation.lock(root.as_str()).is_some();
        let author_lock = root.did().as_str() != writer
            && v.author
                .as_ref()
                .is_ok_and(|a| a.locks.contains_key(root.as_str()));
        if forum_lock || author_lock {
            return Err(EngineError::blocked(format!(
                "{root} is locked: a new comment would be hidden from everyone"
            ))
            .with_detail("kind", "locked"));
        }
        Ok(())
    }

    /// `edit_preflight(uri)` (`social.md` §4.5, `threat-model.md` SOC-3): the notices to show,
    /// and confirm, before `edit` of `uri` is sent. An edit gives the item a new CID, so the
    /// positive assertions in effect on it (a keep, a tier of `frontpage` or `curated`, a pin;
    /// mod-log status `effective`) stop counting until an authority asserts them again on the
    /// new version: one `suspendsAssertions` notice names them. Empty when there are none, or
    /// while the forum's moderation layer is pending. `not_found` when `uri` is not cached.
    pub async fn edit_preflight(&self, uri: &str) -> Result<Vec<ActionNotice>, EngineError> {
        use lessdugin_lexicon::records::{Action, ModAction};
        use lessdugin_lexicon::values::Tier as LexTier;
        let u = records::at_uri(uri)?;
        if RecordStore::new(self.host.cache())
            .get(&u)
            .await
            .map_err(store_error)?
            .is_none()
        {
            return Err(EngineError::not_found(format!("{u} is not in the cache")));
        }
        let key = self.forum_of(&u).await?;
        let l = self.loaded(&key).await?;
        let m = &l.model;
        let Ok(log) = m.view.mod_log.as_ref() else {
            return Ok(Vec::new());
        };
        let uri = u.as_str();
        // (keep, tier, pin): the positive dimensions of social.md §4.2 in effect on the item
        let (mut keep, mut tier, mut pin) = (false, None::<LexTier>, false);
        for x in m.input.records.values() {
            if x.coll != Some(Collection::ModAction) {
                continue;
            }
            let Some(v) = x.value() else { continue };
            if v.get("subject").and_then(Value::as_str) != Some(uri) {
                continue;
            }
            let effective = log
                .iter()
                .any(|e| e.uri == x.uri && e.status == ModStatus::Effective);
            let Some(a) = effective
                .then(|| serde_json::from_value::<ModAction>(v.clone()).ok())
                .flatten()
            else {
                continue;
            };
            match &a.action {
                Action::Hide(h) if !h.hidden() => keep = true,
                Action::Tier(t) => {
                    if let Some(t @ (LexTier::Frontpage | LexTier::Curated)) = t.tier() {
                        tier = tier.max(Some(t));
                    }
                }
                Action::Pin(p) if p.pinned() => pin = true,
                _ => {}
            }
        }
        let tier = tier.map(|t| format!("its {} tier", t.as_str()));
        let named: Vec<&str> = [keep.then_some("a moderator's keep"), tier.as_deref()]
            .into_iter()
            .flatten()
            .chain(pin.then_some("its pin"))
            .collect();
        let Some((last, rest)) = named.split_last() else {
            return Ok(Vec::new());
        };
        let list = if rest.is_empty() {
            (*last).to_owned()
        } else {
            format!("{} and {last}", rest.join(", "))
        };
        Ok(vec![ActionNotice {
            code: NoticeCode::SuspendsAssertions,
            message: format!(
                "editing suspends {list}: they apply to the version a moderator saw, and stop \
                 until one asserts them again on the new version"
            ),
        }])
    }

    /// `edit(uri, Draft, EditOpts)`: the edit protocol (`data-model.md` §8.3) with a revision
    /// snapshot (`history: false`: `putRecord` with swap). The new value is the cached value
    /// with the draft's fields replaced (unknown fields kept); `base` is the cached CID.
    pub async fn edit(
        &self,
        uri: &str,
        draft: &Draft,
        opts: &EditOpts,
    ) -> Result<WriteTicket, EngineError> {
        let u = records::at_uri(uri)?;
        let session = self.session_for(u.did().as_str()).await?;
        let (base, mut value) = self.current(&u).await?;
        let draft = &match draft {
            Draft::Post { id, post } => Draft::Post {
                id: id.clone(),
                post: PostDraft {
                    body: self.with_dids(&post.body).await?,
                    ..post.clone()
                },
            },
            Draft::Comment { id, comment } => Draft::Comment {
                id: id.clone(),
                comment: CommentDraft {
                    body: self.with_dids(&comment.body).await?,
                    ..comment.clone()
                },
            },
        };
        let cached = value.clone();
        let coll = u.collection().as_str();
        let obj = value
            .as_object_mut()
            .ok_or_else(|| EngineError::internal(format!("{u} is not an object")))?;
        let mut excerpt = None;
        let media = match draft {
            Draft::Post { post, .. } => &post.body.media,
            Draft::Comment { comment, .. } => &comment.body.media,
        };
        match draft {
            Draft::Post { post, .. } => {
                if coll != nsid::POST {
                    return Err(EngineError::invalid_input(format!("{u} is not a post")));
                }
                let forum = obj.get("forum").and_then(Value::as_str).unwrap_or_default();
                let key = self.forum_key(&post.forum).await?;
                if key.uri.as_str() != forum {
                    return Err(EngineError::invalid_input(
                        "a post's forum is fixed after creation",
                    ));
                }
                let p = self.post_record(post, key.uri.clone())?;
                let mut pv = to_value(&p)?;
                // a link to the uri the cached version links keeps its cached card (W3): a
                // draft that names neither title nor description keeps the object whole, one
                // that names some keeps the thumb, which no draft carries (robustness-7)
                // unless the edit drops the card (EditOpts.dropLinkCard): then the draft's link
                // is written as it is
                let kept = match (&post.link, obj.get("link")) {
                    _ if opts.drop_link_card => None,
                    (Some(l), Some(old))
                        if old.get("uri").and_then(Value::as_str) == Some(l.uri.as_str()) =>
                    {
                        if l.title.is_none() && l.description.is_none() {
                            Some(old.clone())
                        } else {
                            let mut new = pv.get("link").cloned().unwrap_or(Value::Null);
                            if let (Some(t), Some(n)) = (old.get("thumb"), new.as_object_mut()) {
                                n.insert("thumb".into(), t.clone());
                            }
                            Some(new)
                        }
                    }
                    _ => None,
                };
                if let (Some(kept), Some(m)) = (kept, pv.as_object_mut()) {
                    m.insert("link".into(), kept);
                }
                for k in ["kind", "title", "body", "link", "tags", "labels"] {
                    set(obj, k, pv.get(k).cloned());
                }
                // a spilled excerpt is rewritten by the outbox if the new body spills
                obj.remove("excerpt");
                excerpt = Some(excerpt_of(&post.body.markdown));
            }
            Draft::Comment { comment, .. } => {
                if coll != nsid::COMMENT {
                    return Err(EngineError::invalid_input(format!("{u} is not a comment")));
                }
                set(obj, "body", Some(to_value(&markdown_body(&comment.body)?)?));
                set(
                    obj,
                    "quote",
                    comment
                        .quote
                        .as_ref()
                        .map(lex_quote)
                        .map(|q| to_value(&q))
                        .transpose()?,
                );
            }
        }
        // media new in this version are uploaded with the edit; the others keep the blob
        // objects of the cached version (data-model.md §8.8)
        let blobs = self.edit_media_blobs(&cached, &mut value, media).await?;
        let o = outbox::EditOpts {
            history: opts.history,
            summary: opts.summary.clone(),
            blobs,
            excerpt,
        };
        let t = self.outbox.edit(&session, uri, &value, &base, o).await?;
        self.sent_prepared_media(media).await;
        Ok(t)
    }

    /// `retract(uri)`: an edit of an own comment with `retracted: true` (`data-model.md` §8.3).
    pub async fn retract(&self, uri: &str) -> Result<WriteTicket, EngineError> {
        let u = records::at_uri(uri)?;
        if u.collection().as_str() != nsid::COMMENT {
            return Err(EngineError::invalid_input(format!(
                "{u} is not a comment; only comments are retracted"
            )));
        }
        let session = self.session_for(u.did().as_str()).await?;
        let (base, mut value) = self.current(&u).await?;
        if let Some(m) = value.as_object_mut() {
            m.insert("retracted".into(), Value::Bool(true));
        }
        self.outbox
            .edit(&session, uri, &value, &base, outbox::EditOpts::default())
            .await
    }

    /// `delete(uri)`: the delete protocol for content (with its revisions), the removal
    /// protocol for a modAction, `deleteRecord` otherwise.
    pub async fn delete(&self, uri: &str) -> Result<WriteTicket, EngineError> {
        let u = records::at_uri(uri)?;
        let session = self.session_for(u.did().as_str()).await?;
        self.outbox.delete(&session, uri).await
    }

    /// `opinion(subject, OpinionIntent)` (`engine-api.md` §3.5, `data-model.md` §8.5): the
    /// active account's opinion on a cached post, comment, wiki revision or sequence. The intent
    /// is held for the 4 s undo window ([`Engine::undo`] removes it) and joins a write on the same
    /// subject that is still held (the ticket is that write's). When sent, the record is
    /// re-read and the intents re-applied per axis, so opinions written from two devices merge;
    /// an opinion left empty is deleted. The subject's version (`subject.cid`) is the cached one.
    /// Refused before enqueueing when it could never count (`blocked`: banned, nullified,
    /// restricted, a karma gate; `invalid_input`: an own item, a forum without agreement or
    /// reacts, an unknown react, nothing to react against); a vote by a voter who is not
    /// established yet is recorded with a [`NoticeCode::NotEstablished`] notice.
    pub async fn opinion(
        &self,
        subject: &str,
        intent: &OpinionIntent,
    ) -> Result<ActionTicket, EngineError> {
        let session = self.active_session().await?;
        let u = records::at_uri(subject)?;
        let coll = u.collection().as_str();
        if ![
            nsid::POST,
            nsid::COMMENT,
            nsid::WIKI_REVISION,
            nsid::SEQUENCE,
        ]
        .contains(&coll)
        {
            return Err(EngineError::invalid_ref(format!(
                "{u} is not a post, comment, wiki revision or sequence"
            )));
        }
        let (cid, value) = self.current(&u).await?;
        let forum = value
            .get("forum")
            .and_then(Value::as_str)
            .ok_or_else(|| EngineError::invalid_input(format!("{u} names no forum")))?;
        let key = self.forum_key(forum).await?;
        let mut quote_key = String::new();
        let step = match intent {
            OpinionIntent::Karma { value } => OpinionStep::Karma {
                value: value.map(|v| karma_str(v).to_owned()),
            },
            OpinionIntent::Agreement { value } => OpinionStep::Agreement {
                value: value.map(|v| agreement_str(v).to_owned()),
            },
            OpinionIntent::React {
                id,
                stance,
                quote,
                remove,
            } => {
                let q = match quote {
                    Some(q) if *remove => Some(lex_quote(q)),
                    Some(q) => Some(derived_quote(q, &value)?),
                    None => None,
                };
                quote_key = lessdugin_markdown::qkey(q.as_ref().map(|q| q.exact.as_str()));
                OpinionStep::React {
                    id: id.clone(),
                    stance: match stance {
                        Stance::For => "for",
                        Stance::Against => "against",
                    }
                    .to_owned(),
                    quote: q
                        .map(|q| serde_json::to_string(&q))
                        .transpose()
                        .map_err(|e| EngineError::internal(format!("quote: {e}")))?,
                    remove: *remove,
                }
            }
            OpinionIntent::Tag { slug, value } => OpinionStep::Tag {
                slug: slug.clone(),
                value: value.map(|v| karma_str(v).to_owned()),
            },
        };
        let notices = match self.loaded(&key).await {
            Ok(l) => opinion_checks(
                &l.model.view,
                session.did.as_str(),
                u.as_str(),
                intent,
                &quote_key,
            )?,
            Err(e) => {
                tracing::debug!(forum = %key.uri, error = %e, "opinion checks skipped");
                Vec::new()
            }
        };
        let t = self
            .outbox
            .opinion(&session, key.uri.as_str(), u.as_str(), &cid, step)
            .await?;
        Ok(notices
            .into_iter()
            .fold(ActionTicket::new(t), |t, (c, m)| t.with_notice(c, m)))
    }

    /// `undo(write_id)`: removes a write not yet sent, or inverts a sent opinion
    /// (`sync.md` §7.4).
    pub async fn undo(&self, id: &str) -> Result<WriteTicket, EngineError> {
        let (account, _) = parse_write_id(id)
            .ok_or_else(|| EngineError::invalid_input(format!("bad write id {id:?}")))?;
        let session = self.session_for(&account).await?;
        self.outbox.undo(&session, id).await
    }

    /// Queues a failed or conflicting write again.
    pub async fn retry_write(&self, id: &str) -> Result<WriteTicket, EngineError> {
        let (account, _) = parse_write_id(id)
            .ok_or_else(|| EngineError::invalid_input(format!("bad write id {id:?}")))?;
        let session = self.session_for(&account).await?;
        self.outbox.retry(&session, id).await
    }

    /// The state of write `id` (`None`: unknown or pruned).
    pub async fn write_status(&self, id: &str) -> Result<Option<WriteTicket>, EngineError> {
        self.outbox.ticket(id).await
    }

    /// Every write in the outbox of every account (by account, then `seq`).
    pub async fn writes(&self) -> Result<Vec<WriteTicket>, EngineError> {
        let mut out = Vec::new();
        for a in self.accounts().await? {
            out.extend(self.outbox.writes(&a.did).await?.iter().map(|w| w.ticket()));
        }
        Ok(out)
    }

    /// `outbox_pump()` (long-lived, `engine-api.md` §3.3): drains the outbox of every
    /// logged-in account, one write in flight per account, until [`Engine::outbox_stop`].
    ///
    /// Each round re-reads the accounts (an account added or removed later is picked up) and
    /// runs at most one write per account, concurrently; when nothing is due it sleeps (with
    /// the host) until the earliest hold or back-off ends, at most [`outbox::IDLE_POLL_MS`],
    /// and is woken early by new writes, retries and account changes. Start it once, before or
    /// after login; the host drives the future (`tokio::spawn` / `spawn_local`). A pump started
    /// after a stop runs normally. Errors are logged and retried, never returned.
    pub async fn outbox_pump(&self) -> Result<(), EngineError> {
        let generation = self.outbox.generation();
        let mut woken = self.outbox.waker();
        let running = || self.outbox.generation() == generation;
        let mut pruned_at: Option<i64> = None;
        while running() {
            // wakes before this round are covered by its scans
            while woken.try_recv().is_ok() {}
            let sessions = match self.accounts().await {
                Ok(accounts) => accounts
                    .iter()
                    .filter_map(|a| Session::parse(&a.did, &a.pds).ok())
                    .collect(),
                Err(e) => {
                    tracing::warn!(error = %e, "outbox pump: reading the accounts");
                    Vec::new()
                }
            };
            let steps = join_all(sessions.iter().map(|s| self.outbox.step(s))).await;
            let now = self.host.now_ms();
            let mut wait = outbox::IDLE_POLL_MS;
            let mut idle = Vec::new();
            for (s, step) in sessions.iter().zip(steps) {
                match step {
                    Ok(Step::Processed(_)) => wait = 0,
                    Ok(Step::Wait(t)) => wait = wait.min(t.saturating_sub(now).max(1)),
                    // the login wakes the pump (`record_account` releases the hold)
                    Ok(Step::NeedsLogin(_)) => {}
                    Ok(Step::Idle) => idle.push(s.did.as_str()),
                    Err(e) => {
                        tracing::warn!(account = %s.did, error = %e, "outbox pump step failed");
                        wait = wait.min(outbox::ERROR_WAIT_MS);
                    }
                }
            }
            if !idle.is_empty() && pruned_at.is_none_or(|t| now - t >= outbox::IDLE_POLL_MS) {
                pruned_at = Some(now);
                let before = now.saturating_sub(outbox::DONE_KEEP_MS);
                for account in idle {
                    if let Err(e) = self.outbox.prune(account, before).await {
                        tracing::warn!(account = %account, error = %e, "outbox prune failed");
                    }
                }
            }
            if wait == 0 || !running() {
                continue;
            }
            let sleep = self.host.sleep(u64::try_from(wait).unwrap_or(1));
            futures::pin_mut!(sleep);
            futures::future::select(sleep, woken.next()).await;
        }
        Ok(())
    }

    /// Stops the running pumps (they return after their current round).
    pub fn outbox_stop(&self) {
        self.outbox.stop();
    }

    /// Runs every account's outbox until no write is pending, sleeping (with the host)
    /// through holds and back-offs (the CLI's one-shot commands; tests). Finished writes older
    /// than [`outbox::DONE_KEEP_MS`] are pruned first, as the long-lived pump does.
    pub async fn outbox_flush(&self) -> Result<PumpReport, EngineError> {
        let mut total = PumpReport::default();
        for s in self.sessions().await? {
            let before = self.host.now_ms().saturating_sub(outbox::DONE_KEEP_MS);
            if let Err(e) = self.outbox.prune(s.did.as_str(), before).await {
                tracing::warn!(account = %s.did, error = %e, "outbox prune failed");
            }
            let r = self.outbox.run_until_idle(&s).await?;
            total.processed += r.processed;
            total.done += r.done;
            total.failed += r.failed;
            total.conflicts += r.conflicts;
            total.throttled += r.throttled;
            total.needs_login += r.needs_login;
        }
        Ok(total)
    }

    /// "Send now": ends the back-off of every account's writes that wait after an ambiguous
    /// or transient failure (not the undo window, the write budget, a rate limit or a refused
    /// session), so the next pump step or [`Engine::outbox_flush`] tries them at once. Returns
    /// how many writes were expedited.
    pub async fn outbox_expedite(&self) -> Result<u32, EngineError> {
        let mut n = 0;
        for s in self.sessions().await? {
            n += self.outbox.expedite(s.did.as_str()).await?;
        }
        Ok(n)
    }
}

impl<H: crate::host::Host> Engine<H> {
    /// `redact_revision(uri)` (`engine-api.md` §3.6, M4): deletes the revision records of an item,
    /// so its history can no longer be read and the PDS garbage-collects any blobs those versions
    /// referenced (`synthesis-v1.md` F29-F34, "redact revisions ... so their blobs are
    /// garbage-collected").
    ///
    /// Only the item's AUTHOR (the account that owns the record) may redact its history: the
    /// revisions are records in their own repo, and removing someone else's history would be a
    /// deletion in a repo the caller does not own (W9). A redaction is itself a delete of each
    /// revision record, so it goes through the outbox like any other write.
    ///
    /// Which revisions go is decided when the write is sent, from a fresh listing of the
    /// author's PDS filtered by `subject.uri` (`Outbox::redact_item`), not from this device's
    /// fold: an invalid revision, one past the fetch caps, or one another device wrote since the
    /// last sync is deleted too. A forum declaration's or a modAction's revisions are the mod log
    /// and are refused (`invalid_input`, `detail.kind = "modLog"`, `threat-model.md` SOC-7).
    ///
    /// The item then reads as "history redacted": it was edited (its own record says so) but no
    /// revision is left, which is what [`Engine::history`] reports. Redacting an item without
    /// history is a write that deletes nothing.
    pub async fn redact_revision(&self, uri: &str) -> Result<Vec<WriteTicket>, EngineError> {
        let u = records::at_uri(uri)?;
        let session = self.session_for(u.did().as_str()).await?;
        Ok(vec![self.outbox.redact_item(&session, u.as_str()).await?])
    }
}
