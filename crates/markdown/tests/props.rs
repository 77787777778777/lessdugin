// SPDX-License-Identifier: AGPL-3.0-or-later
//! Property tests (`docs/spec/lfm-1.md` §1.5, §8, §9, §13): rendering never panics, the HTML
//! always passes the allowlist checker (and ammonia, with the feature), P has no line-breaking
//! or tab whitespace, terminal text never carries escapes, and deep nesting is cheap and
//! stack-safe.

use lessdugin_markdown::allowlist::check_html;
use lessdugin_markdown::printer::{PrintOpts, check_printer_output, print_doc};
use lessdugin_markdown::{
    MathMode, MediaItem, RenderOpts, Rendered, SKind, StyledDoc, derive_metadata, plain_projection,
    render, render_with_math_mode,
};
use proptest::prelude::*;
// Natively these are libtest tests; on wasm32 (`cargo xtask test-wasm`) the same bodies run
// under wasm-bindgen-test in node.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// A wall clock for the timing tests. wasm32-unknown-unknown has no `Instant`, so there it reads
/// node's `Date.now()`.
struct Stopwatch(
    #[cfg(not(target_arch = "wasm32"))] std::time::Instant,
    #[cfg(target_arch = "wasm32")] f64,
);

impl Stopwatch {
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::disallowed_methods)] // timing a test is fine; the crate itself never reads clocks
    fn start() -> Self {
        Self(std::time::Instant::now())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn elapsed(&self) -> std::time::Duration {
        self.0.elapsed()
    }

    #[cfg(target_arch = "wasm32")]
    fn start() -> Self {
        Self(js_sys::Date::now())
    }

    #[cfg(target_arch = "wasm32")]
    fn elapsed(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64((js_sys::Date::now() - self.0).max(0.0) / 1000.0)
    }
}

const CID: &str = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku";

fn media() -> Vec<MediaItem> {
    serde_json::from_value(serde_json::json!([
        {"blob": {"ref": {"$link": CID}, "mimeType": "image/png"}, "alt": "m\u{202e}\r\n\u{0}", "aspectRatio": {"width": 3, "height": 2}},
        {"blob": {"ref": {"$link": "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy"}, "mimeType": "video/mp4"}, "name": "v\u{1b}.mp4"}
    ]))
    .expect("media")
}

/// Markdown-heavy tokens so random inputs hit real constructs.
fn token() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("> ".to_owned()),
        Just("- ".to_owned()),
        Just("1. ".to_owned()),
        Just("- [ ] ".to_owned()),
        Just("# ".to_owned()),
        Just("\n".to_owned()),
        Just("\n\n".to_owned()),
        Just("\r\n".to_owned()),
        Just("\r".to_owned()),
        Just("    ".to_owned()),
        Just("```".to_owned()),
        Just("`".to_owned()),
        Just("*".to_owned()),
        Just("_".to_owned()),
        Just("~".to_owned()),
        Just("[".to_owned()),
        Just("]".to_owned()),
        Just("](".to_owned()),
        Just(")".to_owned()),
        Just("![".to_owned()),
        Just("[^1]".to_owned()),
        Just("[^1]: ".to_owned()),
        Just("$".to_owned()),
        Just("$$".to_owned()),
        Just("\\frac{a}{b}".to_owned()),
        Just("\\sqrt".to_owned()),
        Just("\\href".to_owned()),
        Just("{".to_owned()),
        Just("}".to_owned()),
        Just("|".to_owned()),
        Just("|---|".to_owned()),
        Just("<b>".to_owned()),
        Just("<script>".to_owned()),
        Just("<!--".to_owned()),
        Just("&amp;".to_owned()),
        Just("\\".to_owned()),
        Just("http://".to_owned()),
        Just("https://e.com/".to_owned()),
        Just("www.a.com".to_owned()),
        Just("javascript:".to_owned()),
        Just("mailto:a@b.c".to_owned()),
        Just(format!("cid:{CID}")),
        Just("cid:bx".to_owned()),
        Just("at://did:plc:abcdefghijklmnopqrstuvwx".to_owned()),
        Just("/online.7778777.lessdugin.post/3k".to_owned()),
        Just("@".to_owned()),
        Just("#x".to_owned()),
        Just("\"t\"".to_owned()),
        Just("[!spoiler]".to_owned()),
        Just("[!details] ".to_owned()),
        Just("[!NOTE]".to_owned()),
        Just("\u{1b}[31m".to_owned()),
        Just("\u{202e}".to_owned()),
        Just("\u{0}".to_owned()),
        Just("\u{85}".to_owned()),
        Just("\u{a0}".to_owned()),
        Just("é".to_owned()),
        Just("😀".to_owned()),
        "[a-z ]{1,6}",
        any::<char>().prop_map(|c| c.to_string()),
    ]
}

