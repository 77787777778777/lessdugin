// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing: `post`, `comment`, `edit`, `delete`, `retract` (synthesis §6.10), and how every
//! writing command finishes ([`drive`], [`report`]).
//!
//! A write is first persisted in the durable outbox (`engine-api.md` §3.5), then this process
//! sends the account's queued writes in order (one in flight, `sync.md` §7.1) until the new one
//! is finished or [`WRITE_WAIT_MS`] has passed. A write that is still queued then (offline, a
//! rate limit, a back-off) stays in the outbox and is sent by the next writing command,
//! `outbox flush` or `sync`; nothing is lost if the process is killed, and a write interrupted
//! while it was being sent is checked against the PDS before it is sent again.
//!
//! `--json` prints the `WriteTicket` with `"dto": 1`; a failed write prints its ticket and
//! exits with the error's code (`conflict` 5, …).

use lessdugin_atp::syntax::AtUri;
use lessdugin_core::dto::{
    BodyDraft, CommentDraft, Draft, EditOpts, ExternalLinkDraft, PostDraft, TextQuote, WriteState,
    WriteTicket,
};
use lessdugin_core::engine::{Engine, EngineError, ErrorCode};
use lessdugin_core::host::Host;
use lessdugin_core::outbox::Step;
use lessdugin_core::store::records::RecordStore;
use lessdugin_lexicon::collection::nsid;
use lessdugin_native::host::NativeHost;
use serde_json::Value;

use crate::args::{CommentArgs, DeleteArgs, EditArgs, PostArgs, RefArgs};
use crate::cmd::draft::{self, Doc, Drafts, PostSource, kind_str, parse_kind};
use crate::cmd::{read, refs};
use crate::ctx::{CliError, Ctx, Writer};
use crate::out::{Style, cat, dto, lit, tt};
use crate::prompt;
use crate::render;

/// How long a writing command keeps sending before it leaves the write queued.
pub const WRITE_WAIT_MS: i64 = 60_000;

fn finished(s: &WriteState) -> bool {
    matches!(
        s,
        WriteState::Done { .. } | WriteState::Failed { .. } | WriteState::Undone
    )
}

/// Sends the account's queued writes, in order, until write `ticket` is finished, nothing is
/// due, or [`WRITE_WAIT_MS`] has passed; returns the write's latest ticket. With `--offline`
/// nothing is sent.
pub async fn drive(ctx: &Ctx, w: &Writer, ticket: WriteTicket) -> Result<WriteTicket, CliError> {
    if ctx.offline() || finished(&ticket.state) {
        return Ok(ticket);
    }
    let host = w.engine.host();
    let deadline = host.now_ms().saturating_add(WRITE_WAIT_MS);
    loop {
        match w.engine.outbox().step(&w.session).await? {
            Step::Processed(t) if t.id == ticket.id && finished(&t.state) => return Ok(t),
            Step::Processed(_) => {}
            Step::Idle => break,
            Step::Wait(at) => {
                let now = host.now_ms();
                if at >= deadline || now >= deadline {
                    break;
                }
                host.sleep(u64::try_from(at.saturating_sub(now).max(1)).unwrap_or(1))
                    .await;
            }
        }
    }
    Ok(w.engine.write_status(&ticket.id).await?.unwrap_or(ticket))
}

/// Sends every queued write of the account (the same limits as [`drive`]); returns the final
/// ticket of each write that was run, in the order they were first run.
pub async fn drive_all(ctx: &Ctx, w: &Writer) -> Result<Vec<WriteTicket>, CliError> {
    let mut run: Vec<WriteTicket> = Vec::new();
    if ctx.offline() {
        return Ok(run);
    }
    let host = w.engine.host();
    let deadline = host.now_ms().saturating_add(WRITE_WAIT_MS);
    loop {
        match w.engine.outbox().step(&w.session).await? {
            Step::Processed(t) => match run.iter_mut().find(|x| x.id == t.id) {
                Some(x) => *x = t,
                None => run.push(t),
            },
            Step::Idle => return Ok(run),
            Step::Wait(at) => {
                let now = host.now_ms();
                if at >= deadline || now >= deadline {
                    return Ok(run);
                }
                host.sleep(u64::try_from(at.saturating_sub(now).max(1)).unwrap_or(1))
                    .await;
            }
        }
    }
}

