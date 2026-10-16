// SPDX-License-Identifier: AGPL-3.0-or-later
//! LFM/1 parsing (`docs/spec/lfm-1.md` §2–§4): the pinned pulldown-cmark profile, pass 0
//! (normalisation) and post-passes 1–6. Every pass is a forward iteration with explicit stacks.

use std::collections::{HashMap, HashSet};

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, LinkType, Options, Parser, Tag as PTag, TagEnd,
};
use unicase::UniCase;

use crate::event::{
    AlertKind, Align, Ev, FnRef, ImageData, ImageRes, LinkData, LinkKind, LinkRes, Tag, TagKind,
    match_indices, push_literal, push_text,
};
use crate::media::MediaItem;
use crate::plain::{plain_raw, slug};
use crate::url::{Href, HrefCtx, cid_of, safe_href};

/// The pulldown-cmark options of `lfm/1` (§2): exactly the six named "on" flags, `0x0C1E`.
pub const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_MATH)
    .union(Options::ENABLE_GFM);

/// Pass 0 pulls at most this many parser events (§3 step 1).
pub const EVENT_CAP: usize = 1 << 20;

/// The stream after pass 3 (the input of P, §10).
pub(crate) struct S3 {
    pub events: Vec<Ev>,
    pub truncated: bool,
}

/// Parse and run pass 0 and passes 1–3.
pub(crate) fn parse_to_s3(src: &str) -> S3 {
    let (s0, truncated) = pass0(src);
    let s1 = pass1(s0);
    let s2 = pass2(s1);
    let s3 = pass3(s2);
    S3 {
        events: s3,
        truncated,
    }
}

// ---------------------------------------------------------------------------------------------
// Pass 0 (§3)

/// §3 step 3: `"\r\n"` → `"\n"`, then `"\r"` → `"\n"`. U+0000 also becomes U+FFFD, as
/// CommonMark 0.31.2 §2.3 requires and pulldown-cmark 0.13.4 does not do (implementation
/// decision, see `testdata/vectors/lfm1/NOTES.md`).
fn norm_nl(s: &str) -> String {
    if !s.contains(['\r', '\0']) {
        return s.to_owned();
    }
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "\u{FFFD}")
}

/// `lines(s)` (§3 step 2): `SoftBreak` between pieces, `Literal` for non-empty pieces.
fn push_lines(out: &mut Vec<Ev>, s: &str) {
    for (i, piece) in s.split('\n').enumerate() {
        if i > 0 {
            out.push(Ev::SoftBreak);
        }
        push_literal(out, piece);
    }
}

fn flush_html_block(out: &mut Vec<Ev>, buf: &str) {
    let b = norm_nl(buf);
    let b = b.strip_suffix('\n').unwrap_or(&b);
    out.push(Ev::Start(Tag::Paragraph));
    push_lines(out, b);
    out.push(Ev::End(TagKind::Paragraph));
}

/// One entry per parser `Start` that is still open.
enum Open {
    Emitted(TagKind),
    Dropped,
    HtmlBlock,
}

fn link_kind(t: LinkType) -> Option<LinkKind> {
    Some(match t {
        LinkType::Inline => LinkKind::Inline,
        LinkType::Reference | LinkType::ReferenceUnknown => LinkKind::Reference,
        LinkType::Collapsed | LinkType::CollapsedUnknown => LinkKind::Collapsed,
        LinkType::Shortcut | LinkType::ShortcutUnknown => LinkKind::Shortcut,
        LinkType::Autolink => LinkKind::Autolink,
        LinkType::Email => LinkKind::Email,
        LinkType::WikiLink { .. } => return None,
    })
}