fn doc() -> impl Strategy<Value = String> {
    prop::collection::vec(token(), 0..60).prop_map(|v| v.concat())
}

fn styled_ok(d: &StyledDoc) -> Result<(), String> {
    let v = serde_json::to_value(d).map_err(|e| e.to_string())?;
    let mut stack = vec![&v];
    while let Some(x) = stack.pop() {
        match x {
            serde_json::Value::String(s) => {
                if s.chars().any(|c| {
                    matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}' | '\u{2028}' | '\u{2029}' | '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
                }) {
                    return Err(format!("forbidden char in {s:?}"));
                }
            }
            serde_json::Value::Array(a) => stack.extend(a.iter()),
            serde_json::Value::Object(o) => stack.extend(o.values()),
            _ => {}
        }
    }
    if d.blocks.iter().any(|b| b.gutters.len() > 64) {
        return Err("more than 64 gutters".into());
    }
    Ok(())
}

/// P (and so the excerpt) holds no whitespace other than U+0020, no C0/C1 control or DEL and
/// no bidi control (§10.2).
fn p_char_ok(c: char) -> bool {
    !(c.is_whitespace() && c != ' ')
        && !c.is_control()
        && !matches!(c, '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn check_all(src: &str, r: &Rendered) -> Result<(), TestCaseError> {
    check_html(&r.html)
        .map_err(|e| TestCaseError::fail(format!("{e}\nsrc={src:?}\nhtml={}", r.html)))?;
    prop_assert!(
        r.text.chars().all(p_char_ok),
        "P has a forbidden character: {:?}",
        r.text
    );
    prop_assert!(
        r.excerpt.chars().all(p_char_ok),
        "the excerpt has a forbidden character: {:?}",
        r.excerpt
    );
    prop_assert!(!r.text.starts_with(' ') && !r.text.ends_with(' '));
    prop_assert!(r.excerpt.chars().count() <= 500 && r.excerpt.len() <= 5000);
    styled_ok(&r.styled).map_err(TestCaseError::fail)?;
    for opts in [
        PrintOpts {
            color: true,
            hyperlinks: true,
            reveal_spoilers: true,
            width: Some(40),
        },
        PrintOpts::default(),
    ] {
        check_printer_output(&print_doc(&r.styled, &opts)).map_err(TestCaseError::fail)?;
    }
    #[cfg(feature = "ammonia")]
    prop_assert_eq!(
        lessdugin_markdown::allowlist::ammonia_clean(&r.html),
        r.html.clone()
    );
    Ok(())
}

proptest! {
    // 256 cases by default; set PROPTEST_CASES for longer runs.

    #[test]
    fn structured_inputs(src in doc()) {
        let m = media();
        let r = render(&src, &m, &RenderOpts::ALL);
        check_all(&src, &r)?;
        let t = render_with_math_mode(&src, &m, None, &RenderOpts::ALL, MathMode::Temml);
        check_html(&t.html).map_err(TestCaseError::fail)?;
        prop_assert_eq!(&render(&src, &m, &RenderOpts::ALL), &r, "not deterministic");
    }

    #[test]
    fn arbitrary_strings(src in any::<String>()) {
        let r = render(&src, &[], &RenderOpts::ALL);
        check_all(&src, &r)?;
    }

    #[test]
    fn math_never_breaks_the_allowlist(tex in "[a-z\\\\{}^_ &()\\[\\]<>0-9+=-]{0,80}") {
        for src in [format!("${tex}$"), format!("$${tex}$$")] {
            let r = render(&src, &[], &RenderOpts::ALL);
            check_html(&r.html).map_err(TestCaseError::fail)?;
        }
    }

    #[test]
    fn opts_select_outputs_only(src in doc()) {
        let all = render(&src, &[], &RenderOpts::ALL);
        let none = render(&src, &[], &RenderOpts { html: false, styled: false, text: false });
        prop_assert_eq!(&none.excerpt, &all.excerpt);
        prop_assert_eq!(&none.toc, &all.toc);
        prop_assert_eq!(&none.anchors, &all.anchors);
        prop_assert_eq!(none.flags, all.flags);
        prop_assert!(none.html.is_empty() && none.text.is_empty() && none.styled.blocks.is_empty());
    }

    /// `derive_metadata` (no writers, no math engine) agrees with `render`.
    #[test]
    fn metadata_matches_render(src in doc()) {
        let m = media();
        let r = render(&src, &m, &RenderOpts::ALL);
        let d = derive_metadata(&src, &m);
        prop_assert_eq!(&d.text, &r.text);
        prop_assert_eq!(&d.text_x, &plain_projection(&src).text_x);
        prop_assert_eq!(&d.excerpt, &r.excerpt);
        prop_assert_eq!(&d.toc, &r.toc);
        prop_assert_eq!(d.word_count, r.word_count);
        prop_assert_eq!(d.read_minutes, r.read_minutes);
        prop_assert_eq!(&d.links, &r.links);
        prop_assert_eq!(&d.mentions, &r.mentions);
        prop_assert_eq!(&d.media_used, &r.media_used);
    }
}

// Hosts the WHATWG URL parser rejects or rewrites are never linked (§5.1 `host_ok`), so ammonia,
// which parses every `href` with the `url` crate, never strips one. Found by the `render` fuzz
// target (`fuzz/regressions/render/invalid-punycode-host`); the strategy aims at the edges: IPv4
// forms, IPv6 literals and Punycode labels.
#[cfg(feature = "ammonia")]
proptest! {
    #[test]
    fn linked_hosts_survive_ammonia(
        host in "(xn--)?[a-z0-9_-]{0,10}(\\.(xn--)?[a-z0-9x-]{0,6}){0,3}|[0-9x.]{1,16}|\\[[0-9a-f:.]{0,24}\\]"
    ) {
        let src = format!("[a](https://{host}/x)");
        let r = render(&src, &[], &RenderOpts::ALL);
        prop_assert_eq!(
            lessdugin_markdown::allowlist::ammonia_clean(&r.html),
            r.html.clone(),
            "{}",
            src
        );
    }
}

/// `links`/`mentions` (§12) come from the rendered content of S6: nothing that is never shown
/// (an unreferenced footnote definition, a link inside an image's alt) notifies anyone. They
/// are `lfm/1` values, so the limit stage (presentation) does not change them: a flattened
/// link still shows its text.
#[test]
fn links_and_mentions_of_rendered_content() {
    let did = "did:plc:abcdefghijklmnopqrstuvwx";
    let uri = format!("at://{did}/online.7778777.lessdugin.post/3k2a");
    let hidden = [
        format!("x\n\n[^x]: [@v](at://{did}) [r]({uri})"),
        format!("x[^x]\n\n[^x]: a\n\n[^x]: [@v](at://{did}) [r]({uri})"),
        format!("![[@v](at://{did}) [r]({uri})](https://x.example/i.png)"),
    ];
    for src in &hidden {
        let r = render(src, &[], &RenderOpts::ALL);
        assert!(
            r.mentions.is_empty() && r.links.is_empty(),
            "{src:?}: {r:?}"
        );
        assert!(!r.html.contains("data-did") && !r.html.contains("data-at-uri"));
    }
    let shown = [
        format!("x[^x]\n\n[^x]: [@v](at://{did}) [r]({uri})"),
        format!("{}[@v](at://{did}) [r]({uri})", "> ".repeat(70)),
        format!("{}[@v](at://{did}) [r]({uri})", "- ".repeat(70)),
        format!("{}*[@v](at://{did}) [r]({uri})*", "> ".repeat(63)),
    ];
    for src in &shown {
        let r = render(src, &[], &RenderOpts::ALL);
        assert_eq!((r.mentions.len(), r.links.len()), (1, 1), "{src:?}");
        assert!(r.text.contains("@v r"), "{src:?}");
    }
}

/// Output stays proportionate to the input (red-team amplification inputs): a `media.alt`
/// fallback is shown once per item, the HTML has a global budget, and the printer neither pads
/// every row to one huge cell nor repeats OSC 8 or SGR sequences per word or grapheme.
#[test]
fn output_is_bounded() {
    let cid = format!("bafkrei{}", "a".repeat(52));
    let media: Vec<MediaItem> = serde_json::from_value(serde_json::json!([
        {"blob": {"ref": {"$link": cid}, "mimeType": "image/png"}, "alt": "\"".repeat(20_000)}
    ]))
    .expect("media");
    let src = format!("![](cid:{cid}) ").repeat(1_470);
    let r = render(&src, &media, &RenderOpts::ALL);
    assert!(r.html.len() < 400_000, "html {} bytes", r.html.len());
    let color = PrintOpts {
        color: true,
        hyperlinks: true,
        reveal_spoilers: true,
        width: Some(60),
    };
    for opts in [color, PrintOpts::default()] {
        let p = print_doc(&r.styled, &opts);
        assert!(p.len() < 400_000, "printed {} bytes", p.len());
    }
    // Table padding.
    let src = format!(
        "|h|h|\n|-|-|\n|{}|y|\n{}",
        "w".repeat(40_000),
        "|x|y|\n".repeat(10_000)
    );
    let r = render(&src, &[], &RenderOpts::ALL);
    for opts in [color, PrintOpts::default()] {
        let p = print_doc(&r.styled, &opts);
        assert!(p.len() < 8 * src.len(), "table printed {} bytes", p.len());
        check_printer_output(&p).expect("printer");
    }
    // OSC 8 per word / per grapheme.
    for text in ["a ".repeat(48_000), "a".repeat(96_000)] {
        let src = format!("[{text}](https://a.example/{})", "A".repeat(2020));
        let r = render(&src, &[], &RenderOpts::ALL);
        let p = print_doc(&r.styled, &color);
        assert!(
            p.len() < 4 * src.len() + (2 << 20),
            "link printed {} bytes",
            p.len()
        );
        check_printer_output(&p).expect("printer");
    }
    // Footnote references expand to a `sup` and a back-link each; the budget caps the total.
    let src = "x[^1]".repeat(190_000) + "\n\n[^1]: y";
    let r = render(&src, &[], &RenderOpts::ALL);
    assert!(r.html.len() <= 40 << 20, "html {} bytes", r.html.len());
    assert!(check_html(&r.html).is_ok());
}

/// The red-team math inputs that took seconds to hours in math-core 0.9 are refused at once.
#[test]
fn math_engine_is_fast() {
    let n = |s: &str, k: usize| s.repeat(k);
    let cases = [
        format!("${}x$", n("(", 40)),
        format!("$${}$$", n("\\lAngle ", 30)),
        format!("${}$", n("[x", 40)),
        format!("${}$", n("\\{", 40)),
        format!("${}$", n("\\lvert x", 40)),
        format!("${}$", n("\u{27E8}", 40)),
        format!("${}$", n("\u{230A}", 40)),
        format!("${}x$", n("\\bigl(", 40)),
        format!("${}x$", n("\\mathopen{(}", 40)),
        format!("${}x$", n("\\llcorner ", 40)),
        format!("${}x$", n("\\ulcorner ", 40)),
        format!("${}x$", n("\\lmoustache ", 40)),
        format!("${}x$", n("\\sin(", 40)),
        format!("$\\frac{{{}x}}{{y}}$", n("(", 40)),
        format!("$\\mathrm{{{}x}}$", n("(", 40)),
        format!("$\\sqrt[{}x]{{y}}$", n("(", 40)),
    ];
    for c in &cases {
        let t = Stopwatch::start();
        let r = render(
            c,
            &[],
            &RenderOpts {
                html: false,
                styled: false,
                text: true,
            },
        );
        let el = t.elapsed();
        assert!(el.as_secs() < 2, "{c:?} took {el:?}");
        // The guard refuses it (Temml mode, without the `math` feature, has no engine).
        let expected = u32::from(cfg!(feature = "math"));
        assert_eq!(r.flags.math_errors, expected, "{c:?}");
    }
}

/// Deep-nesting inputs (§8.4, F8) on a small stack, as in wasm (1 MiB default). On wasm32 they
/// run on the module's own stack, which is that default.
#[test]
fn deep_nesting_is_stack_safe() {
    let cases: Vec<String> = vec![
        ">".repeat(10_000) + " x",
        "[".repeat(10_000) + "x",
        "![".repeat(10_000) + "x" + &"](y)".repeat(10_000),
        "*a ".repeat(10_000) + "x" + &" b*".repeat(10_000),
        "- ".repeat(5_000) + "x",
        "> - ".repeat(5_000) + "x",
        "1. ".repeat(5_000) + "x",
        "[^1]: ".repeat(2_000) + "x[^1]",
        "> [!details] ".repeat(3_000) + "x",
        "**".repeat(10_000),
        "`".repeat(10_000),
        "<".repeat(10_000) + "a",
        "|".repeat(10_000) + "\n|-|\n",
        format!("${}x$", "\\sqrt".repeat(95)),
        format!("${}x$", "\\left(".repeat(64) + &"\\right)".repeat(64)),
        format!("${}x{}$", "{".repeat(64), "}".repeat(64)),
        format!("${}x{}$", "\\frac{".repeat(32), "}{y}".repeat(32)),
        format!("${}x$", "\\frac a".repeat(47)),
        format!("${}x$", "\\hat".repeat(96)),
        format!("${}x$", "\\mathbf".repeat(96)),
        format!("${}x$", "\\sqrt[".repeat(40) + &"]".repeat(40)),
        format!("${}x$", "x^\\hat".repeat(48)),
        // Delimiter nesting at the limit, inside deep groups and argument chains.
        format!(
            "${}{}x{}{}$",
            "{".repeat(60),
            "(".repeat(64),
            ")".repeat(64),
            "}".repeat(60)
        ),
        format!(
            "${}{{{}x{}}}$",
            "\\sqrt".repeat(90),
            "(".repeat(64),
            ")".repeat(64)
        ),
        format!("${}x$", "-".repeat(4000)),
        format!("${}x$", "\\sin ".repeat(800)),
        format!("${}x$", "\\begingroup ".repeat(95)),
    ];
    let run = move || {
        for c in &cases {
            let r = render(c, &[], &RenderOpts::ALL);
            check_html(&r.html).unwrap_or_else(|e| panic!("{e} for {:?}", &c[..c.len().min(40)]));
            let t = render_with_math_mode(c, &[], None, &RenderOpts::ALL, MathMode::Temml);
            check_html(&t.html).unwrap_or_else(|e| panic!("{e}"));
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(run)
        .expect("thread")
        .join()
        .expect("deep nesting must not overflow a 1 MiB stack");
    #[cfg(target_arch = "wasm32")]
    run();
}

/// Deep nesting finishes quickly (linear work; generous bound for debug builds).
#[test]
fn deep_nesting_is_fast() {
    let cases = [
        ">".repeat(10_000) + " x",
        "[".repeat(10_000) + "x",
        "![".repeat(5_000) + "x" + &"](y)".repeat(5_000),
        "_www.".repeat(20_000),
        "(www.!".repeat(20_000),
        "www.a.com*".repeat(20_000),
        "[a](b) ".repeat(20_000),
        "# a\n".repeat(20_000),
        "x[^a]\n\n[^a]: ".repeat(5_000),
    ];
    for c in &cases {
        let t = Stopwatch::start();
        let r = render(c, &[], &RenderOpts::ALL);
        let el = t.elapsed();
        assert!(el.as_secs() < 20, "{:?}… took {el:?}", &c[..20]);
        assert!(check_html(&r.html).is_ok());
    }
}

/// The SKind enum is exhaustively exercised by the vectors; this guards the empty case.
#[test]
fn empty_and_error_docs() {
    let r = render("", &[], &RenderOpts::ALL);
    assert!(r.styled.blocks.is_empty());
    let r = render(&"a".repeat(1_000_001), &[], &RenderOpts::ALL);
    assert!(matches!(r.styled.blocks[0].kind, SKind::Notice(_)));
    assert!(check_html(&r.html).is_ok());
}
