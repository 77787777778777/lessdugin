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

/// `.` and the code points that read as one (§5.5 step 2): the ideographic full stops IDNA
/// treats as dots, and the rest of what UTS #39 `confusables.txt` maps to U+002E (U+2024, U+FE52
/// and U+FF0E are `.` after NFKC already).
fn is_dot(c: char) -> bool {
    matches!(
        c,
        '.' | '\u{3002}'
            | '\u{FF61}'
            | '\u{0660}'
            | '\u{06F0}'
            | '\u{0701}'
            | '\u{0702}'
            | '\u{A4F8}'
            | '\u{A60E}'
            | '\u{ABEC}'
            | '\u{10A50}'
            | '\u{1D16D}'
            | '\u{1ECAE}'
    )
}

/// `\` and the code points that read as `/` or `\` (§5.5 step 2): browsers read `\` as `/` in
/// an `http` or `https` URL, and these are the symbols and punctuation UTS #39
/// `confusables.txt` maps to U+002F or U+005C (U+FF0F is `/` and U+FE68 and U+FF3C are `\`
/// after NFKC already). The letters it maps there (U+30CE KATAKANA LETTER NO, U+4E3F and the
/// like) stay name characters.
fn is_slash(c: char) -> bool {
    matches!(
        c,
        '\\' | '\u{1735}'
            | '\u{2041}'
            | '\u{2044}'
            | '\u{2215}'
            | '\u{2216}'
            | '\u{2571}'
            | '\u{27CB}'
            | '\u{27CD}'
            | '\u{29F5}'
            | '\u{29F8}'
            | '\u{29F9}'
            | '\u{31D3}'
            | '\u{31D4}'
            | '\u{1D20F}'
            | '\u{1D23A}'
            | '\u{1D23B}'
    )
}

/// The RFC 3986 delimiters, `%` and `~`: with name characters and `.`, the code points of a URL
/// run (§5.5).
fn is_url_char(c: char) -> bool {
    matches!(
        c,
        '!' | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | '/'
            | ':'
            | ';'
            | '='
            | '?'
            | '@'
            | '['
            | ']'
            | '~'
    )
}

/// The end of the run of name characters starting at `t[i]`.
fn name_end(t: &[char], i: usize) -> usize {
    t[i..]
        .iter()
        .position(|&c| !is_name_char(c))
        .map_or(t.len(), |n| i + n)
}

/// The end of the dotted name starting at `t[i]`, if one does: two or more runs of name
/// characters joined by single dots, as long as possible.
fn dotted_end(t: &[char], i: usize) -> Option<usize> {
    let first = name_end(t, i);
    let mut end = first;
    while first > i && t.get(end) == Some(&'.') && t.get(end + 1).is_some_and(|&c| is_name_char(c))
    {
        end = name_end(t, end + 1);
    }
    (end > first).then_some(end)
}

