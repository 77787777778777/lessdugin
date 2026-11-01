// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images in the terminal (synthesis-v1.md F35/F36, §3.9; `threat-model.md` SUP-9).
//!
//! The supply-chain rule here is narrower than the rest of the client, because image decoders are
//! a classic memory-safety and decompression-bomb surface:
//!
//! - PURE-RUST decoders only (`image-defaults`): no C library is linked for this path.
//! - At most 64 MiB of input, 8192 pixels per side, a 256 MiB decode allocation and 200 GIF frames
//!   (`limits.md` §6). The pixel caps are checked from the HEADER, through
//!   [`lessdugin_core::media::decoding_allowed`], BEFORE any pixels are allocated (a header it
//!   cannot measure is refused), and again by the decoder's own limits, in
//!   [`lessdugin_core::media::decode_image`]. A GIF is shown as its first frame: one of the at
//!   most 200 decoded.
//! - The decode and the resize happen OFF the UI thread. A supervisor thread gives each decode
//!   [`limits::DECODE_TIMEOUT`]; past it the image is abandoned (its placeholder says so) and
//!   whatever the decoder still produces is dropped. An abandoned decoder keeps its slot until
//!   its thread ends (it still holds its input and its allocation), so at most
//!   [`limits::MAX_DECODING`] decoders exist at once, however many time out.
//! - Decoded images are scaled down to [`limits::SHOWN_MAX_SIDE`] and at most
//!   [`limits::MAX_SHOWN`] are kept; the oldest is dropped first ([`Images::poll`] says which).
//!
//! # How the protocol is detected
//!
//! The protocol (Kitty graphics, iTerm2 inline images, Sixel, or the halfblock fallback that
//! needs nothing from the terminal) can only be learnt by ASKING the terminal: the query writes a
//! few control sequences and reads the answer from stdin. A terminal that never answers (a pty
//! with no emulator behind it, `script` to a pipe, some multiplexers and IDE consoles) turns that
//! read into a trap: whatever is typed next is swallowed by the reader waiting for the answer.
//! `ratatui-image`'s own query leaves such a reader behind (a thread blocked in `read`, which
//! eats the next keystroke before it dies), so it is not used. Instead:
//!
//! 1. [`detection`] decides, from the `LESSDUGIN_TUI_IMAGES` setting ([`Setting`]) and the
//!    environment, whether the terminal is worth asking at all. `auto` (the default) asks only a
//!    terminal the environment names (`TERM_PROGRAM`, `KITTY_WINDOW_ID`, `WEZTERM_*`,
//!    `ITERM_SESSION_ID`, `TMUX` and the like, or a `TERM` of a known emulator) or one that
//!    reports its cell size in pixels (which an emulator sets and a bare pty does not); anything
//!    else gets halfblocks without a query. `query`, `halfblocks` and `off` force the answer.
//! 2. When it asks, [`query_terminal`] writes the query and reads the answer ITSELF, on the UI
//!    thread's behalf, by polling stdin with one bounded wait ([`QUERY_TIMEOUT`]): the terminal's
//!    answer ends with a Device Status Report every terminal implements, so the read stops at it,
//!    at the timeout, or at end of input, and no reader is left on stdin afterwards. The TUI's
//!    input thread starts only then, so it is the only reader stdin ever has.
//! 3. Bytes read that are not the terminal's answer (keys typed before it arrived, or right after
//!    it) are handed back as key events ([`keys_of`]), so a keystroke is never lost to the query.
//!
//! Which media load, and when, is the state's business (the autoload policy, `sync.md` §11.3);
//! this module only decodes and draws what it is handed.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui_image::FontSize;
use ratatui_image::picker::cap_parser::{Parser, QueryStdioOptions, Response};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};

/// The whole terminal query waits at most this long for the answer (a terminal answers within
/// a round trip; one that never answers costs a forced `query` this much at startup).
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// The `LESSDUGIN_TUI_IMAGES` setting: whether the terminal is asked for its graphics protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Setting {
    /// Ask a terminal the environment names, or one that reports its cell size; halfblocks
    /// otherwise.
    #[default]
    Auto,
    /// Always ask (over SSH, where the emulator's variables are not forwarded).
    Query,
    /// Never ask: halfblocks.
    Halfblocks,
    /// Decode nothing.
    Off,
}

impl Setting {
    /// The environment variable.
    pub const VAR: &'static str = "LESSDUGIN_TUI_IMAGES";

