// SPDX-License-Identifier: AGPL-3.0-or-later
//! A plain-stdout printer for [`StyledDoc`] implementing the §9.4 printer obligations
//! (`docs/spec/lfm-1.md`): only SGR and OSC 8 escape sequences, OSC 8 only for external links
//! and only when allowed, the real-host suffix (F1), spoiler hiding, reverse video for escaped
//! runs and a plain `--color never` form. It returns a `String`; the CLI writes it.

use std::cell::{Cell, RefCell};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::html::fixed;
use crate::styled::{Gutter, SBlock, SKind, SLine, SLink, Span, Style, StyledDoc};
use crate::term::{RunKind, TermCtx, term_sanitize};
use crate::url::term_shows_host;

/// Printer options. The default is the plain `--color never` form without wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrintOpts {
    /// Emit SGR styling (`--color always`, or `auto` on a TTY). `false` is `--color never`.
    pub color: bool,
    /// Emit OSC 8 hyperlinks. Callers set this only when color is enabled *and* stdout is a
    /// TTY (§9.4); it is ignored when `color` is false.
    pub hyperlinks: bool,
    /// Show spoiler content (the CLI's explicit reveal option).
    pub reveal_spoilers: bool,
    /// Wrap prose to this many columns (`None`: no wrapping).
    pub width: Option<usize>,
}

/// The SGR parameters printers may use (§9.4).
pub const ALLOWED_SGR: &[u8] = &[
    0, 1, 2, 3, 4, 7, 9, 22, 23, 24, 27, 29, 39, 30, 31, 32, 33, 34, 35, 36, 37, 90, 91, 92, 93,
    94, 95, 96, 97,
];

const ESC: char = '\u{1b}';

fn sgr(codes: &[u8]) -> String {
    let p: Vec<String> = codes.iter().map(u8::to_string).collect();
    format!("{ESC}[{}m", p.join(";"))
}

fn style_codes(s: Style) -> Vec<u8> {
    let mut c = Vec::new();
    if s.contains(Style::STRONG) || s.contains(Style::MENTION) {
        c.push(1);
    }
    if s.contains(Style::PLACEHOLDER) || s.contains(Style::FOOTREF) {
        c.push(2);
    }
    if s.contains(Style::EMPH) {
        c.push(3);
    }
    if s.contains(Style::LINK) {
        c.push(4);
    }
    if s.contains(Style::STRIKE) {
        c.push(9);
    }
    if s.contains(Style::MENTION) {
        c.push(34);
    } else if s.contains(Style::CODE) {
        c.push(36);
    } else if s.contains(Style::MATH) {
        c.push(35);
    } else if s.contains(Style::NOTICE) {
        c.push(33);
    }
    c
}

/// OSC 8 URI encoding (§9.4): bytes outside 0x21–0x7E, and `;`, are percent-encoded.
/// `None` if the result exceeds 2,048 bytes.
pub fn osc8_uri(url: &str) -> Option<String> {
    let mut out = String::new();
    for &b in url.as_bytes() {
        if (0x21..=0x7e).contains(&b) && b != b';' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    (out.len() <= 2048).then_some(out)
}

/// The most bytes of OSC 8 hyperlink sequences one document may print. Past it, links print
/// without OSC 8 (their text and ` ⟨host⟩` suffix are unchanged), so that a long link text
/// wrapped over many lines cannot repeat a 2,048-byte URI on every line without bound.
pub const OSC8_BUDGET: usize = 1 << 20;

/// The widest a table column is laid out; wider cells wrap.
pub const TABLE_COLUMN_MAX: usize = 40;

/// A run style: the SGR sequences around it and the URI of its OSC 8 hyperlink.
#[derive(Clone, Default, PartialEq, Eq)]
struct Sty {
    pre: String,
    post: String,
    osc: Option<String>,
}

/// One printable piece: a word, a run of spaces, or an escaped run, with its style (an index
/// into [`Printer::styles`]; 0 is unstyled).
#[derive(Clone)]
struct Piece {
    text: String,
    width: usize,
    space: bool,
    sty: usize,
}

struct Printer<'a> {
    opts: PrintOpts,
    handle_of: &'a dyn Fn(&str) -> Option<String>,
    styles: RefCell<Vec<Sty>>,
    osc_left: Cell<usize>,
}

