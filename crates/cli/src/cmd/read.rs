// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reading: `ls`, `read` and `history` (synthesis §6.10). None needs a login.
//!
//! Unless `--offline`, `ls` syncs the forum and `read`/`history` sync the thread first, within
//! [`IMPLICIT_SYNC_MS`]; a failed or partial sync is reported on stderr and the cache is shown.
//! Authors whose handles are not verified yet are verified before the view is printed, so the
//! view shows `@handle` rather than a short DID.
//!
//! `--json` prints the engine's DTO exactly (`PostList`, `ThreadView`, `PostView`, `RawView`,
//! `HistoryView`). The listing numbers (`3`) and comment labels (`c12`) are saved for this
//! terminal even with `--json`.

use std::collections::BTreeMap;

use lessdugin_core::dto::{
    ActorView, CommentNode, ItemState, ListPostsQuery, PostFilter, PostKind, PostList, PostSummary,
    PostView, SyncOpts, ThreadOpts, ThreadRoot, ThreadView,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_native::host::NativeHost;

use crate::args::{LsArgs, ReadArgs, RefArgs};
use crate::cmd::refs;
use crate::ctx::{CliError, Ctx};
use crate::out::{Span, Style, TermText, cat, lit, name, tt};
use crate::render::{self, BodyStyle};

/// The time budget of the sync `ls`, `read` and `history` run first.
pub const IMPLICIT_SYNC_MS: i64 = 20_000;

/// How many queued handle verifications a view runs before printing.
const VERIFY_HANDLES: usize = 256;

/// Syncs a forum before a view; failures are warnings (the cache is shown).
pub async fn sync_forum_quietly(ctx: &Ctx, engine: &Engine<NativeHost>, forum: &str) {
    if ctx.offline() {
        return;
    }
    let opts = SyncOpts {
        budget_ms: Some(IMPLICIT_SYNC_MS),
        max_requests: None,
        full: false,
    };
    let res = engine.sync_forum(forum, &opts).await;
    if res.as_ref().is_ok_and(|r| !r.partial) {
        let _ = ctx.state.mark_synced(forum);
    }
    match res {
        Ok(r) if r.partial => ctx.out.note(&[(
            cat([
                lit("note: showing what was synced in time ("),
                tt(&r.repos_complete.to_string()),
                lit(" of "),
                tt(&r.repos_total.to_string()),
                lit(" repositories); run `lessdugin sync` for the rest"),
            ]),
            Style::Dim,
        )]),
        Ok(_) => {}
        Err(e) => ctx.out.warn(cat([
            lit("could not sync, showing the cache: "),
            tt(&e.message),
        ])),
    }
}

/// Syncs a thread before a view; failures are warnings.
pub async fn sync_thread_quietly(ctx: &Ctx, engine: &Engine<NativeHost>, uri: &str) {
    if ctx.offline() {
        return;
    }
    let opts = SyncOpts {
        budget_ms: Some(IMPLICIT_SYNC_MS),
        max_requests: None,
        full: false,
    };
    if let Err(e) = engine.sync_thread(uri, &opts).await {
        ctx.out.warn(cat([
            lit("could not sync the thread, showing the cache: "),
            tt(&e.message),
        ]));
    }
}

/// The sync before reading `uri`: the thread (a point fetch of the item and its root, the
/// thread's known participants), then the whole forum unless a command synced it within
/// [`crate::state::FORUM_FRESH_MS`] (new participants, such as a member approved since, are
/// only found by a forum sync).
pub async fn sync_for_read(ctx: &Ctx, engine: &Engine<NativeHost>, uri: &str) {
    if ctx.offline() {
        return;
    }
    sync_thread_quietly(ctx, engine, uri).await;
    let forum = crate::cmd::write::cached(engine, uri)
        .await
        .ok()
        .and_then(|(_, v)| v.get("forum")?.as_str().map(str::to_owned));
    if let Some(f) = forum
        && !ctx.state.recently_synced(&f)
    {
        sync_forum_quietly(ctx, engine, &f).await;
    }
}

/// True if some actor has no verified handle yet (and the network may be used).
fn unverified<'a>(ctx: &Ctx, mut actors: impl Iterator<Item = &'a ActorView>) -> bool {
    !ctx.offline() && actors.any(|a| a.handle.is_none())
}

/// Verifies queued handles; `true` if any was checked (the view should be re-queried).
async fn verify_handles(engine: &Engine<NativeHost>) -> bool {
    engine.verify_handles(VERIFY_HANDLES).await > 0
}

