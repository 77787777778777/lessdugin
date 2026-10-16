// SPDX-License-Identifier: AGPL-3.0-or-later
//! Session persistence (`threat-model.md` AUTH-7).
//!
//! A session (tokens, and for OAuth the private DPoP key) is stored as one JSON secret:
//!
//! - in the OS keyring (`keyring` crate: Keychain, Credential Manager, Secret Service), service
//!   [`KEYRING_SERVICE`], account = the DID;
//! - when no keyring backend is usable (headless Linux CI, no D-Bus session) or
//!   `LESSDUGIN_KEYRING=off`: in `<data>/sessions/<h(did)>.json`, a 0600 file in a 0700
//!   directory, written to an `O_EXCL | O_NOFOLLOW` temporary name and renamed atomically, and
//!   refused on read if it is a symlink or grants any group or world permission.
//!
//! App passwords themselves are never stored (AUTH-3), only the resulting session.

use std::path::{Path, PathBuf};

use lessdugin_core::engine::{EngineError, ErrorCode};
use serde::{Deserialize, Serialize};

use super::redact::{Secret, SecretJson};
use crate::config::{Paths, lock_hash};
use crate::fsutil;

/// The keyring service name.
pub const KEYRING_SERVICE: &str = "lessdugin";
/// Environment variable: `off` (or `0`, `false`, `file`) keeps sessions out of the OS keyring.
pub const KEYRING_ENV: &str = "LESSDUGIN_KEYRING";
/// Largest accepted session secret.
pub const SESSION_MAX_BYTES: u64 = 256 * 1024;
/// Version of the stored session format.
pub const SESSION_VERSION: u32 = 1;

/// A stored session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StoredSession {
    /// An app-password session (`createSession`).
    #[serde(rename = "appPassword")]
    AppPassword(AppPasswordData),
    /// An OAuth session (DPoP-bound tokens).
    #[serde(rename = "oauth")]
    Oauth(OAuthData),
}

impl StoredSession {
    /// The account DID.
    pub fn did(&self) -> &str {
        match self {
            Self::AppPassword(d) => &d.did,
            Self::Oauth(d) => &d.did,
        }
    }

    /// The PDS bound at login.
    pub fn pds(&self) -> &str {
        match self {
            Self::AppPassword(d) => &d.pds,
            Self::Oauth(d) => &d.pds,
        }
    }
}

/// An app-password session. `Debug` redacts the tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPasswordData {
    /// Format version.
    pub v: u32,
    /// The account DID.
    pub did: String,
    /// The handle reported by the PDS.
    pub handle: String,
    /// The PDS bound at login (`scheme://host[:port]`).
    pub pds: String,
    /// The access JWT.
    pub access_jwt: Secret,
    /// The refresh JWT (single use).
    pub refresh_jwt: Secret,
    /// Expiry of the access JWT (its `exp`), if readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_exp_ms: Option<i64>,
    /// Expiry of the refresh JWT, if readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_exp_ms: Option<i64>,
    /// When the session was created (ms).
    pub created_ms: i64,
}

/// An OAuth session. `Debug` redacts the tokens and the DPoP key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthData {
    /// Format version.
    pub v: u32,
    /// The account DID (the token's `sub`).
    pub did: String,
    /// The PDS bound at login.
    pub pds: String,
    /// The authorization server issuer (`iss`).
    pub issuer: String,
    /// The token endpoint, bound at login.
    pub token_endpoint: String,
    /// The revocation endpoint, if the server has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    /// The `client_id` used at login (refreshes must use the same one).
    pub client_id: String,
    /// The redirect URI used at login.
    pub redirect_uri: String,
    /// The granted scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The access token (DPoP-bound).
    pub access_token: Secret,
    /// The refresh token (single use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<Secret>,
    /// Expiry of the access token, from `expires_in`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    /// The DPoP key and the authorization server's last nonce (jacquard's `DpopReqData`).
    pub dpop: SecretJson,
    /// The PDS's last DPoP nonce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpop_host_nonce: Option<String>,
    /// When the session was created (ms).
    pub created_ms: i64,
}

/// Where a session was stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The OS keyring.
    Keyring,
    /// The 0600 file fallback.
    File(PathBuf),
}

/// Whether the OS keyring can be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyringStatus {
    /// A keyring backend answered.
    Available,
    /// Disabled by `LESSDUGIN_KEYRING=off`.
    Disabled,
    /// No usable backend (the reason).
    Unavailable(String),
}

