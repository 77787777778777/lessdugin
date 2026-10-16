// SPDX-License-Identifier: AGPL-3.0-or-later
//! The §8.2/§7.3 allowlist as data (the source for the DOMPurify and ammonia configurations)
//! and an independent checker that tokenises renderer output and verifies every element,
//! attribute, attribute order, class token and value grammar (`docs/spec/lfm-1.md` §8.2).

use crate::html::EXTERNAL_REL;
use crate::math::{MATHML_ATTRIBUTES, MATHML_ELEMENTS, mathml_attr_ok};
use crate::plain::is_letter_or_number;

/// HTML elements and their attributes, in serialisation order (§8.2).
pub const HTML_ALLOWLIST: &[(&str, &[&str])] = &[
    ("div", &["class", "dir"]),
    ("p", &["class", "dir"]),
    ("h1", &["id", "dir"]),
    ("h2", &["id", "dir"]),
    ("h3", &["id", "dir"]),
    ("h4", &["id", "dir"]),
    ("h5", &["id", "dir"]),
    ("h6", &["id", "dir"]),
    ("li", &["id", "dir"]),
    ("ol", &["start"]),
    ("blockquote", &[]),
    ("ul", &[]),
    ("table", &[]),
    ("thead", &[]),
    ("tbody", &[]),
    ("tr", &[]),
    ("figure", &[]),
    ("pre", &[]),
    ("em", &[]),
    ("strong", &[]),
    ("del", &[]),
    ("hr", &[]),
    ("br", &[]),
    ("th", &["class", "dir"]),
    ("td", &["class", "dir"]),
    ("input", &["type", "checked", "disabled"]),
    ("code", &["class"]),
    ("sup", &["id"]),
    (
        "a",
        &[
            "href",
            "data-at-uri",
            "data-did",
            "data-anchor",
            "class",
            "rel",
        ],
    ),
    ("details", &["class"]),
    ("summary", &["dir"]),
    ("figcaption", &["dir"]),
    ("img", &["data-cid", "alt", "width", "height"]),
    ("span", &["class", "data-cid", "data-tex"]),
    ("section", &["class"]),
];

/// Class tokens per element (§8.2). `code` also allows `language-[a-z0-9_+-]{1,32}`.
pub const CLASS_ALLOWLIST: &[(&str, &[&str])] = &[
    (
        "div",
        &[
            "lfm",
            "alert-note",
            "alert-tip",
            "alert-important",
            "alert-warning",
            "alert-caution",
            "gallery",
        ],
    ),
    ("p", &["notice"]),
    ("th", &["ta-l", "ta-c", "ta-r"]),
    ("td", &["ta-l", "ta-c", "ta-r"]),
    ("code", &["math-error"]),
    ("a", &["at-link", "mention", "ext-image", "fn-back"]),
    ("details", &["spoiler"]),
    ("span", &["media", "media-missing", "link-host"]),
    ("section", &["footnotes"]),
];

/// MathML elements (§7.3).
pub const MATHML_ALLOWLIST: &[&str] = MATHML_ELEMENTS;
/// MathML attributes (§7.3).
pub const MATHML_ATTR_ALLOWLIST: &[&str] = MATHML_ATTRIBUTES;

/// Elements and attributes that must never appear (§8.2).
pub const NEVER_EMITTED: &[&str] = &[
    "svg", "iframe", "video", "audio", "source", "object", "embed", "script", "style", "template",
    "base", "meta", "link", "mark",
];

const VOID: &[&str] = &["br", "hr", "img", "input"];

fn decode(s: &str, attr: bool) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find(['&', '<', '>', '"']) {
        out.push_str(&rest[..p]);
        let c = rest.as_bytes()[p];
        if c != b'&' {
            if c == b'"' && !attr {
                out.push('"');
                rest = &rest[p + 1..];
                continue;
            }
            return Err(format!("unescaped {:?}", c as char));
        }
        let after = &rest[p..];
        let (ent, ch) = [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&nbsp;", '\u{A0}'),
        ]
        .into_iter()
        .find(|(e, _)| after.starts_with(e))
        .ok_or_else(|| format!("unexpected entity at {:?}", &after[..after.len().min(12)]))?;
        if ent == "&quot;" && !attr {
            return Err("&quot; in text".into());
        }
        out.push(ch);
        rest = &rest[p + ent.len()..];
    }
    out.push_str(rest);
    if out.contains('\u{A0}') && !s.contains("&nbsp;") {
        return Err("raw U+00A0".into());
    }
    Ok(out)
}

