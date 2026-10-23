// SPDX-License-Identifier: AGPL-3.0-or-later
//! Forums and membership: `forum ls|show|follow|unfollow|use|create`, `join`, `leave`,
//! `members`, `approve` and `sync` (synthesis §6.10).
//!
//! `forum create` always writes the membership mode (default `approval`, `docs/plan.md` M2)
//! and makes the new forum the current one, as `join` does. `--json` prints the engine's DTOs
//! (`ForumView`, `SyncReport`, `WriteTicket` with `"dto": 1`) or a `{"dto": 1, …}` object
//! around a list.

use std::io::{IsTerminal, Write as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lessdugin_core::dto::{
    BodyDraft, DTO_VERSION, DefaultTier, Event, ForumDraft, ForumRole, ForumSummary, MemberView,
    MembersQuery, MembershipMode, SyncOpts, SyncReport, WriteTicket,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_native::host::NativeHost;
use serde::Serialize;

use crate::args::{
    ApproveArgs, ForumAction, ForumArgs, JoinArgs, LeaveArgs, MembersArgs, SyncArgs,
};
use crate::cmd::{read, refs, write};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, TermText, cat, lit, name, tt};
use crate::render;

fn mode(m: MembershipMode) -> &'static str {
    match m {
        MembershipMode::Open => "open",
        MembershipMode::Approval => "approval",
    }
}

fn role(r: ForumRole) -> &'static str {
    match r {
        ForumRole::Owner => "owner",
        ForumRole::Admin => "admin",
        ForumRole::Moderator => "moderator",
        ForumRole::Member => "member",
        ForumRole::Established => "established",
        ForumRole::Newcomer => "newcomer",
        ForumRole::None => "none",
    }
}

#[derive(Serialize)]
struct ForumList<'a> {
    dto: u32,
    forums: &'a [ForumSummary],
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FollowView {
    dto: u32,
    uri: String,
    followed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentView {
    dto: u32,
    current_forum: String,
}

#[derive(Serialize)]
struct MemberList<'a> {
    dto: u32,
    forum: &'a str,
    members: &'a [MemberView],
}

#[derive(Serialize)]
struct WriteList<'a> {
    dto: u32,
    writes: &'a [WriteTicket],
}

#[derive(Serialize)]
struct SyncReports<'a> {
    dto: u32,
    reports: &'a [SyncReport],
}

fn forum_name(f: &ForumSummary) -> TermText {
    name(&f.name)
}

