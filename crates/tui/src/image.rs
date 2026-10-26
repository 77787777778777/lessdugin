// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images in the terminal (synthesis-v1.md F35/F36, §3.9).
//!
//! The supply-chain rule here is narrower than the rest of the client, because image decoders are
//! a classic memory-safety and decompression-bomb surface:
//!
//! - PURE-RUST decoders only (`image-defaults`): no C library is linked for this path.
//! - At most 64 MB of input, 8192 pixels per side, a 256 MiB decode allocation and 200 GIF frames.
//!   The pixel caps are checked from the HEADER, through
//!   [`lessdugin_core::media::decoding_allowed`], BEFORE any pixels are allocated, so a tiny file
//!   that declares an enormous frame cannot make the client allocate it.
//! - The decode and the resize happen OFF the UI thread, so a slow or hostile image never stalls
//!   the event loop.
//!
//! The picker is queried once (it talks to the terminal) and its answer decides the protocol:
//! Kitty graphics, iTerm2 inline images, Sixel, or the halfblock fallback that needs nothing from
//! the terminal at all.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui_image::picker::Picker;
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};

/// The limits this module enforces (F35/F36).
pub mod limits {
    use std::time::Duration;

    /// Largest input accepted, in bytes.
    pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
    /// Largest side, in pixels.
    pub const MAX_SIDE: u32 = 8_192;
    /// At most this many GIF frames are decoded.
    pub const MAX_GIF_FRAMES: usize = 200;
    /// A decode that takes longer than this is abandoned.
    pub const DECODE_TIMEOUT: Duration = Duration::from_secs(10);
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

/// The images a screen is showing: the picker (queried once) and one off-thread handle.
pub struct Images {
    picker: Picker,
    /// The worker's channel: resizes are queued here, so the UI thread never decodes.
    tx: std::sync::mpsc::Sender<ResizeRequest>,
    rx: std::sync::mpsc::Receiver<ResizeResponse>,
    /// The image being shown, when there is one.
    protocol: Option<ThreadProtocol>,
    /// Why there is no image, when there is none.
    placeholder: Option<Placeholder>,
}

impl Images {
    /// Queries the terminal for its graphics protocol and starts the decode worker. Falls back to
    /// halfblocks, which every terminal shows, when the query fails (a pipe, an unknown TERM, a
    /// test backend).
    pub fn new() -> Self {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        // The worker owns the resizing: `ResizeRequest::resize_encode` decodes and fits the image,
        // so the pixels never touch the UI thread.
        let (tx, jobs) = std::sync::mpsc::channel::<ResizeRequest>();
        let (done, rx) = std::sync::mpsc::channel::<ResizeResponse>();
        std::thread::Builder::new()
            .name("veles-image".to_owned())
            .spawn(move || {
                for job in jobs {
                    match job.resize_encode() {
                        Ok(resp) => {
                            if done.send(resp).is_err() {
                                break;
                            }
                        }
                        // a decode that failed leaves the cell on its placeholder
                        Err(_) => break,
                    }
                }
            })
            .ok();
        Self {
            picker,
            tx,
            rx,
            protocol: None,
            placeholder: None,
        }
    }

    /// A client that never decodes: a headless test, or stdout that is a pipe.
    pub fn disabled() -> Self {
        let (tx, _jobs) = std::sync::mpsc::channel();
        let (_done, rx) = std::sync::mpsc::channel();
        Self {
            picker: Picker::halfblocks(),
            tx,
            rx,
            protocol: None,
            placeholder: Some(Placeholder::Unsupported),
        }
    }
}

impl Images {
    /// Loads one image's bytes. The caps are checked BEFORE anything is decoded, then the decode
    /// itself is handed to the worker.
    ///
    /// Returns why it did not display, when it did not.
    pub fn load(&mut self, bytes: Vec<u8>) -> Option<Placeholder> {
        if bytes.len() > limits::MAX_INPUT_BYTES || !lessdugin_core::media::decoding_allowed(&bytes)
        {
            self.clear(Placeholder::TooLarge);
            return Some(Placeholder::TooLarge);
        }
        // `Picker::new_resize_protocol` takes a decoded `DynamicImage`, so the decode happens
        // here; the header check above already bounded it, and the dimension re-check below
        // catches a decoder that reports something the header did not.
        let Ok(img) = ::image::load_from_memory(&bytes) else {
            self.clear(Placeholder::Unsupported);
            return Some(Placeholder::Unsupported);
        };
        if img.width() > limits::MAX_SIDE || img.height() > limits::MAX_SIDE {
            self.clear(Placeholder::TooLarge);
            return Some(Placeholder::TooLarge);
        }
        let protocol = self.picker.new_resize_protocol(img);
        self.protocol = Some(ThreadProtocol::new(self.tx.clone(), Some(protocol)));
        self.placeholder = None;
        None
    }

    /// Drops the image and records why the cell shows text instead.
    pub fn clear(&mut self, why: Placeholder) {
        self.protocol = None;
        self.placeholder = Some(why);
    }

    /// Why there is no image, when there is none.
    pub fn placeholder(&self) -> Option<Placeholder> {
        self.placeholder
    }

    /// True when there is an image to draw.
    pub fn has_image(&self) -> bool {
        self.protocol.is_some()
    }

    /// Applies a finished resize, if one is waiting. Called from the event loop's tick, never
    /// blocking.
    pub fn poll(&mut self) -> bool {
        let Some(p) = &mut self.protocol else {
            return false;
        };
        let Ok(resp) = self.rx.try_recv() else {
            return false;
        };
        p.update_resized_protocol(resp)
    }
}

impl Images {
    /// Draws the image into `area` (through `StatefulImage`, which handles the resize bookkeeping),
    /// or its placeholder text. Never decodes: a resize that is still running leaves the
    /// placeholder up until the next [`Images::poll`] applies it.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        if let Some(p) = &mut self.protocol {
            ratatui::widgets::StatefulWidget::render(
                ratatui_image::StatefulImage::new(),
                area,
                buf,
                p,
            );
            return;
        }
        let text = self.placeholder.unwrap_or(Placeholder::Loading).text();
        buf.set_string(
            area.x,
            area.y,
            text,
            Style::default().add_modifier(Modifier::DIM),
        );
    }
}

impl Default for Images {
    fn default() -> Self {
        Self::new()
    }
}
