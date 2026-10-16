// SPDX-License-Identifier: AGPL-3.0-or-later
//! DNS-over-HTTPS JSON (`application/dns-json`) for `_atproto.<handle>` TXT lookups
//! (`sync.md` §3.1).
//!
//! `txt_strings(r)` takes every `Answer[]` entry with `type = 16`; if its `data` begins with `"`,
//! it is parsed as a sequence of quoted character-strings (handling `\"`, `\\` and `\DDD`), which
//! are concatenated; otherwise `data` is taken as is. A response whose `Status` is neither 0
//! (NOERROR) nor 3 (NXDOMAIN) is a transport failure.

use lessdugin_atp::syntax::Did;

/// The CORS-safelisted `accept` value for DoH JSON.
pub const DNS_JSON: &str = "application/dns-json";
/// The TXT record type.
pub const TYPE_TXT: u64 = 16;

/// Why a DoH response is unusable (a transport failure: the next provider is tried).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DohError {
    /// Not a JSON object.
    #[error("DoH response is not a JSON object")]
    NotJson,
    /// `Status` is missing or not an integer.
    #[error("DoH response has no Status")]
    NoStatus,
    /// `Status` is neither NOERROR nor NXDOMAIN.
    #[error("DNS status {0}")]
    Status(i64),
}

/// The query URL `<endpoint>?name=<qname>&type=TXT` (parameters percent-encoded).
pub fn query_url(endpoint: &url::Url, qname: &str) -> Result<url::Url, url::ParseError> {
    let qs = lessdugin_atp::xrpc::query_string(&[("name", qname), ("type", "TXT")]);
    let sep = if endpoint.query().is_some() { '&' } else { '?' };
    url::Url::parse(&format!("{}{sep}{qs}", endpoint.as_str()))
}

/// The TXT strings of a DoH JSON response. NXDOMAIN gives an empty list.
pub fn parse_txt_answer(body: &[u8]) -> Result<Vec<String>, DohError> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| DohError::NotJson)?;
    if !v.is_object() {
        return Err(DohError::NotJson);
    }
    let status = v
        .get("Status")
        .and_then(serde_json::Value::as_i64)
        .ok_or(DohError::NoStatus)?;
    match status {
        0 => {}
        3 => return Ok(Vec::new()),
        s => return Err(DohError::Status(s)),
    }
    let answers = v.get("Answer").and_then(serde_json::Value::as_array);
    Ok(answers
        .into_iter()
        .flatten()
        .filter(|a| a.get("type").and_then(serde_json::Value::as_u64) == Some(TYPE_TXT))
        .filter_map(|a| a.get("data").and_then(serde_json::Value::as_str))
        .filter_map(txt_data)
        .collect())
}

/// Decodes the `data` of one TXT answer. `None` if quoted data is malformed.
pub fn txt_data(data: &str) -> Option<String> {
    if !data.starts_with('"') {
        return Some(data.to_owned());
    }
    let b = data.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => i += 1,
            b'"' => {
                i += 1;
                loop {
                    let c = *b.get(i)?;
                    i += 1;
                    match c {
                        b'"' => break,
                        b'\\' => {
                            let e = *b.get(i)?;
                            if e.is_ascii_digit() {
                                let d = b.get(i..i + 3)?;
                                if !d.iter().all(u8::is_ascii_digit) {
                                    return None;
                                }
                                let v =
                                    d.iter().fold(0u32, |acc, x| acc * 10 + u32::from(x - b'0'));
                                out.push(u8::try_from(v).ok()?);
                                i += 3;
                            } else {
                                out.push(e);
                                i += 1;
                            }
                        }
                        c => out.push(c),
                    }
                }
            }
            _ => return None,
        }
    }
    String::from_utf8(out).ok()
}

/// The DIDs named by a set of TXT strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxtDids {
    /// No `did=` string with a valid DID.
    None,
    /// Exactly one distinct DID.
    One(Did),
    /// More than one distinct DID: the handle is invalid.
    Many,
}

