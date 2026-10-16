// SPDX-License-Identifier: AGPL-3.0-or-later
//! Configuration and platform paths.
//!
//! - [`Paths`]: where the config file, the `state` and `cache` stores, drafts, blobs and lock files
//!   live (`docs/design/synthesis-v1.md` §6.10, `sync.md` §10.1). `--data-dir` or
//!   `LESSDUGIN_DATA_DIR` puts everything under one directory.
//! - [`Config`]: `config.toml` (network profile, sources, verify mode, DoH providers, indexers…;
//!   `sync.md` §2.3, `engine-api.md` §2). Saved with mode 0600.
//! - [`FileLock`]: exclusive `fd-lock` files, e.g. the sync leader lock `<cache>/cache.lock`
//!   (`sync.md` §10.6, invariant I9).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::fsutil;
use crate::host::NetPolicy;

/// Application directory name.
pub const APP_NAME: &str = "lessdugin";
/// Environment variable overriding the data directory (same as `--data-dir`).
pub const DATA_DIR_ENV: &str = "LESSDUGIN_DATA_DIR";
/// Largest accepted `config.toml`.
pub const CONFIG_MAX_BYTES: u64 = 1024 * 1024;

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// No home directory could be determined (and no `--data-dir` was given).
    #[error("cannot determine the home directory; pass --data-dir or set {DATA_DIR_ENV}")]
    NoHome,
    /// File system error.
    #[error("{path}: {source}")]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// `config.toml` is not valid TOML or has wrong value types.
    #[error("{path}: {message}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// The parser's message.
        message: String,
    },
    /// A value is out of range (e.g. a non-https endpoint outside devnet mode).
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ConfigError + '_ {
    move |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    }
}

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------

/// The directories and files lessdugin uses.
///
/// | | default (Linux) | under `--data-dir D` |
/// |---|---|---|
/// | config | `~/.config/lessdugin/config.toml` | `D/config.toml` |
/// | data | `~/.local/share/lessdugin/` (`state.sqlite`, `drafts/`, `locks/`) | `D/` |
/// | cache | `~/.cache/lessdugin/` (`cache.sqlite`, `blobs/`, `locks/`, `cache.lock`) | `D/cache/` |
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `config.toml`.
    pub config_file: PathBuf,
    /// The durable data directory.
    pub data_dir: PathBuf,
    /// The disposable cache directory (deleting it loses only time, `sync.md` I1).
    pub cache_dir: PathBuf,
}