/// Session storage: the OS keyring with a private-file fallback.
#[derive(Clone, Debug)]
pub struct SecretStore {
    dir: PathBuf,
    use_keyring: bool,
}

fn storage_error(what: &str, e: impl std::fmt::Display) -> EngineError {
    EngineError::internal(format!("{what}: {e}"))
}

/// `true` unless `LESSDUGIN_KEYRING` disables the keyring.
pub fn keyring_enabled_by_env() -> bool {
    match std::env::var(KEYRING_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no" | "file"
        ),
        Err(_) => true,
    }
}

impl SecretStore {
    /// The store for `paths`; the keyring is used unless `LESSDUGIN_KEYRING` disables it.
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.data_dir.join("sessions"),
            use_keyring: keyring_enabled_by_env(),
        }
    }

    /// A store that only uses files in `dir` (tests).
    pub fn file_only(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            use_keyring: false,
        }
    }

    /// The fallback directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the keyring is tried at all.
    pub fn uses_keyring(&self) -> bool {
        self.use_keyring
    }

    /// The fallback file of `did`.
    pub fn session_file(&self, did: &str) -> PathBuf {
        self.dir.join(format!("{}.json", lock_hash(did)))
    }

    /// Checks whether the keyring works (a read of an entry that does not exist).
    pub async fn keyring_status(&self) -> KeyringStatus {
        if !self.use_keyring {
            return KeyringStatus::Disabled;
        }
        let probe = tokio::task::spawn_blocking(|| {
            // The store's own initialisation error says why (e.g. no D-Bus session).
            if let Err(e) = keyring::Entry::store_status() {
                return Err(e.to_string());
            }
            let entry = keyring::Entry::new(KEYRING_SERVICE, "lessdugin-doctor-probe")
                .map_err(|e| e.to_string())?;
            match entry.get_secret() {
                Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        })
        .await;
        match probe {
            Ok(Ok(())) => KeyringStatus::Available,
            Ok(Err(e)) => KeyringStatus::Unavailable(e),
            Err(e) => KeyringStatus::Unavailable(e.to_string()),
        }
    }

    /// Loads the session of `did`: the keyring first, then the file fallback.
    pub async fn load(&self, did: &str) -> Result<Option<StoredSession>, EngineError> {
        if self.use_keyring {
            let account = did.to_owned();
            let got = tokio::task::spawn_blocking(move || {
                keyring::Entry::new(KEYRING_SERVICE, &account)?.get_secret()
            })
            .await
            .map_err(|e| storage_error("keyring", e))?;
            match got {
                Ok(bytes) => return parse_session(&bytes, did).map(Some),
                Err(keyring::Error::NoEntry) => {}
                Err(e) => {
                    tracing::debug!(error = %e, "keyring unavailable; trying the session file")
                }
            }
        }
        let path = self.session_file(did);
        let bytes = tokio::task::spawn_blocking(move || {
            fsutil::read_private(&path, SESSION_MAX_BYTES, true)
        })
        .await
        .map_err(|e| storage_error("session file", e))?
        .map_err(|e| {
            EngineError::blocked(format!("refusing to read the session file: {e}"))
                .with_detail("kind", "insecureSessionFile")
        })?;
        match bytes {
            Some(b) => parse_session(&b, did).map(Some),
            None => Ok(None),
        }
    }

    /// Saves a session: in the keyring if it works (removing any fallback file), else in the
    /// fallback file.
    pub async fn save(&self, session: &StoredSession) -> Result<Backend, EngineError> {
        let bytes = serde_json::to_vec(session).map_err(|e| storage_error("session", e))?;
        let did = session.did().to_owned();
        if self.use_keyring {
            let (account, secret) = (did.clone(), bytes.clone());
            let set = tokio::task::spawn_blocking(move || {
                keyring::Entry::new(KEYRING_SERVICE, &account)?.set_secret(&secret)
            })
            .await
            .map_err(|e| storage_error("keyring", e))?;
            match set {
                Ok(()) => {
                    let path = self.session_file(&did);
                    let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
                    return Ok(Backend::Keyring);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "keyring unavailable; using the session file");
                }
            }
        }
        let path = self.session_file(&did);
        let target = path.clone();
        tokio::task::spawn_blocking(move || fsutil::write_private_atomic(&target, &bytes))
            .await
            .map_err(|e| storage_error("session file", e))?
            .map_err(|e| storage_error("cannot write the session file", e))?;
        Ok(Backend::File(path))
    }

    /// Deletes the session of `did` from the keyring and the fallback file.
    pub async fn delete(&self, did: &str) -> Result<(), EngineError> {
        if self.use_keyring {
            let account = did.to_owned();
            let del = tokio::task::spawn_blocking(move || {
                keyring::Entry::new(KEYRING_SERVICE, &account)?.delete_credential()
            })
            .await
            .map_err(|e| storage_error("keyring", e))?;
            match del {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => tracing::debug!(error = %e, "keyring delete failed"),
            }
        }
        let path = self.session_file(did);
        match tokio::task::spawn_blocking(move || std::fs::remove_file(path))
            .await
            .map_err(|e| storage_error("session file", e))?
        {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(storage_error("cannot delete the session file", e)),
        }
    }
}

