// SPDX-License-Identifier: AGPL-3.0-or-later
//! Output (`threat-model.md` TERM-1, TERM-3, TERM-7).
//!
//! - Human output is built from [`TermText`] only: fixed labels via [`TermText::from_static`],
//!   everything else via [`term_sanitize`]. The printer adds SGR sequences (bold, dim, colours,
//!   reverse video for escaped runs) and nothing else, and only when colour is on.
//! - `--json` writes a DTO to stdout; on a TTY, C1 controls, U+2028/2029 and bidi controls are
//!   escaped as `\uXXXX` ([`escape_json_for_tty`]); piped output is byte-exact.
//! - Errors go to stderr: a sanitised one-line message, or the `EngineError` JSON with `--json`.

use std::io::{IsTerminal, Write};

use lessdugin_core::engine::EngineError;
use serde::Serialize;

pub use lessdugin_markdown::term::{
    RunKind, TermCtx, TermText, escape_json_for_tty, term_sanitize,
};

use crate::args::ColorChoice;

/// Sanitised prose (messages, URLs, identifiers).
pub fn tt(s: &str) -> TermText {
    term_sanitize(s, TermCtx::Prose)
}

/// A sanitised name (handles, display names): whitespace collapsed, invisibles removed.
pub fn name(s: &str) -> TermText {
    term_sanitize(s, TermCtx::Name { max_graphemes: 256 })
}

/// A fixed label.
pub fn lit(s: &'static str) -> TermText {
    TermText::from_static(s)
}

/// Concatenates texts.
pub fn cat(parts: impl IntoIterator<Item = TermText>) -> TermText {
    let mut t = TermText::empty();
    for p in parts {
        t.append(p);
    }
    t
}

/// A style the printer may apply (SGR only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// No attributes.
    Plain,
    /// Bold.
    Bold,
    /// Dim.
    Dim,
    /// Green (success).
    Good,
    /// Yellow (warnings).
    Warn,
    /// Red (failures).
    Bad,
}

impl Style {
    fn sgr(self) -> &'static str {
        match self {
            Self::Plain => "",
            Self::Bold => "\x1b[1m",
            Self::Dim => "\x1b[2m",
            Self::Good => "\x1b[32m",
            Self::Warn => "\x1b[33m",
            Self::Bad => "\x1b[31m",
        }
    }
}

/// Renders sanitised text with an optional style. Escaped runs are shown in reverse video.
pub fn render(t: &TermText, style: Style, color: bool) -> String {
    let mut out = String::new();
    for run in t.runs() {
        match (run.kind(), color) {
            (_, false) => out.push_str(run.text()),
            (RunKind::Plain, true) if style == Style::Plain => out.push_str(run.text()),
            (RunKind::Plain, true) => {
                out.push_str(style.sgr());
                out.push_str(run.text());
                out.push_str("\x1b[0m");
            }
            (RunKind::Escaped, true) => {
                out.push_str("\x1b[7m");
                out.push_str(run.text());
                out.push_str("\x1b[0m");
            }
        }
    }
    out
}

/// One styled piece of a line.
pub type Span = (TermText, Style);

/// The printer.
#[derive(Clone, Debug)]
pub struct Printer {
    json: bool,
    color_out: bool,
    color_err: bool,
    stdout_tty: bool,
    quiet: bool,
}

fn color_enabled(choice: ColorChoice, tty: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            tty && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
        }
    }
}

impl Printer {
    /// A printer for these options.
    pub fn new(json: bool, color: ColorChoice, quiet: bool) -> Self {
        let stdout_tty = std::io::stdout().is_terminal();
        let stderr_tty = std::io::stderr().is_terminal();
        Self {
            json,
            color_out: color_enabled(color, stdout_tty),
            color_err: color_enabled(color, stderr_tty),
            stdout_tty,
            quiet,
        }
    }

    /// True with `--json`.
    pub fn is_json(&self) -> bool {
        self.json
    }

