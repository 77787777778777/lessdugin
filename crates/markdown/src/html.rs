// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTML+MathML writer honouring the LFM/1 HTML contract (`docs/spec/lfm-1.md` §8): construct
//! mapping (§8.1), allowlist (§8.2), serialisation (§8.3) and fixed strings (§8.6).
//! The writer is a single forward iteration over SL with an explicit stack.

use crate::event::{Align, Ev, ImageRes, LinkRes, Tag, TagKind, match_indices};
use crate::lfm::Footnote;
use crate::math::MathRenderer;
use crate::media::{MediaClass, MediaItem};
use crate::url::{html_host_suffix, is_valid_cid, short_did};

/// `rel` of every external link.
pub const EXTERNAL_REL: &str = "nofollow ugc noopener noreferrer";

/// Fixed strings (§8.6).
pub mod fixed {
    /// Spoiler summary.
    pub const SPOILER: &str = "Spoiler";
    /// Details default summary.
    pub const DETAILS: &str = "Details";
    /// External image label prefix.
    pub const IMAGE_PREFIX: &str = "image: ";
    /// Footnote back-link.
    pub const BACK: &str = "\u{21A9}";
    /// Truncation notice.
    pub const TRUNCATED: &str = "[content truncated: too many elements]";
    /// `BodyError::TooLarge`.
    pub const TOO_LARGE: &str = "[body too large to render]";
    /// `BodyError::Encoding`.
    pub const ENCODING: &str = "[unreadable body: not valid UTF-8]";
    /// Terminal: spoiler hidden.
    pub const SPOILER_HIDDEN: &str = "[spoiler hidden]";
}

fn escape_into(out: &mut String, s: &str, attr: bool) {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\u{A0}' => out.push_str("&nbsp;"),
            // The HTML parser would rewrite these (input-stream preprocessing), breaking the
            // sanitiser round trip; content never carries them after pass 0, but record fields
            // (media alt and name) might.
            '\0' => out.push('\u{FFFD}'),
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            c => out.push(c),
        }
    }
}

/// Text escaping (§8.3): `&`, `<`, `>`, U+00A0.
pub(crate) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_into(&mut out, s, false);
    out
}

/// Attribute-value escaping (§8.3): `&`, `"`, `<`, `>`, U+00A0.
#[cfg_attr(not(feature = "math"), allow(dead_code))]
pub(crate) fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_into(&mut out, s, true);
    out
}

/// The code-block language class token (§8.1), if valid.
pub(crate) fn code_lang(info: &str) -> Option<String> {
    let l = info.split_whitespace().next()?.to_ascii_lowercase();
    ((1..=32).contains(&l.len())
        && l.bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'+' | b'-')))
    .then_some(l)
}

/// The output of the writer.
pub(crate) struct HtmlOut {
    pub html: String,
    pub anchors: Vec<String>,
    pub math_errors: u32,
}

enum Close {
    Str(&'static str),
    Owned(String),
    Link { emitted: bool, host: Option<String> },
    Figure(String),
    Table,
    TableHead,
}

struct TableState {
    aligns: Vec<Align>,
    in_head: bool,
    col: usize,
    body: bool,
}

struct Writer<'a> {
    out: String,
    anchors: Vec<String>,
    math_errors: u32,
    media: &'a [MediaItem],
    math: &'a mut MathRenderer,
}