fn parse_session(bytes: &[u8], did: &str) -> Result<StoredSession, EngineError> {
    let s: StoredSession = serde_json::from_slice(bytes).map_err(|e| {
        EngineError::new(
            ErrorCode::AuthRequired,
            format!("the stored session is unreadable ({e}); log in again"),
        )
    })?;
    if s.did() != did {
        return Err(EngineError::blocked(format!(
            "the stored session belongs to {}, not {did}",
            s.did()
        )));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(did: &str) -> StoredSession {
        StoredSession::AppPassword(AppPasswordData {
            v: SESSION_VERSION,
            did: did.into(),
            handle: "alice.test".into(),
            pds: "http://localhost:2583".into(),
            access_jwt: Secret::new("access-secret"),
            refresh_jwt: Secret::new("refresh-secret"),
            access_exp_ms: Some(1),
            refresh_exp_ms: None,
            created_ms: 0,
        })
    }

    #[test]
    fn debug_redacts_tokens() {
        let s = format!("{:?}", sample("did:plc:x"));
        assert!(
            !s.contains("access-secret") && !s.contains("refresh-secret"),
            "{s}"
        );
        let o = StoredSession::Oauth(OAuthData {
            v: 1,
            did: "did:plc:x".into(),
            pds: "p".into(),
            issuer: "i".into(),
            token_endpoint: "t".into(),
            revocation_endpoint: None,
            client_id: "c".into(),
            redirect_uri: "r".into(),
            scope: None,
            access_token: Secret::new("at-secret"),
            refresh_token: Some(Secret::new("rt-secret")),
            expires_at_ms: None,
            dpop: SecretJson(serde_json::json!({"dpop_key": {"d": "jwk-private"}})),
            dpop_host_nonce: None,
            created_ms: 0,
        });
        let s = format!("{o:?}");
        assert!(
            !s.contains("at-secret") && !s.contains("rt-secret") && !s.contains("jwk-private"),
            "{s}"
        );
        let json = serde_json::to_string(&o).unwrap();
        assert!(json.starts_with(r#"{"kind":"oauth","#), "{json}");
    }

    #[tokio::test]
    async fn file_fallback_roundtrip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::file_only(dir.path().join("sessions"));
        let did = "did:plc:abc";
        assert_eq!(store.load(did).await.unwrap(), None);
        let backend = store.save(&sample(did)).await.unwrap();
        let path = store.session_file(did);
        assert_eq!(backend, Backend::File(path.clone()));
        assert_eq!(store.load(did).await.unwrap(), Some(sample(did)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            // Group-readable: refused.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
            let err = store.load(did).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::Blocked, "{err:?}");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            // A symlink in place of the file: refused.
            let real = dir.path().join("real.json");
            std::fs::rename(&path, &real).unwrap();
            std::os::unix::fs::symlink(&real, &path).unwrap();
            let err = store.load(did).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::Blocked);
            std::fs::remove_file(&path).unwrap();
        }
        // A session file of another account is refused.
        store.save(&sample("did:plc:other")).await.unwrap();
        std::fs::copy(store.session_file("did:plc:other"), store.session_file(did)).unwrap();
        assert_eq!(store.load(did).await.unwrap_err().code, ErrorCode::Blocked);
        store.delete(did).await.unwrap();
        store.delete(did).await.unwrap();
        assert_eq!(store.load(did).await.unwrap(), None);
    }
}
