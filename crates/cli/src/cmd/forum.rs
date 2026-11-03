// SPDX-License-Identifier: AGPL-3.0-or-later
//! Forums and membership: `forum ls|show|follow|unfollow|use|create|edit`, `join`, `leave`,
//! `members` (and `members revoke`), `approve`, `reject` and `sync` (synthesis §6.10).
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
    MembersQuery, MembershipMode, PublicActionKind, SyncOpts, SyncReport, WriteTicket,
};
use lessdugin_core::engine::{Engine, EngineError};
use lessdugin_core::indexer::AuditReport;
use lessdugin_native::host::NativeHost;
use serde::Serialize;

use crate::args::{
    ApproveArgs, DiscoverArgs, ForumAction, ForumArgs, ForumEditArgs, JoinArgs, LeaveArgs,
    MembersAction, MembersArgs, RejectArgs, SyncArgs,
};
use crate::cmd::{read, refs, social, write};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, TermText, cat, lit, name, tt};
use crate::render;

/// The L7 label of an open forum (`threat-model.md` L7, SOC-13): only approval mode is complete
/// without third-party servers.
const OPEN_MODE_LABEL: &str = "Open mode relies on Bluesky relay, Jetstream and Constellation \
                               (or an indexer) to find new members.";

/// The banner code of the same label, should the engine carry it (shown once).
const RELAY_DEPENDENT: &str = "relayDependent";

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
            if d.membership == MembershipMode::Open {
                ctx.out.warn(lit(OPEN_MODE_LABEL));
            }
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
        ForumAction::Edit(a) => edit(ctx, a).await,
    }
}

/// An optional text option: empty removes the field.
fn text_option(given: Option<String>, current: Option<String>) -> Option<String> {
    match given {
        Some(t) if t.trim().is_empty() => None,
        Some(t) => Some(t),
        None => current,
    }
}

/// `lessdugin forum edit [forum] …` (`engine-api.md` §3.2 `forum_update`): the owner changes
/// the declaration through the edit protocol; every field not given keeps its current value
/// (read from the cached declaration). Another account is refused before anything is read:
/// the declaration lives in the owner's repository, and a spilled about is fetched only by its
/// owner (m4-5), so the engine's refusal of the write never comes after a fetch.
async fn edit(ctx: &Ctx, a: ForumEditArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum_or_current(ctx, &w.engine, a.forum.as_deref()).await?;
    if write::owner_of(&uri)? != w.account.did {
        return Err(EngineError::blocked(
            "only the forum's owner can change it (the declaration is in the owner's repo)",
        )
        .into());
    }
    // loads (and caches) the declaration
    w.engine.forum_open(&uri).await?;
    let (_, decl) = write::cached(&w.engine, &uri).await?;
    let current: lessdugin_lexicon::records::Forum = serde_json::from_value(decl.clone())
        .map_err(|e| EngineError::internal(format!("{uri}: {e}")))?;
    let about = match a.about {
        Some(t) if t.trim().is_empty() => None,
        Some(t) => Some(BodyDraft::text(t)),
        // the current about is kept: its text (a spilled one fetched from the owner's PDS) and
        // its media
        None => match decl.get("about") {
            None => None,
            Some(md) if md.get("text").is_none() && md.get("textBlob").is_none() => {
                return Err(CliError::usage(
                    "this forum's about text is in a format this client cannot keep; pass \
                     --about to replace it",
                ));
            }
            Some(md) => Some(write::kept_body(&w, &uri, md).await?),
        },
    };
    let mut d = ForumDraft::new(String::new(), a.name.unwrap_or(current.name.clone()));
    d.description = text_option(a.description, current.description.clone());
    d.about = about;
    d.membership = match a.mode {
        Some(m) => m.into(),
        None => match current.membership_mode().value {
            lessdugin_lexicon::values::MembershipMode::Open => MembershipMode::Open,
            lessdugin_lexicon::values::MembershipMode::Approval => MembershipMode::Approval,
        },
    };
    d.default_tier = match a.default_tier {
        Some(t) => t.into(),
        None => match current.default_tier().value {
            lessdugin_lexicon::values::DefaultTier::Frontpage => DefaultTier::Frontpage,
            lessdugin_lexicon::values::DefaultTier::Personal => DefaultTier::Personal,
        },
    };
    d.featured_tags = if a.no_featured_tags {
        Vec::new()
    } else if !a.featured_tags.is_empty() {
        a.featured_tags
    } else {
        current.featured_tags.clone().unwrap_or_default()
    };
    d.report_contact = text_option(a.report_contact, current.report_contact.clone());
    if a.mode.is_some() && d.membership == MembershipMode::Open {
        ctx.out.warn(lit(OPEN_MODE_LABEL));
    }
    let ticket = w.engine.forum_update(&uri, &d).await?;
    let t = write::drive(ctx, &w, ticket).await?;
    write::report(ctx, &t, "Updated")
}

