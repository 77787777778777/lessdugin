// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared-vector runner for `testdata/vectors/{lfm1,xss,terminal,mathml}/*.json`
//! (`docs/spec/lfm-1.md` §13, format in `testdata/vectors/README.md`).
//!
//! Every vector's `expected` fields are compared exactly. Every render vector is also checked
//! against the invariants of §13: the allowlist checker, idempotence, the Temml-mode output,
//! the terminal escape invariant, and (feature `ammonia`) `ammonia(html) == html`.
//!
//! `LESSDUGIN_BLESS=1 cargo test -p lessdugin-markdown --test vectors` rewrites the expected
//! values in place (review the diff before committing).
//!
//! The expected `html` values are the math-core output, so this runner needs the default
//! `math` feature; `html_temml` is checked in the same run through `render_with_math_mode`.
//!
//! The same runner also runs on wasm32 under node (`cargo xtask test-wasm`). There is no
//! filesystem there, so the vector files are embedded at compile time from `FILES`, which
//! `wasm_embeds_every_vector_file` keeps equal to the directory listing.
#![cfg(feature = "math")]

use std::path::{Path, PathBuf};

// Natively this is a libtest test; on wasm32 the same body runs under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

use lessdugin_markdown::allowlist::check_html;
use lessdugin_markdown::printer::{PrintOpts, check_printer_output, print_doc};
use lessdugin_markdown::quote::find_first;
use lessdugin_markdown::{
    HrefCtx, MathMode, MediaItem, QuoteTarget, RenderOpts, Rendered, TermCtx, TextQuote,
    derive_metadata, plain_projection, qkey, render_with_math_mode, resolve_quote, safe_href,
    short_did, term_sanitize,
};
use serde_json::{Map, Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/vectors")
}

#[cfg(not(target_arch = "wasm32"))]
const DIRS: &[&str] = &["lfm1", "xss", "terminal", "mathml"];

/// `FILES`: every vector file under `testdata/vectors/{DIRS}`, in run order (`DIRS` order, then by
/// name); on wasm32 also `EMBEDDED`, their text.
macro_rules! vector_files {
    ($($rel:literal),* $(,)?) => {
        const FILES: &[&str] = &[$($rel),*];
        #[cfg(target_arch = "wasm32")]
        const EMBEDDED: &[&str] = &[$(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/vectors/", $rel))
        ),*];
    };
}

vector_files![
    "lfm1/at-links.json",
    "lfm1/autolink.json",
    "lfm1/blocks.json",
    "lfm1/excerpt.json",
    "lfm1/figure.json",
    "lfm1/footnotes.json",
    "lfm1/gallery.json",
    "lfm1/ids.json",
    "lfm1/images.json",
    "lfm1/limits.json",
    "lfm1/links.json",
    "lfm1/math.json",
    "lfm1/mention.json",
    "lfm1/normalise.json",
    "lfm1/options.json",
    "lfm1/projection.json",
    "lfm1/quote.json",
    "lfm1/raw-html.json",
    "lfm1/safe-href.json",
    "lfm1/serialise.json",
    "lfm1/short-did.json",
    "lfm1/spoiler-details.json",
    "lfm1/styled.json",
    "lfm1/term-sanitize.json",
    "xss/html.json",
    "xss/mathml.json",
    "terminal/escapes.json",
    "mathml/math-core-0.9.json",
    "mathml/s10-corpus.json",
    "mathml/s10-probes.json",
];

/// The vector files in run order as `(dir/name, path, text)`, read from disk.
#[cfg(not(target_arch = "wasm32"))]
fn vector_files() -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    for dir in DIRS {
        let d = root().join(dir);
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for path in files {
            let raw = std::fs::read_to_string(&path).expect("read");
            let file = format!("{dir}/{}", path.file_name().unwrap().to_string_lossy());
            out.push((file, path, raw));
        }
    }
    out
}

