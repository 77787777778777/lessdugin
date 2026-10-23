// SPDX-License-Identifier: AGPL-3.0-or-later
//! `outbox [ls|flush|retry|undo]`: the durable write queue (`engine-api.md` §3.5).
//!
//! Every writing command sends its account's queued writes before it reports; `outbox flush`
//! does only that (after a crash, a `kill -9` or `--offline` writes) and is "send now": a write
//! that backs off after a failure is tried at once (`outbox_expedite`; the undo window, the
//! write budget and a PDS rate limit still apply). `retry` queues a failed write again, or
//! sends a backing-off one now; `undo` removes a write not sent yet. The listing says why a
//! pending write waits.

use lessdugin_core::dto::{DTO_VERSION, WriteState, WriteTicket};
use lessdugin_core::engine::EngineError;
use lessdugin_core::outbox::parse_write_id;
use serde::Serialize;

use crate::args::{OutboxAction, OutboxArgs};
use crate::cmd::write;
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, TermText, cat, lit, tt};
use crate::render;

#[derive(Serialize)]
struct WriteList<'a> {
    dto: u32,
    writes: &'a [WriteTicket],
}

fn state_text(s: &WriteState) -> (TermText, Style) {
    let cause = |e: &Option<EngineError>| match e {
        Some(e) => cat([
            lit(" ("),
            tt(e.code.as_str()),
            lit(": "),
            tt(&lessdugin_native::auth::redact::redact_secrets(&e.message)),
            lit(")"),
        ]),
        None => lit(""),
    };
    match s {
        WriteState::Pending {
            needs_login: true,
            error,
            ..
        } => (
            cat([
                lit("waiting for `lessdugin login` (the session was refused)"),
                cause(error),
            ]),
            Style::Bad,
        ),
        WriteState::Pending {
            not_before_ms: Some(at),
            held,
            error,
            ..
        } => (
            cat([
                lit(if *held {
                    "pending (undo window) until "
                } else {
                    "pending until "
                }),
                tt(&render::time(*at)),
                cause(error),
            ]),
            Style::Warn,
        ),
        WriteState::Pending { error, .. } => (cat([lit("pending"), cause(error)]), Style::Warn),
        WriteState::Sending => (lit("sending"), Style::Warn),
        WriteState::Done { uri, .. } => (cat([lit("done "), tt(uri)]), Style::Good),
        WriteState::Failed { error } => (
            cat([
                lit("failed: "),
                tt(error.code.as_str()),
                lit(": "),
                tt(&error.message),
            ]),
            Style::Bad,
        ),
        WriteState::Undone => (lit("undone"), Style::Dim),
    }
}

fn account_of(id: &str) -> Result<String, CliError> {
    parse_write_id(id).map(|(a, _)| a).ok_or_else(|| {
        CliError::Engine(EngineError::invalid_input(format!(
            "{id:?} is not a write id"
        )))
    })
}

/// `lessdugin outbox …`.
pub async fn run(ctx: &Ctx, args: OutboxArgs) -> Result<(), CliError> {
    match args.action.unwrap_or(OutboxAction::Ls) {
        OutboxAction::Ls => {
            let engine = ctx.reader().await?;
            let list = engine.writes().await?;
            if ctx.out.is_json() {
                ctx.out.json(&WriteList {
                    dto: DTO_VERSION,
                    writes: &list,
                });
                return Ok(());
            }
            if list.is_empty() {
                ctx.out.line(&[(lit("The outbox is empty."), Style::Dim)]);
            }
            for t in &list {
                let (s, st) = state_text(&t.state);
                ctx.out
                    .line(&[(tt(&t.id), Style::Bold), (lit("  "), Style::Plain), (s, st)]);
            }
            Ok(())
        }
        OutboxAction::Flush => {
            if ctx.offline() {
                return Err(CliError::usage("flush needs the network; drop --offline"));
            }
            let w = ctx.writer().await?;
            // "send now": back-offs end; holds that are not ours to shorten stay
            w.engine.outbox().expedite(&w.account.did).await?;
            let run = write::drive_all(ctx, &w).await?;
            let list: Vec<WriteTicket> = w
                .engine
                .outbox()
                .writes(&w.account.did)
                .await?
                .iter()
                .map(|p| p.ticket())
                .collect();
            if ctx.out.is_json() {
                ctx.out.json(&WriteList {
                    dto: DTO_VERSION,
                    writes: &list,
                });
            } else {
                let n = u32::try_from(run.len()).unwrap_or(u32::MAX);
                ctx.out.line(&[(
                    cat([lit("Sent "), render::count(n, "write", "writes")]),
                    Style::Plain,
                )]);
                for t in &run {
                    let (s, st) = state_text(&t.state);
                    ctx.out.line(&[
                        (lit("  "), Style::Plain),
                        (tt(&t.id), Style::Bold),
                        (lit("  "), Style::Plain),
                        (s, st),
                    ]);
                }
            }
            let queued = list
                .iter()
                .any(|t| matches!(t.state, WriteState::Pending { .. } | WriteState::Sending));
            if queued && !ctx.out.is_json() {
                ctx.out.note(&[(
                    lit("Some writes are still queued (a rate limit or a back-off); run `lessdugin outbox` to see them."),
                    Style::Dim,
                )]);
            }
            match write::first_failure(&run) {
                Some(e) => Err(CliError::Engine(e)),
                None => Ok(()),
            }
        }
        OutboxAction::Retry { id } => {
            let w = ctx.writer_for(&account_of(&id)?).await?;
            let ticket = w.engine.retry_write(&id).await?;
            let t = write::drive(ctx, &w, ticket).await?;
            write::report(ctx, &t, "Sent")
        }
        OutboxAction::Undo { id } => {
            let w = ctx.writer_for(&account_of(&id)?).await?;
            let t = w.engine.undo(&id).await?;
            write::report(ctx, &t, "Undone")
        }
    }
}
