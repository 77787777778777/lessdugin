// SPDX-License-Identifier: AGPL-3.0-or-later
//! The real TUI runtime on a pseudo-terminal (`crates/tui/src/runtime.rs`; every other TUI
//! test drives the headless loop): `lessdugin --dev tui <forum>` under util-linux `script`,
//! keys written to the terminal, `$EDITOR` a script. It checks what only the runtime does:
//!
//! - the terminal is not asked for its graphics protocol unless the environment names an
//!   emulator (`crates/tui/src/image.rs`), and when it is asked and never answers (this pty has
//!   no emulator behind it), the bounded wait ends and no reader is left on stdin: the keys the
//!   test types afterwards reach the TUI, as the regression of a query thread that ate them;
//! - `$EDITOR` runs with the terminal suspended: in cooked mode, and with the input reader
//!   parked (the keys the test types while the editor runs reach the editor, not the TUI);
//! - the outbox pump sends the comment while the TUI runs;
//! - Ctrl-C quits, and the terminal is back in cooked mode afterwards.
//!
//! Linux only (the `script` options are util-linux's); skipped when `script` is missing.

#![cfg(all(target_os = "linux", feature = "tui"))]
#![allow(clippy::disallowed_methods)] // a real clock and sleeps for a real process

#[path = "support/fakenet.rs"]
mod fakenet;

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fakenet::{Net, User, s};
use lessdugin_lexicon::collection::nsid;

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The Device Status Report that ends the TUI's graphics query (`image.rs`).
const DSR_QUERY: &str = "\u{1b}[5n";

/// The variables an emulator sets, which the TUI reads as "this terminal answers queries"
/// (`crates/tui/src/image.rs`): the test process may run inside one, and the pty must not
/// inherit its evidence.
const EMULATOR_VARS: [&str; 19] = [
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "KITTY_WINDOW_ID",
    "KITTY_PID",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
    "KONSOLE_VERSION",
    "VTE_VERSION",
    "GHOSTTY_RESOURCES_DIR",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "XTERM_VERSION",
    "WT_SESSION",
    "MLTERM",
    "TERMINATOR_UUID",
    "GNOME_TERMINAL_SCREEN",
    "TMUX",
];