struct TagTok<'a> {
    name: &'a str,
    end: bool,
    attrs: Vec<(&'a str, String)>,
}

fn parse_tag(t: &str) -> Result<TagTok<'_>, String> {
    if let Some(n) = t.strip_prefix('/') {
        if !n
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(format!("bad end tag {t:?}"));
        }
        return Ok(TagTok {
            name: n,
            end: true,
            attrs: Vec::new(),
        });
    }
    let name_end = t.find(' ').unwrap_or(t.len());
    let name = &t[..name_end];
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err(format!("bad tag name {name:?}"));
    }
    let mut attrs = Vec::new();
    let mut rest = &t[name_end..];
    while !rest.is_empty() {
        let r = rest
            .strip_prefix(' ')
            .ok_or_else(|| format!("bad attribute syntax in {t:?}"))?;
        let eq = r
            .find("=\"")
            .ok_or_else(|| format!("attribute without quoted value in {t:?}"))?;
        let an = &r[..eq];
        if an.is_empty() || !an.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
            return Err(format!("bad attribute name {an:?}"));
        }
        let vr = &r[eq + 2..];
        let close = vr.find('"').ok_or("unterminated attribute")?;
        attrs.push((an, decode(&vr[..close], true)?));
        rest = &vr[close + 1..];
    }
    Ok(TagTok {
        name,
        end: false,
        attrs,
    })
}

fn is_digits(s: &str, max: usize, allow_zero_lead: bool) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().all(|b| b.is_ascii_digit())
        && (allow_zero_lead || !s.starts_with('0'))
}

fn ln_dash(s: &str) -> bool {
    s.chars()
        .all(|c| c == '-' || c == '_' || is_letter_or_number(c))
}

fn check_html_attrs(tag: &TagTok<'_>, parent: Option<&str>, is_root: bool) -> Result<(), String> {
    let name = tag.name;
    let allowed = HTML_ALLOWLIST
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .ok_or_else(|| format!("element <{name}> not allowed"))?;
    // Order: a subsequence of the allowlist order; no duplicates.
    let mut pos = 0usize;
    for (an, _) in &tag.attrs {
        let k = allowed[pos..]
            .iter()
            .position(|a| a == an)
            .ok_or_else(|| format!("attribute {an} not allowed (or out of order) on <{name}>"))?;
        pos += k + 1;
    }
    let get = |a: &str| attr(tag, a);
    if let Some(c) = get("class") {
        let ok = CLASS_ALLOWLIST
            .iter()
            .find(|(n, _)| *n == name)
            .is_some_and(|(_, cs)| cs.contains(&c))
            || (name == "code"
                && parent == Some("pre")
                && c.strip_prefix("language-").is_some_and(|l| {
                    (1..=32).contains(&l.len())
                        && l.bytes()
                            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'+' | b'-'))
                }));
        if !ok {
            return Err(format!("class {c:?} not allowed on <{name}>"));
        }
    }
    let dir = get("dir");
    match name {
        "div" => {
            let c = get("class").ok_or("div without class")?;
            if (c == "lfm") != is_root || (c == "lfm") != (dir == Some("auto")) {
                return Err("div.lfm must be the root and carry dir=auto (only it)".into());
            }
        }
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "th" | "td" | "summary"
        | "figcaption"
            if dir != Some("auto") =>
        {
            return Err(format!("<{name}> needs dir=\"auto\""));
        }
        _ => {}
    }
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let id = get("id").ok_or("heading without id")?;
            if !id.strip_prefix("md-h-").is_some_and(ln_dash) {
                return Err(format!("bad heading id {id:?}"));
            }
        }
        "li" => {
            if let Some(id) = get("id")
                && !id
                    .strip_prefix("md-fn-")
                    .is_some_and(|n| is_digits(n, 10, false))
            {
                return Err(format!("bad li id {id:?}"));
            }
        }
        "ol" => {
            if let Some(s) = get("start")
                && s != "0"
                && !is_digits(s, 10, false)
            {
                return Err(format!("bad ol start {s:?}"));
            }
        }
        "input" => {
            if get("type") != Some("checkbox")
                || get("disabled") != Some("")
                || get("checked").is_some_and(|v| !v.is_empty())
            {
                return Err("bad checkbox".into());
            }
        }
        "sup" => {
            let id = get("id").ok_or("sup without id")?;
            let ok = id
                .strip_prefix("md-fnref-")
                .and_then(|r| r.split_once('-'))
                .is_some_and(|(n, k)| is_digits(n, 10, false) && is_digits(k, 10, false));
            if !ok {
                return Err(format!("bad sup id {id:?}"));
            }
        }
        "a" => check_a(tag)?,
        "img" => {
            let cid = get("data-cid").ok_or("img without data-cid")?;
            if !crate::url::is_valid_cid(cid) || get("alt").is_none() {
                return Err("bad img".into());
            }
            for d in ["width", "height"] {
                if let Some(v) = get(d)
                    && !is_digits(v, 8, false)
                {
                    return Err(format!("bad img {d}"));
                }
            }
            if get("width").is_some() != get("height").is_some() {
                return Err("img width without height".into());
            }
        }
        "span" => {
            let class = get("class");
            let cid = get("data-cid");
            let tex = get("data-tex");
            let ok = match (class, cid, tex) {
                (Some("media"), Some(c), None) => crate::url::is_valid_cid(c),
                (Some("media-missing" | "link-host"), None, None) => true,
                (None, None, Some("inline" | "block")) => true,
                _ => false,
            };
            if !ok {
                return Err("bad span".into());
            }
            if class == Some("link-host") && parent != Some("a") {
                return Err("span.link-host outside a".into());
            }
        }
        "section" if get("class") != Some("footnotes") => return Err("bad section".into()),
        _ => {}
    }
    Ok(())
}

