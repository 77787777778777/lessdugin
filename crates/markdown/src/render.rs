// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `render` entry point (`docs/spec/lfm-1.md` §1.3–§1.5) and derived metadata (§12).

use serde::Serialize;

use crate::at::{AtUri, Did};
use crate::budget;
use crate::event::{Ev, ImageRes, LinkRes, Tag, TagKind, match_indices};
use crate::html::{error_document, fixed, write_document};
use crate::lfm::{Footnote, parse_to_s3, pass6, pass45, split_footnotes};
use crate::limit::{FOOTNOTE_BASE, limit};
use crate::math::{MATH_MODE, MathMode, MathRenderer};
use crate::media::{MediaItem, assign_alt_fallback, media_used};
use crate::plain::{excerpt, normalize, plain_raw, read_minutes, word_count};
use crate::styled::{StyledDoc, build, error_doc};

/// The presentation version (§1.2), part of the render-cache key `(cid, RENDERER_VERSION)`.
/// Builds without the `math` feature use the Temml fallback (§7.5); that mode is part of the
/// version, so it sets the top bit.
pub const RENDERER_VERSION: u32 = if matches!(MATH_MODE, MathMode::MathCore) {
    2
} else {
    2 | (1 << 31)
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

/// Everything both [`render`] and [`derive_metadata`] compute: P, P_x, the ToC, `media_used`,
/// and SL (the body and the rendered footnotes after the limit stage and the output budget)
/// with the links and mentions it contains.
struct Document {
    p: String,
    px: String,
    toc: Vec<TocEntry>,
    used: Vec<u16>,
    body: Vec<Ev>,
    notes: Vec<Footnote>,
    links: Vec<AtUri>,
    mentions: Vec<Did>,
    truncated: bool,
    depth_limited: bool,
}

fn document(src: &str, media: &[MediaItem]) -> Document {
    let s3 = parse_to_s3(src);
    let mut s = s3.events;
    let p = normalize(&plain_raw(&s, false));
    let px = normalize(&plain_raw(&s, true));

    pass45(&mut s, media);
    let refs = pass6(&mut s);

    // `media_used` from S6 (§6.3).
    let resolved: Vec<usize> = s
        .iter()
        .filter_map(|e| match e {
            Ev::Start(Tag::Image(d)) => match d.res {
                ImageRes::Media(i) => Some(i),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let used = media_used(media, &resolved);
    let toc = toc(&s);

    let (body, notes) = split_footnotes(s, &refs);
    let (mut body, mut depth_limited) = limit(body, 0);
    let mut notes: Vec<Footnote> = notes
        .into_iter()
        .map(|mut n| {
            // A definition inside a spoiler renders inside a `details` of its own.
            let base = FOOTNOTE_BASE + usize::from(n.in_spoiler);
            let (ev, l) = limit(std::mem::take(&mut n.events), base);
            depth_limited |= l;
            n.events = ev;
            n
        })
        .collect();
    assign_alt_fallback(
        std::iter::once(&mut body[..]).chain(notes.iter_mut().map(|n| &mut n.events[..])),
        media.len(),
    );
    let cut = budget::apply(&mut body, &mut notes, media, &used, budget::OUTPUT_BUDGET);

    // Links and mentions of what is rendered (§12): the body, then the rendered footnotes.
    // An unreferenced footnote definition or a link the limit stage removed is invisible, so
    // it must not notify anyone or count as a pingback.
    let mut links: Vec<AtUri> = Vec::new();
    let mut mentions: Vec<Did> = Vec::new();
    for e in body
        .iter()
        .chain(notes.iter().flat_map(|n| n.events.iter()))
    {
        if let Ev::Start(Tag::Link(d)) = e {
            match &d.res {
                LinkRes::Record(u) if links.len() < 256 && !links.contains(u) => {
                    links.push(u.clone())
                }
                LinkRes::Mention(did) if mentions.len() < 128 && !mentions.contains(did) => {
                    mentions.push(did.clone())
                }
                _ => {}
            }
        }
    }

    Document {
        p,
        px,
        toc,
        used,
        body,
        notes,
        links,
        mentions,
        truncated: s3.truncated || cut,
        depth_limited,
    }
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
    let d = document(src, media);
    let wc = word_count(&d.p);

    // The HTML writer also yields `anchors` and `flags.math_errors`, so it always runs; the
    // math engine is bounded by the guard of `math::engine_guard`. Callers that need neither
    // the HTML nor those two fields use `derive_metadata`, which never runs the engine.
    let mut math = MathRenderer::new(mode);
    let out = write_document(&d.body, &d.notes, d.truncated, media, &d.used, &mut math);
    let styled = if opts.styled {
        build(&d.body, &d.notes, d.truncated, media, &d.used)
    } else {
        StyledDoc::default()
    };

    Rendered {
        html: if opts.html { out.html } else { String::new() },
        styled,
        excerpt: excerpt(&d.px),
        text: if opts.text { d.p } else { String::new() },
        toc: d.toc,
        word_count: wc,
        read_minutes: read_minutes(wc),
        links: d.links,
        mentions: d.mentions,
        media_used: d.used,
        anchors: out.anchors,
        flags: Flags {
            truncated: d.truncated,
            depth_limited: d.depth_limited,
            unknown_profile,
            math_errors: out.math_errors,
        },
        error: None,
    }
}

/// The derived values of a body without its HTML or `StyledDoc` (§1.4, §10, §12).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Metadata {
    /// P (§10.1–10.2).
    pub text: String,
    /// P_x (§10.3): P without spoiler content. Search snippets shown to readers should come
    /// from it, so that they never reveal a spoiler.
    pub text_x: String,
    /// The excerpt of P_x (§10.4).
    pub excerpt: String,
    /// Table of contents (§10.6).
    pub toc: Vec<TocEntry>,
    /// Word count (§10.5).
    pub word_count: u32,
    /// Read time (§10.5).
    pub read_minutes: u32,
    /// Record links of the rendered document (§12).
    pub links: Vec<AtUri>,
    /// Mention DIDs of the rendered document (§12).
    pub mentions: Vec<Did>,
    /// Indexes of `media[]` referenced by `cid:` URLs, ascending (§6.3).
    pub media_used: Vec<u16>,
    /// The body error, if the body could not be rendered (§1.3).
    pub error: Option<BodyError>,
}

/// The derived values of [`render`] that writers, the indexer and the mention and pingback
/// pipelines need, without the HTML writer, the `StyledDoc` builder or the math engine. Every
/// field equals the field of the same name of [`render`] (`text_x` is
/// [`crate::plain_projection`]'s).
pub fn derive_metadata(src: &str, media: &[MediaItem]) -> Metadata {
    if src.len() > MAX_SOURCE_BYTES {
        return Metadata {
            error: Some(BodyError::TooLarge),
            ..Metadata::default()
        };
    }
    let d = document(src, media);
    let wc = word_count(&d.p);
    Metadata {
        excerpt: excerpt(&d.px),
        text: d.p,
        text_x: d.px,
        toc: d.toc,
        word_count: wc,
        read_minutes: read_minutes(wc),
        links: d.links,
        mentions: d.mentions,
        media_used: d.used,
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

#[cfg(test)]
mod tests {
    use super::*;

    const CID: &str = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku";

    fn media() -> Vec<MediaItem> {
        serde_json::from_value(serde_json::json!([
            {"blob": {"ref": {"$link": CID}, "mimeType": "image/png"}, "alt": "a\"<&>"},
            {"blob": {"ref": {"$link": CID}, "mimeType": "video/mp4"}, "name": "v&.mp4"},
            {"blob": {"ref": {"$link": CID}, "mimeType": "image/gif"}, "alt": "\"".repeat(300)}
        ]))
        .expect("media")
    }

    const TOKENS: &[&str] = &[
        "> ",
        "- ",
        "1. ",
        "12345678901234567890. ",
        "- [x] ",
        "# ",
        "###### ",
        "\n",
        "\n\n",
        "    ",
        "```rust x\n",
        "```",
        "`",
        "*",
        "_",
        "~~",
        "||",
        "$",
        "$$",
        "\\",
        "&",
        "\"",
        "<",
        "'",
        "[",
        "]",
        "(",
        ")",
        "![",
        "[^1]",
        "[^a]",
        "\n[^1]: ",
        "\n[^a]: ",
        "|",
        "|-|:-:|",
        "\n---\n",
        "  \n",
        "<b>",
        "&amp;",
        "https://e.x/\"",
        "<https://h.x/a>",
        "@alice.test",
        "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/a.b/c",
        "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa",
        "[x](at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa)",
        "[m](mailto:a@b.c)",
        "[i](#h)",
        "![\"](cid:bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku)",
        "![](https://img.x/a.png \"t\")",
        "\n:::spoiler\n",
        "\n:::\n",
        "\n> [!NOTE]\n> ",
        "word ",
        "\u{202e}",
        "\u{0}",
        "\t",
    ];

    /// Documents built from `TOKENS` by a fixed linear congruential generator.
    fn inputs() -> Vec<String> {
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut out = Vec::new();
        for n in 0..400 {
            let mut s = String::new();
            for _ in 0..(8 + n % 120) {
                x = x
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                s.push_str(TOKENS[(x >> 33) as usize % TOKENS.len()]);
            }
            out.push(s);
        }
        out.push("[^1]".repeat(500) + "\n\n[^1]: x");
        out.push("&\"".repeat(5000));
        out.push(
            "![](cid:bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku)".repeat(200),
        );
        out.push("|a|b|c|\n|:-:|-:|:-|\n".to_owned() + &"|1|2|3|\n".repeat(300));
        out.push("> ".repeat(40) + "- " + &"1. ".repeat(40) + "x");
        out
    }

    #[test]
    fn estimate_bounds_html() {
        let media = media();
        for src in inputs() {
            let d = document(&src, &media);
            let est = budget::estimate(&d.body, &d.notes, &media, &d.used);
            let mut math = MathRenderer::new(MathMode::Temml);
            let html = write_document(&d.body, &d.notes, d.truncated, &media, &d.used, &mut math);
            assert!(
                html.html.len() <= est,
                "{} > {est} for {src:?}",
                html.html.len()
            );
        }
    }

    fn balanced(evs: &[Ev]) -> bool {
        let mut open: Vec<TagKind> = Vec::new();
        for e in evs {
            match e {
                Ev::Start(t) => open.push(t.kind()),
                Ev::End(k) if open.pop() != Some(*k) => return false,
                _ => {}
            }
        }
        open.is_empty()
    }

    #[test]
    fn budget_cut() {
        let media = media();
        let src = "> - **a *b `c` d* e**\n>   1. x\n\n".repeat(50) + "y[^1]\n\n[^1]: note";
        let d = document(&src, &media);
        assert!(!d.truncated && d.notes.len() == 1);
        let (mut body, mut notes) = (d.body.clone(), d.notes);
        let full = budget::estimate(&body, &notes, &media, &d.used);
        for budget in [0, 600, 2_000, full / 2, full - 1] {
            let mut b = body.clone();
            let mut n: Vec<Footnote> = notes
                .iter()
                .map(|x| Footnote {
                    ord: x.ord,
                    refs: x.refs,
                    events: x.events.clone(),
                    in_spoiler: x.in_spoiler,
                })
                .collect();
            assert!(budget::apply(&mut b, &mut n, &media, &d.used, budget));
            assert!(balanced(&b), "{budget}");
            assert!(n.iter().all(|x| balanced(&x.events)));
            assert!(budget::estimate(&b, &n, &media, &d.used) <= full);
            if b.len() < body.len() {
                assert!(n.is_empty(), "notes dropped after a body cut");
            }
        }
        assert!(!budget::apply(&mut body, &mut notes, &media, &d.used, full));
    }
}
