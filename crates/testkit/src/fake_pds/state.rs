// SPDX-License-Identifier: AGPL-3.0-or-later
//! The shared state of a [`super::FakeNet`]: PDS hosts, accounts, faults, the request log and
//! the event log.

use std::collections::BTreeMap;
use std::sync::Arc;

use k256::ecdsa::SigningKey;
use lessdugin_atp::Cid;
use lessdugin_atp::syntax::{Did, Tid};
use serde_json::Value;

use super::faults::{ActiveFault, CursorMode, ForkSelector, RateState};
use super::log::{LoggedRequest, RepoEvent, RepoEventKind};
use super::repo::Repo;
use super::{NetConfig, PdsConfig};

/// An account's hosting status, as `com.atproto.sync.getRepoStatus` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountStatus {
    /// Active.
    #[default]
    Active,
    /// Deactivated by the user (`RepoDeactivated`).
    Deactivated,
    /// Taken down by the host (`RepoTakendown`).
    Takendown,
    /// Suspended (`RepoSuspended`).
    Suspended,
    /// Deleted: the PDS no longer knows the repo (`RepoNotFound`).
    Deleted,
}

impl AccountStatus {
    /// The `status` string of `getRepoStatus` (`None` when active).
    pub fn as_status(self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::Deactivated => Some("deactivated"),
            Self::Takendown => Some("takendown"),
            Self::Suspended => Some("suspended"),
            Self::Deleted => Some("deleted"),
        }
    }
}

/// A stored blob.
#[derive(Clone, Debug)]
pub(crate) struct StoredBlob {
    pub bytes: Arc<Vec<u8>>,
    pub mime: String,
    /// Referenced by a record at least once (served by `getBlob`; collected when the last
    /// reference goes). Temporary uploads are never served and wait for `gc_blobs`.
    pub permanent: bool,
}

/// An equivocation fork: what some callers see instead of the real repo.
#[derive(Clone, Debug)]
pub(crate) struct Fork {
    pub repo: Repo,
    pub selector: ForkSelector,
    pub requests: u64,
}

pub(crate) struct Account {
    pub did: Did,
    pub handle: String,
    pub pds: String,
    pub former_pds: Vec<String>,
    pub key: SigningKey,
    pub email: String,
    pub password: String,
    pub app_passwords: Vec<String>,
    pub status: AccountStatus,
    pub plc_tombstoned: bool,
    pub doc_override: Option<Value>,
    pub repo: Repo,
    pub blobs: BTreeMap<Cid, StoredBlob>,
    pub cursor_mode: Option<CursorMode>,
    pub fork: Option<Fork>,
    pub clock_id: u16,
    pub last_tid_us: u64,
}

impl Account {
    /// A fresh TID: the clock in µs, strictly increasing per account.
    pub fn next_tid(&mut self, now_ms: i64) -> Tid {
        let now_us = u64::try_from(now_ms.saturating_mul(1000)).unwrap_or(0);
        let us = now_us.max(self.last_tid_us + 1);
        self.last_tid_us = us;
        Tid::from_parts(us, self.clock_id).expect("TID time in range")
    }

    /// Notes a TID chosen elsewhere (an explicit rev), so later ones stay above it.
    pub fn observe_tid(&mut self, tid: &Tid) {
        self.last_tid_us = self.last_tid_us.max(tid.timestamp_micros());
    }

    /// The DID document served by the PLC directory.
    pub fn doc(&self) -> Value {
        if let Some(d) = &self.doc_override {
            return d.clone();
        }
        crate::identity::did_doc(
            self.did.as_str(),
            Some(&self.handle),
            &crate::identity::multikey(&self.key),
            &self.pds,
        )
    }

    /// True if this account's repo is served (in any status) by `base`.
    pub fn known_at(&self, base: &str) -> bool {
        self.status != AccountStatus::Deleted && self.pds == base
    }

    /// Commits pending writes, if any; returns the commit event.
    pub fn ensure_commit(&mut self, now_ms: i64) -> Option<RepoEventKind> {
        if !self.repo.is_dirty() {
            return None;
        }
        let rev = self.next_tid(now_ms);
        let ops = self.repo.recommit(&self.did, &self.key, rev);
        self.settle_blobs();
        Some(RepoEventKind::Commit {
            rev,
            cid: self.repo.commit.cid,
            ops,
        })
    }

    /// Marks referenced blobs permanent and collects permanent blobs no record references.
    pub fn settle_blobs(&mut self) {
        let referenced = self.repo.referenced_blobs();
        self.blobs.retain(|cid, b| {
            if referenced.contains(cid) {
                b.permanent = true;
                true
            } else {
                !b.permanent
            }
        });
    }
}

/// A session token.
#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub did: String,
    pub refresh: bool,
    pub exp_ms: i64,
    pub revoked: bool,
    pub app_password: bool,
}

/// An opaque listing cursor.
#[derive(Clone, Debug)]
pub(crate) struct OpaqueCursor {
    pub did: String,
    pub collection: String,
    pub ascending: bool,
    pub rkey: String,
}

pub(crate) struct PdsState {
    pub label: String,
    pub server_did: String,
    pub cfg: PdsConfig,
    pub tokens: BTreeMap<String, Token>,
    pub cursors: BTreeMap<String, OpaqueCursor>,
    pub rate: Option<RateState>,
}

pub(crate) struct NetState {
    pub config: NetConfig,
    pub pds: BTreeMap<String, PdsState>,
    pub pds_order: Vec<String>,
    pub accounts: BTreeMap<String, Account>,
    pub faults: Vec<ActiveFault>,
    pub next_id: u64,
    pub log: Vec<LoggedRequest>,
    pub events: Vec<RepoEvent>,
    pub request_seq: u64,
    pub event_seq: u64,
}

impl NetState {
    pub fn new(config: NetConfig) -> Self {
        Self {
            config,
            pds: BTreeMap::new(),
            pds_order: Vec::new(),
            accounts: BTreeMap::new(),
            faults: Vec::new(),
            next_id: 0,
            log: Vec::new(),
            events: Vec::new(),
            request_seq: 0,
            event_seq: 0,
        }
    }

    /// A fresh counter value (tokens, ids).
    pub fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Appends an event.
    pub fn emit(&mut self, did: &Did, now_ms: i64, kind: RepoEventKind) {
        self.event_seq += 1;
        self.events.push(RepoEvent {
            seq: self.event_seq,
            time_us: now_ms.saturating_mul(1000),
            did: did.clone(),
            kind,
        });
    }

    /// Commits the pending writes of `did`, if any.
    pub fn ensure_commit(&mut self, did: &str, now_ms: i64) {
        let Some(acct) = self.accounts.get_mut(did) else {
            return;
        };
        if let Some(ev) = acct.ensure_commit(now_ms) {
            let did = acct.did.clone();
            self.emit(&did, now_ms, ev);
        }
    }

    /// The DID for a handle or DID (any status except deleted).
    pub fn resolve_ident(&self, ident: &str) -> Option<String> {
        if ident.starts_with("did:") {
            return Some(ident.to_owned());
        }
        let h = ident.to_ascii_lowercase();
        self.accounts
            .values()
            .find(|a| a.status != AccountStatus::Deleted && a.handle == h)
            .map(|a| a.did.to_string())
    }

    /// A deterministic opaque token.
    pub fn token(&mut self, kind: &str) -> String {
        let n = self.next();
        let digest = lessdugin_atp::cid::sha256(format!("fake-pds-{kind}:{n}").as_bytes());
        data_encoding::BASE32_NOPAD
            .encode(&digest[..15])
            .to_ascii_lowercase()
    }
}
