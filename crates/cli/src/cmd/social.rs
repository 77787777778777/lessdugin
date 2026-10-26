// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 social commands (synthesis §6.10): opinions (`vote`, `agree`, `react`, `reacts`),
//! `notifications`, `activity`, `mods` and `mod`.
//!
//! Opinions and mod actions are public records (`social.md` §17): the first use of each kind
//! shows the "this is public" interstitial and records the acknowledgement
//! (`engine.acknowledge_public_action`). Every action returns an [`ActionTicket`]: its write is
//! driven like any other write ([`super::write::drive`]), and its notices (a vote recorded but
//! not counted yet, a retroactive restriction, an adoption that runs first) are printed with
//! the outcome.

use lessdugin_atp::syntax::Datetime;
use lessdugin_core::dto::{
    ActionTicket, Capability, ModActionIntent, ModActionValue, ModScope, ModsChange,
    NotificationsQuery, OpinionIntent, PublicActionKind, TextQuote, Tier, WriteState,
};
use lessdugin_core::engine::EngineError;
use lessdugin_core::model::palette;
use lessdugin_native::host::NativeHost;
use serde_json::Value;

use crate::args::{
    ActivityArgs, AgreeArgs, ExportArgs, ExportScopeArg, ModAction, ModArgs, ModSubject,
    ModsAction, ModsArgs, NotificationsArgs, PinArgs, ReactArgs, VoteArgs, scope_of, stance_of,
};
use crate::cmd::{read, refs, write};
use crate::ctx::{CliError, Ctx, Writer};
use crate::out::{Style, cat, dto, lit, name, tt};
use crate::prompt;
use crate::render;

// ---------------------------------------------------------------------------------------------
// the public-action interstitial

/// The interstitial texts of `social.md` §17, per action kind.
fn public_notice(kind: PublicActionKind) -> &'static str {
    match kind {
        PublicActionKind::Vote => {
            "Your votes are public records in your repository: everyone can see what you voted \
             on and how."
        }
        PublicActionKind::React => {
            "Your reacts are public records in your repository, including the quoted passage."
        }
        PublicActionKind::TagVote => "Your tag votes are public records in your repository.",
        PublicActionKind::Join => {
            "Your membership (and its note) is a public record in your repository."
        }
        PublicActionKind::ModAction => {
            "Moderation actions are public records in your repository, with their reasons."
        }
    }
}

/// Shows the first-use interstitial of `kind` unless it was acknowledged, and records the
/// acknowledgement. On a terminal the user is asked; headless use proceeds after the note (a
/// script cannot answer, and refusing every headless vote would make automation impossible).
async fn acknowledge(ctx: &Ctx, w: &Writer, kind: PublicActionKind) -> Result<(), CliError> {
    if w.engine.public_action_acknowledged(kind).await? {
        return Ok(());
    }
    let text = public_notice(kind);
    if prompt::stdin_is_tty() {
        ctx.out.line(&[(tt(text), Style::Warn)]);
        if !prompt::confirm("continue?") {
            return Err(CliError::Reported(1));
        }
    } else {
        ctx.out.note(&[(lit(text), Style::Dim)]);
    }
    w.engine.acknowledge_public_action(kind).await?;
    Ok(())
}

