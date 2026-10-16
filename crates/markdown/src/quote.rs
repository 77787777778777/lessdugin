// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deterministic text-quote resolution against the projection P (`docs/spec/lfm-1.md` §11).
//! No fuzzy matching: an exact (normalised) occurrence, disambiguated by prefix/suffix
//! agreement, then by distance to the `start` hint, then by position.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// A `defs#textQuote` (borrowed form).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextQuote<'a> {
    /// The quoted text.
    pub exact: &'a str,
    /// Text immediately before `exact` (verbatim substring of P).
    pub prefix: Option<&'a str>,
    /// Text immediately after `exact`.
    pub suffix: Option<&'a str>,
    /// Code-point offset hint.
    pub start: Option<u64>,
}

/// An owned `defs#textQuote`, as built for writers by [`build_quote`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedTextQuote {
    /// The quoted text.
    pub exact: String,
    /// Up to 32 grapheme clusters before `exact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Up to 32 grapheme clusters after `exact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    /// Code-point offset of `exact` in P.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
}

impl OwnedTextQuote {
    /// Borrow as a [`TextQuote`].
    pub fn as_quote(&self) -> TextQuote<'_> {
        TextQuote {
            exact: &self.exact,
            prefix: self.prefix.as_deref(),
            suffix: self.suffix.as_deref(),
            start: self.start,
        }
    }
}

/// The target body's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteTarget<'a> {
    /// P of the target's current version (`""` for a target without a body).
    Text(&'a str),
    /// The body is a `textBlob` that is not available yet.
    Pending,
}

/// The result of [`resolve_quote`]. Offsets are code points in T.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum QuoteResolution {
    /// The quote is at `start..end`.
    Anchored {
        /// Start offset (code points).
        start: u64,
        /// End offset (code points).
        end: u64,
    },
    /// The quoted text is no longer present.
    Orphaned,
    /// The target body is still loading.
    Pending,
}

/// Maximum quote size in bytes after normalisation.
pub const MAX_QUOTE_BYTES: usize = 2000;
/// Only this prefix of T is searched.
pub const MAX_TARGET_BYTES: usize = 200_000;
/// At most this many occurrences are scored.
pub const MAX_OCCURRENCES: usize = 1000;

/// `norm(x)` (§11): NFC, then White_Space runs → U+0020 (not trimmed).
fn norm(x: &str) -> String {
    let mut out = String::with_capacity(x.len());
    let mut ws = false;
    for c in x.nfc() {
        if c.is_whitespace() {
            if !ws {
                out.push(' ');
            }
            ws = true;
        } else {
            out.push(c);
            ws = false;
        }
    }
    out
}

/// `qkey(quote)` (§11): `norm(trim_ws(exact))`, or `""` without a quote.
pub fn qkey(exact: Option<&str>) -> String {
    exact.map(|e| norm(e.trim())).unwrap_or_default()
}

/// All overlapping byte offsets of `needle` in `hay` (KMP), at most `cap`. Linear time.
fn find_all(hay: &[u8], needle: &[u8], cap: usize) -> Vec<usize> {
    let n = needle.len();
    let mut out = Vec::new();
    if n == 0 || n > hay.len() {
        return out;
    }
    let mut fail = vec![0usize; n];
    let mut k = 0usize;
    for i in 1..n {
        while k > 0 && needle[i] != needle[k] {
            k = fail[k - 1];
        }
        if needle[i] == needle[k] {
            k += 1;
        }
        fail[i] = k;
    }
    k = 0;
    for (i, &b) in hay.iter().enumerate() {
        while k > 0 && b != needle[k] {
            k = fail[k - 1];
        }
        if b == needle[k] {
            k += 1;
        }
        if k == n {
            out.push(i + 1 - n);
            if out.len() == cap {
                break;
            }
            k = fail[k - 1];
        }
    }
    out
}

fn lcs_suffix(a: &str, b: &str) -> u64 {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .count() as u64
}

fn lcp_prefix(a: &str, b: &str) -> u64 {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() as u64
}

/// Resolve a quote against its target (§11).
pub fn resolve_quote(quote: &TextQuote<'_>, target: QuoteTarget<'_>) -> QuoteResolution {
    let QuoteTarget::Text(t) = target else {
        return QuoteResolution::Pending;
    };
    let q = norm(quote.exact.trim());
    let pf = norm(quote.prefix.unwrap_or(""));
    let sf = norm(quote.suffix.unwrap_or(""));
    if q.is_empty() || q.len() > MAX_QUOTE_BYTES {
        return QuoteResolution::Orphaned;
    }
    let mut cut = t.len().min(MAX_TARGET_BYTES);
    while !t.is_char_boundary(cut) {
        cut -= 1;
    }
    let tp = &t[..cut];
    let offs = find_all(tp.as_bytes(), q.as_bytes(), MAX_OCCURRENCES);
    if offs.is_empty() {
        return QuoteResolution::Orphaned;
    }
    // Byte offsets → code-point offsets (ascending, so one forward scan).
    let mut cps = Vec::with_capacity(offs.len());
    let (mut byte, mut cp) = (0usize, 0u64);
    for &o in &offs {
        cp += tp[byte..o].chars().count() as u64;
        byte = o;
        cps.push(cp);
    }
    let qlen = q.chars().count() as u64;
    let best = if offs.len() == 1 {
        0
    } else {
        let mut best = 0usize;
        let mut best_key: Option<(u64, u64, u64)> = None; // (score, distance, offset) → max, min, min
        for (j, &o) in offs.iter().enumerate() {
            let score = lcs_suffix(&pf, &tp[..o]) + lcp_prefix(&sf, &tp[o + q.len()..]);
            let dist = quote.start.map_or(0, |s| s.abs_diff(cps[j]));
            let better = match best_key {
                None => true,
                Some((bs, bd, bo)) => {
                    score > bs || (score == bs && (dist < bd || (dist == bd && cps[j] < bo)))
                }
            };
            if better {
                best = j;
                best_key = Some((score, dist, cps[j]));
            }
        }
        best
    };
    QuoteResolution::Anchored {
        start: cps[best],
        end: cps[best] + qlen,
    }
}

