// SPDX-License-Identifier: AGPL-3.0-or-later
//! The engine guard (`docs/spec/lfm-1.md` §7.2, presentation): bounds of math-core 0.9's
//! recursion depth and of its operator-inference time that the §7.1 pre-scan does not give.

/// Engine guard (presentation; an engine refusal is a math error, §7.2).
///
/// math-core 0.9 has two input-driven costs that the §7.1 pre-scan does not bound:
///
/// - **Stack.** It parses recursively without a depth limit, so argument chains such as
///   `\sqrt\sqrt…x` or `\frac a\frac a…` can exhaust a 1 MiB (wasm) stack within 4,096 bytes.
///   The guard estimates the parser's nesting from the argument counts of math-core 0.9's
///   commands and refuses anything deeper than [`MAX_ENGINE_NESTING`].
/// - **Time.** Its operator inference (`semantic::enrich_formula`) tries to read a bracketed
///   group at every opening delimiter; when the row has no matching closing delimiter it
///   re-reads the whole rest of the row as if the delimiter were an atom, so every unmatched
///   opening delimiter doubles the work (`$((((…(x$` with 28 `(` takes about ten seconds). The
///   guard simulates delimiter matching and refuses more than [`MAX_UNMATCHED_OPEN`] unmatched
///   opening delimiters in the expression (bounding the blow-up to 2^8), and more than
///   [`MAX_DELIMITER_NESTING`] open delimiters within one row (the inference recursion grows
///   with them).
///
/// The delimiter count is an upper bound by construction. Every token that math-core 0.9 can
/// turn into an opening delimiter counts, including one it takes as a command argument or
/// script, and one inside a group (a group holding a single node passes that node through to
/// the enclosing row). A closing delimiter cancels an opening one only when both sit directly
/// in the same row (not as an argument) and no row break (`&`, `\\`, `\newline`, `\over` and
/// the other infix fractions) lies between them. Real formulas stay far below both limits;
/// anything that deep would exceed the 64-level MathML cap anyway.
pub(crate) const MAX_ENGINE_NESTING: usize = 96;

/// The most unmatched opening delimiters an expression may have (see [`guard_unmatched`]).
pub(crate) const MAX_UNMATCHED_OPEN: u32 = 8;

/// The most opening delimiters that may be open at once within one row.
pub(crate) const MAX_DELIMITER_NESTING: u32 = 64;

fn arity(word: &str) -> u8 {
    match word {
        "genfrac" => 6,
        "mathchoice" => 4,
        "sideset" | "prescript" => 3,
        "frac" | "cfrac" | "dfrac" | "tfrac" | "binom" | "dbinom" | "tbinom" | "overset"
        | "stackrel" | "underset" | "raisebox" | "textcolor" => 2,
        "sqrt"
        | "overbrace"
        | "overbracket"
        | "overparen"
        | "underbrace"
        | "underbracket"
        | "underparen"
        | "Bbb"
        | "bm"
        | "bold"
        | "boldsymbol"
        | "frak"
        | "mathbb"
        | "mathbf"
        | "mathcal"
        | "mathfrak"
        | "mathit"
        | "mathrm"
        | "mathscr"
        | "mathsf"
        | "mathtt"
        | "symbf"
        | "Big"
        | "Bigg"
        | "Biggl"
        | "Biggm"
        | "Biggr"
        | "Bigl"
        | "Bigm"
        | "Bigr"
        | "big"
        | "bigg"
        | "biggl"
        | "biggm"
        | "biggr"
        | "bigl"
        | "bigm"
        | "bigr"
        | "Overrightarrow"
        | "acute"
        | "bar"
        | "breve"
        | "check"
        | "ddddot"
        | "dddot"
        | "ddot"
        | "dot"
        | "grave"
        | "hat"
        | "mathring"
        | "overgroup"
        | "overleftarrow"
        | "overleftharpoon"
        | "overleftrightarrow"
        | "overline"
        | "overrightarrow"
        | "overrightharpoon"
        | "tilde"
        | "undergroup"
        | "underleftarrow"
        | "underleftrightarrow"
        | "underrightarrow"
        | "underline"
        | "vec"
        | "widehat"
        | "widecheck"
        | "utilde"
        | "angl"
        | "bcancel"
        | "boxed"
        | "cancel"
        | "phase"
        | "xcancel"
        | "operatorname"
        | "operatornamewithlimits"
        | "not"
        | "slashed"
        | "vertoverlay"
        | "emph"
        | "text"
        | "textbf"
        | "textit"
        | "textrm"
        | "textsc"
        | "textsf"
        | "texttt"
        | "underbar"
        | "hphantom"
        | "phantom"
        | "vphantom"
        | "mathbin"
        | "mathclose"
        | "mathinner"
        | "mathop"
        | "mathopen"
        | "mathord"
        | "mathpunct"
        | "mathrel"
        | "color"
        | "hskip"
        | "kern"
        | "mkern"
        | "mskip"
        | "hspace"
        | "mspace"
        | "middle"
        | "Bra"
        | "Braket"
        | "Ket"
        | "Set"
        | "bra"
        | "braket"
        | "fbox"
        | "ket"
        | "pmod"
        | "pod"
        | "set"
        | "cramped"
        | "eqref"
        | "label"
        | "tag"
        | "shoveleft"
        | "shoveright" => 1,
        w if w.starts_with('x') && w.len() > 4 => 1, // \xrightarrow and friends
        _ => 0,
    }
}