/// Drives an action's write, prints its outcome and notices, and (in JSON mode) the
/// `ActionTicket` with its final state.
async fn drive_action(
    ctx: &Ctx,
    w: &Writer,
    at: ActionTicket,
    done: &'static str,
) -> Result<(), CliError> {
    let notices = at.notices.clone();
    let t = write::drive(ctx, w, at.ticket()).await?;
    if ctx.out.is_json() {
        // one JSON value: the ticket with its final state and the notices (report's
        // JSON is the bare WriteTicket, which would duplicate it)
        ctx.out.json(&dto(&ActionTicket {
            id: t.id.clone(),
            state: t.state.clone(),
            notices,
        }));
        if let Some(e) = write::needs_login_error(&t) {
            return Err(CliError::Engine(e));
        }
        if let WriteState::Failed { error } = &t.state {
            return Err(CliError::Engine(error.clone()));
        }
        return Ok(());
    }
    write::report(ctx, &t, done)?;
    for n in &notices {
        ctx.out.note(&[(tt(&n.message), Style::Dim)]);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// vote, agree, react

/// `lessdugin vote <ref> <value>`.
pub async fn vote(ctx: &Ctx, args: VoteArgs) -> Result<(), CliError> {
    opinion(
        ctx,
        &args.reference,
        OpinionIntent::Karma {
            value: args.value.into(),
        },
        "Voted",
    )
    .await
}

/// `lessdugin agree <ref> <value>`.
pub async fn agree(ctx: &Ctx, args: AgreeArgs) -> Result<(), CliError> {
    let value = Option::<lessdugin_core::dto::AgreementValue>::from(args.value);
    opinion(
        ctx,
        &args.reference,
        OpinionIntent::Agreement { value },
        "Recorded",
    )
    .await
}

/// `lessdugin react <ref> <react> [--quote …] [--against] [--remove]`.
pub async fn react(ctx: &Ctx, args: ReactArgs) -> Result<(), CliError> {
    if args.remove && args.against {
        return Err(CliError::usage("--remove and --against conflict"));
    }
    let quote = args.quote.map(|exact| TextQuote {
        exact,
        prefix: None,
        suffix: None,
        start: None,
    });
    opinion(
        ctx,
        &args.reference,
        OpinionIntent::React {
            id: args.react,
            stance: stance_of(args.against),
            quote,
            remove: args.remove,
        },
        "Reacted",
    )
    .await
}

/// One opinion on an item: the interstitial, a quiet thread sync (the subject's version is the
/// cached one, `engine-api.md` §3.5), the engine call and the write's report.
async fn opinion(
    ctx: &Ctx,
    reference: &str,
    intent: OpinionIntent,
    done: &'static str,
) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let (uri, _) = refs::post_or_comment(ctx, &w.engine, reference).await?;
    read::sync_thread_quietly(ctx, &w.engine, &uri).await;
    acknowledge(ctx, &w, intent.public_kind()).await?;
    let at = w.engine.opinion(&uri, &intent).await?;
    drive_action(ctx, &w, at, done).await
}

/// `lessdugin reacts`: the `default/1` palette (`social.md` §9.1).
pub async fn reacts(ctx: &Ctx) -> Result<(), CliError> {
    let reacts = palette::default_1();
    if ctx.out.is_json() {
        let v: Vec<Value> = reacts
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id, "glyph": r.glyph, "label": r.label,
                    "description": r.description, "group": r.group,
                })
            })
            .collect();
        ctx.out.json(&dto(&v));
        return Ok(());
    }
    for r in reacts {
        ctx.out.line(&[
            (tt(&r.glyph), Style::Plain),
            (lit(" "), Style::Plain),
            (name(&r.id), Style::Bold),
            (cat([lit("  "), tt(&r.label)]), Style::Plain),
        ]);
        ctx.out
            .line(&[(tt(&format!("    {}", r.description)), Style::Dim)]);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// notifications, activity

/// `lessdugin notifications [--all] [--mark-read]`.
pub async fn notifications(ctx: &Ctx, args: NotificationsArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let list = engine
        .notifications(&NotificationsQuery {
            all: args.all,
            limit: args.limit,
        })
        .await?;
    if args.mark_read && !list.items.is_empty() {
        let ids: Vec<String> = list.items.iter().map(|n| n.id.clone()).collect();
        engine.mark_notifications_read(&ids).await?;
    }
    if ctx.out.is_json() {
        ctx.out.json(&list);
        return Ok(());
    }
    if list.items.is_empty() {
        ctx.out.line(&[(
            lit(if args.all {
                "No notifications."
            } else {
                "Nothing new."
            }),
            Style::Dim,
        )]);
        return Ok(());
    }
    for n in &list.items {
        let who = n
            .actor
            .as_ref()
            .map(render::actor)
            .unwrap_or_else(|| lit("(you)"));
        ctx.out.line(&[
            (lit(if n.read { "  " } else { "• " }), Style::Plain),
            (tt(&format!("{:?}", n.reason)), Style::Bold),
            (lit(" "), Style::Plain),
            (who, Style::Plain),
            (cat([lit(" "), tt(&render::time(n.at_ms))]), Style::Dim),
        ]);
        if let Some(p) = &n.preview
            && !p.is_empty()
        {
            let first = p.lines().next().unwrap_or_default();
            ctx.out.line(&[(tt(&format!("    {first}")), Style::Dim)]);
        }
    }
    if list.unread > 0 {
        ctx.out.note(&[(
            cat([
                tt(&list.unread.to_string()),
                lit(" unread (`lessdugin notifications --mark-read` marks these read)"),
            ]),
            Style::Dim,
        )]);
    }
    Ok(())
}

/// `lessdugin activity [forum]`: Recent Discussion (`social.md` §12.4).
pub async fn activity(ctx: &Ctx, args: ActivityArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = refs::forum_or_current(ctx, &engine, args.forum.as_deref()).await?;
    read::sync_forum_quietly(ctx, &engine, &forum).await;
    let list = engine.recent_discussion(&forum, None, args.limit).await?;
    if ctx.out.is_json() {
        ctx.out.json(&list);
        return Ok(());
    }
    if list.items.is_empty() {
        ctx.out.line(&[(lit("No discussion yet."), Style::Dim)]);
        return Ok(());
    }
    for (i, d) in list.items.iter().enumerate() {
        ctx.out.line(&[
            (tt(&format!("{:>2}", i + 1)), Style::Bold),
            (lit("  "), Style::Plain),
            (
                tt(d.post.title.as_deref().unwrap_or("(untitled)")),
                Style::Bold,
            ),
        ]);
        for c in &d.comments {
            ctx.out.line(&[(
                tt(&format!(
                    "      {} {}",
                    render::actor(&c.author).to_plain_string(),
                    c.body.text.lines().next().unwrap_or_default()
                )),
                Style::Dim,
            )]);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// mods

/// `lessdugin mods add|rm <actor>`.
pub async fn mods(ctx: &Ctx, args: ModsArgs) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let forum = refs::forum_or_current(ctx, &w.engine, None).await?;
    acknowledge(ctx, &w, PublicActionKind::ModAction).await?;
    let change = match args.action {
        ModsAction::Add { actor, role } => ModsChange::Add {
            did: actor,
            role: role.into(),
            title: None,
        },
        ModsAction::Rm { actor, skip_adopt } => ModsChange::Remove {
            did: actor,
            skip_adopt,
        },
    };
    let at = w.engine.mods_set(&forum, &change).await?;
    drive_action(
        ctx,
        &w,
        at,
        match change {
            ModsChange::Add { .. } => "Added",
            ModsChange::Remove { .. } => "Removed",
        },
    )
    .await
}

// ---------------------------------------------------------------------------------------------
// mod

/// The forum a mod action acts in: `--forum`, else the subject item's forum, else the current
/// forum.
async fn mod_forum(
    ctx: &Ctx,
    engine: &lessdugin_core::engine::Engine<NativeHost>,
    explicit: Option<&str>,
    subject: &str,
) -> Result<String, CliError> {
    if let Some(f) = explicit {
        return refs::forum(ctx, engine, f).await;
    }
    // an item subject carries its forum in the cached record
    if let Ok(uri) = item_uri(ctx, engine, subject).await
        && let Ok((_, v)) = write::cached(engine, &uri).await
        && let Some(f) = v.get("forum").and_then(Value::as_str)
    {
        return Ok(f.to_owned());
    }
    refs::forum_or_current(ctx, engine, None).await
}

/// Resolves a mod-action subject to what the engine takes: an item ref becomes its at-uri;
/// a DID, a handle or `tag:<slug>` is passed through.
async fn subject_of(
    ctx: &Ctx,
    engine: &lessdugin_core::engine::Engine<NativeHost>,
    subject: &str,
) -> Result<String, CliError> {
    match item_uri(ctx, engine, subject).await {
        Ok(uri) => Ok(uri),
        Err(_) if subject.starts_with("did:") || subject.starts_with("tag:") => {
            Ok(subject.to_owned())
        }
        // a bare handle: the engine's mod_subject resolves it
        Err(_) if !subject.contains('/') => Ok(subject.to_owned()),
        Err(e) => Err(e),
    }
}

/// An item reference as its at-uri, when it is one.
async fn item_uri(
    ctx: &Ctx,
    engine: &lessdugin_core::engine::Engine<NativeHost>,
    subject: &str,
) -> Result<String, CliError> {
    match refs::item(ctx, engine, subject).await? {
        lessdugin_core::dto::Ref::Post { uri } | lessdugin_core::dto::Ref::Comment { uri } => {
            Ok(uri)
        }
        _ => Err(CliError::Engine(EngineError::invalid_ref(format!(
            "{subject:?} is not a post or a comment"
        )))),
    }
}

/// `--until` as epoch milliseconds.
fn until_ms(until: Option<&str>) -> Result<Option<i64>, CliError> {
    until
        .map(|s| {
            Datetime::parse(s)
                .map(|d| d.timestamp_micros() / 1000)
                .map_err(|e| CliError::usage(format!("--until {s:?}: {e}")))
        })
        .transpose()
}

/// One mod action: the interstitial, the intent and the write's report.
async fn mod_action(
    ctx: &Ctx,
    w: &Writer,
    intent: ModActionIntent,
    done: &'static str,
) -> Result<(), CliError> {
    acknowledge(ctx, w, PublicActionKind::ModAction).await?;
    let at = w.engine.mod_action(&intent).await?;
    drive_action(ctx, w, at, done).await
}

fn assert(
    forum: String,
    scope: ModScope,
    subject: String,
    action: ModActionValue,
    reason: Option<String>,
    until_ms: Option<i64>,
) -> ModActionIntent {
    ModActionIntent::Assert {
        forum,
        scope,
        subject,
        action,
        reason,
        until_ms,
    }
}

fn remove(forum: String, scope: ModScope, subject: String, action_key: &str) -> ModActionIntent {
    ModActionIntent::Remove {
        forum,
        scope,
        subject,
        action_key: action_key.to_owned(),
    }
}

/// `lessdugin mod <action> …` (`social.md` §4).
pub async fn run(ctx: &Ctx, args: ModArgs) -> Result<(), CliError> {
    match args.action {
        ModAction::Queue { forum } => queue(ctx, forum.as_deref()).await,
        ModAction::Log { forum, limit } => log(ctx, forum.as_deref(), limit).await,
        ModAction::Adopt { actor, forum } => adopt(ctx, &actor, forum.as_deref()).await,
        action => assert_or_remove(ctx, action).await,
    }
}

/// `mod queue`.
async fn queue(ctx: &Ctx, forum: Option<&str>) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = refs::forum_or_current(ctx, &engine, forum).await?;
    read::sync_forum_quietly(ctx, &engine, &forum).await;
    let q = engine.mod_queue(&forum).await?;
    if ctx.out.is_json() {
        ctx.out.json(&q);
        return Ok(());
    }
    if q.items.is_empty() {
        ctx.out.line(&[(lit("The queue is empty."), Style::Dim)]);
        return Ok(());
    }
    for item in &q.items {
        ctx.out.line(&[
            (tt(&item.item_kind), Style::Bold),
            (lit("  "), Style::Plain),
            (
                item.actor
                    .as_ref()
                    .map(render::actor)
                    .unwrap_or_else(|| tt(&item.subject)),
                Style::Plain,
            ),
            (cat([lit("  "), tt(&item.reason)]), Style::Dim),
        ]);
        if let Some(d) = &item.detail {
            ctx.out.line(&[(tt(&format!("    {d}")), Style::Dim)]);
        }
    }
    Ok(())
}

/// `mod log`.
async fn log(ctx: &Ctx, forum: Option<&str>, limit: u32) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    let forum = refs::forum_or_current(ctx, &engine, forum).await?;
    read::sync_forum_quietly(ctx, &engine, &forum).await;
    let log = engine.mod_log(&forum, None, limit).await?;
    if ctx.out.is_json() {
        ctx.out.json(&log);
        return Ok(());
    }
    if log.entries.is_empty() {
        ctx.out.line(&[(lit("No moderation yet."), Style::Dim)]);
        return Ok(());
    }
    for e in &log.entries {
        let mut parts = vec![
            tt(&render::time(e.at_ms)),
            render::actor(&e.actor),
            tt(&e.action),
            tt(&e.subject),
        ];
        if let Some(r) = &e.reason {
            parts.push(tt(r));
        }
        parts.push(tt(&format!("{:?}", e.status).to_lowercase()));
        ctx.out.line(&[(cat(intersperse(parts)), Style::Plain)]);
    }
    Ok(())
}

fn intersperse(parts: Vec<crate::out::TermText>) -> Vec<crate::out::TermText> {
    let mut out = Vec::with_capacity(parts.len() * 2);
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.push(lit(" · "));
        }
        out.push(p);
    }
    out
}