    /// Writes a DTO as JSON to stdout.
    pub fn json<T: Serialize + ?Sized>(&self, value: &T) {
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into());
        let text = if self.stdout_tty {
            escape_json_for_tty(&text)
        } else {
            text
        };
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
    }

    /// Writes one line of styled spans to stdout.
    pub fn line(&self, spans: &[Span]) {
        let s: String = spans
            .iter()
            .map(|(t, st)| render(t, *st, self.color_out))
            .collect();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{s}");
    }

    /// `  label  value` on stdout, the label dimmed and padded to `width`.
    pub fn field(&self, label: &'static str, width: usize, value: TermText) {
        let pad = " ".repeat(width.saturating_sub(label.len()));
        self.line(&[
            (lit("  "), Style::Plain),
            (lit(label), Style::Dim),
            (tt(&pad), Style::Plain),
            (lit("  "), Style::Plain),
            (value, Style::Plain),
        ]);
    }

    /// A progress or hint line on stderr (suppressed by `-q`).
    pub fn note(&self, spans: &[Span]) {
        if self.quiet {
            return;
        }
        self.err_line(spans);
    }

    /// A line on stderr (never suppressed: prompts and URLs the user must see).
    pub fn err_line(&self, spans: &[Span]) {
        let s: String = spans
            .iter()
            .map(|(t, st)| render(t, *st, self.color_err))
            .collect();
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{s}");
    }

    /// `warning: …` on stderr.
    pub fn warn(&self, msg: TermText) {
        if self.quiet {
            return;
        }
        self.err_line(&[(lit("warning: "), Style::Warn), (msg, Style::Plain)]);
    }

    /// Reports an engine error on stderr.
    pub fn error(&self, e: &EngineError) {
        if self.json {
            let text = serde_json::to_string(e).unwrap_or_default();
            let text = if std::io::stderr().is_terminal() {
                escape_json_for_tty(&text)
            } else {
                text
            };
            let _ = writeln!(std::io::stderr().lock(), "{text}");
            return;
        }
        let msg = lessdugin_native::auth::redact::redact_secrets(&e.message);
        self.err_line(&[(lit("error: "), Style::Bad), (tt(&msg), Style::Plain)]);
        if let Some(hint) = hint_for(e) {
            self.err_line(&[(lit("hint: "), Style::Dim), (lit(hint), Style::Plain)]);
        }
    }

    /// A usage error on stderr.
    pub fn usage_error(&self, msg: &str) {
        if self.json {
            let v = serde_json::json!({"code": "usage", "message": msg, "retryable": false});
            let _ = writeln!(std::io::stderr().lock(), "{v}");
            return;
        }
        self.err_line(&[(lit("error: "), Style::Bad), (tt(msg), Style::Plain)]);
    }
}

fn hint_for(e: &EngineError) -> Option<&'static str> {
    use lessdugin_core::engine::ErrorCode as C;
    let kind = e
        .detail
        .as_ref()
        .and_then(|d| d.get("kind"))
        .and_then(|k| k.as_str());
    match (e.code, kind) {
        (_, Some("issuerChanged")) => {
            Some("run `lessdugin login` interactively to confirm the new authorization server")
        }
        (_, Some("notAnAppPassword")) => {
            Some("create an app password in your PDS's settings (Settings > App passwords)")
        }
        (C::AuthRequired, _) => Some("log in with `lessdugin login <handle>`"),
        (C::Network, _) => Some("check your connection, or run `lessdugin doctor`"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_only_emits_our_sgr() {
        let t = tt("a\u{1b}]0;title\u{7}b\u{9b}c");
        let plain = render(&t, Style::Bold, false);
        assert!(
            !plain.contains('\u{1b}') && !plain.contains('\u{9b}'),
            "{plain:?}"
        );
        let colored = render(&t, Style::Bold, true);
        // Every ESC in the output starts one of our SGR sequences.
        for (i, _) in colored.match_indices('\u{1b}') {
            let rest = &colored[i..];
            assert!(
                ["\x1b[0m", "\x1b[1m", "\x1b[7m"]
                    .iter()
                    .any(|p| rest.starts_with(p)),
                "{colored:?}"
            );
        }
        assert!(colored.contains("\x1b[7m^[\x1b[0m"));
    }
}