impl Writer<'_> {
    fn text(&mut self, s: &str) {
        escape_into(&mut self.out, s, false);
    }

    fn attr(&mut self, s: &str) {
        escape_into(&mut self.out, s, true);
    }

    /// One media placeholder (§6.2) for item `i` with alt `a`.
    fn media_item(&mut self, i: usize, a: &str) {
        let m = &self.media[i];
        let cid = m.cid();
        if !is_valid_cid(cid) {
            self.out.push_str("<span class=\"media-missing\">");
            self.text(a);
            self.out.push_str("</span>");
            return;
        }
        if m.class() == MediaClass::Image {
            self.out.push_str("<img data-cid=\"");
            self.attr(cid);
            self.out.push_str("\" alt=\"");
            self.attr(a);
            self.out.push('"');
            if let Some((w, h)) = m.dims() {
                self.out.push_str(&format!(" width=\"{w}\" height=\"{h}\""));
            }
            self.out.push('>');
        } else {
            let l = m.label(a);
            self.out.push_str("<span class=\"media\" data-cid=\"");
            self.attr(cid);
            self.out.push_str("\">");
            self.text(&l);
            self.out.push_str("</span>");
        }
    }

    fn external_open(&mut self, url: &str, class: Option<&str>) {
        self.out.push_str("<a href=\"");
        self.attr(url);
        self.out.push('"');
        if let Some(c) = class {
            self.out.push_str(" class=\"");
            self.out.push_str(c);
            self.out.push('"');
        }
        self.out.push_str(" rel=\"");
        self.out.push_str(EXTERNAL_REL);
        self.out.push_str("\">");
    }

    fn host_span(&mut self, host: &str) {
        self.out.push_str("<span class=\"link-host\">");
        self.text(host);
        self.out.push_str("</span>");
    }

    /// Write one flow (the body or a footnote's content).
    fn flow(&mut self, evs: &[Ev]) {
        let m = match_indices(evs);
        let mut stack: Vec<Close> = Vec::new();
        let mut tables: Vec<TableState> = Vec::new();
        let mut in_link = 0usize;
        let mut i = 0usize;
        while i < evs.len() {
            match &evs[i] {
                Ev::Start(tag) => match tag {
                    Tag::Paragraph => {
                        self.out.push_str("<p dir=\"auto\">");
                        stack.push(Close::Str("</p>\n"));
                    }
                    Tag::Heading { level, id } => {
                        self.out.push_str(&format!("<h{level} id=\""));
                        self.attr(id);
                        self.out.push_str("\" dir=\"auto\">");
                        self.anchors.push(id.clone());
                        stack.push(Close::Owned(format!("</h{level}>\n")));
                    }
                    Tag::BlockQuote(None) => {
                        self.out.push_str("<blockquote>\n");
                        stack.push(Close::Str("</blockquote>\n"));
                    }
                    Tag::BlockQuote(Some(k)) => {
                        self.out
                            .push_str(&format!("<div class=\"{}\">\n", k.class()));
                        stack.push(Close::Str("</div>\n"));
                    }
                    Tag::Spoiler => {
                        self.out
                            .push_str("<details class=\"spoiler\">\n<summary dir=\"auto\">");
                        self.out.push_str(fixed::SPOILER);
                        self.out.push_str("</summary>\n");
                        stack.push(Close::Str("</details>\n"));
                    }
                    Tag::Details => {
                        self.out.push_str("<details>\n");
                        stack.push(Close::Str("</details>\n"));
                    }
                    Tag::Summary => {
                        self.out.push_str("<summary dir=\"auto\">");
                        if matches!(evs.get(i + 1), Some(Ev::End(TagKind::Summary))) {
                            self.out.push_str(fixed::DETAILS);
                        }
                        stack.push(Close::Str("</summary>\n"));
                    }
                    Tag::Figure(c) => {
                        self.out.push_str("<figure>\n");
                        stack.push(Close::Figure(c.clone()));
                    }
                    Tag::CodeBlock(info) => {
                        match info.as_deref().and_then(code_lang) {
                            Some(l) => self
                                .out
                                .push_str(&format!("<pre><code class=\"language-{l}\">")),
                            None => self.out.push_str("<pre><code>"),
                        }
                        stack.push(Close::Str("</code></pre>\n"));
                    }
                    Tag::List(None) => {
                        self.out.push_str("<ul>\n");
                        stack.push(Close::Str("</ul>\n"));
                    }
                    Tag::List(Some(n)) => {
                        if *n == 1 {
                            self.out.push_str("<ol>\n");
                        } else {
                            self.out.push_str(&format!("<ol start=\"{n}\">\n"));
                        }
                        stack.push(Close::Str("</ol>\n"));
                    }
                    Tag::Item => {
                        self.out.push_str("<li dir=\"auto\">");
                        stack.push(Close::Str("</li>\n"));
                    }
                    Tag::Table(aligns) => {
                        self.out.push_str("<table>\n");
                        tables.push(TableState {
                            aligns: aligns.clone(),
                            in_head: false,
                            col: 0,
                            body: false,
                        });
                        stack.push(Close::Table);
                    }
                    Tag::TableHead => {
                        self.out.push_str("<thead>\n<tr>\n");
                        if let Some(t) = tables.last_mut() {
                            t.in_head = true;
                            t.col = 0;
                        }
                        stack.push(Close::TableHead);
                    }
                    Tag::TableRow => {
                        if let Some(t) = tables.last_mut() {
                            if !t.body {
                                self.out.push_str("<tbody>\n");
                                t.body = true;
                            }
                            t.col = 0;
                        }
                        self.out.push_str("<tr>\n");
                        stack.push(Close::Str("</tr>\n"));
                    }
                    Tag::TableCell => {
                        let (cell, align) = match tables.last_mut() {
                            Some(t) => {
                                let a = t.aligns.get(t.col).copied().unwrap_or(Align::None);
                                t.col += 1;
                                (if t.in_head { "th" } else { "td" }, a)
                            }
                            None => ("td", Align::None),
                        };
                        let class = match align {
                            Align::None => "",
                            Align::Left => " class=\"ta-l\"",
                            Align::Center => " class=\"ta-c\"",
                            Align::Right => " class=\"ta-r\"",
                        };
                        self.out.push_str(&format!("<{cell}{class} dir=\"auto\">"));
                        stack.push(Close::Str(if cell == "th" { "</th>\n" } else { "</td>\n" }));
                    }
                    Tag::Emphasis => {
                        self.out.push_str("<em>");
                        stack.push(Close::Str("</em>"));
                    }
                    Tag::Strong => {
                        self.out.push_str("<strong>");
                        stack.push(Close::Str("</strong>"));
                    }
                    Tag::Strikethrough => {
                        self.out.push_str("<del>");
                        stack.push(Close::Str("</del>"));
                    }
                    Tag::Link(d) => {
                        let res = if in_link > 0 {
                            &LinkRes::Dropped
                        } else {
                            &d.res
                        };
                        match res {
                            LinkRes::External { url, host } | LinkRes::Mailto { url, host } => {
                                self.external_open(url, None);
                                let host = (!host.is_empty() && html_host_suffix(&d.inner, host))
                                    .then(|| host.clone());
                                in_link += 1;
                                stack.push(Close::Link {
                                    emitted: true,
                                    host,
                                });
                            }
                            LinkRes::Record(uri) => {
                                let u = uri.as_str().strip_prefix("at://").unwrap_or(uri.as_str());
                                self.out.push_str("<a href=\"#/at/");
                                self.attr(u);
                                self.out.push_str("\" data-at-uri=\"");
                                self.attr(uri.as_str());
                                self.out.push_str("\" class=\"at-link\">");
                                in_link += 1;
                                stack.push(Close::Link {
                                    emitted: true,
                                    host: None,
                                });
                            }
                            LinkRes::Profile(did) => {
                                self.out.push_str("<a href=\"#/u/");
                                self.attr(did.as_str());
                                self.out.push_str("\" data-did=\"");
                                self.attr(did.as_str());
                                self.out.push_str("\" class=\"at-link\">");
                                in_link += 1;
                                stack.push(Close::Link {
                                    emitted: true,
                                    host: None,
                                });
                            }
                            LinkRes::Mention(did) => {
                                self.out.push_str("<a href=\"#/u/");
                                self.attr(did.as_str());
                                self.out.push_str("\" data-did=\"");
                                self.attr(did.as_str());
                                self.out.push_str("\" class=\"mention\">@");
                                self.text(&short_did(did.as_str()));
                                self.out.push_str("</a>");
                                i = m[i].min(evs.len() - 1) + 1;
                                continue;
                            }
                            LinkRes::Anchor(v) => {
                                self.out.push_str("<a data-anchor=\"");
                                self.attr(v);
                                self.out.push_str("\">");
                                in_link += 1;
                                stack.push(Close::Link {
                                    emitted: true,
                                    host: None,
                                });
                            }
                            LinkRes::Dropped | LinkRes::Pending => {
                                stack.push(Close::Link {
                                    emitted: false,
                                    host: None,
                                });
                            }
                        }
                    }
                    Tag::Image(d) => {
                        match &d.res {
                            ImageRes::Media(idx) => {
                                let a = self.media[*idx].alt_text(&d.alt);
                                self.media_item(*idx, &a);
                            }
                            ImageRes::Missing => {
                                self.out.push_str("<span class=\"media-missing\">");
                                self.text(&d.alt);
                                self.out.push_str("</span>");
                            }
                            ImageRes::External { url, host } => {
                                let label = if d.alt.is_empty() {
                                    host.as_str()
                                } else {
                                    d.alt.as_str()
                                };
                                if in_link > 0 {
                                    self.text(fixed::IMAGE_PREFIX);
                                    self.text(label);
                                } else {
                                    self.external_open(url, Some("ext-image"));
                                    self.text(fixed::IMAGE_PREFIX);
                                    self.text(label);
                                    if html_host_suffix(&d.alt, host) {
                                        self.host_span(host);
                                    }
                                    self.out.push_str("</a>");
                                }
                            }
                            ImageRes::AltOnly | ImageRes::Pending => self.text(&d.alt),
                        }
                        if matches!(stack.last(), Some(Close::Figure(_))) {
                            self.out.push('\n');
                        }
                        i = m[i].min(evs.len() - 1) + 1;
                        continue;
                    }
                    Tag::FootnoteDef { .. } => {}
                },
                Ev::End(_) => match stack.pop() {
                    Some(Close::Str(s)) => self.out.push_str(s),
                    Some(Close::Owned(s)) => self.out.push_str(&s),
                    Some(Close::Link { emitted, host }) => {
                        if emitted {
                            if let Some(h) = host {
                                self.host_span(&h);
                            }
                            self.out.push_str("</a>");
                            in_link -= 1;
                        }
                    }
                    Some(Close::Figure(c)) => {
                        self.out.push_str("<figcaption dir=\"auto\">");
                        self.text(&c);
                        self.out.push_str("</figcaption>\n</figure>\n");
                    }
                    Some(Close::Table) => {
                        if tables.pop().is_some_and(|t| t.body) {
                            self.out.push_str("</tbody>\n");
                        }
                        self.out.push_str("</table>\n");
                    }
                    Some(Close::TableHead) => {
                        if let Some(t) = tables.last_mut() {
                            t.in_head = false;
                        }
                        self.out.push_str("</tr>\n</thead>\n");
                    }
                    None => {}
                },
                Ev::Text(t) | Ev::Literal(t) => self.text(t),
                Ev::Code(t) => {
                    self.out.push_str("<code>");
                    self.text(t);
                    self.out.push_str("</code>");
                }
                Ev::InlineMath(t) | Ev::DisplayMath(t) => {
                    let (h, err) = self.math.render(t, matches!(evs[i], Ev::DisplayMath(_)));
                    self.out.push_str(&h);
                    if err {
                        self.math_errors += 1;
                    }
                }
                Ev::FnRef(r) => {
                    if r.ord > 0 {
                        let id = format!("md-fnref-{}-{}", r.ord, r.k);
                        self.out.push_str(&format!("<sup id=\"{id}\">"));
                        if in_link > 0 {
                            self.out.push_str(&r.ord.to_string());
                        } else {
                            self.out
                                .push_str(&format!("<a data-anchor=\"md-fn-{0}\">{0}</a>", r.ord));
                        }
                        self.out.push_str("</sup>");
                        self.anchors.push(id);
                    }
                }
                Ev::SoftBreak => self.out.push('\n'),
                Ev::HardBreak => self.out.push_str("<br>\n"),
                Ev::Rule => self.out.push_str("<hr>\n"),
                Ev::Task(b) => self.out.push_str(if *b {
                    "<input type=\"checkbox\" checked=\"\" disabled=\"\"> "
                } else {
                    "<input type=\"checkbox\" disabled=\"\"> "
                }),
            }
            i += 1;
        }
    }
}

