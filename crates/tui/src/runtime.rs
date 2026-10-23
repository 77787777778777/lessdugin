// SPDX-License-Identifier: AGPL-3.0-or-later
//! The runtime on a real terminal (crossterm), inside a Tokio runtime.
//!
//! - Engine effects run as tasks; their results come back as actions on one channel, together
//!   with engine events (`subscribe`) and terminal input (a reader thread).
//! - `live_tail` (polling, `engine-api.md` §3.3) and the outbox pump of the writing account
//!   run for as long as the TUI does, unless offline.
//! - `$EDITOR` runs with the terminal suspended: the input reader is parked first (so it cannot
//!   take the editor's keys), then the alternate screen is left and raw mode switched off; both
//!   are restored afterwards and the screen redrawn.
//! - The terminal is restored on exit, on errors and on a panic of the UI thread. A panic in
//!   an effect task does not touch the terminal from that thread (the UI would go on drawing
//!   in cooked mode): the task reports it, and the UI loop ends with an error, restoring the
//!   terminal as on any error.

use std::collections::VecDeque;
use std::io::{IsTerminal, Stdout};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::thread::ThreadId;
use std::time::Duration;

use crossterm::event::{self, Event as TermEvent};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use futures::FutureExt as _;
use lessdugin_core::dto::AccountView;
use lessdugin_core::engine::Engine;
use lessdugin_core::host::Host;
use lessdugin_core::outbox::Session;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::app::{Action, Effect, State};
use crate::editor::EditorConfig;
use crate::exec::{self, ExecOpts};

/// How the TUI starts.
#[derive(Clone, Debug)]
pub struct Options {
    /// A forum uri to open on top of Home.
    pub forum: Option<String>,
    /// The account that writes (its session must be registered with the engine's host);
    /// `None`: read-only.
    pub account: Option<AccountView>,
    /// No network: nothing is synced, writes stay queued.
    pub offline: bool,
    /// The editor.
    pub editor: EditorConfig,
}

/// What the TUI left behind.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Writes still queued at exit (sent by the next command that writes).
    pub queued_writes: usize,
    /// Writes that failed (sent again only by `lessdugin outbox retry`).
    pub failed_writes: usize,
}

/// Runs the TUI until the user quits. Needs a terminal on stdin and stdout.
pub async fn run<H: Host>(engine: Engine<H>, opts: Options) -> anyhow::Result<Report> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("the TUI needs a terminal (stdin and stdout)");
    }
    let (tx, mut rx) = unbounded_channel::<Action>();
    let events = tx.clone();
    let sub = engine.subscribe(move |e| {
        let _ = events.send(Action::Engine(e));
    });

    // long-lived engine work
    if !opts.offline {
        let e = engine.clone();
        tokio::spawn(async move {
            let _ = e.live_tail().await;
        });
        if let Some(session) = opts
            .account
            .as_ref()
            .and_then(|a| Session::parse(&a.did, &a.pds).ok())
        {
            let e = engine.clone();
            tokio::spawn(async move {
                e.outbox().pump(&session).await;
            });
        }
    }

    let result = ui(&engine, &opts, tx, &mut rx).await;

    engine.unsubscribe(sub);
    engine.live_stop();
    engine.outbox_stop();
    result
}

async fn ui<H: Host>(
    engine: &Engine<H>,
    opts: &Options,
    tx: UnboundedSender<Action>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>,
) -> anyhow::Result<Report> {
    let mut term = Term::enter()?;
    let input = Input::spawn(tx.clone());
    let (w, h) = crossterm::terminal::size()?;
    let mut state = State::new(w, h, opts.account.clone(), opts.offline);
    let exec_opts = ExecOpts {
        offline: opts.offline,
    };
    let mut queue: VecDeque<Effect> = state.start(opts.forum.clone()).into();
    // effect tasks that panicked report here; the UI then stops (see the module docs)
    let (panicked_tx, mut panicked) = unbounded_channel::<String>();
    let outcome: anyhow::Result<()> = async {
        'main: loop {
            while let Some(effect) = queue.pop_front() {
                match effect {
                    Effect::Quit => break 'main,
                    Effect::Editor(req) => {
                        input.pause();
                        term.suspend()?;
                        let cfg = opts.editor.clone();
                        let template = req.template.clone();
                        let result = tokio::task::spawn_blocking(move || cfg.edit(&template))
                            .await
                            .unwrap_or_else(|e| Err(format!("the editor task failed: {e}").into()));
                        term.resume()?;
                        input.resume();
                        if let Ok((w, h)) = crossterm::terminal::size() {
                            queue.extend(state.update(Action::Resize(w, h)));
                        }
                        queue.extend(state.update(Action::Edited {
                            compose: req.compose,
                            result,
                        }));
                    }
                    other => {
                        let engine = engine.clone();
                        let tx = tx.clone();
                        let panicked = panicked_tx.clone();
                        tokio::spawn(async move {
                            let run =
                                std::panic::AssertUnwindSafe(exec::run(&engine, other, exec_opts));
                            match run.catch_unwind().await {
                                Ok(Some(a)) => {
                                    let _ = tx.send(a);
                                }
                                Ok(None) => {}
                                Err(payload) => {
                                    let _ = panicked.send(panic_message(payload.as_ref()));
                                }
                            }
                        });
                    }
                }
            }
            if state.quit {
                break;
            }
            term.terminal.draw(|f| crate::view::draw(&state, f))?;
            let a = tokio::select! {
                a = rx.recv() => a,
                Some(why) = panicked.recv() => {
                    anyhow::bail!("an internal task failed: {why}");
                }
            };
            let Some(a) = a else {
                break;
            };
            queue.extend(state.update(a));
            // take what else arrived before drawing again
            while let Ok(a) = rx.try_recv() {
                queue.extend(state.update(a));
            }
        }
        Ok(())
    }
    .await;
    input.stop();
    drop(term);
    outcome?;
    Ok(Report {
        queued_writes: state.writes.len(),
        failed_writes: state.failed.len(),
    })
}