/// The error of the first write in `run` that failed.
pub fn first_failure(run: &[WriteTicket]) -> Option<EngineError> {
    run.iter().find_map(|t| match &t.state {
        WriteState::Failed { error } => Some(error.clone()),
        _ => None,
    })
}

/// Prints a write's outcome; a failed write is returned as its error (the exit code).
pub fn report(ctx: &Ctx, t: &WriteTicket, done: &'static str) -> Result<(), CliError> {
    if ctx.out.is_json() {
        ctx.out.json(&dto(t));
    }
    match &t.state {
        WriteState::Failed { error } => return Err(CliError::Engine(error.clone())),
        _ if ctx.out.is_json() => {}
        WriteState::Done { uri, .. } => ctx.out.line(&[
            (lit(done), Style::Good),
            (lit(": "), Style::Plain),
            (tt(uri), Style::Bold),
        ]),
        WriteState::Undone => ctx.out.line(&[(lit("Undone."), Style::Plain)]),
        WriteState::Sending | WriteState::Pending { .. } => {
            let why = match &t.state {
                _ if ctx.offline() => lit(" (offline)"),
                WriteState::Pending {
                    not_before_ms: Some(at),
                    ..
                } => cat([lit(" (waiting until "), tt(&render::time(*at)), lit(")")]),
                _ => lit(""),
            };
            ctx.out.line(&[
                (lit("Queued"), Style::Warn),
                (why, Style::Plain),
                (lit(": write "), Style::Plain),
                (tt(&t.id), Style::Bold),
            ]);
            ctx.out.note(&[(
                lit("It is sent by the next command that writes, `lessdugin sync` or `lessdugin outbox flush`."),
                Style::Dim,
            )]);
        }
    }
    Ok(())
}

/// The cached `(cid, value)` of a record (`not_found` if it is not cached).
pub async fn cached(engine: &Engine<NativeHost>, uri: &str) -> Result<(String, Value), CliError> {
    let u = AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
    let row = RecordStore::new(engine.host().cache())
        .get(&u)
        .await
        .map_err(|e| EngineError::internal(e.to_string()))?
        .ok_or_else(|| EngineError::not_found(format!("{uri} is not available")))?;
    let v = row
        .value_json()
        .map_err(|e| EngineError::internal(format!("{uri}: {e}")))?;
    Ok((row.cid.to_string(), v))
}

fn owner_of(uri: &str) -> Result<String, CliError> {
    let u = AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
    Ok(u.did().to_string())
}

/// The text of a body from `-m`, a file or standard input; `None` when none is given.
fn text_arg(
    message: Option<String>,
    file: Option<&std::path::Path>,
) -> Result<Option<String>, CliError> {
    match (message, file) {
        (Some(m), _) => Ok(Some(m)),
        (None, Some(f)) => Ok(Some(draft::read_file(f)?)),
        (None, None) => Ok(None),
    }
}

// ---------------------------------------------------------------------------------------------
// post

