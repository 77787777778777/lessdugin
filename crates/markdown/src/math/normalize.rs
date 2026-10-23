// SPDX-License-Identifier: AGPL-3.0-or-later
//! The MathML normaliser (`docs/spec/lfm-1.md` §7.3, presentation; ADR 0002 decision 2).
//!
//! math-core's output is tokenised and parsed into a tree (iteratively), then re-serialised by
//! our own writer:
//!
//! - every element and attribute outside the §7.3 allowlist is removed;
//! - `style` declarations become class tokens from a closed table ([`STYLE_CLASSES`], styled by
//!   `assets/mathml.css`) or, for `color`, the MathML Core attribute `mathcolor`; anything else
//!   is dropped (fail closed). The output never has a `style` attribute, so it renders under
//!   CSP `style-src 'self'`;
//! - `id` and `href` (from `\label` and `\eqref`, which the pre-scan rejects anyway) are dropped;
//! - the HTML text-style wrappers inside `mtext` are unwrapped and become classes of the `mtext`
//!   ([`TEXT_STYLE_CLASSES`]);
//! - every element gets an explicit end tag (no self-closing forms) and attributes are sorted.
//!
//! The result passes `DOMPurify(html) === html` and `ammonia(html) == html` under the §8.2/§7.3
//! allowlist.

use crate::html::{escape_attr, escape_text};

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

// ---------------------------------------------------------------------------------------------
// Allowlists (§7.3)

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
    "class",
    "mathcolor",
];

/// The style normalisation of §7.3: each `style` declaration math-core 0.9.0 emits, written
/// `property:value` (whitespace around `:` removed, runs of whitespace in the value collapsed
/// to one space), and the class token that replaces it. Several declarations that math-core
/// always emits together map to one token.
pub const STYLE_CLASSES: &[(&str, &str)] = &[
    ("text-align:left", "mc-al"),
    ("justify-items:start", "mc-al"),
    ("text-align:right", "mc-ar"),
    ("justify-items:end", "mc-ar"),
    ("padding-left:0", "mc-pl0"),
    ("padding-right:0", "mc-pr0"),
    ("padding-top:0", "mc-py0"),
    ("padding-bottom:0", "mc-py0"),
    ("padding-left:0.1em", "mc-px01"),
    ("padding-right:0.1em", "mc-px01"),
    ("padding-left:1em", "mc-pl1"),
    ("padding-left:0.1667em", "mc-pl3mu"),
    ("padding-left:0.2222em", "mc-pl4mu"),
    ("padding-left:0.2778em", "mc-pl5mu"),
    ("padding-right:0.1667em", "mc-pr3mu"),
    ("padding-right:0.2222em", "mc-pr4mu"),
    ("padding-right:0.2778em", "mc-pr5mu"),
    ("padding-top:0.1667em", "mc-pt3mu"),
    ("width:7.5%", "mc-w7-5"),
    ("width:50%", "mc-w50"),
    ("width:100%", "mc-w100"),
    ("border-top:0.05em solid currentcolor", "mc-bt-solid"),
    ("border-top:0.05em dashed currentcolor", "mc-bt-dashed"),
    ("border-right:0.05em solid currentcolor", "mc-br-solid"),
    ("border-right:0.05em dashed currentcolor", "mc-br-dashed"),
    ("border-left:0.05em solid currentcolor", "mc-bl-solid"),
    ("border-left:0.05em dashed currentcolor", "mc-bl-dashed"),
    ("margin-left:-0.16666667em", "mc-negthin"),
    ("margin-left:-0.22222222em", "mc-negmed"),
    ("margin-left:-0.2777778em", "mc-negthick"),
    ("math-shift:compact", "mc-cramped"),
    ("font-variant-caps:small-caps", "mc-tsc"),
    ("font-size:50%", "mc-fs50"),
    ("font-size:60%", "mc-fs60"),
    ("font-size:70%", "mc-fs70"),
    ("font-size:80%", "mc-fs80"),
    ("font-size:90%", "mc-fs90"),
    ("font-size:100%", "mc-fs100"),
    ("font-size:120%", "mc-fs120"),
    ("font-size:140%", "mc-fs140"),
    ("font-size:170%", "mc-fs170"),
    ("font-size:200%", "mc-fs200"),
    ("font-size:250%", "mc-fs250"),
];

