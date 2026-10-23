// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin` — command line entry point (`docs/design/synthesis-v1.md` §6.10).
//!
//! Exit codes (`engine-api.md` §1): 0 ok, 1 error, 2 usage, 3 `auth_required`, 4 `not_found`,
//! 5 `conflict`, 6 `partial`.

#![forbid(unsafe_code)]

mod args;
mod cmd;
mod ctx;
mod logging;
mod out;
mod prompt;
mod render;
mod state;
#[cfg(feature = "web")]
mod web;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::args::{Cli, Command};
use crate::ctx::{CliError, Ctx};
use crate::out::Printer;

/// Exit code of usage errors.
const EXIT_USAGE: u8 = 2;

async fn run(ctx: &Ctx, command: Command) -> Result<(), CliError> {
    match command {
        Command::Read(a) => cmd::read::read(ctx, a).await,
        Command::Ls(a) => cmd::read::ls(ctx, a).await,
        Command::Post(a) => cmd::write::post(ctx, a).await,
        Command::Comment(a) => cmd::write::comment(ctx, a).await,
        Command::Edit(a) => cmd::write::edit(ctx, a).await,
        Command::Delete(a) => cmd::write::delete(ctx, a).await,
        Command::Retract(a) => cmd::write::retract(ctx, a).await,
        Command::History(a) => cmd::read::history(ctx, a).await,
        Command::Sync(a) => cmd::forum::sync(ctx, a).await,
        Command::Join(a) => cmd::forum::join(ctx, a).await,
        Command::Leave(a) => cmd::forum::leave(ctx, a).await,
        Command::Forum(a) => cmd::forum::forum(ctx, a).await,
        Command::Members(a) => cmd::forum::members(ctx, a).await,
        Command::Approve(a) => cmd::forum::approve(ctx, a).await,
        Command::Draft(a) => cmd::draft::run(ctx, a).await,
        Command::Outbox(a) => cmd::outbox::run(ctx, a).await,
        Command::Login(a) => cmd::login::run(ctx, a).await,
        Command::Logout(a) => cmd::session::logout(ctx, a).await,
        Command::Whoami => cmd::session::whoami(ctx).await,
        Command::Accounts(a) => cmd::session::accounts(ctx, a).await,
        Command::Resolve(a) => cmd::identity::resolve(ctx, a).await,
        Command::Verify(a) => cmd::identity::verify(ctx, a).await,
        Command::Doctor(a) => cmd::doctor::run(ctx, a).await,
        #[cfg(feature = "web")]
        Command::Web(a) => web::run(ctx, a).await,
        #[cfg(feature = "tui")]
        Command::Tui(a) => tui(ctx, a).await,
        Command::Completions(a) => {
            let mut command = Cli::command();
            clap_complete::generate(a.shell, &mut command, "lessdugin", &mut std::io::stdout());
            Ok(())
        }
    }
}

/// True when stdin and stdout are terminals.
fn on_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Bare `lessdugin` on a terminal: the TUI when an account is logged in, else help.
async fn bare(ctx: &Ctx) -> Result<(), CliError> {
    #[cfg(feature = "tui")]
    if !ctx.engine()?.accounts().await?.is_empty() {
        return tui(ctx, args::TuiArgs::default()).await;
    }
    let _ = ctx;
    let _ = Cli::command().print_help();
    Ok(())
}

/// `lessdugin tui [forum]` (`crates/tui`): the engine this command opened, acting as the
/// account (`--account` or the active one) with its session restored, or read-only when no
/// account is logged in.
#[cfg(feature = "tui")]
async fn tui(ctx: &Ctx, a: args::TuiArgs) -> Result<(), CliError> {
    use lessdugin_core::engine::{EngineError, ErrorCode};
    let (engine, account) = match ctx.writer().await {
        Ok(w) => (w.engine, Some(w.account)),
        Err(CliError::Engine(e))
            if e.code == ErrorCode::AuthRequired && ctx.g.account.is_none() =>
        {
            ctx.out.warn(out::cat([
                out::tt(&e.message),
                out::lit(": the TUI is read-only"),
            ]));
            (ctx.reader().await?, None)
        }
        Err(e) => return Err(e),
    };
    let forum = match a.forum.as_deref().or(ctx.g.forum.as_deref()) {
        Some(f) => Some(cmd::refs::forum(ctx, &engine, f).await?),
        None => None,
    };
    let opts = lessdugin_tui::Options {
        forum,
        account,
        offline: ctx.offline(),
        // template files for $EDITOR: private, under the data directory (TERM-9, FILE-6)
        editor: lessdugin_tui::EditorConfig::from_env(ctx.paths.data_dir.join("tmp")),
    };
    let report = lessdugin_tui::run(engine, opts)
        .await
        .map_err(|e| CliError::Engine(EngineError::internal(e.to_string())))?;
    if report.queued_writes > 0 {
        ctx.out.note(&[(
            out::lit(
                "Some writes are still queued; the next command that writes, `lessdugin sync` \
                 or `lessdugin outbox flush` sends them.",
            ),
            out::Style::Dim,
        )]);
    }
    if report.failed_writes > 0 {
        ctx.out.note(&[(
            out::lit(
                "Some writes failed: `lessdugin outbox` lists them, and `lessdugin outbox retry \
                 <id>` sends one again.",
            ),
            out::Style::Dim,
        )]);
    }
    Ok(())
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn report(printer: &Printer, e: CliError) -> ExitCode {
    match e {
        CliError::Engine(e) => {
            printer.error(&e);
            exit_code(e.exit_code())
        }
        CliError::Usage(msg) => {
            printer.usage_error(&msg);
            ExitCode::from(EXIT_USAGE)
        }
        CliError::Reported(code) => exit_code(code),
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            use clap::error::ErrorKind;
            match e.kind() {
                ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    let _ = e.print();
                }
                // Parse errors echo the raw arguments: sanitise them, never show a password.
                _ => {
                    use std::io::Write;
                    let text = out::clap_error_text(&e.render().to_string());
                    let _ = std::io::stderr().lock().write_all(text.as_bytes());
                }
            }
            return exit_code(e.exit_code());
        }
    };
    // Bare invocation: the TUI on a terminal with an account (`bare`), otherwise help.
    let command = match cli.command {
        Some(c) => Some(c),
        None if cfg!(feature = "tui") && on_terminal() => None,
        None => {
            let _ = Cli::command().print_help();
            return ExitCode::SUCCESS;
        }
    };
    // The TUI owns the terminal: log lines on stderr would draw over it, so it runs without
    // the stderr subscriber.
    #[cfg(feature = "tui")]
    let tui_owns_terminal = matches!(command, None | Some(Command::Tui(_)));
    #[cfg(not(feature = "tui"))]
    let tui_owns_terminal = false;
    if !tui_owns_terminal {
        logging::init(cli.global.verbose, cli.global.quiet);
    }
    let printer = Printer::new(cli.global.json, cli.global.color, cli.global.quiet);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            printer.usage_error(&format!("cannot start the runtime: {e}"));
            return ExitCode::from(1);
        }
    };
    let result = runtime.block_on(async {
        let ctx = Ctx::new(cli.global)?;
        match command {
            Some(c) => run(&ctx, c).await,
            None => bare(&ctx).await,
        }
    });
    // Do not wait for blocking tasks (a pending keyring or stdin read) at exit.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => report(&printer, e),
    }
}
