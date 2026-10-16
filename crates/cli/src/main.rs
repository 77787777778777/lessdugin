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

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use crate::args::{Cli, Command};
use crate::ctx::{CliError, Ctx};
use crate::out::Printer;

/// Exit code of usage errors.
const EXIT_USAGE: u8 = 2;

async fn run(ctx: &Ctx, command: Command) -> Result<(), CliError> {
    match command {
        Command::Login(a) => cmd::login::run(ctx, a).await,
        Command::Logout(a) => cmd::session::logout(ctx, a).await,
        Command::Whoami => cmd::session::whoami(ctx).await,
        Command::Accounts(a) => cmd::session::accounts(ctx, a).await,
        Command::Resolve(a) => cmd::identity::resolve(ctx, a).await,
        Command::Verify(a) => cmd::identity::verify(ctx, a).await,
        Command::Doctor(a) => cmd::doctor::run(ctx, a).await,
        Command::Completions(a) => {
            let mut command = Cli::command();
            clap_complete::generate(a.shell, &mut command, "lessdugin", &mut std::io::stdout());
            Ok(())
        }
    }
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
    logging::init(cli.global.verbose, cli.global.quiet);
    let printer = Printer::new(cli.global.json, cli.global.color, cli.global.quiet);
    let Some(command) = cli.command else {
        // Bare invocation: help (the TUI arrives in M2).
        let _ = Cli::command().print_help();
        return ExitCode::SUCCESS;
    };
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
        run(&ctx, command).await
    });
    // Do not wait for blocking tasks (a pending keyring or stdin read) at exit.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => report(&printer, e),
    }
}
