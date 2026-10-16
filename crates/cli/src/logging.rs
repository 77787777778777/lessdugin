// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `tracing` subscriber (`threat-model.md` TERM-6, AUTH-10): log lines go to stderr through
//! a writer that redacts secrets and applies `term_sanitize(Prose)` to every line, so neither
//! server-controlled text nor tokens reach the terminal raw. ANSI output of the formatter is
//! off; the sanitiser would show it as visible escapes anyway.

use std::io::Write;

use lessdugin_native::auth::redact::redact_secrets;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::out::{TermCtx, term_sanitize};

/// Sanitises one log record (possibly several lines).
pub fn sanitize_log(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let clean = term_sanitize(&redact_secrets(line), TermCtx::Prose);
        out.push_str(&clean.to_plain_string());
        out.push('\n');
    }
    out
}

/// Makes one [`SanitizingWriter`] per log record.
#[derive(Clone, Copy, Debug, Default)]
pub struct SanitizingStderr;

/// Buffers one record and writes it, sanitised, to stderr when dropped.
#[derive(Debug, Default)]
pub struct SanitizingWriter {
    buf: Vec<u8>,
}

impl Write for SanitizingWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for SanitizingWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.buf);
        let _ = std::io::stderr()
            .lock()
            .write_all(sanitize_log(&text).as_bytes());
    }
}

impl<'a> MakeWriter<'a> for SanitizingStderr {
    type Writer = SanitizingWriter;

    fn make_writer(&'a self) -> SanitizingWriter {
        SanitizingWriter::default()
    }
}

/// The default filter for `-q` / `-v` counts.
fn default_filter(verbose: u8, quiet: bool) -> &'static str {
    if quiet {
        return "error";
    }
    match verbose {
        0 => "warn",
        1 => "warn,lessdugin=debug,lessdugin_core=debug,lessdugin_native=debug,lessdugin_atp=debug",
        2 => "info,lessdugin=trace,lessdugin_core=trace,lessdugin_native=trace,lessdugin_atp=trace",
        _ => "trace",
    }
}

/// Installs the subscriber. `RUST_LOG` overrides the `-v`/`-q` levels.
pub fn init(verbose: u8, quiet: bool) {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|v| !v.is_empty())
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_filter(verbose, quiet)));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(SanitizingStderr)
        .with_ansi(false)
        .with_target(true)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_are_sanitised_and_redacted() {
        let s = sanitize_log(
            "DEBUG x: message=\"\u{1b}[2J\u{1b}]52;c;AAAA\u{7}hi\" authorization=Bearer eyJhbGciOiJ.eyJzdWIiOjF9.c2ln\n\u{202e}next",
        );
        assert!(
            !s.contains('\u{1b}') && !s.contains('\u{7}') && !s.contains('\u{202e}'),
            "{s:?}"
        );
        assert!(s.contains("^[[2J"));
        assert!(!s.contains("eyJzdWIiOjF9"), "{s}");
        assert_eq!(s.lines().count(), 2);
    }
}
