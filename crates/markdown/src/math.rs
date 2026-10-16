// SPDX-License-Identifier: AGPL-3.0-or-later
//! Math (`docs/spec/lfm-1.md` §7): acceptance pre-scan, the math-core engine, the MathML
//! filter and serialiser, math errors and the Temml fallback output.
//!
//! The engine's MathML is never inserted as is: it is tokenised and parsed into a tree
//! (iteratively), filtered against the §7.3 allowlist and re-serialised by our own writer.

// Without the `math` feature only the pre-scan and the Temml output are live; the filter and
// the engine guard stay compiled (and unit-tested) so both builds share one implementation.
#![cfg_attr(not(feature = "math"), allow(dead_code))]

use crate::html::{escape_attr, escape_text};

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

const DISABLED: &[&str] = &[
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
];

/// The §7.1 pre-scan: size, group depth and disabled control words. `true` if accepted.
///
/// Control sequences are tokenised left to right: `\` followed by letters is a control word;
/// `\` followed by any other character (including `\{`, `\}` and `\\`) is a two-character
/// control symbol that is neither a group nor the start of a control word.
pub fn prescan(tex: &str) -> bool {
    if tex.len() > 4096 {
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
        if !(0..=64).contains(&depth) {
            return false;
        }
    }
    true
}

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

/// The most unmatched opening delimiters an expression may have (see [`engine_guard`]).
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

/// `true` if `tex` is within the engine's nesting and delimiter limits (see
/// [`MAX_ENGINE_NESTING`]).
pub(crate) fn engine_guard(tex: &str) -> bool {
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
            return false;
        }
    }
    g.unmatched <= MAX_UNMATCHED_OPEN
}

/// The most MathML bytes one document may contain (presentation). Each expression is capped
/// at 65,536 bytes (§7.3); this caps their sum, so that a body of many large expressions cannot
/// expand without bound. Expressions past it render as math errors (§7.4). Real documents stay
/// far below it.
pub const MATHML_BUDGET: usize = 16 << 20;

/// Renders one math expression; returns the HTML fragment and whether it is a math error.
pub(crate) struct MathRenderer {
    mode: MathMode,
    /// MathML bytes still allowed in this document.
    mathml_left: usize,
    #[cfg(feature = "math")]
    engine: Option<math_core::LatexToMathML>,
}

impl MathRenderer {
    pub(crate) fn new(mode: MathMode) -> Self {
        Self {
            mode,
            mathml_left: MATHML_BUDGET,
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
            MathMode::MathCore => match self.engine_mathml(tex, display) {
                Some(html) if html.len() <= self.mathml_left => {
                    self.mathml_left -= html.len();
                    (html, false)
                }
                _ => (error_html(tex), true),
            },
        }
    }

    #[cfg(feature = "math")]
    fn engine_mathml(&mut self, tex: &str, display: bool) -> Option<String> {
        use math_core::{
            LatexToMathML, MathCoreConfig, MathDisplay, MaxExpansions, PrettyPrint,
            UnicodeSubstitution,
        };
        if !engine_guard(tex) {
            return None;
        }
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
        filter_mathml(&out.mathml, display)
    }

    #[cfg(not(feature = "math"))]
    fn engine_mathml(&mut self, _tex: &str, _display: bool) -> Option<String> {
        None
    }
}

/// `<code class="math-error">TEX</code>` (§7.4).
pub(crate) fn error_html(tex: &str) -> String {
    format!("<code class=\"math-error\">{}</code>", escape_text(tex))
}

// ---------------------------------------------------------------------------------------------
// MathML tree, filter and serialiser (§7.3)

#[derive(Debug)]
enum Node {
    Elem {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<usize>,
    },
    Text(String),
}

