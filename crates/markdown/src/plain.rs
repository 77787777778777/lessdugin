// SPDX-License-Identifier: AGPL-3.0-or-later
//! Plain-text projection P (`docs/spec/lfm-1.md` §10): P, P_x, the excerpt, word count, read
//! time, heading text and heading slugs.

use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::event::{Ev, Tag, TagKind};

/// General_Category ∈ {Lu, Ll, Lt, Lm, Lo, Nd, Nl, No}.
pub(crate) fn is_letter_or_number(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    )
}

/// The §10.1 event table over `evs`, without normalisation. With `skip_spoilers`, events
/// strictly inside a `Spoiler` contribute nothing (P_x, §10.3).
pub(crate) fn plain_raw(evs: &[Ev], skip_spoilers: bool) -> String {
    let mut out = String::new();
    let mut spoiler_depth = 0usize;
    let mut captions: Vec<&str> = Vec::new();
    for e in evs {
        if skip_spoilers {
            match e {
                Ev::Start(Tag::Spoiler) => {
                    if spoiler_depth == 0 {
                        out.push('\n');
                    }
                    spoiler_depth += 1;
                    continue;
                }
                Ev::End(TagKind::Spoiler) => {
                    spoiler_depth -= 1;
                    if spoiler_depth == 0 {
                        out.push('\n');
                    }
                    continue;
                }
                _ if spoiler_depth > 0 => continue,
                _ => {}
            }
        }
        match e {
            Ev::Text(t) | Ev::Literal(t) | Ev::Code(t) | Ev::InlineMath(t) | Ev::DisplayMath(t) => {
                out.push_str(t)
            }
            Ev::Start(Tag::Figure(c)) => {
                captions.push(c);
                out.push('\n');
            }
            Ev::End(TagKind::Figure) => {
                out.push('\n');
                out.push_str(captions.pop().unwrap_or(""));
                out.push('\n');
            }
            Ev::Start(t) => match t.kind() {
                TagKind::TableCell => out.push(' '),
                k if contributes_newline(k) => out.push('\n'),
                _ => {}
            },
            Ev::End(k) => match k {
                TagKind::TableCell => out.push(' '),
                k if contributes_newline(*k) => out.push('\n'),
                _ => {}
            },
            Ev::SoftBreak => out.push(' '),
            Ev::HardBreak | Ev::Rule => out.push('\n'),
            Ev::FnRef(_) | Ev::Task(_) => {}
        }
    }
    out
}

fn contributes_newline(k: TagKind) -> bool {
    matches!(
        k,
        TagKind::Paragraph
            | TagKind::Heading
            | TagKind::BlockQuote
            | TagKind::Spoiler
            | TagKind::Details
            | TagKind::Summary
            | TagKind::CodeBlock
            | TagKind::List
            | TagKind::Item
            | TagKind::FootnoteDef
            | TagKind::Table
            | TagKind::TableHead
            | TagKind::TableRow
    )
}

/// §10.2: NFC, White_Space runs → one U+0020, strip one leading and one trailing U+0020.
pub fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_ws = false;
    for c in raw.nfc() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    let s = out.strip_prefix(' ').unwrap_or(&out);
    let s = s.strip_suffix(' ').unwrap_or(s);
    s.to_owned()
}

/// The excerpt of P_x (§10.4): at most 500 grapheme clusters and 5,000 bytes.
pub fn excerpt(x: &str) -> String {
    let mut clusters = 0usize;
    let mut over = false;
    let mut head_end = 0usize; // longest prefix with <= 499 clusters and <= 4997 bytes
    for (i, g) in x.grapheme_indices(true) {
        clusters += 1;
        let end = i + g.len();
        if clusters <= 499 && end <= 4997 {
            head_end = end;
        } else if clusters > 500 {
            over = true;
            break;
        }
    }
    if !over && x.len() <= 5000 {
        return x.to_owned();
    }
    let mut head = &x[..head_end];
    if let Some(k) = head.rfind(' ')
        && k > 0
    {
        head = &head[..k];
    }
    format!("{head}\u{2026}")
}

/// Word count over P (§10.5).
pub fn word_count(p: &str) -> u32 {
    if p.is_empty() {
        0
    } else {
        u32::try_from(p.bytes().filter(|&b| b == b' ').count() + 1).unwrap_or(u32::MAX)
    }
}

/// Read time in minutes (§10.5), 250 words per minute.
pub fn read_minutes(word_count: u32) -> u32 {
    if word_count == 0 {
        0
    } else {
        ((u64::from(word_count) + 125) / 250).max(1) as u32
    }
}

/// The heading slug of `proj` (`plain` of the heading's inline events, §4.6).
pub(crate) fn slug(proj: &str) -> String {
    let nfc: String = proj.nfc().collect();
    nfc.to_lowercase()
        .chars()
        .filter(|&c| c == ' ' || c == '-' || c == '_' || is_letter_or_number(c))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// P and P_x of a source, computed exactly as [`crate::render`] does.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PlainProjection {
    /// P (§10.1–10.2).
    pub text: String,
    /// P_x (§10.3): P without spoiler content.
    pub text_x: String,
}

/// Compute P and P_x for `src` (§10). Sources over 1,000,000 bytes give empty strings, like
/// [`crate::render`].
pub fn plain_projection(src: &str) -> PlainProjection {
    if src.len() > crate::render::MAX_SOURCE_BYTES {
        return PlainProjection {
            text: String::new(),
            text_x: String::new(),
        };
    }
    let s3 = crate::lfm::parse_to_s3(src).events;
    PlainProjection {
        text: normalize(&plain_raw(&s3, false)),
        text_x: normalize(&plain_raw(&s3, true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(normalize("\n a \u{3000}\u{a0} b\n"), "a b");
        assert_eq!(normalize("e\u{301}"), "\u{e9}");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn excerpt_caps() {
        let s = "a ".repeat(250);
        let s = s.trim_end();
        assert_eq!(excerpt(s), s);
        let long = "ab ".repeat(300);
        let e = excerpt(long.trim_end());
        assert!(e.ends_with('\u{2026}'));
        assert!(e.graphemes(true).count() <= 500);
        let nospace = "x".repeat(600);
        let e = excerpt(&nospace);
        assert_eq!(e.graphemes(true).count(), 500);
        // 41-byte clusters: the byte cap binds first.
        let heavy = format!("e{}", "\u{301}".repeat(20)).repeat(200);
        let e = excerpt(&heavy);
        assert_eq!(e.len(), 121 * 41 + 3);
    }

    #[test]
    fn counts() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("a b c"), 3);
        assert_eq!(read_minutes(0), 0);
        assert_eq!(read_minutes(1), 1);
        assert_eq!(read_minutes(374), 1);
        assert_eq!(read_minutes(375), 2);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  a  b "), "--a--b-");
        assert_eq!(slug("Ünïcödé 😀 x_y-z"), "ünïcödé--x_y-z");
    }
}