    /// `auto`, `query`, `halfblocks` or `off` (case does not matter); `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "query" => Some(Self::Query),
            "halfblocks" => Some(Self::Halfblocks),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// The setting from `$LESSDUGIN_TUI_IMAGES` (`auto` when unset or unknown).
    pub fn from_env() -> Self {
        Self::from_var(|k| std::env::var(k).ok())
    }

    /// [`Setting::from_env`] over `var`.
    pub fn from_var(var: impl Fn(&str) -> Option<String>) -> Self {
        let Some(v) = var(Self::VAR) else {
            return Self::Auto;
        };
        Self::parse(&v).unwrap_or_else(|| {
            tracing::warn!(value = %v, "unknown {}: using auto", Self::VAR);
            Self::Auto
        })
    }
}

/// What the runtime does about images before its input reader starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detection {
    /// Ask the terminal ([`query_terminal`]).
    Query,
    /// Halfblocks, no query.
    Halfblocks,
    /// No images ([`Images::off`]).
    Off,
}

/// Variables an emulator (or a multiplexer, which answers queries itself) sets; `TERM` alone is
/// not evidence, since a bare pty inherits one.
const EMULATOR_VARS: [&str; 19] = [
    "TERM_PROGRAM",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
    "KONSOLE_VERSION",
    "VTE_VERSION",
    "GHOSTTY_RESOURCES_DIR",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "XTERM_VERSION",
    "WT_SESSION",
    "MLTERM",
    "TERMINATOR_UUID",
    "GNOME_TERMINAL_SCREEN",
    "TMUX",
    "STY",
];

/// `TERM` values (by prefix) only an emulator sets.
const EMULATOR_TERMS: [&str; 15] = [
    "xterm-kitty",
    "xterm-ghostty",
    "ghostty",
    "foot",
    "wezterm",
    "alacritty",
    "contour",
    "rio",
    "mintty",
    "konsole",
    "iterm",
    "mlterm",
    "rxvt",
    "tmux",
    "screen",
];

/// Whether the terminal is worth asking: `setting` decides, and under `auto` the environment
/// does (see the module docs). `cell_pixels_known`: the terminal reports its size in pixels
/// (`TIOCGWINSZ`), which an emulator does and a bare pty does not. A `TERM` that is unset or
/// `dumb` is never asked under `auto`.
pub fn detection(
    setting: Setting,
    var: impl Fn(&str) -> Option<String>,
    cell_pixels_known: bool,
) -> Detection {
    let set = |k: &str| var(k).is_some_and(|v| !v.trim().is_empty());
    match setting {
        Setting::Off => Detection::Off,
        Setting::Halfblocks => Detection::Halfblocks,
        Setting::Query => Detection::Query,
        Setting::Auto => {
            let term = var("TERM").unwrap_or_default();
            if term.is_empty() || term == "dumb" {
                return Detection::Halfblocks;
            }
            if cell_pixels_known
                || EMULATOR_VARS.iter().any(|k| set(k))
                || EMULATOR_TERMS.iter().any(|p| term.starts_with(p))
            {
                Detection::Query
            } else {
                Detection::Halfblocks
            }
        }
    }
}

/// The emulators whose Kitty and Sixel implementations `ratatui-image` does not trust (its own
/// blacklist): they are not asked for those, and may still draw iTerm2 images.
fn distrusts_kitty_and_sixel(var: &impl Fn(&str) -> Option<String>) -> bool {
    ["WEZTERM_EXECUTABLE", "KONSOLE_VERSION"]
        .iter()
        .any(|k| var(k).is_some_and(|v| !v.is_empty()))
}

/// iTerm2 inline images cannot be queried for: the emulators known to draw them, from the
/// environment, as `ratatui-image` reads it (inside tmux only the outer terminal's variables
/// count).
fn iterm2_hint(is_tmux: bool, var: &impl Fn(&str) -> Option<String>) -> Option<ProtocolType> {
    let set = |k: &str| var(k).is_some_and(|v| !v.is_empty());
    if is_tmux {
        return (set("ITERM_SESSION_ID") || set("WEZTERM_EXECUTABLE"))
            .then_some(ProtocolType::Iterm2);
    }
    const PROGRAMS: [&str; 9] = [
        "iTerm",
        "WezTerm",
        "mintty",
        "vscode",
        "Tabby",
        "Hyper",
        "rio",
        "Bobcat",
        "WarpTerminal",
    ];
    let program = var("TERM_PROGRAM").unwrap_or_default();
    if PROGRAMS.iter().any(|p| program.contains(p))
        || var("LC_TERMINAL").is_some_and(|t| t.contains("iTerm"))
    {
        return Some(ProtocolType::Iterm2);
    }
    None
}