fn body_style(ctx: &Ctx) -> BodyStyle {
    BodyStyle {
        color: ctx.out.color(),
        hyperlinks: ctx.out.color() && ctx.out.stdout_is_tty(),
        width: render::width(ctx.out.stdout_is_tty()),
    }
}

fn filter_of(a: &LsArgs) -> PostFilter {
    if a.frontpage {
        PostFilter::Frontpage
    } else if a.curated {
        PostFilter::Curated
    } else {
        PostFilter::All
    }
}

/// `lessdugin ls [forum]`.
pub async fn ls(ctx: &Ctx, args: LsArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = refs::forum_or_current(ctx, &engine, args.forum.as_deref()).await?;
    sync_forum_quietly(ctx, &engine, &forum).await;
    let q = ListPostsQuery {
        forum,
        sort: args.sort.into(),
        filter: filter_of(&args),
        tag: args.tag.clone(),
        kind: args.kind.map(Into::into),
        author: args.author.clone(),
        cursor: None,
        limit: args.limit,
    };
    let mut list = engine.list_posts(&q).await?;
    if unverified(ctx, list.items.iter().map(|p| &p.author)) && verify_handles(&engine).await {
        list = engine.list_posts(&q).await?;
    }
    let saved = ctx.state.update_listings(|l| {
        l.posts_forum = Some(list.forum.clone());
        l.posts = list.items.iter().map(|p| p.uri.clone()).collect();
    });
    if let Err(e) = saved {
        ctx.out.warn(cat([
            lit("could not save the listing: "),
            tt(&e.to_string()),
        ]));
    }
    if ctx.out.is_json() {
        ctx.out.json(&list);
        return Ok(());
    }
    print_list(ctx, &list);
    Ok(())
}

fn post_title(p: &PostSummary) -> TermText {
    match (&p.title, &p.state) {
        (_, ItemState::Tombstone { .. }) => lit("[deleted]"),
        (_, ItemState::Hidden { .. }) => lit("[hidden]"),
        (Some(t), _) if !t.trim().is_empty() => tt(t),
        _ => {
            let first = p.excerpt.lines().next().unwrap_or_default();
            if first.is_empty() {
                lit("(untitled)")
            } else {
                tt(first)
            }
        }
    }
}

fn post_meta(p: &PostSummary) -> Vec<TermText> {
    let mut parts = vec![
        render::actor(&p.author),
        tt(&render::time(p.created_at_ms)),
        render::count(p.comment_count, "comment", "comments"),
        cat([
            tt(&p.score.score.to_string()),
            lit(if p.score.score.abs() == 1 {
                " point"
            } else {
                " points"
            }),
        ]),
    ];
    match p.post_kind {
        PostKind::Article => {}
        PostKind::Link => parts.push(lit("link")),
        PostKind::Question => parts.push(lit("question")),
        PostKind::QuickTake => parts.push(lit("quick take")),
    }
    if !p.tags.is_empty() {
        parts.push(cat(p.tags.iter().enumerate().map(|(i, t)| {
            cat([if i == 0 { lit("#") } else { lit(" #") }, name(t)])
        })));
    }
    if let Some(s) = render::state_label(&p.state) {
        parts.push(s);
    }
    for b in render::badges(&p.badges) {
        parts.push(lit(b));
    }
    parts
}

fn joined(parts: Vec<TermText>) -> TermText {
    let mut out = TermText::empty();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.append(tt(" · "));
        }
        out.append(p);
    }
    out
}

fn print_list(ctx: &Ctx, list: &PostList) {
    if list.items.is_empty() {
        ctx.out.line(&[(lit("No posts."), Style::Dim)]);
    }
    let w = list.items.len().to_string().len();
    for (i, p) in list.items.iter().enumerate() {
        let n = format!("{:>w$}", i + 1);
        ctx.out.line(&[
            (tt(&n), Style::Bold),
            (lit("  "), Style::Plain),
            (post_title(p), Style::Bold),
        ]);
        ctx.out.line(&[
            (tt(&" ".repeat(w + 2)), Style::Plain),
            (joined(post_meta(p)), Style::Dim),
        ]);
    }
    if list.completeness.partial || list.completeness.degraded {
        ctx.out.note(&[(
            cat([
                lit("("),
                tt(&list.completeness.repos_complete.to_string()),
                lit(" of "),
                tt(&list.completeness.repos_total.to_string()),
                lit(" repositories synced"),
                lit(if list.completeness.degraded {
                    "; moderation data incomplete)"
                } else {
                    ")"
                }),
            ]),
            Style::Dim,
        )]);
    }
}

