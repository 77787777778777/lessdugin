// SPDX-License-Identifier: AGPL-3.0-or-later
//! The output budget (presentation, `docs/spec/lfm-1.md` §1.2): an upper-bound estimate of the
//! HTML each SL event produces, and a cut that keeps a document within [`OUTPUT_BUDGET`] bytes
//! of that estimate.
//!
//! The input is capped (1,000,000 bytes and 2^20 parser events), but some constructs still
//! expand: escaping (`&` → `&amp;`), fixed markup around short syntax (`[^1]` becomes a `sup`
//! and a back-link), MathML, and record fields such as `media.alt`. The budget runs on SL,
//! after the limit stage, so the HTML writer and the `StyledDoc` builder still consume the same
//! events. When the estimate would exceed the budget, the flow is cut there (every open tag is
//! closed, innermost first), the remaining footnotes are dropped and `flags.truncated` is set,
//! exactly as when the event cap truncates a document (§3 step 1).

use crate::event::{Ev, ImageRes, LinkRes, Tag, TagKind, match_indices};
use crate::lfm::Footnote;
use crate::media::MediaItem;

/// The most HTML bytes (as estimated by this module) one document may produce, MathML aside.
/// Real documents stay far below it: a 1,000,000-byte body of plain prose is estimated at about
/// 6 MB, and a document at the event cap made of the most expensive fixed markup at about
/// 30 MB. MathML has a budget of its own ([`crate::math::MATHML_BUDGET`]), applied by the HTML
/// writer: it depends on the engine, and this one must not.
pub(crate) const OUTPUT_BUDGET: usize = 48 << 20;

/// Escaped text: at most six bytes per input byte (`"` → `&quot;` in attributes).
fn esc(len: usize) -> usize {
    6 * len
}

fn image_cost(d: &crate::event::ImageData, media: &[MediaItem]) -> usize {
    match &d.res {
        ImageRes::Media(i) => {
            let m = &media[*i];
            let a = m.alt_text(&d.alt, d.fallback_alt).len();
            let name = m.name.as_ref().map_or(0, String::len);
            160 + m.cid().len() + esc(a) + esc(name)
        }
        ImageRes::External { url, host } => {
            160 + esc(url.len()) + esc(d.alt.len().max(host.len())) + esc(host.len())
        }
        ImageRes::Missing | ImageRes::AltOnly | ImageRes::Pending => 64 + esc(d.alt.len()),
    }
}

/// Start tags: the longest fixed markup of each (`<th class="ta-c" dir="auto">` is 28 bytes,
/// `<ol start="` with 20 digits 33, `<pre><code class="language-…">` 61, the spoiler's
/// `details` and `summary` 63) plus the escaped values it carries.
fn start_cost(tag: &Tag, media: &[MediaItem]) -> usize {
    match tag {
        Tag::Heading { id, .. } => 32 + id.len(),
        // The figcaption and its end tags are written at the figure's end.
        Tag::Figure(caption) => 64 + esc(caption.len()),
        Tag::CodeBlock(_) | Tag::Spoiler => 64,
        Tag::List(_) => 40,
        Tag::Link(d) => match &d.res {
            LinkRes::External { url, host } | LinkRes::Mailto { url, host } => {
                96 + esc(url.len()) + esc(host.len())
            }
            LinkRes::Record(u) => 64 + 2 * esc(u.as_str().len()),
            LinkRes::Profile(did) => 64 + 2 * esc(did.as_str().len()),
            // The writers replace the whole subtree with `@short_did`.
            LinkRes::Mention(did) => 128 + 3 * esc(did.as_str().len()),
            LinkRes::Anchor(v) => 32 + esc(v.len()),
            LinkRes::Dropped | LinkRes::Pending => 0,
        },
        Tag::Image(d) => image_cost(d, media),
        Tag::FootnoteDef { .. } => 0,
        _ => 32,
    }
}

/// End tags: `</tbody>\n</table>\n` (17 bytes) is the longest; a link's `link-host` span
/// (its host counted at the start) and a figure's caption markup are written there too.
fn end_cost(k: TagKind) -> usize {
    match k {
        TagKind::Link => 40,
        TagKind::Figure => 48,
        _ => 20,
    }
}