async fn ls(ctx: &Ctx) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let list = engine.followed().await?;
    print_forums(ctx, list, ctx.state.current_forum(), true)
}

/// `lessdugin discover [query]`: forums found through the configured discovery sources.
/// The engine reports direct-mode and unsupported-source errors rather than substituting an
/// empty local list.
pub async fn discover(ctx: &Ctx, args: DiscoverArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let list = engine
        .forum_discover(args.query.as_deref().unwrap_or(""))
        .await?;
    print_forums(ctx, list, None, false)
}

fn print_forums(
    ctx: &Ctx,
    list: Vec<ForumSummary>,
    current: Option<String>,
    followed: bool,
) -> Result<(), CliError> {
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
            lit(if followed {
                "No followed forums. Join one with `lessdugin join <handle>/<slug>`."
            } else {
                "No forums found."
            }),
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
    // one view per process, as `read`: the download of a spilled about the view left pending
    // ends with the process, so it is awaited and the forum opened again (`limits.md`
    // `BODY_BLOB_WAIT_MS`)
    let about_pending = |v: &lessdugin_core::dto::ForumView| {
        v.about.as_ref().is_some_and(|a| {
            a.notices
                .iter()
                .any(|n| n == lessdugin_core::model::render::NOTICE_SPILLED)
        })
    };
    if about_pending(&v) {
        let owner = [(v.forum.uri.clone(), v.forum.owner.did.clone())];
        if read::await_spilled(ctx, &engine, "about", &owner).await {
            v = engine.forum_open(&uri).await?;
        }
    }
    if about_pending(&v) {
        ctx.out.warn(lit(
            "the forum's about is still not loaded from its owner's PDS (the excerpt shows); \
             show it again later",
        ));
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
    if f.membership == MembershipMode::Open {
        out.field("", 11, lit(OPEN_MODE_LABEL));
    }
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
    for b in v.banners.iter().filter(|b| b.code != RELAY_DEPENDENT) {
        out.warn(tt(&b.message));
    }
    if let Some(view) = &v.about {
        let style = render::BodyStyle {
            color: out.color(),
            hyperlinks: out.color() && out.stdout_is_tty(),
            width: render::width(out.stdout_is_tty()),
        };
        // the cached markdown, else (a spilled about) what the view shows: its excerpt and
        // notice
        let local = match write::cached(&engine, &f.uri).await {
            Ok((_, decl)) => decl
                .get("about")
                .and_then(|about| render::markdown(about, &style, 2, &|_| None)),
            Err(_) => None,
        };
        let text = local.unwrap_or_else(|| render::view_body(view, &style, 2));
        if !text.trim().is_empty() {
            out.page(&format!("\n{text}"), false);
        }
    }
    Ok(())
}

/// `lessdugin join <forum> [--note] [--alias]`.
pub async fn join(ctx: &Ctx, args: JoinArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum(ctx, &w.engine, &args.forum).await?;
    // joining (applying, with its note) is public (`social.md` §17)
    social::acknowledge(ctx, &w, PublicActionKind::Join, args.yes).await?;
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

/// `lessdugin members [forum] [--pending]`, and `members revoke <actor>...`.
pub async fn members(ctx: &Ctx, args: MembersArgs) -> Result<(), CliError> {
    if let Some(MembersAction::Revoke { actors, yes }) = args.action {
        return revoke(ctx, args.forum.as_deref(), &actors, yes).await;
    }
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
    // a grant is an `#access` mod action (`social.md` §17)
    social::acknowledge(ctx, &w, PublicActionKind::ModAction, args.yes).await?;
    // the grant, and the actors leave this device's rejected list (`reject`)
    let tickets = w.engine.approve(&uri, &args.actors).await?;
    let mut done = Vec::with_capacity(tickets.len());
    for t in tickets {
        done.push(write::drive(ctx, &w, t).await?);
    }
    report_each(ctx, &args.actors, &done, "Approved ", "Queued approval of ")
}

/// `lessdugin members revoke <actor>...` (`engine-api.md` §3.2 `revoke`): removes the
/// account's grant of membership (its `#access{member}`) of each actor; the removal is a mod
/// action (`social.md` §17).
async fn revoke(
    ctx: &Ctx,
    forum: Option<&str>,
    actors: &[String],
    yes: bool,
) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum_or_current(ctx, &w.engine, forum).await?;
    social::acknowledge(ctx, &w, PublicActionKind::ModAction, yes).await?;
    let tickets = w.engine.revoke(&uri, actors).await?;
    let mut done = Vec::with_capacity(tickets.len());
    for t in tickets {
        done.push(write::drive(ctx, &w, t).await?);
    }
    report_each(ctx, actors, &done, "Revoked ", "Queued revocation of ")
}

/// Reports one write per actor (`approve`, `members revoke`): `--json` prints the tickets as
/// one list; the first failure is the exit code.
fn report_each(
    ctx: &Ctx,
    actors: &[String],
    done: &[WriteTicket],
    did: &'static str,
    queued: &'static str,
) -> Result<(), CliError> {
    if ctx.out.is_json() {
        ctx.out.json(&WriteList {
            dto: DTO_VERSION,
            writes: done,
        });
    }
    let mut first_error = None;
    for (actor, t) in actors.iter().zip(done) {
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
                .line(&[(lit(did), Style::Good), (name(actor), Style::Bold)]),
            _ => ctx.out.line(&[
                (lit(queued), Style::Warn),
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

/// `lessdugin reject <actor>...` (`engine-api.md` §3.2): dismisses the applicants on this
/// device (nothing public is written for them; they leave `members --pending`), and removes the
/// account's own grant of an actor, if it made one.
pub async fn reject(ctx: &Ctx, args: RejectArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let uri = refs::forum_or_current(ctx, &w.engine, None).await?;
    let tickets = w.engine.reject(&uri, &args.actors).await?;
    let mut done = Vec::with_capacity(tickets.len());
    for t in tickets {
        done.push(write::drive(ctx, &w, t).await?);
    }
    if ctx.out.is_json() {
        ctx.out.json(&WriteList {
            dto: DTO_VERSION,
            writes: &done,
        });
        if let Some(e) = done.iter().find_map(|t| match &t.state {
            lessdugin_core::dto::WriteState::Failed { error } => Some(error.clone()),
            _ => None,
        }) {
            return Err(CliError::Engine(e));
        }
        return Ok(());
    }
    for actor in &args.actors {
        ctx.out
            .line(&[(lit("Rejected "), Style::Good), (name(actor), Style::Bold)]);
    }
    for t in &done {
        write::report(ctx, t, "Removed your grant")?;
    }
    Ok(())
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
    if args.audit {
        let input = args
            .forum
            .clone()
            .or_else(|| ctx.g.forum.clone())
            .or_else(|| ctx.state.current_forum());
        let forum = match input {
            Some(f) => refs::forum(ctx, &engine, &f).await?,
            None => {
                return Err(CliError::usage(
                    "sync --audit needs exactly one forum (name it or set a current forum)",
                ));
            }
        };
        let report = engine.audit_indexer(&forum).await?.ok_or_else(|| {
            CliError::usage("sync --audit needs assisted mode and a consented indexer")
        })?;
        print_audit(ctx, &report);
        if report.has_contradiction_or_omission() {
            return Err(CliError::Reported(6));
        }
        return Ok(());
    }
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

fn print_audit(ctx: &Ctx, report: &AuditReport) {
    if ctx.out.is_json() {
        ctx.out.json(report);
        return;
    }
    ctx.out.line(&[
        (lit("Audited "), Style::Plain),
        (tt(&report.forum), Style::Bold),
        (lit(" against "), Style::Plain),
        (tt(&report.indexer), Style::Bold),
    ]);
    for check in &report.checks {
        ctx.out.line(&[(
            cat([
                lit("  "),
                tt(&check.name),
                lit(": "),
                tt(&check.counts.checked.to_string()),
                lit(" checked · "),
                tt(&check.counts.contradictions.to_string()),
                lit(" contradictions · "),
                tt(&check.counts.omissions.to_string()),
                lit(" omissions · "),
                tt(&check.counts.lag.to_string()),
                lit(" lag"),
            ]),
            Style::Dim,
        )]);
        for finding in &check.findings {
            ctx.out.warn(cat([
                lit(match finding.kind {
                    lessdugin_core::indexer::AuditKind::Contradiction => "CONTRADICTION: ",
                    lessdugin_core::indexer::AuditKind::Omission => "OMISSION: ",
                    lessdugin_core::indexer::AuditKind::Lag => "LAG: ",
                }),
                tt(&finding.subject),
                lit(" — "),
                tt(&finding.detail),
            ]));
        }
    }
}