/// The message of a panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".to_owned())
}

/// The thread that owns the terminal while a [`Term`] is active: only a panic there restores
/// it from the panic hook.
static UI_THREAD: Mutex<Option<ThreadId>> = Mutex::new(None);

fn is_ui_thread() -> bool {
    UI_THREAD
        .lock()
        .ok()
        .and_then(|t| *t)
        .is_some_and(|t| t == std::thread::current().id())
}

/// Restores the terminal (best effort).
fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show);
}

/// The terminal in raw mode on the alternate screen; restored when dropped.
struct Term {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl Term {
    fn enter() -> std::io::Result<Self> {
        static HOOK: Once = Once::new();
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if is_ui_thread() {
                    restore();
                }
                previous(info);
            }));
        });
        if let Ok(mut t) = UI_THREAD.lock() {
            *t = Some(std::thread::current().id());
        }
        enable_raw_mode()?;
        let terminal = match Self::fresh() {
            Ok(t) => t,
            Err(e) => {
                restore();
                return Err(e);
            }
        };
        Ok(Self {
            terminal,
            active: true,
        })
    }

    /// A cleared alternate screen and a terminal whose buffers match it (blank), so the next
    /// draw writes everything. `Terminal::clear` is not used: it asks the terminal for the
    /// cursor position, which some terminals never answer.
    fn fresh() -> std::io::Result<Terminal<CrosstermBackend<Stdout>>> {
        execute!(
            std::io::stdout(),
            EnterAlternateScreen,
            Clear(ClearType::All),
            cursor::Hide
        )?;
        Terminal::new(CrosstermBackend::new(std::io::stdout()))
    }

    fn suspend(&mut self) -> std::io::Result<()> {
        self.active = false;
        execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show)?;
        disable_raw_mode()
    }

    fn resume(&mut self) -> std::io::Result<()> {
        enable_raw_mode()?;
        self.active = true;
        self.terminal = Self::fresh()?;
        Ok(())
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        if self.active {
            restore();
        }
        if let Ok(mut t) = UI_THREAD.lock() {
            *t = None;
        }
    }
}

/// The input reader's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Running,
    PauseRequested,
    Paused,
    Stop,
}

/// A thread reading terminal events into the action channel; it can be parked while another
/// program (the editor) owns the terminal.
struct Input {
    ctl: Arc<(Mutex<Mode>, Condvar)>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Input {
    fn spawn(tx: UnboundedSender<Action>) -> Self {
        let ctl = Arc::new((Mutex::new(Mode::Running), Condvar::new()));
        let c = Arc::clone(&ctl);
        // a plain thread: crossterm's reads block, and this is native-only UI code
        #[allow(clippy::disallowed_methods)]
        let handle = std::thread::spawn(move || {
            let (lock, cv) = &*c;
            loop {
                {
                    let Ok(mut m) = lock.lock() else { return };
                    loop {
                        match *m {
                            Mode::Running => break,
                            Mode::Stop => return,
                            Mode::PauseRequested => {
                                *m = Mode::Paused;
                                cv.notify_all();
                            }
                            Mode::Paused => match cv.wait(m) {
                                Ok(g) => m = g,
                                Err(_) => return,
                            },
                        }
                    }
                }
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => {
                        let action = match event::read() {
                            Ok(TermEvent::Key(k)) => Some(Action::Key(k)),
                            Ok(TermEvent::Resize(w, h)) => Some(Action::Resize(w, h)),
                            _ => None,
                        };
                        if let Some(a) = action
                            && tx.send(a).is_err()
                        {
                            return;
                        }
                    }
                    Ok(false) => {}
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        Self {
            ctl,
            handle: Some(handle),
        }
    }

    fn set(&self, mode: Mode) {
        let (lock, cv) = &*self.ctl;
        if let Ok(mut m) = lock.lock() {
            *m = mode;
            cv.notify_all();
        }
    }

    /// Parks the reader and waits (up to a second) until it no longer reads.
    fn pause(&self) {
        self.set(Mode::PauseRequested);
        let (lock, cv) = &*self.ctl;
        if let Ok(m) = lock.lock() {
            let _ =
                cv.wait_timeout_while(m, Duration::from_secs(1), |m| *m == Mode::PauseRequested);
        }
    }

    fn resume(&self) {
        self.set(Mode::Running);
    }

    fn stop(mut self) {
        self.set(Mode::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
