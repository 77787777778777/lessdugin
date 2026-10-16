// SPDX-License-Identifier: AGPL-3.0-or-later
//! Interactive input on the terminal: a no-echo secret prompt (crossterm raw mode), a line
//! prompt and a yes/no confirmation. Prompts are written to stderr, so stdout stays clean for
//! `--json`.

use std::io::{BufRead, IsTerminal, Read, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;

/// True if standard input is a terminal.
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

struct RawMode;

impl RawMode {
    fn enable() -> std::io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Reads a secret without echoing it. Ctrl-C or Ctrl-D on an empty line aborts.
pub fn read_secret(prompt: &str) -> std::io::Result<String> {
    let mut err = std::io::stderr();
    write!(err, "{prompt}")?;
    err.flush()?;
    let mut secret = String::new();
    {
        let raw = RawMode::enable()?;
        loop {
            let ev = event::read()?;
            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = ev
            else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }
            let ctrl = modifiers.contains(KeyModifiers::CONTROL);
            match code {
                KeyCode::Enter => break,
                KeyCode::Char('c') if ctrl => {
                    drop(raw);
                    writeln!(err)?;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "cancelled",
                    ));
                }
                KeyCode::Char('d') if ctrl && secret.is_empty() => {
                    drop(raw);
                    writeln!(err)?;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "no input",
                    ));
                }
                KeyCode::Char('u') if ctrl => secret.clear(),
                KeyCode::Backspace => {
                    secret.pop();
                }
                KeyCode::Char(c) if !ctrl => secret.push(c),
                _ => {}
            }
        }
    }
    writeln!(err)?;
    Ok(secret)
}

/// Reads one line from stdin after a prompt on stderr (`None` at end of input).
pub fn read_line(prompt: &str) -> std::io::Result<Option<String>> {
    let mut err = std::io::stderr();
    write!(err, "{prompt}")?;
    err.flush()?;
    let mut line = String::new();
    let n = std::io::stdin()
        .lock()
        .take(64 * 1024)
        .read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
}

/// Asks a yes/no question on a terminal; anything but `y`/`yes` is no.
pub fn confirm(question: &str) -> bool {
    if !stdin_is_tty() {
        return false;
    }
    match read_line(&format!("{question} [y/N] ")) {
        Ok(Some(answer)) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        _ => false,
    }
}