/// `lessdugin post [file.md]`.
pub async fn post(ctx: &Ctx, args: PostArgs) -> Result<(), CliError> {
    let drafts = Drafts::new(ctx);
    let (mut doc, from_editor) = match &args.file {
        Some(f) => {
            let text = draft::read_file(f)?;
            let doc = Doc::parse(&text)
                .map_err(|e| EngineError::invalid_input(format!("{}: {e}", f.display())))?;
            (doc, false)
        }
        None => (
            Doc {
                fields: vec![("type".into(), draft::Value::Str("post".into()))],
                body: String::new(),
            },
            true,
        ),
    };
    // flags override the frontmatter
    if let Some(f) = &ctx.g.forum {
        doc.set("forum", draft::Value::Str(f.clone()));
    }
    if let Some(t) = &args.title {
        doc.set("title", draft::Value::Str(t.clone()));
    }
    if let Some(k) = args.kind {
        doc.set("kind", draft::Value::Str(kind_str(k.into()).into()));
    }
    if let Some(l) = &args.link {
        doc.set("link", draft::Value::Str(l.clone()));
        if doc.get("kind").is_none() {
            doc.set("kind", draft::Value::Str("link".into()));
        }
    }
    if !args.tags.is_empty() {
        doc.set("tags", draft::Value::List(args.tags.clone()));
    }
    let mut draft_id = None;
    if from_editor {
        // a new draft: name the forum and the title fields so the editor shows them
        if doc.get("forum").is_none()
            && let Some(f) = ctx.state.current_forum()
        {
            doc.set("forum", draft::Value::Str(f));
        }
        for k in ["title", "kind", "tags"] {
            if !doc.fields.iter().any(|(key, _)| key == k) {
                let v = match k {
                    "kind" => draft::Value::Str("article".into()),
                    "tags" => draft::Value::List(Vec::new()),
                    _ => draft::Value::Str(String::new()),
                };
                doc.set(k, v);
            }
        }
        let id = drafts.new_id(ctx.engine()?.host());
        let path = drafts.save(&id, &doc, &[])?;
        if !args.draft {
            draft::edit_file(&path)?;
            doc = drafts.load(&id)?;
        }
        draft_id = Some(id);
    }
    if args.draft {
        let id = match draft_id {
            Some(id) => id,
            None => {
                let id = drafts.new_id(ctx.engine()?.host());
                drafts.save(&id, &doc, &[])?;
                id
            }
        };
        if ctx.out.is_json() {
            ctx.out.json(&serde_json::json!({
                "dto": 1,
                "id": id,
                "path": drafts.path(&id).display().to_string(),
            }));
        } else {
            ctx.out.line(&[
                (lit("Saved draft "), Style::Plain),
                (tt(&id), Style::Bold),
                (lit(" ("), Style::Plain),
                (tt(&drafts.path(&id).display().to_string()), Style::Dim),
                (lit(")"), Style::Plain),
            ]);
        }
        return Ok(());
    }
    if from_editor && doc.body_is_empty() && doc.get("title").is_none() {
        if let Some(id) = &draft_id {
            drafts.remove(id)?;
        }
        return Err(CliError::usage("the draft is empty; nothing was published"));
    }
    draft::publish_post(
        ctx,
        PostSource {
            doc,
            draft: draft_id,
        },
        args.yes,
    )
    .await
}

// ---------------------------------------------------------------------------------------------
// comment

/// The root and parent of a comment on `uri` (a post, or a comment replied to).
async fn thread_of(
    engine: &Engine<NativeHost>,
    uri: &str,
    is_comment: bool,
) -> Result<(String, Option<String>), CliError> {
    if !is_comment {
        return Ok((uri.to_owned(), None));
    }
    let (_, v) = cached(engine, uri).await?;
    let root = v
        .get("root")
        .and_then(|r| r.get("uri"))
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::invalid_input(format!("{uri} names no thread")))?;
    Ok((root.to_owned(), Some(uri.to_owned())))
}

async fn send_comment(ctx: &Ctx, w: &Writer, d: &CommentDraft) -> Result<(), CliError> {
    let ticket = w.engine.comment(d).await?;
    let t = drive(ctx, w, ticket).await?;
    report(ctx, &t, "Commented")
}