/// The estimate of one event (for an image or a mention, of its whole subtree).
fn cost(e: &Ev, media: &[MediaItem]) -> usize {
    match e {
        Ev::Start(tag) => start_cost(tag, media),
        Ev::End(k) => end_cost(*k),
        Ev::Text(t) | Ev::Literal(t) => esc(t.len()),
        Ev::Code(t) => 16 + esc(t.len()),
        // The error and Temml forms (escaped TeX in a `code` or `span`); MathML is budgeted by
        // the HTML writer.
        Ev::InlineMath(t) | Ev::DisplayMath(t) => 48 + esc(t.len()),
        // The `sup` (with two ordinals of up to 10 digits) and its back-link.
        Ev::FnRef(_) => 128,
        Ev::SoftBreak => 1,
        Ev::HardBreak | Ev::Rule => 8,
        Ev::Task(_) => 64,
    }
}

/// Cut `flow` where its estimate stops fitting in `left` (which is reduced by what is kept).
/// Returns whether it was cut.
fn cut(flow: &mut Vec<Ev>, left: &mut usize, media: &[MediaItem]) -> bool {
    let m = match_indices(flow);
    let mut open: Vec<TagKind> = Vec::new();
    let mut i = 0usize;
    while i < flow.len() {
        let e = &flow[i];
        let c = cost(e, media);
        if c > *left {
            flow.truncate(i);
            while let Some(k) = open.pop() {
                flow.push(Ev::End(k));
            }
            *left = 0;
            return true;
        }
        *left -= c;
        match e {
            // Leaves for the writers: their inner events are never written.
            Ev::Start(Tag::Image(_)) => {
                i = m[i].min(flow.len() - 1) + 1;
                continue;
            }
            Ev::Start(Tag::Link(d)) if matches!(d.res, LinkRes::Mention(_)) => {
                i = m[i].min(flow.len() - 1) + 1;
                continue;
            }
            Ev::Start(t) => open.push(t.kind()),
            Ev::End(_) => {
                open.pop();
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Apply the budget to a rendered document: the body, then the footnotes in order; the
/// gallery (`unused` media) is reserved first. Returns whether anything was cut.
pub(crate) fn apply(
    body: &mut Vec<Ev>,
    notes: &mut Vec<Footnote>,
    media: &[MediaItem],
    used: &[u16],
    budget: usize,
) -> bool {
    let gallery: usize = media
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.iter().any(|&u| usize::from(u) == *i))
        .map(|(_, m)| {
            let alt = m.alt.as_ref().map_or(0, String::len);
            let name = m.name.as_ref().map_or(0, String::len);
            192 + m.cid().len() + esc(alt) + esc(name)
        })
        .sum();
    // The root, the notice, the footnotes section and the gallery wrapper.
    let mut left = budget.saturating_sub(512 + gallery);
    if cut(body, &mut left, media) {
        notes.clear();
        return true;
    }
    for k in 0..notes.len() {
        // The `li`, its spoiler wrapper and its back-links' separators.
        let overhead = 256 + 2 * notes[k].refs as usize;
        if overhead > left {
            notes.truncate(k);
            return true;
        }
        left -= overhead;
        if cut(&mut notes[k].events, &mut left, media) {
            notes.truncate(k + 1);
            return true;
        }
    }
    false
}

/// The estimate of a whole rendered document (for tests: it must bound the HTML).
#[cfg(test)]
pub(crate) fn estimate(
    body: &[Ev],
    notes: &[Footnote],
    media: &[MediaItem],
    used: &[u16],
) -> usize {
    let gallery: usize = media
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.iter().any(|&u| usize::from(u) == *i))
        .map(|(_, m)| {
            192 + m.cid().len()
                + esc(m.alt.as_ref().map_or(0, String::len))
                + esc(m.name.as_ref().map_or(0, String::len))
        })
        .sum();
    let walk = |flow: &[Ev]| {
        let start = usize::MAX / 2;
        let mut l = start;
        let mut f = flow.to_vec();
        assert!(!cut(&mut f, &mut l, media));
        start - l
    };
    let mut spent = 512 + gallery + walk(body);
    for x in notes {
        spent += 256 + 2 * x.refs as usize + walk(&x.events);
    }
    spent
}
