// SPDX-License-Identifier: AGPL-3.0-or-later
//! URL policy (`docs/spec/lfm-1.md` §5): [`safe_href`], `at://` links, hover-preview
//! eligibility, mention display ([`short_did`]) and the real-host display rules (F1, F9).

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::at::{self, AtUri, Did};
use crate::plain::is_letter_or_number;

/// Where a URL comes from (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HrefCtx {
    /// A markdown link or image destination.
    Markdown,
    /// A record field: `defs#link.uri`, `defs#externalLink.uri`, `forum.reportContact`,
    /// `#endpoint.url`.
    Field,
}

/// The result of [`safe_href`]: the only URLs that may ever become clickable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Href {
    /// An `http`/`https` URL, normalised. May be opened outside the app.
    External {
        /// Normalised URL (scheme and host lowercased, path/query/fragment percent-encoded).
        url: String,
        /// Lowercase host (or bracketed IPv6 literal).
        host: String,
    },
    /// A `mailto:` URL. May be opened outside the app.
    Mailto {
        /// Normalised URL.
        url: String,
        /// Lowercase domain of the address, or `""`.
        host: String,
    },
    /// `at://did/nsid/rkey`: an internal route with a hover preview.
    Record {
        /// The canonical record URI.
        uri: AtUri,
    },
    /// `at://did`: a profile route.
    Profile {
        /// The DID.
        did: Did,
    },
    /// `at://handle` from a record field; navigable only after bidirectional verification.
    ProfileHandle {
        /// The lowercased handle.
        handle: String,
    },
    /// An in-document `#fragment` (markdown only): decoded, lowercased, charset-checked.
    Anchor {
        /// The anchor value (`data-anchor`).
        id: String,
    },
}

impl Href {
    /// True for the variants that may be opened outside the app (§5.1).
    pub fn is_openable_externally(&self) -> bool {
        matches!(self, Href::External { .. } | Href::Mailto { .. })
    }
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn forbidden_in_href(c: char) -> bool {
    c <= '\u{20}'
        || matches!(c, '\u{7F}'..='\u{9F}' | '\u{2028}' | '\u{2029}' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\\')
        || is_bidi_control(c)
}

/// The single gate for every URL that becomes clickable anywhere (§5.1).
pub fn safe_href(raw: &str, ctx: HrefCtx) -> Option<Href> {
    if raw.is_empty() || raw.len() > 3000 {
        return None;
    }
    if raw.chars().any(forbidden_in_href) {
        return None;
    }
    if let Some(frag) = raw.strip_prefix('#') {
        if ctx != HrefCtx::Markdown {
            return None;
        }
        let v = percent_decode_utf8(frag)?.to_lowercase();
        if v.is_empty()
            || v.len() > 512
            || !v
                .chars()
                .all(|c| c == '-' || c == '_' || is_letter_or_number(c))
        {
            return None;
        }
        return Some(Href::Anchor { id: v });
    }
    let colon = scheme_len(raw)?;
    let scheme = raw[..colon].to_ascii_lowercase();
    let rest = &raw[colon + 1..];
    match scheme.as_str() {
        "https" | "http" => http_href(&scheme, rest),
        "mailto" => {
            if rest.is_empty() || rest.starts_with('/') {
                return None;
            }
            let (addr, query) = match rest.split_once('?') {
                Some((a, q)) => (a, Some(q)),
                None => (rest, None),
            };
            let host = mailto_host(addr)?;
            if query.is_some_and(|q| !mailto_query_ok(q)) {
                return None;
            }
            Some(Href::Mailto {
                url: format!("mailto:{}", pct(rest)),
                host,
            })
        }
        "at" => parse_at_link(rest, ctx),
        _ => None,
    }
}

/// The domain of a `mailto:` address part, which must be exactly one address: a non-empty
/// local part and a domain under the `http` host grammar (ASCII, `[a-z0-9._-]+`, not only
/// dots), checked after percent-decoding as a mail client would decode it. More addresses
/// (`a@x,b@y`), an empty address (`mailto:?to=…`) or an internationalised domain are refused:
/// the host shown next to the link (§5.5, §9.4) must be the only one the mail goes to.
fn mailto_host(addr: &str) -> Option<String> {
    let addr = percent_decode_utf8(addr)?;
    let (local, domain) = addr.split_once('@')?;
    if local.is_empty()
        || local
            .chars()
            .any(|c| matches!(c, '@' | ',' | ';' | '<' | '>') || forbidden_in_href(c))
    {
        return None;
    }
    let domain = domain.to_ascii_lowercase();
    let ok = !domain.is_empty()
        && domain
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
        && !domain.bytes().all(|b| b == b'.');
    ok.then_some(domain)
}

/// A `mailto:` query may not add recipients: no `to`, `cc` or `bcc` field (names compared
/// after percent-decoding, ASCII case-insensitively).
fn mailto_query_ok(q: &str) -> bool {
    q.split('&').all(|field| {
        let name = field.split('=').next().unwrap_or("");
        match percent_decode_utf8(name) {
            Some(n) => !["to", "cc", "bcc"]
                .iter()
                .any(|r| n.eq_ignore_ascii_case(r)),
            None => false,
        }
    })
}

/// Length of the scheme (the index of its `:`), for `^[A-Za-z][A-Za-z0-9+.-]*:`.
fn scheme_len(raw: &str) -> Option<usize> {
    let b = raw.as_bytes();
    if !b.first()?.is_ascii_alphabetic() {
        return None;
    }
    let mut i = 1;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'+' | b'.' | b'-')) {
        i += 1;
    }
    (i < b.len() && b[i] == b':').then_some(i)
}