/// One output line being built. A style is opened when the first piece that uses it is
/// written and closed when the style changes or the line ends, so SGR and OSC 8 sequences
/// are written once per style run per line, never once per word or grapheme.
struct LineBuf<'p, 'a> {
    p: &'p Printer<'a>,
    s: String,
    w: usize,
    /// The open style and whether its OSC 8 was written.
    open: Option<(usize, bool)>,
}

impl<'p, 'a> LineBuf<'p, 'a> {
    fn new(p: &'p Printer<'a>) -> Self {
        Self {
            p,
            s: String::new(),
            w: 0,
            open: None,
        }
    }

    fn close(&mut self) {
        if let Some((sty, osc)) = self.open.take() {
            let styles = self.p.styles.borrow();
            self.s.push_str(&styles[sty].post);
            if osc {
                self.s.push_str("\u{1b}]8;;\u{1b}\\");
            }
        }
    }

    fn push(&mut self, text: &str, width: usize, sty: usize) {
        let same = match self.open {
            Some((o, _)) => o == sty || self.p.styles.borrow()[o] == self.p.styles.borrow()[sty],
            None => sty == 0,
        };
        if !same {
            self.close();
            if sty != 0 {
                let styles = self.p.styles.borrow();
                let st = &styles[sty];
                let mut osc = false;
                if let Some(uri) = &st.osc {
                    let cost = uri.len() + 12;
                    if self.p.osc_left.get() >= cost {
                        self.p.osc_left.set(self.p.osc_left.get() - cost);
                        self.s.push_str("\u{1b}]8;;");
                        self.s.push_str(uri);
                        self.s.push_str("\u{1b}\\");
                        osc = true;
                    }
                }
                self.s.push_str(&st.pre);
                self.open = Some((sty, osc));
            }
        }
        self.s.push_str(text);
        self.w += width;
    }

    fn finish(mut self) -> String {
        self.close();
        self.s
    }
}