impl Paths {
    /// `--data-dir` if given, else `$LESSDUGIN_DATA_DIR` if set and non-empty, else the
    /// platform directories.
    pub fn resolve(data_dir_flag: Option<&Path>) -> Result<Self, ConfigError> {
        if let Some(dir) = data_dir_flag.filter(|d| !d.as_os_str().is_empty()) {
            return Ok(Self::under(dir));
        }
        if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|d| !d.is_empty()) {
            return Ok(Self::under(Path::new(&dir)));
        }
        Self::platform()
    }

    /// The platform directories (XDG on Linux, `~/Library/…` on macOS, `%APPDATA%` on Windows).
    pub fn platform() -> Result<Self, ConfigError> {
        let dirs = directories::ProjectDirs::from("", "", APP_NAME).ok_or(ConfigError::NoHome)?;
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            data_dir: dirs.data_dir().to_path_buf(),
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    /// Everything under one directory (tests, portable installs, `--data-dir`).
    pub fn under(root: &Path) -> Self {
        Self {
            config_file: root.join("config.toml"),
            data_dir: root.to_path_buf(),
            cache_dir: root.join("cache"),
        }
    }

    /// The paths of a devnet: its own data and cache directories,
    /// `<data>/devnet-<id>/` and `<cache>/devnet-<id>/` ([`DevnetProfile::id`]), so that devnet
    /// identities, accounts, issuer pins and sessions never meet production ones (or another
    /// devnet's). The configuration file is shared.
    pub fn for_devnet(&self, devnet: &DevnetProfile) -> Self {
        let dir = format!("devnet-{}", devnet.id());
        Self {
            config_file: self.config_file.clone(),
            data_dir: self.data_dir.join(&dir),
            cache_dir: self.cache_dir.join(&dir),
        }
    }

    /// The durable `state` store.
    pub fn state_db(&self) -> PathBuf {
        self.data_dir.join("state.sqlite")
    }

    /// Draft files (`<id>.md`).
    pub fn drafts_dir(&self) -> PathBuf {
        self.data_dir.join("drafts")
    }

    /// Per-account lock files in the data directory (outbox).
    pub fn data_locks_dir(&self) -> PathBuf {
        self.data_dir.join("locks")
    }

    /// The disposable `cache` store.
    pub fn cache_db(&self) -> PathBuf {
        self.cache_dir.join("cache.sqlite")
    }

    /// Verified blobs, `blobs/<last two characters of the CID>/<cid>` (`sync.md` §11.2).
    pub fn blobs_dir(&self) -> PathBuf {
        self.cache_dir.join("blobs")
    }

    /// Per-repo lock files in the cache directory.
    pub fn cache_locks_dir(&self) -> PathBuf {
        self.cache_dir.join("locks")
    }

    /// The sync-leader lock file, `<cache>/cache.lock`.
    pub fn sync_leader_lock_file(&self) -> PathBuf {
        self.cache_dir.join("cache.lock")
    }

    /// The lock file for a host lock name (`sync.md` §10.6, §12.3): `sync-leader`,
    /// `repo:<did>`, `outbox:<did>`, or `refresh:<did>` (token refresh, `threat-model.md`
    /// AUTH-7). `None` for an unknown name.
    pub fn lock_file(&self, name: &str) -> Option<PathBuf> {
        if name == "sync-leader" {
            Some(self.sync_leader_lock_file())
        } else if let Some(did) = name.strip_prefix("refresh:") {
            Some(
                self.data_locks_dir()
                    .join(format!("refresh-{}.lock", lock_hash(did))),
            )
        } else if let Some(did) = name.strip_prefix("repo:") {
            Some(
                self.cache_locks_dir()
                    .join(format!("repo-{}.lock", lock_hash(did))),
            )
        } else {
            name.strip_prefix("outbox:").map(|did| {
                self.data_locks_dir()
                    .join(format!("outbox-{}.lock", lock_hash(did)))
            })
        }
    }

    /// Creates the data, drafts, cache and blob directories (mode 0700) and the config
    /// directory.
    pub fn ensure_dirs(&self) -> Result<(), ConfigError> {
        let mut dirs = vec![
            self.data_dir.clone(),
            self.drafts_dir(),
            self.data_locks_dir(),
            self.cache_dir.clone(),
            self.blobs_dir(),
            self.cache_locks_dir(),
        ];
        if let Some(parent) = self.config_file.parent() {
            dirs.push(parent.to_path_buf());
        }
        for d in dirs {
            fsutil::ensure_private_dir(&d).map_err(io_err(&d))?;
        }
        Ok(())
    }

    /// Tries to become the background-sync leader for this cache without waiting
    /// (`sync.md` I9). `Ok(None)`: another process holds it; do only P0 fetches.
    pub fn try_sync_leader(&self) -> io::Result<Option<FileLock>> {
        FileLock::try_acquire(&self.sync_leader_lock_file())
    }

    /// Takes the lock called `name` (see [`Paths::lock_file`]); with `wait = false` returns
    /// `Ok(None)` when it is busy. Waiting happens on a blocking thread.
    pub async fn lock(&self, name: &str, wait: bool) -> io::Result<Option<FileLock>> {
        let path = self.lock_file(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown lock name {name:?}"),
            )
        })?;
        if wait {
            FileLock::acquire_async(path).await.map(Some)
        } else {
            FileLock::try_acquire(&path)
        }
    }
}

/// `h(x)`: the first 16 characters of `b32s(sha256(UTF8(x)))` (`sync.md` §10.6,
/// `data-model.md` §3.1).
pub fn lock_hash(x: &str) -> String {
    b32s(&Sha256::digest(x.as_bytes()))[..16].to_owned()
}