fn attr<'t>(tag: &'t TagTok<'_>, name: &str) -> Option<&'t str> {
    tag.attrs
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.as_str())
}

fn check_a(tag: &TagTok<'_>) -> Result<(), String> {
    let get = |a: &str| attr(tag, a);
    let href = get("href");
    let uri = get("data-at-uri");
    let did = get("data-did");
    let anchor = get("data-anchor");
    let class = get("class");
    let rel = get("rel");
    let ok = match (href, uri, did, anchor) {
        (Some(h), None, None, None)
            if h.starts_with("https://")
                || h.starts_with("http://")
                || h.starts_with("mailto:") =>
        {
            rel == Some(EXTERNAL_REL) && matches!(class, None | Some("ext-image"))
        }
        (Some(h), Some(u), None, None) => {
            h.strip_prefix("#/at/")
                .is_some_and(|r| Some(r) == u.strip_prefix("at://"))
                && class == Some("at-link")
                && rel.is_none()
        }
        (Some(h), None, Some(d), None) => {
            h.strip_prefix("#/u/") == Some(d)
                && matches!(class, Some("at-link" | "mention"))
                && rel.is_none()
        }
        (None, None, None, Some(v)) => {
            !v.is_empty()
                && v.len() <= 512
                && ln_dash(v)
                && matches!(class, None | Some("fn-back"))
                && rel.is_none()
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("bad <a> shape: href={href:?} class={class:?}"))
    }
}

/// Verify that `html` is renderer output that only uses the §8.2 and §7.3 allowlists, with
/// balanced tags, no nested `a`, and MathML only inside `math`.
pub fn check_html(html: &str) -> Result<(), String> {
    let mut stack: Vec<&str> = Vec::new();
    let mut seen_root = false;
    let mut rest = html;
    let mut a_depth = 0usize;
    let mut math_depth = 0usize;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            decode(rest, false)?;
            if !rest.chars().all(|c| c == '\n') {
                return Err("text after the root".into());
            }
            break;
        };
        let text = &rest[..lt];
        decode(text, false)?;
        if stack.is_empty() && !text.chars().all(|c| c == '\n') {
            return Err("text outside the root".into());
        }
        let gt = rest[lt..].find('>').ok_or("unterminated tag")? + lt;
        let tag = parse_tag(&rest[lt + 1..gt])?;
        rest = &rest[gt + 1..];
        if NEVER_EMITTED.contains(&tag.name) {
            return Err(format!("forbidden element {}", tag.name));
        }
        if tag.end {
            let open = stack
                .pop()
                .ok_or_else(|| format!("stray </{}>", tag.name))?;
            if open != tag.name {
                return Err(format!("</{}> closes <{open}>", tag.name));
            }
            if tag.name == "a" {
                a_depth -= 1;
            }
            math_depth = math_depth.saturating_sub(1);
            continue;
        }
        let parent = stack.last().copied();
        if math_depth > 0 || tag.name == "math" {
            if !MATHML_ELEMENTS.contains(&tag.name) {
                return Err(format!("<{}> not allowed in MathML", tag.name));
            }
            if tag.name == "math" && math_depth > 0 {
                return Err("nested math".into());
            }
            let mut prev: Option<&str> = None;
            for (an, v) in &tag.attrs {
                if !mathml_attr_ok(tag.name, an, v) {
                    return Err(format!(
                        "MathML attribute {an}={v:?} not allowed on <{}>",
                        tag.name
                    ));
                }
                if prev.is_some_and(|p| p >= *an) {
                    return Err("MathML attributes not sorted".into());
                }
                prev = Some(an);
            }
            math_depth += 1;
            if math_depth > 64 {
                return Err("MathML deeper than 64".into());
            }
            stack.push(tag.name);
            continue;
        }
        let is_root = stack.is_empty();
        if is_root {
            if seen_root {
                return Err("more than one root".into());
            }
            seen_root = true;
        }
        check_html_attrs(&tag, parent, is_root)?;
        if tag.name == "a" {
            if a_depth > 0 {
                return Err("nested <a>".into());
            }
            a_depth += 1;
        }
        if !VOID.contains(&tag.name) {
            stack.push(tag.name);
        }
    }
    if !stack.is_empty() {
        return Err(format!("unclosed <{}>", stack.join("><")));
    }
    if !seen_root {
        return Err("no root".into());
    }
    Ok(())
}

