// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images in the terminal (synthesis-v1.md F35/F36, §3.9; `threat-model.md` SUP-9).
//!
//! The supply-chain rule here is narrower than the rest of the client, because image decoders are
//! a classic memory-safety and decompression-bomb surface:
//!
//! - PURE-RUST decoders only (`image-defaults`): no C library is linked for this path.
//! - At most 64 MiB of input, 8192 pixels per side, a 256 MiB decode allocation and 200 GIF frames
//!   (`limits.md` §6). The pixel caps are checked from the HEADER, through
//!   [`lessdugin_core::media::decoding_allowed`], BEFORE any pixels are allocated, and again by
//!   the decoder's own [`image::Limits`], so a header the pre-check cannot read does not get
//!   past them either. A GIF is shown as its first frame: one of the at most 200 decoded.
//! - The decode and the resize happen OFF the UI thread. A supervisor thread gives each decode
//!   [`limits::DECODE_TIMEOUT`]; past it the image is abandoned (its placeholder says so) and
//!   whatever the decoder still produces is dropped.
//! - Decoded images are scaled down to [`limits::SHOWN_MAX_SIDE`] and at most
//!   [`limits::MAX_SHOWN`] are kept; the oldest is dropped first ([`Images::poll`] says which).
//!
//! The picker is queried once (it talks to the terminal) and its answer decides the protocol:
//! Kitty graphics, iTerm2 inline images, Sixel, or the halfblock fallback that needs nothing from
//! the terminal at all. Which media load, and when, is the state's business (the autoload policy,
//! `sync.md` §11.3); this module only decodes and draws what it is handed.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};

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
    /// Decodes that run at once; the others wait their turn.
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
    /// Over a limit: show why instead of decoding.
    TooLarge,
    /// The format is one this client will not decode.
    Unsupported,
    /// The image is being decoded off-thread.
    Loading,
    /// Decoding failed (corrupt, or past the timeout).
    Failed,
}

impl Placeholder {
    /// The one-line text a cell shows in place of the image.
    pub fn text(self) -> &'static str {
        match self {
            Placeholder::TooLarge => "[image too large]",
            Placeholder::Unsupported => "[image: unsupported format]",
            Placeholder::Loading => "[image loading]",
            Placeholder::Failed => "[image failed to decode]",
        }
    }
}

/// Decodes `bytes` under the caps, on the CALLER's thread ([`Images`] calls it on a worker).
///
/// # Errors
/// [`Placeholder::TooLarge`] past a cap, [`Placeholder::Unsupported`] for anything that is not a
/// PNG, JPEG, GIF or WebP, [`Placeholder::Failed`] for a file the decoder rejects.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage, Placeholder> {
    // the input cap and the header check come before any decoder sees the bytes
    if bytes.len() > limits::MAX_INPUT_BYTES || !lessdugin_core::media::decoding_allowed(bytes) {
        return Err(Placeholder::TooLarge);
    }
    capped_decode(bytes)
}

/// The decode itself, under the decoder's own [`image::Limits`]: they hold even for a header
/// [`lessdugin_core::media::decoding_allowed`] could not read (it lets those through).
///
/// # Errors
/// As [`decode`].
pub fn capped_decode(bytes: &[u8]) -> Result<DynamicImage, Placeholder> {
    use image::{ImageFormat, ImageReader, Limits};
    let format = image::guess_format(bytes).map_err(|_| Placeholder::Unsupported)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return Err(Placeholder::Unsupported);
    }
    let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut caps = Limits::default();
    caps.max_image_width = Some(limits::MAX_SIDE);
    caps.max_image_height = Some(limits::MAX_SIDE);
    caps.max_alloc = Some(limits::MAX_ALLOC);
    reader.limits(caps);
    // a GIF decodes to its first frame here
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => Placeholder::TooLarge,
        image::ImageError::Unsupported(_) => Placeholder::Unsupported,
        _ => Placeholder::Failed,
    })?;
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

/// One decoded image, ready to draw, and its resize worker's answers.
struct Shown {
    cid: String,
    protocol: ThreadProtocol,
    resized: Receiver<ResizeResponse>,
}

/// The images a terminal shows: the picker (queried once), the decodes under way and the
/// decoded images, by blob CID.
pub struct Images {
    /// `None`: nothing is decoded (a pipe, a test backend).
    picker: Option<Picker>,
    notify: Notify,
    timeout: std::time::Duration,
    done_tx: Sender<(String, Result<StatefulProtocol, Placeholder>)>,
    done_rx: Receiver<(String, Result<StatefulProtocol, Placeholder>)>,
    /// Decodes running now.
    running: usize,
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

    /// Queries the terminal for its graphics protocol (call it in raw mode, before anything
    /// else reads the terminal's input). Falls back to halfblocks, which every terminal shows,
    /// when the query fails (an unknown TERM, no answer).
    pub fn query(notify: Notify) -> Self {
        Self::new(
            Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks()),
            notify,
        )
    }

    /// A client that never decodes: a headless test, or stdout that is a pipe.
    pub fn disabled() -> Self {
        Self::with(None, Arc::new(|| {}))
    }

    fn with(picker: Option<Picker>, notify: Notify) -> Self {
        let (done_tx, done_rx) = channel();
        Self {
            picker,
            notify,
            timeout: limits::DECODE_TIMEOUT,
            done_tx,
            done_rx,
            running: 0,
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

    /// Starts decoding the blob `cid` off the UI thread; [`Images::poll`] reports how it went.
    /// The input cap is checked here, before anything is handed on.
    pub fn decode(&mut self, cid: String, bytes: MediaBytes) {
        if self.picker.is_none() {
            self.early.push((cid, Err(Placeholder::Unsupported)));
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
        while self.running < limits::MAX_DECODING {
            let Some((cid, bytes)) = self.queue.pop_front() else {
                return;
            };
            let Some(picker) = self.picker.clone() else {
                return;
            };
            let done = self.done_tx.clone();
            let notify = Arc::clone(&self.notify);
            let name = cid.clone();
            let timeout = self.timeout;
            // the supervisor waits for the decoder at most DECODE_TIMEOUT; a decoder past it
            // is left to finish on its own, and its image is dropped
            let started = std::thread::Builder::new()
                .name("veles-image".to_owned())
                .spawn(move || {
                    let (tx, rx) = channel();
                    let decoder = std::thread::Builder::new()
                        .name("veles-image-decode".to_owned())
                        .spawn(move || {
                            let _ = tx.send(decode(&bytes.0));
                        });
                    let result = match decoder {
                        Ok(_) => rx.recv_timeout(timeout).unwrap_or(Err(Placeholder::Failed)),
                        Err(_) => Err(Placeholder::Failed),
                    };
                    let _ = done.send((cid, result.map(|img| picker.new_resize_protocol(img))));
                    notify();
                });
            match started {
                Ok(_) => self.running += 1,
                Err(_) => self.early.push((name, Err(Placeholder::Failed))),
            }
        }
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
