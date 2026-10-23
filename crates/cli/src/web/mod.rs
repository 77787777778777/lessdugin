// SPDX-License-Identifier: AGPL-3.0-or-later
//! `lessdugin web`: the embedded SPA and the bridge on `127.0.0.1` — the zero-trust path
//! (`threat-model.md` §7, `docs/design/synthesis-v1.md` §6.13).
//!
//! The CLI keeps the sessions; the browser holds no long-lived secret. The tab proves it was
//! opened from this terminal with a single-use capability in the URL fragment, exchanges it for
//! an in-memory session token, and then reaches the bridge:
//!
//! - [`server`]: the listener, the Host check, the §7.3 checks, static files and
//!   `lessdugin-config.json` (`{"oauth":"bridge"}`), `POST /bridge/session`;
//! - [`xrpc`]: `POST /bridge/xrpc/<nsid>?account=<did>`, the allowlisted writes;
//! - [`kv`]: `POST /bridge/kv`, the SPA's `state` store in native `state.sqlite`;
//! - [`lock`]: `POST /bridge/lock`, the native `outbox:<did>` lock held for a tab;
//! - [`login`]: `POST /bridge/login`, native loopback OAuth for a tab, confirmed in this
//!   terminal;
//! - [`caps`]: capabilities and sessions; [`policy`]: headers and limits; [`assets`]: the SPA
//!   compiled in by `build.rs`; [`config`]: `web.port`.
//!
//! `/bridge/fetch` is Reserved (P2): it answers 404.

mod assets;
mod caps;
mod config;
mod kv;
mod lock;
mod login;
mod policy;
mod server;
mod xrpc;

#[cfg(test)]
mod tests;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use lessdugin_core::dto::DTO_VERSION;
use lessdugin_core::engine::{EngineError, ErrorCode};
use serde_json::json;

use self::assets::Assets;
use self::caps::Caps;
use self::login::{LoginSlot, Prompt};
use self::server::Web;
use crate::ctx::{CliError, Ctx};
use crate::out::{Style, cat, lit, tt};

/// `web`.
#[derive(Debug, clap::Args)]
pub struct WebArgs {
    /// The port on 127.0.0.1 (default: `web.port` in config.toml, else 47778). It is part of
    /// the web app's origin, so a busy port is an error, never a reason to pick another.
    #[arg(long, value_name = "PORT", value_parser = clap::value_parser!(u16).range(1..))]
    pub port: Option<u16>,

    /// Print the link instead of opening it in a browser.
    #[arg(long)]
    pub no_open: bool,
}

fn bind_error(port: u16, e: &std::io::Error) -> CliError {
    let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
        format!(
            "port {port} on 127.0.0.1 is in use; lessdugin web never picks another port (the \
             port is part of the web app's origin). Stop the program using it, or choose a \
             port with --port or web.port in config.toml"
        )
    } else {
        format!("cannot listen on 127.0.0.1:{port}: {e}")
    };
    CliError::Engine(EngineError::new(ErrorCode::Network, msg).with_detail("port", port))
}

/// Prints a link (and opens it unless `--no-open`).
async fn announce_link(web: &Web, url: String, open: bool) {
    let out = &web.out;
    if out.is_json() {
        out.json(&json!({"dto": DTO_VERSION, "url": url}));
    } else {
        out.line(&[(tt(&url), Style::Bold)]);
    }
    if open {
        let u = url.clone();
        let opened = tokio::task::spawn_blocking(move || webbrowser::open(&u).is_ok())
            .await
            .unwrap_or(false);
        if !opened {
            out.note(&[(
                lit("(no browser could be opened; open the link above yourself)"),
                Style::Dim,
            )]);
        }
    }
}

/// Reads lines from stdin on a thread. A line answers the question the terminal asks (a
/// browser sign-in's confirmation); otherwise each Enter asks for a fresh link (§7.2). The
/// channel closes at the end of stdin.
fn stdin_lines() -> tokio::sync::mpsc::UnboundedReceiver<String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line.clone()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// `lessdugin web`.
pub async fn run(ctx: &Ctx, args: WebArgs) -> Result<(), CliError> {
    let port = config::port(args.port, &ctx.paths.config_file).map_err(CliError::usage)?;
    let engine = ctx.engine()?.clone();
    // WEB-1: 127.0.0.1 only.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|e| bind_error(port, &e))?;
    let web = Arc::new(Web {
        port,
        dev: ctx.config.is_devnet(),
        assets: Assets::embedded(),
        caps: Caps::default(),
        engine,
        store: ctx.store(),
        paths: ctx.paths.clone(),
        out: ctx.out.clone(),
        login: LoginSlot::default(),
        prompt: Prompt::default(),
        shutdown: tokio::sync::watch::channel(false).0,
    });
    let out = &ctx.out;
    out.note(&[
        (lit("lessdugin web is serving on "), Style::Plain),
        (tt(&web.origin()), Style::Bold),
        (lit(" (127.0.0.1 only)."), Style::Plain),
    ]);
    if web.assets.is_placeholder() {
        out.warn(lit(
            "this binary was built without the web app (web/dist): only a placeholder page \
             explaining how to build it is served",
        ));
    } else if let Some(h) = web.assets.build_hash() {
        out.note(&[(cat([lit("web app build sha256 "), tt(&h)]), Style::Dim)]);
    }
    if web.dev {
        out.note(&[(
            lit("devnet mode: the development policy (connect-src also http: ws:) is served"),
            Style::Dim,
        )]);
    }
    out.note(&[(
        lit("Each link works once. Press Enter for a fresh link; Ctrl-C stops the server."),
        Style::Dim,
    )]);

    let app = Router::new()
        .fallback(server::handle)
        .with_state(web.clone());
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = stop_rx.await;
    });
    let mut server = tokio::spawn(async move { server.await });

    announce_link(&web, web.mint_link(), !args.no_open).await;
    let mut lines = stdin_lines();
    let minting = {
        let web = web.clone();
        let open = !args.no_open;
        tokio::spawn(async move {
            while let Some(line) = lines.recv().await {
                if web.prompt.offer(line).is_some() {
                    announce_link(&web, web.mint_link(), open).await;
                }
            }
            web.prompt.close();
        })
    };
    // Ctrl-C stops accepting connections and gives open ones WEB_SHUTDOWN_GRACE_MS to finish
    // (held /bridge/lock bodies end at once); a second Ctrl-C, or the grace running out, stops
    // the server regardless (a stalled client cannot keep it alive).
    let result = tokio::select! {
        r = &mut server => r,
        _ = tokio::signal::ctrl_c() => {
            let _ = stop_tx.send(());
            web.shutdown.send_replace(true);
            web.login.cancel();
            let grace = tokio::time::sleep(Duration::from_millis(policy::WEB_SHUTDOWN_GRACE_MS));
            tokio::select! {
                r = &mut server => r,
                () = grace => {
                    server.abort();
                    Ok(Ok(()))
                }
                _ = tokio::signal::ctrl_c() => {
                    server.abort();
                    Ok(Ok(()))
                }
            }
        }
    };
    minting.abort();
    web.login.cancel();
    match result {
        Ok(Ok(())) => {
            out.note(&[(lit("lessdugin web stopped."), Style::Dim)]);
            Ok(())
        }
        Ok(Err(e)) => {
            Err(EngineError::new(ErrorCode::Network, format!("the server failed: {e}")).into())
        }
        Err(e) => Err(EngineError::internal(format!("the server task failed: {e}")).into()),
    }
}