/// The query to write to the terminal (`ratatui-image`'s, so its parser reads the answer):
/// Kitty and Sixel probes unless the emulator is distrusted for them, the cell size, and the
/// Device Status Report that ends the answer; wrapped for tmux passthrough when `is_tmux`.
pub fn query_string(is_tmux: bool, var: &impl Fn(&str) -> Option<String>) -> String {
    let mut options = QueryStdioOptions::default();
    if distrusts_kitty_and_sixel(var) {
        options.blacklist_protocols = vec![ProtocolType::Kitty, ProtocolType::Sixel];
    }
    Parser::query(is_tmux, options)
}

/// The terminal's answer to [`query_string`], fed byte by byte as it is read.
///
/// Bytes before the first ESC were typed before the terminal answered, and bytes after the
/// Device Status Report that ends the answer were typed after it: both are kept as
/// [`QueryAnswer::typed`], for the caller to forward as keys. (A key typed in the middle of the
/// answer burst would be lost; the terminal writes its answer in one go, so the window is a
/// single write.)
#[derive(Default)]
pub struct QueryAnswer {
    parser: Parser,
    responses: Vec<Response>,
    typed: Vec<u8>,
    seen_esc: bool,
    done: bool,
}

impl QueryAnswer {
    /// Nothing read yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes; `true` once the whole answer has arrived (nothing more should be read).
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        for (i, &b) in bytes.iter().enumerate() {
            if self.done {
                self.typed.extend_from_slice(&bytes[i..]);
                return true;
            }
            if !self.seen_esc {
                if b != 0x1b {
                    self.typed.push(b);
                    continue;
                }
                self.seen_esc = true;
            }
            // the parser reads one byte as one char, as ratatui-image feeds it
            for r in self.parser.push(char::from(b)) {
                if r == Response::Status {
                    self.done = true;
                } else {
                    self.responses.push(r);
                }
            }
        }
        self.done
    }

    /// `true` once the Device Status Report arrived.
    pub fn complete(&self) -> bool {
        self.done
    }

    /// The bytes that were not the terminal's answer.
    pub fn typed(&self) -> &[u8] {
        &self.typed
    }

    /// The picker for what the terminal answered, as `ratatui-image` would pick it: Kitty when
    /// supported, else Sixel, else the iTerm2 hint from the environment; the cell size from the
    /// answer, else `cell_pixels` (the window size in pixels over its size in cells). A protocol
    /// cannot draw without a cell size, and halfblocks need neither: they are the fallback.
    pub fn picker(
        &self,
        is_tmux: bool,
        var: &impl Fn(&str) -> Option<String>,
        cell_pixels: Option<(u16, u16)>,
    ) -> Picker {
        let distrusted = distrusts_kitty_and_sixel(var);
        let mut proto = None;
        let mut font: Option<FontSize> = None;
        for r in &self.responses {
            match r {
                Response::Kitty if !distrusted => proto = Some(ProtocolType::Kitty),
                Response::Sixel if !distrusted && proto.is_none() => {
                    proto = Some(ProtocolType::Sixel);
                }
                Response::CellSize(Some((w, h))) => font = Some(FontSize::new(*w, *h)),
                _ => {}
            }
        }
        let proto = proto
            .or_else(|| iterm2_hint(is_tmux, var))
            .unwrap_or(ProtocolType::Halfblocks);
        let font = font.or_else(|| {
            cell_pixels
                .filter(|(w, h)| *w > 0 && *h > 0)
                .map(|(w, h)| FontSize::new(w, h))
        });
        match (proto, font) {
            (ProtocolType::Halfblocks, _) | (_, None) => Picker::halfblocks(),
            (proto, Some(font)) => {
                // the only constructor that takes a measured cell size (the other one runs
                // the query this module replaces); the protocol it guesses from the
                // environment is overridden by what the terminal answered
                #[allow(deprecated)]
                let mut p = Picker::from_fontsize(font);
                p.set_protocol_type(proto);
                p
            }
        }
    }
}

