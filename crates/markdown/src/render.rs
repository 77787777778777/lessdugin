// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `render` entry point (`docs/spec/lfm-1.md` §1.3–§1.5) and derived metadata (§12).

use serde::Serialize;

use crate::at::{AtUri, Did};
use crate::event::{Ev, ImageRes, LinkRes, Tag, TagKind, match_indices};
use crate::html::{error_document, fixed, write_document};
use crate::lfm::{parse_to_s3, pass6, pass45, split_footnotes};
use crate::limit::{FOOTNOTE_BASE, limit};
use crate::math::{MATH_MODE, MathMode, MathRenderer};
use crate::media::{MediaItem, media_used};
use crate::plain::{excerpt, normalize, plain_raw, read_minutes, word_count};
use crate::styled::{StyledDoc, build, error_doc};

/// The presentation version (§1.2), part of the render-cache key `(cid, RENDERER_VERSION)`.
/// Builds without the `math` feature use the Temml fallback (§7.5); that mode is part of the
/// version, so it sets the top bit.
pub const RENDERER_VERSION: u32 = if matches!(MATH_MODE, MathMode::MathCore) {
    1
} else {
    1 | (1 << 31)
};

/// Sources longer than this are not parsed (§1.3).
pub const MAX_SOURCE_BYTES: usize = 1_000_000;

/// The only profile value (`NS.markdown.profile`).
pub const PROFILE: &str = "lfm/1";

/// Selects which outputs to compute. It never changes their content (§1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOpts {
    /// Compute `html`.
    pub html: bool,
    /// Compute `styled`.
    pub styled: bool,
    /// Compute `text` (P). The excerpt and counts are always computed.
    pub text: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self::ALL
    }
}

impl RenderOpts {
    /// Every output.
    pub const ALL: RenderOpts = RenderOpts {
        html: true,
        styled: true,
        text: true,
    };
}

/// Why a body could not be rendered (§1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
pub enum BodyError {
    /// The source is longer than 1,000,000 bytes.
    #[error("body too large to render")]
    TooLarge,
    /// A `textBlob` was not valid UTF-8.
    #[error("unreadable body: not valid UTF-8")]
    Encoding,
}

impl BodyError {
    /// The fixed notice string (§8.6).
    pub fn notice(self) -> &'static str {
        match self {
            BodyError::TooLarge => fixed::TOO_LARGE,
            BodyError::Encoding => fixed::ENCODING,
        }
    }
}

/// Rendering flags (§1.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Flags {
    /// The event cap was hit (§3).
    pub truncated: bool,
    /// The limit stage flattened something (§8.4).
    pub depth_limited: bool,
    /// The `profile` was not `lfm/1` (rendered as `lfm/1`; clients show a badge).
    pub unknown_profile: bool,
    /// Number of math errors rendered (§7.4).
    pub math_errors: u32,
}

/// A table-of-contents entry (§10.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TocEntry {
    /// 1–6.
    pub level: u8,
    /// The heading id (`md-h-…`).
    pub id: String,
    /// The normalised heading text.
    pub text: String,
}

/// Every output of [`render`] (§1.4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Rendered {
    /// HTML+MathML, root `<div class="lfm" dir="auto">` (§8). Empty unless requested.
    pub html: String,
    /// Terminal projection (§9). Empty unless requested.
    pub styled: StyledDoc,
    /// P (§10.1–10.2). Empty unless requested.
    pub text: String,
    /// The excerpt of P_x (§10.4).
    pub excerpt: String,
    /// Table of contents (§10.6).
    pub toc: Vec<TocEntry>,
    /// Word count (§10.5).
    pub word_count: u32,
    /// Read time (§10.5).
    pub read_minutes: u32,
    /// Record links: deduplicated, document order, at most 256 (§12).
    pub links: Vec<AtUri>,
    /// Mention DIDs: deduplicated, document order, at most 128 (§12).
    pub mentions: Vec<Did>,
    /// Indexes of `media[]` referenced by `cid:` URLs, ascending (§6.3).
    pub media_used: Vec<u16>,
    /// Every element id emitted in the HTML, in document order (§12).
    pub anchors: Vec<String>,
    /// Flags (§1.4).
    pub flags: Flags,
    /// The body error, if the body could not be rendered (§1.3).
    pub error: Option<BodyError>,
}

/// Render an LFM/1 source with its body media (`profile` = `lfm/1`).
pub fn render(src: &str, media: &[MediaItem], opts: &RenderOpts) -> Rendered {
    render_impl(src, media, None, opts, MATH_MODE)
}

/// Render with the `NS.markdown.profile` value. Absent or `"lfm/1"` renders normally; any
/// other value renders as `lfm/1` with `flags.unknown_profile`.
pub fn render_profile(
    src: &str,
    media: &[MediaItem],
    profile: Option<&str>,
    opts: &RenderOpts,
) -> Rendered {
    render_impl(src, media, profile, opts, MATH_MODE)
}

