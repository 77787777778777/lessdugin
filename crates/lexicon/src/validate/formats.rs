// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lexicon string formats (`/specs/lexicon`, "String Formats").
//!
//! The identifier formats delegate to `lessdugin-atp`'s syntax types, which pass the atproto
//! interop fixtures. `uri` and `language` are implemented here and checked against the same
//! fixtures (`testdata/interop/syntax/{uri,language}_*.txt`).

use lessdugin_atp::cid::is_valid_cid_syntax;
use lessdugin_atp::syntax::{
    AtIdentifier, Datetime, Did, Handle, LooseAtUri, Nsid, RecordKey, Tid,
};

use crate::schemas::StringFormat;

/// Maximum length of a `uri` value (8 KiB, as for at-uris).
pub const MAX_URI_LEN: usize = 8192;

/// Checks `s` against a Lexicon string format; `Err` holds a short reason.
pub fn check_format(format: StringFormat, s: &str) -> Result<(), &'static str> {
    let ok = match format {
        StringFormat::AtIdentifier => AtIdentifier::parse(s).is_ok(),
        StringFormat::AtUri => LooseAtUri::parse(s).is_ok(),
        StringFormat::Cid => is_valid_cid_syntax(s),
        StringFormat::Datetime => Datetime::parse(s).is_ok(),
        StringFormat::Did => Did::parse(s).is_ok(),
        StringFormat::Handle => Handle::parse(s).is_ok(),
        StringFormat::Nsid => Nsid::parse(s).is_ok(),
        StringFormat::Tid => Tid::parse(s).is_ok(),
        StringFormat::RecordKey => RecordKey::parse(s).is_ok(),
        StringFormat::Uri => is_valid_uri(s),
        StringFormat::Language => is_valid_language_syntax(s),
    };
    if ok {
        Ok(())
    } else {
        Err(match format {
            StringFormat::AtIdentifier => "not a DID or handle",
            StringFormat::AtUri => "not an at-uri",
            StringFormat::Cid => "not a CID",
            StringFormat::Datetime => "not an atproto datetime",
            StringFormat::Did => "not a DID",
            StringFormat::Handle => "not a handle",
            StringFormat::Nsid => "not an NSID",
            StringFormat::Tid => "not a TID",
            StringFormat::RecordKey => "not a record key",
            StringFormat::Uri => "not a URI",
            StringFormat::Language => "not a BCP 47 language tag",
        })
    }
}

/// The Lexicon `uri` format: `scheme ":" rest`, where the scheme is `[a-z][a-z.-]{0,80}` and the
/// rest is one or more printable ASCII characters other than space, at most [`MAX_URI_LEN`]
/// bytes in total. This is the reference implementations' regex
/// `^[a-z][a-z.-]{0,80}:[!-~]+$`; it accepts exactly the interop fixtures' valid list.
/// Non-ASCII IRIs must be percent-encoded.
pub fn is_valid_uri(s: &str) -> bool {
    if s.len() > MAX_URI_LEN {
        return false;
    }
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let sb = scheme.as_bytes();
    !sb.is_empty()
        && sb.len() <= 81
        && sb[0].is_ascii_lowercase()
        && sb[1..]
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b == b'.' || b == b'-')
        && !rest.is_empty()
        && rest.bytes().all(|b| (b'!'..=b'~').contains(&b))
}

const IRREGULAR_GRANDFATHERED: &[&str] = &[
    "en-GB-oed",
    "i-ami",
    "i-bnn",
    "i-default",
    "i-enochian",
    "i-hak",
    "i-klingon",
    "i-lux",
    "i-mingo",
    "i-navajo",
    "i-pwn",
    "i-tao",
    "i-tay",
    "i-tsu",
    "sgn-BE-FR",
    "sgn-BE-NL",
    "sgn-CH-DE",
];

/// A parsed BCP 47 language tag's checkable parts.
#[derive(Debug, Default)]
struct LangParts<'a> {
    variants: Vec<&'a str>,
    singletons: Vec<u8>,
}

fn alpha(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphabetic())
}

