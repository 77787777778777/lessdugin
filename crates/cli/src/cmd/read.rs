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
//!
//! A view waits `BODY_BLOB_WAIT_MS` for a spilled body and returns it pending; its download runs
//! on, but only while the process does, and `read` is one view per process. So `read` waits for
//! the host downloads of the bodies its view left pending (each capped by
//! `BLOB_TOTAL_TIMEOUT_MS`) and reads the thread again, or says which body is still not loaded
//! (`limits.md` `BODY_BLOB_WAIT_MS`, `lfm-1.md` §1.3).

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
    // Only a sync that reached every repository counts as fresh: one whose repositories failed
    // (a hung or broken PDS) is retried by the next command instead of skipped for minutes.
    if res
        .as_ref()
        .is_ok_and(|r| !r.partial && r.errors.is_empty())
    {
        let _ = ctx.state.mark_synced(forum);
    }
    match res {
        Ok(r) if !r.errors.is_empty() => {
            let first = &r.errors[0];
            ctx.out.warn(cat([
                lit("the sync failed for "),
                render::count(
                    u32::try_from(r.errors.len()).unwrap_or(u32::MAX),
                    "repository",
                    "repositories",
                ),
                lit(", showing the cache (first: "),
                tt(first
                    .did
                    .as_deref()
                    .or(first.host.as_deref())
                    .unwrap_or("?")),
                lit(": "),
                tt(&first.message),
                lit(")"),
            ]));
        }
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

/// The M3 default is `frontpage` (`engine-api.md` §3.4, `social.md` §12.3);
/// `--all` and `--curated` override it.
fn filter_of(a: &LsArgs) -> PostFilter {
    if a.all {
        PostFilter::All
    } else if a.curated {
        PostFilter::Curated
    } else {
        PostFilter::Frontpage
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
        l.posts_sequence = None;
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
        // the displayed time is t_rank, never the claimed createdAt (`social.md` §2.3)
        tt(&render::time(p.t_rank_ms)),
        render::count(p.comment_count, "comment", "comments"),
    ];
    parts.extend(render::score(&p.score));
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
    for b in &list.banners {
        ctx.out
            .line(&[(cat([lit("! "), tt(&b.message)]), Style::Warn)]);
    }
    print_posts(ctx, &list.items);
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

fn print_posts(ctx: &Ctx, posts: &[PostSummary]) {
    if posts.is_empty() {
        ctx.out.line(&[(lit("No posts."), Style::Dim)]);
    }
    let w = posts.len().to_string().len();
    for (i, p) in posts.iter().enumerate() {
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
}

/// `lessdugin linked <uri>`: the listed posts whose rendered bodies link to the item.
pub async fn linked(ctx: &Ctx, args: RefArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let uri = match refs::item(ctx, &engine, &args.reference).await? {
        lessdugin_core::dto::Ref::Post { uri }
        | lessdugin_core::dto::Ref::Comment { uri }
        | lessdugin_core::dto::Ref::Sequence { uri }
        | lessdugin_core::dto::Ref::Record { uri } => uri,
        _ => {
            return Err(CliError::Engine(EngineError::invalid_ref(format!(
                "{:?} is not an item",
                args.reference
            ))));
        }
    };
    let mut posts = engine.backlinks(&uri).await?;
    if unverified(ctx, posts.iter().map(|p| &p.author)) && verify_handles(&engine).await {
        posts = engine.backlinks(&uri).await?;
    }
    let saved = ctx.state.update_listings(|l| {
        l.posts_forum = None;
        l.posts = posts.iter().map(|p| p.uri.clone()).collect();
        l.posts_sequence = None;
    });
    if let Err(e) = saved {
        ctx.out.warn(cat([
            lit("could not save the listing: "),
            tt(&e.to_string()),
        ]));
    }
    if ctx.out.is_json() {
        ctx.out.json(&posts);
    } else {
        print_posts(ctx, &posts);
    }
    Ok(())
}

/// `lessdugin read <ref>`.
///
/// A post read from a sequence (`--sequence`, a number of `sequence read`, `next`, `prev`) is
/// read in it (`get_post(uri, sequence)`, `social.md` §14): the thread's post carries that
/// sequence's navigation, and the post becomes its continue-reading position on this device.
pub async fn read(ctx: &Ctx, args: ReadArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let sequence = args
        .sequence
        .clone()
        .or_else(|| refs::sequence_of(ctx, &args.reference));
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
    if await_spilled(ctx, &engine, "body", &pending_bodies(&thread)).await {
        thread = engine.get_thread(&uri, &opts).await?;
    }
    for (item, _) in pending_bodies(&thread) {
        ctx.out.warn(cat([
            lit("the body of "),
            tt(&item),
            lit(
                " is still not loaded from its author's PDS (the excerpt shows); read it again \
                 later",
            ),
        ]));
    }
    // next and previous follow the sequence the post was read from
    if let (Some(sequence), ThreadRoot::Post(p)) = (&sequence, &mut thread.post) {
        p.sequences = engine
            .get_post_in(&p.post.uri, Some(sequence))
            .await?
            .sequences;
    }
    let root = match &thread.post {
        ThreadRoot::Post(p) => p.post.uri.clone(),
        ThreadRoot::Missing { missing } => missing.clone(),
    };
    let nav = match &thread.post {
        ThreadRoot::Post(p) => p.sequences.first().cloned(),
        ThreadRoot::Missing { .. } => None,
    };
    let saved = ctx.state.update_listings(|l| {
        l.thread = Some(root.clone());
        l.comments = thread.comments.iter().map(|c| c.uri.clone()).collect();
        l.sequence_nav = nav;
    });
    if let Err(e) = saved {
        ctx.out.warn(cat([
            lit("could not save the comment labels: "),
            tt(&e.to_string()),
        ]));
    }
    // viewing a thread sets its read mark (`social.md` §16); this view keeps its unread marks
    if !args.no_comments
        && let ThreadRoot::Post(p) = &thread.post
        && let Err(e) = engine.mark_read(&p.post.uri).await
    {
        ctx.out.warn(cat([
            lit("could not mark the thread read: "),
            tt(&e.message),
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

/// Whether a view left this spilled body pending (`lfm-1.md` §1.3: it carries
/// `NOTICE_SPILLED`).
pub(crate) fn spilled_pending(body: &lessdugin_core::dto::RenderedBody) -> bool {
    use lessdugin_core::model::render::NOTICE_SPILLED;
    body.notices.iter().any(|n| n == NOTICE_SPILLED)
}

/// The items of `thread` whose spilled body its view left pending, with their authors.
fn pending_bodies(thread: &ThreadView) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let ThreadRoot::Post(p) = &thread.post
        && spilled_pending(&p.body)
    {
        out.push((p.post.uri.clone(), p.post.author.did.clone()));
    }
    for c in &thread.comments {
        if spilled_pending(&c.body) {
            out.push((c.uri.clone(), c.author.did.clone()));
        }
    }
    out
}

/// Awaits the host download of every spilled body in `pending` (the item, its author), the
/// `field` of the item's cached record: the one its view started (the host runs one per CID,
/// so this joins it, `sync.md` §12.2), else a new one, each capped by `BLOB_TOTAL_TIMEOUT_MS`.
/// The blob comes from the author's PDS as their DID document names it, as the engine's own
/// fetch does (`sync.md` §11.1). Returns true when one was stored, so the caller views again;
/// nothing is waited for `--offline`. `read`, `forum show` (a spilled `about`), `tag <slug>` (its
/// page) and `sequence read` use it.
pub(crate) async fn await_spilled(
    ctx: &Ctx,
    engine: &Engine<NativeHost>,
    field: &str,
    pending: &[(String, String)],
) -> bool {
    use lessdugin_core::host::Host;
    use lessdugin_core::identity::IdentityLevel;
    if pending.is_empty() || ctx.offline() {
        return false;
    }
    ctx.out.note(&[(
        cat([
            lit("Waiting for "),
            render::count(
                u32::try_from(pending.len()).unwrap_or(u32::MAX),
                "body",
                "bodies",
            ),
            lit(" still downloading from the author's PDS (Ctrl-C stops)..."),
        ]),
        Style::Dim,
    )]);
    let fetches = pending.iter().map(|(item, author)| async move {
        let (_, record) = crate::cmd::write::cached(engine, item).await.ok()?;
        let cid = lessdugin_core::model::render::text_blob_cid(record.get(field)?)?.to_owned();
        let identity = engine.resolve_identity(author).await.ok()?.identity;
        if identity.level != IdentityLevel::Doc {
            return None;
        }
        let pds = url::Url::parse(identity.pds.as_deref()?).ok()?;
        match engine
            .host()
            .fetch_blob(
                pds,
                identity.did,
                cid,
                lessdugin_core::limits::BLOB_TEXT_MAX_BYTES,
            )
            .await
        {
            Ok(_) => Some(()),
            Err(e) => {
                tracing::debug!(uri = %item, error = %e, "a spilled body did not download");
                None
            }
        }
    });
    futures::future::join_all(fetches)
        .await
        .iter()
        .any(Option::is_some)
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
    let mut s = banner_lines(ctx, &thread.banners);
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
    s.push_str(&react_lines(ctx, &p.reacts, 0, handle_of));
    s.push_str(&sequence_lines(ctx, &p.sequences));
    s
}

/// The post's place in a sequence (`social.md` §14): the sequence, and its previous and next
/// posts, which `read prev` and `read next` read in it.
fn sequence_lines(ctx: &Ctx, navs: &[lessdugin_core::dto::SequenceNav]) -> String {
    let mut s = String::new();
    for nav in navs {
        s.push('\n');
        s.push_str(&line(
            ctx,
            &[
                (lit("sequence: "), Style::Dim),
                (tt(&nav.title), Style::Bold),
                (lit("  "), Style::Plain),
                (tt(&nav.sequence), Style::Dim),
            ],
        ));
        for (label, step, word) in [
            ("  previous: ", &nav.prev, "prev"),
            ("  next:     ", &nav.next, "next"),
        ] {
            if let Some(uri) = step {
                s.push_str(&line(
                    ctx,
                    &[
                        (lit(label), Style::Dim),
                        (tt(uri), Style::Plain),
                        (cat([lit("  (`read "), lit(word), lit("`)")]), Style::Dim),
                    ],
                ));
            }
        }
    }
    s
}

/// The react tallies listed under an item (`social.md` §9.2), `indent` columns in.
fn react_lines(
    ctx: &Ctx,
    reacts: &[lessdugin_core::dto::ReactTally],
    indent: usize,
    handle_of: &dyn Fn(&str) -> Option<String>,
) -> String {
    let pad = " ".repeat(indent);
    reacts
        .iter()
        .map(|t| {
            line(
                ctx,
                &[
                    (tt(&pad), Style::Plain),
                    (render::react(t, handle_of), Style::Dim),
                ],
            )
        })
        .collect()
}

/// A view's banners (`social.md` §1.6, §4.11: "moderation data unavailable"), above it.
fn banner_lines(ctx: &Ctx, banners: &[lessdugin_core::dto::Banner]) -> String {
    banners
        .iter()
        .map(|b| line(ctx, &[(cat([lit("! "), tt(&b.message)]), Style::Warn)]))
        .collect()
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
    let mut meta = vec![render::actor(&c.author), tt(&render::time(c.t_rank_ms))];
    meta.extend(render::score(&c.score));
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
        s.push_str(&react_lines(ctx, &c.reacts, indent + 4, handle_of));
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
