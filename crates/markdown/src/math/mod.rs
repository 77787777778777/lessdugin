// SPDX-License-Identifier: AGPL-3.0-or-later
//! Math (`docs/spec/lfm-1.md` §7): the acceptance pre-scan, the math-core engine and its guard,
//! the MathML normaliser and serialiser, math errors and the Temml fallback output.
//!
//! Following ADR 0002 decision 2 the engine is wrapped in two mandatory passes:
//!
//! - [`prescan()`] (lexical, before the engine, part of `lfm/1`): size, group depth and the
//!   disabled control words ([`prescan::DISABLED`]);
//! - the normaliser ([`normalize`], after the engine, presentation): the engine's MathML is never
//!   inserted as is. It is parsed into a tree (iteratively), its `style` values are mapped to the
//!   closed set of `mc-*` classes and `mathcolor`, and it is filtered against the §7.3 allowlist
//!   and re-serialised by our own writer. [`MATHML_CSS`] styles the classes.

// Without the `math` feature only the pre-scan and the Temml output are live; the normaliser and
// the engine guard stay compiled (and unit-tested) so both builds share one implementation.
#![cfg_attr(not(feature = "math"), allow(dead_code))]

mod guard;
pub mod normalize;
pub mod prescan;

#[cfg(feature = "math")]
use guard::guard_unmatched;
pub use normalize::{
    ENGINE_CLASSES, MATHML_ATTRIBUTES, MATHML_CLASSES, MATHML_ELEMENTS, STYLE_CLASSES,
    TEXT_STYLE_CLASSES, mathml_attr_ok,
};
pub use prescan::prescan;

use crate::html::escape_text;

/// The stylesheet for rendered MathML (§7.3): math-core's `mathmlfixes.css` (MIT, with its
/// notice) followed by the `mc-*` classes of the normaliser (`assets/mathml.css`). The SPA
/// imports the same two files (`web/src/styles/mathml.css`); exported HTML can embed this.
pub const MATHML_CSS: &str = concat!(
    include_str!("../../assets/mathmlfixes.css"),
    "\n",
    include_str!("../../assets/mathml.css")
);

/// Which math output a build produces (§7.5). Part of [`crate::RENDERER_VERSION`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathMode {
    /// MathML through math-core (feature `math`).
    MathCore,
    /// `<span data-tex>` for the SPA's lazily loaded Temml.
    Temml,
}

/// The mode of this build.
pub const MATH_MODE: MathMode = if cfg!(feature = "math") {
    MathMode::MathCore
} else {
    MathMode::Temml
};

/// The most MathML bytes one document may contain (presentation). Each expression is capped
/// at 65,536 bytes (§7.3); this caps their sum, so that a body of many large expressions cannot
/// expand without bound. Expressions past it render as math errors (§7.4). Real documents stay
/// far below it.
pub const MATHML_BUDGET: usize = 16 << 20;

/// The engine work one document may use (presentation), in byte-equivalents. An expression of
/// `n` bytes that the guard accepts with `u` unmatched opening delimiters costs `n << u`: the
/// guard bounds one expression (math-core 0.9's operator inference does up to `2^u` times the
/// work of a balanced row, `u ≤ 8`), and this bounds their sum. Once a document has spent it,
/// further expressions render as math errors (§7.4). Without it a 1 MB body of 243 4 KB
/// expressions with 8 unmatched delimiters each took 7.8 s natively (NOTES bug 19); with it that
/// body takes 0.17 s (four expressions spend the budget), and no 1 MB body measured took longer
/// than a balanced one, about 0.45 s. A body (at most 1,000,000 bytes) whose expressions each
/// have at most 2 unmatched delimiters can never reach it.
pub const ENGINE_BUDGET: u64 = 1 << 22;

/// Renders one math expression; returns the HTML fragment and whether it is a math error.
pub(crate) struct MathRenderer {
    mode: MathMode,
    /// MathML bytes still allowed in this document.
    mathml_left: usize,
    /// Engine work still allowed in this document ([`ENGINE_BUDGET`]).
    engine_left: u64,
    #[cfg(feature = "math")]
    engine: Option<math_core::LatexToMathML>,
}