fn decode_entities(s: &str) -> Option<String> {
    if !s.contains('&') {
        return Some(s.to_owned());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let after = &rest[p + 1..];
        let semi = after.find(';')?;
        let ent = &after[..semi];
        let c = match ent {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{A0}',
            "ApplyFunction" | "af" => '\u{2061}',
            "InvisibleTimes" | "it" => '\u{2062}',
            "InvisibleComma" | "ic" => '\u{2063}',
            e if e.starts_with("#x") || e.starts_with("#X") => {
                char::from_u32(u32::from_str_radix(&e[2..], 16).ok()?)?
            }
            e if e.starts_with('#') => char::from_u32(e[1..].parse().ok()?)?,
            _ => return None,
        };
        out.push(c);
        rest = &after[semi + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Parse the engine's MathML (a single well-formed `math` element) into an arena. Iterative.
fn parse_tree(src: &str) -> Option<(Vec<Node>, usize)> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut root: Option<usize> = None;
    let b = src.as_bytes();
    let mut i = 0usize;
    let attach = |nodes: &mut Vec<Node>, stack: &Vec<usize>, id: usize| {
        if let Some(&p) = stack.last()
            && let Node::Elem { children, .. } = &mut nodes[p]
        {
            children.push(id);
        }
    };
    while i < b.len() {
        if b[i] == b'<' {
            let close = src[i..].find('>')? + i;
            let inner = &src[i + 1..close];
            if let Some(name) = inner.strip_prefix('/') {
                let open = stack.pop()?;
                match &nodes[open] {
                    Node::Elem { name: n, .. } if n == name.trim() => {}
                    _ => return None,
                }
            } else {
                let (inner, self_closing) = match inner.strip_suffix('/') {
                    Some(x) => (x, true),
                    None => (inner, false),
                };
                let name_end = inner
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(inner.len());
                let name = &inner[..name_end];
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b':')
                {
                    return None;
                }
                let attrs = parse_attrs(&inner[name_end..])?;
                let id = nodes.len();
                nodes.push(Node::Elem {
                    name: name.to_ascii_lowercase(),
                    attrs,
                    children: Vec::new(),
                });
                if stack.is_empty() {
                    if root.is_some() {
                        return None;
                    }
                    root = Some(id);
                } else {
                    attach(&mut nodes, &stack, id);
                }
                if !self_closing {
                    stack.push(id);
                }
            }
            i = close + 1;
        } else {
            let end = src[i..].find('<').map_or(b.len(), |p| p + i);
            let text = decode_entities(&src[i..end])?;
            if stack.is_empty() {
                if !text.trim().is_empty() {
                    return None;
                }
            } else {
                let id = nodes.len();
                nodes.push(Node::Text(text));
                attach(&mut nodes, &stack, id);
            }
            i = end;
        }
    }
    if !stack.is_empty() {
        return None;
    }
    Some((nodes, root?))
}

fn parse_attrs(mut s: &str) -> Option<Vec<(String, String)>> {
    // The value contains no '>' (the tag was cut at the first '>'), which the engine's
    // escaping guarantees; anything else fails closed.
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Some(out);
        }
        let eq = s.find('=')?;
        let name = s[..eq].trim();
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b':' | b'_'))
        {
            return None;
        }
        let rest = s[eq + 1..].trim_start();
        let q = rest.chars().next()?;
        if q != '"' && q != '\'' {
            return None;
        }
        let end = rest[1..].find(q)? + 1;
        let value = decode_entities(&rest[1..end])?;
        if !out
            .iter()
            .any(|(n, _): &(String, String)| n.eq_ignore_ascii_case(name))
        {
            out.push((name.to_ascii_lowercase(), value));
        }
        s = &rest[end + 1..];
    }
}

const ALLOWED: &[&str] = &[
    "math",
    "mrow",
    "mi",
    "mn",
    "mo",
    "ms",
    "mtext",
    "mspace",
    "msub",
    "msup",
    "msubsup",
    "munder",
    "mover",
    "munderover",
    "mmultiscripts",
    "mprescripts",
    "none",
    "mfrac",
    "msqrt",
    "mroot",
    "mtable",
    "mtr",
    "mtd",
    "mpadded",
    "mphantom",
    "mstyle",
    "merror",
    "menclose",
];

