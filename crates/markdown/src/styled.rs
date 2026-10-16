// SPDX-License-Identifier: AGPL-3.0-or-later
//! `StyledDoc` (`docs/spec/lfm-1.md` §9.2–§9.3): the terminal projection of SL, made of
//! [`TermText`] spans only. Built by one forward iteration with explicit stacks.

use serde::{Serialize, Serializer};

use crate::at::{AtUri, Did};
use crate::event::{
    AlertKind, Align, Ev, ImageData, ImageRes, LinkRes, Tag, TagKind, match_indices,
};
use crate::html::{code_lang, fixed};
use crate::lfm::Footnote;
use crate::media::{MediaClass, MediaItem};
use crate::term::{TermCtx, TermText, term_sanitize};
use crate::url::short_did;

/// A styled document: flat blocks in document order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct StyledDoc {
    /// The blocks.
    pub blocks: Vec<SBlock>,
}

/// One block with its container gutters (outermost first, at most 64).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SBlock {
    /// Container gutters, outermost first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gutters: Vec<Gutter>,
    /// The block.
    pub kind: SKind,
}

/// A container gutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Gutter {
    /// A block quote.
    Quote,
    /// A GFM alert.
    Alert(AlertKind),
    /// Spoiler content (hidden until revealed); the id matches its `Summary`.
    Spoiler(u32),
    /// Details content; the id matches its `Summary`.
    Details(u32),
    /// A list item. `first` only on the item's first block; `ordinal` is `None` for bullets.
    Item {
        /// The item number for ordered lists.
        ordinal: Option<u64>,
        /// True only on the item's first block.
        first: bool,
    },
    /// Footnote `N` content.
    Footnote(u32),
}

/// A line of spans.
pub type SLine = Vec<Span>;

/// The block kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SKind {
    /// Prose; adapters re-wrap each line to the width.
    Para(Vec<SLine>),
    /// A heading.
    Heading {
        /// 1–6.
        level: u8,
        /// The element id (`md-h-…`).
        id: String,
        /// The heading text.
        line: SLine,
    },
    /// A code block, never re-wrapped.
    Code {
        /// The language token, if valid.
        lang: Option<TermText>,
        /// The lines (Code context).
        lines: Vec<TermText>,
    },
    /// Display-math TeX lines.
    Math(Vec<TermText>),
    /// A table.
    Table {
        /// Column alignments.
        aligns: Vec<Align>,
        /// Header cells.
        head: Vec<SLine>,
        /// Body rows.
        rows: Vec<Vec<SLine>>,
    },
    /// A thematic break.
    Rule,
    /// The summary line of a spoiler or details block.
    Summary {
        /// Matches the content's `Spoiler(id)`/`Details(id)` gutter.
        id: u32,
        /// True for spoilers.
        spoiler: bool,
        /// The summary text.
        line: SLine,
    },
    /// Block-level media (figure, lone `cid:` image, gallery item).
    Media {
        /// Index into `media[]`.
        index: u16,
        /// The placeholder line.
        line: SLine,
    },
    /// A figure caption.
    Caption(SLine),
    /// A notice (truncation, body errors).
    Notice(TermText),
}

/// A styled run of text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Span {
    /// The sanitised text.
    pub text: TermText,
    /// Style flags.
    #[serde(skip_serializing_if = "Style::is_empty")]
    pub style: Style,
    /// The link target, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<SLink>,
}

/// A span's link target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SLink {
    /// `External` or `Mailto` from `safe_href`: the only kind printers may emit OSC 8 for.
    External {
        /// The normalised URL.
        url: String,
        /// The host to show (F1).
        host: String,
    },
    /// An internal record route.
    Record(AtUri),
    /// A profile route.
    Profile(Did),
    /// A mention (display layers substitute the verified handle).
    Mention(Did),
    /// An in-document anchor.
    Anchor(String),
    /// A media item.
    Media(u16),
}

/// Span style flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style(u16);

impl Style {
    /// Emphasis.
    pub const EMPH: Style = Style(1);
    /// Strong.
    pub const STRONG: Style = Style(1 << 1);
    /// Strikethrough.
    pub const STRIKE: Style = Style(1 << 2);
    /// Inline code.
    pub const CODE: Style = Style(1 << 3);
    /// Inline math.
    pub const MATH: Style = Style(1 << 4);
    /// A link.
    pub const LINK: Style = Style(1 << 5);
    /// A mention.
    pub const MENTION: Style = Style(1 << 6);
    /// A footnote reference.
    pub const FOOTREF: Style = Style(1 << 7);
    /// A task-list marker.
    pub const TASK: Style = Style(1 << 8);
    /// A media placeholder.
    pub const PLACEHOLDER: Style = Style(1 << 9);
    /// A notice.
    pub const NOTICE: Style = Style(1 << 10);