fn map_tag(t: PTag<'_>) -> Option<Tag> {
    Some(match t {
        PTag::Paragraph => Tag::Paragraph,
        PTag::Heading { level, .. } => Tag::Heading {
            level: level as u8,
            id: String::new(),
        },
        PTag::BlockQuote(k) => Tag::BlockQuote(k.map(|k| match k {
            BlockQuoteKind::Note => AlertKind::Note,
            BlockQuoteKind::Tip => AlertKind::Tip,
            BlockQuoteKind::Important => AlertKind::Important,
            BlockQuoteKind::Warning => AlertKind::Warning,
            BlockQuoteKind::Caution => AlertKind::Caution,
        })),
        PTag::CodeBlock(CodeBlockKind::Indented) => Tag::CodeBlock(None),
        PTag::CodeBlock(CodeBlockKind::Fenced(info)) => Tag::CodeBlock(Some(norm_nl(&info))),
        PTag::List(start) => Tag::List(start),
        PTag::Item => Tag::Item,
        PTag::FootnoteDefinition(label) => Tag::FootnoteDef {
            label: norm_nl(&label),
            ord: None,
        },
        PTag::Table(aligns) => Tag::Table(
            aligns
                .into_iter()
                .map(|a| match a {
                    Alignment::None => Align::None,
                    Alignment::Left => Align::Left,
                    Alignment::Center => Align::Center,
                    Alignment::Right => Align::Right,
                })
                .collect(),
        ),
        PTag::TableHead => Tag::TableHead,
        PTag::TableRow => Tag::TableRow,
        PTag::TableCell => Tag::TableCell,
        PTag::Emphasis => Tag::Emphasis,
        PTag::Strong => Tag::Strong,
        PTag::Strikethrough => Tag::Strikethrough,
        PTag::Link {
            link_type,
            dest_url,
            title,
            ..
        } => Tag::Link(Box::new(LinkData {
            kind: link_kind(link_type)?,
            dest: norm_nl(&dest_url),
            title: norm_nl(&title),
            inner: String::new(),
            res: LinkRes::Pending,
        })),
        PTag::Image {
            link_type,
            dest_url,
            title,
            ..
        } => Tag::Image(Box::new(ImageData {
            kind: link_kind(link_type)?,
            dest: norm_nl(&dest_url),
            title: norm_nl(&title),
            alt: String::new(),
            res: ImageRes::Pending,
        })),
        // Superscript, Subscript, definition lists and metadata blocks cannot occur under
        // OPTIONS; the tag is dropped and its content kept (§3 step 2). HtmlBlock is handled
        // by the caller.
        PTag::Superscript
        | PTag::Subscript
        | PTag::DefinitionList
        | PTag::DefinitionListTitle
        | PTag::DefinitionListDefinition
        | PTag::MetadataBlock(_)
        | PTag::HtmlBlock => return None,
    })
}

/// Push an event with text coalescing (§3 step 4).
fn push_ev(out: &mut Vec<Ev>, e: Ev) {
    match e {
        Ev::Text(t) => push_text(out, &t),
        Ev::Literal(t) => push_literal(out, &t),
        e => out.push(e),
    }
}

