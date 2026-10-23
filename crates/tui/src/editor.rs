// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing in `$EDITOR` (`docs/design/synthesis-v1.md` §6.11, `threat-model.md` TERM-8,
//! TERM-9, FILE-5).
//!
//! - **Templates** ([`comment_template`], [`post_template`]): the text to send goes above a
//!   scissors line; everything from it down is ignored. Remote text (titles, names, the quoted
//!   comment) appears only below it, sanitised (`term_sanitize(Prose)`), on lines prefixed with
//!   `# `, with editor modelines neutralised ([`neutralise_modelines`]), and is followed by six
//!   fixed lines, so it never sits in the last five lines (vim modelines).
//! - **Files** are created with mode 0600 in a 0700 directory with `O_EXCL` and `O_NOFOLLOW`
//!   ([`lessdugin_native::fsutil`]), read back and removed after the editor exits.
//! - **The editor** is the first of `$VISUAL`, `$EDITOR`, `editor`, `nano`, `vi` that starts
//!   (`notepad` on Windows). `$VISUAL`/`$EDITOR` are split into words like a POSIX shell would,
//!   without any expansion ([`split_words`]); the file path is a separate argument. No shell.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::text::tt;

/// The scissors line (as git writes it).
pub const SCISSORS: &str = "# ------------------------ >8 ------------------------";

/// The largest file read back from the editor.
pub const MAX_TEXT_BYTES: u64 = 1 << 20;

/// How the editor is found and where its files go.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    /// The commands to try, in order (each split with [`split_words`]).
    pub chain: Vec<String>,
    /// The private directory for template files.
    pub dir: PathBuf,
}

impl EditorConfig {
    /// The chain from the process environment.
    pub fn from_env(dir: PathBuf) -> Self {
        Self::from_vars(dir, |k| std::env::var(k).ok())
    }

    /// The chain from `var` (`$VISUAL`, then `$EDITOR`), then the fallbacks.
    pub fn from_vars(dir: PathBuf, var: impl Fn(&str) -> Option<String>) -> Self {
        let mut chain: Vec<String> = ["VISUAL", "EDITOR"]
            .iter()
            .filter_map(|k| var(k))
            .filter(|s| !s.trim().is_empty())
            .collect();
        if cfg!(windows) {
            chain.push("notepad".into());
        } else {
            chain.extend(["editor", "nano", "vi"].map(String::from));
        }
        Self { chain, dir }
    }

    /// Writes `template` to a new private file, runs the editor on it and returns the file's
    /// text (the file is removed). An editor that exits with an error is a cancellation.
    pub fn edit(&self, template: &str) -> Result<String, String> {
        let path = self.create(template)?;
        let result = self.run(&path).and_then(|()| read_back(&path));
        let _ = std::fs::remove_file(&path);
        result
    }

    fn create(&self, template: &str) -> Result<PathBuf, String> {
        lessdugin_native::fsutil::ensure_private_dir(&self.dir)
            .map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        static N: AtomicU64 = AtomicU64::new(0);
        // the directory is private (0700) and the file is created with O_EXCL, so a predictable
        // name is safe; a name in use is skipped
        for _ in 0..64 {
            let name = format!(
                "lessdugin-{}-{}.md",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            );
            let path = self.dir.join(name);
            match lessdugin_native::fsutil::create_new_private(&path) {
                Ok(mut f) => {
                    f.write_all(template.as_bytes())
                        .and_then(|()| f.sync_all())
                        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("cannot create {}: {e}", path.display())),
            }
        }
        Err("cannot create a template file".into())
    }

    fn run(&self, path: &Path) -> Result<(), String> {
        for cmd in &self.chain {
            let Some(words) = split_words(cmd) else {
                continue;
            };
            let Some((program, args)) = words.split_first() else {
                continue;
            };
            let status = std::process::Command::new(program)
                .args(args)
                .arg(path)
                .status();
            match status {
                Ok(s) if s.success() => return Ok(()),
                Ok(s) => {
                    return Err(format!(
                        "the editor ({program}) exited with {s}; nothing was sent"
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("could not start the editor ({program}): {e}")),
            }
        }
        Err("no editor found; set $VISUAL or $EDITOR".into())
    }
}

fn read_back(path: &Path) -> Result<String, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("cannot read the text back: {e}"))?;
    let mut bytes = Vec::new();
    f.take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read the text back: {e}"))?;
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err("the text is too long".into());
    }
    String::from_utf8(bytes).map_err(|_| "the text is not UTF-8".into())
}

