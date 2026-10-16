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
            let addr = rest.split('?').next().unwrap_or("");
            let host = match addr.rfind('@') {
                Some(i) => addr[i + 1..].to_ascii_lowercase(),
                None => String::new(),
            };
            Some(Href::Mailto {
                url: format!("mailto:{}", pct(rest)),
                host,
            })
        }
        "at" => parse_at_link(rest, ctx),
        _ => None,
    }
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
        if inner.is_empty()
            || !inner
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        {
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

/// `[a-z0-9_-]+(\.[a-z0-9_-]+)+` at the start of `t`, followed by end, `/`, `:`, `?` or `#`.
fn host_like(t: &str) -> Option<&str> {
    let end = t
        .bytes()
        .position(|b| !matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'))
        .unwrap_or(t.len());
    if end < t.len() && !matches!(t.as_bytes()[end], b'/' | b':' | b'?' | b'#') {
        return None;
    }
    let m = &t[..end];
    let segs: Vec<&str> = m.split('.').collect();
    (segs.len() >= 2 && segs.iter().all(|s| !s.is_empty())).then_some(m)
}

/// The HTML real-host rule (§5.5, F9): true when a `<span class="link-host">` must be
/// appended to an external link whose visible text is `inner` (`plain(inner)`, unnormalised).
pub(crate) fn html_host_suffix(inner: &str, host: &str) -> bool {
    let nfc: String = inner.nfc().collect();
    let mut t = nfc.trim().to_ascii_lowercase();
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
    let Some(m) = host_like(&t) else {
        return false;
    };
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
    fn host_rule() {
        assert!(html_host_suffix("google.com", "evil.com"));
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