fn http_href(scheme: &str, rest: &str) -> Option<Href> {
    let after = rest.strip_prefix("//")?;
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, tail) = after.split_at(end);
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let close = v6.find(']')?;
        let inner = &v6[..close];
        // The RFC 4291 text form (§5.1), which the WHATWG IPv6 parser also accepts.
        if inner.parse::<core::net::Ipv6Addr>().is_err() {
            return None;
        }
        let after_br = &v6[close + 1..];
        let port = match after_br {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        (format!("[{inner}]"), port)
    } else {
        let (h, port) = match authority.find(':') {
            Some(i) => (&authority[..i], Some(&authority[i + 1..])),
            None => (authority, None),
        };
        let h = h.to_ascii_lowercase();
        if h.is_empty()
            || !h
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
            || h.bytes().all(|b| b == b'.')
            || !whatwg_host_ok(&h)
        {
            return None;
        }
        (h, port)
    };
    if let Some(p) = port {
        if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if p.parse::<u32>().ok()? > 65535 {
            return None;
        }
    }
    let mut url = format!("{scheme}://{host}");
    if let Some(p) = port {
        url.push(':');
        url.push_str(p);
    }
    url.push_str(&pct(tail));
    Some(Href::External { url, host })
}

/// The checks that keep every accepted `http(s)` host one the WHATWG URL parser accepts, and so
/// one the browser and ammonia (§8.5) accept too (§5.1, `host_ok`). `h` already matches
/// `^[a-z0-9._-]+$`.
/// - A host that ends in a number (its last label, after removing one trailing `.`, is all ASCII
///   digits, or `0x` and hex digits) is parsed by WHATWG as an IPv4 address, which can fail
///   (`256.1.1.1`, `a.123`) or rewrite it (`0x7f.1` is 127.0.0.1). It must be a dotted-quad IPv4
///   address, so that the host shown is the host visited.
/// - A label that starts with `xn--` is Punycode, and must decode ([`punycode_ok`]). It is still
///   never decoded for display (threat model ID-3).
fn whatwg_host_ok(h: &str) -> bool {
    let trimmed = h.strip_suffix('.').unwrap_or(h);
    let last = trimmed.rsplit('.').next().unwrap_or("");
    let ends_in_number = (!last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()))
        || last
            .strip_prefix("0x")
            .is_some_and(|x| x.bytes().all(|b| b.is_ascii_hexdigit()));
    if ends_in_number {
        return h.parse::<core::net::Ipv4Addr>().is_ok();
    }
    h.split('.').all(|label| match label.strip_prefix("xn--") {
        Some(p) => punycode_ok(p.as_bytes()),
        None => true,
    })
}

