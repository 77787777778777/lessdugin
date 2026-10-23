// SPDX-License-Identifier: AGPL-3.0-or-later
//! The real TUI runtime on a pseudo-terminal (`crates/tui/src/runtime.rs`; every other TUI
//! test drives the headless loop): `lessdugin --dev tui <forum>` under util-linux `script`,
//! keys written to the terminal, `$EDITOR` a script. It checks what only the runtime does:
//!
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
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fakenet::{Net, s};
use lessdugin_lexicon::collection::nsid;

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

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

#[test]
fn the_tui_comments_through_the_editor_on_a_real_terminal() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: util-linux `script` is not installed");
        return;
    }
    let net = Net::new();
    let alice = net.user("alice.test", true);
    // --frontpage: the M3 default listing shows only frontpage-tier posts, and the
    // TUI lists with the default filter (this test is about the runtime, not tiers)
    alice.json(&["forum", "create", "meta", "--frontpage"]);
    let f = alice.file(
        "p.md",
        "---\ntitle: Terminal post\n---\nRead me in a pty.\n",
    );
    alice.json(&["post", s(&f), "--yes"]);

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

    // `script` runs the TUI on a pseudo-terminal (80 × 24), then prints the terminal mode
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
    cmd.env("EDITOR", &editor)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut keys = child.stdin.take().unwrap();
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
    let mut type_keys = |k: &str| {
        keys.write_all(k.as_bytes()).unwrap();
        keys.flush().unwrap();
    };

    // the forum lists the post: open it, then comment
    wait(30, "the forum's post list", &out, |t| {
        t.contains("Terminal post")
    });
    type_keys("\r");
    wait(30, "the thread", &out, |t| t.contains("No comments yet."));
    type_keys("c");
    wait(30, "the editor", &out, |_| mode.exists());
    // typed while the editor runs: the editor reads it (cooked mode, line by line)
    type_keys("Written on a real terminal\r");

    // the pump sends it while the TUI runs
    let start = Instant::now();
    while alice.account.list(nsid::COMMENT).is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "the comment was not sent; the terminal showed:\n{}",
            String::from_utf8_lossy(&out.lock().unwrap())
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

    // Ctrl-C quits; the terminal is in cooked mode again
    type_keys("\u{3}");
    wait(30, "the TUI to exit", &out, |t| t.contains("AFTER-TUI"));
    drop(keys);
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            panic!("script did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    reader.join().unwrap();
    let text = String::from_utf8_lossy(&out.lock().unwrap()).into_owned();
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(status.success(), "{status}: {err}\n{text}");

    let cooked = |stty: &str| {
        let flags: Vec<&str> = stty.split_whitespace().collect();
        flags.contains(&"icanon") && flags.contains(&"echo")
    };
    let in_editor = std::fs::read_to_string(&mode).unwrap();
    assert!(
        cooked(&in_editor),
        "the editor ran in raw mode:\n{in_editor}"
    );
    let after = text.rsplit("AFTER-TUI").next().unwrap();
    assert!(cooked(after), "the terminal was left in raw mode:\n{after}");
    // the alternate screen was left
    assert!(text.contains("\u{1b}[?1049l"), "{text:?}");
}