/// `lessdugin read <ref>`.
pub async fn read(ctx: &Ctx, args: ReadArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let (uri, is_comment) = refs::post_or_comment(ctx, &engine, &args.reference).await?;
    if args.raw {
        sync_thread_quietly(ctx, &engine, &uri).await;
        let raw = engine.raw(&uri).await?;
        if ctx.out.is_json() {
            ctx.out.json(&raw);
        } else {
            let text = serde_json::to_string_pretty(&raw).unwrap_or_default();
            let mut s = String::new();
            for line in text.lines() {
                s.push_str(&ctx.out.spans(&[(tt(line), Style::Plain)]));
                s.push('\n');
            }
            ctx.out.page(&s, !args.no_pager);
        }
        return Ok(());
    }
    sync_for_read(ctx, &engine, &uri).await;
    let opts = ThreadOpts {
        sort: args.sort.into(),
        unread_only: false,
    };
    let mut thread = engine.get_thread(&uri, &opts).await?;
    let actors = |t: &ThreadView| -> Vec<ActorView> {
        let mut v: Vec<ActorView> = t.comments.iter().map(|c| c.author.clone()).collect();
        if let ThreadRoot::Post(p) = &t.post {
            v.push(p.post.author.clone());
        }
        v
    };
    if unverified(ctx, actors(&thread).iter()) && verify_handles(&engine).await {
        thread = engine.get_thread(&uri, &opts).await?;
    }
    let root = match &thread.post {
        ThreadRoot::Post(p) => p.post.uri.clone(),
        ThreadRoot::Missing { missing } => missing.clone(),
    };
    let saved = ctx.state.update_listings(|l| {
        l.thread = Some(root.clone());
        l.comments = thread.comments.iter().map(|c| c.uri.clone()).collect();
    });
    if let Err(e) = saved {
        ctx.out.warn(cat([
            lit("could not save the comment labels: "),
            tt(&e.to_string()),
        ]));
    }
    if ctx.out.is_json() {
        if args.no_comments {
            match &thread.post {
                ThreadRoot::Post(p) => ctx.out.json(p.as_ref()),
                ThreadRoot::Missing { missing } => {
                    return Err(EngineError::not_found(format!(
                        "the post {missing} is not available"
                    ))
                    .into());
                }
            }
        } else {
            ctx.out.json(&thread);
        }
        return Ok(());
    }
    let target = is_comment.then_some(uri.as_str());
    let text = thread_text(ctx, &engine, &thread, target, !args.no_comments).await;
    ctx.out.page(&text, !args.no_pager);
    Ok(())
}

fn line(ctx: &Ctx, spans: &[Span]) -> String {
    let mut s = ctx.out.spans(spans);
    s.push('\n');
    s
}

/// The text of a thread: the post, then (with `comments`) its comments labelled `c1…cN`, the
/// comment `target` marked.
async fn thread_text(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    thread: &ThreadView,
    target: Option<&str>,
    comments: bool,
) -> String {
    let style = body_style(ctx);
    let handles: BTreeMap<String, String> = thread
        .comments
        .iter()
        .map(|c| &c.author)
        .chain(match &thread.post {
            ThreadRoot::Post(p) => Some(&p.post.author),
            ThreadRoot::Missing { .. } => None,
        })
        .filter_map(|a| Some((a.did.clone(), a.handle.clone()?)))
        .collect();
    let handle_of = |did: &str| handles.get(did).cloned();
    let mut s = String::new();
    match &thread.post {
        ThreadRoot::Post(p) => s.push_str(&post_text(ctx, engine, p, &style, &handle_of).await),
        ThreadRoot::Missing { missing } => {
            s.push_str(&line(
                ctx,
                &[(lit("[the post is not available]"), Style::Dim)],
            ));
            s.push_str(&line(ctx, &[(tt(missing), Style::Dim)]));
        }
    }
    if !comments {
        return s;
    }
    s.push('\n');
    let n = u32::try_from(thread.comments.len()).unwrap_or(u32::MAX);
    s.push_str(&line(
        ctx,
        &[(render::count(n, "comment", "comments"), Style::Bold)],
    ));
    let min_depth = thread.comments.iter().map(|c| c.depth).min().unwrap_or(1);
    for (i, c) in thread.comments.iter().enumerate() {
        let depth = usize::try_from(c.depth.saturating_sub(min_depth)).unwrap_or(0);
        s.push('\n');
        s.push_str(
            &comment_text(
                ctx,
                engine,
                c,
                i + 1,
                depth * 2,
                target == Some(c.uri.as_str()),
                &style,
                &handle_of,
            )
            .await,
        );
    }
    s
}