/// Sortable base32 (`data-model.md` §3.1): MSB-first 5-bit groups, alphabet
/// `234567abcdefghijklmnopqrstuvwxyz`, no padding.
fn b32s(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"234567abcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Locks
// ---------------------------------------------------------------------------------------------

/// An exclusive advisory lock on a file (`fd-lock`: `flock` on unix, `LockFileEx` on Windows).
/// Dropping it releases the lock. Two `FileLock`s on the same file exclude each other even in
/// one process.
#[derive(Debug)]
pub struct FileLock {
    // Held only to keep the descriptor (and so the lock) open; closing it releases the lock.
    _lock: fd_lock::RwLock<File>,
    path: PathBuf,
}

impl FileLock {
    /// Takes the lock if it is free; `Ok(None)` if another holder has it.
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        let mut lock = fd_lock::RwLock::new(fsutil::open_or_create_private(path)?);
        match lock.try_write() {
            // The guard would unlock on drop; keep the lock until the descriptor closes instead,
            // so the lock can own its file.
            Ok(guard) => std::mem::forget(guard),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        }
        Ok(Some(Self {
            _lock: lock,
            path: path.to_path_buf(),
        }))
    }

    /// Takes the lock, blocking the current thread until it is free.
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let mut lock = fd_lock::RwLock::new(fsutil::open_or_create_private(path)?);
        std::mem::forget(lock.write()?);
        Ok(Self {
            _lock: lock,
            path: path.to_path_buf(),
        })
    }

    /// [`FileLock::acquire`] on a blocking thread.
    pub async fn acquire_async(path: PathBuf) -> io::Result<Self> {
        tokio::task::spawn_blocking(move || Self::acquire(&path))
            .await
            .map_err(io::Error::other)?
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------------------------

/// Which network to talk to (`engine-api.md` §2, `sync.md` §3.7). Devnet mode is never enabled
/// implicitly: it needs `--dev` or this profile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
// Read once per process; the size difference between variants does not matter.
#[allow(clippy::large_enum_variant)]
pub enum NetworkProfile {
    /// The public AT Protocol network.
    #[default]
    Production,
    /// A local devnet (ADR 0006).
    Devnet(DevnetProfile),
}

/// Devnet endpoints. `http://`/`ws://` and loopback are allowed for exactly these origins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevnetProfile {
    /// The PLC directory, e.g. `http://localhost:2582`.
    pub plc: Url,
    /// The PDS (handles resolve through its `com.atproto.identity.resolveHandle`).
    pub pds: Url,
    /// A local Jetstream, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jetstream: Option<Url>,
}

impl DevnetProfile {
    /// `h(plc + "\n" + pds)`: names this devnet's directories ([`Paths::for_devnet`]) and its
    /// keyring service (`lessdugin-devnet-<id>`).
    pub fn id(&self) -> String {
        lock_hash(&format!("{}\n{}", self.plc.as_str(), self.pds.as_str()))
    }

    /// The keyring service of this devnet's sessions.
    pub fn keyring_service(&self) -> String {
        format!("lessdugin-devnet-{}", self.id())
    }
}

/// Which sources to use (`sync.md` §2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sources {
    /// CORE only.
    Direct,
    /// CORE+ and enabled indexers.
    #[default]
    Assisted,
}

/// Verification mode (`sync.md` §2.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyMode {
    /// Admit at level `pds`.
    #[default]
    Pds,
    /// Verify every displayed item (Reserved, P2).
    Proof,
}

/// How handles are resolved over DNS (native only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DnsMode {
    /// DNS over HTTPS.
    #[default]
    Doh,
    /// The system resolver (answers are flagged `dns-unauthenticated`).
    System,
}

/// The `indexer` setting: `"forum"`, `"none"`, or a list of indexer URLs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IndexerSetting {
    /// `"forum"` (forum-declared indexers after consent) or `"none"`.
    Mode(IndexerMode),
    /// Exactly these indexers.
    Urls(Vec<Url>),
}

impl Default for IndexerSetting {
    fn default() -> Self {
        Self::Mode(IndexerMode::Forum)
    }
}

/// See [`IndexerSetting`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexerMode {
    /// Use forum-declared indexers after one-time consent.
    Forum,
    /// Use no indexer.
    None,
}

