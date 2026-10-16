// SPDX-License-Identifier: AGPL-3.0-or-later
//! A plain-stdout printer for [`StyledDoc`] implementing the §9.4 printer obligations
//! (`docs/spec/lfm-1.md`): only SGR and OSC 8 escape sequences, OSC 8 only for external links
//! and only when allowed, the real-host suffix (F1), spoiler hiding, reverse video for escaped
//! runs and a plain `--color never` form. It returns a `String`; the CLI writes it.

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

/// One printable piece: a word, a run of spaces, or an escaped run.
#[derive(Clone)]
struct Piece {
    pre: String,
    text: String,
    post: String,
    width: usize,
    space: bool,
}

struct Printer<'a> {
    opts: PrintOpts,
    handle_of: &'a dyn Fn(&str) -> Option<String>,
}

impl Printer<'_> {
    fn wrap_style(&self, style: Style, extra: &[u8]) -> (String, String) {
        if !self.opts.color {
            return (String::new(), String::new());
        }
        let mut codes = style_codes(style);
        codes.extend_from_slice(extra);
        if codes.is_empty() {
            return (String::new(), String::new());
        }
        (sgr(&codes), sgr(&[0]))
    }

    fn push_text_pieces(
        &self,
        out: &mut Vec<Piece>,
        text: &str,
        kind: RunKind,
        style: Style,
        osc: Option<&str>,
    ) {
        let (mut pre, mut post) =
            self.wrap_style(style, if kind == RunKind::Escaped { &[7] } else { &[] });
        if let Some(uri) = osc {
            pre = format!("{ESC}]8;;{uri}{ESC}\\{pre}");
            post = format!("{post}{ESC}]8;;{ESC}\\");
        }
        if kind == RunKind::Escaped {
            out.push(Piece {
                pre,
                text: text.to_owned(),
                post,
                width: text.width(),
                space: false,
            });
            return;
        }
        let mut word = String::new();
        let mut spaces = String::new();
        let flush_word = |out: &mut Vec<Piece>, w: &mut String| {
            if !w.is_empty() {
                out.push(Piece {
                    pre: pre.clone(),
                    text: std::mem::take(w),
                    post: post.clone(),
                    width: 0,
                    space: false,
                });
                if let Some(p) = out.last_mut() {
                    p.width = p.text.width();
                }
            }
        };
        for c in text.chars() {
            if c == ' ' {
                flush_word(out, &mut word);
                spaces.push(c);
            } else {
                if !spaces.is_empty() {
                    let n = spaces.len();
                    out.push(Piece {
                        pre: String::new(),
                        text: std::mem::take(&mut spaces),
                        post: String::new(),
                        width: n,
                        space: true,
                    });
                }
                word.push(c);
            }
        }
        flush_word(out, &mut word);
        if !spaces.is_empty() {
            let n = spaces.len();
            out.push(Piece {
                pre: String::new(),
                text: spaces,
                post: String::new(),
                width: n,
                space: true,
            });
        }
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
                let (pre, post) = if self.opts.color {
                    (sgr(&[2]), sgr(&[22]))
                } else {
                    (String::new(), String::new())
                };
                out.push(Piece {
                    pre: String::new(),
                    text: " ".into(),
                    post: String::new(),
                    width: 1,
                    space: true,
                });
                let t = format!("\u{27E8}{h}\u{27E9}");
                let w = t.width();
                out.push(Piece {
                    pre,
                    text: t,
                    post,
                    width: w,
                    space: false,
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

    /// Lay pieces out into lines of at most `avail` columns (no wrapping when `None`).
    fn layout(pieces: Vec<Piece>, avail: Option<usize>) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0usize;
        let mut pending_space: Option<Piece> = None;
        let Some(avail) = avail.map(|a| a.max(1)) else {
            let s: String = pieces
                .iter()
                .map(|p| format!("{}{}{}", p.pre, p.text, p.post))
                .collect();
            return vec![s.trim_end_matches(' ').to_owned()];
        };
        for p in pieces {
            if p.space {
                if cur_w > 0 {
                    pending_space = Some(p);
                }
                continue;
            }
            let sp = pending_space.as_ref().map_or(0, |s| s.width);
            if cur_w > 0 && cur_w + sp + p.width > avail {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
                pending_space = None;
            } else if let Some(s) = pending_space.take() {
                cur.push_str(&s.text);
                cur_w += s.width;
            }
            if p.width <= avail.saturating_sub(cur_w) {
                cur.push_str(&p.pre);
                cur.push_str(&p.text);
                cur.push_str(&p.post);
                cur_w += p.width;
                continue;
            }
            // A word longer than the line: break at grapheme boundaries.
            for g in p.text.graphemes(true) {
                let gw = g.width();
                if cur_w > 0 && cur_w + gw > avail {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push_str(&p.pre);
                cur.push_str(g);
                cur.push_str(&p.post);
                cur_w += gw;
            }
        }
        lines.push(cur);
        lines
    }

    fn render_line(&self, line: &SLine, avail: Option<usize>) -> Vec<String> {
        Self::layout(self.line_pieces(line), avail)
    }

    fn styled_plain(&self, text: &str, style: Style) -> String {
        let (pre, post) = self.wrap_style(style, &[]);
        format!("{pre}{text}{post}")
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
    let p = Printer {
        opts: *opts,
        handle_of,
    };
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
        let first_prefix: String = block
            .gutters
            .iter()
            .map(|g| gutter_text(g, first_footnote))
            .collect();
        let cont_prefix: String = block
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
            SKind::Table { head, rows, .. } => table_lines(&p, head, rows),
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

fn table_lines(p: &Printer<'_>, head: &[SLine], rows: &[Vec<SLine>]) -> Vec<String> {
    let cols = rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(head.len()))
        .max()
        .unwrap_or(0);
    let render = |l: &SLine| -> (String, usize) {
        let pieces = p.line_pieces(l);
        let w: usize = pieces.iter().map(|x| x.width).sum();
        (
            pieces
                .iter()
                .map(|x| format!("{}{}{}", x.pre, x.text, x.post))
                .collect(),
            w,
        )
    };
    let head_r: Vec<(String, usize)> = head.iter().map(render).collect();
    let rows_r: Vec<Vec<(String, usize)>> = rows
        .iter()
        .map(|r| r.iter().map(render).collect())
        .collect();
    let mut widths = vec![0usize; cols];
    for r in std::iter::once(&head_r).chain(rows_r.iter()) {
        for (c, (_, w)) in r.iter().enumerate() {
            widths[c] = widths[c].max(*w);
        }
    }
    let fmt_row = |r: &[(String, usize)]| -> String {
        let cells: Vec<String> = (0..cols)
            .map(|c| match r.get(c) {
                Some((s, w)) => format!("{s}{}", " ".repeat(widths[c] - w)),
                None => " ".repeat(widths[c]),
            })
            .collect();
        cells.join(" \u{2502} ").trim_end().to_owned()
    };
    let mut out = vec![fmt_row(&head_r)];
    out.push(
        widths
            .iter()
            .map(|w| "\u{2500}".repeat(*w))
            .collect::<Vec<_>>()
            .join("\u{2500}\u{253C}\u{2500}"),
    );
    for r in &rows_r {
        out.push(fmt_row(r));
    }
    out
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