/// Write the whole document: root, body, truncation notice, footnotes and gallery.
pub(crate) fn write_document(
    body: &[Ev],
    notes: &[Footnote],
    truncated: bool,
    media: &[MediaItem],
    used: &[u16],
    math: &mut MathRenderer,
) -> HtmlOut {
    let mut w = Writer {
        out: String::new(),
        anchors: Vec::new(),
        math_errors: 0,
        media,
        math,
    };
    w.out.push_str("<div class=\"lfm\" dir=\"auto\">\n");
    w.flow(body);
    if truncated {
        notice(&mut w.out, fixed::TRUNCATED);
    }
    if !notes.is_empty() {
        w.out.push_str("<section class=\"footnotes\">\n<ol>\n");
        for n in notes {
            let id = format!("md-fn-{}", n.ord);
            w.out.push_str(&format!("<li id=\"{id}\" dir=\"auto\">"));
            w.anchors.push(id);
            w.flow(&n.events);
            for k in 1..=n.refs {
                if k > 1 {
                    w.out.push(' ');
                }
                let mark = if k == 1 {
                    fixed::BACK.to_owned()
                } else {
                    format!("{}{k}", fixed::BACK)
                };
                w.out.push_str(&format!(
                    "<a data-anchor=\"md-fnref-{}-{k}\" class=\"fn-back\">{mark}</a>",
                    n.ord
                ));
            }
            w.out.push_str("</li>\n");
        }
        w.out.push_str("</ol>\n</section>\n");
    }
    let gallery: Vec<usize> = (0..media.len())
        .filter(|i| !used.iter().any(|&u| usize::from(u) == *i))
        .collect();
    if !gallery.is_empty() {
        w.out.push_str("<div class=\"gallery\">\n");
        for i in gallery {
            let a = media[i].alt.clone().unwrap_or_default();
            w.media_item(i, &a);
            w.out.push('\n');
        }
        w.out.push_str("</div>\n");
    }
    w.out.push_str("</div>\n");
    HtmlOut {
        html: w.out,
        anchors: w.anchors,
        math_errors: w.math_errors,
    }
}

fn notice(out: &mut String, text: &str) {
    out.push_str("<p class=\"notice\" dir=\"auto\">");
    escape_into(out, text, false);
    out.push_str("</p>\n");
}

/// The HTML of a [`crate::BodyError`] (§1.3).
pub(crate) fn error_document(text: &str) -> String {
    let mut out = String::from("<div class=\"lfm\" dir=\"auto\">\n");
    notice(&mut out, text);
    out.push_str("</div>\n");
    out
}
