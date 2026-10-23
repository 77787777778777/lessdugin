// SPDX-License-Identifier: AGPL-3.0-or-later
//! The LFM/1 renderer (`lessdugin_markdown`, `docs/spec/lfm-1.md`) on raw body bytes (a fetched
//! `textBlob`): HTML+MathML (both math modes), the terminal `StyledDoc` and its printer, `TermText`
//! sanitising, the plain-text projection P, the excerpt, derived metadata, URL policy and quote
//! anchors. Body media are a fixed pair plus one item for each `cid:` reference in the input, so
//! media references resolve.
//!
//! Contract (lfm-1.md §1.5, §8, §9, §10, §11): never panics or overflows a 1 MiB stack (wasm32's
//! default), and
//! - the HTML passes the allowlist checker, and ammonia leaves it unchanged (§8.5);
//! - P and the excerpt hold no control, bidi control or non-space whitespace (§10.2), and the
//!   excerpt is within 500 grapheme clusters and 5,000 bytes (§10.4);
//! - no `StyledDoc` or `TermText` string carries a terminal control (§9.1), and printed output
//!   passes the printer checker (§9.4), with no escape at all without color;
//! - `derive_metadata` agrees with `render` (§1.6);
//! - a quote built from a selection of P resolves to that selection (§11).

#![no_main]

use std::sync::LazyLock;

use lessdugin_fuzz::{fnv, is_terminal_forbidden, on_small_stack};
use lessdugin_markdown::allowlist::check_html;
use lessdugin_markdown::printer::{
    PrintOpts, check_printer_output, osc8_uri, print_doc, print_doc_with,
};
use lessdugin_markdown::quote::{MAX_OCCURRENCES, find_first};
use lessdugin_markdown::{
    BodyError, HrefCtx, MathMode, MediaItem, QuoteResolution, QuoteTarget, RenderOpts, Rendered,
    StyledDoc, TermCtx, TextQuote, build_quote, decode_text_blob, derive_metadata,
    plain_projection, qkey, render, render_error, render_with_math_mode, resolve_quote, safe_href,
    short_did, term_sanitize, term_shows_host,
};
use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

const CID: &str = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku";
const MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "video/mp4",
    "application/pdf",
    "text/plain",
];

static MEDIA: LazyLock<Vec<MediaItem>> = LazyLock::new(|| {
    serde_json::from_value(json!([
        {"blob": {"ref": {"$link": CID}, "mimeType": "image/png"}, "alt": "m\u{202e}\r\n\u{0}", "aspectRatio": {"width": 3, "height": 2}},
        {"blob": {"ref": {"$link": "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy"}, "mimeType": "video/mp4"}, "name": "v\u{1b}.mp4"}
    ]))
    .expect("media")
});

