// SPDX-License-Identifier: AGPL-3.0-or-later
//! The headless driver: the TUI's state, update and view over a ratatui `TestBackend`, with
//! the effects run in order against an engine (tests, and the parity scenario).
//!
//! [`Headless::settle`] runs every queued effect and engine event until nothing is left, so a
//! sequence of key presses is deterministic. Engine calls are awaited in place; `$EDITOR`
//! runs without a terminal to suspend; after a write is queued the outbox is flushed (as the
//! runtime's pump would), unless [`HeadlessOpts::flush_writes`] is off. Images decode (in
//! halfblocks) only with [`HeadlessOpts::images`]; `settle` then waits for their workers.
//!
//! [`capture_thread`] is the parity hook (`docs/plan.md` §4.2): it opens the Thread screen
//! for a uri and returns what the screen shows and the exact `ThreadView` it holds, as dto/1
//! JSON.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use lessdugin_core::dto::{Event, ThreadView};
use lessdugin_core::engine::{Engine, EngineError, SubscriptionId};
use lessdugin_core::host::Host;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::app::{Action, Effect, ScreenKind, State};
use crate::editor::EditorConfig;
use crate::exec::{self, ExecOpts};
use crate::keys;

/// The most effects one [`Headless::settle`] runs (a loop guard).
const MAX_STEPS: usize = 10_000;

/// Options of the headless driver.
#[derive(Clone, Debug)]
pub struct HeadlessOpts {
    /// Columns.
    pub width: u16,
    /// Rows.
    pub height: u16,
    /// No network (no syncs, no handle checks; writes stay queued).
    pub offline: bool,
    /// Sync a forum or thread when its screen opens (as the TUI does).
    pub auto_sync: bool,
    /// Flush the outbox after a write is queued (the runtime's pump).
    pub flush_writes: bool,
    /// The editor (`None`: editing fails with a message).
    pub editor: Option<EditorConfig>,
    /// Decode and draw images, in halfblocks (off: no image is decoded).
    pub images: bool,
}

impl Default for HeadlessOpts {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            offline: false,
            auto_sync: true,
            flush_writes: true,
            editor: None,
            images: false,
        }
    }
}

/// The TUI without a terminal.
pub struct Headless<H: Host> {
    engine: Engine<H>,
    /// The application state.
    pub state: State,
    terminal: Terminal<TestBackend>,
    opts: HeadlessOpts,
    events: Arc<Mutex<Vec<Event>>>,
    sub: SubscriptionId,
    queue: VecDeque<Effect>,
    /// The image decoder and the decoded images.
    pub images: crate::image::Images,
}

impl<H: Host> Drop for Headless<H> {
    fn drop(&mut self) {
        self.engine.unsubscribe(self.sub);
    }
}

impl<H: Host> Headless<H> {
    /// A driver over `engine`, writing as its active account (if any). Nothing is open yet:
    /// call [`Headless::start`] or [`Headless::open_thread`].
    pub async fn new(engine: Engine<H>, opts: HeadlessOpts) -> Result<Self, EngineError> {
        let account = engine.active_account().await?;
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = Arc::clone(&events);
        let sub = engine.subscribe(move |e| {
            if let Ok(mut v) = sink.lock() {
                v.push(e);
            }
        });
        let mut state = State::new(opts.width, opts.height, account, opts.offline);
        state.auto_sync = opts.auto_sync;
        let terminal = Terminal::new(TestBackend::new(opts.width, opts.height))
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let images = if opts.images {
            crate::image::Images::new(
                ratatui_image::picker::Picker::halfblocks(),
                std::sync::Arc::new(|| {}),
            )
        } else {
            crate::image::Images::disabled()
        };
        Ok(Self {
            engine,
            state,
            terminal,
            opts,
            events,
            sub,
            queue: VecDeque::new(),
            images,
        })
    }

    /// The engine.
    pub fn engine(&self) -> &Engine<H> {
        &self.engine
    }

    /// Opens Home (and `forum` on top of it) and settles.
    pub async fn start(&mut self, forum: Option<String>) {
        let fx = self.state.start(forum);
        self.queue.extend(fx);
        self.settle().await;
    }

    /// Opens the thread of `uri` on top and settles.
    pub async fn open_thread(&mut self, uri: &str) {
        let fx = self.state.open_thread(uri.to_owned());
        self.queue.extend(fx);
        self.settle().await;
    }

