// SPDX-License-Identifier: AGPL-3.0-or-later
//! Accounts and the login binding (`engine-api.md` §3.1, `sync.md` §3.6, `data-model.md` §10.5).
//!
//! The core never sees tokens or passwords: host-specific login code (OAuth or app password)
//! obtains a session, then calls [`Engine::add_account`] with the session's DID, PDS and (OAuth)
//! issuer **before using the session for anything**. `add_account` checks the binding:
//!
//! ```text
//! intended ∧ did ≠ intended                     → conflict  (detail.kind = "accountMismatch")
//! doc = fresh DID document of did (level doc)   → else not_found / blocked (hint)
//! session PDS ≠ PDS of doc                      → conflict  (detail.kind = "pdsMismatch")
//! OAuth: iss ∉ <pds>/.well-known/oauth-protected-resource .authorization_servers
//!                                               → blocked   (detail.kind = "issuerNotListed")
//!        pin absent → pin iss (TOFU); pin = iss → ok;
//!        pin ≠ iss  → conflict (detail.kind = "issuerChanged", pinned, issuer) unless
//!                     `confirm_issuer_change`, then re-pin
//! ```
//!
//! Login code that must order its own steps (store the session only after the binding holds,
//! record the account only after the session is stored) calls [`Engine::bind_login`], then
//! [`Engine::record_account`] with the [`LoginBinding`] it returned: the binding is checked once.
//! A `LoginBinding` can only be made by `bind_login`.
//!
//! [`Engine::refresh_account`] re-checks a recorded account against its current DID document:
//! the recorded handle follows the identity (verified both ways, or none), and a document whose
//! PDS is no longer the one bound at login means `needs-relogin` (`threat-model.md` AUTH-8).
//!
//! State rows (in `state`, `sync.md` §10.4 `loc` kinds): `("loc", "account", did)`,
//! `("loc", "active_account")` and `("loc", "as_pin", did)`.

use serde::{Deserialize, Serialize};

use lessdugin_atp::syntax::Did;

use super::{Engine, EngineError};
use crate::dto::{AccountView, AuthKind, Topic};
use crate::engine::ErrorCode;
use crate::host::{Dir, Host, KeyRange, KvOp, KvStore};
use crate::identity::IdentityLevel;
use crate::identity::doc::check_service_endpoint;
use crate::limits;
use crate::store::row::{self, ROW_VERSION};
use crate::xrpc::{CallOpts, XrpcCallError};

/// What host login code reports after obtaining a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    /// The session's DID (the token's `sub`, or `createSession`'s `did`).
    pub did: String,
    /// The PDS the session talks to.
    pub pds: String,
    /// OAuth or app password.
    pub auth: AuthKind,
    /// The OAuth authorization server issuer (`iss`); required for OAuth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// The DID the user asked to log in as (resolved from their input), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intended: Option<String>,
    /// The user confirmed a changed authorization server (`sync.md` §3.6).
    #[serde(default)]
    pub confirm_issuer_change: bool,
}

impl AccountInfo {
    /// An app-password session (`createSession` at the DID document's PDS).
    pub fn app_password(did: impl Into<String>, pds: impl Into<String>) -> Self {
        Self {
            did: did.into(),
            pds: pds.into(),
            auth: AuthKind::AppPassword,
            issuer: None,
            intended: None,
            confirm_issuer_change: false,
        }
    }

    /// An OAuth session from the token response's `sub` and `iss`.
    pub fn oauth(
        did: impl Into<String>,
        pds: impl Into<String>,
        issuer: impl Into<String>,
    ) -> Self {
        Self {
            auth: AuthKind::Oauth,
            issuer: Some(issuer.into()),
            ..Self::app_password(did, pds)
        }
    }

    /// Sets the intended DID.
    pub fn with_intended(mut self, did: impl Into<String>) -> Self {
        self.intended = Some(did.into());
        self
    }

    /// Confirms an authorization-server change.
    pub fn confirming_issuer_change(mut self) -> Self {
        self.confirm_issuer_change = true;
        self
    }
}