impl Printer<'_> {
    fn new<'a>(opts: PrintOpts, handle_of: &'a dyn Fn(&str) -> Option<String>) -> Printer<'a> {
        Printer {
            opts,
            handle_of,
            styles: RefCell::new(vec![Sty::default()]),
            osc_left: Cell::new(OSC8_BUDGET),
        }
    }

    /// The style index of `style` (plus `extra` SGR codes and an optional OSC 8 URI).
    fn style(&self, style: Style, extra: &[u8], osc: Option<&str>) -> usize {
        let osc = osc.map(str::to_owned);
        let (pre, post) = if self.opts.color {
            let mut codes = style_codes(style);
            codes.extend_from_slice(extra);
            if codes.is_empty() {
                (String::new(), String::new())
            } else {
                (sgr(&codes), sgr(&[0]))
            }
        } else {
            (String::new(), String::new())
        };
        if pre.is_empty() && osc.is_none() {
            return 0;
        }
        let mut styles = self.styles.borrow_mut();
        styles.push(Sty { pre, post, osc });
        styles.len() - 1
    }

    fn push_text_pieces(
        &self,
        out: &mut Vec<Piece>,
        text: &str,
        kind: RunKind,
        style: Style,
        osc: Option<&str>,
    ) {
        let sty = self.style(
            style,
            if kind == RunKind::Escaped { &[7] } else { &[] },
            osc,
        );
        if kind == RunKind::Escaped {
            out.push(Piece {
                text: text.to_owned(),
                width: text.width(),
                space: false,
                sty,
            });
            return;
        }
        let mut start = 0usize;
        let mut in_space: Option<bool> = None;
        let flush = |out: &mut Vec<Piece>, s: &str, space: bool| {
            if !s.is_empty() {
                out.push(Piece {
                    text: s.to_owned(),
                    width: s.width(),
                    space,
                    sty,
                });
            }
        };
        for (i, c) in text.char_indices() {
            let sp = c == ' ';
            if in_space.is_some_and(|s| s != sp) {
                flush(out, &text[start..i], !sp);
                start = i;
            }
            in_space = Some(sp);
        }
        flush(out, &text[start..], in_space.unwrap_or(false));
    }

    fn line_pieces(&self, line: &SLine) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < line.len() {
            // A run of consecutive spans sharing one link prints as one link.
            let mut j = i + 1;
            while j < line.len() && line[j].link.is_some() && line[j].link == line[i].link {
                j += 1;
            }
            let group = &line[i..j];
            let ext = match &line[i].link {
                Some(SLink::External { url, host }) => Some((url.as_str(), host.as_str())),
                _ => None,
            };
            let osc = match ext {
                Some((url, _)) if self.opts.color && self.opts.hyperlinks => osc8_uri(url),
                _ => None,
            };
            let mut plain = String::new();
            for span in group {
                self.span_pieces(&mut out, span, osc.as_deref(), &mut plain);
            }
            let h = ext.map(|(_, host)| {
                term_sanitize(host, TermCtx::Name { max_graphemes: 0 }).to_plain_string()
            });
            // F1: show the host unless the text is the URL, or already shows exactly this host
            // (a standalone external-image placeholder `[image: A ⟨host⟩]`).
            if let (Some((url, _)), Some(h)) = (ext, h)
                && !plain.contains(&format!("\u{27E8}{h}\u{27E9}"))
                && term_shows_host(&plain, url)
            {
                out.push(Piece {
                    text: " ".into(),
                    width: 1,
                    space: true,
                    sty: 0,
                });
                let t = format!("\u{27E8}{h}\u{27E9}");
                let sty = if self.opts.color {
                    let mut styles = self.styles.borrow_mut();
                    styles.push(Sty {
                        pre: sgr(&[2]),
                        post: sgr(&[22]),
                        osc: None,
                    });
                    styles.len() - 1
                } else {
                    0
                };
                out.push(Piece {
                    width: t.width(),
                    text: t,
                    space: false,
                    sty,
                });
            }
            i = j;
        }
        out
    }

    fn span_pieces(
        &self,
        out: &mut Vec<Piece>,
        span: &Span,
        osc: Option<&str>,
        plain: &mut String,
    ) {
        if let Some(SLink::Mention(did)) = &span.link
            && let Some(h) = (self.handle_of)(did.as_str())
        {
            let mut t = term_sanitize("@", TermCtx::Prose);
            t.append(term_sanitize(&h, TermCtx::Name { max_graphemes: 0 }));
            for run in t.runs() {
                plain.push_str(run.text());
                self.push_text_pieces(out, run.text(), run.kind(), span.style, None);
            }
            return;
        }
        for run in span.text.runs() {
            plain.push_str(run.text());
            self.push_text_pieces(out, run.text(), run.kind(), span.style, osc);
        }
    }

    /// Lay pieces out into lines of at most `avail` columns. Without `avail` there is one line
    /// that keeps its leading spaces; trailing spaces are always dropped, and so are spaces at a
    /// wrap.
    fn layout(&self, pieces: Vec<Piece>, avail: Option<usize>) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = LineBuf::new(self);
        let mut pending_space: Option<Piece> = None;
        let keep_leading = avail.is_none();
        let avail = avail.map_or(usize::MAX, |a| a.max(1));
        for p in pieces {
            if p.space {
                if cur.w > 0 {
                    pending_space = Some(p);
                } else if keep_leading {
                    cur.push(&p.text, p.width, p.sty);
                }
                continue;
            }
            let sp = pending_space.as_ref().map_or(0, |s| s.width);
            if cur.w > 0 && cur.w.saturating_add(sp).saturating_add(p.width) > avail {
                lines.push(std::mem::replace(&mut cur, LineBuf::new(self)).finish());
                pending_space = None;
            } else if let Some(s) = pending_space.take() {
                cur.push(&s.text, s.width, s.sty);
            }
            if p.width <= avail.saturating_sub(cur.w) {
                cur.push(&p.text, p.width, p.sty);
                continue;
            }
            // A word longer than the line: break at grapheme boundaries.
            for g in p.text.graphemes(true) {
                let gw = g.width();
                if cur.w > 0 && cur.w + gw > avail {
                    lines.push(std::mem::replace(&mut cur, LineBuf::new(self)).finish());
                }
                cur.push(g, gw, p.sty);
            }
        }
        lines.push(cur.finish());
        lines
    }

    fn render_line(&self, line: &SLine, avail: Option<usize>) -> Vec<String> {
        self.layout(self.line_pieces(line), avail)
    }

    fn styled_plain(&self, text: &str, style: Style) -> String {
        let sty = self.style(style, &[], None);
        let mut l = LineBuf::new(self);
        l.push(text, 0, sty);
        l.finish()
    }
}