    const NAMES: [(Style, &'static str); 11] = [
        (Style::EMPH, "EMPH"),
        (Style::STRONG, "STRONG"),
        (Style::STRIKE, "STRIKE"),
        (Style::CODE, "CODE"),
        (Style::MATH, "MATH"),
        (Style::LINK, "LINK"),
        (Style::MENTION, "MENTION"),
        (Style::FOOTREF, "FOOTREF"),
        (Style::TASK, "TASK"),
        (Style::PLACEHOLDER, "PLACEHOLDER"),
        (Style::NOTICE, "NOTICE"),
    ];

    /// No flags.
    pub const fn empty() -> Self {
        Style(0)
    }

    /// True if no flag is set.
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// True if every flag of `other` is set.
    pub fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }

    /// The union.
    #[must_use]
    pub const fn union(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }

    /// The flag names, in declaration order.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(f, _)| self.contains(*f))
            .map(|(_, n)| *n)
            .collect()
    }
}

impl std::ops::BitOr for Style {
    type Output = Style;
    fn bitor(self, rhs: Style) -> Style {
        self.union(rhs)
    }
}

impl Serialize for Style {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.names().serialize(s)
    }
}

fn prose(s: &str) -> TermText {
    term_sanitize(s, TermCtx::Prose)
}

/// The terminal placeholder (§6.2, §8.6) for media item `i` with alt `a`.
pub(crate) fn media_placeholder(media: &[MediaItem], i: usize, a: &str) -> TermText {
    let m = &media[i];
    let n = i + 1;
    let (kind, body) = match m.class() {
        MediaClass::Image => ("image", prose(a)),
        class => {
            let kind = match class {
                MediaClass::Video => "video",
                MediaClass::Audio => "audio",
                _ => "file",
            };
            if a.is_empty() {
                let name = m.label("");
                (
                    kind,
                    term_sanitize(&name, TermCtx::Name { max_graphemes: 0 }),
                )
            } else {
                (kind, prose(a))
            }
        }
    };
    let mut t = prose(&format!("[{kind} {n}"));
    if !body.is_empty() {
        t.append(prose(": "));
        t.append(body);
    }
    t.append(prose("]"));
    t
}

fn missing_placeholder(alt: &str) -> TermText {
    let mut t = prose("[missing media");
    if !alt.is_empty() {
        t.append(prose(": "));
        t.append(prose(alt));
    }
    t.append(prose("]"));
    t
}

fn ext_image_placeholder(alt: &str, host: &str) -> TermText {
    let mut t = prose("[image");
    if !alt.is_empty() {
        t.append(prose(": "));
        t.append(prose(alt));
    }
    t.append(prose(&format!(" \u{27E8}{host}\u{27E9}]")));
    t
}

#[derive(Clone, Copy)]
enum Ctx {
    Flow,
    Heading,
    Summary,
    Cell,
    Code,
}

struct ListState {
    start: Option<u64>,
    idx: u64,
}

struct TableBuild {
    aligns: Vec<Align>,
    head: Vec<SLine>,
    rows: Vec<Vec<SLine>>,
    row: Vec<SLine>,
}

struct Builder<'a> {
    blocks: Vec<SBlock>,
    gutters: Vec<Gutter>,
    lines: Vec<SLine>,
    line: SLine,
    media: &'a [MediaItem],
    next_summary_id: u32,
    /// A soft break not yet materialised as a `" "` span: dropped at the end of a line or
    /// paragraph and before display math.
    pending_space: Option<(Style, Option<SLink>)>,
}