/// The vector files in run order as `(dir/name, path, text)`, embedded at compile time.
#[cfg(target_arch = "wasm32")]
fn vector_files() -> Vec<(String, PathBuf, String)> {
    FILES
        .iter()
        .zip(EMBEDDED)
        .map(|(f, raw)| ((*f).to_owned(), root().join(f), (*raw).to_owned()))
        .collect()
}

/// The wasm32 run sees exactly the files the native run reads.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn wasm_embeds_every_vector_file() {
    let on_disk: Vec<String> = vector_files().into_iter().map(|(f, ..)| f).collect();
    assert_eq!(
        on_disk, FILES,
        "tests/vectors.rs FILES (embedded for the wasm32 run) must list every vector file"
    );
}

fn input_text(input: &Value) -> String {
    if let Some(t) = input.get("text").and_then(Value::as_str) {
        return t.to_owned();
    }
    let g = input.get("gen").expect("input needs text or gen");
    let s = |k: &str| g.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let times = g.get("times").and_then(Value::as_u64).unwrap_or(1) as usize;
    let mut out = s("prefix");
    out.push_str(&s("repeat").repeat(times));
    out.push_str(&s("suffix"));
    out
}

fn input_media(input: &Value) -> Vec<MediaItem> {
    match input.get("media") {
        Some(m) => serde_json::from_value(m.clone()).expect("media"),
        None => Vec::new(),
    }
}

fn render_input(input: &Value, mode: MathMode) -> Rendered {
    let text = input_text(input);
    let media = input_media(input);
    let profile = input.get("profile").and_then(Value::as_str);
    if input.get("encoding_error").and_then(Value::as_bool) == Some(true) {
        return lessdugin_markdown::render_error(lessdugin_markdown::BodyError::Encoding, profile);
    }
    render_with_math_mode(&text, &media, profile, &RenderOpts::ALL, mode)
}

fn print_opts(v: &Value) -> PrintOpts {
    let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    PrintOpts {
        color: b("color"),
        hyperlinks: b("hyperlinks"),
        reveal_spoilers: b("reveal_spoilers"),
        width: v.get("width").and_then(Value::as_u64).map(|w| w as usize),
    }
}

fn term_ctx(input: &Value) -> TermCtx {
    match input["ctx"].as_str().expect("ctx") {
        "prose" => TermCtx::Prose,
        "code" => TermCtx::Code,
        "name" => TermCtx::Name {
            max_graphemes: input.get("max").and_then(Value::as_u64).unwrap_or(0) as usize,
        },
        other => panic!("unknown ctx {other}"),
    }
}