/// Waits until `cond` holds (or panics after `secs`, with `what` and the terminal output).
fn wait(secs: u64, what: &str, out: &Arc<Mutex<Vec<u8>>>, mut cond: impl FnMut(&str) -> bool) {
    let start = Instant::now();
    loop {
        let text = String::from_utf8_lossy(&out.lock().unwrap()).into_owned();
        if cond(&text) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(secs),
            "timed out waiting for {what}; the terminal showed:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `script` running the TUI on a pseudo-terminal (80 × 24), its keys and its output.
struct Pty {
    child: Child,
    keys: Option<ChildStdin>,
    out: Arc<Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Pty {
    /// Starts `lessdugin tui alice.test/meta` as `alice`, with `editor` as `$EDITOR` and
    /// `images` as `LESSDUGIN_TUI_IMAGES` (unset: the default, `auto`). The emulator's
    /// variables are scrubbed: the pty is a terminal nothing answers for.
    fn start(alice: &User, editor: &std::path::Path, images: Option<&str>) -> Self {
        let base = alice.std_cmd();
        let program = base.get_program().to_string_lossy().into_owned();
        let mut line = String::from("stty rows 24 cols 80; ");
        line.push_str(&shell_quote(&program));
        for a in base.get_args() {
            line.push(' ');
            line.push_str(&shell_quote(&a.to_string_lossy()));
        }
        line.push_str(" tui alice.test/meta; echo; echo AFTER-TUI; stty -a");
        let mut cmd = Command::new("script");
        cmd.args(["-q", "-f", "-e", "-c", &line, "/dev/null"]);
        for (k, v) in base.get_envs() {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        for k in EMULATOR_VARS {
            cmd.env_remove(k);
        }
        cmd.env("EDITOR", editor)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match images {
            Some(v) => cmd.env("LESSDUGIN_TUI_IMAGES", v),
            None => cmd.env_remove("LESSDUGIN_TUI_IMAGES"),
        };
        let mut child = cmd.spawn().unwrap();
        let keys = child.stdin.take().unwrap();
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut stdout = child.stdout.take().unwrap();
        let sink = Arc::clone(&out);
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Self {
            child,
            keys: Some(keys),
            out,
            reader: Some(reader),
        }
    }

    fn type_keys(&mut self, k: &str) {
        let keys = self.keys.as_mut().unwrap();
        keys.write_all(k.as_bytes()).unwrap();
        keys.flush().unwrap();
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }

    /// Ctrl-C quits the TUI; `script` exits; returns everything the terminal showed (the
    /// process must have succeeded, and the terminal be in cooked mode again).
    fn quit(mut self) -> String {
        self.type_keys("\u{3}");
        wait(30, "the TUI to exit", &self.out, |t| {
            t.contains("AFTER-TUI")
        });
        drop(self.keys.take());
        let start = Instant::now();
        let status = loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                break st;
            }
            if start.elapsed() > Duration::from_secs(30) {
                let _ = self.child.kill();
                panic!("script did not exit");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        self.reader.take().unwrap().join().unwrap();
        let text = self.text();
        let mut err = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        assert!(status.success(), "{status}: {err}\n{text}");
        let after = text.rsplit("AFTER-TUI").next().unwrap();
        assert!(cooked(after), "the terminal was left in raw mode:\n{after}");
        // the alternate screen was left
        assert!(text.contains("\u{1b}[?1049l"), "{text:?}");
        text
    }
}

fn cooked(stty: &str) -> bool {
    let flags: Vec<&str> = stty.split_whitespace().collect();
    flags.contains(&"icanon") && flags.contains(&"echo")
}

/// alice's forum with one frontpage post for the TUI to list.
fn forum_with_a_post(alice: &User) {
    // --frontpage: the M3 default listing shows only frontpage-tier posts, and the
    // TUI lists with the default filter (this test is about the runtime, not tiers)
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    let f = alice.file(
        "p.md",
        "---\ntitle: Terminal post\n---\nRead me in a pty.\n",
    );
    alice.json(&["post", s(&f), "--yes"]);
}

#[test]
fn the_tui_comments_through_the_editor_on_a_real_terminal() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: util-linux `script` is not installed");
        return;
    }
    let net = Net::new();
    let alice = net.user("alice.test", true);
    forum_with_a_post(&alice);

    // the editor records the terminal mode, then waits for a line typed on the terminal (the
    // TUI's input reader must not take it) and writes it as the comment
    let mode = alice.path("stty-in-editor.txt");
    let editor = alice.file(
        "editor.sh",
        &format!(
            "#!/bin/sh\nstty -a > {}\nIFS= read -r line\nsed -i \"/ >8 /i $line\" \"$1\"\n",
            shell_quote(s(&mode))
        ),
    );
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut pty = Pty::start(&alice, &editor, None);
    // the forum lists the post: open it, then comment
    wait(30, "the forum's post list", &pty.out, |t| {
        t.contains("Terminal post")
    });
    // an anonymous pty is not asked for its graphics protocol (nothing would answer, and a
    // reader waiting for the answer would eat the Enter below)
    assert!(!pty.text().contains(DSR_QUERY), "{:?}", pty.text());
    pty.type_keys("\r");
    wait(30, "the thread", &pty.out, |t| {
        t.contains("No comments yet.")
    });
    pty.type_keys("c");
    wait(30, "the editor", &pty.out, |_| mode.exists());
    // typed while the editor runs: the editor reads it (cooked mode, line by line)
    pty.type_keys("Written on a real terminal\r");

    // the pump sends it while the TUI runs
    let start = Instant::now();
    while alice.account.list(nsid::COMMENT).is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the comment was not sent; the terminal showed:\n{}",
            pty.text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let comments = alice.account.list(nsid::COMMENT);
    assert_eq!(comments.len(), 1);
    let rec = alice.account.get(nsid::COMMENT, &comments[0].0).unwrap();
    assert!(
        rec.to_string().contains("Written on a real terminal"),
        "{rec}"
    );

    // Ctrl-C quits; the terminal is in cooked mode again, as it was for the editor
    let _ = pty.quit();
    let in_editor = std::fs::read_to_string(&mode).unwrap();
    assert!(
        cooked(&in_editor),
        "the editor ran in raw mode:\n{in_editor}"
    );
}

/// `LESSDUGIN_TUI_IMAGES=query` on a terminal that never answers: the TUI asks, waits its
/// bounded time, falls back to halfblocks, and the keys typed afterwards still reach it (no
/// reader is left waiting for the answer).
#[test]
fn a_forced_query_on_a_terminal_that_never_answers_loses_no_key() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: util-linux `script` is not installed");
        return;
    }
    let net = Net::new();
    let alice = net.user("alice.test", true);
    forum_with_a_post(&alice);
    let editor = alice.file("editor.sh", "#!/bin/sh\nexit 1\n");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut pty = Pty::start(&alice, &editor, Some("query"));
    wait(30, "the forum's post list", &pty.out, |t| {
        t.contains("Terminal post")
    });
    // it was asked (the query ends with a DSR), and nobody answered
    assert!(pty.text().contains(DSR_QUERY), "{:?}", pty.text());
    pty.type_keys("\r");
    wait(30, "the thread", &pty.out, |t| {
        t.contains("No comments yet.")
    });
    // `h` goes back to the forum: a second key, after the first was taken
    pty.type_keys("h");
    wait(30, "the forum again", &pty.out, |t| {
        t.rsplit("No comments yet.")
            .next()
            .is_some_and(|after| after.contains("Terminal post"))
    });
    let _ = pty.quit();
}