impl Builder<'_> {
    fn emit(&mut self, kind: SKind) {
        let gutters = self.gutters.clone();
        for g in &mut self.gutters {
            if let Gutter::Item { first, .. } = g {
                *first = false;
            }
        }
        self.blocks.push(SBlock { gutters, kind });
    }

    fn push_span(&mut self, text: TermText, style: Style, link: Option<SLink>) {
        if text.is_empty() {
            return;
        }
        if let Some((s, l)) = self.pending_space.take() {
            self.push_span(prose(" "), s, l);
        }
        if let Some(last) = self.line.last_mut()
            && last.style == style
            && last.link == link
        {
            last.text.append(text);
            return;
        }
        self.line.push(Span { text, style, link });
    }

    fn take_line(&mut self) -> SLine {
        self.pending_space = None;
        std::mem::take(&mut self.line)
    }

    /// Emit pending flow inline content as a `Para`.
    fn flush_para(&mut self) {
        self.pending_space = None;
        if !self.line.is_empty() {
            let l = self.take_line();
            self.lines.push(l);
        }
        if !self.lines.is_empty() {
            let lines = std::mem::take(&mut self.lines);
            self.emit(SKind::Para(lines));
        }
    }

    fn image_span(&mut self, d: &ImageData, style: Style, link: Option<SLink>) {
        let st = style | Style::PLACEHOLDER;
        let alt = d.alt.as_str();
        match &d.res {
            ImageRes::Media(i) => {
                let a = self.media[*i].alt_text(alt, d.fallback_alt);
                let t = media_placeholder(self.media, *i, &a);
                let l = link.or(Some(SLink::Media(u16::try_from(*i).unwrap_or(u16::MAX))));
                self.push_span(t, st, l);
            }
            ImageRes::Missing => self.push_span(missing_placeholder(alt), st, link),
            ImageRes::External { url, host } => match link {
                // Inside a link the image is not a link of its own (as in HTML): no image host.
                Some(outer) => {
                    let mut t = prose("[image");
                    if !alt.is_empty() {
                        t.append(prose(": "));
                        t.append(prose(alt));
                    }
                    t.append(prose("]"));
                    self.push_span(t, st, Some(outer));
                }
                None => {
                    let l = Some(SLink::External {
                        url: url.clone(),
                        host: host.clone(),
                    });
                    self.push_span(ext_image_placeholder(alt, host), st, l);
                }
            },
            ImageRes::AltOnly | ImageRes::Pending => self.push_span(prose(alt), style, link),
        }
    }

    fn flow(&mut self, evs: &[Ev]) {
        let m = match_indices(evs);
        let mut styles: Vec<Style> = vec![Style::empty()];
        let mut links: Vec<Option<SLink>> = vec![None];
        let mut ctx: Vec<Ctx> = vec![Ctx::Flow];
        let mut heading: Vec<(u8, String)> = Vec::new();
        let mut summaries: Vec<(u32, bool)> = Vec::new();
        let mut pending_details: Vec<u32> = Vec::new();
        let mut lists: Vec<ListState> = Vec::new();
        let mut tables: Vec<TableBuild> = Vec::new();
        let mut code: Vec<(Option<String>, String)> = Vec::new();
        let mut figures: Vec<String> = Vec::new();
        // Per open tag: what its End must undo.
        #[derive(Clone, Copy)]
        enum Undo {
            Nothing,
            Style,
            Link,
            Gutter,
            Item,
            Para,
        }
        let mut undo: Vec<Undo> = Vec::new();
        let mut i = 0usize;
        while i < evs.len() {
            let style = *styles.last().unwrap_or(&Style::empty());
            let link = links.last().cloned().flatten();
            match &evs[i] {
                Ev::Start(tag) => match tag {
                    Tag::Paragraph => {
                        self.flush_para();
                        // A paragraph holding only one cid: image hit is a Media block.
                        let end = m[i];
                        if end < evs.len()
                            && i + 1 < end
                            && let Ev::Start(Tag::Image(d)) = &evs[i + 1]
                            && m[i + 1] + 1 == end
                            && let ImageRes::Media(idx) = d.res
                        {
                            let a = self.media[idx].alt_text(&d.alt, d.fallback_alt);
                            let t = media_placeholder(self.media, idx, &a);
                            let ix = u16::try_from(idx).unwrap_or(u16::MAX);
                            let line = vec![Span {
                                text: t,
                                style: Style::PLACEHOLDER,
                                link: Some(SLink::Media(ix)),
                            }];
                            self.emit(SKind::Media { index: ix, line });
                            i = end + 1;
                            continue;
                        }
                        undo.push(Undo::Para);
                    }
                    Tag::Heading { level, id } => {
                        self.flush_para();
                        heading.push((*level, id.clone()));
                        ctx.push(Ctx::Heading);
                        undo.push(Undo::Nothing);
                    }
                    Tag::BlockQuote(k) => {
                        self.flush_para();
                        self.gutters.push(match k {
                            None => Gutter::Quote,
                            Some(k) => Gutter::Alert(*k),
                        });
                        undo.push(Undo::Gutter);
                    }
                    Tag::Spoiler => {
                        self.flush_para();
                        self.next_summary_id += 1;
                        let id = self.next_summary_id;
                        let line = vec![Span {
                            text: prose(fixed::SPOILER),
                            style: Style::empty(),
                            link: None,
                        }];
                        self.emit(SKind::Summary {
                            id,
                            spoiler: true,
                            line,
                        });
                        self.gutters.push(Gutter::Spoiler(id));
                        undo.push(Undo::Gutter);
                    }
                    Tag::Details => {
                        self.flush_para();
                        self.next_summary_id += 1;
                        pending_details.push(self.next_summary_id);
                        undo.push(Undo::Nothing);
                    }
                    Tag::Summary => {
                        self.flush_para();
                        let id = pending_details.last().copied().unwrap_or(0);
                        summaries.push((id, false));
                        ctx.push(Ctx::Summary);
                        undo.push(Undo::Nothing);
                    }
                    Tag::Figure(c) => {
                        self.flush_para();
                        figures.push(c.clone());
                        undo.push(Undo::Nothing);
                    }
                    Tag::CodeBlock(info) => {
                        self.flush_para();
                        code.push((info.clone(), String::new()));
                        ctx.push(Ctx::Code);
                        undo.push(Undo::Nothing);
                    }
                    Tag::List(start) => {
                        self.flush_para();
                        lists.push(ListState {
                            start: *start,
                            idx: 0,
                        });
                        undo.push(Undo::Nothing);
                    }
                    Tag::Item => {
                        self.flush_para();
                        let ordinal = lists.last_mut().and_then(|l| {
                            let o = l.start.map(|s| s.saturating_add(l.idx));
                            l.idx += 1;
                            o
                        });
                        self.gutters.push(Gutter::Item {
                            ordinal,
                            first: true,
                        });
                        undo.push(Undo::Item);
                    }
                    Tag::Table(aligns) => {
                        self.flush_para();
                        tables.push(TableBuild {
                            aligns: aligns.clone(),
                            head: Vec::new(),
                            rows: Vec::new(),
                            row: Vec::new(),
                        });
                        undo.push(Undo::Nothing);
                    }
                    Tag::TableHead | Tag::TableRow => {
                        if let Some(t) = tables.last_mut() {
                            t.row.clear();
                        }
                        undo.push(Undo::Nothing);
                    }
                    Tag::TableCell => {
                        self.line.clear();
                        ctx.push(Ctx::Cell);
                        undo.push(Undo::Nothing);
                    }
                    Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                        let f = match tag {
                            Tag::Emphasis => Style::EMPH,
                            Tag::Strong => Style::STRONG,
                            _ => Style::STRIKE,
                        };
                        styles.push(style | f);
                        undo.push(Undo::Style);
                    }
                    Tag::Link(d) => {
                        let target = match &d.res {
                            LinkRes::External { url, host } | LinkRes::Mailto { url, host } => {
                                Some(SLink::External {
                                    url: url.clone(),
                                    host: host.clone(),
                                })
                            }
                            LinkRes::Record(u) => Some(SLink::Record(u.clone())),
                            LinkRes::Profile(did) => Some(SLink::Profile(did.clone())),
                            LinkRes::Anchor(v) => Some(SLink::Anchor(v.clone())),
                            LinkRes::Mention(did) => {
                                let text = prose(&format!("@{}", short_did(did.as_str())));
                                self.push_span(
                                    text,
                                    style | Style::MENTION | Style::LINK,
                                    Some(SLink::Mention(did.clone())),
                                );
                                i = m[i].min(evs.len() - 1) + 1;
                                continue;
                            }
                            LinkRes::Dropped | LinkRes::Pending => None,
                        };
                        if link.is_none()
                            && let Some(t) = target
                        {
                            styles.push(style | Style::LINK);
                            links.push(Some(t));
                        } else {
                            styles.push(style);
                            links.push(link.clone());
                        }
                        undo.push(Undo::Link);
                    }
                    Tag::Image(d) => {
                        if let Some(c) = figures.last().cloned()
                            && matches!(evs.get(m[i] + 1), Some(Ev::End(TagKind::Figure)))
                        {
                            // The figure's media element, then its caption.
                            let cap = vec![Span {
                                text: prose(&c),
                                style: Style::empty(),
                                link: None,
                            }];
                            match &d.res {
                                ImageRes::Media(idx) => {
                                    let a = self.media[*idx].alt_text(&d.alt, d.fallback_alt);
                                    let t = media_placeholder(self.media, *idx, &a);
                                    let ix = u16::try_from(*idx).unwrap_or(u16::MAX);
                                    let line = vec![Span {
                                        text: t,
                                        style: Style::PLACEHOLDER,
                                        link: Some(SLink::Media(ix)),
                                    }];
                                    self.emit(SKind::Media { index: ix, line });
                                }
                                _ => {
                                    self.image_span(d, style, link.clone());
                                    self.flush_para();
                                }
                            }
                            self.emit(SKind::Caption(cap));
                        } else {
                            self.image_span(d, style, link.clone());
                        }
                        i = m[i].min(evs.len() - 1) + 1;
                        continue;
                    }
                    Tag::FootnoteDef { .. } => undo.push(Undo::Nothing),
                },
                Ev::End(k) => {
                    let u = undo.pop().unwrap_or(Undo::Nothing);
                    match k {
                        TagKind::Paragraph => self.flush_para(),
                        TagKind::Heading => {
                            ctx.pop();
                            let line = self.take_line();
                            if let Some((level, id)) = heading.pop() {
                                self.emit(SKind::Heading { level, id, line });
                            }
                        }
                        TagKind::Summary => {
                            ctx.pop();
                            let mut line = self.take_line();
                            if line.is_empty() {
                                line.push(Span {
                                    text: prose(fixed::DETAILS),
                                    style: Style::empty(),
                                    link: None,
                                });
                            }
                            let (id, spoiler) = summaries.pop().unwrap_or((0, false));
                            self.emit(SKind::Summary { id, spoiler, line });
                            self.gutters.push(Gutter::Details(id));
                        }
                        TagKind::Details => {
                            self.flush_para();
                            pending_details.pop();
                            // The Details gutter was pushed at End(Summary).
                            if matches!(self.gutters.last(), Some(Gutter::Details(_))) {
                                self.gutters.pop();
                            }
                        }
                        TagKind::CodeBlock => {
                            ctx.pop();
                            if let Some((info, text)) = code.pop() {
                                let lang = info
                                    .as_deref()
                                    .and_then(code_lang)
                                    .map(|l| term_sanitize(&l, TermCtx::Code));
                                let mut ls: Vec<&str> = text.split('\n').collect();
                                if ls.last() == Some(&"") {
                                    ls.pop();
                                }
                                let lines =
                                    ls.iter().map(|l| term_sanitize(l, TermCtx::Code)).collect();
                                self.emit(SKind::Code { lang, lines });
                            }
                        }
                        TagKind::List => {
                            self.flush_para();
                            lists.pop();
                        }
                        TagKind::Item => {
                            self.flush_para();
                            if let Some(Gutter::Item { first: true, .. }) = self.gutters.last() {
                                self.emit(SKind::Para(Vec::new()));
                            }
                            self.gutters.pop();
                        }
                        TagKind::TableCell => {
                            ctx.pop();
                            let l = self.take_line();
                            if let Some(t) = tables.last_mut() {
                                t.row.push(l);
                            }
                        }
                        TagKind::TableHead => {
                            if let Some(t) = tables.last_mut() {
                                t.head = std::mem::take(&mut t.row);
                            }
                        }
                        TagKind::TableRow => {
                            if let Some(t) = tables.last_mut() {
                                let r = std::mem::take(&mut t.row);
                                t.rows.push(r);
                            }
                        }
                        TagKind::Table => {
                            if let Some(t) = tables.pop() {
                                self.emit(SKind::Table {
                                    aligns: t.aligns,
                                    head: t.head,
                                    rows: t.rows,
                                });
                            }
                        }
                        TagKind::Figure => {
                            self.flush_para();
                            figures.pop();
                        }
                        TagKind::BlockQuote | TagKind::Spoiler => {
                            self.flush_para();
                            if let Undo::Gutter = u {
                                self.gutters.pop();
                            }
                        }
                        _ => match u {
                            Undo::Style => {
                                styles.pop();
                            }
                            Undo::Link => {
                                styles.pop();
                                links.pop();
                            }
                            _ => {}
                        },
                    }
                }
                Ev::Text(t) | Ev::Literal(t) => match ctx.last() {
                    Some(Ctx::Code) => {
                        if let Some((_, buf)) = code.last_mut() {
                            buf.push_str(t);
                        }
                    }
                    _ => self.push_span(prose(t), style, link),
                },
                Ev::Code(t) => {
                    self.push_span(term_sanitize(t, TermCtx::Code), style | Style::CODE, link)
                }
                Ev::InlineMath(t) => self.push_span(
                    term_sanitize(&format!("${t}$"), TermCtx::Code),
                    style | Style::MATH,
                    link,
                ),
                Ev::DisplayMath(t) => match ctx.last() {
                    Some(Ctx::Flow) | None => {
                        self.flush_para();
                        let ls: Vec<&str> = t.trim_matches('\n').split('\n').collect();
                        self.emit(SKind::Math(
                            ls.iter().map(|l| term_sanitize(l, TermCtx::Code)).collect(),
                        ));
                    }
                    _ => self.push_span(
                        term_sanitize(&format!("$${t}$$"), TermCtx::Code),
                        style | Style::MATH,
                        link,
                    ),
                },
                Ev::FnRef(r) => {
                    if r.ord > 0 {
                        let l = link.or_else(|| Some(SLink::Anchor(format!("md-fn-{}", r.ord))));
                        self.push_span(prose(&format!("[{}]", r.ord)), style | Style::FOOTREF, l);
                    }
                }
                // A soft break at the very start of a paragraph (after display math) is dropped.
                Ev::SoftBreak if self.line.is_empty() && self.lines.is_empty() => {}
                Ev::SoftBreak => self.pending_space = Some((style, link)),
                Ev::HardBreak => match ctx.last() {
                    Some(Ctx::Flow) | None => {
                        let l = self.take_line();
                        self.lines.push(l);
                    }
                    _ => self.push_span(prose(" "), style, link),
                },
                Ev::Rule => {
                    self.flush_para();
                    self.emit(SKind::Rule);
                }
                Ev::Task(b) => {
                    self.push_span(prose(if *b { "[x] " } else { "[ ] " }), Style::TASK, None);
                }
            }
            i += 1;
        }
        self.flush_para();
    }
}