/// `lessdugin forum …`.
pub async fn forum(ctx: &Ctx, args: ForumArgs) -> Result<(), CliError> {
    match args.action {
        ForumAction::Ls => ls(ctx).await,
        ForumAction::Show { forum } => show(ctx, forum.as_deref()).await,
        ForumAction::Follow { forum, alias } => {
            let engine = ctx.reader().await?;
            let uri = refs::forum(ctx, &engine, &forum).await?;
            // the declaration first: a forum that does not exist is not followed, and
            // `forum ls` shows its name and mode (offline, it is followed unchecked)
            if !ctx.offline() {
                engine.forum_open(&uri).await?;
            }
            engine.forum_follow(&uri, alias.as_deref()).await?;
            if ctx.out.is_json() {
                ctx.out.json(&FollowView {
                    dto: DTO_VERSION,
                    uri,
                    followed: true,
                    alias,
                });
            } else {
                ctx.out
                    .line(&[(lit("Following "), Style::Plain), (tt(&uri), Style::Bold)]);
            }
            Ok(())
        }
        ForumAction::Unfollow { forum } => {
            let engine = ctx.reader().await?;
            let uri = refs::forum_or_current(ctx, &engine, forum.as_deref()).await?;
            engine.forum_unfollow(&uri).await?;
            if ctx.state.current_forum().as_deref() == Some(uri.as_str()) {
                let _ = ctx.state.set_current_forum(None);
            }
            if ctx.out.is_json() {
                ctx.out.json(&FollowView {
                    dto: DTO_VERSION,
                    uri,
                    followed: false,
                    alias: None,
                });
            } else {
                ctx.out.line(&[
                    (lit("No longer following "), Style::Plain),
                    (tt(&uri), Style::Bold),
                ]);
            }
            Ok(())
        }
        ForumAction::Use { forum } => {
            let engine = ctx.reader().await?;
            let uri = refs::forum(ctx, &engine, &forum).await?;
            ctx.state
                .set_current_forum(Some(&uri))
                .map_err(|e| EngineError::internal(format!("saving the current forum: {e}")))?;
            if ctx.out.is_json() {
                ctx.out.json(&CurrentView {
                    dto: DTO_VERSION,
                    current_forum: uri,
                });
            } else {
                ctx.out.line(&[
                    (lit("Current forum: "), Style::Plain),
                    (tt(&uri), Style::Bold),
                ]);
            }
            Ok(())
        }
        ForumAction::Create {
            slug,
            mode,
            name,
            description,
            about,
            frontpage,
        } => {
            let w = ctx.writer().await?;
            let mut d = ForumDraft::new(slug.clone(), name.unwrap_or_else(|| slug.clone()));
            d.membership = mode.into();
            d.description = description;
            d.about = about.map(BodyDraft::text);
            d.default_tier = if frontpage {
                DefaultTier::Frontpage
            } else {
                DefaultTier::Personal
            };
            let ticket = w.engine.forum_create(&d).await?;
            let uri = format!(
                "at://{}/{}/{}",
                w.account.did,
                lessdugin_lexicon::collection::nsid::FORUM,
                slug
            );
            let _ = ctx.state.set_current_forum(Some(&uri));
            let t = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &t, "Created")
        }
    }
}

async fn ls(ctx: &Ctx) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let list = engine.followed().await?;
    let current = ctx.state.current_forum();
    let saved = ctx
        .state
        .update_listings(|l| l.forums = list.iter().map(|f| f.uri.clone()).collect());
    if let Err(e) = saved {
        ctx.out.warn(cat([
            lit("could not save the listing: "),
            tt(&e.to_string()),
        ]));
    }
    if ctx.out.is_json() {
        ctx.out.json(&ForumList {
            dto: DTO_VERSION,
            forums: &list,
            current,
        });
        return Ok(());
    }
    if list.is_empty() {
        ctx.out.line(&[(
            lit("No followed forums. Join one with `lessdugin join <handle>/<slug>`."),
            Style::Dim,
        )]);
    }
    let w = list.len().to_string().len();
    for (i, f) in list.iter().enumerate() {
        let mut meta = vec![
            render::actor(&f.owner),
            lit(mode(f.membership)),
            lit(if f.member { "member" } else { "not a member" }),
        ];
        if let Some(a) = &f.alias {
            meta.push(cat([lit("alias "), name(a)]));
        }
        let is_current = current.as_deref() == Some(f.uri.as_str());
        ctx.out.line(&[
            (tt(&format!("{:>w$}", i + 1)), Style::Bold),
            (lit(if is_current { " * " } else { "   " }), Style::Good),
            (forum_name(f), Style::Bold),
            (lit("  "), Style::Plain),
            (joined(meta), Style::Dim),
        ]);
        ctx.out.line(&[
            (tt(&" ".repeat(w + 3)), Style::Plain),
            (tt(&f.uri), Style::Dim),
        ]);
    }
    Ok(())
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

