// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin login` (`docs/design/synthesis-v1.md` §6.7, `threat-model.md` AUTH-1…AUTH-5).

use std::time::Duration;

use lessdugin_core::engine::{EngineError, ErrorCode, PinStatus, is_app_password_shape};
use lessdugin_native::auth::oauth::{self, ClientKind, OAuthOptions};
use lessdugin_native::auth::{AppPassword, Backend, LoginOutcome, login_app_password};

use crate::args::{LoginArgs, OAuthClientArg};
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, TermText, cat, lit, name, tt};
use crate::prompt;

/// How long the loopback listener waits for the browser.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

fn argv_password_refusal() -> CliError {
    CliError::usage(
        "passwords are never accepted as command-line arguments (they leak through shell \
         history and the process list). If that was a real app password, revoke it in your \
         PDS's settings. Use the prompt (`lessdugin login <handle> --app-password`), \
         --password-stdin, or LESSDUGIN_APP_PASSWORD.",
    )
}

/// The app password, from stdin, the environment or a no-echo prompt (never argv).
fn read_app_password(args: &LoginArgs) -> Result<AppPassword, CliError> {
    if args.password_stdin {
        return Ok(AppPassword::from_reader(std::io::stdin().lock())?);
    }
    if let Some(p) = AppPassword::from_env() {
        return Ok(p?);
    }
    if !prompt::stdin_is_tty() {
        return Err(CliError::usage(
            "no terminal to prompt for the app password: use --password-stdin or \
             LESSDUGIN_APP_PASSWORD",
        ));
    }
    let secret = prompt::read_secret("App password (xxxx-xxxx-xxxx-xxxx): ")
        .map_err(|e| EngineError::invalid_input(format!("no app password: {e}")))?;
    Ok(AppPassword::new(secret)?)
}

fn read_actor(args: &LoginArgs) -> Result<String, CliError> {
    if let Some(a) = &args.actor {
        return Ok(a.clone());
    }
    if args.password_stdin || !prompt::stdin_is_tty() {
        return Err(CliError::usage("give the handle or DID to log in as"));
    }
    match prompt::read_line("Handle or DID: ") {
        Ok(Some(a)) if !a.trim().is_empty() => Ok(a.trim().to_owned()),
        _ => Err(CliError::usage("give the handle or DID to log in as")),
    }
}

/// `lessdugin login`.
pub async fn run(ctx: &Ctx, args: LoginArgs) -> Result<(), CliError> {
    if args.password_argv.is_some() {
        return Err(argv_password_refusal());
    }
    if args.actor.as_deref().is_some_and(is_app_password_shape) {
        return Err(argv_password_refusal());
    }
    let app_password_mode = args.app_password || args.password_stdin;
    // The password is checked before anything touches the network (AUTH-3).
    let password = if app_password_mode {
        Some(read_app_password(&args)?)
    } else {
        None
    };
    let actor = read_actor(&args)?;
    let engine = ctx.engine()?;
    let store = ctx.store();
    let outcome = match password {
        Some(pw) => login_app_password(engine, &store, &ctx.paths, &actor, &pw).await?,
        None => oauth_login(ctx, &actor, &args).await?,
    };
    report(ctx, &outcome);
    Ok(())
}

