// SPDX-License-Identifier: AGPL-3.0-or-later
//! The §7.1 acceptance pre-scan (`docs/spec/lfm-1.md`, part of `lfm/1`): size, group depth and
//! disabled control words, checked on the raw TeX before any engine sees it.

/// The most bytes one expression may have (§7.1 rule 1).
pub const MAX_TEX_BYTES: usize = 4096;

/// The deepest group nesting an expression may have (§7.1 rule 2).
pub const MAX_GROUP_DEPTH: i64 = 64;

/// The disabled control words of §7.1 rule 3 (without the `\`); every control word starting
/// with `html` is disabled too.
///
/// math-core 0.9 rejects some of these itself (`\href`, `\url`, `\includegraphics`, `\class`,
/// `\style`, `\html…`), but it expands `\def`, `\newcommand` and `\let`, and turns `\label`
/// into an `id` and `\eqref` into an `<a href="#…">` (ADR 0002 decision 2). The list does not
/// depend on the engine.
pub const DISABLED: &[&str] = &[
    "href",
    "url",
    "includegraphics",
    "class",
    "style",
    "def",
    "gdef",
    "edef",
    "xdef",
    "let",
    "futurelet",
    "newcommand",
    "renewcommand",
    "providecommand",
    "newenvironment",
    "renewenvironment",
    "DeclareMathOperator",
    "DeclareRobustCommand",
    "catcode",
    "input",
    "include",
    "label",
    "eqref",
    "ref",
];

/// The §7.1 pre-scan: size, group depth and disabled control words. `true` if accepted.
///
/// Control sequences are tokenised left to right: `\` followed by letters is a control word;
/// `\` followed by any other character (including `\{`, `\}` and `\\`) is a two-character
/// control symbol that is neither a group nor the start of a control word.
pub fn prescan(tex: &str) -> bool {
    if tex.len() > MAX_TEX_BYTES {
        return false;
    }
    let b = tex.as_bytes();
    let mut depth: i64 = 0;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                let j = i + 1;
                if j < b.len() && b[j].is_ascii_alphabetic() {
                    let mut k = j;
                    while k < b.len() && b[k].is_ascii_alphabetic() {
                        k += 1;
                    }
                    let w = &tex[j..k];
                    if DISABLED.contains(&w) || w.starts_with("html") {
                        return false;
                    }
                    match w {
                        "left" | "begin" => depth += 1,
                        "right" | "end" => depth -= 1,
                        _ => {}
                    }
                    i = k;
                } else if j < b.len() {
                    let cl = tex[j..].chars().next().map_or(1, char::len_utf8);
                    i = j + cl;
                } else {
                    i = j;
                }
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
        if !(0..=MAX_GROUP_DEPTH).contains(&depth) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescan_rules() {
        assert!(prescan("x^2"));
        assert!(!prescan("\\href{x}{y}"));
        assert!(!prescan("\\text{\\url{x}}"));
        assert!(!prescan("x % \\def"));
        assert!(!prescan("\\htmlClass{a}{b}"));
        assert!(prescan("\\\\href"));
        assert!(prescan("\\{ \\}"));
        assert!(!prescan("}{"));
        assert!(prescan(&format!("{}x{}", "{".repeat(64), "}".repeat(64))));
        assert!(!prescan(&format!("{}x{}", "{".repeat(65), "}".repeat(65))));
        assert!(prescan(&"x".repeat(4096)));
        assert!(!prescan(&"x".repeat(4097)));
    }

    /// ADR 0002 decision 2: the label and reference commands are disabled; `\tag` is not.
    #[test]
    fn label_and_references_disabled() {
        for bad in [
            "\\begin{equation} x \\label{e} \\end{equation}",
            "\\eqref{e}",
            "\\ref{e}",
            "x \\text{\\label{a}}",
            "x % \\ref",
        ] {
            assert!(!prescan(bad), "{bad}");
        }
        for ok in [
            "\\begin{equation} x \\tag{1} \\end{equation}",
            "\\labelx",
            "\\refx",
            "\\eqrefs",
            "\\\\ref",
            "\\rightarrow \\rfloor",
        ] {
            assert!(prescan(ok), "{ok}");
        }
    }
}