async fn post_text(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    p: &PostView,
    style: &BodyStyle,
    handle_of: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut s = line(ctx, &[(post_title(&p.post), Style::Bold)]);
    let mut meta = post_meta(&p.post);
    if p.history.revisions > 0 {
        meta.push(render::count(p.history.revisions, "revision", "revisions"));
    }
    s.push_str(&line(ctx, &[(joined(meta), Style::Dim)]));
    if let Some(l) = &p.post.link {
        s.push_str(&line(
            ctx,
            &[(lit("link: "), Style::Dim), (tt(&l.uri), Style::Plain)],
        ));
    }
    s.push_str(&line(ctx, &[(tt(&p.post.uri), Style::Dim)]));
    let listed = matches!(
        p.post.state,
        ItemState::Visible | ItemState::Collapsed { .. }
    );
    if listed {
        s.push('\n');
        s.push_str(
            &render::body(
                engine.host(),
                &p.post.uri,
                &p.post.cid,
                &p.body,
                style,
                0,
                handle_of,
            )
            .await,
        );
    }
    s
}

#[allow(clippy::too_many_arguments)]
async fn comment_text(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    c: &CommentNode,
    label: usize,
    indent: usize,
    target: bool,
    style: &BodyStyle,
    handle_of: &dyn Fn(&str) -> Option<String>,
) -> String {
    let pad = " ".repeat(indent);
    let mut head: Vec<Span> = vec![
        (tt(&pad), Style::Plain),
        (
            if target { lit("> ") } else { TermText::empty() },
            Style::Warn,
        ),
        (tt(&format!("c{label}")), Style::Bold),
        (lit("  "), Style::Plain),
    ];
    if c.missing {
        head.push((lit("[missing comment]"), Style::Dim));
        return line(ctx, &head);
    }
    let mut meta = vec![
        render::actor(&c.author),
        tt(&render::time(c.created_at_ms)),
        cat([
            tt(&c.score.score.to_string()),
            lit(if c.score.score.abs() == 1 {
                " point"
            } else {
                " points"
            }),
        ]),
    ];
    if c.retracted {
        meta.push(lit("retracted"));
    }
    if c.orphaned {
        meta.push(lit("orphaned"));
    }
    if let Some(sl) = render::state_label(&c.state) {
        meta.push(sl);
    }
    for b in render::badges(&c.badges) {
        meta.push(lit(b));
    }
    head.push((joined(meta), Style::Dim));
    let mut s = line(ctx, &head);
    let shown = matches!(c.state, ItemState::Visible | ItemState::Collapsed { .. });
    if shown {
        s.push_str(
            &render::body(
                engine.host(),
                &c.uri,
                &c.cid,
                &c.body,
                style,
                indent + 4,
                handle_of,
            )
            .await,
        );
    }
    s
}

/// `lessdugin history <ref>`.
pub async fn history(ctx: &Ctx, args: RefArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let (uri, _) = refs::post_or_comment(ctx, &engine, &args.reference).await?;
    sync_thread_quietly(ctx, &engine, &uri).await;
    let h = engine.history(&uri).await?;
    if ctx.out.is_json() {
        ctx.out.json(&h);
        return Ok(());
    }
    ctx.out.line(&[(tt(&h.uri), Style::Dim)]);
    for v in &h.versions {
        let mut spans: Vec<Span> = vec![
            (tt(&format!("v{}", v.n)), Style::Bold),
            (lit("  "), Style::Plain),
            (tt(&render::time(v.created_at_ms)), Style::Plain),
            (lit("  "), Style::Plain),
            (tt(&v.cid), Style::Dim),
        ];
        if let Some(sum) = &v.summary {
            spans.push((lit("  "), Style::Plain));
            spans.push((tt(sum), Style::Plain));
        }
        if !v.available {
            spans.push((lit("  (not available)"), Style::Dim));
        }
        ctx.out.line(&spans);
    }
    Ok(())
}