fn takes_optional(word: &str) -> bool {
    word == "sqrt" || word == "cramped" || (word.starts_with('x') && word.len() > 4)
}

/// Commands whose next token is a delimiter (not a delimiter of the row).
fn takes_delimiter(word: &str) -> bool {
    matches!(
        word,
        "left"
            | "right"
            | "middle"
            | "big"
            | "Big"
            | "bigg"
            | "Bigg"
            | "bigl"
            | "Bigl"
            | "biggl"
            | "Biggl"
            | "bigr"
            | "Bigr"
            | "biggr"
            | "Biggr"
            | "bigm"
            | "Bigm"
            | "biggm"
            | "Biggm"
    )
}

/// Operator-inference roles of math-core 0.9 tokens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Delim {
    Open,
    Close,
}

/// Control words that math-core 0.9 turns into opening or closing delimiters (`Open`,
/// `ForceOpen` and `Big(_, Some(Left))`; `Close`, `ForceClose` and `Big(_, Some(Right))`).
fn word_delim(w: &str) -> Option<Delim> {
    match w {
        "lAngle" | "lBrace" | "lVert" | "lang" | "langle" | "lbrace" | "lbrack" | "lceil"
        | "lfloor" | "lgroup" | "llangle" | "llbracket" | "llparenthesis" | "lparen" | "lvert"
        | "llcorner" | "lmoustache" | "ulcorner" | "bigl" | "Bigl" | "biggl" | "Biggl" => {
            Some(Delim::Open)
        }
        "rAngle" | "rBrace" | "rVert" | "rang" | "rangle" | "rbrace" | "rbrack" | "rceil"
        | "rfloor" | "rgroup" | "rrangle" | "rrbracket" | "rrparenthesis" | "rvert"
        | "lrcorner" | "rmoustache" | "urcorner" | "bigr" | "Bigr" | "biggr" | "Biggr" => {
            Some(Delim::Close)
        }
        _ => None,
    }
}