/// Compute the actual value of every key in `expected` for one vector.
fn actual(
    kind: &str,
    input: &Value,
    keys: &[String],
    renders: Option<&(Rendered, Rendered)>,
) -> Map<String, Value> {
    let mut out = Map::new();
    match kind {
        "render" | "terminal" => {
            let (r, t) = renders.expect("render vectors are pre-rendered");
            let rv = serde_json::to_value(r).expect("json");
            for k in keys {
                let v = match k.as_str() {
                    "html_temml" => json!(t.html),
                    "text_x" => json!(plain_projection(&input_text(input)).text_x),
                    "html_head" => json!(r.html.chars().take(400).collect::<String>()),
                    "html_rle" => json!(rle_lines(&r.html)),
                    "html_tail" => {
                        let n = r.html.chars().count();
                        json!(
                            r.html
                                .chars()
                                .skip(n.saturating_sub(400))
                                .collect::<String>()
                        )
                    }
                    "html_bytes" => json!(r.html.len()),
                    "printed" | "printed_bytes" | "printed_rle" => {
                        let opts = print_opts(input.get("print").unwrap_or(&Value::Null));
                        let p = print_doc(&r.styled, &opts);
                        match k.as_str() {
                            "printed" => json!(p),
                            "printed_bytes" => json!(p.len()),
                            _ => json!(rle_lines(&p)),
                        }
                    }
                    k => rv
                        .get(k)
                        .cloned()
                        .unwrap_or_else(|| panic!("unknown render field {k}")),
                };
                out.insert(k.clone(), v);
            }
        }
        "safe_href" => {
            let ctx = match input["ctx"].as_str().unwrap_or("markdown") {
                "field" => HrefCtx::Field,
                _ => HrefCtx::Markdown,
            };
            let raw = input_text_field(input, "raw");
            out.insert(
                "href".into(),
                serde_json::to_value(safe_href(&raw, ctx)).expect("json"),
            );
        }
        "quote" => {
            let q = &input["quote"];
            let target = input
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let target_owned = match input.get("target_gen") {
                Some(_) => Some(input_text(&json!({"gen": input["target_gen"]}))),
                None => target,
            };
            for k in keys {
                let v = match k.as_str() {
                    "result" => {
                        let exact = input_text_field(q, "exact");
                        let tq = TextQuote {
                            exact: &exact,
                            prefix: q.get("prefix").and_then(Value::as_str),
                            suffix: q.get("suffix").and_then(Value::as_str),
                            start: q.get("start").and_then(Value::as_u64),
                        };
                        let t = match &target_owned {
                            Some(t) => QuoteTarget::Text(t),
                            None => QuoteTarget::Pending,
                        };
                        serde_json::to_value(resolve_quote(&tq, t)).expect("json")
                    }
                    "qkey" => json!(qkey(q.get("exact").and_then(Value::as_str))),
                    "first" => {
                        let t = target_owned.clone().unwrap_or_default();
                        json!(find_first(q["exact"].as_str().unwrap_or(""), &t))
                    }
                    other => panic!("unknown quote field {other}"),
                };
                out.insert(k.clone(), v);
            }
        }
        "term_sanitize" => {
            let t = term_sanitize(&input_text_field(input, "text"), term_ctx(input));
            out.insert("out".into(), serde_json::to_value(&t).expect("json"));
        }
        "short_did" => {
            out.insert(
                "out".into(),
                json!(short_did(input["did"].as_str().expect("did"))),
            );
        }
        "mathml" => {
            let tex = input["tex"].as_str().expect("tex");
            let display = input
                .get("display")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let src = if display {
                format!("$${tex}$$")
            } else {
                format!("${tex}$")
            };
            let r = render_with_math_mode(&src, &[], None, &RenderOpts::ALL, MathMode::MathCore);
            for k in keys {
                let v = match k.as_str() {
                    "html" => json!(r.html),
                    "math_errors" => json!(r.flags.math_errors),
                    other => panic!("unknown mathml field {other}"),
                };
                out.insert(k.clone(), v);
            }
        }
        other => panic!("unknown kind {other}"),
    }
    out
}

/// The HTML as lines, with each run of identical consecutive lines written once as
/// `"<line> ×N"` (keeps deep-nesting vectors readable).
fn rle_lines(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<&str> = None;
    let mut n = 0usize;
    let flush = |out: &mut Vec<String>, line: Option<&str>, n: usize| {
        if let Some(l) = line {
            out.push(if n > 1 {
                format!("{l} \u{d7}{n}")
            } else {
                l.to_owned()
            });
        }
    };
    for line in html.split('\n') {
        if prev == Some(line) {
            n += 1;
        } else {
            flush(&mut out, prev, n);
            prev = Some(line);
            n = 1;
        }
    }
    flush(&mut out, prev, n);
    out
}

/// A string field that may be given as `<name>` or as a generator `<name>_gen`.
fn input_text_field(v: &Value, name: &str) -> String {
    if let Some(s) = v.get(name).and_then(Value::as_str) {
        return s.to_owned();
    }
    match v.get(format!("{name}_gen")) {
        Some(g) => input_text(&json!({ "gen": g })),
        None => panic!("missing {name}"),
    }
}

fn styled_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| styled_texts(x, out)),
        Value::Object(o) => o.values().for_each(|x| styled_texts(x, out)),
        _ => {}
    }
}

