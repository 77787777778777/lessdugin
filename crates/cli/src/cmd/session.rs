// SPDX-License-Identifier: AGPL-3.0-or-later
//! `logout`, `whoami` and `accounts` (`threat-model.md` AUTH-11).

use lessdugin_atp::syntax::Did;
use lessdugin_atp::xrpc::GetSession;
use lessdugin_core::dto::{AccountView, AuthKind, DTO_VERSION};
use lessdugin_core::engine::EngineError;
use lessdugin_core::limits;
use lessdugin_core::xrpc::{CallOpts, Service};
use lessdugin_native::auth::logout_session;
use serde::Serialize;

use crate::args::{AccountsAction, AccountsArgs, LogoutArgs};
use crate::ctx::{CliError, Ctx, find_account};
use crate::out::{Style, cat, lit, name, tt};
use crate::prompt;

fn who(a: &AccountView) -> crate::out::TermText {
    match &a.handle {
        Some(h) => cat([lit("@"), name(h), lit(" ("), tt(&a.did), lit(")")]),
        None => tt(&a.did),
    }
}

fn auth_label(kind: AuthKind) -> &'static str {
    match kind {
        AuthKind::Oauth => "oauth",
        AuthKind::AppPassword => "app password",
    }
}

/// The `logout --json` output.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogoutView {
    dto: u32,
    did: String,
    revoked: bool,
    purged: bool,
}

/// `lessdugin logout [--purge]`.
pub async fn logout(ctx: &Ctx, args: LogoutArgs) -> Result<(), CliError> {
    let account = ctx.account().await?;
    if args.purge && !args.yes {
        if !prompt::stdin_is_tty() {
            return Err(CliError::usage(
                "--purge deletes local state; add --yes when not running interactively",
            ));
        }
        ctx.out.err_line(&[(
            cat([
                lit("--purge also deletes the local state of "),
                who(&account),
                lit(": its outbox (unsent writes), write budget and notification state."),
            ]),
            Style::Warn,
        )]);
        if !prompt::confirm("Delete it?") {
            return Err(CliError::Reported(1));
        }
    }
    let engine = ctx.engine()?;
    let report = logout_session(engine.host(), &ctx.store(), &account.did).await?;
    engine.remove_account(&account.did, args.purge).await?;
    if ctx.out.is_json() {
        ctx.out.json(&LogoutView {
            dto: DTO_VERSION,
            did: account.did.clone(),
            revoked: report.revoked,
            purged: args.purge,
        });
        return Ok(());
    }
    ctx.out.line(&[
        (lit("Logged out of "), Style::Plain),
        (who(&account), Style::Bold),
    ]);
    if let Some(e) = &report.revoke_error {
        ctx.out.warn(cat([
            lit("the server did not confirm the revocation ("),
            tt(e),
            lit("); the local session was deleted"),
        ]));
    } else if !report.had_session {
        ctx.out
            .note(&[(lit("(no stored session was found)"), Style::Dim)]);
    }
    if args.purge {
        ctx.out
            .note(&[(lit("Local state of the account deleted."), Style::Dim)]);
    }
    Ok(())
}

/// `lessdugin whoami`: the account, after checking its session with `getSession`.
pub async fn whoami(ctx: &Ctx) -> Result<(), CliError> {
    let account = ctx.account_with_session().await?;
    let engine = ctx.engine()?;
    let did = Did::parse(&account.did).map_err(|e| EngineError::internal(e.to_string()))?;
    let session = engine
        .xrpc()
        .query_endpoint(
            &Service::UserPds(did),
            &GetSession,
            &CallOpts::capped(limits::SMALL_RESPONSE_MAX_BYTES),
        )
        .await
        .map_err(EngineError::from)?;
    if session.did != account.did {
        return Err(EngineError::conflict(format!(
            "the session belongs to {}, not {}",
            session.did, account.did
        ))
        .into());
    }
    if ctx.out.is_json() {
        ctx.out.json(&account);
        return Ok(());
    }
    ctx.out.line(&[(who(&account), Style::Bold)]);
    ctx.out.field("pds", 7, tt(&account.pds));
    ctx.out.field("auth", 7, lit(auth_label(account.auth_kind)));
    ctx.out.field(
        "session",
        7,
        cat([
            lit("valid (handle at the PDS: "),
            name(&session.handle),
            lit(")"),
        ]),
    );
    Ok(())
}

/// `lessdugin accounts [use <account>]`.
pub async fn accounts(ctx: &Ctx, args: AccountsArgs) -> Result<(), CliError> {
    let engine = ctx.engine()?;
    if let Some(AccountsAction::Use { account }) = args.action {
        let a = find_account(engine, &account).await?;
        engine.set_active_account(&a.did).await?;
        let a = engine.account(&a.did).await?.unwrap_or(a);
        if ctx.out.is_json() {
            ctx.out.json(&a);
        } else {
            ctx.out.line(&[
                (lit("Active account: "), Style::Plain),
                (who(&a), Style::Bold),
            ]);
        }
        return Ok(());
    }
    let list = engine.accounts().await?;
    if ctx.out.is_json() {
        ctx.out.json(&list);
        return Ok(());
    }
    if list.is_empty() {
        ctx.out.note(&[(
            lit("No accounts. Log in with `lessdugin login <handle>`."),
            Style::Dim,
        )]);
        return Ok(());
    }
    for a in &list {
        let marker = if a.active { lit("* ") } else { lit("  ") };
        ctx.out.line(&[
            (marker, Style::Good),
            (who(a), if a.active { Style::Bold } else { Style::Plain }),
            (
                cat([
                    lit("  "),
                    tt(&a.pds),
                    lit("  "),
                    lit(auth_label(a.auth_kind)),
                ]),
                Style::Dim,
            ),
        ]);
    }
    Ok(())
}