/// Whether `p`, the ASCII rest of an `xn--` label, is the Punycode (RFC 3492 §6.2) of a label
/// that UTS #46 processing accepts without Unicode tables: non-empty and not ending in `-`, at
/// most 2,000 bytes, decoding without error or overflow to at most 1,000 code points, the first
/// of them not U+200C or U+200D (ContextJ). These are the checks the WHATWG URL parser applies
/// before any table lookup. The decoded text itself is discarded.
fn punycode_ok(p: &[u8]) -> bool {
    const BASE: u32 = 36;
    const T_MIN: u32 = 1;
    const T_MAX: u32 = 26;
    fn adapt(mut delta: u32, points: u32, first: bool) -> u32 {
        delta /= if first { 700 } else { 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - T_MIN) * T_MAX) / 2 {
            delta /= BASE - T_MIN;
            k += BASE;
        }
        k + (BASE - T_MIN + 1) * delta / (delta + 38)
    }
    if p.is_empty() || p.ends_with(b"-") || p.len() > 2000 {
        return false;
    }
    // Basic code points come before the last `-`; a `-` in first position is not a delimiter
    // (it then fails as a digit).
    let (basic, ext) = match p.iter().rposition(|&b| b == b'-') {
        Some(i) if i > 0 => (&p[..i], &p[i + 1..]),
        _ => (&p[..0], p),
    };
    // Only the first code point matters (ContextJ); insertions at 0 replace it.
    let mut first = basic.first().map(|&b| char::from(b));
    let mut len = basic.len() as u32;
    let (mut n, mut bias, mut i) = (0x80u32, 72u32, 0u32);
    let mut digits = ext.iter();
    while let Some(&b0) = digits.next() {
        let prev = i;
        let (mut w, mut k, mut b) = (1u32, BASE, b0);
        loop {
            let digit = match b {
                b'a'..=b'z' => u32::from(b - b'a'),
                b'0'..=b'9' => u32::from(b - b'0') + 26,
                _ => return false,
            };
            let Some(next) = digit.checked_mul(w).and_then(|d| i.checked_add(d)) else {
                return false;
            };
            i = next;
            let t = if k <= bias {
                T_MIN
            } else if k >= bias + T_MAX {
                T_MAX
            } else {
                k - bias
            };
            if digit < t {
                break;
            }
            let (Some(nw), Some(&nb)) = (w.checked_mul(BASE - t), digits.next()) else {
                return false;
            };
            (w, k, b) = (nw, k + BASE, nb);
        }
        bias = adapt(i - prev, len + 1, prev == 0);
        let Some(cp) = n.checked_add(i / (len + 1)) else {
            return false;
        };
        n = cp;
        i %= len + 1;
        let Some(c) = char::from_u32(n) else {
            return false;
        };
        if i == 0 {
            first = Some(c);
        }
        len += 1;
        i += 1;
    }
    len <= 1000 && !matches!(first, Some('\u{200C}' | '\u{200D}'))
}