/// `mod adopt <former-mod>`.
async fn adopt(ctx: &Ctx, actor: &str, forum: Option<&str>) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let forum = refs::forum_or_current(ctx, &w.engine, forum).await?;
    acknowledge(ctx, &w, PublicActionKind::ModAction).await?;
    let ticket = w.engine.mod_adopt(&forum, actor).await?;
    let t = write::drive(ctx, &w, ticket).await?;
    write::report(ctx, &t, "Adopted")
}

/// Every `mod <verb> <subject>` that asserts or removes one action.
async fn assert_or_remove(ctx: &Ctx, action: ModAction) -> Result<(), CliError> {
    let w = ctx.writer().await?;
    let (subject, forum_opt, scope, value, reason, until, done) = match action {
        ModAction::Hide(a) => (
            a.subject.clone(),
            a.forum.clone(),
            scope_of(a.author_scope),
            ModActionValue::Hide {
                hidden: true,
                spam: a.spam,
                show_reason: true,
            },
            a.reason.clone(),
            until_ms(a.until.as_deref())?,
            "Hidden",
        ),
        ModAction::Keep(a) => (
            a.subject.clone(),
            a.forum.clone(),
            scope_of(a.author_scope),
            ModActionValue::Hide {
                hidden: false,
                spam: false,
                show_reason: true,
            },
            a.reason.clone(),
            until_ms(a.until.as_deref())?,
            "Kept",
        ),
        ModAction::Ban(a) => (
            a.subject.clone(),
            a.forum.clone(),
            scope_of(a.author_scope),
            ModActionValue::Access {
                level: lessdugin_core::dto::AccessLevel::Banned,
            },
            a.reason.clone(),
            until_ms(a.until.as_deref())?,
            "Banned",
        ),
        ModAction::Restrict(a) => (
            a.subject.clone(),
            a.forum.clone(),
            scope_of(a.author_scope),
            ModActionValue::Restrict {
                capabilities: if a.capabilities.is_empty() {
                    vec![
                        Capability::Post,
                        Capability::Comment,
                        Capability::Vote,
                        Capability::React,
                        Capability::Tag,
                        Capability::Wiki,
                        Capability::Sequence,
                    ]
                } else {
                    a.capabilities.iter().map(|c| (*c).into()).collect()
                },
                nullify_votes: false,
            },
            a.reason.clone(),
            until_ms(a.until.as_deref())?,
            "Restricted",
        ),
        ModAction::Nullify(a) => (
            a.subject.clone(),
            a.forum.clone(),
            scope_of(a.author_scope),
            ModActionValue::Restrict {
                capabilities: Vec::new(),
                nullify_votes: true,
            },
            a.reason.clone(),
            until_ms(a.until.as_deref())?,
            "Votes nullified",
        ),
        ModAction::Personal(s) => {
            return tier(ctx, &w, s, Tier::Personal, "Moved to personal").await;
        }
        ModAction::Frontpage(s) => {
            return tier(ctx, &w, s, Tier::Frontpage, "Moved to frontpage").await;
        }
        ModAction::Curate(s) => {
            return tier(ctx, &w, s, Tier::Curated, "Curated").await;
        }
        ModAction::Pin(a) => {
            return pin(ctx, &w, a, true).await;
        }
        ModAction::Unpin(a) => {
            return pin(ctx, &w, a, false).await;
        }
        ModAction::Lock(s) => (
            s.subject.clone(),
            s.forum.clone(),
            ModScope::Forum,
            ModActionValue::Lock { locked: true },
            None,
            None,
            "Locked",
        ),
        ModAction::Unlock(s) => (
            s.subject.clone(),
            s.forum.clone(),
            ModScope::Forum,
            ModActionValue::Lock { locked: false },
            None,
            None,
            "Unlocked",
        ),
        ModAction::Unhide(s) => {
            return removal(ctx, &w, s, "hide", "Unhidden").await;
        }
        ModAction::Unban(s) => {
            return removal(ctx, &w, s, "access", "Unbanned").await;
        }
        ModAction::Unrestrict(s) => {
            return removal(ctx, &w, s, "restrict", "Unrestricted").await;
        }
        ModAction::Approve(s) => {
            let forum = refs::forum_or_current(ctx, &w.engine, s.forum.as_deref()).await?;
            let tickets = w
                .engine
                .grant(&forum, std::slice::from_ref(&s.subject))
                .await?;
            let mut out = Vec::new();
            for t in tickets {
                out.push(write::drive(ctx, &w, t).await?);
            }
            if ctx.out.is_json() {
                // one JSON value, like `approve`
                let failure = out.iter().find_map(|t| match &t.state {
                    WriteState::Failed { error } => Some(error.clone()),
                    _ => None,
                });
                ctx.out.json(&serde_json::json!({
                    "dto": lessdugin_core::dto::DTO_VERSION,
                    "writes": out,
                }));
                if let Some(e) = failure {
                    return Err(CliError::Engine(e));
                }
                return Ok(());
            }
            for t in &out {
                write::report(ctx, t, "Approved")?;
            }
            return Ok(());
        }
        ModAction::Queue { .. } | ModAction::Log { .. } | ModAction::Adopt { .. } => {
            unreachable!("handled in run()")
        }
    };
    let forum = mod_forum(ctx, &w.engine, forum_opt.as_deref(), &subject).await?;
    let subject = subject_of(ctx, &w.engine, &subject).await?;
    read::sync_thread_quietly(ctx, &w.engine, &subject).await;
    mod_action(
        ctx,
        &w,
        assert(forum, scope, subject, value, reason, until),
        done,
    )
    .await
}