fn gutter_text(g: &Gutter, first_footnote: bool) -> String {
    match g {
        Gutter::Quote | Gutter::Alert(_) => "\u{2502} ".into(),
        Gutter::Spoiler(_) => "\u{2506} ".into(),
        Gutter::Details(_) => "  ".into(),
        Gutter::Item { ordinal, first } => {
            let marker = match ordinal {
                Some(n) => format!("{n}. "),
                None => "\u{2022} ".into(),
            };
            if *first {
                marker
            } else {
                " ".repeat(marker.width())
            }
        }
        Gutter::Footnote(n) => {
            let marker = format!("[{n}] ");
            if first_footnote {
                marker
            } else {
                " ".repeat(marker.width())
            }
        }
    }
}

/// The widest gutter prefix printed before a line, in columns.
const GUTTER_MAX: usize = 32;

/// Print a document for plain stdout (§9.4). Mentions keep their placeholder.
pub fn print_doc(doc: &StyledDoc, opts: &PrintOpts) -> String {
    print_doc_with(doc, opts, &|_| None)
}

/// Like [`print_doc`], with `handle_of(did)` returning the bidirectionally verified handle
/// for a mention (§5.4), which replaces the `@short_did` placeholder.
pub fn print_doc_with(
    doc: &StyledDoc,
    opts: &PrintOpts,
    handle_of: &dyn Fn(&str) -> Option<String>,
) -> String {
    let p = Printer::new(*opts, handle_of);
    let mut out = String::new();
    let mut prev: Option<&SBlock> = None;
    let mut last_footnote: Option<u32> = None;
    for block in &doc.blocks {
        let hidden_spoiler = !opts.reveal_spoilers
            && block
                .gutters
                .iter()
                .any(|g| matches!(g, Gutter::Spoiler(_)));
        if hidden_spoiler {
            continue;
        }
        let footnote = block.gutters.iter().find_map(|g| match g {
            Gutter::Footnote(n) => Some(*n),
            _ => None,
        });
        let first_footnote = footnote.is_some() && footnote != last_footnote;
        last_footnote = footnote;
        // Blank line between blocks, except inside lists, before a caption and after a summary.
        if let Some(pb) = prev {
            let in_list = |b: &SBlock| b.gutters.iter().any(|g| matches!(g, Gutter::Item { .. }));
            let opens = |pb: &SBlock, b: &SBlock| match pb.kind {
                SKind::Summary { id, .. } => b
                    .gutters
                    .iter()
                    .any(|g| matches!(g, Gutter::Spoiler(x) | Gutter::Details(x) if *x == id)),
                _ => false,
            };
            let tight = (in_list(pb) && in_list(block))
                || matches!(block.kind, SKind::Caption(_))
                || opens(pb, block);
            if !tight {
                out.push('\n');
            }
        }
        prev = Some(block);
        let first: Vec<String> = block
            .gutters
            .iter()
            .map(|g| gutter_text(g, first_footnote))
            .collect();
        let cont: Vec<String> = block
            .gutters
            .iter()
            .map(|g| match g {
                Gutter::Item { ordinal, .. } => gutter_text(
                    &Gutter::Item {
                        ordinal: *ordinal,
                        first: false,
                    },
                    false,
                ),
                g => gutter_text(g, false),
            })
            .collect();
        // Deeply nested blocks show only their innermost gutters (at most GUTTER_MAX columns),
        // after `… `, so that the prefix repeated on every line stays bounded.
        let mut keep = 0usize;
        let mut w = 0usize;
        for g in first.iter().rev() {
            if w + g.width() > GUTTER_MAX {
                break;
            }
            w += g.width();
            keep += 1;
        }
        let elide = if keep < first.len() { "\u{2026} " } else { "" };
        let first_prefix = format!("{elide}{}", first[first.len() - keep..].concat());
        let cont_prefix = format!("{elide}{}", cont[cont.len() - keep..].concat());
        let avail = opts
            .width
            .map(|w| w.saturating_sub(first_prefix.width()).max(10));
        let lines: Vec<String> = match &block.kind {
            SKind::Para(ls) => {
                let mut v = Vec::new();
                for l in ls {
                    v.extend(p.render_line(l, avail));
                }
                if v.is_empty() {
                    v.push(String::new());
                }
                v
            }
            SKind::Heading { level, line, .. } => {
                let mut head = p.styled_plain(&"#".repeat(usize::from(*level)), Style::STRONG);
                head.push(' ');
                let mut v = p.render_line(line, avail);
                if let Some(f) = v.first_mut() {
                    *f = format!("{head}{f}");
                }
                v
            }
            SKind::Code { lines, .. } => lines
                .iter()
                .map(|t| p.render_line(&code_line(t), None).concat())
                .collect(),
            SKind::Math(lines) => lines
                .iter()
                .map(|t| p.render_line(&styled_line(t, Style::MATH), None).concat())
                .collect(),
            SKind::Table { head, rows, .. } => table_lines(&p, head, rows, avail),
            SKind::Rule => vec!["\u{2500}".repeat(opts.width.unwrap_or(40).min(80))],
            SKind::Summary { spoiler, line, .. } => {
                if *spoiler && !opts.reveal_spoilers {
                    vec![p.styled_plain(fixed::SPOILER_HIDDEN, Style::PLACEHOLDER)]
                } else {
                    let mut v = p.render_line(line, avail);
                    if let Some(f) = v.first_mut() {
                        *f = format!("\u{25B8} {f}");
                    }
                    v
                }
            }
            SKind::Media { line, .. } => p.render_line(line, avail),
            SKind::Caption(line) => p
                .render_line(line, avail)
                .into_iter()
                .map(|l| format!("  {l}"))
                .collect(),
            SKind::Notice(t) => p.render_line(&styled_line(t, Style::NOTICE), avail),
        };
        for (k, l) in lines.iter().enumerate() {
            out.push_str(if k == 0 { &first_prefix } else { &cont_prefix });
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

fn styled_line(t: &crate::term::TermText, style: Style) -> SLine {
    vec![Span {
        text: t.clone(),
        style,
        link: None,
    }]
}

fn code_line(t: &crate::term::TermText) -> SLine {
    styled_line(t, Style::CODE)
}

/// A table, aligned in columns of at most [`TABLE_COLUMN_MAX`] (or `avail`) columns, wider
/// cells wrapping. When aligning would multiply the output (padding every row to columns that
/// one long cell made wide), the rows print unaligned instead, so the output stays within a
/// small multiple of the table's own text.
fn table_lines(
    p: &Printer<'_>,
    head: &[SLine],
    rows: &[Vec<SLine>],
    avail: Option<usize>,
) -> Vec<String> {
    let cols = rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(head.len()))
        .max()
        .unwrap_or(0);
    let cap = avail
        .map_or(TABLE_COLUMN_MAX, |a| a.min(TABLE_COLUMN_MAX))
        .max(1);
    let all: Vec<&[SLine]> = std::iter::once(head)
        .chain(rows.iter().map(Vec::as_slice))
        .collect();
    let pieces: Vec<Vec<(Vec<Piece>, usize)>> = all
        .iter()
        .map(|r| {
            r.iter()
                .map(|l| {
                    let ps = p.line_pieces(l);
                    let w = ps.iter().map(|x| x.width).sum();
                    (ps, w)
                })
                .collect()
        })
        .collect();
    let mut widths = vec![0usize; cols];
    let mut content = 0usize;
    for r in &pieces {
        for (c, (_, w)) in r.iter().enumerate() {
            widths[c] = widths[c].max((*w).min(cap));
            content += w;
        }
    }
    let sep = " \u{2502} ";
    // Lay every cell out within its column.
    let laid: Vec<Vec<Vec<String>>> = pieces
        .into_iter()
        .map(|r| {
            r.into_iter()
                .enumerate()
                .map(|(c, (ps, _))| p.layout(ps, Some(widths[c].max(1))))
                .collect()
        })
        .collect();
    let line_w: usize = widths.iter().sum::<usize>() + 3 * cols.saturating_sub(1);
    let height = |r: &Vec<Vec<String>>| r.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let padded: usize = laid.iter().map(|r| height(r) * line_w).sum();
    if padded > 8 * content + 4096 {
        // Unaligned: each row on one line, cells as written.
        let mut out = Vec::new();
        for (k, r) in all.iter().enumerate() {
            let cells: Vec<String> = r
                .iter()
                .map(|l| p.layout(p.line_pieces(l), None).concat())
                .collect();
            out.push(cells.join(sep).trim_end().to_owned());
            if k == 0 {
                out.push(vec!["\u{2500}"; cols.max(1)].join("\u{2500}\u{253C}\u{2500}"));
            }
        }
        return out;
    }
    let mut out = Vec::new();
    for (k, r) in laid.iter().enumerate() {
        for line in 0..height(r) {
            let cells: Vec<String> = (0..cols)
                .map(|c| {
                    let (text, w) = match r.get(c).and_then(|cell| cell.get(line)) {
                        Some(t) => (t.as_str(), visible_width(t)),
                        None => ("", 0),
                    };
                    format!("{text}{}", " ".repeat(widths[c].saturating_sub(w)))
                })
                .collect();
            out.push(cells.join(sep).trim_end().to_owned());
        }
        if k == 0 {
            out.push(
                widths
                    .iter()
                    .map(|w| "\u{2500}".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("\u{2500}\u{253C}\u{2500}"),
            );
        }
    }
    out
}

/// The display width of printer output (escape sequences excluded).
fn visible_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut rest = s;
    while let Some(i) = rest.find('\u{1b}') {
        w += rest[..i].width();
        let after = &rest[i + 1..];
        rest = if let Some(csi) = after.strip_prefix('[') {
            csi.find('m').map_or("", |e| &csi[e + 1..])
        } else if let Some(osc) = after.strip_prefix(']') {
            osc.find("\u{1b}\\").map_or("", |e| &osc[e + 2..])
        } else {
            after
        };
    }
    w + rest.width()
}

/// Check that `s` contains no escape sequence other than the SGR and OSC 8 forms of §9.4,
/// and no other C0/C1 control except `\n`.
pub fn check_printer_output(s: &str) -> Result<(), String> {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            0x1b => match b.get(i + 1) {
                Some(b'[') => {
                    let end = s[i + 2..].find('m').ok_or("unterminated CSI")? + i + 2;
                    let params = &s[i + 2..end];
                    for p in params.split(';') {
                        let n: u8 = p.parse().map_err(|_| format!("bad SGR parameter {p:?}"))?;
                        if !ALLOWED_SGR.contains(&n) {
                            return Err(format!("SGR {n} not allowed"));
                        }
                    }
                    i = end + 1;
                }
                Some(b']') => {
                    let rest = &s[i + 2..];
                    let body = rest.strip_prefix("8;;").ok_or("OSC other than 8")?;
                    let end = body.find("\u{1b}\\").ok_or("unterminated OSC 8")?;
                    let uri = &body[..end];
                    if uri.len() > 2048
                        || !uri.bytes().all(|c| (0x21..=0x7e).contains(&c) && c != b';')
                    {
                        return Err("bad OSC 8 URI".into());
                    }
                    i += 2 + 3 + end + 2;
                }
                _ => return Err("escape sequence other than SGR/OSC 8".into()),
            },
            b'\n' => i += 1,
            c if c < 0x20 || c == 0x7f => return Err(format!("control byte {c:#04x}")),
            _ => {
                let ch = s[i..].chars().next().ok_or("bad utf-8")?;
                if matches!(ch, '\u{80}'..='\u{9f}' | '\u{2028}' | '\u{2029}' | '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                {
                    return Err(format!("forbidden character {ch:?}"));
                }
                i += ch.len_utf8();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderOpts, render};

    fn doc(src: &str) -> StyledDoc {
        render(src, &[], &RenderOpts::ALL).styled
    }

    #[test]
    fn verified_handle_replaces_placeholder_sanitised() {
        let d = doc("hi [@x](at://did:plc:abcdefghijklmnopqrstuvwx)");
        let out = print_doc_with(&d, &PrintOpts::default(), &|did| {
            (did == "did:plc:abcdefghijklmnopqrstuvwx")
                .then(|| "ali\u{202e}ce\u{1b}[2J.example".to_owned())
        });
        assert_eq!(out, "hi @alice^[[2J.example\n");
        assert!(check_printer_output(&out).is_ok());
        let plain = print_doc(&d, &PrintOpts::default());
        assert_eq!(plain, "hi @did:plc:abcdefgh\u{2026}\n");
    }

    #[test]
    fn osc8_only_when_allowed() {
        let d = doc("[x](https://example.com)");
        let no_tty = print_doc(
            &d,
            &PrintOpts {
                color: true,
                hyperlinks: false,
                ..PrintOpts::default()
            },
        );
        assert!(!no_tty.contains("\u{1b}]8"));
        let tty = print_doc(
            &d,
            &PrintOpts {
                color: true,
                hyperlinks: true,
                ..PrintOpts::default()
            },
        );
        assert!(tty.contains("\u{1b}]8;;https://example.com\u{1b}\\"));
        // hyperlinks without color (`--color never`) are ignored
        let never = print_doc(
            &d,
            &PrintOpts {
                color: false,
                hyperlinks: true,
                ..PrintOpts::default()
            },
        );
        assert_eq!(never, "x \u{27E8}example.com\u{27E9}\n");
    }

    #[test]
    fn checker_rejects_foreign_sequences() {
        assert!(check_printer_output("\u{1b}[31mx\u{1b}[0m\n").is_ok());
        assert!(check_printer_output("\u{1b}[38;5;1mx").is_err());
        assert!(check_printer_output("\u{1b}]0;title\u{7}").is_err());
        assert!(check_printer_output("\u{1b}[2J").is_err());
        assert!(check_printer_output("a\u{202e}b").is_err());
        assert!(check_printer_output("a\rb").is_err());
        assert_eq!(
            osc8_uri("https://e.com/a;b c"),
            Some("https://e.com/a%3Bb%20c".into())
        );
    }
}
