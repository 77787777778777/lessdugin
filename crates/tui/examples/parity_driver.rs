// SPDX-License-Identifier: AGPL-3.0-or-later
//! The TUI's part of the M2 parity scenario (`docs/plan.md` §4.2, text subset;
//! `packages/e2e/README.md`): a command-line driver over the headless TUI
//! ([`lessdugin_tui::Headless`]). The Playwright scenario (`web/e2e/devnet/scenario.spec.ts`)
//! runs it as a separate process, next to the `lessdugin` binary.
//!
//! It opens the engine the way `lessdugin --dev tui` does: the data directory of
//! `LESSDUGIN_DATA_DIR`, the devnet of `LESSDUGIN_PLC_URL` and `LESSDUGIN_PDS_URL`, parity mode
//! from `LESSDUGIN_PARITY_NOW_MS` (`engine-api.md` §2.1), the session `lessdugin login` stored
//! there, and `$EDITOR` for writing.
//!
//! ```text
//! parity_driver reply <forum-uri> <post-uri>
//!     As the logged-in account: opens the forum (which syncs it), selects the post, opens it,
//!     presses `c` and writes the comment in $EDITOR. The write is sent before it returns (the
//!     outbox is flushed, as the runtime's pump would). Prints {"uri": <the comment>}.
//! parity_driver read <forum-uri> <post-uri> <out-dir>
//!     Read-only (a fresh data directory is an anonymous reader): opens the forum (which syncs
//!     it), then the post's thread (which syncs it). Writes to <out-dir>: list.json and
//!     thread.json (the Forum screen's query with the other clients' page size,
//!     `exec::PARITY_LIST_LIMIT`, checked against the screen, and the Thread screen's ThreadView, dto/1 JSON
//!     exactly as dto.rs serialises it, the bytes the wasm boundary carries), list.pretty.json
//!     and thread.pretty.json (the same, as `lessdugin --json` prints them to a pipe), and the
//!     screens as text (forum-screen.txt, thread-screen.txt, thread-document.txt).
//! ```
//!
//! Options: `--width N` and `--height N` (default 100 × 30). Exit code 0 on success, 1 on an
//! error (on stderr).
//!
//! Build: `cargo build -p lessdugin-tui --example parity_driver`
//! (`target/debug/examples/parity_driver`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lessdugin_core::dto::{PostList, ThreadRoot};
use lessdugin_core::engine::Engine;
use lessdugin_native::auth::{SecretStore, restore_session};
use lessdugin_native::config::{Config, DevnetProfile, NetworkProfile, Paths};
use lessdugin_native::engine::open_engine;
use lessdugin_native::host::NativeHost;
use lessdugin_tui::app::ScreenKind;
use lessdugin_tui::exec;
use lessdugin_tui::{CaptureOpts, EditorConfig, Headless, HeadlessOpts, capture_thread};

type Res<T> = Result<T, String>;

/// The devnet engine of `LESSDUGIN_DATA_DIR`, as `lessdugin --dev` opens it.
struct Device {
    engine: Engine<NativeHost>,
    paths: Paths,
    keyring_service: String,
}

fn var(k: &str) -> Res<String> {
    std::env::var(k)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{k} is not set"))
}

fn open() -> Res<Device> {
    let dir = PathBuf::from(var("LESSDUGIN_DATA_DIR")?);
    let url =
        |k: &str| -> Res<url::Url> { url::Url::parse(&var(k)?).map_err(|e| format!("{k}: {e}")) };
    let profile = DevnetProfile {
        plc: url("LESSDUGIN_PLC_URL")?,
        pds: url("LESSDUGIN_PDS_URL")?,
        jetstream: None,
    };
    let paths = Paths::resolve(Some(&dir)).map_err(|e| e.to_string())?;
    let mut config = Config::load(&paths.config_file).unwrap_or_default();
    config.plc = profile.plc.clone();
    config.network = NetworkProfile::Devnet(profile.clone());
    config.validate().map_err(|e| e.to_string())?;
    let paths = paths.for_devnet(&profile);
    let engine = open_engine(&paths, &config).map_err(|e| e.message)?;
    Ok(Device {
        engine,
        paths,
        keyring_service: profile.keyring_service(),
    })
}

/// The Forum screen's post list (the screen on top must be the forum).
fn forum_list<H: lessdugin_core::host::Host>(h: &Headless<H>) -> Res<PostList> {
    match h.state.top().map(|s| &s.kind) {
        Some(ScreenKind::Forum(f)) => f
            .data
            .as_ref()
            .map(|d| d.list.clone())
            .ok_or_else(|| format!("the forum did not load: {:?}", h.top_error())),
        _ => Err("the forum is not on top".into()),
    }
}

/// Opens `forum` (Home under it), applies any pending update, selects `post` and opens it.
async fn open_post<H: lessdugin_core::host::Host>(
    h: &mut Headless<H>,
    forum: &str,
    post: &str,
) -> Res<PostList> {
    h.start(Some(forum.to_owned())).await;
    h.state.show_pending();
    let list = forum_list(h)?;
    let i = list
        .items
        .iter()
        .position(|p| p.uri == post)
        .ok_or_else(|| format!("{post} is not in the forum's list:\n{}", h.render()))?;
    for _ in 0..i {
        h.press("j").await;
    }
    h.press("Enter").await;
    let opened = h.thread_view().and_then(|v| match &v.post {
        ThreadRoot::Post(p) => Some(p.post.uri.clone()),
        ThreadRoot::Missing { .. } => None,
    });
    if opened.as_deref() != Some(post) {
        return Err(format!(
            "the thread of {post} did not open:\n{}",
            h.render()
        ));
    }
    Ok(list)
}

