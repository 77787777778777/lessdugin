// SPDX-License-Identifier: AGPL-3.0-or-later
//! LFM/1 (`lfm/1`): the lessdugin markdown profile.
//!
//! One event pipeline produces every output — sanitised HTML+MathML, the terminal
//! [`StyledDoc`] made of [`TermText`] spans, the plain-text projection P used for quote
//! anchors, excerpts and search, and derived metadata (ToC, record links, mentions, media).
//! Normative source: `docs/spec/lfm-1.md`.
//!
//! The crate is wasm-safe: no I/O, clocks or threads, and every stage is an iterative walk
//! over an event vector with explicit stacks (no recursion proportional to input nesting; the
//! recursive math engine only sees expressions that pass a nesting guard, see [`math`]).
//!
//! | Item | Spec |
//! |---|---|
//! | [`render`], [`Rendered`], [`RenderOpts`], [`RENDERER_VERSION`], [`derive_metadata`] | §1, §12 |
//! | [`safe_href`], [`Href`], [`short_did`], [`preview_eligible`] | §5 |
//! | [`MediaItem`] (`defs#media`) | §6 |
//! | [`math`] (pre-scan, MathML filter, Temml mode) | §7 |
//! | [`html`] (fixed strings), [`allowlist`] (tables, checker, DOMPurify/ammonia configs) | §8 |
//! | [`StyledDoc`], [`term_sanitize`], [`printer`] (plain stdout, §9.4) | §9 |
//! | [`plain_projection`], [`excerpt`], [`word_count`], [`read_minutes`] | §10 |
//! | [`resolve_quote`], [`qkey`], [`build_quote`] | §11 |
//!
//! ```
//! use lessdugin_markdown::{render, RenderOpts};
//! let r = render("# Hi\n\nSee [docs](https://example.com).", &[], &RenderOpts::ALL);
//! assert!(r.html.starts_with("<div class=\"lfm\" dir=\"auto\">"));
//! assert_eq!(r.text, "Hi See docs.");
//! assert_eq!(r.toc[0].id, "md-h-hi");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod allowlist;
mod at;
mod budget;
mod event;
pub mod html;
mod lfm;
mod limit;
pub mod math;
mod media;
pub mod plain;
pub mod printer;
pub mod quote;
mod render;
pub mod styled;
pub mod term;
pub mod url;

pub use at::{AtUri, Did};
pub use event::{AlertKind, Align};
pub use lfm::{EVENT_CAP, OPTIONS};
pub use math::{MATH_MODE, MathMode};
pub use media::{AspectRatio, BlobRef, CidLink, MediaItem};
pub use plain::{PlainProjection, excerpt, plain_projection, read_minutes, word_count};
pub use quote::{
    OwnedTextQuote, QuoteBuildError, QuoteResolution, QuoteTarget, TextQuote, build_quote, qkey,
    resolve_quote,
};
pub use render::{
    BodyError, Flags, MAX_SOURCE_BYTES, Metadata, PROFILE, RENDERER_VERSION, RenderOpts, Rendered,
    TocEntry, decode_text_blob, derive_metadata, render, render_error, render_profile,
    render_with_math_mode,
};
pub use styled::{Gutter, SBlock, SKind, SLine, SLink, Span, Style, StyledDoc};
pub use term::{RunKind, TermCtx, TermRun, TermText, term_sanitize};
pub use url::{Href, HrefCtx, preview_eligible, safe_href, short_did, term_shows_host};