/// `pct(x)` (§5.1): every UTF-8 byte outside `[A-Za-z0-9]` and `-._~:/?#[]@!$&'()*+,;=%`
/// becomes `%XX` (uppercase hex).
pub(crate) fn pct(x: &str) -> String {
    let mut out = String::with_capacity(x.len());
    for &b in x.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode `%XX` escapes; invalid escapes are kept literally. `None` on invalid UTF-8.
fn percent_decode_utf8(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16);
            let l = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// `parse_at_link` (§5.2); `rest` is the text after `at:`.
fn parse_at_link(rest: &str, ctx: HrefCtx) -> Option<Href> {
    let body = rest.strip_prefix("//")?;
    if body.contains(['?', '#']) {
        return None;
    }
    let segs: Vec<&str> = body.split('/').collect();
    if segs[0].starts_with("did:") {
        let did = at::lfm_did(segs[0])?;
        return match segs.len() {
            1 => Some(Href::Profile { did }),
            3 => at::record_uri(&did, segs[1], segs[2]).map(|uri| Href::Record { uri }),
            _ => None,
        };
    }
    if segs.len() == 1 && ctx == HrefCtx::Field {
        return at::handle(segs[0]).map(|handle| Href::ProfileHandle { handle });
    }
    None
}

/// The mention placeholder text without the `@` (§5.4).
pub fn short_did(did: &str) -> String {
    if let Some(id) = did.strip_prefix("did:plc:") {
        let head: String = id.chars().take(8).collect();
        format!("did:plc:{head}\u{2026}")
    } else if did.chars().count() <= 40 {
        did.to_owned()
    } else {
        let head: String = did.chars().take(39).collect();
        format!("{head}\u{2026}")
    }
}

/// Hover-preview eligibility for a record link (§5.3): the collection must be one of
/// `NS.post`, `NS.comment`, `NS.sequence`, `NS.wikiRevision` or `NS.forum` for the build's `ns`
/// (`online.7778777.lessdugin` or its `.temp` development variant). DID links (`a[data-did]`)
/// are always eligible for a user card.
pub fn preview_eligible(uri: &AtUri, ns: &str) -> bool {
    let col = at::collection(uri);
    col.strip_prefix(ns)
        .and_then(|r| r.strip_prefix('.'))
        .is_some_and(|name| {
            matches!(
                name,
                "post" | "comment" | "sequence" | "wikiRevision" | "forum"
            )
        })
}

/// Default_Ignorable_Code_Point (Unicode DerivedCoreProperties): characters that render as
/// nothing, such as U+00AD SOFT HYPHEN, U+200B ZERO WIDTH SPACE and U+2060 WORD JOINER.
fn is_default_ignorable(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}'..='\u{1160}' | '\u{17B4}'..='\u{17B5}'
        | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFA0}'
        | '\u{FFF0}'..='\u{FFF8}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0000}'..='\u{E0FFF}')
}

/// A character of a host-like name: a letter, mark or number of any script (so that
/// lookalikes such as `ɡoogle.com` or Cyrillic `gооgle.com` count), `_` or `-`.
fn is_name_char(c: char) -> bool {
    use unicode_general_category::{GeneralCategory as G, get_general_category};
    c == '_'
        || c == '-'
        || matches!(
            get_general_category(c),
            G::UppercaseLetter
                | G::LowercaseLetter
                | G::TitlecaseLetter
                | G::ModifierLetter
                | G::OtherLetter
                | G::NonspacingMark
                | G::SpacingMark
                | G::EnclosingMark
                | G::DecimalNumber
                | G::LetterNumber
                | G::OtherNumber
        )
}

/// True if `t` starts with a dotted name: two or more non-empty runs of name characters
/// joined by `.`, whatever follows (`google.com!` is host-like).
fn starts_host_like(t: &str) -> bool {
    let mut segs = t.split('.');
    let name = |s: Option<&str>| s.and_then(|s| s.chars().next()).is_some_and(is_name_char);
    name(segs.next()) && name(segs.next())
}

/// The HTML real-host rule (§5.5, F9): true when a `<span class="link-host">` must be
/// appended to an external link whose visible text is `inner` (`plain(inner)`, unnormalised).
///
/// The visible text is first normalised the way it is displayed: NFKC (fullwidth and other
/// compatibility forms become ASCII), Default_Ignorable code points removed (invisible
/// characters cannot split a name), the ideographic full stops IDNA treats as dots mapped to
/// `.`. It is host-like if it starts with a dotted name of any script; the host is then shown
/// unless the text starts with exactly the link's host (one `www.` stripped from both), followed
/// by something other than a name character or a dot.
pub(crate) fn html_host_suffix(inner: &str, host: &str) -> bool {
    let visible: String = inner
        .chars()
        .filter(|&c| !is_default_ignorable(c))
        .nfkc()
        .map(|c| {
            if matches!(c, '\u{3002}' | '\u{FF61}') {
                '.'
            } else {
                c
            }
        })
        .collect();
    let mut t = visible.trim().to_ascii_lowercase();
    if let Some(s) = t.strip_prefix('@') {
        t = s.to_owned();
    }
    if let Some(i) = t.rfind('@') {
        t = t[i + 1..].to_owned();
    }
    for p in ["http://", "https://", "mailto:"] {
        if let Some(s) = t.strip_prefix(p) {
            t = s.to_owned();
            break;
        }
    }
    if let Some(s) = t.strip_prefix("www.") {
        t = s.to_owned();
    }
    if !starts_host_like(&t) {
        return false;
    }
    let end = t
        .find(|c: char| c != '.' && !is_name_char(c))
        .unwrap_or(t.len());
    let m = t[..end].trim_end_matches('.');
    let h = host.strip_prefix("www.").unwrap_or(host);
    m != h
}