/// The fixed media plus one item per `cid:` reference (at most 8).
fn media_for(src: &str, h: u64) -> Vec<MediaItem> {
    let mut media = MEDIA.clone();
    for (n, (i, _)) in src.match_indices("cid:").take(8).enumerate() {
        let c: String = src[i + 4..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .take(100)
            .collect();
        let k = (h as usize).wrapping_add(n);
        let item = json!({
            "blob": {"$type": "blob", "ref": {"$link": c}, "mimeType": MIMES[k % MIMES.len()], "size": 1},
            "alt": if k % 3 == 0 { "" } else { "alt \u{1b}[2J\u{202e}" },
            "name": "n\u{7}.bin",
            "aspectRatio": {"width": k % 5, "height": 1 + k % 3},
        });
        if let Ok(m) = serde_json::from_value(item) {
            media.push(m);
        }
    }
    media
}

/// P and the excerpt: no White_Space but U+0020, no control, no bidi control (§10.2).
fn p_char_ok(c: char) -> bool {
    !(c.is_whitespace() && c != ' ')
        && !c.is_control()
        && !matches!(c, '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn check_styled(d: &StyledDoc) {
    let v = serde_json::to_value(d).expect("StyledDoc serialises");
    let mut stack = vec![&v];
    while let Some(x) = stack.pop() {
        match x {
            Value::String(s) => assert!(
                !s.chars().any(is_terminal_forbidden),
                "terminal control in the StyledDoc: {s:?}"
            ),
            Value::Array(a) => stack.extend(a.iter()),
            Value::Object(o) => stack.extend(o.values()),
            _ => {}
        }
    }
    assert!(
        d.blocks.iter().all(|b| b.gutters.len() <= 64),
        "more than 64 gutters"
    );
}

fn check_rendered(r: &Rendered) {
    if let Err(e) = check_html(&r.html) {
        panic!("allowlist: {e}\nhtml={}", r.html);
    }
    #[cfg(feature = "ammonia")]
    assert_eq!(
        lessdugin_markdown::allowlist::ammonia_clean(&r.html),
        r.html,
        "ammonia changed the HTML"
    );
    assert!(r.text.chars().all(p_char_ok), "P: {:?}", r.text);
    assert!(
        !r.text.starts_with(' ') && !r.text.ends_with(' '),
        "P is not trimmed"
    );
    assert!(r.excerpt.chars().all(p_char_ok), "excerpt: {:?}", r.excerpt);
    assert!(
        r.excerpt.graphemes(true).count() <= 500 && r.excerpt.len() <= 5000,
        "excerpt over 500 grapheme clusters or 5,000 bytes"
    );
    check_styled(&r.styled);
}

fn check_printed(r: &Rendered, h: u64) {
    let color = PrintOpts {
        color: true,
        hyperlinks: true,
        reveal_spoilers: true,
        width: Some(1 + (h % 120) as usize),
    };
    let plain = PrintOpts::default();
    let narrow = PrintOpts {
        color: true,
        hyperlinks: false,
        reveal_spoilers: false,
        width: Some(1 + (h % 7) as usize),
    };
    for opts in [color, plain, narrow] {
        let p = print_doc(&r.styled, &opts);
        if let Err(e) = check_printer_output(&p) {
            panic!("printer ({opts:?}): {e}");
        }
        assert!(
            opts.color || !p.contains('\u{1b}'),
            "printer without color wrote an escape"
        );
    }
    let handle = |_: &str| Some("alice.example.com".to_owned());
    if let Err(e) = check_printer_output(&print_doc_with(&r.styled, &color, &handle)) {
        panic!("printer with handles: {e}");
    }
}

fn check_metadata(src: &str, media: &[MediaItem], r: &Rendered) {
    let d = derive_metadata(src, media);
    assert_eq!(d.text, r.text, "metadata P");
    assert_eq!(d.text_x, plain_projection(src).text_x, "metadata P_x");
    assert_eq!(d.excerpt, r.excerpt, "metadata excerpt");
    assert_eq!(d.toc, r.toc, "metadata ToC");
    assert_eq!(
        (d.word_count, d.read_minutes),
        (r.word_count, r.read_minutes)
    );
    assert_eq!(d.links, r.links, "metadata links");
    assert_eq!(d.mentions, r.mentions, "metadata mentions");
    assert_eq!(d.media_used, r.media_used, "metadata media_used");
}

/// The char boundary at or below `i`.
fn floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn check_quotes(src: &str, p: &str, h: u64) {
    // A quote built from a selection of P resolves to it (§11; writers build, readers resolve).
    // With fewer than MAX_OCCURRENCES bytes in P, every occurrence is scored.
    let n = p.chars().count() as u64;
    if n > 0 {
        let a = h % (n + 1);
        let b = (h >> 32) % (n + 1);
        if let Ok(q) = build_quote(p, a.min(b) as usize, a.max(b) as usize) {
            let start = q.start.expect("built quotes carry a start");
            let res = resolve_quote(&q.as_quote(), QuoteTarget::Text(p));
            if p.len() < MAX_OCCURRENCES {
                assert_eq!(qkey(Some(&q.exact)), q.exact, "qkey of a quote of P");
                assert_eq!(
                    res,
                    QuoteResolution::Anchored {
                        start,
                        end: start + q.exact.chars().count() as u64
                    },
                    "quote {q:?}"
                );
            }
            let _ = find_first(&q.exact, p);
        }
    }
    // Quotes made of arbitrary pieces of the source, against P and against the source itself.
    let i = floor(src, (h % (src.len() as u64 + 1)) as usize);
    let j = floor(src, i + ((h >> 20) % 64) as usize);
    let quote = TextQuote {
        exact: &src[i..j],
        prefix: Some(&src[floor(src, i.saturating_sub(40))..i]),
        suffix: Some(&src[j..floor(src, j + 40)]),
        start: Some(h >> 40),
    };
    for t in [
        QuoteTarget::Text(p),
        QuoteTarget::Text(src),
        QuoteTarget::Pending,
    ] {
        let _ = resolve_quote(&quote, t);
    }
    let _ = qkey(Some(src));
    let _ = find_first(&src[i..j], src);
}

fn check_term(src: &str) {
    for ctx in [
        TermCtx::Prose,
        TermCtx::Code,
        TermCtx::Name { max_graphemes: 0 },
        TermCtx::Name { max_graphemes: 5 },
    ] {
        let t = term_sanitize(src, ctx);
        for run in t.runs() {
            assert!(
                !run.text().chars().any(is_terminal_forbidden),
                "term_sanitize({ctx:?}) let a control through: {:?}",
                run.text()
            );
        }
        let _ = t.width();
    }
    let line = &src[..floor(src, src.find('\n').unwrap_or(src.len()).min(4096))];
    for ctx in [HrefCtx::Markdown, HrefCtx::Field] {
        if let Some(href) = safe_href(line, ctx) {
            let _ = href.is_openable_externally();
        }
    }
    let _ = short_did(line);
    let _ = term_shows_host(line, "https://example.com/x");
    let _ = osc8_uri(line);
}

fn check(data: &[u8]) {
    let src = match decode_text_blob(data) {
        Ok(s) => s,
        Err(e) => {
            assert_eq!(e, BodyError::Encoding);
            check_rendered(&render_error(e, None));
            return;
        }
    };
    let h = fnv(data);
    let media = media_for(&src, h);
    let r = render(&src, &media, &RenderOpts::ALL);
    check_rendered(&r);
    check_printed(&r, h);
    check_metadata(&src, &media, &r);
    let t = render_with_math_mode(&src, &media, None, &RenderOpts::ALL, MathMode::Temml);
    if let Err(e) = check_html(&t.html) {
        panic!("allowlist (Temml mode): {e}\nhtml={}", t.html);
    }
    check_quotes(&src, &r.text, h);
    check_term(&src);
}

fuzz_target!(|data: &[u8]| {
    on_small_stack(|| check(data));
});
