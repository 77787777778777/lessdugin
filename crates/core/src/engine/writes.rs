// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writes (`engine-api.md` §3.5) through the durable outbox: `publish`, `comment`, `edit`,
//! `delete`, `retract`, `undo`, the write status, and the pump (`outbox_pump`, long-lived;
//! `outbox_flush` runs every account's outbox until idle).
//!
//! New content is written by the active account; edits, deletes and undos by the account that
//! owns the record or write (W9). Each returns a [`WriteTicket`] once the write is persisted;
//! progress arrives as `Write` events, and a confirmed write as `Changed` for its forum and
//! thread. Bodies are LFM/1 markdown with the derived `mentions`/`links` (W8); media are
//! refused until M4 (`unsupported`). TODO(M3): `at://handle` mention rewriting (W7) and the
//! quote's derived `prefix`/`suffix`/`start` (W8).

use futures::StreamExt;
use futures::future::join_all;
use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::{
    Comment, ExternalLink, Labels, Post, SelfLabels, StrongRef, TextQuote as LexQuote,
};
use serde_json::{Map, Value};

use crate::dto::{CommentDraft, Draft, EditOpts, PostDraft, PostKind, TextQuote, WriteTicket};
use crate::host::Host;
use crate::outbox::{self, ContentOpts, PumpReport, Session, Step, parse_write_id, records};
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
        let post = self.post_record(draft, key.uri.clone())?;
        let opts = ContentOpts {
            blobs: Vec::new(),
            excerpt: Some(excerpt_of(&draft.body.markdown)),
        };
        self.outbox.create_post(&session, &post, opts).await
    }

    /// The current `(cid, value)` of a cached record.
    async fn current(&self, uri: &AtUri) -> Result<(String, Value), EngineError> {
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
        c.body = markdown_body(&draft.body)?;
        c.quote = draft.quote.as_ref().map(lex_quote);
        self.outbox
            .create_comment(&session, &c, ContentOpts::default())
            .await
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
        let coll = u.collection().as_str();
        let obj = value
            .as_object_mut()
            .ok_or_else(|| EngineError::internal(format!("{u} is not an object")))?;
        let mut excerpt = None;
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
                let pv = to_value(&p)?;
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
        let o = outbox::EditOpts {
            history: opts.history,
            summary: opts.summary.clone(),
            blobs: Vec::new(),
            excerpt,
        };
        self.outbox.edit(&session, uri, &value, &base, o).await
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
        }
        Ok(total)
    }
}
