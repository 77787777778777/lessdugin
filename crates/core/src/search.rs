// SPDX-License-Identifier: AGPL-3.0-or-later
//! Text matching and snippets for local search over synced records.

const CONTEXT: usize = 60;
const SNIPPET: usize = 160;

/// A case-folded string and, for each folded code point, its source code-point offset.
struct Folded {
    text: String,
    origin: Vec<usize>,
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

/// Matches of `needle` in `haystack`, as non-overlapping source code-point ranges. A case-fold
/// expansion (for example `İ`) highlights the whole source character, never a byte offset into it.
fn matches(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let h = folded(haystack);
    let n = folded(needle);
    if n.text.is_empty() {
        return Vec::new();
    }
    let source_len = haystack.chars().count();
    let mut out = Vec::new();
    for (byte, _) in h.text.match_indices(&n.text) {
        let start = h.text[..byte].chars().count();
        let end = start + n.text.chars().count();
        let Some(&first) = h.origin.get(start) else {
            continue;
        };
        let last = h.origin.get(end - 1).copied().unwrap_or(first);
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

/// A snippet around the first case-insensitive term match and code-point highlight ranges into
/// that snippet (including its ellipses). Whitespace-separated terms are ANDed, in any order;
/// every one is highlighted. At most 160 source code points are copied, with up to 60 before
/// the first match.
pub fn snippet(haystack: &str, needle: &str) -> Option<(String, Vec<(u32, u32)>)> {
    let terms: Vec<_> = needle.split_whitespace().collect();
    if terms.is_empty() {
        return None;
    }
    let term_matches: Vec<_> = terms.iter().map(|term| matches(haystack, term)).collect();
    if term_matches.iter().any(Vec::is_empty) {
        return None;
    }
    let first = term_matches
        .iter()
        .flatten()
        .copied()
        .min_by_key(|range| range.0)?;
    let chars: Vec<char> = haystack.chars().collect();
    let start = first.0.saturating_sub(CONTEXT);
    let end = (start + SNIPPET).min(chars.len());
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.extend(&chars[start..end]);
    if end < chars.len() {
        snippet.push('…');
    }
    let mut highlights: Vec<_> = terms
        .iter()
        .flat_map(|term| matches(&snippet, term))
        .filter_map(|(start, end)| Some((u32::try_from(start).ok()?, u32::try_from(end).ok()?)))
        .collect();
    highlights.sort_unstable();
    highlights.dedup();
    Some((snippet, highlights))
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
