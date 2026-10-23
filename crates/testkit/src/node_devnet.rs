// SPDX-License-Identifier: AGPL-3.0-or-later
//! The node devnet (`packages/e2e/devnet.mjs`, ADR 0006) for the Rust end-to-end tests that run
//! with `LESSDUGIN_E2E=1`.
//!
//! The devnet's output goes to a log file, not `/dev/null`: when node exits before the devnet
//! is up, or it does not come up within [`BOOT_TIMEOUT`], the test fails at once with that
//! log, so a CI boot failure says why.

#![allow(clippy::disallowed_methods)] // a real process, waited for with real time

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long the devnet may take to boot.
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(60);

/// A running devnet process (killed when dropped).
pub struct NodeDevnet {
    child: Child,
    log: PathBuf,
}

impl Drop for NodeDevnet {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log);
    }
}

/// The last 8 KiB of `path`.
fn tail(path: &Path) -> String {
    let text = std::fs::read(path).unwrap_or_default();
    let from = text.len().saturating_sub(8 * 1024);
    String::from_utf8_lossy(&text[from..]).into_owned()
}

impl NodeDevnet {
    /// Starts `node <root>/packages/e2e/devnet.mjs --json <file>` and waits for the JSON it
    /// writes once the devnet is up (`label` names the files, which are per process and per
    /// call, so parallel tests each get their own devnet).
    ///
    /// # Panics
    /// When node cannot be started, exits early, or the devnet is not up in time: with the
    /// devnet's output.
    pub fn start(root: &Path, label: &str) -> (Self, Value) {
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        let n = STARTED.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "lessdugin-{label}-devnet-{}-{n}",
            std::process::id()
        ));
        let json = base.with_extension("json");
        let log = base.with_extension("log");
        let _ = std::fs::remove_file(&json);
        let out = std::fs::File::create(&log).expect("the devnet log file");
        let err = out.try_clone().expect("the devnet log file");
        let child = Command::new("node")
            .arg(root.join("packages/e2e/devnet.mjs"))
            .arg("--json")
            .arg(&json)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("node (the devnet needs node on PATH)");
        let mut devnet = Self { child, log };
        let start = Instant::now();
        loop {
            let parsed = std::fs::read_to_string(&json)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if let Some(v) = parsed {
                let _ = std::fs::remove_file(&json);
                return (devnet, v);
            }
            if let Ok(Some(status)) = devnet.child.try_wait() {
                panic!(
                    "the devnet exited before it was up ({status}); its output:\n{}",
                    tail(&devnet.log)
                );
            }
            if start.elapsed() > BOOT_TIMEOUT {
                panic!(
                    "the devnet was not up within {} s; its output:\n{}",
                    BOOT_TIMEOUT.as_secs(),
                    tail(&devnet.log)
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A devnet that cannot start fails the test at once, with node's output.
    #[test]
    fn a_devnet_that_exits_fails_at_once_with_its_output() {
        if Command::new("node").arg("--version").output().is_err() {
            eprintln!("skipped: node is not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!("lessdugin-no-devnet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let start = Instant::now();
        let r = std::panic::catch_unwind(|| NodeDevnet::start(&dir, "selftest"));
        let _ = std::fs::remove_dir_all(&dir);
        let msg = r
            .err()
            .and_then(|p| p.downcast_ref::<String>().cloned())
            .expect("a panic with a message");
        assert!(msg.contains("exited before it was up"), "{msg}");
        assert!(msg.contains("devnet.mjs"), "node's error is shown: {msg}");
        assert!(start.elapsed() < Duration::from_secs(30));
    }
}