/// Characters that math-core 0.9 turns into delimiters (its lexer and `UNICODE_OPERATORS`).
/// Every other opening punctuation (General_Category Ps) also counts as opening, which can
/// only make the guard stricter; the closing set is exact.
fn char_delim(c: char) -> Option<Delim> {
    use unicode_general_category::{GeneralCategory, get_general_category};
    match c {
        '(' | '[' | '\u{2308}' | '\u{230A}' | '\u{231C}' | '\u{231E}' | '\u{23B0}' | '\u{2772}'
        | '\u{27C5}' | '\u{27E6}' | '\u{27E8}' | '\u{27EA}' | '\u{27EC}' | '\u{27EE}'
        | '\u{2983}' | '\u{2985}' | '\u{2987}' | '\u{2989}' | '\u{298B}' | '\u{298D}'
        | '\u{298F}' | '\u{2991}' | '\u{2993}' | '\u{2995}' | '\u{2997}' | '\u{29D8}'
        | '\u{29DA}' | '\u{29FC}' => Some(Delim::Open),
        ')' | ']' | '!' | '\u{2309}' | '\u{230B}' | '\u{231D}' | '\u{231F}' | '\u{23B1}'
        | '\u{2773}' | '\u{27C6}' | '\u{27E7}' | '\u{27E9}' | '\u{27EB}' | '\u{27ED}'
        | '\u{27EF}' | '\u{2984}' | '\u{2986}' | '\u{2988}' | '\u{298A}' | '\u{298C}'
        | '\u{298E}' | '\u{2990}' | '\u{2992}' | '\u{2994}' | '\u{2996}' | '\u{2998}'
        | '\u{29D9}' | '\u{29DB}' | '\u{29FD}' => Some(Delim::Close),
        c if get_general_category(c) == GeneralCategory::OpenPunctuation => Some(Delim::Open),
        _ => None,
    }
}

/// One open construct of the guard's simulated parse.
#[derive(Clone, Copy)]
enum Frame {
    /// A row: `{…}`, `\left…\right`, `\begin…\end` or `\begingroup…\endgroup`. The count is
    /// the number of its opening delimiters that a closing one may still cancel.
    Group(u32),
    /// The optional `[…]` argument of `\sqrt`, `\cramped` or an extensible arrow (a row too).
    Bracket(u32),
    /// A command still waiting for this many arguments.
    Pending(u8),
}

struct Guard {
    st: Vec<Frame>,
    /// The cancellable opening delimiters of the outermost row.
    top: u32,
    /// Opening delimiters not (yet) cancelled, over the whole expression.
    unmatched: u32,
}

impl Guard {
    /// A token completed: pop every command whose last argument it was.
    fn complete(&mut self) {
        while let Some(Frame::Pending(n)) = self.st.last_mut() {
            if *n > 1 {
                *n -= 1;
                return;
            }
            self.st.pop();
        }
    }

    /// Pop to (and including) the nearest row (only a `Bracket` row when `bracket`).
    fn pop_to(&mut self, bracket: bool) -> bool {
        let pos = self.st.iter().rposition(|f| match f {
            Frame::Group(_) => true,
            Frame::Bracket(_) => bracket,
            Frame::Pending(_) => false,
        });
        match pos {
            Some(p) if bracket && !matches!(self.st[p], Frame::Bracket(_)) => false,
            Some(p) => {
                self.st.truncate(p);
                true
            }
            None => false,
        }
    }

    /// A delimiter at the current position (before it completes a pending command). `false`
    /// if the row's delimiter nesting is over the limit.
    fn delimiter(&mut self, d: Delim) -> bool {
        let row = match self.st.last_mut() {
            None => Some(&mut self.top),
            Some(Frame::Group(n) | Frame::Bracket(n)) => Some(n),
            // An argument or script: never cancels, and an opening one is never cancelled.
            Some(Frame::Pending(_)) => None,
        };
        match (row, d) {
            (Some(n), Delim::Open) => {
                *n += 1;
                self.unmatched += 1;
                *n <= MAX_DELIMITER_NESTING
            }
            (None, Delim::Open) => {
                self.unmatched += 1;
                true
            }
            (Some(n), Delim::Close) => {
                if *n > 0 {
                    *n -= 1;
                    self.unmatched -= 1;
                }
                true
            }
            (None, Delim::Close) => true,
        }
    }