/// A tier action (`mod personal|frontpage|curate`): an assertion on the item.
async fn tier(
    ctx: &Ctx,
    w: &Writer,
    s: ModSubject,
    tier: Tier,
    done: &'static str,
) -> Result<(), CliError> {
    let forum = mod_forum(ctx, &w.engine, s.forum.as_deref(), &s.subject).await?;
    let subject = subject_of(ctx, &w.engine, &s.subject).await?;
    read::sync_thread_quietly(ctx, &w.engine, &subject).await;
    mod_action(
        ctx,
        w,
        assert(
            forum,
            ModScope::Forum,
            subject,
            ModActionValue::Tier { tier, notice: None },
            None,
            None,
        ),
        done,
    )
    .await
}

/// `mod pin|unpin`.
async fn pin(ctx: &Ctx, w: &Writer, a: PinArgs, pinned: bool) -> Result<(), CliError> {
    let forum = mod_forum(ctx, &w.engine, a.forum.as_deref(), &a.subject).await?;
    let subject = subject_of(ctx, &w.engine, &a.subject).await?;
    read::sync_thread_quietly(ctx, &w.engine, &subject).await;
    mod_action(
        ctx,
        w,
        assert(
            forum,
            ModScope::Forum,
            subject,
            ModActionValue::Pin {
                scope: a.scope.into(),
                pinned,
            },
            a.reason.clone(),
            None,
        ),
        if pinned { "Pinned" } else { "Unpinned" },
    )
    .await
}