/// The `ALLOWED_URI_REGEXP` of the DOMPurify configuration (§8.5).
pub const ALLOWED_URI_REGEXP: &str = "^(?:https?:|mailto:|#/at/|#/u/)";

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let v: Vec<String> = items.map(json_str).collect();
    format!("[{}]", v.join(","))
}

/// The DOMPurify configuration generated from §8.2 and §7.3 (§8.5), as JSON, usable as is:
///
/// - `ALLOWED_TAGS` and `ALLOWED_ATTR` exactly as listed, `ALLOW_DATA_ATTR: false`,
///   `ALLOW_ARIA_ATTR: false` and `ALLOWED_URI_REGEXP`;
/// - `ADD_URI_SAFE_ATTR`: every allowed attribute except `href`, which is the only URL-valued
///   one. Without it DOMPurify tests other values (`dir="auto"`, `rel`, `data-cid`, …) against
///   the URI pattern and drops them;
/// - `SAFE_FOR_XML: false`: that mode drops any attribute whose value merely contains
///   `</script`, `-->` and similar text (an `img` `alt` may), and the output is only ever
///   parsed as HTML, with no raw-text element or comment allowed;
/// - `LFM_CLASSES` (class tokens per element), for the hook that enforces class tokens and
///   value grammars; strip it before passing the object to DOMPurify.
///
/// Deterministic (table order).
pub fn dompurify_config_json() -> String {
    let mut tags: Vec<&str> = HTML_ALLOWLIST.iter().map(|(t, _)| *t).collect();
    tags.extend(MATHML_ELEMENTS.iter().copied());
    let mut attrs: Vec<&str> = Vec::new();
    for a in HTML_ALLOWLIST
        .iter()
        .flat_map(|(_, a)| a.iter())
        .chain(MATHML_ATTRIBUTES.iter())
    {
        if !attrs.contains(a) {
            attrs.push(a);
        }
    }
    let classes: Vec<String> = CLASS_ALLOWLIST
        .iter()
        .map(|(t, cs)| format!("{}:{}", json_str(t), json_list(cs.iter().copied())))
        .collect();
    let uri_safe: Vec<&str> = attrs.iter().copied().filter(|a| *a != "href").collect();
    format!(
        "{{\"ALLOWED_TAGS\":{},\"ALLOWED_ATTR\":{},\"ALLOW_DATA_ATTR\":false,\"ALLOW_ARIA_ATTR\":false,\"ADD_URI_SAFE_ATTR\":{},\"SAFE_FOR_XML\":false,\"ALLOWED_URI_REGEXP\":{},\"LFM_CLASSES\":{{{}}}}}",
        json_list(tags.into_iter()),
        json_list(attrs.iter().copied()),
        json_list(uri_safe.into_iter()),
        json_str(ALLOWED_URI_REGEXP),
        classes.join(",")
    )
}

/// ammonia's relative-URL rule: keep `#/at/…` and `#/u/…`, drop everything else.
#[cfg(feature = "ammonia")]
fn internal_route(url: &str) -> Option<std::borrow::Cow<'_, str>> {
    (url.starts_with("#/at/") || url.starts_with("#/u/")).then_some(std::borrow::Cow::Borrowed(url))
}

