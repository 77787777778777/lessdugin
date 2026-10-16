// SPDX-License-Identifier: AGPL-3.0-or-later
//! Property tests (`docs/spec/lfm-1.md` §1.5, §8, §9, §13): rendering never panics, the HTML
//! always passes the allowlist checker (and ammonia, with the feature), P has no line-breaking
//! or tab whitespace, terminal text never carries escapes, and deep nesting is cheap and
//! stack-safe.

use lessdugin_markdown::allowlist::check_html;
use lessdugin_markdown::printer::{PrintOpts, check_printer_output, print_doc};
use lessdugin_markdown::{
    MathMode, MediaItem, RenderOpts, Rendered, SKind, StyledDoc, render, render_with_math_mode,
};
use proptest::prelude::*;

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

fn check_all(src: &str, r: &Rendered) -> Result<(), TestCaseError> {
    check_html(&r.html)
        .map_err(|e| TestCaseError::fail(format!("{e}\nsrc={src:?}\nhtml={}", r.html)))?;
    prop_assert!(
        !r.text.chars().any(|c| c.is_whitespace() && c != ' '),
        "P has whitespace: {:?}",
        r.text
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
}

/// Deep-nesting inputs (§8.4, F8) on a small stack, as in wasm (1 MiB default).
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
    ];
    let h = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || {
            for c in &cases {
                let r = render(c, &[], &RenderOpts::ALL);
                check_html(&r.html)
                    .unwrap_or_else(|e| panic!("{e} for {:?}", &c[..c.len().min(40)]));
                let t = render_with_math_mode(c, &[], None, &RenderOpts::ALL, MathMode::Temml);
                check_html(&t.html).unwrap_or_else(|e| panic!("{e}"));
            }
        })
        .expect("thread");
    h.join()
        .expect("deep nesting must not overflow a 1 MiB stack");
}

/// Deep nesting finishes quickly (linear work; generous bound for debug builds).
#[test]
#[allow(clippy::disallowed_methods)] // timing a test is fine; the crate itself never reads clocks
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
        let t = std::time::Instant::now();
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
