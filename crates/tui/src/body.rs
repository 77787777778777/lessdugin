// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bodies: the LFM/1 terminal projection (`StyledDoc`, `lfm-1.md` §9.2) laid out at a width.
//!
//! A body is rendered from the markdown source of the cached record when its CID is the one
//! the view shows (as the CLI does); otherwise the view's plain text P is shown. The
//! `StyledDoc` is width-independent and kept in the screen state; [`Body::lines`] lays it out
//! for the current width with the core's printer (`lfm-1.md` §9.4, SGR only, no OSC 8 here)
//! and turns its SGR runs into ratatui styles. The printer's output is checked with
//! [`check_printer_output`] first; anything unexpected falls back to the plain (colourless)
//! printing, which contains no escape sequence at all.

use std::collections::BTreeMap;

use lessdugin_core::dto::RenderedBody;
use lessdugin_lexicon::collection::nsid;
use lessdugin_markdown::printer::{PrintOpts, check_printer_output, print_doc_with};
use lessdugin_markdown::styled::StyledDoc;
use lessdugin_markdown::{MediaItem, RenderOpts};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::text::{TermCtx, pad, spans, term_sanitize, tt};

/// A body as the screen keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Body {
    /// The content.
    pub doc: BodyDoc,
    /// The view's notices (truncation, body errors), shown dimmed after the content.
    pub notices: Vec<String>,
}

/// A body's content.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BodyDoc {
    /// The terminal projection of the record's markdown.
    Styled(StyledDoc),
    /// The view's plain text (another version is shown, or the body is not inline).
    Plain(String),
    /// Nothing to show (hidden and deleted items).
    #[default]
    Empty,
}

impl Body {
    /// The body of an item from its cached record value (when it holds the CID the view shows)
    /// or the view.
    pub fn from_record(record: Option<&Value>, view: &RenderedBody) -> Self {
        let doc = record
            .and_then(|v| v.get("body"))
            .and_then(styled_doc)
            .map(BodyDoc::Styled)
            .unwrap_or_else(|| {
                if view.text.is_empty() {
                    BodyDoc::Empty
                } else {
                    BodyDoc::Plain(view.text.clone())
                }
            });
        Self {
            doc,
            notices: view.notices.clone(),
        }
    }

    /// The body's first line of prose, or `None` when it has nothing to show.
    pub fn plain_excerpt(&self) -> Option<String> {
        let text = match &self.doc {
            BodyDoc::Styled(doc) => {
                // printed wide so the excerpt is the source's own line, not a wrap
                print_doc_with(
                    doc,
                    &PrintOpts {
                        color: false,
                        hyperlinks: false,
                        reveal_spoilers: false,
                        width: None,
                    },
                    &|_| None,
                )
            }
            BodyDoc::Plain(t) => t.clone(),
            BodyDoc::Empty => return None,
        };
        let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        Some(first.to_owned())
    }