/// `config.toml`. Every field has a default, so an empty or missing file is valid.
///
/// ```toml
/// sources = "assisted"            # direct | assisted
/// verify = "pds"                  # pds | proof
/// dns = "doh"                     # doh | system
/// doh = ["https://cloudflare-dns.com/dns-query", "https://dns.google/resolve"]
/// plc = "https://plc.directory"
/// indexer = "forum"               # forum | none | ["https://…"]
///
/// [network]
/// mode = "devnet"                 # production | devnet
/// plc = "http://localhost:2582"
/// pds = "http://localhost:2583"
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Sources (`direct`, `assisted`).
    pub sources: Sources,
    /// Verification mode.
    pub verify: VerifyMode,
    /// DNS mode for handle TXT lookups.
    pub dns: DnsMode,
    /// DoH JSON endpoints, in order of preference.
    pub doh: Vec<Url>,
    /// The PLC directory (production).
    pub plc: Url,
    /// Relays for `listReposByCollection`.
    pub relays: Vec<Url>,
    /// Jetstream endpoints.
    pub jetstream: Vec<Url>,
    /// Constellation (assisted mode only).
    pub constellation: Option<Url>,
    /// Slingshot (hint-only identity fallback; assisted mode only).
    pub slingshot: Option<Url>,
    /// Indexers.
    pub indexer: IndexerSetting,
    /// Media autoload allowlist (hostnames, `sync.md` §11.3).
    pub media_hosts: Vec<String>,
    /// Network profile; a table, so it must stay the last field for TOML output.
    pub network: NetworkProfile,
}

fn url(s: &str) -> Url {
    // Only called with the literal defaults below, which are valid URLs.
    Url::parse(s).unwrap_or_else(|_| unreachable!("invalid default URL {s}"))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sources: Sources::default(),
            verify: VerifyMode::default(),
            dns: DnsMode::default(),
            doh: vec![
                url("https://cloudflare-dns.com/dns-query"),
                url("https://dns.google/resolve"),
            ],
            plc: url("https://plc.directory"),
            relays: vec![
                url("https://bsky.network"),
                url("https://relay1.us-east.bsky.network"),
            ],
            jetstream: vec![
                url("wss://jetstream.us-east.bsky.network"),
                url("wss://jetstream.us-west.bsky.network"),
            ],
            constellation: Some(url("https://constellation.microcosm.blue")),
            slingshot: Some(url("https://slingshot.microcosm.blue")),
            indexer: IndexerSetting::default(),
            media_hosts: Vec::new(),
            network: NetworkProfile::Production,
        }
    }
}

impl Config {
    /// A devnet configuration (`--dev`, e2e tests).
    pub fn devnet(plc: Url, pds: Url) -> Self {
        Self {
            plc: plc.clone(),
            network: NetworkProfile::Devnet(DevnetProfile {
                plc,
                pds,
                jetstream: None,
            }),
            ..Self::default()
        }
    }

    /// True in devnet mode.
    pub fn is_devnet(&self) -> bool {
        matches!(self.network, NetworkProfile::Devnet(_))
    }

    /// The PLC directory to use: the devnet's in devnet mode.
    pub fn effective_plc(&self) -> &Url {
        match &self.network {
            NetworkProfile::Devnet(d) => &d.plc,
            NetworkProfile::Production => &self.plc,
        }
    }

    /// The SSRF-guard policy for this configuration: production rules, or devnet rules that
    /// allow loopback and `http://`/`ws://` for the configured devnet origins.
    pub fn net_policy(&self) -> NetPolicy {
        match &self.network {
            NetworkProfile::Production => NetPolicy::production(),
            NetworkProfile::Devnet(d) => {
                NetPolicy::devnet([&d.plc, &d.pds].into_iter().chain(d.jetstream.as_ref()))
            }
        }
    }

    /// Checks schemes: production endpoints must be `https` (`wss` for Jetstream); devnet
    /// endpoints may also be `http` (`ws`).
    pub fn validate(&self) -> Result<(), ConfigError> {
        let devnet = self.is_devnet();
        let https = |what: &str, u: &Url| -> Result<(), ConfigError> {
            if u.scheme() == "https" || (devnet && u.scheme() == "http") {
                Ok(())
            } else {
                Err(ConfigError::Invalid(format!(
                    "{what} must be an https URL: {u}"
                )))
            }
        };
        for u in &self.doh {
            https("doh", u)?;
        }
        https("plc", &self.plc)?;
        for u in &self.relays {
            https("relays", u)?;
        }
        for u in &self.jetstream {
            if !(u.scheme() == "wss" || (devnet && u.scheme() == "ws")) {
                return Err(ConfigError::Invalid(format!(
                    "jetstream must be a wss URL: {u}"
                )));
            }
        }
        for (what, u) in [
            ("constellation", &self.constellation),
            ("slingshot", &self.slingshot),
        ] {
            if let Some(u) = u {
                https(what, u)?;
            }
        }
        if let IndexerSetting::Urls(urls) = &self.indexer {
            for u in urls {
                https("indexer", u)?;
            }
        }
        if let NetworkProfile::Devnet(d) = &self.network {
            for (what, u) in [("network.plc", &d.plc), ("network.pds", &d.pds)] {
                if !matches!(u.scheme(), "http" | "https") || u.host().is_none() {
                    return Err(ConfigError::Invalid(format!(
                        "{what} must be an http(s) URL"
                    )));
                }
            }
            if let Some(j) = &d.jetstream
                && (!matches!(j.scheme(), "ws" | "wss") || j.host().is_none())
            {
                return Err(ConfigError::Invalid(
                    "network.jetstream must be a ws(s) URL".into(),
                ));
            }
        }
        Ok(())
    }