async fn reply(forum: &str, post: &str, size: (u16, u16)) -> Res<()> {
    let d = open()?;
    let account = d
        .engine
        .active_account()
        .await
        .map_err(|e| e.message)?
        .ok_or("not logged in: run `lessdugin --dev login` with this data directory first")?;
    let store = SecretStore::new(&d.paths).with_keyring_service(d.keyring_service.clone());
    let account = restore_session(&d.engine, &store, &d.paths, &account)
        .await
        .map_err(|e| e.message)?;
    let engine = d
        .engine
        .acting_as(&account.did)
        .await
        .map_err(|e| e.message)?;
    let mut h = Headless::new(
        engine.clone(),
        HeadlessOpts {
            width: size.0,
            height: size.1,
            // template files for $EDITOR, as `lessdugin tui` keeps them
            editor: Some(EditorConfig::from_env(d.paths.data_dir.join("tmp"))),
            ..HeadlessOpts::default()
        },
    )
    .await
    .map_err(|e| e.message)?;
    open_post(&mut h, forum, post).await?;
    let before: Vec<String> = h
        .thread_view()
        .map(|v| v.comments.iter().map(|c| c.uri.clone()).collect())
        .unwrap_or_default();
    h.press("c").await;
    let screen = h.render();
    let view = h.thread_view().ok_or("the thread is gone")?;
    let mine = view
        .comments
        .iter()
        .find(|c| c.author.did == account.did && !before.contains(&c.uri))
        .ok_or_else(|| format!("no new comment by {} on the screen:\n{screen}", account.did))?;
    if mine.badges.pending {
        let writes = engine.writes().await.map_err(|e| e.message)?;
        return Err(format!(
            "the comment was not sent: {}\n{screen}",
            serde_json::to_string(&writes).unwrap_or_default()
        ));
    }
    eprintln!("{screen}");
    println!("{}", serde_json::json!({ "uri": mine.uri }));
    Ok(())
}

fn write(dir: &Path, name: &str, text: &str) -> Res<()> {
    std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))
}

async fn read(forum: &str, post: &str, out: &Path, size: (u16, u16)) -> Res<()> {
    let d = open()?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    // the Forum screen: opening it syncs the forum (as the TUI does)
    let mut h = Headless::new(
        d.engine.clone(),
        HeadlessOpts {
            width: size.0,
            height: size.1,
            ..HeadlessOpts::default()
        },
    )
    .await
    .map_err(|e| e.message)?;
    h.start(Some(forum.to_owned())).await;
    h.state.show_pending();
    let screen = forum_list(&h)?;
    let sort = match h.state.top().map(|s| &s.kind) {
        Some(ScreenKind::Forum(f)) => f.sort,
        _ => return Err("the forum is not on top".into()),
    };
    // The page the other clients compare: the screen's query with their page size. The
    // screen lists more (LIST_LIMIT); its first posts must be this page, in this order.
    let list = d
        .engine
        .list_posts(&exec::posts_query(forum, sort, exec::PARITY_LIST_LIMIT))
        .await
        .map_err(|e| e.message)?;
    let uris = |l: &PostList| l.items.iter().map(|p| p.uri.clone()).collect::<Vec<_>>();
    let shown = uris(&screen);
    if !shown.starts_with(&uris(&list)) || (list.cursor.is_none() && shown != uris(&list)) {
        return Err(format!(
            "the Forum screen does not start with the compared page:\nscreen {shown:?}\npage {:?}",
            uris(&list)
        ));
    }
    write(out, "forum-screen.txt", &h.render())?;
    drop(h);
    // the Thread screen through the parity hook: opening it syncs the thread
    let t = capture_thread(
        &d.engine,
        post,
        CaptureOpts {
            width: size.0,
            height: size.1,
            sync: true,
        },
    )
    .await
    .map_err(|e| e.message)?;
    // as `capture_thread` serialises the ThreadView
    let list_compact = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    let mut list_pretty = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    list_pretty.push('\n');
    write(out, "list.json", &list_compact)?;
    write(out, "list.pretty.json", &list_pretty)?;
    write(out, "thread.json", &t.json)?;
    write(out, "thread.pretty.json", &t.json_pretty)?;
    write(out, "thread-screen.txt", &t.screen)?;
    write(out, "thread-document.txt", &t.document)?;
    Ok(())
}

const USAGE: &str = "usage: parity_driver reply <forum-uri> <post-uri> [--width N] [--height N]\n       parity_driver read <forum-uri> <post-uri> <out-dir> [--width N] [--height N]";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut size = (100u16, 30u16);
    let mut i = 0;
    while i < args.len() {
        let target = match args[i].as_str() {
            "--width" => &mut size.0,
            "--height" => &mut size.1,
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(n) = args.get(i + 1).and_then(|v| v.parse().ok()) else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        };
        *target = n;
        args.drain(i..i + 2);
    }
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("parity_driver: {e}");
            return ExitCode::FAILURE;
        }
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let res = match args.as_slice() {
        ["reply", forum, post] => rt.block_on(reply(forum, post, size)),
        ["read", forum, post, out] => rt.block_on(read(forum, post, Path::new(out), size)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parity_driver: {e}");
            ExitCode::FAILURE
        }
    }
}
