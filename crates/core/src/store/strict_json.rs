// SPDX-License-Identifier: AGPL-3.0-or-later
//! A strict scan of raw JSON response bytes, run **before** serde parsing (`sync.md` §5.5,
//! step 2).
//!
//! `serde_json` silently keeps the last of two duplicate object keys and accepts numbers with a
//! fraction or an exponent, so record values are checked on the response bytes:
//! - **hard errors** ([`ScanError`]) reject the whole response: invalid UTF-8, malformed JSON
//!   (including lone surrogate escapes and control characters in strings), trailing bytes, and
//!   nesting deeper than the document limit (`NESTING_MAX_DOCUMENT`, R1);
//! - **issues** ([`Issue`]) are attributed to the place they occur, so a caller can drop just the
//!   affected record: duplicate keys (compared after unescaping), numbers with a fraction or an
//!   exponent (`1.0` and `1e3` included), and integers outside ±(2^53 − 1).
//!
//! Paths are kept only for the first `span_depth` levels (3 covers `records[i].value` of a
//! `listRecords` page); an issue deeper down carries the truncated path of its ancestor. The scan
//! also records the byte span of every value at levels 1..=`span_depth`, so callers can check the
//! size of a value "in its response encoding".
//!
//! The scanner recurses once per container level and checks the depth limit before descending;
//! `max_depth` is clamped to [`MAX_DEPTH_CAP`].

use std::collections::BTreeSet;

/// The largest integer allowed in a record: 2^53 − 1.
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The scanner never descends more than this many containers, whatever `max_depth` says.
pub const MAX_DEPTH_CAP: usize = 256;

/// One path segment.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Seg {
    /// An object member (the unescaped key).
    Key(String),
    /// An array item.
    Index(usize),
}

impl Seg {
    /// A key segment.
    pub fn key(k: &str) -> Self {
        Seg::Key(k.to_owned())
    }
}

/// What is wrong with a value that is still well-formed JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// An object has this key twice (or more).
    DuplicateKey(String),
    /// A number has a fraction or an exponent (no floats in atproto records).
    NotInteger,
    /// An integer outside ±(2^53 − 1).
    UnsafeInteger,
}

impl core::fmt::Display for IssueKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateKey(k) => write!(f, "duplicate key {k:?}"),
            Self::NotInteger => f.write_str("number with a fraction or exponent"),
            Self::UnsafeInteger => f.write_str("integer outside ±(2^53 − 1)"),
        }
    }
}

/// An issue and where it occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// The path of the value (for a duplicate key: of the object), truncated to `span_depth`.
    pub path: Vec<Seg>,
    /// Byte offset of the number or of the second key.
    pub offset: usize,
    /// The issue.
    pub kind: IssueKind,
}

/// The byte span of a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// The value's path (1..=`span_depth` segments).
    pub path: Vec<Seg>,
    /// First byte of the value.
    pub start: usize,
    /// One past its last byte.
    pub end: usize,
}

impl Span {
    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// `true` for an empty span (never produced for a value).
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// The result of a successful scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Spans of the values at levels 1..=`span_depth`, in document order.
    pub spans: Vec<Span>,
    /// Issues, in document order.
    pub issues: Vec<Issue>,
}

impl Scan {
    /// The span of the value at `path` (the first one, should a key repeat).
    pub fn span(&self, path: &[Seg]) -> Option<&Span> {
        self.spans.iter().find(|s| s.path == path)
    }

    /// Issues whose path starts with `prefix`.
    pub fn issues_under<'a>(&'a self, prefix: &'a [Seg]) -> impl Iterator<Item = &'a Issue> {
        self.issues
            .iter()
            .filter(move |i| i.path.starts_with(prefix))
    }

    /// `true` if there is no issue at all.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Errors that reject the whole document.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// Not JSON, or not UTF-8.
    #[error("invalid JSON at byte {offset}: {reason}")]
    Syntax {
        /// Byte offset of the problem.
        offset: usize,
        /// What is wrong.
        reason: &'static str,
    },
    /// Nesting deeper than the limit.
    #[error("JSON nesting exceeds depth {max}")]
    TooDeep {
        /// The limit.
        max: usize,
    },
}

