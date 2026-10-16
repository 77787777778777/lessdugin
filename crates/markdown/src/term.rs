// SPDX-License-Identifier: AGPL-3.0-or-later
//! Terminal output: [`TermText`] (sanitised runs) and [`term_sanitize`] (`docs/spec/lfm-1.md` §9.1).
//!
//! Every piece of text that reaches a terminal — rendered markdown, names, handles, error
//! messages, `--raw` lines — goes through [`term_sanitize`]. Its output can never contain a C0
//! control, DEL, a C1 control, U+2028/U+2029 or a bidi control, so content cannot inject escape
//! sequences, reorder text or move the cursor. Printers add their own SGR/OSC 8 sequences around
//! these runs and render [`RunKind::Escaped`] runs in reverse video.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Whether a run is content as written, or a visible stand-in for something that was not safe
/// to print (for example `^[` for ESC).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunKind {
    /// Text as written (after whitespace mapping).
    Plain,
    /// A visible replacement for a control or invisible character; printed in reverse video.
    Escaped,
}

/// One run of sanitised text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TermRun {
    text: String,
    kind: RunKind,
}

impl TermRun {
    /// The run's text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The run's kind.
    pub fn kind(&self) -> RunKind {
        self.kind
    }
}

/// Text that is safe to print to a terminal. Only [`term_sanitize`] (and the safe combinators
/// on this type) can construct it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TermText {
    runs: Vec<TermRun>,
}

impl TermText {
    /// The empty text.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The runs, in order. Adjacent runs always have different kinds.
    pub fn runs(&self) -> &[TermRun] {
        &self.runs
    }

    /// True if there is no text.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The concatenated text of all runs (escaped runs included as their visible stand-ins).
    pub fn to_plain_string(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    /// Display width in terminal columns (`unicode-width`, non-CJK).
    pub fn width(&self) -> usize {
        self.runs.iter().map(|r| r.text.width()).sum()
    }

    /// Append another sanitised text. Concatenation preserves the invariant.
    pub fn append(&mut self, other: TermText) {
        for run in other.runs {
            self.push(run.kind, &run.text);
        }
    }

    /// A text consisting of fixed ASCII produced by our own code (labels, separators).
    ///
    /// # Panics
    /// Panics if `s` contains anything other than printable ASCII; use [`term_sanitize`] for
    /// anything that is not a compile-time constant.
    pub fn from_static(s: &'static str) -> Self {
        assert!(
            s.bytes().all(|b| (0x20..0x7f).contains(&b)),
            "TermText::from_static requires printable ASCII"
        );
        let mut t = Self::empty();
        t.push(RunKind::Plain, s);
        t
    }

    fn push(&mut self, kind: RunKind, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.runs.last_mut() {
            Some(last) if last.kind == kind => last.text.push_str(s),
            _ => self.runs.push(TermRun { text: s.to_owned(), kind }),
        }
    }

    fn push_char(&mut self, kind: RunKind, c: char) {
        let mut buf = [0u8; 4];
        self.push(kind, c.encode_utf8(&mut buf));
    }
}

impl std::fmt::Display for TermText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for r in &self.runs {
            f.write_str(&r.text)?;
        }
        Ok(())
    }
}

/// The sanitising context (`lfm-1.md` §9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermCtx {
    /// Text, literal HTML, details summaries, error messages.
    Prose,
    /// Code blocks, inline code, math TeX, `--raw` lines. Invisible characters become visible.
    Code,
    /// Display names, handles, titles, forum and tag names, react labels, media names.
    /// Whitespace collapses; invisible characters and badge glyphs are removed; at most
    /// `max_graphemes` extended grapheme clusters are kept (`…` marks truncation).
    Name {
        /// Maximum number of grapheme clusters (including the `…`). `0` means unlimited.
        max_graphemes: usize,
    },
}

/// Glyphs reserved for our own verification badges; never shown inside names.
pub const BADGE_GLYPHS: [char; 10] = [
    '\u{2611}', '\u{2612}', '\u{2705}', '\u{2713}', '\u{2714}', '\u{2715}', '\u{2716}', '\u{2717}',
    '\u{2718}', '\u{26A0}',
];

fn bidi_name(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{061C}' => "ALM",
        '\u{200E}' => "LRM",
        '\u{200F}' => "RLM",
        '\u{202A}' => "LRE",
        '\u{202B}' => "RLE",
        '\u{202C}' => "PDF",
        '\u{202D}' => "LRO",
        '\u{202E}' => "RLO",
        '\u{2066}' => "LRI",
        '\u{2067}' => "RLI",
        '\u{2068}' => "FSI",
        '\u{2069}' => "PDI",
        _ => return None,
    })
}