    /// Parses TOML text.
    pub fn from_toml_str(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Serialises to TOML text.
    pub fn to_toml_string(&self) -> String {
        // Serialising plain data with string keys cannot fail.
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Loads `path`; a missing file gives the defaults. The result is validated.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let Some(bytes) = fsutil::read_private(path, CONFIG_MAX_BYTES, false)
            .or_else(|e| follow_user_symlink(path, e))
            .map_err(io_err(path))?
        else {
            return Ok(Self::default());
        };
        let text = String::from_utf8(bytes).map_err(|_| ConfigError::Parse {
            path: path.to_path_buf(),
            message: "not UTF-8".into(),
        })?;
        let cfg = Self::from_toml_str(&text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Saves to `path` atomically with mode 0600 (directory 0700). A symlink at `path` is
    /// replaced by a regular file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        fsutil::write_private_atomic(path, self.to_toml_string().as_bytes()).map_err(io_err(path))
    }
}

/// `config.toml` is not secret, and users commonly symlink it from a dotfiles repository, so
/// a symlink is followed here (secrets use [`fsutil::read_private`] directly, which refuses).
fn follow_user_symlink(path: &Path, e: io::Error) -> io::Result<Option<Vec<u8>>> {
    let is_link = std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_link {
        return Err(e);
    }
    let target = std::fs::canonicalize(path)?;
    fsutil::read_private(&target, CONFIG_MAX_BYTES, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml_string();
        assert_eq!(Config::from_toml_str(&text).unwrap(), cfg);
        assert_eq!(Config::from_toml_str("").unwrap(), cfg);
        cfg.validate().unwrap();
        assert!(!cfg.net_policy().is_devnet());
    }

    #[test]
    fn devnet_profile_parses() {
        let text = r#"
            sources = "direct"
            indexer = ["https://idx.example.com"]

            [network]
            mode = "devnet"
            plc = "http://localhost:2582"
            pds = "http://localhost:2583"
        "#;
        let cfg = Config::from_toml_str(text).unwrap();
        assert_eq!(cfg.sources, Sources::Direct);
        assert!(cfg.is_devnet());
        assert_eq!(cfg.effective_plc().as_str(), "http://localhost:2582/");
        assert!(matches!(cfg.indexer, IndexerSetting::Urls(ref u) if u.len() == 1));
        cfg.validate().unwrap();
        assert!(cfg.net_policy().is_devnet());
        let back = Config::from_toml_str(&cfg.to_toml_string()).unwrap();
        assert_eq!(back, cfg);
        assert!(
            Config::from_toml_str("indexer = \"none\"").unwrap().indexer
                == IndexerSetting::Mode(IndexerMode::None)
        );
    }

    #[test]
    fn production_rejects_plain_http() {
        let cfg = Config::from_toml_str("plc = \"http://plc.example\"").unwrap();
        assert!(matches!(cfg.validate(), Err(ConfigError::Invalid(_))));
        assert!(Config::from_toml_str("sources = \"sideways\"").is_err());
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert_eq!(Config::load(&paths.config_file).unwrap(), Config::default());
        let cfg = Config::devnet(
            Url::parse("http://localhost:1").unwrap(),
            Url::parse("http://localhost:2").unwrap(),
        );
        cfg.save(&paths.config_file).unwrap();
        assert_eq!(Config::load(&paths.config_file).unwrap(), cfg);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&paths.config_file)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            // A symlinked config (dotfiles) is followed on load.
            let link = dir.path().join("link.toml");
            std::os::unix::fs::symlink(&paths.config_file, &link).unwrap();
            assert_eq!(Config::load(&link).unwrap(), cfg);
        }
    }

