// SPDX-License-Identifier: AGPL-3.0-or-later
//! The CLI's own small state, next to the engine's stores in the data directory:
//!
//! - `cli/state.json`: the current forum (`forum use`, `join`);
//! - `cli/refs-<h>.json`: the last listings of one terminal session, so `3` (a post of the
//!   last `ls`), `c12` (a comment of the last `read`) and a forum number (the last `forum ls`)
//!   work as references (synthesis §6.10). `<h>` is a hash of the session key: the terminal
//!   of standard input/output (Linux `/proc/self/fd`), else the parent process (the shell), or
//!   `LESSDUGIN_SESSION` when set (tests, scripts).
//!
//! Files are written atomically with private permissions. Unreadable or malformed files are
//! treated as empty: this state is a convenience, never needed for correctness. Listing files
//! not touched for [`REFS_MAX_AGE`] are removed when another one is written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lessdugin_native::config::{Paths, lock_hash};
use lessdugin_native::fsutil::{ensure_private_dir, read_private, write_private_atomic};
use serde::{Deserialize, Serialize};

/// The environment variable that names the terminal session explicitly.
pub const SESSION_ENV: &str = "LESSDUGIN_SESSION";

/// Listing files older than this are removed.
pub const REFS_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// `cli/state.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Global {
    /// The current forum's uri.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_forum: Option<String>,
    /// When each forum was last synced by a command (ms): `read` syncs the whole forum, not
    /// only the thread, when this is older than [`FORUM_FRESH_MS`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub synced: BTreeMap<String, i64>,
}

/// How long a forum sync counts as recent for `read` (see [`Global::synced`]).
pub const FORUM_FRESH_MS: i64 = 5 * 60 * 1000;

/// At most this many forums are remembered in [`Global::synced`] (the oldest are dropped).
const SYNCED_MAX: usize = 256;

/// The wall clock (native only; the CLI's own state is not part of the engine).
#[allow(clippy::disallowed_methods)]
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `cli/refs-<h>.json`: the listings of one terminal session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listings {
    /// The forum of the last `ls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posts_forum: Option<String>,
    /// The posts of the last `ls`, in order (`1` is the first).
    #[serde(default)]
    pub posts: Vec<String>,
    /// The thread of the last `read`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The comments of the last `read`, in order (`c1` is the first).
    #[serde(default)]
    pub comments: Vec<String>,
    /// The forums of the last `forum ls`, in order.
    #[serde(default)]
    pub forums: Vec<String>,
}

/// The CLI state of one data directory.
#[derive(Clone, Debug)]
pub struct CliState {
    dir: PathBuf,
    session: String,
}

/// The session key of this process (see the module docs).
pub fn session_key() -> String {
    if let Some(s) = std::env::var_os(SESSION_ENV).filter(|s| !s.is_empty()) {
        return format!("env:{}", s.to_string_lossy());
    }
    #[cfg(target_os = "linux")]
    for fd in 0..3 {
        if let Ok(p) = std::fs::read_link(format!("/proc/self/fd/{fd}")) {
            let p = p.to_string_lossy();
            if p.starts_with("/dev/pts/") || p.starts_with("/dev/tty") {
                return format!("tty:{p}");
            }
        }
    }
    #[cfg(unix)]
    {
        format!("ppid:{}", std::os::unix::process::parent_id())
    }
    #[cfg(not(unix))]
    {
        "default".to_owned()
    }
}

fn load<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    match read_private(path, MAX_BYTES, false) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_default(),
        _ => T::default(),
    }
}

fn save<T: Serialize>(dir: &Path, path: &Path, v: &T) -> std::io::Result<()> {
    ensure_private_dir(dir)?;
    let bytes = serde_json::to_vec_pretty(v).map_err(std::io::Error::other)?;
    write_private_atomic(path, &bytes)
}

impl CliState {
    /// The state under `paths` (the devnet's own paths in devnet mode) for this terminal.
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.data_dir.join("cli"),
            session: session_key(),
        }
    }

    fn global_file(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn refs_file(&self) -> PathBuf {
        self.dir
            .join(format!("refs-{}.json", lock_hash(&self.session)))
    }

    /// The current forum.
    pub fn current_forum(&self) -> Option<String> {
        load::<Global>(&self.global_file()).current_forum
    }

    /// Sets (or clears) the current forum.
    pub fn set_current_forum(&self, uri: Option<&str>) -> std::io::Result<()> {
        let mut g: Global = load(&self.global_file());
        g.current_forum = uri.map(str::to_owned);
        save(&self.dir, &self.global_file(), &g)
    }

    /// True if `forum` was synced within [`FORUM_FRESH_MS`].
    pub fn recently_synced(&self, forum: &str) -> bool {
        load::<Global>(&self.global_file())
            .synced
            .get(forum)
            .is_some_and(|t| now_ms().saturating_sub(*t) < FORUM_FRESH_MS)
    }

    /// Records that `forum` was synced now.
    pub fn mark_synced(&self, forum: &str) -> std::io::Result<()> {
        let mut g: Global = load(&self.global_file());
        g.synced.insert(forum.to_owned(), now_ms());
        while g.synced.len() > SYNCED_MAX {
            let Some(oldest) = g
                .synced
                .iter()
                .min_by_key(|(_, t)| **t)
                .map(|(f, _)| f.clone())
            else {
                break;
            };
            g.synced.remove(&oldest);
        }
        save(&self.dir, &self.global_file(), &g)
    }

    /// This terminal's listings.
    pub fn listings(&self) -> Listings {
        load(&self.refs_file())
    }

    /// Updates this terminal's listings.
    pub fn update_listings(&self, f: impl FnOnce(&mut Listings)) -> std::io::Result<()> {
        let mut l = self.listings();
        f(&mut l);
        save(&self.dir, &self.refs_file(), &l)?;
        self.prune();
        Ok(())
    }

    /// Removes listing files of sessions not used for [`REFS_MAX_AGE`].
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        #[allow(clippy::disallowed_methods)] // file ages, native only
        let now = SystemTime::now();
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("refs-") || !name.ends_with(".json") {
                continue;
            }
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > REFS_MAX_AGE);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let st = CliState {
            dir: dir.path().join("cli"),
            session: "env:t".into(),
        };
        assert_eq!(st.current_forum(), None);
        assert!(!st.recently_synced("f"));
        st.mark_synced("f").unwrap();
        assert!(st.recently_synced("f"));
        st.set_current_forum(Some("at://did:plc:x/f/meta")).unwrap();
        assert_eq!(st.current_forum().as_deref(), Some("at://did:plc:x/f/meta"));
        st.update_listings(|l| l.posts = vec!["a".into(), "b".into()])
            .unwrap();
        assert_eq!(st.listings().posts, ["a", "b"]);
        std::fs::write(st.refs_file(), b"{not json").unwrap();
        assert_eq!(st.listings(), Listings::default());
        let other = CliState {
            session: "env:u".into(),
            ..st.clone()
        };
        assert_ne!(other.refs_file(), st.refs_file());
    }
}