async fn show(ctx: &Ctx, forum: Option<&str>) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let uri = refs::forum_or_current(ctx, &engine, forum).await?;
    let mut v = engine.forum_open(&uri).await?;
    let unverified =
        v.forum.owner.handle.is_none() || v.moderators.iter().any(|m| m.actor.handle.is_none());
    if unverified && !ctx.offline() && engine.verify_handles(64).await > 0 {
        v = engine.forum_open(&uri).await?;
    }
    if ctx.out.is_json() {
        ctx.out.json(&v);
        return Ok(());
    }
    let f = &v.forum;
    let out = &ctx.out;
    out.line(&[(forum_name(f), Style::Bold)]);
    if let Some(d) = &f.description {
        out.line(&[(tt(d), Style::Plain)]);
    }
    out.field("uri", 11, tt(&f.uri));
    out.field("owner", 11, render::actor(&f.owner));
    out.field("membership", 11, lit(mode(f.membership)));
    out.field("your role", 11, lit(role(v.my_role)));
    out.field(
        "followed",
        11,
        lit(match (f.followed, &f.alias) {
            (true, Some(_)) => "yes (with an alias)",
            (true, None) => "yes",
            (false, _) => "no",
        }),
    );
    if let Some(a) = &f.alias {
        out.field("alias", 11, name(a));
    }
    for m in &v.moderators {
        let mut t = cat([
            render::actor(&m.actor),
            lit(match m.role {
                lessdugin_core::dto::ModRole::Admin => " (admin)",
                lessdugin_core::dto::ModRole::Moderator => " (moderator)",
            }),
        ]);
        if let Some(title) = &m.title {
            t.append(cat([lit(" "), tt(title)]));
        }
        out.field("moderator", 11, t);
    }
    out.field(
        "synced",
        11,
        cat([
            tt(&v.completeness.repos_complete.to_string()),
            lit(" of "),
            tt(&v.completeness.repos_total.to_string()),
            lit(" repositories"),
            lit(if v.completeness.degraded {
                " (moderation data incomplete)"
            } else {
                ""
            }),
        ]),
    );
    for b in &v.banners {
        out.warn(tt(&b.message));
    }
    if v.about.is_some() {
        let style = render::BodyStyle {
            color: out.color(),
            hyperlinks: out.color() && out.stdout_is_tty(),
            width: render::width(out.stdout_is_tty()),
        };
        if let Ok((_, decl)) = write::cached(&engine, &f.uri).await
            && let Some(about) = decl.get("about")
            && let Some(text) = render::markdown(about, &style, 2, &|_| None)
        {
            out.page(&format!("\n{text}"), false);
        }
    }
    Ok(())
}

/// `lessdugin join <forum> [--note] [--alias]`.
pub async fn join(ctx: &Ctx, args: JoinArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum(ctx, &w.engine, &args.forum).await?;
    let ticket = w.engine.join(&uri, args.note.as_deref()).await?;
    if let Some(a) = &args.alias {
        w.engine.forum_follow(&uri, Some(a)).await?;
    }
    let _ = ctx.state.set_current_forum(Some(&uri));
    let t = write::drive(ctx, &w, ticket).await?;
    write::report(ctx, &t, "Joined")?;
    if !ctx.out.is_json()
        && !ctx.offline()
        && let Ok(v) = w.engine.forum_open(&uri).await
        && v.forum.membership == MembershipMode::Approval
        && matches!(v.my_role, ForumRole::None)
    {
        ctx.out.note(&[(
            lit("This forum needs approval: an owner or admin must approve you before your posts are shown."),
            Style::Dim,
        )]);
    }
    Ok(())
}

/// `lessdugin leave [forum]`.
pub async fn leave(ctx: &Ctx, args: LeaveArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum_or_current(ctx, &w.engine, args.forum.as_deref()).await?;
    let ticket = w.engine.leave(&uri).await?;
    let t = write::drive(ctx, &w, ticket).await?;
    write::report(ctx, &t, "Left")
}