/// The HTML real-host rule (§5.5, F9): true when a `<span class="link-host">` must be
/// appended to an external link whose visible text is `inner` (`plain(inner)`, unnormalised).
///
/// The visible text is first normalised the way it is displayed: Default_Ignorable code points
/// removed (invisible characters cannot split a name), NFKC (fullwidth and other compatibility
/// forms become ASCII), the code points that read as a dot mapped to `.`, `\` and the symbols
/// that read as `/` or `\` mapped to `/`, and blanks, symbols and other gaps next to a dot
/// removed (`google.⠀com`). Every dotted name of any script in it is then a host the text
/// names, except an e-mail address's mailbox (never one in a URL's authority, after `//`) and
/// the names in a URL's path, query or fragment. The host is shown unless every name the text
/// names is exactly the link's host (one `www.` stripped from both): wherever a name stands in
/// the text, a different one cannot go unflagged (red-team F2).
pub(crate) fn html_host_suffix(inner: &str, host: &str) -> bool {
    // Steps 1-3. White_Space is kept out of NFKC, which would turn the narrow spaces into the
    // U+0020 that step 4 keeps.
    fn push_nfkc(t: &mut Vec<char>, s: &str) {
        t.extend(s.nfkc().map(|c| {
            if is_dot(c) {
                '.'
            } else if is_slash(c) {
                '/'
            } else {
                c.to_ascii_lowercase()
            }
        }));
    }
    let mut t: Vec<char> = Vec::with_capacity(inner.len());
    let mut word = String::new();
    for c in inner.chars().filter(|&c| !is_default_ignorable(c)) {
        if c.is_whitespace() {
            push_nfkc(&mut t, &word);
            word.clear();
            t.push(c);
        } else {
            word.push(c);
        }
    }
    push_nfkc(&mut t, &word);
    // Step 4: remove every gap (a run of code points that are not name characters, `.`, URL
    // characters or ASCII whitespace) next to a `.`: U+2800 BRAILLE PATTERN BLANK, U+200A HAIR
    // SPACE, an emoji.
    let gap = |c: char| {
        c != '.'
            && !is_name_char(c)
            && !is_url_char(c)
            && !matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
    };
    let mut u: Vec<char> = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let j = t[i..]
            .iter()
            .position(|&c| !gap(c))
            .map_or(t.len(), |n| i + n);
        if j == i {
            u.push(t[i]);
            i += 1;
            continue;
        }
        if u.last() != Some(&'.') && t.get(j) != Some(&'.') {
            u.extend_from_slice(&t[i..j]);
        }
        i = j;
    }
    // Step 5. `seen`: the current URL run has had a dotted name; `path`: a `/`, `?` or `#`
    // has followed one, so the names after it are in a path, query or fragment; `authority`:
    // the run has had `//`, so every name before that `/`, `?` or `#` is a host, whatever
    // stands between the `//` and it (`https://u@google.com@evil.com` names both). Step 2
    // made every `\` and lookalike slash a `/`, so they count here as `/` does.
    let h = host.strip_prefix("www.").unwrap_or(host);
    let (mut seen, mut path, mut authority) = (false, false, false);
    let mut i = 0;
    while i < u.len() {
        let c = u[i];
        if !is_name_char(c) {
            if matches!(c, '/' | '?' | '#') {
                path |= seen;
                authority |= c == '/' && i >= 1 && u[i - 1] == '/';
            } else if c != '.' && !is_url_char(c) {
                (seen, path, authority) = (false, false, false);
            }
            i += 1;
            continue;
        }
        let Some(end) = dotted_end(&u, i) else {
            i = name_end(&u, i);
            continue;
        };
        // `first.last@corp.com` names only `corp.com`; in a URL's authority no name is a mailbox.
        let mailbox = u.get(end) == Some(&'@') && dotted_end(&u, end + 1).is_some() && !authority;
        if !mailbox && !path {
            let d: String = u[i..end].iter().collect();
            if d.strip_prefix("www.").unwrap_or(&d) != h {
                return true;
            }
        }
        seen = true;
        i = end;
    }
    false
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
        // Leading code points that are not name characters do not hide the host (red-team F2):
        // wrapping punctuation, symbols, emoji and blanks that are neither White_Space nor
        // Default_Ignorable (U+2800 braille blank, U+1D159 musical null notehead).
        let leads = "(|\"|[|\u{A1}|\u{2192} |\u{2022} |\u{1F512} |\u{2800}|\u{1D159}|...|@(|<@";
        for lead in leads.split('|') {
            let text = format!("{lead}google.com)");
            assert!(html_host_suffix(&text, "evil.com"), "{text:?}");
            assert!(!html_host_suffix(&text, "google.com"), "{text:?}");
        }
        // ... after the `@`, `http://` and `www.` steps too.
        for text in [
            "bob@(google.com",
            "https://(google.com)",
            "(https://www.google.com)",
            "www.\u{2800}google.com",
        ] {
            assert!(html_host_suffix(text, "evil.com"), "{text:?}");
            assert!(!html_host_suffix(text, "google.com"), "{text:?}");
        }
        assert!(!html_host_suffix("(click here)", "evil.com"));
        // Every dotted name the text names is tested, wherever it stands, and a gap or a
        // lookalike next to or in place of its dot does not hide it (red-team F2): a sentence
        // before it, an `@` after it, U+2800, U+1D159, U+200A HAIR SPACE, U+00A0 or U+3000 next
        // to the dot, U+A4F8, U+0701, U+0660 or U+1D16D in its place.
        for text in [
            "See. google.com",
            "docs. (google.com)",
            "google.com (ask bob@corp)",
            "google.com@corp",
            "@google.com",
            "google.\u{2800}com",
            "google.\u{1D159}com",
            "google\u{2800}.com",
            "google\u{1F512}.\u{2800}com",
            "google.\u{200A}com",
            "google.\u{A0}com",
            "google\u{3000}.com",
            "google\u{A4F8}com",
            "google\u{0701}com",
            "google\u{0660}com",
            "google\u{1D16D}com",
            "https://www.google\u{A4F8}com/x",
        ] {
            assert!(html_host_suffix(text, "evil.com"), "{text:?}");
            assert!(!html_host_suffix(text, "google.com"), "{text:?}");
        }
        // Two names: the span shows whichever one the link does not go to.
        for text in [
            "evil.com google.com",
            "evil.com, not google.com",
            "evil.com. google.com",
            "evil.com\u{2800}google.com",
            "https://google.com@evil.com/",
            "evil.com/x google.com",
        ] {
            assert!(html_host_suffix(text, "evil.com"), "{text:?}");
            assert!(html_host_suffix(text, "google.com"), "{text:?}");
        }
        // In a URL's authority (after `//`) no name is a mailbox, whatever stands between the
        // `//` and it: the exception for a name directly after `//` was bypassed by one `@` or a
        // user name (post-fix review security-3).
        for text in [
            "https://@google.com@evil.com",
            "https://u@google.com@evil.com",
            "https://u:p@google.com@evil.com",
            "https://x@google.com@evil.com/path",
            "//google.com@evil.com",
            "(https://@google.com@evil.com)",
        ] {
            assert!(html_host_suffix(text, "evil.com"), "{text:?}");
        }
        // ... while outside one the mailbox rule stands, and a user name is no host.
        assert!(!html_host_suffix("https://u:p@evil.com", "evil.com"));
        assert!(!html_host_suffix("bob@google.com@evil.com", "evil.com"));
        // A backslash, which URL parsers read as `/` in an `https` URL, and the symbols that
        // read as `/` or `\` start an authority as `//` does, also next to a dot (review2
        // m3m4-continuation-4): before, they reset the URL run, so the name passed as a mailbox.
        for text in [
            "https:\\\\u@google.com@evil.com",
            "https:/\\u@google.com@evil.com",
            "https:\\\\@google.com@evil.com",
            "https:\\\\.google.com@evil.com",
            "https:\u{FF3C}\u{FE68}u@google.com@evil.com",
            "https:\u{2215}\u{2215}u@google.com@evil.com",
            "https:\u{2044}\u{2044}u@google.com@evil.com",
            "https:\u{29F8}\u{29F8}u@google.com@evil.com",
            "https:\u{2571}\u{27CB}u@google.com@evil.com",
            "https:\u{1735}\u{2041}u@google.com@evil.com",
            "https:\u{31D3}\u{1D23A}u@google.com@evil.com",
            "https:\u{2216}\u{29F5}u@google.com@evil.com",
            "https:\u{29F9}\u{27CD}u@google.com@evil.com",
            "https:\u{31D4}\u{1D20F}\u{1D23B}u@google.com@evil.com",
            "https:\u{2215}\u{2215}.google.com@evil.com",
        ] {
            assert!(html_host_suffix(text, "evil.com"), "{text:?}");
        }
        // ... and a path as `/` does, so a name after one is no host; next to a dot one is a
        // `/`, never a gap, so `google.∕com` names no host, as `google./com` names none.
        for (text, host) in [
            ("evil.com\\google.com", "evil.com"),
            ("evil.com\u{2215}google.com", "evil.com"),
            ("https:\\\\evil.com\\a\\b.html", "evil.com"),
            ("https:\u{2044}\u{2044}evil.com\u{2044}a.html", "evil.com"),
            ("google.\u{2215}com", "evil.com"),
            ("google\\.com", "evil.com"),
        ] {
            assert!(!html_host_suffix(text, host), "{text:?} {host}");
        }
        // The code points that read as two or three slashes are as many `/`, and a nonspacing
        // or enclosing mark after a `/` draws on it (U+29F6 is `/` with a macron), so the `//`
        // stands, also without a scheme or next to a dot (review 2, m3m4-continuation-4).
        for lead in [
            "\u{2AFD}",
            "\u{2AFB}",
            "\u{244A}",
            "\u{2CF9}",
            "\u{29F6}\u{29F6}",
            "\u{2572}\u{31D2}",
            "/\u{0338}/\u{0338}",
            "\u{2215}\u{20DD}\u{2215}",
        ] {
            for text in [
                format!("{lead}u@google.com@evil.com"),
                format!("x:{lead}u@google.com@evil.com"),
                format!("{lead}.google.com@evil.com"),
            ] {
                assert!(html_host_suffix(&text, "evil.com"), "{text:?}");
            }
        }
        for (text, host) in [
            ("evil.com\u{2AFD}google.com", "evil.com"),
            ("evil.com\u{2CF9}google.com", "evil.com"),
            ("evil.com\u{29F6}google.com", "evil.com"),
            ("evil.com\u{2572}google.com", "evil.com"),
            ("google.\u{2AFB}com", "evil.com"),
            ("google\u{244A}.com", "evil.com"),
            ("/\u{0338}google.com", "google.com"),
        ] {
            assert!(!html_host_suffix(text, host), "{text:?} {host}");
        }
        // After a special scheme other than `file` and its `:`, URL parsers read an authority
        // with one `/` or none, so no name there is a mailbox either.
        for text in [
            "https:u@google.com@evil.com",
            "https:/u@google.com@evil.com",
            "https:\\u@google.com@evil.com",
            "HTTP:u@google.com@evil.com",
            "\u{FF48}ttps\u{FF1A}u@google.com@evil.com",
            "ftp:google.com@evil.com",
            "ws:u@google.com@evil.com",
            "wss:/@google.com@evil.com",
            "(https:u@google.com@evil.com)",
        ] {
            assert!(html_host_suffix(text, "evil.com"), "{text:?}");
        }
        // ... and only there: other schemes and longer names keep the mailbox rule.
        for (text, host) in [
            ("news:first.last@corp.com", "corp.com"),
            ("file:first.last@corp.com", "corp.com"),
            ("xhttps:first.last@corp.com", "corp.com"),
            ("https-x:first.last@corp.com", "corp.com"),
            ("https:u:p@evil.com", "evil.com"),
        ] {
            assert!(!html_host_suffix(text, host), "{text:?} {host}");
        }
        // The letters that read as `/` stay name characters (Japanese and Chinese words), and
        // the symbols that read as a slash beside a set sign stay gaps.
        assert!(html_host_suffix("evil.com\u{30CE}google.com", "evil.com"));
        assert!(html_host_suffix("evil.com\u{4E3F}google.com", "evil.com"));
        assert!(html_host_suffix("evil.com\u{FF89}google.com", "evil.com"));
        assert!(html_host_suffix("evil.com\u{2CC7}google.com", "evil.com"));
        assert!(html_host_suffix("evil.com\u{27C8}google.com", "evil.com"));
        // Mailboxes and path, query or fragment names are not hosts; ASCII whitespace still
        // ends a name, so prose is not one.
        for (text, host) in [
            ("https://example.com/index.html?v=1.2#sec.3", "example.com"),
            ("example.com/a/b.c", "example.com"),
            ("first.last@corp.com", "corp.com"),
            ("mailto:first.last@corp.com", "corp.com"),
            ("see example.com", "example.com"),
            ("example.com./x", "example.com"),
            ("Hello. World", "evil.com"),
            ("google. com", "evil.com"),
            ("a..b", "evil.com"),
        ] {
            assert!(!html_host_suffix(text, host), "{text:?} {host}");
        }
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