    /// A row break (`&`, `\\`, `\newline`, `\over`, …): closing delimiters after it cannot
    /// cancel opening ones before it.
    fn barrier(&mut self) {
        let row = self.st.iter_mut().rev().find_map(|f| match f {
            Frame::Group(n) | Frame::Bracket(n) => Some(n),
            Frame::Pending(_) => None,
        });
        *row.unwrap_or(&mut self.top) = 0;
    }
}

/// The engine guard: `None` if `tex` exceeds the engine's nesting or delimiter limits (see
/// [`MAX_ENGINE_NESTING`]); otherwise the upper bound on its unmatched opening delimiters (at
/// most [`MAX_UNMATCHED_OPEN`]), since math-core's operator inference may do up to
/// `2^unmatched` times the work of a balanced row ([`super::ENGINE_BUDGET`]).
pub(crate) fn guard_unmatched(tex: &str) -> Option<u32> {
    let b = tex.as_bytes();
    let mut g = Guard {
        st: Vec::new(),
        top: 0,
        unmatched: 0,
    };
    let mut opt_allowed = false;
    // The next token is the delimiter of `\left`, `\right`, `\middle` or `\big…`.
    let mut delim_next = false;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        let mut next_opt = false;
        let mut next_delim = false;
        let mut ok = true;
        match c {
            b'\\' => {
                let j = i + 1;
                if j < b.len() && b[j].is_ascii_alphabetic() {
                    let mut k = j;
                    while k < b.len() && b[k].is_ascii_alphabetic() {
                        k += 1;
                    }
                    let w = &tex[j..k];
                    if !delim_next && let Some(d) = word_delim(w) {
                        ok = g.delimiter(d);
                    }
                    match w {
                        "left" => {
                            g.st.push(Frame::Group(0));
                            g.st.push(Frame::Pending(1));
                            next_delim = true;
                        }
                        "right" => {
                            g.pop_to(false);
                            g.st.push(Frame::Pending(1));
                            next_delim = true;
                        }
                        "begin" | "begingroup" => g.st.push(Frame::Group(0)),
                        "end" => {
                            g.pop_to(false);
                        }
                        "endgroup" => {
                            g.pop_to(false);
                            g.complete();
                        }
                        "over" | "atop" | "choose" | "brace" | "brack" | "newline" => {
                            g.barrier();
                            g.complete();
                        }
                        w => match arity(w) {
                            0 => g.complete(),
                            n => {
                                g.st.push(Frame::Pending(n));
                                next_opt = takes_optional(w);
                                next_delim = takes_delimiter(w);
                            }
                        },
                    }
                    i = k;
                } else {
                    let sym = tex.get(j..).and_then(|r| r.chars().next());
                    if !delim_next {
                        match sym {
                            Some('{') => ok = g.delimiter(Delim::Open),
                            Some('}') => ok = g.delimiter(Delim::Close),
                            Some('\\') => g.barrier(),
                            _ => {}
                        }
                    }
                    g.complete();
                    i = j + sym.map_or(0, char::len_utf8);
                }
            }
            b'{' => {
                g.st.push(Frame::Group(0));
                i += 1;
            }
            b'}' => {
                g.pop_to(false);
                g.complete();
                i += 1;
            }
            b'[' if opt_allowed => {
                g.st.push(Frame::Bracket(0));
                i += 1;
            }
            b']' => {
                if delim_next {
                    g.complete();
                } else if !g.pop_to(true) {
                    ok = g.delimiter(Delim::Close);
                    g.complete();
                }
                i += 1;
            }
            b'^' | b'_' => {
                g.st.push(Frame::Pending(1));
                i += 1;
            }
            b'%' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'&' => {
                g.barrier();
                i += 1;
            }
            c if c.is_ascii_whitespace() => {
                i += 1;
                next_opt = opt_allowed;
                next_delim = delim_next;
            }
            _ => {
                let ch = tex[i..].chars().next().unwrap_or('\u{FFFD}');
                if !delim_next && let Some(d) = char_delim(ch) {
                    ok = g.delimiter(d);
                }
                g.complete();
                i += ch.len_utf8();
            }
        }
        opt_allowed = next_opt;
        delim_next = next_delim;
        if !ok || g.st.len() > MAX_ENGINE_NESTING {
            return None;
        }
    }
    (g.unmatched <= MAX_UNMATCHED_OPEN).then_some(g.unmatched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_guard(tex: &str) -> bool {
        guard_unmatched(tex).is_some()
    }

    #[test]
    fn guard() {
        assert!(engine_guard("\\frac{a}{b} + \\sqrt[3]{x} \\cdot y^{2}_{i}"));
        assert!(engine_guard(&"a \\cdot ".repeat(500)));
        assert!(engine_guard(&"\\frac12 + ".repeat(300)));
        assert!(!engine_guard(&format!("{}x", "\\sqrt".repeat(200))));
        assert!(!engine_guard(&format!("{}x", "\\frac a".repeat(200))));
        assert!(!engine_guard(&format!("x{}y", "^\\hat".repeat(60))));
        // \begingroup is a group too.
        assert!(!engine_guard(&format!("{}x", "\\begingroup ".repeat(100))));
    }

    /// math-core 0.9 doubles its operator-inference work for every unmatched opening
    /// delimiter (red-team finding: `$((((…(x$` with 28 `(` took 9.6 s).
    #[test]
    fn guard_unmatched_delimiters() {
        let n = |s: &str, k: usize| s.repeat(k);
        for bad in [
            n("(", 28) + "x",
            n("[x", 30),
            n("\\{", 30),
            n("\\lvert x", 30),
            n("\u{27E8}", 30),
            n("\u{230A}", 30),
            n("\\bigl(", 30),
            n("\\Biggl\\langle ", 30),
            n("\\mathopen{(}", 30),
            n("\\mathclose(", 30),
            n("{(}", 30),
            n("\\llcorner ", 30),
            n("\\ulcorner ", 30),
            n("\\lmoustache ", 30),
            n("\\lAngle ", 30),
            n("\\sin(", 24),
            n("(^2", 30),
            format!("\\frac{{{}x}}{{y}}", n("(", 30)),
            format!("\\mathrm{{{}x}}", n("(", 30)),
            format!("\\sqrt[{}x]{{y}}", n("(", 30)),
            format!("\\left({}x\\right)", n("(", 30)),
            // Closing delimiters that are arguments or scripts cancel nothing.
            n("(\\sqrt)", 30),
            n("(x^)", 30),
            n("(\\big)", 30),
            n("(\\left.\\right)", 30),
            // Nor do closing delimiters after a row break.
            format!("{}x & {}", n("(", 30), n(")", 30)),
            format!("{}x \\\\ {}", n("(", 30), n(")", 30)),
            format!("{}x \\over {}", n("(", 30), n(")", 30)),
            // Delimiter nesting deeper than 64 within one row.
            format!("{}x{}", n("(", 65), n(")", 65)),
        ] {
            assert!(!engine_guard(&bad), "accepted {bad:?}");
        }
        for ok in [
            format!("{}x{}", n("(", 64), n(")", 64)),
            n("[0, 1) \\cup ", 40),
            n("f(\\pi) + ", 40),
            n("\\left( x \\right) ", 40),
            n("\\big( x \\big) ", 40),
            n("\\bigl( x \\bigr) ", 40),
            n("\\langle x | ", 8),
            n("n! + ", 40),
            n("\\{ x \\} ", 40),
            format!("{}x", n("(", 8)),
            "\\begin{pmatrix} (a) & [b] \\\\ \\{c\\} & \\langle d \\rangle \\end{pmatrix}"
                .to_owned(),
            "\\sqrt[3]{(x+1)} \\xrightarrow[(a)]{(b)} \\pmod{(n)}".to_owned(),
        ] {
            assert!(engine_guard(&ok), "refused {ok:?}");
        }
    }
}