/// A removal of one of the account's assertions (`mod unhide|unban|unrestrict`).
async fn removal(
    ctx: &Ctx,
    w: &Writer,
    s: ModSubject,
    action_key: &str,
    done: &'static str,
) -> Result<(), CliError> {
    let forum = mod_forum(ctx, &w.engine, s.forum.as_deref(), &s.subject).await?;
    let subject = subject_of(ctx, &w.engine, &s.subject).await?;
    mod_action(
        ctx,
        w,
        remove(forum, ModScope::Forum, subject, action_key),
        done,
    )
    .await
}

/// `export`: this device's local state as JSON, or `--import` to merge one back
/// (`engine-api.md` §3.6, `synthesis-v1.md` §6.10, `threat-model.md` PRIV-8/PRIV-10).
///
/// `--mine` is the default: the active account's state plus the rows that belong to no account.
/// An import merges, so a partial or older export never destroys state it does not mention.
pub async fn export(ctx: &Ctx, args: ExportArgs) -> Result<(), CliError> {
    let engine = ctx.reader().await?;
    if let Some(path) = args.import {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| CliError::usage(format!("could not read {}: {e}", path.display())))?;
        let parsed: lessdugin_core::engine::LocalExport = serde_json::from_str(&text)
            .map_err(|e| CliError::usage(format!("{} is not an export: {e}", path.display())))?;
        let scope = scope_of_export(args.scope);
        let report = engine.import_local(&parsed, scope).await?;
        if ctx.out.is_json() {
            ctx.out.json(&report);
        } else {
            ctx.out.line(&[(
                crate::out::tt(&format!(
                    "Imported {} rows ({} out of scope, {} unreadable).",
                    report.imported, report.out_of_scope, report.undecodable
                )),
                crate::out::Style::Plain,
            )]);
        }
        return Ok(());
    }
    let scope = scope_of_export(args.scope);
    let x = engine.export_local(scope).await?;
    match args.out {
        Some(path) => {
            let text = serde_json::to_string_pretty(&x)
                .map_err(|e| CliError::usage(format!("could not serialise the export: {e}")))?;
            std::fs::write(&path, text)
                .map_err(|e| CliError::usage(format!("could not write {}: {e}", path.display())))?;
            if !ctx.out.is_quiet() {
                ctx.out.line(&[(
                    crate::out::tt(&format!(
                        "Wrote {} rows to {}.",
                        x.members.len(),
                        path.display()
                    )),
                    crate::out::Style::Plain,
                )]);
            }
        }
        None => ctx.out.json(&x),
    }
    Ok(())
}

fn scope_of_export(a: ExportScopeArg) -> lessdugin_core::engine::ExportScope {
    match a {
        ExportScopeArg::Mine => lessdugin_core::engine::ExportScope::Mine,
        ExportScopeArg::Everything => lessdugin_core::engine::ExportScope::Everything,
    }
}