/// `lessdugin comment <ref> [file|-] [-m text]`.
pub async fn comment(ctx: &Ctx, args: CommentArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let (uri, is_comment) = refs::post_or_comment(ctx, &w.engine, &args.reference).await?;
    read::sync_thread_quietly(ctx, &w.engine, &uri).await;
    let (root, parent) = thread_of(&w.engine, &uri, is_comment).await?;
    let text = text_arg(args.message, args.file.as_deref())?;
    let text = match text {
        Some(t) => t,
        None => {
            let drafts = Drafts::new(ctx);
            let mut doc = Doc {
                fields: vec![
                    ("type".into(), draft::Value::Str("comment".into())),
                    ("root".into(), draft::Value::Str(root.clone())),
                ],
                body: String::new(),
            };
            doc.set_opt("parent", parent.as_deref());
            let id = drafts.new_id(w.engine.host());
            let path = drafts.save(&id, &doc, &[])?;
            draft::edit_file(&path)?;
            let doc = drafts.load(&id)?;
            if doc.body_is_empty() {
                drafts.remove(&id)?;
                return Err(CliError::usage("the comment is empty; nothing was posted"));
            }
            let d = CommentDraft::new(root, parent, doc.body_text());
            let r = send_comment(ctx, &w, &d).await;
            if r.is_ok() {
                drafts.remove(&id)?;
            }
            return r;
        }
    };
    if text.trim().is_empty() {
        return Err(CliError::usage("the comment is empty; nothing was posted"));
    }
    send_comment(ctx, &w, &CommentDraft::new(root, parent, text)).await
}

/// `draft publish` of a comment draft.
pub async fn publish_comment_draft(ctx: &Ctx, id: &str, doc: Doc) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let root = doc
        .get("root")
        .ok_or_else(|| EngineError::invalid_input(format!("draft {id} has no `root`")))?
        .to_owned();
    if doc.body_is_empty() {
        return Err(EngineError::invalid_input(format!("draft {id} is empty")).into());
    }
    read::sync_thread_quietly(ctx, &w.engine, doc.get("parent").unwrap_or(&root)).await;
    let d = CommentDraft::new(root, doc.get("parent").map(str::to_owned), doc.body_text());
    send_comment(ctx, &w, &d).await?;
    Drafts::new(ctx).remove(id)
}

// ---------------------------------------------------------------------------------------------
// edit