/// The terminal real-host rule (§9.4, F1): print ` ⟨host⟩` after the link text unless
/// `norm(text) == norm(url)`.
pub fn term_shows_host(text: &str, url: &str) -> bool {
    fn norm(s: &str) -> String {
        let mut t = s;
        for p in ["https://", "http://", "mailto:"] {
            if let Some(r) = t.strip_prefix(p) {
                t = r;
                break;
            }
        }
        if let Some(r) = t.strip_prefix("www.") {
            t = r;
        }
        if let Some(r) = t.strip_suffix('/') {
            t = r;
        }
        t.to_ascii_lowercase()
    }
    norm(text) != norm(url)
}

/// `cid:` resolution (§6.1): the CID string if `dest` is a well-formed `cid:` URL.
/// `Some(Err(()))` for a `cid:` URL with a malformed CID (always a miss).
pub(crate) fn cid_of(dest: &str) -> Option<Result<&str, ()>> {
    if dest.len() < 4 || !dest.is_char_boundary(4) || !dest[..4].eq_ignore_ascii_case("cid:") {
        return None;
    }
    let c = &dest[4..];
    Some(if is_valid_cid(c) { Ok(c) } else { Err(()) })
}

/// `^b[a-z2-7]{1,127}$`.
pub(crate) fn is_valid_cid(c: &str) -> bool {
    let b = c.as_bytes();
    (2..=128).contains(&b.len())
        && b[0] == b'b'
        && b[1..]
            .iter()
            .all(|&x| matches!(x, b'a'..=b'z' | b'2'..=b'7'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(raw: &str) -> Option<(String, String)> {
        match safe_href(raw, HrefCtx::Markdown)? {
            Href::External { url, host } => Some((url, host)),
            _ => None,
        }
    }

    #[test]
    fn http_basics() {
        assert_eq!(
            ext("HTTPS://Example.COM/Ä?q=1#f"),
            Some((
                "https://example.com/%C3%84?q=1#f".into(),
                "example.com".into()
            ))
        );
        assert_eq!(
            ext("http://example.com:8080"),
            Some(("http://example.com:8080".into(), "example.com".into()))
        );
        assert_eq!(
            ext("http://[::1]:80/x"),
            Some(("http://[::1]:80/x".into(), "[::1]".into()))
        );
        assert!(ext("http://u@example.com").is_none());
        assert!(ext("http://example.com:65536").is_none());
        assert!(ext("http://example.com:").is_none());
        assert!(ext("http://.../").is_none());
        assert!(ext("http://exämple.com").is_none());
        assert!(ext("//example.com").is_none());
        assert!(ext("http:example.com").is_none());
        assert!(safe_href("javascript:alert(1)", HrefCtx::Markdown).is_none());
        assert!(safe_href("http://a.b\\c", HrefCtx::Markdown).is_none());
    }

    /// Hosts the WHATWG URL parser rejects or rewrites are not linked (§5.1). Found by the
    /// `render` fuzz target (`fuzz/regressions/render/invalid-punycode-host`): ammonia, which
    /// parses URLs with the `url` crate, stripped the `href` of an invalid Punycode host. Every
    /// case was checked against `url` 2.5.8 (idna 1.1.0, the workspace's ASCII-only
    /// idna_adapter 1.0.0): it rejects each `None` below except the rewritten IPv4 forms, and
    /// accepts each `Some`.
    #[test]
    fn hosts_the_url_parser_accepts() {
        for ok in [
            "xn--exmple-cua.com",
            "a.xn--p1ai",
            "xn--a",
            "xn--a_b-xyz",
            "xn--ls8h",
            "a_b.com",
            "-a.com",
            "a..b",
            "a.",
            "1.2.3.4",
            "255.255.255.255",
            "a.0x1g",
            "1e1.com",
            "[::1]",
            "[::]",
            "[1:2:3:4:5:6:7:8]",
            "[::ffff:1.2.3.4]",
            "[::1.2.3.4]",
        ] {
            assert!(ext(&format!("https://{ok}/")).is_some(), "{ok}");
        }
        for bad in [
            // the fuzz reproducer
            "xn--exmple-cuattps2//ex%C3%A4mple.com",
            "xn--",
            "xn--a-",
            "xn---",
            "xn---abc",
            "xn--abc-",
            "xn--zz",
            "xn--9",
            "a.xn--zz.com",
            "xn--0ug",
            "xn--1ug.com",
            "256.1.1.1",
            "1.2.3.4.5",
            "a.123",
            "example.0x",
            "example.1",
            "a.b.0",
            "a.09",
            "999999999999",
            "4294967296",
            // valid for WHATWG, but rewritten to another address than the one shown
            "0x7f.1",
            "1.2.3",
            "01.02.03.04",
            "4294967295",
            "1.2.3.4.",
            "[1]",
            "[1::2::3]",
            "[1.2.3.4]",
            "[:::]",
            "[::1:]",
            "[12345::]",
            "[1:2:3:4::5:6:7:8]",
            "[::01.2.3.4]",
            "[1:2:3:4:5:6:7:1.2.3.4]",
        ] {
            assert!(ext(&format!("https://{bad}/")).is_none(), "{bad}");
        }
        let long = format!("https://xn--{}/", "a".repeat(2001));
        assert!(ext(&long).is_none());
        // 1,001 decoded code points (one basic code point and 1,000 insertions)
        assert!(ext(&format!("https://xn--a-{}/", "a".repeat(1000))).is_none());
        assert!(ext(&format!("https://xn--a-{}/", "a".repeat(999))).is_some());
    }

    #[test]
    fn anchors() {
        assert_eq!(safe_href("#Foo%20", HrefCtx::Markdown), None);
        assert_eq!(
            safe_href("#Fo%C3%96", HrefCtx::Markdown),
            Some(Href::Anchor { id: "foö".into() })
        );
        assert_eq!(safe_href("#x", HrefCtx::Field), None);
        assert_eq!(safe_href("#%FF", HrefCtx::Markdown), None);
        assert_eq!(safe_href("#/x", HrefCtx::Markdown), None);
        assert_eq!(safe_href("#", HrefCtx::Markdown), None);
    }

    #[test]
    fn at_links() {
        let d = "did:plc:abcdefghijklmnopqrstuvwx";
        assert!(matches!(
            safe_href(&format!("at://{d}"), HrefCtx::Markdown),
            Some(Href::Profile { .. })
        ));
        assert!(matches!(
            safe_href(
                &format!("at://{d}/online.7778777.lessdugin.post/3k2a"),
                HrefCtx::Markdown
            ),
            Some(Href::Record { .. })
        ));
        assert!(safe_href(&format!("at://{d}/"), HrefCtx::Markdown).is_none());
        assert!(safe_href(&format!("at://{d}/a.b.c"), HrefCtx::Markdown).is_none());
        assert!(safe_href("at://alice.example.com", HrefCtx::Markdown).is_none());
        assert!(matches!(
            safe_href("at://Alice.Example.com", HrefCtx::Field),
            Some(Href::ProfileHandle { .. })
        ));
        assert!(safe_href("at://did:key:zabc", HrefCtx::Markdown).is_none());
        assert!(safe_href("at://did:web:example.com", HrefCtx::Markdown).is_some());
        assert!(safe_href("at://did:web:Example.com", HrefCtx::Markdown).is_none());
    }

    #[test]
    fn mailto() {
        let host = |raw: &str| match safe_href(raw, HrefCtx::Markdown) {
            Some(Href::Mailto { host, .. }) => Some(host),
            _ => None,
        };
        assert_eq!(
            host("mailto:a@Example.com?subject=x"),
            Some("example.com".into())
        );
        assert_eq!(host("mailto:a%40b@example.com"), None);
        assert_eq!(host("mailto:?to=attacker@evil.com"), None);
        assert_eq!(host("mailto:a@evil.com,b@google.com"), None);
        assert_eq!(host("mailto:a@evil.com%2Cb@google.com"), None);
        assert_eq!(host("mailto:a@evil.com?cc=b@google.com"), None);
        assert_eq!(host("mailto:a@evil.com?subject=x&BCC=b@google.com"), None);
        assert_eq!(host("mailto:a@evil.com?%74o=b@google.com"), None);
        assert_eq!(host("mailto:support@g\u{43e}\u{43e}gle.com"), None);
        assert_eq!(host("mailto:example.com"), None);
    }

    #[test]
    fn host_rule() {
        assert!(html_host_suffix("google.com", "evil.com"));
        assert!(html_host_suffix("google.com\u{200B}", "evil.com"));
        assert!(html_host_suffix("goo\u{AD}gle.com", "evil.com"));
        assert!(html_host_suffix("google.com!", "evil.com"));
        assert!(!html_host_suffix("google.com!", "google.com"));
        assert!(html_host_suffix("\u{261}oogle.com", "evil.com"));
        assert!(html_host_suffix("g\u{43e}\u{43e}gle.com", "google.com"));
        assert!(html_host_suffix("\u{FF47}oogle\u{FF0E}com", "evil.com"));
        assert!(!html_host_suffix("\u{FF47}oogle\u{FF0E}com", "google.com"));
        assert!(html_host_suffix("google\u{3002}com", "evil.com"));
        assert!(html_host_suffix("google.com.evil.com", "google.com"));
        assert!(!html_host_suffix("google.com.", "google.com"));
        assert!(!html_host_suffix(
            "https://www.Example.com/x",
            "example.com"
        ));
        assert!(!html_host_suffix("click here", "evil.com"));
        assert!(html_host_suffix("@bob@google.com", "evil.com"));
        assert!(!html_host_suffix("example.com", "www.example.com"));
        assert!(term_shows_host("click", "https://x.com"));
        assert!(!term_shows_host("https://www.x.com/", "https://x.com"));
    }

    #[test]
    fn previews() {
        let ok = |s: &str| match safe_href(s, HrefCtx::Markdown) {
            Some(Href::Record { uri }) => uri,
            other => panic!("{other:?}"),
        };
        let d = "did:plc:abcdefghijklmnopqrstuvwx";
        for (nsid, ns, want) in [
            (
                "online.7778777.lessdugin.post",
                "online.7778777.lessdugin",
                true,
            ),
            (
                "online.7778777.lessdugin.wikiRevision",
                "online.7778777.lessdugin",
                true,
            ),
            (
                "online.7778777.lessdugin.temp.comment",
                "online.7778777.lessdugin.temp",
                true,
            ),
            (
                "online.7778777.lessdugin.temp.comment",
                "online.7778777.lessdugin",
                false,
            ),
            (
                "online.7778777.lessdugin.vote",
                "online.7778777.lessdugin",
                false,
            ),
            ("app.bsky.feed.post", "online.7778777.lessdugin", false),
        ] {
            assert_eq!(
                preview_eligible(&ok(&format!("at://{d}/{nsid}/x")), ns),
                want,
                "{nsid} {ns}"
            );
        }
    }

    #[test]
    fn short() {
        assert_eq!(
            short_did("did:plc:abcdefghijklmnopqrstuvwx"),
            "did:plc:abcdefgh\u{2026}"
        );
        assert_eq!(short_did("did:web:example.com"), "did:web:example.com");
    }
}