/// The key events of bytes the terminal query read but that were not its answer: the keys a
/// terminal in raw mode sends for them (printable text, Enter, Tab, Backspace, Ctrl-letter).
/// Nothing in them is an escape sequence (those were the answer), so no key with Alt or an
/// arrow is among them.
pub fn keys_of(bytes: &[u8]) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    for c in String::from_utf8_lossy(bytes).chars() {
        let k = match c {
            '\r' | '\n' => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            '\t' => KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            '\x7f' | '\x08' => KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            '\x1b' => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            c if (c as u32) >= 1 && (c as u32) <= 26 => KeyEvent::new(
                KeyCode::Char(char::from(b'a' + (c as u8) - 1)),
                KeyModifiers::CONTROL,
            ),
            c if c.is_control() || c == '\u{fffd}' => continue,
            c => KeyEvent::new(
                KeyCode::Char(c),
                if c.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                },
            ),
        };
        out.push(k);
    }
    out
}

/// The terminal's cell size in pixels from its window size (`TIOCGWINSZ`), when it reports one.
pub fn cell_pixels() -> Option<(u16, u16)> {
    let w = crossterm::terminal::window_size().ok()?;
    if w.width == 0 || w.height == 0 || w.columns == 0 || w.rows == 0 {
        return None;
    }
    Some((w.width / w.columns, w.height / w.rows))
}

/// Asks the terminal for its graphics protocol and cell size (see the module docs). Call it in
/// raw mode, on the alternate screen, BEFORE anything else reads the terminal: it writes the
/// query to stdout and polls stdin for the answer, at most `timeout` in all, and leaves no
/// reader behind. Returns the picker for the answer (halfblocks when the terminal answered
/// nothing, or nothing useful) and the bytes that were typed rather than answered, for
/// [`keys_of`].
#[cfg(unix)]
#[allow(clippy::disallowed_methods)] // the terminal answers on wall time
pub fn query_terminal(timeout: Duration) -> (Picker, Vec<u8>) {
    use std::io::Write as _;
    use std::time::Instant;

    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    let var = |k: &str| std::env::var(k).ok();
    // (this also turns tmux passthrough on, as ratatui-image does)
    let is_tmux = Picker::halfblocks().tmux_detected();
    let pixels = cell_pixels();
    let mut answer = QueryAnswer::new();
    let query = query_string(is_tmux, &var);
    let mut out = std::io::stdout();
    if out
        .write_all(query.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        return (Picker::halfblocks(), Vec::new());
    }
    let stdin = rustix::stdio::stdin();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let ts = Timespec {
            tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(left.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(&mut fds, Some(&ts)) {
            Ok(0) => break,
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => break,
        }
        let ready = fds[0].revents();
        if ready.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL)
            && !ready.contains(PollFlags::IN)
        {
            break;
        }
        match rustix::io::read(stdin, &mut buf[..]) {
            Ok(0) => break,
            Ok(n) => {
                if answer.push(&buf[..n]) {
                    break;
                }
            }
            Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => continue,
            Err(_) => break,
        }
    }
    let picker = answer.picker(is_tmux, &var, pixels);
    (picker, answer.typed().to_vec())
}

/// Without `poll` on stdin the terminal cannot be asked safely: halfblocks.
#[cfg(not(unix))]
pub fn query_terminal(_timeout: Duration) -> (Picker, Vec<u8>) {
    (Picker::halfblocks(), Vec::new())
}

/// The limits this module enforces (F35/F36, `limits.md` §6).
pub mod limits {
    use std::time::Duration;

    /// Largest input accepted, in bytes (`IMAGE_DECODE_MAX_INPUT_BYTES`).
    pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
    /// Largest side, in pixels (`IMAGE_DECODE_MAX_SIDE`).
    pub const MAX_SIDE: u32 = 8_192;
    /// The decoder's allocation limit (`IMAGE_DECODE_MAX_ALLOC`: one 8,192² RGBA frame).
    pub const MAX_ALLOC: u64 = 256 * 1024 * 1024;
    /// At most this many GIF frames are decoded (the terminal shows the first).
    pub const MAX_GIF_FRAMES: usize = 200;
    /// A decode that takes longer than this is abandoned (`IMAGE_DECODE_TIMEOUT_MS`).
    pub const DECODE_TIMEOUT: Duration = Duration::from_secs(10);
    /// Decoder threads that exist at once (an abandoned one counts until it ends); the other
    /// decodes wait their turn.
    pub const MAX_DECODING: usize = 2;
    /// Decoded images kept for drawing; past it the oldest is dropped.
    pub const MAX_SHOWN: usize = 8;
    /// A decoded image is scaled down to fit this many pixels a side: no terminal draws more,
    /// and it bounds what the kept images hold.
    pub const SHOWN_MAX_SIDE: u32 = 2_048;
}