impl MathRenderer {
    pub(crate) fn new(mode: MathMode) -> Self {
        Self {
            mode,
            mathml_left: MATHML_BUDGET,
            engine_left: ENGINE_BUDGET,
            #[cfg(feature = "math")]
            engine: None,
        }
    }

    pub(crate) fn render(&mut self, tex: &str, display: bool) -> (String, bool) {
        if !prescan(tex) {
            return (error_html(tex), true);
        }
        match self.mode {
            MathMode::Temml => {
                let mode = if display { "block" } else { "inline" };
                (
                    format!("<span data-tex=\"{mode}\">{}</span>", escape_text(tex)),
                    false,
                )
            }
            MathMode::MathCore => match self.engine_mathml(tex, display, &mut Vec::new()) {
                Some(html) if html.len() <= self.mathml_left => {
                    self.mathml_left -= html.len();
                    (html, false)
                }
                _ => (error_html(tex), true),
            },
        }
    }

    /// The engine (behind its guard) and the normaliser. `None` is an engine error.
    #[cfg(feature = "math")]
    fn engine_mathml(
        &mut self,
        tex: &str,
        display: bool,
        unknown: &mut Vec<String>,
    ) -> Option<String> {
        use math_core::{
            LatexToMathML, MathCoreConfig, MathDisplay, MaxExpansions, PrettyPrint,
            UnicodeSubstitution,
        };
        let unmatched = guard_unmatched(tex)?;
        let cost = (tex.len() as u64) << unmatched;
        if cost > self.engine_left {
            return None;
        }
        self.engine_left -= cost;
        if self.engine.is_none() {
            let cfg = MathCoreConfig {
                pretty_print: PrettyPrint::Never,
                xml_namespace: false,
                ignore_unknown_commands: false,
                annotation: false,
                allow_unreliable_rendering: false,
                global_group: false,
                unicode_substitution: UnicodeSubstitution::Conventional,
                max_expansions: MaxExpansions(1000),
                ..Default::default()
            };
            self.engine = LatexToMathML::new(cfg, Vec::new()).ok();
        }
        let engine = self.engine.as_ref()?;
        let mode = if display {
            MathDisplay::Block
        } else {
            MathDisplay::Inline
        };
        let out = engine.convert_with_local_state(tex, mode).ok()?;
        normalize::normalise_mathml(&out.mathml, display, unknown)
    }

    #[cfg(not(feature = "math"))]
    fn engine_mathml(
        &mut self,
        _tex: &str,
        _display: bool,
        _unknown: &mut Vec<String>,
    ) -> Option<String> {
        None
    }
}

/// `<code class="math-error">TEX</code>` (§7.4).
pub(crate) fn error_html(tex: &str) -> String {
    format!("<code class=\"math-error\">{}</code>", escape_text(tex))
}

