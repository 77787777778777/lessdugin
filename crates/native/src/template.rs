// SPDX-License-Identifier: AGPL-3.0-or-later
//! Editor template helpers shared by the CLI and the TUI (`threat-model.md` TERM-8).
//!
//! A file opened in `$EDITOR` must not let text that came from the network configure the
//! editor: vim reads modelines from the first and last five lines of a file (`modelines`), and
//! Emacs reads `-*- … -*-` and `Local Variables:`. Templates therefore keep remote text away
//! from those lines (fixed lines at both ends, [`SCISSORS`] before a fixed tail), and lines of
//! remote text that must appear are passed through [`neutralise_modelines`].

/// The scissors line (as git writes it): a template's text ends above it, and everything from
/// it down is ignored.
pub const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// Lines vim reads modelines from at each end of a file (its `modelines` default).
pub const MODELINE_LINES: usize = 5;

/// Neutralises editor modelines in a line of remote text (TERM-8): a space before the `:` of
/// every `(?i)(^|\s)(vi|vim|ex)([<=>]?[0-9]*)?:` and `(?i)local variables:`, and `-*-` becomes
/// `- * -`.
pub fn neutralise_modelines(line: &str) -> String {
    let s = line.replace("-*-", "- * -");
    let chars: Vec<char> = s.chars().collect();
    let mut colons = Vec::new(); // char indices of colons to push right
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let at = |i: usize, w: &str| -> bool {
        w.chars()
            .enumerate()
            .all(|(k, c)| lower.get(i + k) == Some(&c))
    };
    for i in 0..chars.len() {
        if at(i, "local variables:") {
            colons.push(i + "local variables".len());
        }
        if i > 0 && !chars[i - 1].is_whitespace() {
            continue;
        }
        for word in ["vim", "vi", "ex"] {
            if !at(i, word) {
                continue;
            }
            let mut j = i + word.len();
            if matches!(chars.get(j), Some('<' | '=' | '>')) {
                j += 1;
            }
            while chars.get(j).is_some_and(char::is_ascii_digit) {
                j += 1;
            }
            if chars.get(j) == Some(&':') {
                colons.push(j);
                break;
            }
        }
    }
    let mut out = String::with_capacity(s.len() + colons.len());
    for (i, c) in chars.iter().enumerate() {
        if colons.contains(&i) {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modelines() {
        assert_eq!(neutralise_modelines("vim: set ft=sh:"), "vim : set ft=sh:");
        assert_eq!(neutralise_modelines("x vi=7:y ex:z"), "x vi=7 :y ex :z");
        assert_eq!(neutralise_modelines("VIM>12:"), "VIM>12 :");
        assert_eq!(neutralise_modelines("savvi:"), "savvi:");
        assert_eq!(
            neutralise_modelines("Local Variables: -*- mode: x -*-"),
            "Local Variables : - * - mode: x - * -"
        );
    }
}