/// Why a cell has no image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placeholder {
    /// Over a limit, or a frame size the header does not let us read: show why instead of
    /// decoding.
    TooLarge,
    /// The format is one this client will not decode.
    Unsupported,
    /// The image is being decoded off-thread.
    Loading,
    /// Decoding failed (corrupt, or past the timeout).
    Failed,
    /// Images are off (`LESSDUGIN_TUI_IMAGES=off`).
    Off,
}

impl Placeholder {
    /// The one-line text a cell shows in place of the image.
    pub fn text(self) -> &'static str {
        match self {
            Placeholder::TooLarge => "[image too large]",
            Placeholder::Unsupported => "[image: unsupported format]",
            Placeholder::Loading => "[image loading]",
            Placeholder::Failed => "[image failed to decode]",
            Placeholder::Off => "[images off]",
        }
    }
}

/// Decodes `bytes` under the caps, on the CALLER's thread ([`Images`] calls it on a worker).
///
/// Only PNG, JPEG, GIF and WebP are handed on, to the core's decode
/// ([`lessdugin_core::media::decode_image`]): SUP-9's header check, then the decoder under the
/// same caps as its own limits, the one policy every surface that decodes media uses.
///
/// # Errors
/// [`Placeholder::TooLarge`] past a cap (or for a header whose frame size cannot be read: it is
/// refused, F35/F36 fail closed), [`Placeholder::Unsupported`] for anything that is not a PNG,
/// JPEG, GIF or WebP, [`Placeholder::Failed`] for a file the decoder rejects.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage, Placeholder> {
    use image::ImageFormat;
    // the input cap comes before anything parses the bytes
    if bytes.len() > limits::MAX_INPUT_BYTES {
        return Err(Placeholder::TooLarge);
    }
    let format = image::guess_format(bytes).map_err(|_| Placeholder::Unsupported)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return Err(Placeholder::Unsupported);
    }
    // the header check before any decoder sees the bytes (`decode_image` makes it again)
    if !lessdugin_core::media::decoding_allowed(bytes) {
        return Err(Placeholder::TooLarge);
    }
    // a GIF decodes to its first frame here
    let img = lessdugin_core::media::decode_image(bytes).ok_or(Placeholder::Failed)?;
    if img.width() > limits::MAX_SIDE || img.height() > limits::MAX_SIDE {
        return Err(Placeholder::TooLarge);
    }
    if img.width().max(img.height()) > limits::SHOWN_MAX_SIDE {
        return Ok(img.thumbnail(limits::SHOWN_MAX_SIDE, limits::SHOWN_MAX_SIDE));
    }
    Ok(img)
}

/// Blob bytes on their way to the decoder (their `Debug` is their length, not their content).
#[derive(Clone, PartialEq, Eq)]
pub struct MediaBytes(pub Arc<Vec<u8>>);

impl std::fmt::Debug for MediaBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MediaBytes({} bytes)", self.0.len())
    }
}

/// Called from a worker thread when it has a result for [`Images::poll`]: the runtime wakes its
/// loop with it.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The decode function a worker runs ([`decode`]; tests hand in a slower one).
pub type Decoder = Arc<dyn Fn(&[u8]) -> Result<DynamicImage, Placeholder> + Send + Sync>;

/// One of the [`limits::MAX_DECODING`] decode slots. The supervisor and the decoder thread each
/// hold it, so it is handed back only once BOTH have ended: a decoder abandoned past the
/// timeout keeps it for as long as it runs. Handing it back wakes the loop, whose next
/// [`Images::poll`] starts a waiting decode.
struct Slot {
    live: Arc<AtomicUsize>,
    notify: Notify,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
        (self.notify)();
    }
}

/// One decoded image, ready to draw, and its resize worker's answers.
struct Shown {
    cid: String,
    protocol: ThreadProtocol,
    resized: Receiver<ResizeResponse>,
}