    /// Presses `key` (as [`keys::parse_key`] names it) and settles.
    ///
    /// # Panics
    /// Panics on a key name it does not know.
    pub async fn press(&mut self, key: &str) {
        let k = keys::parse_key(key).unwrap_or_else(|| panic!("unknown key {key:?}"));
        self.act(Action::Key(k));
        self.settle().await;
    }

    /// Presses each key in turn.
    pub async fn keys(&mut self, keys: &[&str]) {
        for k in keys {
            self.press(k).await;
        }
    }

    /// Resizes the terminal and settles.
    pub async fn resize(&mut self, width: u16, height: u16) {
        self.terminal.backend_mut().resize(width, height);
        self.act(Action::Resize(width, height));
        self.settle().await;
    }

    /// Runs an action.
    pub fn act(&mut self, action: Action) {
        let fx = self.state.update(action);
        self.queue.extend(fx);
    }

    fn drain_events(&mut self) {
        let events = match self.events.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(_) => Vec::new(),
        };
        for e in events {
            self.act(Action::Engine(e));
        }
    }

    /// Runs queued effects and engine events until there are none.
    ///
    /// # Panics
    /// Panics if they never settle (more than 10,000 effects).
    pub async fn settle(&mut self) {
        let exec_opts = ExecOpts {
            offline: self.opts.offline,
        };
        let mut steps = 0usize;
        loop {
            self.drain_events();
            let Some(effect) = self.queue.pop_front() else {
                // the image workers' results, waited for (each decode has its own timeout)
                if self.wait_for_images() {
                    continue;
                }
                break;
            };
            steps += 1;
            assert!(steps < MAX_STEPS, "the TUI does not settle");
            match effect {
                Effect::Quit => {}
                Effect::DecodeMedia { cid, bytes } => self.images.decode(cid, bytes),
                Effect::Editor(req) => {
                    let result = match &self.opts.editor {
                        Some(cfg) => cfg.edit(&req.template),
                        None => Err("no editor in this session".into()),
                    };
                    self.act(Action::Edited {
                        compose: req.compose,
                        result,
                    });
                }
                other => {
                    let write = matches!(
                        other,
                        Effect::Comment { .. }
                            | Effect::Publish { .. }
                            | Effect::WikiEdit { .. }
                            | Effect::SequenceCreate { .. }
                            | Effect::SequenceEdit { .. }
                            | Effect::SequenceAdd { .. }
                            | Effect::SequenceRemove { .. }
                            | Effect::SequenceMove { .. }
                    );
                    // The effect runner contains the whole engine sync future. Keep it off the
                    // synchronous headless caller's stack (Rust test workers have a small one).
                    if let Some(a) = Box::pin(exec::run(&self.engine, other, exec_opts)).await {
                        // the events the call emitted come first, as on the runtime's channel
                        self.drain_events();
                        let queued = matches!(
                            &a,
                            Action::Queued { result: Ok(_), .. }
                                | Action::Written { result: Ok(_), .. }
                        );
                        self.act(a);
                        if write
                            && queued
                            && self.opts.flush_writes
                            && !self.opts.offline
                            && let Err(e) = self.engine.outbox_flush().await
                        {
                            tracing::warn!(error = %e, "headless outbox flush failed");
                        }
                    }
                }
            }
        }
    }

    /// Waits for the decodes under way and hands their outcomes to the state; `true` if there
    /// were any.
    #[allow(clippy::disallowed_methods)] // the image workers are threads on wall time
    fn wait_for_images(&mut self) -> bool {
        let deadline = std::time::Instant::now() + crate::image::limits::DECODE_TIMEOUT * 2;
        loop {
            let polled = self.images.poll();
            if !polled.decoded.is_empty() || !polled.evicted.is_empty() {
                for (cid, result) in polled.decoded {
                    self.act(Action::MediaDecoded { cid, result });
                }
                for cid in polled.evicted {
                    self.act(Action::MediaEvicted { cid });
                }
                return true;
            }
            if !self.images.busy() || std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Draws the frame (with the decoded images); returns the screen as text (rows without
    /// trailing blanks).
    pub fn render(&mut self) -> String {
        let (state, images) = (&self.state, &mut self.images);
        let _ = self
            .terminal
            .draw(|f| crate::view::draw_with(state, Some(images), f));
        buffer_text(self.terminal.backend().buffer())
    }

    /// [`Headless::render`] once the resizes it asked the image workers for are done (the
    /// first draw of an image only asks for its size).
    #[allow(clippy::disallowed_methods)] // the resize worker is a thread on wall time
    pub fn render_images(&mut self) -> String {
        let mut screen = self.render();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if self.images.poll().redraw {
                screen = self.render();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        screen
    }

    /// The `ThreadView` the Thread screen on top holds.
    pub fn thread_view(&self) -> Option<&ThreadView> {
        Some(&self.state.top_thread()?.data.as_ref()?.view)
    }

    /// The whole thread as laid out (every line, without the selection gutter).
    pub fn thread_document(&self) -> Option<String> {
        let t = self.state.top_thread()?;
        Some(
            t.layout
                .lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// The error of the top screen's last load.
    pub fn top_error(&self) -> Option<&EngineError> {
        self.state.top()?.error.as_ref()
    }

    /// True if the top screen is a thread.
    pub fn on_thread(&self) -> bool {
        matches!(
            self.state.top().map(|s| &s.kind),
            Some(ScreenKind::Thread(_))
        )
    }
}

/// The text of a buffer: one line per row, trailing blanks removed, the cells hidden by wide
/// characters skipped.
pub fn buffer_text(buf: &Buffer) -> String {
    let w = usize::from(buf.area.width).max(1);
    let mut rows = Vec::new();
    for cells in buf.content.chunks(w) {
        let mut row = String::new();
        let mut skip = 0usize;
        for c in cells {
            if skip == 0 {
                row.push_str(c.symbol());
            }
            let cw = unicode_width::UnicodeWidthStr::width(c.symbol()).max(1);
            skip = skip.max(cw).saturating_sub(1);
        }
        rows.push(row.trim_end().to_owned());
    }
    rows.join("\n")
}

/// What [`capture_thread`] returns.
#[derive(Clone, Debug)]
pub struct ThreadCapture {
    /// The screen as text (`width` × `height`, rows without trailing blanks).
    pub screen: String,
    /// The whole thread as laid out at `width` (every line, without the selection gutter).
    pub document: String,
    /// The `ThreadView` the screen holds, as dto/1 JSON exactly as `dto.rs` serialises it
    /// (`serde_json::to_string`; the bytes the wasm boundary carries).
    pub json: String,
    /// The same, as the CLI's `read --json` prints it to a pipe (`to_string_pretty` and a
    /// newline).
    pub json_pretty: String,
    /// The view.
    pub view: ThreadView,
}

/// Options of [`capture_thread`].
#[derive(Clone, Copy, Debug)]
pub struct CaptureOpts {
    /// Columns.
    pub width: u16,
    /// Rows.
    pub height: u16,
    /// Run the sync the Thread screen runs when it opens (`sync_thread`). Off: the cache as
    /// it is (sync the same way as the other clients first).
    pub sync: bool,
}

impl Default for CaptureOpts {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            sync: false,
        }
    }
}

/// The parity hook: opens the Thread screen for `uri` over `engine` (a post or a comment; the
/// default comment sort, as `lessdugin read` and the SPA), settles, shows any pending live
/// update, and returns the screen and the view it holds.
pub async fn capture_thread<H: Host>(
    engine: &Engine<H>,
    uri: &str,
    opts: CaptureOpts,
) -> Result<ThreadCapture, EngineError> {
    let mut h = Headless::new(
        engine.clone(),
        HeadlessOpts {
            width: opts.width,
            height: opts.height,
            offline: false,
            auto_sync: opts.sync,
            flush_writes: false,
            editor: None,
            images: false,
        },
    )
    .await?;
    h.open_thread(uri).await;
    h.state.show_pending();
    if let Some(e) = h.top_error()
        && h.thread_view().is_none()
    {
        return Err(e.clone());
    }
    let view = h
        .thread_view()
        .cloned()
        .ok_or_else(|| EngineError::internal("the thread did not load"))?;
    let screen = h.render();
    let document = h.thread_document().unwrap_or_default();
    let json = serde_json::to_string(&view).map_err(|e| EngineError::internal(e.to_string()))?;
    let mut json_pretty =
        serde_json::to_string_pretty(&view).map_err(|e| EngineError::internal(e.to_string()))?;
    json_pretty.push('\n');
    Ok(ThreadCapture {
        screen,
        document,
        json,
        json_pretty,
        view,
    })
}