/// The HTML text-style wrappers math-core 0.9.0 emits inside `mtext` (`\textbf`, `\textit`,
/// `\emph`, `\texttt`, `\sout`, `\underline`, `\textsf`, `\textrm`) and the class token the
/// `mtext` gets for each (§7.3). `span` is identified by its own `class` (or by the small-caps
/// `style`, through [`STYLE_CLASSES`]).
pub const TEXT_STYLE_CLASSES: &[(&str, &str)] = &[
    ("b", "mc-tb"),
    ("strong", "mc-tb"),
    ("i", "mc-ti"),
    ("em", "mc-ti"),
    ("code", "mc-tt"),
    ("s", "mc-ts"),
    ("u", "mc-tu"),
    ("span.math-core-sans-serif-font", "mc-tsf"),
    ("span.math-core-serif-font", "mc-trm"),
];

/// The class tokens math-core 0.9.0 emits itself that are kept as they are (the `menclose`
/// polyfill of math-core's `mathmlfixes.css`).
pub const ENGINE_CLASSES: &[&str] = &[
    "menclose-downdiagonalstrike",
    "menclose-phasorangle",
    "menclose-updiagonalstrike",
];

/// Every class token a MathML element may carry (§7.3), sorted.
pub const MATHML_CLASSES: &[&str] = &[
    "mc-al",
    "mc-ar",
    "mc-bl-dashed",
    "mc-bl-solid",
    "mc-br-dashed",
    "mc-br-solid",
    "mc-bt-dashed",
    "mc-bt-solid",
    "mc-cramped",
    "mc-fs100",
    "mc-fs120",
    "mc-fs140",
    "mc-fs170",
    "mc-fs200",
    "mc-fs250",
    "mc-fs50",
    "mc-fs60",
    "mc-fs70",
    "mc-fs80",
    "mc-fs90",
    "mc-negmed",
    "mc-negthick",
    "mc-negthin",
    "mc-pl0",
    "mc-pl1",
    "mc-pl3mu",
    "mc-pl4mu",
    "mc-pl5mu",
    "mc-pr0",
    "mc-pr3mu",
    "mc-pr4mu",
    "mc-pr5mu",
    "mc-pt3mu",
    "mc-px01",
    "mc-py0",
    "mc-tb",
    "mc-ti",
    "mc-trm",
    "mc-ts",
    "mc-tsc",
    "mc-tsf",
    "mc-tt",
    "mc-tu",
    "mc-w100",
    "mc-w50",
    "mc-w7-5",
    "menclose-downdiagonalstrike",
    "menclose-phasorangle",
    "menclose-updiagonalstrike",
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

/// `#` followed by six lower-case hexadecimal digits.
fn is_color(v: &str) -> bool {
    v.len() == 7
        && v.starts_with('#')
        && v[1..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A `class` value: one or more tokens of [`MATHML_CLASSES`], in ascending order, without
/// duplicates, separated by one space.
fn is_class_list(v: &str) -> bool {
    let mut prev: Option<&str> = None;
    for t in v.split(' ') {
        if !MATHML_CLASSES.contains(&t) || prev.is_some_and(|p| p >= t) {
            return false;
        }
        prev = Some(t);
    }
    prev.is_some()
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
        "class" => is_class_list(value),
        "mathcolor" => is_color(value),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Style normalisation (§7.3, ADR 0002 decision 2)

/// What the normaliser makes of one element's `style` and `class` (and, for `mtext`, of its
/// text-style wrappers).
#[derive(Default)]
struct Presentation {
    classes: Vec<&'static str>,
    color: Option<String>,
}

fn style_class(decl: &str) -> Option<&'static str> {
    STYLE_CLASSES
        .iter()
        .find(|(d, _)| *d == decl)
        .map(|(_, c)| *c)
}

/// `property:value` with the whitespace normalised as in [`STYLE_CLASSES`].
pub(crate) fn normalise_declaration(raw: &str) -> Option<String> {
    let (p, v) = raw.split_once(':')?;
    let v: Vec<&str> = v.split_ascii_whitespace().collect();
    Some(format!("{}:{}", p.trim(), v.join(" ")))
}

impl Presentation {
    /// One `style` attribute. Known declarations become classes or `mathcolor`; the two
    /// declarations math-core emits that have no class are dropped as known (a `width` on
    /// `mspace` equal to its `width` attribute, the Firefox `rem` work-around, and any other
    /// negative `margin-left` on `mspace`); everything else is dropped and reported in
    /// `unknown` (fail closed).
    fn style(
        &mut self,
        elem: &str,
        style: &str,
        width_attr: Option<&str>,
        unknown: &mut Vec<String>,
    ) {
        for raw in style.split(';') {
            if raw.trim().is_empty() {
                continue;
            }
            let Some(decl) = normalise_declaration(raw) else {
                unknown.push(format!("{elem} style {raw:?}"));
                continue;
            };
            if let Some(c) = style_class(&decl) {
                self.classes.push(c);
            } else if let Some(hex) = decl.strip_prefix("color:#") {
                let color = format!("#{}", hex.to_ascii_lowercase());
                if is_color(&color) {
                    self.color = Some(color);
                } else {
                    unknown.push(format!("{elem} style {decl:?}"));
                }
            } else if elem == "mspace"
                && (decl
                    .strip_prefix("width:")
                    .is_some_and(|w| Some(w) == width_attr)
                    || decl
                        .strip_prefix("margin-left:-")
                        .is_some_and(|l| is_length(l) && l != "0"))
            {
                // known, no class
            } else {
                unknown.push(format!("{elem} style {decl:?}"));
            }
        }
    }

    /// The engine's own `class` attribute: only [`ENGINE_CLASSES`] are kept.
    fn engine_class(&mut self, elem: &str, class: &str, unknown: &mut Vec<String>) {
        for t in class.split_ascii_whitespace() {
            match ENGINE_CLASSES.iter().find(|c| **c == t) {
                Some(c) => self.classes.push(c),
                None => unknown.push(format!("{elem} class {t:?}")),
            }
        }
    }

    /// One text-style wrapper inside `mtext` (`b`, `i`, `span class=…`, …).
    fn wrapper(&mut self, name: &str, attrs: &[(String, String)], unknown: &mut Vec<String>) {
        let get = |a: &str| attrs.iter().find(|(n, _)| n == a).map(|(_, v)| v.as_str());
        let key = match (name, get("class")) {
            ("span", Some(c)) => format!("span.{c}"),
            (n, _) => n.to_owned(),
        };
        if let Some((_, c)) = TEXT_STYLE_CLASSES.iter().find(|(k, _)| *k == key) {
            self.classes.push(c);
        } else if name == "span" && get("class").is_some() {
            unknown.push(format!("mtext {key}"));
        }
        if let Some(s) = get("style") {
            self.style(name, s, None, unknown);
        }
    }

    /// The `class` value: sorted, without duplicates, space-separated.
    fn class_value(&mut self) -> Option<String> {
        self.classes.sort_unstable();
        self.classes.dedup();
        (!self.classes.is_empty()).then(|| self.classes.join(" "))
    }
}

/// For a token element: the text-style wrappers when its content is exactly one chain of them
/// around text (the only shape math-core 0.9.0 emits). Other shapes get no classes; their
/// wrappers are unwrapped all the same.
fn text_wrappers(nodes: &[Node], id: usize, p: &mut Presentation, unknown: &mut Vec<String>) {
    let mut cur = id;
    loop {
        let Node::Elem { children, .. } = &nodes[cur] else {
            return;
        };
        let [only] = children.as_slice() else {
            return;
        };
        match &nodes[*only] {
            Node::Elem { name, attrs, .. } if UNWRAP.contains(&name.as_str()) => {
                p.wrapper(name, attrs, unknown);
                cur = *only;
            }
            _ => return,
        }
    }
}

/// Normalise, filter and re-serialise the engine's MathML (§7.3). `None` is a math error
/// (unparseable output, depth over 64 or more than 65,536 bytes). Style declarations and class
/// tokens the normaliser does not know are dropped and appended to `unknown`.
pub(crate) fn normalise_mathml(
    src: &str,
    display: bool,
    unknown: &mut Vec<String>,
) -> Option<String> {
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
                    let get = |a: &str| attrs.iter().find(|(n, _)| n == a).map(|(_, v)| v.as_str());
                    let mut p = Presentation::default();
                    if let Some(s) = get("style") {
                        p.style(static_name, s, get("width"), unknown);
                    }
                    if let Some(c) = get("class") {
                        p.engine_class(static_name, c, unknown);
                    }
                    if TOKEN.contains(&static_name) {
                        text_wrappers(&nodes, id, &mut p, unknown);
                    }
                    let class = p.class_value();
                    let mut kept: Vec<(&str, &str)> = if static_name == "math" {
                        if display {
                            vec![("display", "block")]
                        } else {
                            Vec::new()
                        }
                    } else {
                        attrs
                            .iter()
                            .filter(|(n, v)| {
                                !matches!(n.as_str(), "display" | "class" | "mathcolor" | "style")
                                    && mathml_attr_ok(static_name, n, v)
                            })
                            .map(|(n, v)| (n.as_str(), v.as_str()))
                            .collect()
                    };
                    if let Some(c) = &class {
                        kept.push(("class", c));
                    }
                    if let Some(c) = &p.color {
                        kept.push(("mathcolor", c));
                    }
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

/// [`normalise_mathml`] without the report.
#[cfg(test)]
fn filter_mathml(src: &str, display: bool) -> Option<String> {
    normalise_mathml(src, display, &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_basic() {
        let out = filter_mathml(
            "<math display=\"block\" class=\"x\"><semantics><mrow style=\"color:red\" id=\"q\"><mi mathvariant=\"normal\">d</mi><mspace width=\"1em\"/><mtext><a href=\"javascript:x\"><b>t&amp;</b></a></mtext></mrow><annotation encoding=\"x\">src</annotation></semantics></math>",
            true,
        )
        .unwrap();
        assert_eq!(
            out,
            "<math display=\"block\"><mrow><mi mathvariant=\"normal\">d</mi><mspace width=\"1em\"></mspace><mtext class=\"mc-tb\">t&amp;</mtext></mrow></math>"
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

    /// ADR 0002 decision 2: styles become classes or `mathcolor`, `id` and `href` go, nothing
    /// is self-closing, unknown declarations are dropped and reported.
    #[test]
    fn normaliser() {
        let mut unknown = Vec::new();
        let out = normalise_mathml(
            "<math><mtable displaystyle=\"true\" scriptlevel=\"0\" style=\"width: 100%\"><mtr><mtd style=\"width: 50%\"></mtd><mtd style=\"text-align: left;justify-items: start;padding-right: 0;padding-left:1em;\" id=\"eq:1\"><mi>x</mi></mtd></mtr></mtable><mrow style=\"color:#FF0000;\"><mi>y</mi></mrow><mspace style=\"margin-left:-0.16666667em;\"/><mspace width=\"0.2rem\" style=\"width:0.2rem\"/><mspace style=\"margin-left:-1em;\"/><mrow class=\"menclose-updiagonalstrike evil\"></mrow><mtext style=\"font-size:120%\"><span style=\"font-variant-caps: small-caps\">s</span></mtext><mtext><span class=\"math-core-sans-serif-font\">f</span></mtext><mtext><a href=\"#x\">(1)</a></mtext><mrow style=\"position:fixed;color:#12345\"></mrow></math>",
            false,
            &mut unknown,
        )
        .unwrap();
        assert_eq!(
            out,
            "<math><mtable class=\"mc-w100\" displaystyle=\"true\" scriptlevel=\"0\"><mtr><mtd class=\"mc-w50\"></mtd><mtd class=\"mc-al mc-pl1 mc-pr0\"><mi>x</mi></mtd></mtr></mtable><mrow mathcolor=\"#ff0000\"><mi>y</mi></mrow><mspace class=\"mc-negthin\"></mspace><mspace width=\"0.2rem\"></mspace><mspace></mspace><mrow class=\"menclose-updiagonalstrike\"></mrow><mtext class=\"mc-fs120 mc-tsc\">s</mtext><mtext class=\"mc-tsf\">f</mtext><mtext>(1)</mtext><mrow></mrow></math>"
        );
        assert_eq!(
            unknown,
            [
                "mrow class \"evil\"",
                "mrow style \"position:fixed\"",
                "mrow style \"color:#12345\""
            ]
        );
    }

    #[test]
    fn class_and_color_values() {
        assert!(mathml_attr_ok("mtd", "class", "mc-al mc-pl1 mc-pr0"));
        assert!(mathml_attr_ok("mrow", "class", "menclose-updiagonalstrike"));
        for bad in [
            "",
            " mc-al",
            "mc-al ",
            "mc-al  mc-ar",
            "mc-ar mc-al",
            "mc-al mc-al",
            "mc-evil",
            "math-core-serif-font",
        ] {
            assert!(!mathml_attr_ok("mtd", "class", bad), "{bad:?}");
        }
        assert!(mathml_attr_ok("mrow", "mathcolor", "#0a1b2c"));
        for bad in ["#0A1B2C", "#fff", "red", "#0a1b2cc", "#0a1b2g", ""] {
            assert!(!mathml_attr_ok("mrow", "mathcolor", bad), "{bad:?}");
        }
    }

    #[test]
    fn class_tables_are_consistent() {
        let mut sorted = MATHML_CLASSES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, MATHML_CLASSES,
            "MATHML_CLASSES must be sorted and unique"
        );
        let mut used: Vec<&str> = STYLE_CLASSES
            .iter()
            .chain(TEXT_STYLE_CLASSES)
            .map(|(_, c)| *c)
            .chain(ENGINE_CLASSES.iter().copied())
            .collect();
        used.sort_unstable();
        used.dedup();
        assert_eq!(used, MATHML_CLASSES, "every class token comes from a table");
        for (d, _) in STYLE_CLASSES {
            assert_eq!(normalise_declaration(d).as_deref(), Some(*d));
        }
    }
}