/// CLI `--quote "…"` (§11): the first occurrence of the normalised text in `t`.
pub fn find_first(text: &str, t: &str) -> Option<(u64, u64)> {
    let q = norm(text.trim());
    if q.is_empty() || q.len() > MAX_QUOTE_BYTES {
        return None;
    }
    let o = t.find(q.as_str())?;
    let start = t[..o].chars().count() as u64;
    Some((start, start + q.chars().count() as u64))
}

/// Errors from [`build_quote`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuoteBuildError {
    /// The selection is out of range or not on code-point boundaries.
    #[error("selection out of range")]
    Range,
    /// The trimmed selection is empty or longer than 2,000 bytes.
    #[error("quote must be 1-2000 bytes")]
    Size,
}

/// Writers (§11, `data-model.md` W8): build a quote from a selection `[start, end)` (code
/// points) of T. The selection is trimmed to start and end on a non-space; prefix and suffix
/// are up to 32 grapheme clusters each, shortened from the far end to at most 640 bytes.
pub fn build_quote(t: &str, start: usize, end: usize) -> Result<OwnedTextQuote, QuoteBuildError> {
    let idx: Vec<usize> = t
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(t.len()))
        .collect();
    if start > end || end >= idx.len() {
        return Err(QuoteBuildError::Range);
    }
    let (mut b0, mut b1) = (idx[start], idx[end]);
    let sel = &t[b0..b1];
    let lead = sel.len() - sel.trim_start().len();
    b0 += lead;
    b1 -= t[b0..b1].len() - t[b0..b1].trim_end().len();
    let exact = &t[b0..b1];
    if exact.is_empty() || exact.len() > MAX_QUOTE_BYTES {
        return Err(QuoteBuildError::Size);
    }
    let before: Vec<&str> = t[..b0].graphemes(true).rev().take(32).collect();
    let mut before: Vec<&str> = before.into_iter().rev().collect();
    while before.iter().map(|g| g.len()).sum::<usize>() > 640 {
        before.remove(0);
    }
    let mut after: Vec<&str> = t[b1..].graphemes(true).take(32).collect();
    while after.iter().map(|g| g.len()).sum::<usize>() > 640 {
        after.pop();
    }
    let prefix = before.concat();
    let suffix = after.concat();
    Ok(OwnedTextQuote {
        exact: exact.to_owned(),
        prefix: (!prefix.is_empty()).then_some(prefix),
        suffix: (!suffix.is_empty()).then_some(suffix),
        start: Some(t[..b0].chars().count() as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(exact: &str) -> TextQuote<'_> {
        TextQuote {
            exact,
            ..Default::default()
        }
    }

    #[test]
    fn basic() {
        let t = "the cat sat on the mat";
        assert_eq!(
            resolve_quote(&q("cat"), QuoteTarget::Text(t)),
            QuoteResolution::Anchored { start: 4, end: 7 }
        );
        assert_eq!(
            resolve_quote(&q("dog"), QuoteTarget::Text(t)),
            QuoteResolution::Orphaned
        );
        assert_eq!(
            resolve_quote(&q("  "), QuoteTarget::Text(t)),
            QuoteResolution::Orphaned
        );
        assert_eq!(
            resolve_quote(&q("cat"), QuoteTarget::Pending),
            QuoteResolution::Pending
        );
        // two occurrences: lowest offset by default, suffix decides otherwise
        assert_eq!(
            resolve_quote(&q("the"), QuoteTarget::Text(t)),
            QuoteResolution::Anchored { start: 0, end: 3 }
        );
        let with_sf = TextQuote {
            exact: "the",
            suffix: Some(" mat"),
            ..Default::default()
        };
        assert_eq!(
            resolve_quote(&with_sf, QuoteTarget::Text(t)),
            QuoteResolution::Anchored { start: 15, end: 18 }
        );
        let with_start = TextQuote {
            exact: "the",
            start: Some(14),
            ..Default::default()
        };
        assert_eq!(
            resolve_quote(&with_start, QuoteTarget::Text(t)),
            QuoteResolution::Anchored { start: 15, end: 18 }
        );
    }

    #[test]
    fn overlapping() {
        assert_eq!(find_all(b"aaaa", b"aa", 10), vec![0, 1, 2]);
        assert_eq!(find_all(b"abababa", b"aba", 2), vec![0, 2]);
    }

    #[test]
    fn build() {
        let t = "one two three four";
        let b = build_quote(t, 3, 8).unwrap();
        assert_eq!(b.exact, "two");
        assert_eq!(b.prefix.as_deref(), Some("one "));
        assert_eq!(b.suffix.as_deref(), Some(" three four"));
        assert_eq!(b.start, Some(4));
        assert_eq!(
            resolve_quote(&b.as_quote(), QuoteTarget::Text(t)),
            QuoteResolution::Anchored { start: 4, end: 7 }
        );
    }
}