fn zero_width_name(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{200B}' => "ZWSP",
        '\u{200C}' => "ZWNJ",
        '\u{200D}' => "ZWJ",
        '\u{2060}' => "WJ",
        '\u{FEFF}' => "BOM",
        _ => return None,
    })
}

/// Unicode `White_Space` (the full property list).
fn is_white_space(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{0085}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
    )
}

fn c0_caret(c: char) -> String {
    // c is in U+0000..=U+001F: '^' + chr(c + 0x40), e.g. ESC -> "^[".
    let b = (c as u32 as u8) + 0x40;
    format!("^{}", b as char)
}

fn c1_escape(c: char) -> String {
    format!("\\u{{{:x}}}", c as u32)
}

/// Sanitise `input` for a terminal in the given context (`docs/spec/lfm-1.md` §9.1).
///
/// Callers split multi-line input into lines (after normalising CRLF and CR to LF) before calling;
/// an LF that still reaches this function becomes a space.
pub fn term_sanitize(input: &str, ctx: TermCtx) -> TermText {
    let mut out = TermText::empty();
    let mut column = 0usize; // display width so far (Code context tab stops)
    for c in input.chars() {
        match ctx {
            TermCtx::Name { .. } => sanitize_name_char(&mut out, c),
            TermCtx::Prose => sanitize_prose_char(&mut out, c),
            TermCtx::Code => column += sanitize_code_char(&mut out, c, column),
        }
    }
    if let TermCtx::Name { max_graphemes } = ctx {
        out = finish_name(out, max_graphemes);
    }
    out
}

/// Characters handled identically in every context: C0 (other than the whitespace handled by
/// the caller), DEL and C1. Returns true if `c` was consumed.
fn sanitize_controls(out: &mut TermText, c: char) -> bool {
    match c {
        '\u{0000}'..='\u{001F}' => {
            out.push(RunKind::Escaped, &c0_caret(c));
            true
        }
        '\u{007F}' => {
            out.push(RunKind::Escaped, "^?");
            true
        }
        '\u{0080}'..='\u{009F}' => {
            out.push(RunKind::Escaped, &c1_escape(c));
            true
        }
        _ => false,
    }
}

fn sanitize_prose_char(out: &mut TermText, c: char) {
    match c {
        '\t' | '\n' | '\u{2028}' | '\u{2029}' => out.push(RunKind::Plain, " "),
        '\r' => out.push(RunKind::Escaped, "^M"),
        _ if bidi_name(c).is_some() => {}
        _ if sanitize_controls(out, c) => {}
        _ => out.push_char(RunKind::Plain, c),
    }
}

/// Returns the display width appended (for tab stops).
fn sanitize_code_char(out: &mut TermText, c: char, column: usize) -> usize {
    let (kind, piece): (RunKind, String) = match c {
        '\t' => (RunKind::Plain, " ".repeat(4 - (column % 4))),
        '\n' | '\u{2028}' | '\u{2029}' => (RunKind::Plain, " ".to_owned()),
        '\r' => (RunKind::Escaped, "^M".to_owned()),
        '\u{0000}'..='\u{001F}' => (RunKind::Escaped, c0_caret(c)),
        '\u{007F}' => (RunKind::Escaped, "^?".to_owned()),
        '\u{0080}'..='\u{009F}' => (RunKind::Escaped, c1_escape(c)),
        _ => match bidi_name(c).or_else(|| zero_width_name(c)) {
            Some(name) => (RunKind::Escaped, format!("\u{27E8}{name}\u{27E9}")),
            None => (RunKind::Plain, c.to_string()),
        },
    };
    out.push(kind, &piece);
    piece.width()
}

fn sanitize_name_char(out: &mut TermText, c: char) {
    if is_white_space(c) {
        out.push(RunKind::Plain, " ");
    } else if bidi_name(c).is_some() || zero_width_name(c).is_some() || BADGE_GLYPHS.contains(&c)
    {
        // removed
    } else if !sanitize_controls(out, c) {
        out.push_char(RunKind::Plain, c);
    }
}

/// Collapse runs of spaces in plain runs, trim, and apply the grapheme cap.
fn finish_name(text: TermText, max_graphemes: usize) -> TermText {
    // Collapse and trim across run boundaries.
    let mut collapsed = TermText::empty();
    let mut pending_space = false;
    let mut started = false;
    for run in text.runs {
        match run.kind {
            RunKind::Escaped => {
                if pending_space && started {
                    collapsed.push(RunKind::Plain, " ");
                }
                pending_space = false;
                collapsed.push(RunKind::Escaped, &run.text);
                started = true;
            }
            RunKind::Plain => {
                for c in run.text.chars() {
                    if c == ' ' {
                        pending_space = true;
                    } else {
                        if pending_space && started {
                            collapsed.push(RunKind::Plain, " ");
                        }
                        pending_space = false;
                        collapsed.push_char(RunKind::Plain, c);
                        started = true;
                    }
                }
            }
        }
    }
    if max_graphemes == 0 {
        return collapsed;
    }
    let total: usize = collapsed.runs.iter().map(|r| r.text.graphemes(true).count()).sum();
    if total <= max_graphemes {
        return collapsed;
    }
    let mut keep = max_graphemes - 1;
    let mut truncated = TermText::empty();
    for run in collapsed.runs {
        if keep == 0 {
            break;
        }
        let gs: Vec<&str> = run.text.graphemes(true).take(keep).collect();
        keep -= gs.len();
        truncated.push(run.kind, &gs.concat());
    }
    truncated.push(RunKind::Plain, "\u{2026}");
    truncated
}