async fn oauth_login(ctx: &Ctx, actor: &str, args: &LoginArgs) -> Result<LoginOutcome, CliError> {
    let engine = ctx.engine()?;
    let store = ctx.store();
    let client = match args.oauth_client {
        Some(OAuthClientArg::Hosted) => ClientKind::Hosted,
        _ => ClientKind::Loopback,
    };
    let opts = OAuthOptions {
        client,
        hosted_fallback: args.oauth_client.is_none(),
        listen: !args.no_browser,
    };
    let pending = oauth::begin(engine, actor, &opts).await?;
    let out = &ctx.out;
    out.err_line(&[
        (lit("Signing in at "), Style::Plain),
        (tt(&pending.authorization_server_host()), Style::Bold),
        (lit(" (authorization server of "), Style::Plain),
        (name(actor), Style::Plain),
        (lit(")"), Style::Plain),
    ]);
    let code = if args.no_browser {
        out.err_line(&[(
            lit("Open this URL in a browser, sign in and approve:"),
            Style::Plain,
        )]);
        out.err_line(&[(tt(pending.authorize_url().as_str()), Style::Plain)]);
        out.err_line(&[(
            cat([
                lit("The browser then fails to load "),
                tt(pending.redirect_uri().as_str()),
                lit("?...: copy that address and paste it here."),
            ]),
            Style::Plain,
        )]);
        let mut attempts = 0;
        loop {
            attempts += 1;
            let line = prompt::read_line("Redirect URL: ")
                .map_err(|e| EngineError::invalid_input(e.to_string()))?
                .ok_or_else(|| {
                    EngineError::new(ErrorCode::AuthRequired, "no redirect URL was pasted")
                })?;
            if line.trim().is_empty() {
                continue;
            }
            match pending.parse_pasted(&line) {
                Ok(code) => break code,
                Err(e) if attempts < 3 && prompt::stdin_is_tty() => out.error(&e),
                Err(e) => return Err(e.into()),
            }
        }
    } else {
        out.err_line(&[(
            lit("Opening your browser. If it does not open, visit:"),
            Style::Plain,
        )]);
        out.err_line(&[(tt(pending.authorize_url().as_str()), Style::Plain)]);
        let url = pending.authorize_url().to_string();
        let opened = tokio::task::spawn_blocking(move || webbrowser::open(&url).is_ok())
            .await
            .unwrap_or(false);
        if !opened {
            out.note(&[(
                lit("(no browser could be opened; use the URL above, or --no-browser over SSH)"),
                Style::Dim,
            )]);
        }
        out.note(&[(lit("Waiting for the browser..."), Style::Dim)]);
        pending.wait_for_callback(CALLBACK_TIMEOUT).await?
    };
    let grant = pending.exchange(code).await?;
    match grant.finish(engine, &store, &ctx.paths, false).await {
        Ok(o) => Ok(o),
        Err(e) if is_issuer_change(&e) => {
            let pinned = detail_str(&e, "pinned");
            let issuer = detail_str(&e, "issuer");
            out.err_line(&[
                (lit("warning: "), Style::Warn),
                (
                    lit("the authorization server of this account changed: "),
                    Style::Plain,
                ),
                (tt(&pinned), Style::Bold),
                (lit(" -> "), Style::Plain),
                (tt(&issuer), Style::Bold),
            ]);
            if prompt::confirm("Trust the new authorization server?") {
                Ok(grant.finish(engine, &store, &ctx.paths, true).await?)
            } else {
                grant.abandon().await;
                Err(e.into())
            }
        }
        Err(e) => {
            grant.abandon().await;
            Err(e.into())
        }
    }
}

fn is_issuer_change(e: &EngineError) -> bool {
    e.code == ErrorCode::Conflict && detail_str(e, "kind") == "issuerChanged"
}

fn detail_str(e: &EngineError, key: &str) -> String {
    e.detail
        .as_ref()
        .and_then(|d| d.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

fn report(ctx: &Ctx, o: &LoginOutcome) {
    let out = &ctx.out;
    if out.is_json() {
        out.json(&o.account);
        return;
    }
    let who = match &o.account.handle {
        Some(h) => cat([lit("@"), name(h), lit(" ("), tt(&o.account.did), lit(")")]),
        None => tt(&o.account.did),
    };
    out.line(&[(lit("Logged in as "), Style::Plain), (who, Style::Bold)]);
    out.field("pds", 6, tt(&o.account.pds));
    if let Some(iss) = &o.binding.issuer {
        let note = match o.binding.pin {
            PinStatus::New => lit(" (first login: pinned for this account)"),
            PinStatus::Changed => lit(" (changed and re-pinned)"),
            _ => TermText::empty(),
        };
        out.field("issuer", 6, cat([tt(iss), note]));
    }
    match &o.backend {
        Backend::Keyring => out.note(&[(lit("Session stored in the OS keyring."), Style::Dim)]),
        Backend::File(p) => out.note(&[(
            cat([
                lit("Session stored in "),
                tt(&p.display().to_string()),
                lit(" (mode 0600; no OS keyring available)."),
            ]),
            Style::Dim,
        )]),
    }
}