/// The images a terminal shows: the picker (queried once), the decodes under way and the
/// decoded images, by blob CID.
pub struct Images {
    /// `None`: nothing is decoded (a pipe, a test backend, images off).
    picker: Option<Picker>,
    /// Why, when nothing is decoded.
    disabled_as: Placeholder,
    notify: Notify,
    timeout: std::time::Duration,
    decoder: Decoder,
    done_tx: Sender<(String, Result<StatefulProtocol, Placeholder>)>,
    done_rx: Receiver<(String, Result<StatefulProtocol, Placeholder>)>,
    /// Decodes whose outcome [`Images::poll`] has not taken yet.
    running: usize,
    /// The decode slots taken ([`Slot`]): by the decodes running and the abandoned decoders
    /// that have not ended yet.
    live: Arc<AtomicUsize>,
    /// Decodes waiting for a turn.
    queue: VecDeque<(String, MediaBytes)>,
    /// Decoded, oldest first.
    shown: Vec<Shown>,
    /// Outcomes found by [`Images::decode`] itself (a cap, a disabled client).
    early: Vec<(String, Result<(), Placeholder>)>,
}

/// What [`Images::poll`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    /// Decodes that finished: shown, or why not.
    pub decoded: Vec<(String, Result<(), Placeholder>)>,
    /// Images dropped to keep at most [`limits::MAX_SHOWN`].
    pub evicted: Vec<String>,
    /// A resize finished: draw again.
    pub redraw: bool,
}

impl Images {
    /// Images drawn with `picker`'s protocol; `notify` wakes the event loop when a worker has a
    /// result.
    pub fn new(picker: Picker, notify: Notify) -> Self {
        Self::with(Some(picker), notify)
    }

    /// A client that never decodes: a headless test, or stdout that is a pipe.
    pub fn disabled() -> Self {
        Self::with(None, Arc::new(|| {}))
    }

    /// A client whose user turned images off (`LESSDUGIN_TUI_IMAGES=off`): nothing is decoded,
    /// and the cells say so.
    pub fn off() -> Self {
        Self {
            disabled_as: Placeholder::Off,
            ..Self::disabled()
        }
    }

    fn with(picker: Option<Picker>, notify: Notify) -> Self {
        let (done_tx, done_rx) = channel();
        Self {
            picker,
            disabled_as: Placeholder::Unsupported,
            notify,
            timeout: limits::DECODE_TIMEOUT,
            decoder: Arc::new(decode),
            done_tx,
            done_rx,
            running: 0,
            live: Arc::new(AtomicUsize::new(0)),
            queue: VecDeque::new(),
            shown: Vec::new(),
            early: Vec::new(),
        }
    }