/// Splits a command into words like a POSIX shell, without expansion: blanks separate words,
/// `'…'` is literal, `"…"` keeps blanks (with `\"`, `\\`, `\$` and `` \` `` escapes), and `\`
/// escapes the next character outside quotes. `None` for an unterminated quote.
pub fn split_words(s: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match it.next()? {
                        '\'' => break,
                        c => cur.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match it.next()? {
                        '"' => break,
                        '\\' => match it.next()? {
                            c @ ('"' | '\\' | '$' | '`') => cur.push(c),
                            '\n' => {}
                            c => {
                                cur.push('\\');
                                cur.push(c);
                            }
                        },
                        c => cur.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match it.next() {
                    Some('\n') | None => {}
                    Some(c) => cur.push(c),
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

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

/// One line of remote text for a template: sanitised, on one line, modelines neutralised.
fn remote(s: &str) -> String {
    neutralise_modelines(&tt(s).to_plain_string())
}

/// The fixed lines that end every template (at least six, TERM-8).
fn tail(lines: [&str; 5]) -> String {
    let mut s = String::from("#\n");
    for l in lines {
        s.push_str("# ");
        s.push_str(l);
        s.push('\n');
    }
    s
}

fn head(prefill: &str) -> String {
    let mut s = String::new();
    let p = prefill.trim_end();
    if !p.is_empty() {
        s.push_str(p);
        s.push('\n');
    }
    s.push('\n');
    s.push_str(SCISSORS);
    s.push('\n');
    s.push_str("# Do not change or remove the line above; everything below it is ignored.\n");
    s
}

/// The template of a comment: `context` (e.g. who and what is replied to) and the `quoted`
/// text are remote and shown below the scissors line; `prefill` is the user's own text (kept
/// from an earlier attempt).
pub fn comment_template(prefill: &str, context: &[String], quoted: Option<&str>) -> String {
    let mut s = head(prefill);
    s.push_str("#\n");
    for c in context {
        s.push_str("# ");
        s.push_str(&remote(c));
        s.push('\n');
    }
    if let Some(q) = quoted {
        let q = q.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = q.trim_end().lines().collect();
        for l in lines.iter().take(40) {
            s.push_str("# > ");
            s.push_str(&remote(l));
            s.push('\n');
        }
        if lines.len() > 40 {
            s.push_str("# > ...\n");
        }
    }
    s.push_str(&tail([
        "Write above the scissors line, in markdown.",
        "An empty text cancels: nothing is sent.",
        "Save and close the editor to send it.",
        "It is queued and sent in the background.",
        "lessdugin",
    ]));
    s
}

/// The template of a new post in the forum named `forum` (remote).
pub fn post_template(prefill: &str, forum: &str) -> String {
    let mut s = head(prefill);
    s.push_str("#\n");
    s.push_str("# New post in: ");
    s.push_str(&remote(forum));
    s.push('\n');
    s.push_str(&tail([
        "The first line is the title; the rest is the body, in markdown.",
        "An empty file cancels: nothing is published.",
        "Save and close the editor to publish.",
        "It is queued and sent in the background.",
        "lessdugin",
    ]));
    s
}

/// The text above the scissors line (all of it if the line was removed), without trailing
/// blanks; `None` when nothing is left.
pub fn strip_template(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    let mut out = String::new();
    for line in text.split('\n') {
        if line.trim_end() == SCISSORS {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    let mut t = out.trim_end();
    // drop leading blank lines (not the indentation of the first line)
    while let Some((first, rest)) = t.split_once('\n') {
        if !first.trim().is_empty() {
            break;
        }
        t = rest;
    }
    if t.trim().is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

/// A post's `(title, body)` from the stripped text: the first line is the title (a leading
/// `#` heading marker is dropped), the rest the body.
pub fn split_post(text: &str) -> Result<(String, String), &'static str> {
    let text = text.trim_start();
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let title = first.trim().trim_start_matches('#').trim();
    if title.is_empty() {
        return Err("the first line, the title, is empty");
    }
    Ok((title.to_owned(), rest.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(split_words("vim").unwrap(), vec!["vim"]);
        assert_eq!(split_words("code --wait").unwrap(), vec!["code", "--wait"]);
        assert_eq!(
            split_words(r#"'/opt/my editor/bin/ed' -c "set tw=72" a\ b $HOME"#).unwrap(),
            vec!["/opt/my editor/bin/ed", "-c", "set tw=72", "a b", "$HOME"]
        );
        assert_eq!(split_words(r#""a\"b""#).unwrap(), vec![r#"a"b"#]);
        assert!(split_words("'open").is_none());
        assert!(split_words("   ").unwrap().is_empty());
    }

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

    #[test]
    fn templates_keep_remote_text_safe() {
        let t = comment_template(
            "",
            &["Replying to @bob in vim: set x:".into()],
            Some("hello\u{1b}[31m\nvi: bad:\n"),
        );
        let lines: Vec<&str> = t.lines().collect();
        let scissors = lines.iter().position(|l| *l == SCISSORS).unwrap();
        // everything after the scissors line is a comment line
        assert!(lines[scissors..].iter().all(|l| l.starts_with('#')), "{t}");
        // remote text never sits in the last six lines
        assert!(
            lines[lines.len() - 6..]
                .iter()
                .all(|l| !l.contains("bob") && !l.contains("hello")),
            "{t}"
        );
        assert!(!t.contains('\u{1b}'));
        assert!(t.contains("# > vi : bad:"), "{t}");
        assert!(t.contains("in vim : set x:"), "{t}");
        assert_eq!(strip_template(&t), None);
        let written = format!("My reply\n\nsecond paragraph\n{t}");
        assert_eq!(
            strip_template(&written).as_deref(),
            Some("My reply\n\nsecond paragraph")
        );
        let p = post_template("Title\n\nbody", "Meta");
        assert_eq!(strip_template(&p).as_deref(), Some("Title\n\nbody"));
    }

    #[test]
    fn posts() {
        assert_eq!(
            split_post("# Hello\n\nThe body.\n").unwrap(),
            ("Hello".to_owned(), "The body.".to_owned())
        );
        assert!(split_post("\n").is_err());
    }

    #[test]
    fn edit_round_trip_with_a_script() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("ed.sh");
        std::fs::write(&script, "#!/bin/sh\nprintf 'from the editor\\n' | cat - \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let cfg = EditorConfig::from_vars(dir.path().join("tmp"), |k| {
                (k == "EDITOR").then(|| script.display().to_string())
            });
            let out = cfg.edit(&comment_template("", &[], None)).unwrap();
            assert_eq!(strip_template(&out).as_deref(), Some("from the editor"));
            // the template file is gone and the directory is private
            let meta = std::fs::metadata(dir.path().join("tmp")).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o700);
            assert_eq!(
                std::fs::read_dir(dir.path().join("tmp")).unwrap().count(),
                0
            );
        }
    }
}