/// `lessdugin members [forum] [--pending]`.
pub async fn members(ctx: &Ctx, args: MembersArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let uri = refs::forum_or_current(ctx, &engine, args.forum.as_deref()).await?;
    read::sync_forum_quietly(ctx, &engine, &uri).await;
    let q = MembersQuery {
        status: None,
        pending: args.pending.then_some(true),
    };
    let mut list = engine.members(&uri, &q).await?;
    if !ctx.offline()
        && list.iter().any(|m| m.actor.handle.is_none())
        && engine.verify_handles(256).await > 0
    {
        list = engine.members(&uri, &q).await?;
    }
    if ctx.out.is_json() {
        ctx.out.json(&MemberList {
            dto: DTO_VERSION,
            forum: &uri,
            members: &list,
        });
        return Ok(());
    }
    if list.is_empty() {
        ctx.out.line(&[(
            lit(if args.pending {
                "No applicants."
            } else {
                "No members."
            }),
            Style::Dim,
        )]);
    }
    for m in &list {
        let mut meta = vec![lit(role(m.status))];
        if m.granted {
            meta.push(lit("approved"));
        }
        meta.push(cat([lit("joined "), tt(&render::time(m.joined_at_ms))]));
        if let Some(n) = &m.membership_note {
            meta.push(cat([lit("note: "), tt(n)]));
        }
        ctx.out.line(&[
            (render::actor(&m.actor), Style::Bold),
            (lit("  "), Style::Plain),
            (joined(meta), Style::Dim),
        ]);
    }
    Ok(())
}

/// `lessdugin approve <actor>...`.
pub async fn approve(ctx: &Ctx, args: ApproveArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum_or_current(ctx, &w.engine, None).await?;
    let tickets = w.engine.grant(&uri, &args.actors).await?;
    let mut done = Vec::with_capacity(tickets.len());
    for t in tickets {
        done.push(write::drive(ctx, &w, t).await?);
    }
    if ctx.out.is_json() {
        ctx.out.json(&WriteList {
            dto: DTO_VERSION,
            writes: &done,
        });
    }
    let mut first_error = None;
    for (actor, t) in args.actors.iter().zip(&done) {
        match &t.state {
            lessdugin_core::dto::WriteState::Failed { error } => {
                if !ctx.out.is_json() {
                    ctx.out.err_line(&[
                        (lit("error: "), Style::Bad),
                        (name(actor), Style::Bold),
                        (lit(": "), Style::Plain),
                        (tt(&error.message), Style::Plain),
                    ]);
                }
                first_error.get_or_insert_with(|| error.clone());
            }
            _ if ctx.out.is_json() => {}
            lessdugin_core::dto::WriteState::Done { .. } => ctx
                .out
                .line(&[(lit("Approved "), Style::Good), (name(actor), Style::Bold)]),
            _ => ctx.out.line(&[
                (lit("Queued approval of "), Style::Warn),
                (name(actor), Style::Bold),
                (lit(": write "), Style::Plain),
                (tt(&t.id), Style::Plain),
            ]),
        }
    }
    match first_error {
        Some(e) => {
            if ctx.out.is_json() {
                Err(CliError::Engine(e))
            } else {
                Err(CliError::Reported(e.exit_code()))
            }
        }
        None => Ok(()),
    }
}

/// Shows `SyncProgress` on a terminal's stderr while `sync` runs.
fn progress(ctx: &Ctx, engine: &Engine<NativeHost>) -> Option<(Arc<AtomicBool>, u64)> {
    if ctx.out.is_json() || ctx.out.is_quiet() || !std::io::stderr().is_terminal() {
        return None;
    }
    let shown = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&shown);
    let id = engine.subscribe(move |e: Event| {
        if let Event::SyncProgress { done, total, .. } = e {
            s.store(true, Ordering::SeqCst);
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[2Ksyncing: {done}/{total} repositories");
            let _ = err.flush();
        }
    });
    Some((shown, id.0))
}