/// Render with an explicit math mode. Only for vector runners that check both the
/// math-core and the Temml output from one build; clients use [`render`].
#[doc(hidden)]
pub fn render_with_math_mode(
    src: &str,
    media: &[MediaItem],
    profile: Option<&str>,
    opts: &RenderOpts,
    mode: MathMode,
) -> Rendered {
    render_impl(src, media, profile, opts, mode)
}

/// The `Rendered` of a body that could not be rendered (§1.3): a single notice, empty text
/// and lists.
pub fn render_error(err: BodyError, profile: Option<&str>) -> Rendered {
    Rendered {
        html: error_document(err.notice()),
        styled: error_doc(err.notice()),
        flags: Flags {
            unknown_profile: profile.is_some_and(|p| p != PROFILE),
            ..Flags::default()
        },
        error: Some(err),
        ..Rendered::default()
    }
}

/// Decode a fetched `textBlob` (§1.3): UTF-8 whatever the declared charset, one leading
/// U+FEFF stripped; invalid UTF-8 is [`BodyError::Encoding`].
pub fn decode_text_blob(bytes: &[u8]) -> Result<String, BodyError> {
    let s = std::str::from_utf8(bytes).map_err(|_| BodyError::Encoding)?;
    Ok(s.strip_prefix('\u{FEFF}').unwrap_or(s).to_owned())
}

fn render_impl(
    src: &str,
    media: &[MediaItem],
    profile: Option<&str>,
    opts: &RenderOpts,
    mode: MathMode,
) -> Rendered {
    if src.len() > MAX_SOURCE_BYTES {
        return render_error(BodyError::TooLarge, profile);
    }
    let unknown_profile = profile.is_some_and(|p| p != PROFILE);
    let s3 = parse_to_s3(src);
    let truncated = s3.truncated;
    let mut s = s3.events;
    let p = normalize(&plain_raw(&s, false));
    let px = normalize(&plain_raw(&s, true));
    let wc = word_count(&p);

    pass45(&mut s, media);
    let refs = pass6(&mut s);

    // Derived metadata from S6 (§12, §6.3).
    let mut links: Vec<AtUri> = Vec::new();
    let mut mentions: Vec<Did> = Vec::new();
    let mut resolved: Vec<usize> = Vec::new();
    for e in &s {
        match e {
            Ev::Start(Tag::Link(d)) => match &d.res {
                LinkRes::Record(u) if links.len() < 256 && !links.contains(u) => {
                    links.push(u.clone())
                }
                LinkRes::Mention(did) if mentions.len() < 128 && !mentions.contains(did) => {
                    mentions.push(did.clone())
                }
                _ => {}
            },
            Ev::Start(Tag::Image(d)) => {
                if let ImageRes::Media(i) = d.res {
                    resolved.push(i);
                }
            }
            _ => {}
        }
    }
    let used = media_used(media, &resolved);
    let toc = toc(&s);

    let (body, notes) = split_footnotes(s, &refs);
    let (body, mut depth_limited) = limit(body, 0);
    let notes: Vec<_> = notes
        .into_iter()
        .map(|mut n| {
            let (ev, l) = limit(std::mem::take(&mut n.events), FOOTNOTE_BASE);
            depth_limited |= l;
            n.events = ev;
            n
        })
        .collect();

    let mut math = MathRenderer::new(mode);
    let out = write_document(&body, &notes, truncated, media, &used, &mut math);
    let styled = if opts.styled {
        build(&body, &notes, truncated, media, &used)
    } else {
        StyledDoc::default()
    };

    Rendered {
        html: if opts.html { out.html } else { String::new() },
        styled,
        excerpt: excerpt(&px),
        text: if opts.text { p } else { String::new() },
        toc,
        word_count: wc,
        read_minutes: read_minutes(wc),
        links,
        mentions,
        media_used: used,
        anchors: out.anchors,
        flags: Flags {
            truncated,
            depth_limited,
            unknown_profile,
            math_errors: out.math_errors,
        },
        error: None,
    }
}

/// Headings outside block quotes, spoilers, details, lists, tables and footnote definitions.
fn toc(s: &[Ev]) -> Vec<TocEntry> {
    let m = match_indices(s);
    let mut out = Vec::new();
    let mut nested = 0usize;
    for (i, e) in s.iter().enumerate() {
        match e {
            Ev::Start(t)
                if matches!(
                    t.kind(),
                    TagKind::BlockQuote
                        | TagKind::Spoiler
                        | TagKind::Details
                        | TagKind::List
                        | TagKind::Table
                        | TagKind::FootnoteDef
                ) =>
            {
                nested += 1
            }
            Ev::End(
                TagKind::BlockQuote
                | TagKind::Spoiler
                | TagKind::Details
                | TagKind::List
                | TagKind::Table
                | TagKind::FootnoteDef,
            ) => nested = nested.saturating_sub(1),
            Ev::Start(Tag::Heading { level, id }) if nested == 0 => {
                let text = normalize(&plain_raw(&s[i + 1..m[i].min(s.len())], false));
                out.push(TocEntry {
                    level: *level,
                    id: id.clone(),
                    text,
                });
            }
            _ => {}
        }
    }
    out
}