/// The authorization-server pin outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PinStatus {
    /// App-password session: no issuer.
    NotApplicable,
    /// First login for this DID: the issuer was pinned (show it to the user).
    New,
    /// The issuer equals the pin.
    Same,
    /// The issuer changed and the change was confirmed; re-pinned.
    Changed,
}

/// Proof that a [`LoginBinding`] came from [`Engine::bind_login`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sealed;

/// A verified login binding. Only [`Engine::bind_login`] makes one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginBinding {
    /// The account DID.
    pub did: String,
    /// The PDS from the fresh DID document.
    pub pds: String,
    /// The handle, if verified in both directions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// The OAuth issuer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// The pin outcome.
    pub pin: PinStatus,
    /// The previously pinned issuer, when it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_issuer: Option<String>,
    /// The auth kind that was bound.
    #[serde(skip)]
    auth: Option<AuthKind>,
    #[serde(skip)]
    _sealed: Sealed,
}

/// `true` for the app-password shape `xxxx-xxxx-xxxx-xxxx` (`threat-model.md` AUTH-3). Login code
/// refuses anything else before any network call, so main passwords are never sent.
pub fn is_app_password_shape(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 4
        && groups
            .iter()
            .all(|g| g.len() == 4 && g.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountRow {
    v: u32,
    did: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    pds: String,
    auth: AuthKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    iss: Option<String>,
    added: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActiveRow {
    v: u32,
    did: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PinRow {
    v: u32,
    iss: String,
    at: i64,
}

#[derive(Deserialize)]
struct ProtectedResource {
    #[serde(default)]
    authorization_servers: Vec<String>,
}

fn account_key(did: &str) -> Vec<u8> {
    crate::key!("loc", "account", did)
}

fn active_key() -> Vec<u8> {
    crate::key!("loc", "active_account")
}

fn pin_key(did: &str) -> Vec<u8> {
    crate::key!("loc", "as_pin", did)
}

fn parse_did(s: &str) -> Result<Did, EngineError> {
    let did =
        Did::parse(s.trim()).map_err(|e| EngineError::invalid_input(format!("{s:?}: {e}")))?;
    did.check_lessdugin_policy()
        .map_err(|e| EngineError::invalid_input(format!("{s:?}: {e}")))?;
    Ok(did)
}

impl AccountRow {
    fn view(&self, active: Option<&str>) -> AccountView {
        AccountView {
            did: self.did.clone(),
            handle: self.handle.clone(),
            pds: self.pds.clone(),
            auth_kind: self.auth,
            active: active == Some(self.did.as_str()),
        }
    }
}

impl<H: Host> Engine<H> {
    async fn state_get<T: serde::de::DeserializeOwned>(
        &self,
        key: Vec<u8>,
    ) -> Result<Option<T>, EngineError> {
        let mut v = self.host().state().get_many(vec![key]).await?;
        Ok(v.pop().flatten().and_then(|b| row::decode(&b).ok()))
    }

    async fn state_write(&self, ops: Vec<KvOp>) -> Result<(), EngineError> {
        Ok(self.host().state().write(ops).await?)
    }

    fn encode_row<T: Serialize>(value: &T) -> Result<Vec<u8>, EngineError> {
        row::encode(value).map_err(|e| EngineError::internal(e.to_string()))
    }

    /// Checks the login binding of a new session (`sync.md` §3.6) without storing the account.
    /// On first OAuth login (and on a confirmed change) the issuer is pinned.
    pub async fn bind_login(&self, info: &AccountInfo) -> Result<LoginBinding, EngineError> {
        let did = parse_did(&info.did)?;
        if let Some(intended) = &info.intended {
            let intended = parse_did(intended)?;
            if intended != did {
                return Err(EngineError::conflict(format!(
                    "logged in as a different account ({did}, not {intended})"
                ))
                .with_detail("kind", "accountMismatch")
                .with_detail("intended", intended.as_str())
                .with_detail("did", did.as_str()));
            }
        }
        let ident = self.resolver().resolve_did_fresh(&did, 0).await?;
        if ident.level != IdentityLevel::Doc {
            return Err(EngineError::blocked(format!(
                "only a hint identity is available for {did}; login needs its DID document"
            )));
        }
        let pds = ident.pds.clone().ok_or_else(|| {
            EngineError::not_found(format!(
                "{did} has no usable PDS: {}",
                ident.pds_problem.as_deref().unwrap_or("unknown")
            ))
        })?;
        let session_pds = check_service_endpoint(&info.pds, self.config().is_devnet())
            .map_err(|e| EngineError::invalid_input(format!("session PDS {:?}: {e}", info.pds)))?;
        if session_pds != pds {
            return Err(EngineError::conflict(format!(
                "the session's PDS {session_pds} is not the PDS in {did}'s DID document ({pds})"
            ))
            .with_detail("kind", "pdsMismatch")
            .with_detail("sessionPds", session_pds)
            .with_detail("docPds", pds));
        }
        let mut binding = LoginBinding {
            did: did.to_string(),
            pds: pds.clone(),
            handle: ident.handle().map(str::to_owned),
            issuer: None,
            pin: PinStatus::NotApplicable,
            previous_issuer: None,
            auth: Some(info.auth),
            _sealed: Sealed,
        };
        if info.auth == AuthKind::AppPassword {
            return Ok(binding);
        }
        let iss = info
            .issuer
            .clone()
            .ok_or_else(|| EngineError::invalid_input("an OAuth session needs its issuer (iss)"))?;
        let servers = self.protected_resource_servers(&pds).await?;
        if !servers.contains(&iss) {
            return Err(EngineError::blocked(format!(
                "the authorization server {iss} is not listed by the PDS {pds}"
            ))
            .with_detail("kind", "issuerNotListed")
            .with_detail("issuer", iss.as_str())
            .with_detail("authorizationServers", servers));
        }
        let now = self.host().now_ms();
        let pin_row = || {
            Self::encode_row(&PinRow {
                v: ROW_VERSION,
                iss: iss.clone(),
                at: now,
            })
        };
        match self.state_get::<PinRow>(pin_key(did.as_str())).await? {
            None => {
                self.state_write(vec![KvOp::Put(pin_key(did.as_str()), pin_row()?)])
                    .await?;
                binding.pin = PinStatus::New;
            }
            Some(p) if p.iss == iss => binding.pin = PinStatus::Same,
            Some(p) if info.confirm_issuer_change => {
                self.state_write(vec![KvOp::Put(pin_key(did.as_str()), pin_row()?)])
                    .await?;
                binding.pin = PinStatus::Changed;
                binding.previous_issuer = Some(p.iss);
            }
            Some(p) => {
                return Err(EngineError::conflict(format!(
                    "authorization server changed: {} → {iss}; confirm to continue",
                    p.iss
                ))
                .with_detail("kind", "issuerChanged")
                .with_detail("pinned", p.iss)
                .with_detail("issuer", iss.as_str()));
            }
        }
        binding.issuer = Some(iss);
        Ok(binding)
    }

    async fn protected_resource_servers(&self, pds: &str) -> Result<Vec<String>, EngineError> {
        let url = url::Url::parse(&format!("{pds}/.well-known/oauth-protected-resource"))
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let opts = CallOpts::capped(limits::OAUTH_METADATA_MAX_BYTES);
        let resp = self
            .xrpc()
            .get_url(url, &opts)
            .await
            .map_err(|e: XrpcCallError| EngineError::from(e).context("oauth-protected-resource"))?;
        let prm: ProtectedResource = serde_json::from_slice(&resp.body).map_err(|e| {
            EngineError::blocked(format!("invalid oauth-protected-resource document: {e}"))
        })?;
        Ok(prm.authorization_servers)
    }

    /// Records a logged-in account after checking its login binding, and makes it the active
    /// account. Call it before using the session (`sync.md` §3.6).
    pub async fn add_account(&self, info: AccountInfo) -> Result<AccountView, EngineError> {
        let binding = self.bind_login(&info).await?;
        self.record_account(&info, &binding).await
    }

    /// Records the account of a binding that [`Engine::bind_login`] verified for `info` (without
    /// checking it a second time), and makes it the active account. Emits
    /// `Changed [Accounts]`; the followed list is seeded from the account's memberships at the
    /// next `followed()` or `live_tail` round (`sync.md` §4.8).
    pub async fn record_account(
        &self,
        info: &AccountInfo,
        binding: &LoginBinding,
    ) -> Result<AccountView, EngineError> {
        let did = parse_did(&info.did)?;
        if did.as_str() != binding.did
            || binding.auth != Some(info.auth)
            || (info.auth == AuthKind::Oauth && binding.issuer != info.issuer)
        {
            return Err(EngineError::internal(
                "the login binding belongs to another session",
            ));
        }
        let added = match self
            .state_get::<AccountRow>(account_key(&binding.did))
            .await?
        {
            Some(existing) => existing.added,
            None => self.host().now_ms(),
        };
        let row = AccountRow {
            v: ROW_VERSION,
            did: binding.did.clone(),
            handle: binding.handle.clone(),
            pds: binding.pds.clone(),
            auth: info.auth,
            iss: binding.issuer.clone(),
            added,
        };
        let active = ActiveRow {
            v: ROW_VERSION,
            did: binding.did.clone(),
        };
        self.state_write(vec![
            KvOp::Put(account_key(&binding.did), Self::encode_row(&row)?),
            KvOp::Put(active_key(), Self::encode_row(&active)?),
            // a login re-seeds the followed list (`sync.md` §4.8)
            KvOp::Delete(super::forums::seeded_key(&binding.did)),
        ])
        .await?;
        self.accounts_changed(false);
        Ok(row.view(Some(&binding.did)))
    }

    /// `Changed [Accounts]` (plus `Outbox` after a purge), and wakes the outbox pump so it
    /// picks up the new account set.
    fn accounts_changed(&self, purged: bool) {
        let mut topics = vec![Topic::Accounts];
        if purged {
            topics.push(Topic::Outbox);
        }
        self.rt.bus.changed(topics);
        self.outbox.wake_all();
    }

    /// Re-checks a recorded account against its DID document (the cached one, at most 24 h old):
    /// the recorded handle becomes the identity's handle verified both ways (or none), and a
    /// document whose PDS is not the PDS bound at login fails with `auth_required`
    /// (`detail.kind = "needsRelogin"`, `threat-model.md` AUTH-8) — never silent re-targeting.
    /// A hint identity changes nothing. Resolution failures are returned as errors and leave the
    /// record unchanged.
    pub async fn refresh_account(&self, did: &str) -> Result<AccountView, EngineError> {
        let did = parse_did(did)?;
        let Some(mut row) = self
            .state_get::<AccountRow>(account_key(did.as_str()))
            .await?
        else {
            return Err(EngineError::not_found(format!("no account {did}")));
        };
        let active = self.active_did().await?;
        let ident = self.resolver().resolve_did(&did).await?;
        if ident.level != IdentityLevel::Doc {
            return Ok(row.view(active.as_deref()));
        }
        if ident.pds.as_deref() != Some(row.pds.as_str()) {
            let now = ident
                .pds
                .clone()
                .or(ident.pds_problem.clone())
                .unwrap_or_default();
            return Err(EngineError::new(
                ErrorCode::AuthRequired,
                format!(
                    "the PDS of {did} changed ({} at login, now {now}); log in again",
                    row.pds
                ),
            )
            .with_detail("kind", "needsRelogin")
            .with_detail("did", did.as_str())
            .with_detail("boundPds", row.pds.as_str())
            .with_detail("docPds", ident.pds.as_deref().unwrap_or("")));
        }
        let handle = ident.handle().map(str::to_owned);
        if handle != row.handle {
            row.handle = handle;
            self.state_write(vec![KvOp::Put(
                account_key(did.as_str()),
                Self::encode_row(&row)?,
            )])
            .await?;
        }
        Ok(row.view(active.as_deref()))
    }

    async fn active_did(&self) -> Result<Option<String>, EngineError> {
        Ok(self
            .state_get::<ActiveRow>(active_key())
            .await?
            .map(|r| r.did))
    }

    /// Every recorded account, in the order they were first added.
    pub async fn accounts(&self) -> Result<Vec<AccountView>, EngineError> {
        let rows = self
            .host()
            .state()
            .scan(
                KeyRange::prefix(&crate::key!("loc", "account")),
                Dir::Forward,
                10_000,
            )
            .await?;
        let mut rows: Vec<AccountRow> = rows
            .iter()
            .filter_map(|(_, v)| row::decode(v).ok())
            .collect();
        rows.sort_by(|a, b| (a.added, &a.did).cmp(&(b.added, &b.did)));
        let active = self.active_did().await?;
        Ok(rows.iter().map(|r| r.view(active.as_deref())).collect())
    }

    /// One account.
    pub async fn account(&self, did: &str) -> Result<Option<AccountView>, EngineError> {
        let did = parse_did(did)?;
        let active = self.active_did().await?;
        Ok(self
            .state_get::<AccountRow>(account_key(did.as_str()))
            .await?
            .map(|r| r.view(active.as_deref())))
    }

    /// Forgets an account (emits `Changed [Accounts]`, plus `Outbox` with `purge`). With
    /// `purge`, also deletes its outbox, write budget and notification state
    /// (`threat-model.md` AUTH-11). The authorization-server pin is kept. Deleting the
    /// session secret is the host's job.
    pub async fn remove_account(&self, did: &str, purge: bool) -> Result<(), EngineError> {
        let did = parse_did(did)?;
        if self
            .state_get::<AccountRow>(account_key(did.as_str()))
            .await?
            .is_none()
        {
            return Err(EngineError::not_found(format!("no account {did}")));
        }
        let mut ops = vec![KvOp::Delete(account_key(did.as_str()))];
        if self.active_did().await?.as_deref() == Some(did.as_str()) {
            ops.push(KvOp::Delete(active_key()));
        }
        if purge {
            // writes, blob chunks, the write budget and the TID meta row
            ops.extend(crate::outbox::purge_ops(did.as_str()));
            ops.push(KvOp::DeleteRange(KeyRange::prefix(&crate::key!(
                "notif",
                did.as_str()
            ))));
        }
        ops.push(KvOp::Delete(super::forums::seeded_key(did.as_str())));
        self.state_write(ops).await?;
        self.accounts_changed(purge);
        Ok(())
    }

    /// Makes a recorded account the active one (emits `Changed [Accounts]`).
    pub async fn set_active_account(&self, did: &str) -> Result<(), EngineError> {
        let did = parse_did(did)?;
        if self
            .state_get::<AccountRow>(account_key(did.as_str()))
            .await?
            .is_none()
        {
            return Err(EngineError::not_found(format!("no account {did}")));
        }
        let active = ActiveRow {
            v: ROW_VERSION,
            did: did.to_string(),
        };
        self.state_write(vec![KvOp::Put(active_key(), Self::encode_row(&active)?)])
            .await?;
        self.accounts_changed(false);
        Ok(())
    }

    /// The active account, if any.
    pub async fn active_account(&self) -> Result<Option<AccountView>, EngineError> {
        match self.active_did().await? {
            Some(did) => Ok(self
                .state_get::<AccountRow>(account_key(&did))
                .await?
                .map(|r| r.view(Some(&did)))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_password_shape() {
        assert!(is_app_password_shape("abcd-EFGH-1234-x9z0"));
        assert!(!is_app_password_shape("hunter2"));
        assert!(!is_app_password_shape("abcd-efgh-ijkl"));
        assert!(!is_app_password_shape("abcd-efgh-ijkl-mnop-qrst"));
        assert!(!is_app_password_shape("abcd-efgh-ijkl-mno_"));
        assert!(!is_app_password_shape("abcd-efgh-ijkl-mnopq"));
        assert!(!is_app_password_shape("ábcd-efgh-ijkl-mnop"));
        assert!(!is_app_password_shape(" abcd-efgh-ijkl-mnop"));
    }
}