fn alnum(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn digit(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// Private use: `x` (either case) followed by one or more `-` 1*8alphanum subtags.
fn private_use(subtags: &[&str]) -> bool {
    !subtags.is_empty()
        && subtags
            .iter()
            .all(|t| (1..=8).contains(&t.len()) && alnum(t))
}

/// Parses `langtag` (RFC 5646 §2.1) with lessdugin's (and the interop fixtures') restriction
/// that the primary language subtag is 2–3 *lowercase* letters. `None` if not well-formed.
fn parse_langtag(s: &str) -> Option<LangParts<'_>> {
    let tags: Vec<&str> = s.split('-').collect();
    let mut i = 0;
    let lang = tags.first()?;
    if !(2..=3).contains(&lang.len()) || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    i += 1;
    // extlang: up to three 3-letter subtags
    let mut ext = 0;
    while ext < 3 && i < tags.len() && tags[i].len() == 3 && alpha(tags[i]) {
        i += 1;
        ext += 1;
    }
    // script
    if i < tags.len() && tags[i].len() == 4 && alpha(tags[i]) {
        i += 1;
    }
    // region
    if i < tags.len()
        && ((tags[i].len() == 2 && alpha(tags[i])) || (tags[i].len() == 3 && digit(tags[i])))
    {
        i += 1;
    }
    let mut parts = LangParts::default();
    // variants
    while i < tags.len() {
        let t = tags[i];
        let is_variant = ((5..=8).contains(&t.len()) && alnum(t))
            || (t.len() == 4 && t.as_bytes()[0].is_ascii_digit() && alnum(t));
        if !is_variant {
            break;
        }
        parts.variants.push(t);
        i += 1;
    }
    // extensions: singleton (not x) then one or more 2..8 alphanum subtags
    while i < tags.len() {
        let t = tags[i];
        if t.len() != 1 || !alnum(t) || t.eq_ignore_ascii_case("x") {
            break;
        }
        let singleton = t.as_bytes()[0].to_ascii_lowercase();
        i += 1;
        let start = i;
        while i < tags.len() && (2..=8).contains(&tags[i].len()) && alnum(tags[i]) {
            i += 1;
        }
        if i == start {
            return None;
        }
        parts.singletons.push(singleton);
    }
    // private use
    if i < tags.len() {
        if !tags[i].eq_ignore_ascii_case("x") || !private_use(&tags[i + 1..]) {
            return None;
        }
        i = tags.len();
    }
    (i == tags.len()).then_some(parts)
}

/// The Lexicon `language` format: a well-formed BCP 47 tag (RFC 5646 §2.1): a `langtag` whose
/// primary subtag is 2–3 lowercase letters, a private-use tag (`x-…`, either case) or an
/// irregular grandfathered tag. Matches the interop fixtures (`jaja`, `JA` and `a-DE` are
/// rejected). Repeated variants or singletons are well-formed but not *valid*; see
/// [`is_valid_language`].
pub fn is_valid_language_syntax(s: &str) -> bool {
    if IRREGULAR_GRANDFATHERED.contains(&s) {
        return true;
    }
    let tags: Vec<&str> = s.split('-').collect();
    if tags[0].eq_ignore_ascii_case("x") {
        return private_use(&tags[1..]);
    }
    parse_langtag(s).is_some()
}

/// A well-formed language tag that is also *valid* per RFC 5646 §2.2.9: no repeated variant
/// (case-insensitive) and no repeated extension singleton.
pub fn is_valid_language(s: &str) -> bool {
    if !is_valid_language_syntax(s) {
        return false;
    }
    let Some(parts) = parse_langtag(s) else {
        return true; // grandfathered or private use
    };
    let mut vs: Vec<String> = parts
        .variants
        .iter()
        .map(|v| v.to_ascii_lowercase())
        .collect();
    vs.sort();
    let mut ss = parts.singletons.clone();
    ss.sort_unstable();
    vs.windows(2).all(|w| w[0] != w[1]) && ss.windows(2).all(|w| w[0] != w[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri() {
        for ok in [
            "https://example.com",
            "mailto:a@example.com",
            "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/a.b.c/x",
            "content-type:text/plan",
            "go://?Mercedes%20Benz",
        ] {
            assert!(is_valid_uri(ok), "{ok}");
        }
        for bad in [
            "",
            "https:",
            "HTTPS://example.com",
            "https://exa mple.com",
            "https://例え.jp/",
            "1http://x",
            ":x",
        ] {
            assert!(!is_valid_uri(bad), "{bad}");
        }
        let long = format!("https://example.com/{}", "x".repeat(MAX_URI_LEN));
        assert!(!is_valid_uri(&long));
    }

    #[test]
    fn language() {
        for ok in [
            "ja",
            "pt-BR",
            "zh-Hant",
            "es-419",
            "i-default",
            "x-private",
            "X-fr-CH",
            "de-CH-1901",
            "en-u-co-phonebk-t-en-US",
            "zh-min-nan",
        ] {
            assert!(is_valid_language_syntax(ok), "{ok}");
            assert!(is_valid_language(ok), "{ok}");
        }
        for bad in [
            "", "-", "jaja", "JA", "j", "ja-", "a-DE", "x", "en--US", "en-a", "en-x",
        ] {
            assert!(!is_valid_language_syntax(bad), "{bad}");
        }
        for dup in ["de-DE-1901-1901", "en-rozaj-ROZAJ", "en-a-foo-a-bar"] {
            assert!(is_valid_language_syntax(dup), "{dup}");
            assert!(!is_valid_language(dup), "{dup}");
        }
    }
}