/// The MathML elements of §7.3.
pub const MATHML_ELEMENTS: &[&str] = ALLOWED;

/// The MathML attributes of §7.3 (names only; values are checked by [`mathml_attr_ok`]).
pub const MATHML_ATTRIBUTES: &[&str] = &[
    "display",
    "displaystyle",
    "fence",
    "separator",
    "stretchy",
    "symmetric",
    "largeop",
    "movablelimits",
    "accent",
    "accentunder",
    "scriptlevel",
    "mathvariant",
    "form",
    "lspace",
    "rspace",
    "minsize",
    "maxsize",
    "linethickness",
    "width",
    "height",
    "depth",
    "voffset",
    "columnspan",
    "rowspan",
    "notation",
];

const TOKEN: &[&str] = &["mi", "mn", "mo", "ms", "mtext"];
const EMPTY: &[&str] = &["mspace", "none", "mprescripts"];
const UNWRAP: &[&str] = &["a", "span", "b", "i", "em", "strong", "code", "s", "u"];
const DROP_SUBTREE: &[&str] = &["annotation", "annotation-xml", "maction"];

fn is_length(v: &str) -> bool {
    if v.is_empty() || v.len() > 32 {
        return false;
    }
    let b = v.as_bytes();
    let mut i = 0;
    if matches!(b[0], b'+' | b'-') {
        i = 1;
    }
    let num_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    let num = &v[num_start..i];
    // [0-9]*\.?[0-9]+
    let dots = num.matches('.').count();
    let num_ok = match dots {
        0 => !num.is_empty(),
        1 => !num.ends_with('.'),
        _ => false,
    };
    if !num_ok {
        return false;
    }
    let unit = &v[i..];
    unit.is_empty()
        || unit == "%"
        || ((1..=3).contains(&unit.len()) && unit.bytes().all(|c| c.is_ascii_lowercase()))
}

/// The §7.3 attribute table: whether `name="value"` may appear on `elem`.
pub fn mathml_attr_ok(elem: &str, name: &str, value: &str) -> bool {
    match name {
        "display" => elem == "math" && value == "block",
        "displaystyle" | "fence" | "separator" | "stretchy" | "symmetric" | "largeop"
        | "movablelimits" | "accent" | "accentunder" => value == "true" || value == "false",
        "scriptlevel" => {
            let d = value.strip_prefix(['+', '-']).unwrap_or(value);
            (1..=2).contains(&d.len()) && d.bytes().all(|c| c.is_ascii_digit())
        }
        "mathvariant" => value == "normal",
        "form" => elem == "mo" && matches!(value, "prefix" | "infix" | "postfix"),
        "lspace" | "rspace" | "minsize" | "maxsize" | "linethickness" | "width" | "height"
        | "depth" | "voffset" => is_length(value),
        "columnspan" | "rowspan" => {
            elem == "mtd"
                && (1..=3).contains(&value.len())
                && value.bytes().all(|c| c.is_ascii_digit())
                && !value.starts_with('0')
        }
        "notation" => {
            elem == "menclose"
                && (1..=128).contains(&value.len())
                && value.bytes().all(|c| c.is_ascii_lowercase() || c == b' ')
        }
        _ => false,
    }
}

