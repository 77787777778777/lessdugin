// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runs several `lessdugin` processes at once against one data directory (M1 acceptance,
//! `docs/plan.md` §3: "parallel CLI processes refreshing one session never invalidate it") and
//! reads what each one did from its debug log: the HTTP requests it sent
//! (`lessdugin_native::host::net`) and whether it adopted a session another process had
//! refreshed (`lessdugin_native::auth`).
//!
//! Shared by `concurrent_refresh.rs` (offline) and `e2e_devnet.rs` (devnet) with `#[path]`.
#![allow(dead_code)] // each includer uses a part

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The log filter given to every racing process.
pub const RACE_LOG: &str = "warn,lessdugin_native::host::net=debug,lessdugin_native::auth=debug";

/// What a process logs when it adopts the session another process refreshed meanwhile.
pub const ADOPTED: &str = "a session refreshed by another process";

/// The outputs of one race.
pub struct Race {
    /// One output per process, in spawn order.
    pub outputs: Vec<Output>,
}

/// Spawns every command before waiting for any (stdin closed, output captured, `RUST_LOG` set
/// to [`RACE_LOG`]); each output is collected on its own thread so no pipe fills up.
pub fn race(cmds: Vec<Command>) -> Race {
    race_with(cmds, || {})
}

/// [`race`], running `during` once every process has been spawned (and its output is being
/// collected), before waiting for them to exit.
pub fn race_with(cmds: Vec<Command>, during: impl FnOnce()) -> Race {
    let children: Vec<_> = cmds
        .into_iter()
        .map(|mut c| {
            c.env("RUST_LOG", RACE_LOG)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            c.spawn().expect("spawn lessdugin")
        })
        .collect();
    let waiters: Vec<_> = children
        .into_iter()
        .map(|c| std::thread::spawn(move || c.wait_with_output().expect("wait for lessdugin")))
        .collect();
    during();
    Race {
        outputs: waiters.into_iter().map(|w| w.join().unwrap()).collect(),
    }
}

/// How many processes wait for the `flock` of `path`, from `/proc/locks` (`None` where that is
/// not available: not Linux).
fn lock_waiters(path: &Path) -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let ino = std::fs::metadata(path).ok()?.ino().to_string();
        let locks = std::fs::read_to_string("/proc/locks").ok()?;
        // `6: -> FLOCK  ADVISORY  WRITE <pid> <major>:<minor>:<inode> 0 EOF` is a waiter.
        Some(
            locks
                .lines()
                .map(|l| l.split_whitespace().collect::<Vec<_>>())
                .filter(|t| {
                    t.get(1) == Some(&"->")
                        && t.iter().any(|f| {
                            f.matches(':').count() == 2 && f.rsplit(':').next() == Some(&ino)
                        })
                })
                .count(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
}

/// Waits until `n` processes wait for the `flock` of `path`, at most `max`; returns how many were
/// seen waiting. Without `/proc/locks` it just sleeps [`LOCK_WAIT_FALLBACK`] and returns `None`.
pub fn wait_for_lock_waiters(path: &Path, n: usize, max: Duration) -> Option<usize> {
    let start = Instant::now();
    loop {
        let Some(waiting) = lock_waiters(path) else {
            std::thread::sleep(LOCK_WAIT_FALLBACK);
            return None;
        };
        if waiting >= n || start.elapsed() > max {
            return Some(waiting);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// How long [`wait_for_lock_waiters`] waits where it cannot see the waiters.
pub const LOCK_WAIT_FALLBACK: Duration = Duration::from_secs(3);

/// The log line of an HTTP request (`method=POST url=<url>`), as `host::net` writes it.
pub fn request_line(method: &str, url: &str) -> String {
    format!("method={method} url={url} ")
}

impl Race {
    /// The stderr of process `i`.
    pub fn stderr(&self, i: usize) -> String {
        String::from_utf8_lossy(&self.outputs[i].stderr).into_owned()
    }

    /// Asserts that every process exited 0 and returns their stdout parsed as JSON.
    pub fn all_ok_json(&self, what: &str) -> Vec<serde_json::Value> {
        self.outputs
            .iter()
            .enumerate()
            .map(|(i, out)| {
                assert!(
                    out.status.success(),
                    "{what}: process {i} failed ({:?}):\n{}",
                    out.status.code(),
                    self.stderr(i)
                );
                serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
                    panic!(
                        "{what}: process {i}: {e}: {}",
                        String::from_utf8_lossy(&out.stdout)
                    )
                })
            })
            .collect()
    }

    /// How many processes logged `needle` at least once, and how many times it was logged in
    /// all.
    pub fn count(&self, needle: &str) -> (usize, usize) {
        let per: Vec<usize> = (0..self.outputs.len())
            .map(|i| self.stderr(i).matches(needle).count())
            .collect();
        (per.iter().filter(|n| **n > 0).count(), per.iter().sum())
    }

    /// A summary of the race for failure messages and the test log.
    pub fn summary(&self, refresh: &str) -> String {
        let (refreshers, refreshes) = self.count(refresh);
        // A process may adopt a stored session and still refresh it (when that one is expiring
        // too): it counts as a refresher only.
        let (mut adopters, mut neither) = (0, 0);
        for i in 0..self.outputs.len() {
            let log = self.stderr(i);
            if !log.contains(refresh) {
                if log.contains(ADOPTED) {
                    adopters += 1;
                } else {
                    neither += 1;
                }
            }
        }
        format!(
            "{} processes: {refreshes} refresh request(s) from {refreshers} process(es), \
             {adopters} adopted the refreshed session, {neither} used the stored session as \
             loaded",
            self.outputs.len(),
        )
    }

    /// Every stderr, labelled (for failure messages).
    pub fn logs(&self) -> String {
        (0..self.outputs.len())
            .map(|i| format!("--- process {i}\n{}", self.stderr(i)))
            .collect()
    }
}