/// `lessdugin sync [forum] [--full]`: sends the account's queued writes (when logged in), then
/// syncs the forum, or every followed forum. Exit 6 when a sync was partial.
pub async fn sync(ctx: &Ctx, args: SyncArgs) -> Result<(), CliError> {
    if ctx.offline() {
        return Err(CliError::usage("sync needs the network; drop --offline"));
    }
    let engine = ctx.reader().await?;
    // queued writes first, so the sync reads them back
    if engine.active_account().await?.is_some() || ctx.g.account.is_some() {
        match ctx.writer().await {
            Ok(w) => {
                let run = write::drive_all(ctx, &w).await?;
                if !run.is_empty() {
                    let n = u32::try_from(run.len()).unwrap_or(u32::MAX);
                    ctx.out.note(&[(
                        cat([
                            lit("sent "),
                            render::count(n, "queued write", "queued writes"),
                        ]),
                        Style::Dim,
                    )]);
                }
                if let Some(e) = write::first_failure(&run) {
                    ctx.out
                        .warn(cat([lit("a queued write failed: "), tt(&e.message)]));
                }
            }
            Err(CliError::Engine(e)) => ctx
                .out
                .warn(cat([lit("queued writes were not sent: "), tt(&e.message)])),
            Err(e) => return Err(e),
        }
    }
    let forums = match args.forum.as_deref().or(ctx.g.forum.as_deref()) {
        Some(f) => vec![refs::forum(ctx, &engine, f).await?],
        None => match ctx.state.current_forum() {
            Some(f) => vec![f],
            None => engine
                .followed()
                .await?
                .into_iter()
                .map(|f| f.uri)
                .collect(),
        },
    };
    if forums.is_empty() {
        return Err(CliError::usage(
            "no forum to sync: name one, or follow one with `lessdugin join` / `forum follow`",
        ));
    }
    let opts = SyncOpts {
        budget_ms: None,
        max_requests: None,
        full: args.full,
    };
    let shown = progress(ctx, &engine);
    let mut reports = Vec::new();
    let mut first_error = None;
    for f in &forums {
        match engine.sync_forum(f, &opts).await {
            Ok(r) => reports.push(r),
            Err(e) => {
                if forums.len() == 1 {
                    first_error = Some(e);
                } else {
                    ctx.out.warn(cat([
                        lit("could not sync "),
                        tt(f),
                        lit(": "),
                        tt(&e.message),
                    ]));
                    first_error.get_or_insert(e);
                }
            }
        }
    }
    if let Some((shown, id)) = shown {
        engine.unsubscribe(lessdugin_core::engine::SubscriptionId(id));
        if shown.load(Ordering::SeqCst) {
            let _ = write!(std::io::stderr().lock(), "\r\x1b[2K");
        }
    }
    if reports.is_empty()
        && let Some(e) = first_error
    {
        return Err(e.into());
    }
    if ctx.out.is_json() {
        match reports.as_slice() {
            [one] if forums.len() == 1 => ctx.out.json(one),
            _ => ctx.out.json(&SyncReports {
                dto: DTO_VERSION,
                reports: &reports,
            }),
        }
    } else {
        for r in &reports {
            ctx.out
                .line(&[(lit("Synced "), Style::Plain), (tt(&r.forum), Style::Bold)]);
            ctx.out.line(&[(
                cat([
                    lit("  "),
                    tt(&r.repos_complete.to_string()),
                    lit(" of "),
                    tt(&r.repos_total.to_string()),
                    tt(" repositories complete · "),
                    render::count(r.changed, "record changed", "records changed"),
                    tt(" · "),
                    render::count(r.requests, "request", "requests"),
                ]),
                Style::Dim,
            )]);
            for i in &r.errors {
                let who = i.did.as_deref().or(i.host.as_deref()).unwrap_or("");
                ctx.out.warn(cat([tt(who), lit(": "), tt(&i.message)]));
            }
        }
    }
    if reports.iter().any(|r| r.partial) {
        if !ctx.out.is_json() {
            ctx.out.note(&[(
                lit("partial: some repositories could not be synced completely"),
                Style::Dim,
            )]);
        }
        return Err(CliError::Reported(
            lessdugin_core::engine::ErrorCode::Partial.exit_code(),
        ));
    }
    if let Some(e) = first_error {
        return Err(CliError::Reported(e.exit_code()));
    }
    Ok(())
}