/// The editable fields of a cached record.
struct Current {
    is_post: bool,
    forum: String,
    markdown: String,
    title: Option<String>,
    kind: String,
    link: Option<String>,
    tags: Vec<String>,
    labels: Vec<String>,
    root: Option<String>,
    parent: Option<String>,
    quote: Option<TextQuote>,
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn current(engine: &Engine<NativeHost>, uri: &str) -> Result<Current, CliError> {
    let u = AtUri::parse(uri).map_err(|e| EngineError::invalid_ref(format!("{uri:?}: {e}")))?;
    let coll = u.collection().as_str();
    let is_post = coll == nsid::POST;
    if !is_post && coll != nsid::COMMENT {
        return Err(EngineError::invalid_ref(format!("{uri} is not a post or a comment")).into());
    }
    let (_, v) = cached(engine, uri).await?;
    let body = v.get("body");
    let markdown = match body.and_then(|b| b.get("text")).and_then(Value::as_str) {
        Some(t) => t.to_owned(),
        None if body.is_none() => String::new(),
        None => {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                "this body is stored as a blob; editing it arrives with media support (M4)",
            )
            .into());
        }
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Ok(Current {
        is_post,
        forum: s("forum").unwrap_or_default(),
        markdown,
        title: s("title"),
        kind: s("kind").unwrap_or_else(|| "article".into()),
        link: v
            .get("link")
            .and_then(|l| l.get("uri"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        tags: strs(v.get("tags")),
        labels: v
            .get("labels")
            .and_then(|l| l.get("values"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("val").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        root: v
            .get("root")
            .and_then(|r| r.get("uri"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        parent: v
            .get("parent")
            .and_then(|r| r.get("uri"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        quote: v
            .get("quote")
            .and_then(|q| serde_json::from_value::<TextQuote>(q.clone()).ok()),
    })
}

/// The edit draft of a record: its post fields (posts) and its current markdown.
fn edit_doc(uri: &str, c: &Current) -> Doc {
    let mut doc = Doc {
        fields: vec![
            ("type".into(), draft::Value::Str("edit".into())),
            ("target".into(), draft::Value::Str(uri.to_owned())),
        ],
        body: c.markdown.clone(),
    };
    if c.is_post {
        doc.set(
            "title",
            draft::Value::Str(c.title.clone().unwrap_or_default()),
        );
        doc.set("kind", draft::Value::Str(c.kind.clone()));
        doc.set_opt("link", c.link.as_deref());
        doc.set("tags", draft::Value::List(c.tags.clone()));
    }
    doc.set("summary", draft::Value::Str(String::new()));
    doc
}

/// The engine draft of an edit: `doc`'s fields over the current ones.
fn edit_draft(c: &Current, doc: &Doc, body: Option<String>) -> Result<Draft, CliError> {
    let markdown = body.unwrap_or_else(|| c.markdown.clone());
    if c.is_post {
        let kind = match doc.get("kind") {
            Some(k) => parse_kind(k).map_err(EngineError::invalid_input)?,
            None => parse_kind(&c.kind).unwrap_or(lessdugin_core::dto::PostKind::Article),
        };
        let has = |k: &str| doc.fields.iter().any(|(key, _)| key == k);
        let link = if has("link") {
            doc.get("link").map(str::to_owned)
        } else {
            c.link.clone()
        };
        Ok(Draft::Post {
            id: None,
            post: PostDraft {
                forum: c.forum.clone(),
                kind,
                title: if has("title") {
                    doc.get("title").map(str::to_owned)
                } else {
                    c.title.clone()
                },
                body: BodyDraft::text(markdown),
                link: link.map(|uri| ExternalLinkDraft {
                    uri,
                    title: None,
                    description: None,
                }),
                tags: if has("tags") {
                    doc.list("tags")
                } else {
                    c.tags.clone()
                },
                labels: c.labels.clone(),
            },
        })
    } else {
        Ok(Draft::Comment {
            id: None,
            comment: CommentDraft {
                root: c.root.clone().unwrap_or_default(),
                parent: c.parent.clone(),
                body: BodyDraft::text(markdown),
                quote: c.quote.clone(),
            },
        })
    }
}

async fn send_edit(
    ctx: &Ctx,
    w: &Writer,
    uri: &str,
    d: &Draft,
    opts: &EditOpts,
) -> Result<(), CliError> {
    let ticket = w.engine.edit(uri, d, opts).await?;
    let t = drive(ctx, w, ticket).await?;
    report(ctx, &t, "Edited")
}

/// `lessdugin edit <ref> [file|-] [-m text] [--title t] [--summary s] [--no-history]`.
pub async fn edit(ctx: &Ctx, args: EditArgs) -> Result<(), CliError> {
    let reader = ctx.reader().await?;
    let (uri, _) = refs::post_or_comment(ctx, &reader, &args.reference).await?;
    let w = ctx.writer_for(&owner_of(&uri)?).await?;
    read::sync_thread_quietly(ctx, &w.engine, &uri).await;
    let c = current(&w.engine, &uri).await?;
    let mut opts = EditOpts {
        history: !args.no_history,
        summary: args.summary.clone(),
    };
    let given = text_arg(args.message, args.file.as_deref())?;
    let (doc, body) = match given {
        Some(text) => {
            // a file may carry post fields in frontmatter
            let doc = Doc::parse(&text).map_err(EngineError::invalid_input)?;
            let body = doc.body_text();
            (doc, Some(body))
        }
        None if args.title.is_some() => (Doc::default(), None),
        None => {
            let drafts = Drafts::new(ctx);
            let id = drafts.new_id(w.engine.host());
            let path = drafts.save(&id, &edit_doc(&uri, &c), &[])?;
            draft::edit_file(&path)?;
            let doc = drafts.load(&id)?;
            if opts.summary.is_none() {
                opts.summary = doc.get("summary").map(str::to_owned);
            }
            let body = doc.body_text();
            let mut d = edit_draft(&c, &doc, Some(body))?;
            if let (Some(t), Draft::Post { post, .. }) = (&args.title, &mut d) {
                post.title = Some(t.clone());
            }
            let r = send_edit(ctx, &w, &uri, &d, &opts).await;
            if r.is_ok() {
                drafts.remove(&id)?;
            } else {
                ctx.out.note(&[(
                    cat([lit("The edit is kept as draft "), tt(&id), lit(".")]),
                    Style::Dim,
                )]);
            }
            return r;
        }
    };
    let mut d = edit_draft(&c, &doc, body)?;
    if let Some(t) = &args.title {
        match &mut d {
            Draft::Post { post, .. } => post.title = Some(t.clone()),
            Draft::Comment { .. } => {
                return Err(CliError::usage("--title applies to posts only"));
            }
        }
    }
    send_edit(ctx, &w, &uri, &d, &opts).await
}

/// `draft publish` of an edit draft.
pub async fn publish_edit_draft(ctx: &Ctx, id: &str, doc: Doc) -> Result<(), CliError> {
    let uri = doc
        .get("target")
        .ok_or_else(|| EngineError::invalid_input(format!("draft {id} has no `target`")))?
        .to_owned();
    let w = ctx.writer_for(&owner_of(&uri)?).await?;
    read::sync_thread_quietly(ctx, &w.engine, &uri).await;
    let c = current(&w.engine, &uri).await?;
    let d = edit_draft(&c, &doc, Some(doc.body_text()))?;
    let opts = EditOpts {
        history: doc.get("history") != Some("false"),
        summary: doc.get("summary").map(str::to_owned),
    };
    send_edit(ctx, &w, &uri, &d, &opts).await?;
    Drafts::new(ctx).remove(id)
}

// ---------------------------------------------------------------------------------------------
// delete, retract

/// `lessdugin delete <ref> [--yes]`.
pub async fn delete(ctx: &Ctx, args: DeleteArgs) -> Result<(), CliError> {
    let reader = ctx.reader().await?;
    let (uri, _) = refs::post_or_comment(ctx, &reader, &args.reference).await?;
    let w = ctx.writer_for(&owner_of(&uri)?).await?;
    if !args.yes {
        if !prompt::stdin_is_tty() {
            return Err(CliError::usage(
                "delete removes the record and its revisions; add --yes when not running \
                 interactively",
            ));
        }
        ctx.out
            .err_line(&[(lit("Delete "), Style::Plain), (tt(&uri), Style::Bold)]);
        if !prompt::confirm("and its revisions?") {
            return Err(CliError::Reported(1));
        }
    }
    let ticket = w.engine.delete(&uri).await?;
    let t = drive(ctx, &w, ticket).await?;
    report(ctx, &t, "Deleted")
}

/// `lessdugin retract <ref>`.
pub async fn retract(ctx: &Ctx, args: RefArgs) -> Result<(), CliError> {
    let reader = ctx.reader().await?;
    let (uri, is_comment) = refs::post_or_comment(ctx, &reader, &args.reference).await?;
    if !is_comment {
        return Err(EngineError::invalid_input(
            "only comments are retracted; edit or delete a post instead",
        )
        .into());
    }
    let w = ctx.writer_for(&owner_of(&uri)?).await?;
    read::sync_thread_quietly(ctx, &w.engine, &uri).await;
    let ticket = w.engine.retract(&uri).await?;
    let t = drive(ctx, &w, ticket).await?;
    report(ctx, &t, "Retracted")
}