/// `{ s[4..] : s starts with "did=", s[4..] is a valid DID (data-model.md §2.3) }`.
pub fn dids_from_txt(txts: &[String]) -> TxtDids {
    let mut found: Option<Did> = None;
    for s in txts {
        let Some(rest) = s.strip_prefix("did=") else {
            continue;
        };
        let Ok(did) = Did::parse(rest) else { continue };
        if did.check_lessdugin_policy().is_err() {
            continue;
        }
        match &found {
            None => found = Some(did),
            Some(f) if *f == did => {}
            Some(_) => return TxtDids::Many,
        }
    }
    found.map_or(TxtDids::None, TxtDids::One)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLC: &str = "did:plc:z72i7hdynmk6r22z27h6tvur";
    const PLC2: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

    #[test]
    fn quoted_strings() {
        assert_eq!(
            txt_data(r#""did=did:plc:abc""#).as_deref(),
            Some("did=did:plc:abc")
        );
        assert_eq!(
            txt_data("did=did:plc:abc").as_deref(),
            Some("did=did:plc:abc")
        );
        assert_eq!(
            txt_data(r#""did=did:pl" "c:abc""#).as_deref(),
            Some("did=did:plc:abc")
        );
        assert_eq!(txt_data(r#""a\"b\\c\065""#).as_deref(), Some("a\"b\\cA"));
        assert_eq!(txt_data(r#""unterminated"#), None);
        assert_eq!(txt_data(r#""bad\25""#), None);
        assert_eq!(txt_data(r#""x\999""#), None);
        assert_eq!(txt_data(r#""a" junk"#), None);
    }

    #[test]
    fn answers() {
        let body = format!(
            r#"{{"Status":0,"TC":false,"Answer":[
              {{"name":"_atproto.bsky.app.","type":16,"TTL":300,"data":"\"did={PLC}\""}},
              {{"name":"x","type":5,"data":"cname.example."}},
              {{"name":"y","type":16,"data":"other text"}}]}}"#
        );
        let txts = parse_txt_answer(body.as_bytes()).unwrap();
        assert_eq!(txts, vec![format!("did={PLC}"), "other text".to_owned()]);
        assert_eq!(dids_from_txt(&txts), TxtDids::One(Did::parse(PLC).unwrap()));
        assert_eq!(
            parse_txt_answer(br#"{"Status":3}"#).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_txt_answer(br#"{"Status":0}"#).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_txt_answer(br#"{"Status":2}"#),
            Err(DohError::Status(2))
        );
        assert_eq!(
            parse_txt_answer(br#"{"Answer":[]}"#),
            Err(DohError::NoStatus)
        );
        assert_eq!(parse_txt_answer(b"[]"), Err(DohError::NotJson));
        assert_eq!(parse_txt_answer(b"<html>"), Err(DohError::NotJson));
    }

    #[test]
    fn did_sets() {
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let one = Did::parse(PLC).unwrap();
        assert_eq!(dids_from_txt(&s(&[])), TxtDids::None);
        assert_eq!(
            dids_from_txt(&s(&["v=spf1", "did= did:plc:x"])),
            TxtDids::None
        );
        // duplicates of the same DID are one DID
        let a = format!("did={PLC}");
        assert_eq!(dids_from_txt(&s(&[&a, &a])), TxtDids::One(one));
        assert_eq!(
            dids_from_txt(&s(&[&a, &format!("did={PLC2}")])),
            TxtDids::Many
        );
        // exact prefix, valid lessdugin DIDs only
        assert_eq!(dids_from_txt(&s(&[&format!("DID={PLC}")])), TxtDids::None);
        assert_eq!(dids_from_txt(&s(&["did=did:key:zQ3abc"])), TxtDids::None);
        assert_eq!(dids_from_txt(&s(&["did=did:plc:tooshort"])), TxtDids::None);
        assert_eq!(
            dids_from_txt(&s(&["did=did:web:example.com:8080"])),
            TxtDids::None
        );
        assert!(matches!(
            dids_from_txt(&s(&["did=did:web:example.com"])),
            TxtDids::One(_)
        ));
    }

    #[test]
    fn urls() {
        let e = url::Url::parse("https://cloudflare-dns.com/dns-query").unwrap();
        assert_eq!(
            query_url(&e, "_atproto.alice.com").unwrap().as_str(),
            "https://cloudflare-dns.com/dns-query?name=_atproto.alice.com&type=TXT"
        );
        let e = url::Url::parse("https://dns.example/q?ct=json").unwrap();
        assert_eq!(
            query_url(&e, "_atproto.a.b").unwrap().as_str(),
            "https://dns.example/q?ct=json&name=_atproto.a.b&type=TXT"
        );
    }
}