    /// The same, with decodes abandoned after `timeout` instead of [`limits::DECODE_TIMEOUT`]
    /// (tests).
    #[must_use]
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The same, with the workers running `decoder` instead of [`decode`] (tests: a decode that
    /// takes as long as they want).
    #[doc(hidden)]
    #[must_use]
    pub fn with_decoder(mut self, decoder: Decoder) -> Self {
        self.decoder = decoder;
        self
    }

    /// Starts decoding the blob `cid` off the UI thread; [`Images::poll`] reports how it went.
    /// The input cap is checked here, before anything is handed on.
    pub fn decode(&mut self, cid: String, bytes: MediaBytes) {
        if self.picker.is_none() {
            self.early.push((cid, Err(self.disabled_as)));
            return;
        }
        if bytes.0.len() > limits::MAX_INPUT_BYTES {
            self.early.push((cid, Err(Placeholder::TooLarge)));
            return;
        }
        self.queue.push_back((cid, bytes));
        self.start_next();
    }

    fn start_next(&mut self) {
        while self.live.load(Ordering::SeqCst) < limits::MAX_DECODING {
            let Some((cid, bytes)) = self.queue.pop_front() else {
                return;
            };
            let Some(picker) = self.picker.clone() else {
                return;
            };
            let done = self.done_tx.clone();
            let notify = Arc::clone(&self.notify);
            let decode = Arc::clone(&self.decoder);
            let name = cid.clone();
            let timeout = self.timeout;
            self.live.fetch_add(1, Ordering::SeqCst);
            let slot = Arc::new(Slot {
                live: Arc::clone(&self.live),
                notify: Arc::clone(&self.notify),
            });
            // the supervisor waits for the decoder at most DECODE_TIMEOUT; a decoder past it
            // is left to finish on its own, and its image is dropped. Each holds the slot (a
            // thread that cannot start drops its closure, and the slot with it)
            let started = std::thread::Builder::new()
                .name("veles-image".to_owned())
                .spawn(move || {
                    let (tx, rx) = channel();
                    let held = Arc::clone(&slot);
                    let decoder = std::thread::Builder::new()
                        .name("veles-image-decode".to_owned())
                        .spawn(move || {
                            let result = decode(&bytes.0);
                            drop(bytes);
                            // an abandoned decoder's image is dropped here, then its slot
                            let _ = tx.send(result);
                            drop(held);
                        });
                    let result = match decoder {
                        Ok(_) => rx.recv_timeout(timeout).unwrap_or(Err(Placeholder::Failed)),
                        Err(_) => Err(Placeholder::Failed),
                    };
                    let _ = done.send((cid, result.map(|img| picker.new_resize_protocol(img))));
                    notify();
                    drop(slot);
                });
            match started {
                Ok(_) => self.running += 1,
                Err(_) => self.early.push((name, Err(Placeholder::Failed))),
            }
        }
    }

    /// The decode slots taken: decodes running, and abandoned decoders still running.
    pub fn decoding(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Takes what the workers finished, never blocking: decoded images become drawable (the
    /// oldest is dropped past [`limits::MAX_SHOWN`]), finished resizes are applied.
    pub fn poll(&mut self) -> Polled {
        let mut out = Polled {
            decoded: std::mem::take(&mut self.early),
            ..Polled::default()
        };
        while let Ok((cid, result)) = self.done_rx.try_recv() {
            self.running = self.running.saturating_sub(1);
            match result {
                Ok(protocol) => {
                    self.shown.retain(|s| s.cid != cid);
                    self.shown.push(self.start_resizer(cid.clone(), protocol));
                    out.decoded.push((cid, Ok(())));
                }
                Err(p) => out.decoded.push((cid, Err(p))),
            }
        }
        while self.shown.len() > limits::MAX_SHOWN {
            out.evicted.push(self.shown.remove(0).cid);
        }
        for s in &mut self.shown {
            while let Ok(resp) = s.resized.try_recv() {
                out.redraw |= s.protocol.update_resized_protocol(resp);
            }
        }
        self.start_next();
        out
    }

    /// The resize worker of one image: it lives as long as the image (it ends when the
    /// protocol, which holds its channel, is dropped), and a resize that fails leaves the
    /// image as it was rather than ending the worker.
    fn start_resizer(&self, cid: String, protocol: StatefulProtocol) -> Shown {
        let (tx, jobs) = channel::<ResizeRequest>();
        let (done, resized) = channel::<ResizeResponse>();
        let notify = Arc::clone(&self.notify);
        let _ = std::thread::Builder::new()
            .name("veles-image-resize".to_owned())
            .spawn(move || {
                for job in jobs {
                    match job.resize_encode() {
                        Ok(resp) => {
                            if done.send(resp).is_err() {
                                break;
                            }
                            notify();
                        }
                        Err(_) => continue,
                    }
                }
            });
        Shown {
            cid,
            protocol: ThreadProtocol::new(tx, Some(protocol)),
            resized,
        }
    }

    /// True when the blob `cid` is decoded and can be drawn.
    pub fn has(&self, cid: &str) -> bool {
        self.shown.iter().any(|s| s.cid == cid)
    }

    /// True when a decode is running or waiting.
    pub fn busy(&self) -> bool {
        self.running > 0 || !self.queue.is_empty()
    }

    /// Draws the image `cid` into `area` (through `StatefulImage`, which asks the worker for a
    /// resize when the area changed), or [`Placeholder::Loading`]'s text while there is none.
    /// Never decodes.
    pub fn render(&mut self, cid: &str, area: Rect, buf: &mut Buffer) {
        if let Some(s) = self.shown.iter_mut().find(|s| s.cid == cid) {
            ratatui::widgets::StatefulWidget::render(
                ratatui_image::StatefulImage::new(),
                area,
                buf,
                &mut s.protocol,
            );
            return;
        }
        buf.set_string(
            area.x,
            area.y,
            Placeholder::Loading.text(),
            Style::default().add_modifier(Modifier::DIM),
        );
    }
}

impl Default for Images {
    fn default() -> Self {
        Self::disabled()
    }
}