/// Escape characters that are dangerous on a terminal inside already-serialised JSON
/// (`--json` on a TTY, `lfm-1.md` §9.4): C1 controls, U+2028, U+2029 and bidi controls become
/// `\uXXXX`. `serde_json` already escapes C0. When output is piped, callers must not use this.
pub fn escape_json_for_tty(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        let dangerous = matches!(c, '\u{0080}'..='\u{009F}' | '\u{2028}' | '\u{2029}')
            || bidi_name(c).is_some()
            || c == '\u{007F}';
        if dangerous {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: &TermText) -> String {
        t.to_plain_string()
    }

    fn assert_invariant(t: &TermText) {
        for c in s(t).chars() {
            assert!(
                !matches!(c, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}' | '\u{2028}' | '\u{2029}')
                    && bidi_name(c).is_none(),
                "invariant violated by {c:?} in {:?}",
                s(t)
            );
        }
    }

    #[test]
    fn prose_escapes_controls() {
        let t = term_sanitize("a\u{1b}[31mred\u{7}b\rc\u{85}d\u{9b}e", TermCtx::Prose);
        assert_eq!(s(&t), "a^[[31mred^Gb^Mc\\u{85}d\\u{9b}e");
        assert_invariant(&t);
        assert!(t.runs().iter().any(|r| r.kind() == RunKind::Escaped && r.text() == "^["));
    }

    #[test]
    fn prose_whitespace_and_bidi() {
        let t = term_sanitize("a\tb\nc\u{2028}d\u{202E}e\u{200B}f\u{00A0}g", TermCtx::Prose);
        assert_eq!(s(&t), "a b c de\u{200B}f\u{00A0}g");
        assert_invariant(&t);
    }

    #[test]
    fn vertical_tab_and_form_feed_are_escaped() {
        let t = term_sanitize("a\u{b}b\u{c}c", TermCtx::Prose);
        assert_eq!(s(&t), "a^Kb^Lc");
        assert_invariant(&t);
    }

    #[test]
    fn code_tabs_and_visible_invisibles() {
        let t = term_sanitize("ab\tc\td", TermCtx::Code);
        assert_eq!(s(&t), "ab  c   d");
        let t = term_sanitize("x\u{202E}y\u{200D}z", TermCtx::Code);
        assert_eq!(s(&t), "x\u{27E8}RLO\u{27E9}y\u{27E8}ZWJ\u{27E9}z");
        assert_invariant(&t);
    }

    #[test]
    fn name_collapses_and_strips() {
        let t = term_sanitize(
            "  alice\u{2705} \u{200B}\u{202E} the\t\tgreat  ",
            TermCtx::Name { max_graphemes: 0 },
        );
        assert_eq!(s(&t), "alice the great");
        assert_invariant(&t);
    }

    #[test]
    fn name_grapheme_cap() {
        let t = term_sanitize("abcdef", TermCtx::Name { max_graphemes: 4 });
        assert_eq!(s(&t), "abc\u{2026}");
        let t = term_sanitize("abcd", TermCtx::Name { max_graphemes: 4 });
        assert_eq!(s(&t), "abcd");
        // family emoji is one grapheme
        let t = term_sanitize("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}xyz", TermCtx::Name {
            max_graphemes: 3,
        });
        // ZWJ is removed in names, so the family splits into three graphemes.
        assert_eq!(t.to_plain_string().graphemes(true).count(), 3);
    }

    #[test]
    fn json_tty_escape() {
        assert_eq!(escape_json_for_tty("\"a\u{202E}b\u{85}\""), "\"a\\u202eb\\u0085\"");
    }

    #[test]
    #[should_panic]
    fn from_static_rejects_controls() {
        let _ = TermText::from_static("bad\u{1b}");
    }

    #[test]
    fn exhaustive_invariant_over_bmp_controls() {
        for ctx in [TermCtx::Prose, TermCtx::Code, TermCtx::Name { max_graphemes: 0 }] {
            let all: String = (0u32..0x3000).filter_map(char::from_u32).collect();
            assert_invariant(&term_sanitize(&all, ctx));
        }
    }
}