/// The `style` declarations and class tokens of the engine's output for `tex` that the
/// normaliser does not know (and so dropped). Empty for every expression of the
/// `testdata/vectors/mathml` corpus; a math-core upgrade that emits new styles shows up here
/// (ADR 0002 consequences). Test support, not part of the rendering API.
#[doc(hidden)]
pub fn unknown_engine_styles(tex: &str, display: bool) -> Vec<String> {
    let mut unknown = Vec::new();
    if prescan(tex) {
        let _ = MathRenderer::new(MathMode::MathCore).engine_mathml(tex, display, &mut unknown);
    }
    unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class token has a rule in the stylesheet, and the stylesheet keeps math-core's
    /// licence notice.
    #[test]
    fn stylesheet_covers_every_class() {
        for c in MATHML_CLASSES {
            assert!(
                MATHML_CSS.contains(&format!(".{c} ")) || MATHML_CSS.contains(&format!(".{c},")),
                "no rule for .{c} in assets/mathml.css"
            );
        }
        assert!(MATHML_CSS.contains("SPDX-License-Identifier: MIT"));
        assert!(MATHML_CSS.contains("Permission is hereby granted, free of charge"));
        assert!(!MATHML_CSS.contains("!important;"));
    }

    /// The rule of each class in `assets/mathml.css` has exactly the declarations that map to it
    /// in [`STYLE_CLASSES`] (§7.3), so the class restores what the style did.
    #[test]
    fn stylesheet_matches_style_table() {
        let css = include_str!("../../assets/mathml.css");
        for (_, class) in STYLE_CLASSES {
            let head = format!("math .{class}.{class} {{");
            let start = css.find(&head).expect("rule") + head.len();
            let body = &css[start..start + css[start..].find('}').expect("end")];
            let mut got: Vec<String> = body
                .split(';')
                .filter(|d| !d.trim().is_empty())
                .map(|d| normalize::normalise_declaration(d).expect("declaration"))
                .collect();
            let mut want: Vec<String> = STYLE_CLASSES
                .iter()
                .filter(|(_, c)| c == class)
                .map(|(d, _)| (*d).to_owned())
                .collect();
            got.sort();
            want.sort();
            assert_eq!(got, want, "{class}");
        }
    }

    #[cfg(feature = "math")]
    #[test]
    fn label_eqref_ref_are_math_errors() {
        let mut m = MathRenderer::new(MathMode::MathCore);
        for tex in [
            "\\begin{equation} x \\label{e} \\end{equation}",
            "\\eqref{e}",
            "\\ref{e}",
        ] {
            let (html, err) = m.render(tex, true);
            assert!(err, "{tex}");
            assert!(html.starts_with("<code class=\"math-error\">"), "{html}");
        }
        let (html, err) = m.render("\\begin{equation} x \\tag{1} \\end{equation}", true);
        assert!(!err);
        assert!(!html.contains(" id=") && !html.contains("href"), "{html}");
    }

    /// An expression costs `len << unmatched` of [`ENGINE_BUDGET`], charged before the engine
    /// runs; once the rest is too small every expression is a math error.
    #[cfg(feature = "math")]
    #[test]
    fn engine_budget() {
        let mut m = MathRenderer::new(MathMode::MathCore);
        m.engine_left = 2 * 9 * 256 + 3;
        let eight = "((((((((x";
        assert_eq!(guard::guard_unmatched(eight), Some(8));
        assert!(!m.render(eight, false).1);
        assert!(!m.render(eight, false).1);
        assert_eq!(m.engine_left, 3);
        assert!(m.render(eight, false).1, "over the budget");
        assert!(!m.render("x+y", false).1);
        assert!(m.render("x", false).1, "budget spent");
        assert_eq!(m.engine_left, 0);
        // Refused by the guard or the pre-scan: nothing is charged.
        let mut m = MathRenderer::new(MathMode::MathCore);
        assert!(m.render("(((((((((x", false).1);
        assert!(m.render("\\ref{a}", false).1);
        assert_eq!(m.engine_left, ENGINE_BUDGET);
    }

    #[cfg(feature = "math")]
    #[test]
    fn no_style_attribute_ever() {
        let mut m = MathRenderer::new(MathMode::MathCore);
        let (html, err) = m.render(
            "\\color{red} \\begin{cases} a & b \\end{cases} \\begin{array}{|l|} \\hline x \\end{array} a\\!b \\text{\\small\\textsc{c}} \\cancel{y} \\cfrac{1}{2} \\cramped{x^2}",
            true,
        );
        assert!(!err, "{html}");
        assert!(!html.contains(" style="), "{html}");
        assert!(
            html.contains("mathcolor=\"#ff0000\"") && html.contains("class=\"mc-"),
            "{html}"
        );
        assert_eq!(
            crate::allowlist::check_html(&format!(
                "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\">{html}</p>\n</div>\n"
            )),
            Ok(())
        );
    }
}