fn forbidden_terminal_char(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}' | '\u{2028}' | '\u{2029}' | '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The invariants over one `mathml` vector (`docs/spec/lfm-1.md` §7.3, ADR 0002): the
/// allowlist checker accepts the document, the normaliser met no `style` declaration or class
/// token it does not know (a math-core upgrade that emits new ones shows up here), and with the
/// `ammonia` feature `ammonia(html) == html`.
fn mathml_invariants(name: &str, input: &Value, html: &str) -> Vec<String> {
    let mut errs = Vec::new();
    if let Err(e) = check_html(html) {
        errs.push(format!("{name}: allowlist: {e}"));
    }
    let tex = input["tex"].as_str().expect("tex");
    let display = input
        .get("display")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let unknown = lessdugin_markdown::math::unknown_engine_styles(tex, display);
    if !unknown.is_empty() {
        errs.push(format!(
            "{name}: unknown math-core styles dropped: {unknown:?}"
        ));
    }
    #[cfg(feature = "ammonia")]
    {
        let a = lessdugin_markdown::allowlist::ammonia_clean(html);
        if a != html {
            errs.push(format!(
                "{name}: ammonia changed the output:\n  ours:    {html:?}\n  ammonia: {a:?}"
            ));
        }
    }
    errs
}

/// The invariants of §13 over one render input.
fn invariants(name: &str, input: &Value, r: &Rendered, t: &Rendered) -> Vec<String> {
    let mut errs = Vec::new();
    if let Err(e) = check_html(&r.html) {
        errs.push(format!("{name}: allowlist: {e}"));
    }
    if let Err(e) = check_html(&t.html) {
        errs.push(format!("{name}: allowlist (temml): {e}"));
    }
    if &render_input(input, MathMode::MathCore) != r {
        errs.push(format!("{name}: render is not deterministic"));
    }
    // `derive_metadata` (no HTML writer, no math engine) agrees with `render`.
    if input.get("encoding_error").is_none() {
        let m = derive_metadata(&input_text(input), &input_media(input));
        let same = m.text == r.text
            && m.excerpt == r.excerpt
            && m.toc == r.toc
            && m.word_count == r.word_count
            && m.read_minutes == r.read_minutes
            && m.links == r.links
            && m.mentions == r.mentions
            && m.media_used == r.media_used
            && m.error == r.error;
        if !same {
            errs.push(format!("{name}: derive_metadata differs from render"));
        }
    }
    for (label, s) in [("text", &r.text), ("excerpt", &r.excerpt)] {
        if s.chars().any(|c| c.is_whitespace() && c != ' ') {
            errs.push(format!("{name}: {label} contains non-space whitespace"));
        }
        if s.chars().any(forbidden_terminal_char) {
            errs.push(format!("{name}: {label} contains a control character"));
        }
    }
    // StyledDoc text never contains ESC, C0, C1, U+2028/9 or bidi controls.
    let mut texts = Vec::new();
    let sv = serde_json::to_value(&r.styled).expect("json");
    styled_texts(&sv, &mut texts);
    for s in &texts {
        if s.chars().any(forbidden_terminal_char) {
            errs.push(format!(
                "{name}: StyledDoc text {s:?} contains a forbidden character"
            ));
        }
    }
    for opts in [
        PrintOpts {
            color: true,
            hyperlinks: true,
            reveal_spoilers: true,
            width: Some(80),
        },
        PrintOpts {
            color: false,
            hyperlinks: false,
            reveal_spoilers: false,
            width: None,
        },
    ] {
        let p = print_doc(&r.styled, &opts);
        if let Err(e) = check_printer_output(&p) {
            errs.push(format!("{name}: printer: {e}"));
        }
        if !opts.color && p.contains('\u{1b}') {
            errs.push(format!("{name}: --color never output contains ESC"));
        }
    }
    #[cfg(feature = "ammonia")]
    {
        let a = lessdugin_markdown::allowlist::ammonia_clean(&r.html);
        if a != r.html {
            errs.push(format!(
                "{name}: ammonia changed the output:\n  ours:    {:?}\n  ammonia: {a:?}",
                r.html
            ));
        }
        let a = lessdugin_markdown::allowlist::ammonia_clean(&t.html);
        if a != t.html {
            errs.push(format!(
                "{name}: ammonia changed the temml output:\n  ours:    {:?}\n  ammonia: {a:?}",
                t.html
            ));
        }
    }
    errs
}