/// Filter and re-serialise the engine's MathML. `None` is a math error (unparseable output,
/// depth over 64 or more than 65,536 bytes).
pub(crate) fn filter_mathml(src: &str, display: bool) -> Option<String> {
    let (nodes, root) = parse_tree(src)?;
    match &nodes[root] {
        Node::Elem { name, .. } if name == "math" => {}
        _ => return None,
    }
    enum Step {
        Enter(usize, Mode),
        Exit(&'static str),
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Normal,
        TextOnly,
    }
    let mut out = String::new();
    let mut depth = 0usize;
    let mut stack: Vec<Step> = vec![Step::Enter(root, Mode::Normal)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Exit(name) => {
                out.push_str("</");
                out.push_str(name);
                out.push('>');
                depth -= 1;
            }
            Step::Enter(id, mode) => match &nodes[id] {
                Node::Text(t) => {
                    if mode == Mode::TextOnly {
                        out.push_str(&escape_text(t));
                    }
                }
                Node::Elem {
                    name,
                    attrs,
                    children,
                } => {
                    let name = name.as_str();
                    if DROP_SUBTREE.contains(&name) {
                        continue;
                    }
                    if UNWRAP.contains(&name) {
                        for &c in children.iter().rev() {
                            stack.push(Step::Enter(c, mode));
                        }
                        continue;
                    }
                    if mode == Mode::TextOnly {
                        // Token elements hold text only (keeps the HTML parser's MathML text
                        // integration points inert).
                        continue;
                    }
                    if name == "semantics" {
                        if let Some(&first) = children
                            .iter()
                            .find(|&&c| matches!(nodes[c], Node::Elem { .. }))
                        {
                            stack.push(Step::Enter(first, Mode::Normal));
                        }
                        continue;
                    }
                    let Some(&static_name) = ALLOWED.iter().find(|&&a| a == name) else {
                        continue;
                    };
                    depth += 1;
                    if depth > 64 {
                        return None;
                    }
                    let mut kept: Vec<(&str, &str)> = if static_name == "math" {
                        if display {
                            vec![("display", "block")]
                        } else {
                            Vec::new()
                        }
                    } else {
                        attrs
                            .iter()
                            .filter(|(n, v)| n != "display" && mathml_attr_ok(static_name, n, v))
                            .map(|(n, v)| (n.as_str(), v.as_str()))
                            .collect()
                    };
                    kept.sort_by(|a, b| a.0.cmp(b.0));
                    out.push('<');
                    out.push_str(static_name);
                    for (n, v) in kept {
                        out.push(' ');
                        out.push_str(n);
                        out.push_str("=\"");
                        out.push_str(&escape_attr(v));
                        out.push('"');
                    }
                    out.push('>');
                    stack.push(Step::Exit(static_name));
                    if EMPTY.contains(&static_name) {
                        continue;
                    }
                    let child_mode = if TOKEN.contains(&static_name) {
                        Mode::TextOnly
                    } else {
                        Mode::Normal
                    };
                    for &c in children.iter().rev() {
                        stack.push(Step::Enter(c, child_mode));
                    }
                }
            },
        }
        if out.len() > 65_536 {
            return None;
        }
    }
    Some(out)
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

    #[test]
    fn filter_basic() {
        let out = filter_mathml(
            "<math display=\"block\" class=\"x\"><semantics><mrow style=\"color:red\" id=\"q\"><mi mathvariant=\"normal\">d</mi><mspace width=\"1em\"/><mtext><a href=\"javascript:x\"><b>t&amp;</b></a></mtext></mrow><annotation encoding=\"x\">src</annotation></semantics></math>",
            true,
        )
        .unwrap();
        assert_eq!(
            out,
            "<math display=\"block\"><mrow><mi mathvariant=\"normal\">d</mi><mspace width=\"1em\"></mspace><mtext>t&amp;</mtext></mrow></math>"
        );
        assert!(filter_mathml("<math><mi>x</mi>", false).is_none());
        assert!(filter_mathml("<math><mo>&bogus;</mo></math>", false).is_none());
    }

    #[test]
    fn lengths() {
        for ok in ["0", "1.5em", "-0.1667em", ".5ex", "100%", "+3px"] {
            assert!(is_length(ok), "{ok}");
        }
        for bad in ["", "1.", "em", "1.2.3", "1emxx", "1 em", "1EM"] {
            assert!(!is_length(bad), "{bad}");
        }
    }
}