/// Scans `bytes` as one JSON document: `max_depth` containers at most (the root container is
/// depth 1), paths and spans kept for `span_depth` levels.
pub fn scan(bytes: &[u8], max_depth: usize, span_depth: usize) -> Result<Scan, ScanError> {
    if let Err(e) = std::str::from_utf8(bytes) {
        return Err(ScanError::Syntax {
            offset: e.valid_up_to(),
            reason: "invalid UTF-8",
        });
    }
    let mut p = Parser {
        b: bytes,
        i: 0,
        max_depth: max_depth.min(MAX_DEPTH_CAP),
        span_depth,
        level: 0,
        path: Vec::new(),
        out: Scan::default(),
    };
    p.ws();
    p.value(0)?;
    p.ws();
    if p.i != bytes.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(p.out)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    max_depth: usize,
    span_depth: usize,
    /// Logical path length (may exceed `path.len()`).
    level: usize,
    path: Vec<Seg>,
    out: Scan,
}

impl Parser<'_> {
    fn err(&self, reason: &'static str) -> ScanError {
        ScanError::Syntax {
            offset: self.i,
            reason,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.i += 1;
        }
    }

    fn issue(&mut self, offset: usize, kind: IssueKind) {
        self.out.issues.push(Issue {
            path: self.path.clone(),
            offset,
            kind,
        });
    }

    /// Enters a member or item: keeps the segment only within `span_depth`.
    fn enter(&mut self, seg: impl FnOnce() -> Seg) {
        self.level += 1;
        if self.level <= self.span_depth {
            self.path.push(seg());
        }
    }

    fn leave(&mut self) {
        if self.level <= self.span_depth {
            self.path.pop();
        }
        self.level -= 1;
    }

    /// One value; `depth` is the number of containers around it.
    fn value(&mut self, depth: usize) -> Result<(), ScanError> {
        let start = self.i;
        match self.peek() {
            Some(b'{') => self.object(depth + 1)?,
            Some(b'[') => self.array(depth + 1)?,
            Some(b'"') => {
                self.string(false)?;
            }
            Some(b't') => self.literal(b"true")?,
            Some(b'f') => self.literal(b"false")?,
            Some(b'n') => self.literal(b"null")?,
            Some(b'-' | b'0'..=b'9') => self.number()?,
            Some(_) => return Err(self.err("unexpected character")),
            None => return Err(self.err("unexpected end of input")),
        }
        if self.level >= 1 && self.level <= self.span_depth {
            self.out.spans.push(Span {
                path: self.path.clone(),
                start,
                end: self.i,
            });
        }
        Ok(())
    }

    fn object(&mut self, depth: usize) -> Result<(), ScanError> {
        if depth > self.max_depth {
            return Err(ScanError::TooDeep {
                max: self.max_depth,
            });
        }
        self.i += 1; // '{'
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(());
        }
        let mut keys = BTreeSet::new();
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a string key"));
            }
            let key_at = self.i;
            let key = self.string(true)?.unwrap_or_default();
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected ':'"));
            }
            self.i += 1;
            self.ws();
            let seg_key = (self.level < self.span_depth).then(|| key.clone());
            if !keys.insert(key) {
                let dup = seg_key.clone().unwrap_or_else(|| {
                    // Re-decode for the message only (rare path).
                    self.decode_key_at(key_at)
                });
                self.issue(key_at, IssueKind::DuplicateKey(dup));
            }
            self.enter(|| Seg::Key(seg_key.unwrap_or_default()));
            let r = self.value(depth);
            self.leave();
            r?;
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn decode_key_at(&mut self, at: usize) -> String {
        let saved = self.i;
        self.i = at;
        let k = self.string(true).ok().flatten().unwrap_or_default();
        self.i = saved;
        k
    }

    fn array(&mut self, depth: usize) -> Result<(), ScanError> {
        if depth > self.max_depth {
            return Err(ScanError::TooDeep {
                max: self.max_depth,
            });
        }
        self.i += 1; // '['
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(());
        }
        let mut idx = 0usize;
        loop {
            self.ws();
            self.enter(|| Seg::Index(idx));
            let r = self.value(depth);
            self.leave();
            r?;
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                    idx += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn literal(&mut self, lit: &[u8]) -> Result<(), ScanError> {
        if self.b[self.i..].starts_with(lit) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while let Some(b'0'..=b'9') = self.peek() {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<(), ScanError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let int_start = self.i;
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("invalid number")),
        }
        let int_end = self.i;
        let mut integral = true;
        if self.peek() == Some(b'.') {
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.err("invalid number fraction"));
            }
            integral = false;
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.i += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("invalid number exponent"));
            }
            integral = false;
        }
        if !integral {
            self.issue(start, IssueKind::NotInteger);
            return Ok(());
        }
        let digits = &self.b[int_start..int_end];
        let safe = digits.len() <= 16
            && std::str::from_utf8(digits)
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .is_some_and(|n| n <= MAX_SAFE_INTEGER);
        if !safe {
            self.issue(start, IssueKind::UnsafeInteger);
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, ScanError> {
        let h = self
            .b
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("truncated \\u escape"))?;
        let mut v = 0u32;
        for &c in h {
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => return Err(self.err("invalid \\u escape")),
            };
            v = v * 16 + u32::from(d);
        }
        self.i += 4;
        Ok(v)
    }

    /// A string; returns its unescaped value if `decode`.
    fn string(&mut self, decode: bool) -> Result<Option<String>, ScanError> {
        self.i += 1; // '"'
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err("unterminated string"));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\\' => {
                    self.i += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.err("unterminated escape"));
                    };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = match hi {
                                0xD800..=0xDBFF => {
                                    if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                                        return Err(self.err("lone surrogate escape"));
                                    }
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..=0xDFFF).contains(&lo) {
                                        return Err(self.err("lone surrogate escape"));
                                    }
                                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                                }
                                0xDC00..=0xDFFF => {
                                    return Err(self.err("lone surrogate escape"));
                                }
                                _ => hi,
                            };
                            char::from_u32(cp).ok_or_else(|| self.err("invalid code point"))?
                        }
                        _ => return Err(self.err("invalid escape")),
                    };
                    if decode {
                        let mut tmp = [0u8; 4];
                        buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                    }
                }
                0x00..=0x1F => return Err(self.err("control character in string")),
                _ => {
                    if decode {
                        buf.push(c);
                    }
                    self.i += 1;
                }
            }
        }
        if decode {
            // The input is valid UTF-8 and escapes add whole characters.
            String::from_utf8(buf)
                .map(Some)
                .map_err(|_| self.err("invalid UTF-8 in string"))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ok(s: &str) -> Scan {
        scan(s.as_bytes(), 36, 3).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn kinds(s: &str) -> Vec<IssueKind> {
        ok(s).issues.into_iter().map(|i| i.kind).collect()
    }

    #[test]
    fn accepts_plain_json() {
        for s in [
            "{}",
            "[]",
            " {\"a\" : [1, -2, 0, -0, true, false, null, \"x\\n\\u00e9\\ud83d\\ude00\"]} ",
            "\"just a string\"",
            "9007199254740991",
            "-9007199254740991",
            "{\"a\":{\"b\":{\"c\":{}}}}",
        ] {
            assert!(ok(s).is_clean(), "{s}");
        }
    }

    #[test]
    fn flags_floats_and_big_integers() {
        assert_eq!(kinds("[1.0]"), vec![IssueKind::NotInteger]);
        assert_eq!(kinds("[1e3]"), vec![IssueKind::NotInteger]);
        assert_eq!(kinds("[-1E-3]"), vec![IssueKind::NotInteger]);
        assert_eq!(kinds("[0.5, 2]"), vec![IssueKind::NotInteger]);
        assert_eq!(kinds("[9007199254740992]"), vec![IssueKind::UnsafeInteger]);
        assert_eq!(kinds("[-9007199254740992]"), vec![IssueKind::UnsafeInteger]);
        assert_eq!(
            kinds("[123456789012345678901234567890]"),
            vec![IssueKind::UnsafeInteger]
        );
    }

    #[test]
    fn flags_duplicate_keys_after_unescaping() {
        assert_eq!(
            kinds(r#"{"a":1,"a":2}"#),
            vec![IssueKind::DuplicateKey("a".into())]
        );
        assert_eq!(
            kinds(r#"{"a":1,"\u0061":2}"#),
            vec![IssueKind::DuplicateKey("a".into())]
        );
        // deep duplicate: the message still names the key
        let deep = r#"{"a":{"b":{"c":{"d":{"k":1,"k":2}}}}}"#;
        let s = ok(deep);
        assert_eq!(s.issues.len(), 1);
        assert_eq!(s.issues[0].kind, IssueKind::DuplicateKey("k".into()));
        assert_eq!(
            s.issues[0].path,
            vec![Seg::key("a"), Seg::key("b"), Seg::key("c")]
        );
        // distinct keys that differ only after escaping are fine
        assert!(ok(r#"{"a":1,"A":2,"\u00e9":3,"é ":4}"#).is_clean());
    }

    #[test]
    fn attributes_issues_to_records() {
        let page = r#"{"cursor":"c","records":[
            {"uri":"u0","cid":"c0","value":{"x":1}},
            {"uri":"u1","cid":"c1","value":{"x":1.5}},
            {"uri":"u2","uri":"dup","value":{}}
        ]}"#;
        let s = ok(page);
        let rec = |i| vec![Seg::key("records"), Seg::Index(i)];
        assert_eq!(s.issues_under(&rec(0)).count(), 0);
        assert_eq!(s.issues_under(&rec(1)).count(), 1);
        assert_eq!(
            s.issues_under(&rec(1)).next().unwrap().path,
            vec![Seg::key("records"), Seg::Index(1), Seg::key("value")]
        );
        assert_eq!(s.issues_under(&rec(2)).count(), 1);
        let v0 = s
            .span(&[Seg::key("records"), Seg::Index(0), Seg::key("value")])
            .unwrap();
        assert_eq!(&page[v0.start..v0.end], r#"{"x":1}"#);
        assert_eq!(v0.len(), 7);
        let c = s.span(&[Seg::key("cursor")]).unwrap();
        assert_eq!(&page[c.start..c.end], "\"c\"");
        // nothing deeper than 3 levels is recorded
        assert!(s.spans.iter().all(|sp| sp.path.len() <= 3));
    }

    #[test]
    fn rejects_malformed() {
        for s in [
            "",
            "{",
            "{\"a\"}",
            "{\"a\":1,}",
            "[1,]",
            "[01]",
            "[1.]",
            "[.5]",
            "[1e]",
            "[+1]",
            "[tru]",
            "{\"a\":1} x",
            "{a:1}",
            "\"unterminated",
            "\"bad \\x escape\"",
            "\"lone \\ud800\"",
            "\"lone \\udc00\"",
            "\"pair \\ud800\\u0041\"",
            "\"ctl \u{1}\"",
            "[NaN]",
            "\u{feff}{}",
        ] {
            assert!(
                matches!(scan(s.as_bytes(), 36, 3), Err(ScanError::Syntax { .. })),
                "{s:?}"
            );
        }
        assert!(matches!(
            scan(b"[\"\xff\"]", 36, 3),
            Err(ScanError::Syntax {
                reason: "invalid UTF-8",
                ..
            })
        ));
    }

    #[test]
    fn depth_limit() {
        let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(scan(nest(36).as_bytes(), 36, 3).is_ok());
        assert_eq!(
            scan(nest(37).as_bytes(), 36, 3),
            Err(ScanError::TooDeep { max: 36 })
        );
        // a huge nesting fails fast without recursing that deep
        let huge = "[".repeat(1_000_000);
        assert_eq!(
            scan(huge.as_bytes(), 36, 3),
            Err(ScanError::TooDeep { max: 36 })
        );
        let objs = format!("{}1{}", "{\"a\":".repeat(37), "}".repeat(37));
        assert_eq!(
            scan(objs.as_bytes(), 36, 3),
            Err(ScanError::TooDeep { max: 36 })
        );
    }

    fn arb_json() -> impl Strategy<Value = serde_json::Value> {
        let leaf = prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            (-(MAX_SAFE_INTEGER as i64)..=(MAX_SAFE_INTEGER as i64)).prop_map(|i| i.into()),
            ".{0,8}".prop_map(serde_json::Value::String),
        ];
        leaf.prop_recursive(5, 64, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::Array),
                proptest::collection::btree_map(".{0,6}", inner, 0..6)
                    .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
            ]
        })
    }

    proptest! {
        /// Everything serde_json writes from integer-only values scans clean, and the spans of
        /// top-level members re-parse to the member values.
        #[test]
        fn serde_output_scans_clean(v in arb_json()) {
            let text = serde_json::to_string(&v).unwrap();
            let s = scan(text.as_bytes(), 64, 3).unwrap();
            prop_assert!(s.is_clean(), "{text}: {:?}", s.issues);
            if let serde_json::Value::Object(m) = &v {
                for (k, member) in m {
                    let sp = s.span(&[Seg::Key(k.clone())]).unwrap();
                    let back: serde_json::Value =
                        serde_json::from_slice(&text.as_bytes()[sp.start..sp.end]).unwrap();
                    prop_assert_eq!(&back, member);
                }
            }
        }

        /// A document the scanner accepts without issues is one serde_json accepts too (the
        /// scanner may be stricter, never laxer).
        #[test]
        fn never_accepts_what_serde_rejects(
            bytes in proptest::collection::vec(
                prop_oneof![
                    Just(b'{'), Just(b'}'), Just(b'['), Just(b']'), Just(b'"'), Just(b':'),
                    Just(b','), Just(b'1'), Just(b'-'), Just(b'.'), Just(b'e'), Just(b'\\'),
                    Just(b'u'), Just(b'a'), Just(b' '), any::<u8>()
                ],
                0..24,
            )
        ) {
            if let Ok(s) = scan(&bytes, 128, 3)
                && s.is_clean()
            {
                prop_assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_ok());
            }
        }
    }
}
