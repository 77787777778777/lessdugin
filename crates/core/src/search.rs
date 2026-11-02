// SPDX-License-Identifier: AGPL-3.0-or-later
//! Text matching and snippets for local search over synced records.
//!
//! Every function here is linear in the length of the text it reads (plus, for highlights,
//! the 160-code-point snippet), however many times a term occurs: a body made of one short word
//! repeated must not cost the engine more than one pass over it per term.

pub(crate) mod index;

const CONTEXT: usize = 60;
const SNIPPET: usize = 160;

/// A case-folded string and, for each folded code point, its source code-point offset.
struct Folded {
    text: String,
    origin: Vec<usize>,
}

impl Folded {
    /// The source code-point offset of the folded byte offset `byte` (a char boundary).
    fn source_of(&self, byte: usize) -> Option<usize> {
        self.origin.get(self.text[..byte].chars().count()).copied()
    }
}

fn folded(source: &str) -> Folded {
    let mut text = String::new();
    let mut origin = Vec::new();
    for (i, c) in source.chars().enumerate() {
        for lower in c.to_lowercase() {
            text.push(lower);
            origin.push(i);
        }
    }
    Folded { text, origin }
}

/// `s` case-folded code point by code point, as the text is folded for matching.
pub(crate) fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// The whitespace-separated terms of a query, case-folded.
pub(crate) fn terms(q: &str) -> Vec<String> {
    q.split_whitespace().map(fold).collect()
}

/// Matches of the folded `term` in `h`, as non-overlapping source code-point ranges. A case-fold
/// expansion (for example `İ`) highlights the whole source character, never a byte offset into it.
fn matches(h: &Folded, source_len: usize, term: &str) -> Vec<(usize, usize)> {
    let len = term.chars().count();
    if len == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // `match_indices` yields byte offsets in increasing order: the code points between two
    // matches are counted once, so k matches cost one pass over the text, not k.
    let (mut byte_at, mut char_at) = (0, 0);
    for (byte, _) in h.text.match_indices(term) {
        char_at += h.text[byte_at..byte].chars().count();
        byte_at = byte;
        let Some(&first) = h.origin.get(char_at) else {
            continue;
        };
        let last = h.origin.get(char_at + len - 1).copied().unwrap_or(first);
        let range = (first, (last + 1).min(source_len));
        if out
            .last()
            .is_none_or(|(_, previous_end)| range.0 >= *previous_end)
        {
            out.push(range);
        }
    }
    out
}

/// `true` if every folded term occurs in `text`, case-insensitively.
pub(crate) fn matches_all(text: &str, terms: &[String]) -> bool {
    let text = fold(text);
    terms.iter().all(|t| text.contains(t.as_str()))
}

/// A snippet of `shown` around the first match of any folded term, with code-point highlight
/// ranges of every term into the snippet (including its ellipses, never on them). At most 160
/// source code points are copied, with up to 60 before the first match; when no term occurs in
/// `shown` (every match was in text it leaves out) the snippet is its start, with no highlights.
pub(crate) fn snippet_of(shown: &str, terms: &[String]) -> (String, Vec<(u32, u32)>) {
    let h = folded(shown);
    let first = terms
        .iter()
        .filter_map(|t| h.text.find(t.as_str()))
        .min()
        .and_then(|byte| h.source_of(byte));
    let len = shown.chars().count();
    let start = first.map_or(0, |f| f.saturating_sub(CONTEXT));
    let end = (start + SNIPPET).min(len);
    let window: String = shown.chars().skip(start).take(end - start).collect();
    let lead = usize::from(start > 0);
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.push_str(&window);
    if end < len {
        snippet.push('…');
    }
    let mut highlights = Vec::new();
    if first.is_some() {
        let w = folded(&window);
        let window_len = end - start;
        highlights = terms
            .iter()
            .flat_map(|term| matches(&w, window_len, term))
            .filter_map(|(s, e)| {
                Some((u32::try_from(s + lead).ok()?, u32::try_from(e + lead).ok()?))
            })
            .collect();
        highlights.sort_unstable();
        highlights.dedup();
    }
    (snippet, highlights)
}

/// A search hit: `None` unless every whitespace-separated term of `q` occurs in `matched`
/// (case-insensitively, in any order); then a snippet cut from `shown` ([`snippet_of`]).
/// Search matches title + P but shows title + P_x, so that a snippet never reveals a spoiler
/// (`lfm-1.md` §12).
pub fn hit(matched: &str, shown: &str, q: &str) -> Option<(String, Vec<(u32, u32)>)> {
    let terms = terms(q);
    if terms.is_empty() || !matches_all(matched, &terms) {
        return None;
    }
    Some(snippet_of(shown, &terms))
}

/// [`hit`] with the snippet cut from the matched text itself.
pub fn snippet(haystack: &str, needle: &str) -> Option<(String, Vec<(u32, u32)>)> {
    hit(haystack, haystack, needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_offsets_are_code_points() {
        let (s, h) = snippet("αβ Café café", "CAFÉ").unwrap();
        assert_eq!(s, "αβ Café café");
        assert_eq!(h, vec![(3, 7), (8, 12)]);
        assert_eq!(snippet("nothing here", "café"), None);
    }

    #[test]
    fn terms_are_anded_in_any_order_and_all_are_highlighted() {
        let (text, highlights) = snippet("αβ shared Café notes", "café shared").unwrap();
        assert_eq!(text, "αβ shared Café notes");
        assert_eq!(highlights, vec![(3, 9), (10, 14)]);
        assert_eq!(snippet("Café only", "café missing"), None);
        assert_eq!(snippet("Café only", "   "), None);
    }

    #[test]
    fn case_fold_expansion_does_not_split_a_character() {
        let (s, h) = snippet("xİy", "i").unwrap();
        assert_eq!(s, "xİy");
        assert_eq!(h, vec![(1, 2)]);
    }
}