    #[test]
    fn paths_layout() {
        let p = Paths::under(Path::new("/x"));
        assert_eq!(p.state_db(), Path::new("/x/state.sqlite"));
        assert_eq!(p.cache_db(), Path::new("/x/cache/cache.sqlite"));
        assert_eq!(p.blobs_dir(), Path::new("/x/cache/blobs"));
        assert_eq!(p.sync_leader_lock_file(), Path::new("/x/cache/cache.lock"));
        assert_eq!(p.lock_file("sync-leader"), Some(p.sync_leader_lock_file()));
        let repo = p.lock_file("repo:did:plc:abc").unwrap();
        assert!(repo.starts_with("/x/cache/locks"));
        let name = repo.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), "repo-.lock".len() + 16);
        assert!(
            p.lock_file("outbox:did:plc:abc")
                .unwrap()
                .starts_with("/x/locks")
        );
        assert!(
            p.lock_file("refresh:did:plc:abc")
                .unwrap()
                .starts_with("/x/locks")
        );
        assert_eq!(p.lock_file("nope"), None);
        assert_eq!(
            Paths::resolve(Some(Path::new("/y"))).unwrap(),
            Paths::under(Path::new("/y"))
        );
    }

    #[test]
    fn devnets_have_their_own_directories() {
        let p = Paths::under(Path::new("/x"));
        let dev = |plc: &str, pds: &str| DevnetProfile {
            plc: Url::parse(plc).unwrap(),
            pds: Url::parse(pds).unwrap(),
            jetstream: None,
        };
        let a = dev("http://localhost:2582", "http://localhost:2583");
        let d = p.for_devnet(&a);
        let id = a.id();
        assert_eq!(id.len(), 16);
        assert_eq!(d.config_file, p.config_file, "the configuration is shared");
        assert_eq!(d.data_dir, Path::new("/x").join(format!("devnet-{id}")));
        assert_eq!(
            d.cache_dir,
            Path::new("/x/cache").join(format!("devnet-{id}"))
        );
        assert!(d.state_db().starts_with(&d.data_dir));
        assert!(d.cache_db().starts_with(&d.cache_dir));
        assert_ne!(d.state_db(), p.state_db());
        assert_ne!(d.cache_db(), p.cache_db());
        // Another devnet, another place; the same devnet, the same place.
        let b = dev("http://localhost:2582", "http://localhost:9999");
        assert_ne!(p.for_devnet(&b).data_dir, d.data_dir);
        assert_eq!(p.for_devnet(&a.clone()), d);
        assert_eq!(a.keyring_service(), format!("lessdugin-devnet-{id}"));
    }

    #[test]
    fn b32s_matches_spec_alphabet() {
        // MSB-first 5-bit groups; the trailing partial group is zero-padded.
        assert_eq!(b32s(&[0x00, 0x00]), "2222");
        assert_eq!(b32s(&[0xff, 0xff]), "zzzk");
        assert_eq!(b32s(&[0b1000_0100, 0b0010_0000]), "kkk2");
        assert_eq!(lock_hash("x").len(), 16);
        assert!(
            lock_hash("did:plc:abc")
                .bytes()
                .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c))
        );
    }

    #[test]
    fn sync_leader_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let first = paths.try_sync_leader().unwrap().expect("free");
        assert!(
            paths.try_sync_leader().unwrap().is_none(),
            "held by the first lock"
        );
        drop(first);
        let again = paths.try_sync_leader().unwrap();
        assert!(again.is_some(), "released on drop");
    }

    #[tokio::test]
    async fn named_locks_wait() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let held = paths
            .lock("outbox:did:plc:abc", false)
            .await
            .unwrap()
            .unwrap();
        assert!(
            paths
                .lock("outbox:did:plc:abc", false)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            paths
                .lock("outbox:did:plc:other", false)
                .await
                .unwrap()
                .is_some()
        );
        let p2 = paths.clone();
        let waiter = tokio::spawn(async move { p2.lock("outbox:did:plc:abc", true).await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiter.is_finished());
        drop(held);
        assert!(waiter.await.unwrap().unwrap().is_some());
        assert!(paths.lock("bogus", false).await.is_err());
    }
}