/// Pass 0: E → S0. Returns the stream and whether the event cap truncated it.
fn pass0(src: &str) -> (Vec<Ev>, bool) {
    let mut parser = Parser::new_ext(src, OPTIONS);
    let mut out: Vec<Ev> = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    let mut html: Option<String> = None;
    let mut pulled = 0usize;
    let mut truncated = false;
    loop {
        if pulled == EVENT_CAP {
            truncated = parser.next().is_some();
            break;
        }
        let Some(ev) = parser.next() else { break };
        pulled += 1;
        match ev {
            Event::Start(PTag::HtmlBlock) => {
                html = Some(String::new());
                open.push(Open::HtmlBlock);
            }
            Event::Start(t) => match map_tag(t) {
                Some(tag) => {
                    open.push(Open::Emitted(tag.kind()));
                    out.push(Ev::Start(tag));
                }
                None => open.push(Open::Dropped),
            },
            Event::End(TagEnd::HtmlBlock) => {
                open.pop();
                if let Some(buf) = html.take() {
                    flush_html_block(&mut out, &buf);
                }
            }
            Event::End(_) => {
                if let Some(Open::Emitted(k)) = open.pop() {
                    out.push(Ev::End(k));
                }
            }
            Event::Text(t) => push_text(&mut out, &norm_nl(&t)),
            Event::Code(t) => out.push(Ev::Code(norm_nl(&t))),
            Event::InlineMath(t) => out.push(Ev::InlineMath(norm_nl(&t))),
            Event::DisplayMath(t) => out.push(Ev::DisplayMath(norm_nl(&t))),
            Event::Html(t) => match html.as_mut() {
                Some(buf) => buf.push_str(&t),
                None => push_lines(&mut out, &norm_nl(&t)),
            },
            Event::InlineHtml(t) => push_lines(&mut out, &norm_nl(&t)),
            Event::FootnoteReference(l) => out.push(Ev::FnRef(FnRef {
                label: norm_nl(&l),
                ord: 0,
                k: 0,
            })),
            Event::SoftBreak => out.push(Ev::SoftBreak),
            Event::HardBreak => out.push(Ev::HardBreak),
            Event::Rule => out.push(Ev::Rule),
            Event::TaskListMarker(b) => out.push(Ev::Task(b)),
        }
    }
    if truncated {
        if let Some(buf) = html.take() {
            flush_html_block(&mut out, &buf);
        }
        while let Some(o) = open.pop() {
            if let Open::Emitted(k) = o {
                out.push(Ev::End(k));
            }
        }
    }
    // Step 5: dangling footnote references.
    let defined: HashSet<UniCase<String>> = out
        .iter()
        .filter_map(|e| match e {
            Ev::Start(Tag::FootnoteDef { label, .. }) => Some(UniCase::new(label.clone())),
            _ => None,
        })
        .collect();
    let dangling =
        |e: &Ev| matches!(e, Ev::FnRef(r) if !defined.contains(&UniCase::new(r.label.clone())));
    if out.iter().any(dangling) {
        let mut fixed = Vec::with_capacity(out.len());
        for e in out {
            if dangling(&e) {
                if let Ev::FnRef(r) = e {
                    push_text(&mut fixed, &format!("[^{}]", r.label));
                }
            } else {
                push_ev(&mut fixed, e);
            }
        }
        out = fixed;
    }
    (out, truncated)
}

// ---------------------------------------------------------------------------------------------
// Pass 1: GFM extended autolinks (§4.1)

fn pass1(evs: Vec<Ev>) -> Vec<Ev> {
    let mut out = Vec::with_capacity(evs.len());
    let mut skip = 0usize;
    for e in evs {
        match e {
            Ev::Start(Tag::Link(_) | Tag::Image(_) | Tag::CodeBlock(_)) => {
                skip += 1;
                out.push(e);
            }
            Ev::End(TagKind::Link | TagKind::Image | TagKind::CodeBlock) => {
                skip = skip.saturating_sub(1);
                out.push(e);
            }
            Ev::Text(t) if skip == 0 => autolink(&t, &mut out),
            e => out.push(e),
        }
    }
    out
}

fn is_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

fn boundary(t: &[char], i: usize) -> bool {
    i == 0 || t[i - 1].is_whitespace() || matches!(t[i - 1], '*' | '_' | '~' | '(')
}

fn prefix_at(t: &[char], i: usize) -> Option<usize> {
    for p in ["www.", "http://", "https://"] {
        let n = p.len();
        if i + n <= t.len()
            && t[i..i + n]
                .iter()
                .zip(p.chars())
                .all(|(a, b)| a.eq_ignore_ascii_case(&b))
        {
            return Some(n);
        }
    }
    None
}