/// Sanitise `html` with ammonia configured from exactly the allowlist tables. Renderer
/// output must come back unchanged (§8.5).
#[cfg(feature = "ammonia")]
pub fn ammonia_clean(html: &str) -> String {
    use std::collections::{HashMap, HashSet};
    let mut tags: HashSet<&str> = HTML_ALLOWLIST.iter().map(|(t, _)| *t).collect();
    tags.extend(MATHML_ELEMENTS.iter().copied());
    let mut attrs: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (t, a) in HTML_ALLOWLIST {
        attrs.insert(t, a.iter().copied().filter(|a| *a != "class").collect());
    }
    for t in MATHML_ELEMENTS {
        attrs.insert(t, MATHML_ATTRIBUTES.iter().copied().collect());
    }
    let mut classes: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (t, cs) in CLASS_ALLOWLIST {
        classes.insert(t, cs.iter().copied().collect());
    }
    // `code.language-*` tokens are open-ended; allow the class attribute on code and rely on
    // the checker for the grammar.
    attrs.get_mut("code").map(|s| s.insert("class"));
    classes.remove("code");
    let mut b = ammonia::Builder::empty();
    b.tags(tags)
        .tag_attributes(attrs)
        .allowed_classes(classes)
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        // The only relative URLs the renderer emits are the internal routes (§5.2); anything
        // else relative (`//host`, `/path`, `x`) is removed, as DOMPurify's URI pattern does.
        .url_relative(ammonia::UrlRelative::Custom(Box::new(internal_route)))
        .link_rel(None)
        .strip_comments(true);
    b.clean(html).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "ammonia")]
    #[test]
    fn ammonia_is_live() {
        let ours = crate::render(
            "# T\n\n[x](https://e.com) $x^2$ ![a](https://i.example/p.png)\n\n- [x] t\n",
            &[],
            &crate::RenderOpts::ALL,
        )
        .html;
        assert_eq!(ammonia_clean(&ours), ours);
        let evil = "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\" onclick=\"x\">a<script>b</script><a href=\"javascript:x\">c</a></p>\n</div>\n";
        let cleaned = ammonia_clean(evil);
        assert_ne!(cleaned, evil);
        assert!(
            !cleaned.contains("onclick")
                && !cleaned.contains("script")
                && !cleaned.contains("javascript"),
            "{cleaned}"
        );
        // Relative URLs other than the internal routes are removed.
        for href in ["//evil.com/x", "/x", "x.html", "#x"] {
            let html = format!(
                "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\"><a href=\"{href}\" rel=\"nofollow ugc noopener noreferrer\">x</a></p>\n</div>\n"
            );
            assert!(!ammonia_clean(&html).contains("href"), "{href}");
        }
        let route = "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\"><a href=\"#/u/did:plc:abcdefghijklmnopqrstuvwx\" data-did=\"did:plc:abcdefghijklmnopqrstuvwx\" class=\"mention\">x</a></p>\n</div>\n";
        assert_eq!(ammonia_clean(route), route);
    }

    #[test]
    fn dompurify_config_is_json() {
        let v: serde_json::Value = serde_json::from_str(&dompurify_config_json()).unwrap();
        assert_eq!(v["ALLOW_DATA_ATTR"], false);
        assert!(
            v["ALLOWED_TAGS"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t == "mfrac")
        );
        assert!(
            v["ALLOWED_ATTR"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t == "data-at-uri")
        );
        assert_eq!(v["LFM_CLASSES"]["details"][0], "spoiler");
        assert_eq!(v["ALLOW_ARIA_ATTR"], false);
        assert_eq!(v["SAFE_FOR_XML"], false);
        let safe = v["ADD_URI_SAFE_ATTR"].as_array().unwrap();
        assert!(safe.iter().any(|a| a == "dir") && safe.iter().any(|a| a == "data-cid"));
        assert!(!safe.iter().any(|a| a == "href"));
    }

    #[test]
    fn checker_accepts_and_rejects() {
        let ok = "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\">a &amp; <a href=\"https://x.y\" rel=\"nofollow ugc noopener noreferrer\">x</a></p>\n</div>\n";
        assert_eq!(check_html(ok), Ok(()));
        for bad in [
            "<div class=\"lfm\" dir=\"auto\">\n<script>x</script></div>\n",
            "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\" onclick=\"x\">a</p>\n</div>\n",
            "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\"><a href=\"javascript:x\" rel=\"nofollow ugc noopener noreferrer\">x</a></p>\n</div>\n",
            "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\">a < b</p>\n</div>\n",
            "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\"><math><mi style=\"x\">a</mi></math></p>\n</div>\n",
            "<div class=\"lfm\" dir=\"auto\">\n<p dir=\"auto\"><math><svg></svg></math></p>\n</div>\n",
        ] {
            assert!(check_html(bad).is_err(), "{bad}");
        }
    }
}