/// Escape invisible and confusable code points inside the pretty JSON (they can only occur
/// inside strings) so that reviewers see `\u202e` rather than a live bidi override.
fn escape_invisibles(json: &str) -> String {
    use unicode_general_category::{GeneralCategory as G, get_general_category};
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        let esc = match get_general_category(c) {
            G::Control
            | G::Format
            | G::PrivateUse
            | G::Unassigned
            | G::Surrogate
            | G::LineSeparator
            | G::ParagraphSeparator
            | G::NonspacingMark
            | G::EnclosingMark
            | G::SpacingMark => c != '\n',
            G::SpaceSeparator => c != ' ',
            _ => false,
        };
        if esc {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn vectors() {
    let bless_all = std::env::var("LESSDUGIN_BLESS").is_ok_and(|v| v == "1");
    // `LESSDUGIN_BLESS_ONLY=lfm1/a.json,xss/b.json` limits blessing to those files; the
    // others are checked as usual.
    let bless_only: Option<Vec<String>> = std::env::var("LESSDUGIN_BLESS_ONLY")
        .ok()
        .map(|v| v.split(',').map(|f| f.trim().to_owned()).collect());
    let mut failures: Vec<String> = Vec::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (file, path, raw) in vector_files() {
        let bless = bless_all && bless_only.as_ref().is_none_or(|l| l.contains(&file));
        let mut doc: Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(doc["behaviour"], "lfm/1", "{}", path.display());
        let kind = doc["kind"].as_str().expect("kind").to_owned();
        let vectors = doc["vectors"].as_array_mut().expect("vectors");
        counts.push((file.clone(), vectors.len()));
        let mut changed = false;
        let mut names = std::collections::HashSet::new();
        for v in vectors.iter_mut() {
            let name = format!("{file}#{}", v["name"].as_str().expect("name"));
            assert!(names.insert(name.clone()), "duplicate vector name {name}");
            let input = v["input"].clone();
            let keys: Vec<String> = v["expected"]
                .as_object()
                .expect("expected")
                .keys()
                .cloned()
                .collect();
            let renders = matches!(kind.as_str(), "render" | "terminal").then(|| {
                (
                    render_input(&input, MathMode::MathCore),
                    render_input(&input, MathMode::Temml),
                )
            });
            let got = actual(&kind, &input, &keys, renders.as_ref());
            let exp = v["expected"].as_object_mut().expect("expected");
            for k in &keys {
                if exp[k] != got[k] {
                    if bless {
                        exp.insert(k.clone(), got[k].clone());
                        changed = true;
                    } else {
                        failures.push(format!(
                            "{name}: field {k}\n  expected: {}\n  actual:   {}",
                            exp[k], got[k]
                        ));
                    }
                }
            }
            if kind == "mathml" {
                let html = got.get("html").and_then(Value::as_str).unwrap_or_default();
                failures.extend(mathml_invariants(&name, &input, html));
            }
            if let Some((r, t)) = &renders {
                failures.extend(invariants(&name, &input, r, t));
            }
        }
        let _ = changed;
        if bless {
            let mut s = escape_invisibles(&serde_json::to_string_pretty(&doc).expect("json"));
            s.push('\n');
            std::fs::write(&path, s).expect("write");
        }
    }
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    let mut report = format!("{total} vectors:");
    for (f, n) in &counts {
        report.push_str(&format!("\n  {f}: {n}"));
    }
    // wasm32-unknown-unknown has no stderr; the runner shows console output (`--nocapture`)
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{report}");
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test::console_log!("{report}");
    assert!(
        failures.is_empty(),
        "{} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    let lfm1: usize = counts
        .iter()
        .filter(|(f, _)| f.starts_with("lfm1/"))
        .map(|(_, n)| n)
        .sum();
    assert!(lfm1 >= 150, "lfm1 needs at least 150 vectors (has {lfm1})");
}