fn valid_domain(d: &[char]) -> bool {
    let segs: Vec<&[char]> = d.split(|&c| c == '.').collect();
    if segs.len() < 2 {
        return false;
    }
    if !segs.iter().all(|s| {
        !s.is_empty()
            && s.iter()
                .all(|&c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }) {
        return false;
    }
    segs[segs.len() - 2..].iter().all(|s| !s.contains(&'_'))
}

/// cmark-gfm `autolink_delim`, linear form. Returns the kept length.
fn trim_trailing(s: &[char]) -> usize {
    let opens = s.iter().filter(|&&c| c == '(').count();
    let mut closes = s.iter().filter(|&&c| c == ')').count();
    let mut len = s.len();
    while len > 0 {
        let c = s[len - 1];
        if matches!(
            c,
            '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '\'' | '"'
        ) {
            len -= 1;
        } else if c == ';' {
            let semi = len - 1;
            let mut k = semi;
            while k > 0 && s[k - 1].is_ascii_alphabetic() {
                k -= 1;
            }
            if k < semi && k > 0 && s[k - 1] == '&' {
                len = k - 1;
            } else {
                len -= 1;
            }
        } else if c == ')' && closes > opens {
            len -= 1;
            closes -= 1;
        } else {
            break;
        }
    }
    len
}

fn autolink(text: &str, out: &mut Vec<Ev>) {
    let t: Vec<char> = text.chars().collect();
    let n = t.len();
    let mut last = 0usize;
    let mut i = 0usize;
    let collect = |a: usize, b: usize| -> String { t[a..b].iter().collect() };
    while i < n {
        if boundary(&t, i)
            && let Some(plen) = prefix_at(&t, i)
        {
            let www = plen == 4;
            let d0 = if www { i } else { i + plen };
            let mut d1 = d0;
            while d1 < n && is_domain_char(t[d1]) {
                d1 += 1;
                if d1 - d0 > 253 {
                    break;
                }
            }
            if d1 - d0 <= 253 && valid_domain(&t[d0..d1]) {
                let mut e = d1;
                while e < n && !t[e].is_whitespace() && t[e] != '<' {
                    e += 1;
                }
                let e = i + trim_trailing(&t[i..e]);
                if e > d0 && valid_domain(&t[d0..d1.min(e)]) {
                    if i > last {
                        push_text(out, &collect(last, i));
                    }
                    let lit = collect(i, e);
                    let dest = if www {
                        format!("https://{lit}")
                    } else {
                        lit.clone()
                    };
                    out.push(Ev::Start(Tag::Link(Box::new(LinkData {
                        kind: LinkKind::ExtAutolink,
                        dest,
                        title: String::new(),
                        inner: String::new(),
                        res: LinkRes::Pending,
                    }))));
                    out.push(Ev::Text(lit));
                    out.push(Ev::End(TagKind::Link));
                    last = e;
                    i = e;
                    continue;
                }
            }
        }
        i += 1;
    }
    if last < n {
        push_text(out, &collect(last, n));
    }
}

// ---------------------------------------------------------------------------------------------
// Pass 2: spoiler and details blocks (§4.2)

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    Keep,
    Spoiler,
    Details,
}

struct Marker {
    kind: Quote,
    summary: Vec<Ev>,
    /// Index to resume copying from (the remaining paragraph content, or after the paragraph).
    resume: usize,
    /// Whether the (non-empty) paragraph must be re-opened before `resume`.
    reopen_paragraph: bool,
}

fn match_marker(s: &[Ev], b: usize) -> Option<Marker> {
    let i = b + 1;
    if !matches!(s.get(i), Some(Ev::Start(Tag::Paragraph))) {
        return None;
    }
    let Some(Ev::Text(t)) = s.get(i + 1) else {
        return None;
    };
    let head = t.get(..10)?;
    let kind = if head.eq_ignore_ascii_case("[!spoiler]") {
        Quote::Spoiler
    } else if head.eq_ignore_ascii_case("[!details]") {
        Quote::Details
    } else {
        return None;
    };
    let r = &t[10..];
    match kind {
        Quote::Spoiler => {
            if !r.chars().all(|c| c == ' ' || c == '\t') {
                return None;
            }
            if !matches!(
                s.get(i + 2),
                Some(Ev::SoftBreak | Ev::HardBreak | Ev::End(TagKind::Paragraph))
            ) {
                return None;
            }
        }
        _ => {
            if !(r.is_empty() || r.starts_with([' ', '\t'])) {
                return None;
            }
        }
    }
    // k: first SoftBreak/HardBreak at inline depth 0 after i+1, or End(Paragraph).
    let mut depth = 0usize;
    let mut k = i + 2;
    while k < s.len() {
        match &s[k] {
            Ev::Start(_) => depth += 1,
            Ev::End(TagKind::Paragraph) if depth == 0 => break,
            Ev::End(_) => depth = depth.saturating_sub(1),
            Ev::SoftBreak | Ev::HardBreak if depth == 0 => break,
            _ => {}
        }
        k += 1;
    }
    let mut summary = Vec::new();
    if kind == Quote::Details {
        let r0 = r.trim_start();
        if !r0.is_empty() {
            summary.push(Ev::Text(r0.to_owned()));
        }
        summary.extend(s[i + 2..k].iter().cloned());
        if let Some(Ev::Text(last)) = summary.last_mut() {
            let trimmed = last.trim_end().len();
            last.truncate(trimmed);
        }
        summary.retain(|e| !matches!(e, Ev::Text(t) if t.is_empty()));
    }
    let after = if matches!(s.get(k), Some(Ev::SoftBreak | Ev::HardBreak)) {
        k + 1
    } else {
        k
    };
    let (resume, reopen_paragraph) = if matches!(s.get(after), Some(Ev::End(TagKind::Paragraph))) {
        (after + 1, false)
    } else {
        (after, true)
    };
    Some(Marker {
        kind,
        summary,
        resume,
        reopen_paragraph,
    })
}