    /// The body laid out at `width` columns (at least 20), each line indented by `indent`.
    pub fn lines(
        &self,
        width: usize,
        indent: usize,
        handles: &BTreeMap<String, String>,
    ) -> Vec<Line<'static>> {
        let width = width.max(20);
        let mut out = match &self.doc {
            BodyDoc::Styled(doc) => styled_lines(doc, width, handles),
            BodyDoc::Plain(text) => plain_lines(text, width),
            BodyDoc::Empty => Vec::new(),
        };
        for n in &self.notices {
            out.push(Line::from(spans(
                &tt(n),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        if indent > 0 {
            for l in out.iter_mut().filter(|l| l.width() > 0) {
                let mut v = spans(&pad(indent), Style::default());
                v.append(&mut l.spans);
                l.spans = v;
            }
        }
        out
    }
}

/// The `StyledDoc` of an `NS.markdown` value (inline text only); `None` for any other value
/// (a spilled or unknown body).
pub fn styled_doc(md: &Value) -> Option<StyledDoc> {
    let ty = md.get("$type")?.as_str()?;
    if ty != nsid::MARKDOWN && ty.strip_suffix("#main") != Some(nsid::MARKDOWN) {
        return None;
    }
    let text = md.get("text")?.as_str()?;
    let media: Vec<MediaItem> = md
        .get("media")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|m| serde_json::from_value(m.clone()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let profile = md.get("profile").and_then(Value::as_str);
    let opts = RenderOpts {
        html: false,
        styled: true,
        text: false,
    };
    Some(lessdugin_markdown::render_profile(text, &media, profile, &opts).styled)
}

fn styled_lines(
    doc: &StyledDoc,
    width: usize,
    handles: &BTreeMap<String, String>,
) -> Vec<Line<'static>> {
    let handle_of = |did: &str| handles.get(did).cloned();
    let mut opts = PrintOpts {
        color: true,
        hyperlinks: false,
        reveal_spoilers: false,
        width: Some(width),
    };
    let printed = print_doc_with(doc, &opts, &handle_of);
    if check_printer_output(&printed).is_ok() {
        return printed
            .trim_end_matches('\n')
            .split('\n')
            .map(sgr_line)
            .collect();
    }
    // never expected: print without any escape sequence
    opts.color = false;
    let printed = print_doc_with(doc, &opts, &handle_of);
    printed
        .trim_end_matches('\n')
        .split('\n')
        .map(|l| Line::from(spans(&tt(l), Style::default())))
        .collect()
}

/// The view's plain text, wrapped at `width` (each source line sanitised on its own).
fn plain_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for src in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let t = term_sanitize(src, TermCtx::Prose);
        for piece in wrap(&t.to_plain_string(), width) {
            // re-sanitising an already safe line only re-splits its runs
            out.push(Line::from(spans(&tt(&piece), Style::default())));
        }
    }
    while out.last().is_some_and(|l| l.width() == 0) {
        out.pop();
    }
    out
}

/// Greedy word wrap by display width; words longer than `width` are split.
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in s.split(' ') {
        let w = word.width();
        let sep = usize::from(!cur.is_empty());
        if cur_w + sep + w <= width {
            if sep == 1 {
                cur.push(' ');
            }
            cur.push_str(word);
            cur_w += sep + w;
            continue;
        }
        if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if w <= width {
            cur.push_str(word);
            cur_w = w;
            continue;
        }
        // split a long word by characters
        for ch in word.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if cur_w + cw > width && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push(ch);
            cur_w += cw;
        }
    }
    out.push(cur);
    out
}

/// One printed line (already checked: only SGR sequences, no other control) as ratatui spans.
fn sgr_line(s: &str) -> Line<'static> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut text = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('\u{1b}') {
        text.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(csi) = after.strip_prefix('[') else {
            // not SGR (excluded by the check): drop the rest of the line
            rest = "";
            break;
        };
        let Some(end) = csi.find('m') else {
            rest = "";
            break;
        };
        if !text.is_empty() {
            out.push(Span::styled(std::mem::take(&mut text), style));
        }
        style = apply_sgr(style, &csi[..end]);
        rest = &csi[end + 1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Span::styled(text, style));
    }
    // belt and braces: no control character survives (the check already guarantees it)
    for sp in &mut out {
        if sp.content.chars().any(char::is_control) {
            sp.content = sp
                .content
                .chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
                .into();
        }
    }
    Line::from(out)
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    for p in params.split(';') {
        let n: u8 = p.parse().unwrap_or(0);
        style = match n {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            9 => style.add_modifier(Modifier::CROSSED_OUT),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            29 => style.remove_modifier(Modifier::CROSSED_OUT),
            39 => style.fg(Color::Reset),
            30..=37 => style.fg(Color::Indexed(n - 30)),
            90..=97 => style.fg(Color::Indexed(n - 90 + 8)),
            _ => style,
        };
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn markdown_is_styled_and_wrapped() {
        let md = serde_json::json!({
            "$type": nsid::MARKDOWN,
            "text": "# Hi\n\nHello **everyone**, a \u{1b}]0;x\u{7} long line that wraps somewhere.",
        });
        let body = Body {
            doc: BodyDoc::Styled(styled_doc(&md).unwrap()),
            notices: vec!["truncated".into()],
        };
        let lines = body.lines(24, 2, &BTreeMap::new());
        let text = text_of(&lines);
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
        assert!(text.starts_with("  # Hi"), "{text:?}");
        assert!(text.contains("everyone"), "{text:?}");
        assert!(lines.iter().all(|l| l.width() <= 26), "{text:?}");
        assert!(text.ends_with("truncated"));
        let bold = lines.iter().flat_map(|l| l.spans.iter()).any(|s| {
            s.content.contains("everyone") && s.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(bold);
    }

    #[test]
    fn other_values_are_not_markdown() {
        assert!(styled_doc(&serde_json::json!({"$type": "x.y", "text": "a"})).is_none());
    }

    #[test]
    fn plain_text_wraps() {
        let lines = plain_lines("one two three four\nfive", 9);
        assert_eq!(text_of(&lines), "one two\nthree\nfour\nfive");
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }
}