/// Build the `StyledDoc` of a rendered document (§9.3).
pub(crate) fn build(
    body: &[Ev],
    notes: &[Footnote],
    truncated: bool,
    media: &[MediaItem],
    used: &[u16],
) -> StyledDoc {
    let mut b = Builder {
        blocks: Vec::new(),
        gutters: Vec::new(),
        lines: Vec::new(),
        line: Vec::new(),
        media,
        next_summary_id: 0,
        pending_space: None,
    };
    b.flow(body);
    if truncated {
        b.emit(SKind::Notice(prose(fixed::TRUNCATED)));
    }
    for n in notes {
        b.gutters = vec![Gutter::Footnote(n.ord)];
        if n.in_spoiler {
            // A definition written inside a spoiler stays hidden behind one of its own.
            b.next_summary_id += 1;
            let id = b.next_summary_id;
            let line = vec![Span {
                text: prose(fixed::SPOILER),
                style: Style::empty(),
                link: None,
            }];
            b.emit(SKind::Summary {
                id,
                spoiler: true,
                line,
            });
            b.gutters.push(Gutter::Spoiler(id));
        }
        b.flow(&n.events);
    }
    b.gutters.clear();
    for i in 0..media.len() {
        if used.iter().any(|&u| usize::from(u) == i) {
            continue;
        }
        let a = media[i].gallery_alt();
        let ix = u16::try_from(i).unwrap_or(u16::MAX);
        let line = vec![Span {
            text: media_placeholder(media, i, &a),
            style: Style::PLACEHOLDER,
            link: Some(SLink::Media(ix)),
        }];
        b.emit(SKind::Media { index: ix, line });
    }
    StyledDoc { blocks: b.blocks }
}

/// The `StyledDoc` of a body error: one notice.
pub(crate) fn error_doc(text: &str) -> StyledDoc {
    StyledDoc {
        blocks: vec![SBlock {
            gutters: Vec::new(),
            kind: SKind::Notice(prose(text)),
        }],
    }
}