fn pass2(s: Vec<Ev>) -> Vec<Ev> {
    let mut out = Vec::with_capacity(s.len() + 4);
    let mut stack: Vec<Quote> = Vec::new();
    let mut i = 0usize;
    while i < s.len() {
        match &s[i] {
            Ev::Start(Tag::BlockQuote(None)) => {
                if let Some(m) = match_marker(&s, i) {
                    match m.kind {
                        Quote::Spoiler => out.push(Ev::Start(Tag::Spoiler)),
                        _ => {
                            out.push(Ev::Start(Tag::Details));
                            out.push(Ev::Start(Tag::Summary));
                            out.extend(m.summary);
                            out.push(Ev::End(TagKind::Summary));
                        }
                    }
                    if m.reopen_paragraph {
                        out.push(Ev::Start(Tag::Paragraph));
                    }
                    stack.push(m.kind);
                    i = m.resume;
                    continue;
                }
                stack.push(Quote::Keep);
                out.push(s[i].clone());
            }
            Ev::Start(Tag::BlockQuote(Some(_))) => {
                stack.push(Quote::Keep);
                out.push(s[i].clone());
            }
            Ev::End(TagKind::BlockQuote) => match stack.pop() {
                Some(Quote::Spoiler) => out.push(Ev::End(TagKind::Spoiler)),
                Some(Quote::Details) => out.push(Ev::End(TagKind::Details)),
                _ => out.push(s[i].clone()),
            },
            e => out.push(e.clone()),
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Pass 3: figures (§4.3)

fn pass3(mut s: Vec<Ev>) -> Vec<Ev> {
    let m = match_indices(&s);
    for i in 0..s.len() {
        if !matches!(s[i], Ev::Start(Tag::Paragraph)) {
            continue;
        }
        let end = m[i];
        if end >= s.len() || i + 1 >= end {
            continue;
        }
        let caption = match &s[i + 1] {
            Ev::Start(Tag::Image(d)) if m[i + 1] + 1 == end => d.title.trim().to_owned(),
            _ => continue,
        };
        if caption.is_empty() {
            continue;
        }
        s[i] = Ev::Start(Tag::Figure(caption));
        s[end] = Ev::End(TagKind::Figure);
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Passes 4 and 5: mentions and URL policy (§4.4, §4.5)

pub(crate) fn pass45(s: &mut [Ev], media: &[MediaItem]) {
    let m = match_indices(s);
    let mut image_depth = 0usize;
    for i in 0..s.len() {
        match &s[i] {
            Ev::Start(Tag::Link(_)) => {
                let end = m[i].min(s.len());
                let inner = plain_raw(&s[i + 1..end], false);
                let first_at = matches!(s.get(i + 1), Some(Ev::Text(t)) if t.starts_with('@'));
                if let Ev::Start(Tag::Link(d)) = &mut s[i] {
                    let dest = if d.kind == LinkKind::Email {
                        format!("mailto:{}", d.dest)
                    } else {
                        d.dest.clone()
                    };
                    d.inner = inner;
                    d.res = match safe_href(&dest, HrefCtx::Markdown) {
                        Some(Href::External { url, host }) => LinkRes::External { url, host },
                        Some(Href::Mailto { url, host }) => LinkRes::Mailto { url, host },
                        Some(Href::Record { uri }) => LinkRes::Record(uri),
                        Some(Href::Profile { did }) if first_at => LinkRes::Mention(did),
                        Some(Href::Profile { did }) => LinkRes::Profile(did),
                        Some(Href::Anchor { id }) => LinkRes::Anchor(id),
                        Some(Href::ProfileHandle { .. }) | None => LinkRes::Dropped,
                    };
                }
            }
            Ev::Start(Tag::Image(_)) => {
                // Only top-level images are ever rendered (nested images are part of the alt),
                // so only they need their alt; this keeps the pass linear.
                let alt = if image_depth == 0 {
                    plain_raw(&s[i + 1..m[i].min(s.len())], false)
                } else {
                    String::new()
                };
                image_depth += 1;
                if let Ev::Start(Tag::Image(d)) = &mut s[i] {
                    d.alt = alt;
                    d.res = match cid_of(&d.dest) {
                        Some(Ok(c)) => match media.iter().position(|mi| mi.cid() == c) {
                            Some(idx) => ImageRes::Media(idx),
                            None => ImageRes::Missing,
                        },
                        Some(Err(())) => ImageRes::Missing,
                        None => match safe_href(&d.dest, HrefCtx::Markdown) {
                            Some(Href::External { url, host }) => ImageRes::External { url, host },
                            _ => ImageRes::AltOnly,
                        },
                    };
                }
            }
            Ev::End(TagKind::Image) => image_depth = image_depth.saturating_sub(1),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pass 6: ids (§4.6)

/// Assign heading ids and footnote ordinals. Returns the number of references per ordinal
/// (index 0 unused).
pub(crate) fn pass6(s: &mut [Ev]) -> Vec<u32> {
    let m = match_indices(s);
    // Heading ids (github-slugger dedup).
    let mut seen: HashSet<String> = HashSet::new();
    let mut counts: HashMap<String, u32> = HashMap::new();
    for i in 0..s.len() {
        if !matches!(s[i], Ev::Start(Tag::Heading { .. })) {
            continue;
        }
        let base = slug(&plain_raw(&s[i + 1..m[i].min(s.len())], false));
        let mut r = base.clone();
        while seen.contains(&r) {
            let n = counts.entry(base.clone()).or_insert(0);
            *n += 1;
            r = format!("{base}-{n}");
        }
        seen.insert(r.clone());
        if let Ev::Start(Tag::Heading { id, .. }) = &mut s[i] {
            *id = format!("md-h-{r}");
        }
    }

    // Footnotes. Owner of an event: the innermost enclosing definition (its start index), or
    // the body.
    const BODY: usize = usize::MAX;
    let mut first_def: HashMap<UniCase<String>, usize> = HashMap::new();
    let mut refs_of: HashMap<usize, Vec<UniCase<String>>> = HashMap::new();
    let mut owner_of_ref: Vec<(usize, usize)> = Vec::new(); // (event index, owner)
    let mut stack: Vec<usize> = Vec::new();
    for (i, e) in s.iter().enumerate() {
        match e {
            Ev::Start(Tag::FootnoteDef { label, .. }) => {
                first_def.entry(UniCase::new(label.clone())).or_insert(i);
                stack.push(i);
            }
            Ev::End(TagKind::FootnoteDef) => {
                stack.pop();
            }
            Ev::FnRef(r) => {
                let owner = stack.last().copied().unwrap_or(BODY);
                refs_of
                    .entry(owner)
                    .or_default()
                    .push(UniCase::new(r.label.clone()));
                owner_of_ref.push((i, owner));
            }
            _ => {}
        }
    }
    // Rendered definitions: reachable from the body through first definitions.
    let mut rendered: HashSet<usize> = HashSet::new(); // def start indexes
    let mut queue: Vec<UniCase<String>> = refs_of.get(&BODY).cloned().unwrap_or_default();
    queue.reverse();
    while let Some(label) = queue.pop() {
        let Some(&d) = first_def.get(&label) else {
            continue;
        };
        if rendered.insert(d)
            && let Some(more) = refs_of.get(&d)
        {
            queue.extend(more.iter().rev().cloned());
        }
    }
    // Ordinals in event order over rendered references.
    let mut ord_of: HashMap<UniCase<String>, u32> = HashMap::new();
    let mut refs_per_ord: Vec<u32> = vec![0];
    for (i, owner) in owner_of_ref {
        if owner != BODY && !rendered.contains(&owner) {
            continue;
        }
        if let Ev::FnRef(r) = &mut s[i] {
            let key = UniCase::new(r.label.clone());
            let next = u32::try_from(refs_per_ord.len()).unwrap_or(u32::MAX);
            let ord = *ord_of.entry(key).or_insert_with(|| {
                refs_per_ord.push(0);
                next
            });
            refs_per_ord[ord as usize] += 1;
            r.ord = ord;
            r.k = refs_per_ord[ord as usize];
        }
    }
    for (i, e) in s.iter_mut().enumerate() {
        if let Ev::Start(Tag::FootnoteDef { label, ord }) = e
            && rendered.contains(&i)
        {
            *ord = ord_of.get(&UniCase::new(label.clone())).copied();
        }
    }
    refs_per_ord
}

/// A rendered footnote definition.
pub(crate) struct Footnote {
    pub ord: u32,
    pub refs: u32,
    pub events: Vec<Ev>,
}

/// Split S6 into the body and the rendered footnote definitions (in ordinal order).
/// Unrendered and duplicate definitions are dropped; nested definitions are extracted too.
pub(crate) fn split_footnotes(s: Vec<Ev>, refs_per_ord: &[u32]) -> (Vec<Ev>, Vec<Footnote>) {
    let mut body = Vec::with_capacity(s.len());
    let mut defs: Vec<(Option<u32>, Vec<Ev>)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for e in s {
        match e {
            Ev::Start(Tag::FootnoteDef { ord, .. }) => {
                defs.push((ord, Vec::new()));
                stack.push(defs.len() - 1);
            }
            Ev::End(TagKind::FootnoteDef) => {
                stack.pop();
            }
            e => match stack.last() {
                Some(&d) => defs[d].1.push(e),
                None => body.push(e),
            },
        }
    }
    let mut notes: Vec<Footnote> = defs
        .into_iter()
        .filter_map(|(ord, events)| {
            let ord = ord?;
            Some(Footnote {
                ord,
                refs: refs_per_ord.get(ord as usize).copied().unwrap_or(0),
                events,
            })
        })
        .collect();
    notes.sort_by_key(|n| n.ord);
    (body, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_value() {
        assert_eq!(OPTIONS.bits(), 0x0000_0C1E);
    }

    fn texts(s: &[Ev]) -> Vec<String> {
        s.iter()
            .map(|e| match e {
                Ev::Text(t) => format!("T({t})"),
                Ev::Literal(t) => format!("L({t})"),
                Ev::Start(Tag::Link(d)) => format!("<a {}>", d.dest),
                Ev::End(TagKind::Link) => "</a>".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn autolinks() {
        let mut out = Vec::new();
        autolink(
            "see www.example.com/a(b)c). and (http://x.y/z) &amp; https://a.b?c=1&d;",
            &mut out,
        );
        let t = texts(&out);
        assert_eq!(
            t,
            vec![
                "T(see )",
                "<a https://www.example.com/a(b)c>",
                "T(www.example.com/a(b)c)",
                "</a>",
                "T(). and ()",
                "<a http://x.y/z>",
                "T(http://x.y/z)",
                "</a>",
                "T() &amp; )",
                "<a https://a.b?c=1>",
                "T(https://a.b?c=1)",
                "</a>",
                "T(&d;)",
            ]
        );
    }

    #[test]
    fn autolink_rejects() {
        for s in [
            "www",
            "http://x",
            "xwww.a.com",
            "www.a_b.com",
            "http://a.b_c",
            "www.",
        ] {
            let mut out = Vec::new();
            autolink(s, &mut out);
            assert_eq!(out, vec![Ev::Text(s.into())], "{s}");
        }
    }

    #[test]
    fn spoiler_and_details() {
        let s3 = parse_to_s3("> [!spoiler]\n> hidden\n\n> [!Details] Sum *x*\n> body\n");
        let kinds: Vec<String> = s3.events.iter().map(|e| format!("{e:?}")).collect();
        let joined = kinds.join("|");
        assert!(
            joined.starts_with("Start(Spoiler)|Start(Paragraph)|Text(\"hidden\")"),
            "{joined}"
        );
        assert!(
            joined.contains("Start(Details)|Start(Summary)|Text(\"Sum \")|Start(Emphasis)"),
            "{joined}"
        );
    }
}
