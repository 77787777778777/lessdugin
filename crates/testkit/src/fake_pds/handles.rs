// SPDX-License-Identifier: AGPL-3.0-or-later
//! [`PdsHandle`], [`AccountHandle`] and [`RepoWriter`]: the builder side of a [`FakeNet`].

use std::collections::BTreeMap;
use std::sync::Arc;

use k256::ecdsa::SigningKey;
use lessdugin_atp::Cid;
use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::syntax::{Did, Tid};
use serde_json::{Value, json};

use super::faults::{CursorMode, FaultId, FaultRule, ForkSelector, RateState};
use super::log::RepoEventKind;
use super::repo::{Rec, Repo};
use super::state::{Account, AccountStatus, Fork, StoredBlob};
use super::{FakeNet, PdsConfig, RateLimit, sniff_mime};
use crate::identity::{fake_key, fake_plc_did};

/// A PDS on a [`FakeNet`].
#[derive(Clone)]
pub struct PdsHandle {
    pub(super) net: FakeNet,
    pub(super) base: String,
}

impl std::fmt::Debug for PdsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PdsHandle").field(&self.base).finish()
    }
}

impl PdsHandle {
    /// The base URL (`https://pds.example.com`, or `http://127.0.0.1:<port>` over HTTP).
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The base URL, parsed.
    pub fn url(&self) -> url::Url {
        url::Url::parse(&self.base).expect("valid base URL")
    }

    /// The name it was added with.
    pub fn label(&self) -> String {
        self.net.state().pds[&self.base].label.clone()
    }

    /// The server DID (`did:web:<host>`).
    pub fn server_did(&self) -> String {
        self.net.state().pds[&self.base].server_did.clone()
    }

    /// The configuration.
    pub fn config(&self) -> PdsConfig {
        self.net.state().pds[&self.base].cfg.clone()
    }

    /// Changes the configuration.
    pub fn configure(&self, f: impl FnOnce(&mut PdsConfig)) {
        let now = self.net.now();
        let mut st = self.net.state();
        let p = st.pds.get_mut(&self.base).expect("known PDS");
        let before = p.cfg.rate_limit;
        f(&mut p.cfg);
        if p.cfg.rate_limit != before {
            p.rate = p.cfg.rate_limit.map(|r| RateState::new(r, now));
        }
    }

    /// Sets the `listRecords` cursor behaviour.
    pub fn set_cursor_mode(&self, mode: CursorMode) {
        self.configure(|c| c.cursor_mode = mode);
    }

    /// Caps every `listRecords` page. Not a conforming PDS model: see
    /// [`super::PdsConfig::max_page`].
    pub fn set_max_page(&self, max: Option<u32>) {
        self.configure(|c| c.max_page = max);
    }

    /// Sets (or removes) the read budget; a new budget starts a fresh window.
    pub fn set_rate_limit(&self, limit: Option<RateLimit>) {
        let now = self.net.now();
        let mut st = self.net.state();
        let p = st.pds.get_mut(&self.base).expect("known PDS");
        p.cfg.rate_limit = limit;
        p.rate = limit.map(|r| RateState::new(r, now));
    }

    /// Sets `authorization_servers` in the protected-resource metadata.
    pub fn set_authorization_servers(&self, servers: Vec<String>) {
        self.configure(|c| c.authorization_servers = Some(servers));
    }

    /// Injects a fault limited to this PDS.
    pub fn inject(&self, rule: FaultRule) -> FaultId {
        self.net.inject(rule.on_host(&self.base))
    }

    /// Accounts hosted here (any status but deleted).
    pub fn accounts(&self) -> Vec<AccountHandle> {
        let dids: Vec<Did> = self
            .net
            .state()
            .accounts
            .values()
            .filter(|a| a.known_at(&self.base))
            .map(|a| a.did.clone())
            .collect();
        dids.into_iter()
            .map(|did| AccountHandle {
                net: self.net.clone(),
                did,
            })
            .collect()
    }

    /// Creates an account here: a deterministic `did:plc` and k256 key derived from the handle,
    /// a DID document (`#atproto` Multikey, `#atproto_pds` = this PDS, `alsoKnownAs` the handle)
    /// at the PLC directory, `_atproto` TXT answers on the DoH endpoints, a main password, one
    /// app password and an empty repo with a signed initial commit. Panics if the handle is
    /// taken.
    pub fn add_account(&self, handle: &str) -> AccountHandle {
        let handle = handle.to_ascii_lowercase();
        let now = self.net.now();
        let mut st = self.net.state();
        assert!(
            st.resolve_ident(&handle).is_none(),
            "handle {handle} is taken"
        );
        let mut n = 0u32;
        let did = loop {
            let seed = if n == 0 {
                handle.clone()
            } else {
                format!("{handle}#{n}")
            };
            let did = fake_plc_did(&seed);
            if !st.accounts.contains_key(did.as_str()) {
                break did;
            }
            n += 1;
        };
        let key = fake_key(did.as_str());
        let digest = lessdugin_atp::cid::sha256(did.as_str().as_bytes());
        let clock_id = u16::from_be_bytes([digest[0], digest[1]]) % 1024;
        let now_us = u64::try_from(now.saturating_mul(1000)).unwrap_or(0);
        let rev = Tid::from_parts(now_us, clock_id).expect("TID time in range");
        let local = handle.split('.').next().unwrap_or("user").to_owned();
        let repo = Repo::new(&did, &key, rev);
        let commit = (repo.commit.cid, repo.commit.rev);
        st.accounts.insert(
            did.to_string(),
            Account {
                did: did.clone(),
                handle: handle.clone(),
                pds: self.base.clone(),
                former_pds: Vec::new(),
                key,
                email: format!("{local}@mail.test"),
                password: format!("main-password-{local}"),
                app_passwords: vec![app_password(&did, 0)],
                status: AccountStatus::Active,
                plc_tombstoned: false,
                doc_override: None,
                repo,
                blobs: BTreeMap::new(),
                cursor_mode: None,
                fork: None,
                clock_id,
                last_tid_us: now_us,
            },
        );
        st.emit(
            &did,
            now,
            RepoEventKind::Identity {
                handle: Some(handle.clone()),
            },
        );
        st.emit(
            &did,
            now,
            RepoEventKind::Account {
                active: true,
                status: None,
            },
        );
        st.emit(
            &did,
            now,
            RepoEventKind::Commit {
                rev: commit.1,
                cid: commit.0,
                ops: Vec::new(),
            },
        );
        AccountHandle {
            net: self.net.clone(),
            did,
        }
    }
}

fn app_password(did: &Did, n: u32) -> String {
    let digest = lessdugin_atp::cid::sha256(format!("fake-app-password:{did}:{n}").as_bytes());
    let s = data_encoding::BASE32_NOPAD
        .encode(&digest)
        .to_ascii_lowercase();
    format!("{}-{}-{}-{}", &s[0..4], &s[4..8], &s[8..12], &s[12..16])
}

/// Writes to a repository (or an equivocation fork). Panics on invalid record JSON or keys
/// (test helper).
pub struct RepoWriter<'a> {
    repo: &'a mut Repo,
}

impl RepoWriter<'_> {
    /// Creates or replaces a record from atproto JSON; returns its CID.
    pub fn put(&mut self, collection: &str, rkey: &str, value: &Value) -> Cid {
        let cbor = lessdugin_atp::datamodel::record_to_cbor(value).expect("valid record JSON");
        self.put_cbor(collection, rkey, cbor)
    }

    /// Creates or replaces a record from DAG-CBOR bytes (stored as given); returns its CID.
    pub fn put_cbor(&mut self, collection: &str, rkey: &str, cbor: Vec<u8>) -> Cid {
        self.repo.put(collection, rkey, Rec::from_cbor(cbor))
    }

    /// Deletes a record; true if it existed.
    pub fn delete(&mut self, collection: &str, rkey: &str) -> bool {
        self.repo.delete(collection, rkey).is_some()
    }

    /// A record as atproto JSON.
    pub fn get(&self, collection: &str, rkey: &str) -> Option<Value> {
        self.repo.get(collection, rkey).map(Rec::json)
    }

    /// A record's CID.
    pub fn cid(&self, collection: &str, rkey: &str) -> Option<Cid> {
        self.repo.get(collection, rkey).map(|r| r.cid)
    }
}

/// An account on a [`FakeNet`].
#[derive(Clone)]
pub struct AccountHandle {
    pub(super) net: FakeNet,
    pub(super) did: Did,
}

impl std::fmt::Debug for AccountHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AccountHandle").field(&self.did).finish()
    }
}

impl AccountHandle {
    fn with<T>(&self, f: impl FnOnce(&mut Account) -> T) -> T {
        let mut st = self.net.state();
        f(st.accounts.get_mut(self.did.as_str()).expect("account"))
    }

    fn emit(&self, kind: RepoEventKind) {
        let now = self.net.now();
        self.net.state().emit(&self.did, now, kind);
    }

    fn emit_identity(&self) {
        let handle = self.handle();
        self.emit(RepoEventKind::Identity {
            handle: Some(handle),
        });
    }

    /// The DID.
    pub fn did(&self) -> &Did {
        &self.did
    }

    /// The handle.
    pub fn handle(&self) -> String {
        self.with(|a| a.handle.clone())
    }

    /// The hosting PDS.
    pub fn pds(&self) -> PdsHandle {
        PdsHandle {
            net: self.net.clone(),
            base: self.with(|a| a.pds.clone()),
        }
    }

    /// The hosting PDS's base URL.
    pub fn pds_base(&self) -> String {
        self.with(|a| a.pds.clone())
    }

    /// The repo signing key.
    pub fn signing_key(&self) -> SigningKey {
        self.with(|a| a.key.clone())
    }

    /// The repo public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::Secp256k1(*self.signing_key().verifying_key())
    }

    /// The DID document the PLC directory serves.
    pub fn doc(&self) -> Value {
        self.with(|a| a.doc())
    }

    /// The account email (`<first label>@mail.test`).
    pub fn email(&self) -> String {
        self.with(|a| a.email.clone())
    }

    /// The main password (accepted by `createSession`; clients refuse to send it).
    pub fn password(&self) -> String {
        self.with(|a| a.password.clone())
    }

    /// The first app password (`xxxx-xxxx-xxxx-xxxx`).
    pub fn app_password(&self) -> String {
        self.with(|a| a.app_passwords.first().cloned().unwrap_or_default())
    }

    /// Adds an app password and returns it.
    pub fn add_app_password(&self) -> String {
        self.with(|a| {
            let n = u32::try_from(a.app_passwords.len()).unwrap_or(u32::MAX) + 1000;
            let p = app_password(&a.did, n);
            a.app_passwords.push(p.clone());
            p
        })
    }

    /// Revokes every app password.
    pub fn revoke_app_passwords(&self) {
        self.with(|a| a.app_passwords.clear());
    }

    /// An access token for this account at its PDS, as `createSession` would issue it.
    pub fn access_token(&self) -> String {
        let now = self.net.now();
        let base = self.pds_base();
        let mut st = self.net.state();
        st.issue_session(&base, self.did.as_str(), true, now).0
    }

    /// An access/refresh token pair.
    pub fn session_tokens(&self) -> (String, String) {
        let now = self.net.now();
        let base = self.pds_base();
        let mut st = self.net.state();
        st.issue_session(&base, self.did.as_str(), true, now)
    }

    /// `at://<did>/<collection>/<rkey>`.
    pub fn uri(&self, collection: &str, rkey: &str) -> String {
        format!("at://{}/{collection}/{rkey}", self.did)
    }

    // -----------------------------------------------------------------------------------------
    // Records

    /// Writes with a [`RepoWriter`]; everything lands in one commit (signed lazily, see the
    /// module docs).
    pub fn batch<T>(&self, f: impl FnOnce(&mut RepoWriter<'_>) -> T) -> T {
        self.with(|a| f(&mut RepoWriter { repo: &mut a.repo }))
    }

    /// Creates or replaces a record from atproto JSON (no checks; panics on invalid JSON).
    pub fn put(&self, collection: &str, rkey: &str, value: &Value) -> Cid {
        self.batch(|w| w.put(collection, rkey, value))
    }

    /// Creates or replaces a record from DAG-CBOR bytes (stored as given, even if invalid).
    pub fn put_cbor(&self, collection: &str, rkey: &str, cbor: Vec<u8>) -> Cid {
        self.batch(|w| w.put_cbor(collection, rkey, cbor))
    }

    /// Deletes a record; true if it existed.
    pub fn delete(&self, collection: &str, rkey: &str) -> bool {
        self.batch(|w| w.delete(collection, rkey))
    }

    /// A record as atproto JSON.
    pub fn get(&self, collection: &str, rkey: &str) -> Option<Value> {
        self.with(|a| a.repo.get(collection, rkey).map(Rec::json))
    }

    /// A record's CID.
    pub fn cid(&self, collection: &str, rkey: &str) -> Option<Cid> {
        self.with(|a| a.repo.get(collection, rkey).map(|r| r.cid))
    }

    /// `(rkey, cid)` of a collection, ascending.
    pub fn list(&self, collection: &str) -> Vec<(String, Cid)> {
        self.with(|a| {
            a.repo
                .list(collection, None, true, usize::MAX)
                .into_iter()
                .map(|(k, r)| (k.to_owned(), r.cid))
                .collect()
        })
    }

    /// The collections with records.
    pub fn collections(&self) -> Vec<String> {
        self.with(|a| a.repo.collections())
    }

    /// Commits pending helper writes now (if any); returns the head `(cid, rev)`.
    pub fn head(&self) -> (Cid, Tid) {
        let now = self.net.now();
        let mut st = self.net.state();
        st.ensure_commit(self.did.as_str(), now);
        let c = &st.accounts[self.did.as_str()].repo.commit;
        (c.cid, c.rev)
    }

    /// Signs a new commit now, even without changes; returns the head.
    pub fn commit_now(&self) -> (Cid, Tid) {
        self.with(|a| a.repo.force_commit());
        self.head()
    }

    /// Re-signs the current tree at a new rev with `key` instead of the account's key (the DID
    /// document is unchanged): proofs then fail with a bad signature until the next commit.
    pub fn resign_with(&self, key: &SigningKey) -> (Cid, Tid) {
        self.head();
        let now = self.net.now();
        self.with(|a| {
            let rev = a.next_tid(now);
            let did = a.did.clone();
            a.repo.resign(&did, key, rev);
            (a.repo.commit.cid, a.repo.commit.rev)
        })
    }

    /// Like [`AccountHandle::commit_now`] but at an explicit rev (for rollback and
    /// far-future-rev tests); later revs continue above it.
    pub fn commit_at(&self, rev: Tid) -> (Cid, Tid) {
        self.head();
        let (cid, ops) = self.with(|a| {
            a.observe_tid(&rev);
            a.repo.force_commit();
            let did = a.did.clone();
            let key = a.key.clone();
            let ops = a.repo.recommit(&did, &key, rev);
            a.settle_blobs();
            (a.repo.commit.cid, ops)
        });
        self.emit(RepoEventKind::Commit { rev, cid, ops });
        (cid, rev)
    }

    // -----------------------------------------------------------------------------------------
    // Blobs

    /// Stores a blob as `uploadBlob` would (MIME sniffed; temporary until a record references
    /// it); returns the blob object to embed.
    pub fn upload_blob(&self, bytes: Vec<u8>, declared_mime: &str) -> Value {
        let mime = sniff_mime(&bytes, declared_mime);
        let cid = lessdugin_atp::cid::for_raw(&bytes);
        let size = bytes.len();
        let mime = self.with(|a| {
            a.blobs
                .entry(cid)
                .or_insert_with(|| StoredBlob {
                    bytes: Arc::new(bytes),
                    mime,
                    permanent: false,
                })
                .mime
                .clone()
        });
        json!({ "$type": "blob", "ref": { "$link": cid.to_string() }, "mimeType": mime, "size": size })
    }

    /// True if the PDS stores the blob (temporary or referenced).
    pub fn has_blob(&self, cid: &Cid) -> bool {
        self.with(|a| a.blobs.contains_key(cid))
    }

    /// True if `getBlob` serves the blob (pending helper writes are committed first).
    pub fn serves_blob(&self, cid: &Cid) -> bool {
        self.head();
        self.with(|a| a.blobs.get(cid).is_some_and(|b| b.permanent))
    }

    // -----------------------------------------------------------------------------------------
    // Status and identity

    /// Sets the hosting status (and emits an account event).
    pub fn set_status(&self, status: AccountStatus) {
        self.with(|a| a.status = status);
        self.emit(RepoEventKind::Account {
            active: status == AccountStatus::Active,
            status: status.as_status().map(str::to_owned),
        });
    }

    /// [`AccountStatus::Deactivated`].
    pub fn deactivate(&self) {
        self.set_status(AccountStatus::Deactivated);
    }

    /// [`AccountStatus::Takendown`].
    pub fn takedown(&self) {
        self.set_status(AccountStatus::Takendown);
    }

    /// [`AccountStatus::Suspended`].
    pub fn suspend(&self) {
        self.set_status(AccountStatus::Suspended);
    }

    /// [`AccountStatus::Active`].
    pub fn activate(&self) {
        self.set_status(AccountStatus::Active);
    }

    /// [`AccountStatus::Deleted`]: the PDS forgets the repo (`RepoNotFound`); the DID document
    /// stays unless [`AccountHandle::tombstone_plc`] is called.
    pub fn delete_account(&self) {
        self.set_status(AccountStatus::Deleted);
    }

    /// The PLC directory answers `410` for this DID.
    pub fn tombstone_plc(&self) {
        self.with(|a| a.plc_tombstoned = true);
        self.emit_identity();
    }

    /// Changes the handle (DID document, `resolveHandle`, DoH answers).
    pub fn set_handle(&self, handle: &str) {
        let h = handle.to_ascii_lowercase();
        {
            let st = self.net.state();
            if let Some(other) = st.resolve_ident(&h) {
                assert_eq!(other, self.did.as_str(), "handle {h} is taken");
            }
        }
        self.with(|a| a.handle = h);
        self.emit_identity();
    }

    /// Rotates the repo key to one derived from `seed` (the DID document changes at once; the
    /// current commit stays signed by the old key until the next commit). Returns the new key.
    pub fn rotate_key(&self, seed: &str) -> SigningKey {
        let key = fake_key(seed);
        self.with(|a| a.key = key.clone());
        self.emit_identity();
        key
    }

    /// Serves this document instead of the generated one (`None` restores it).
    pub fn set_doc_override(&self, doc: Option<Value>) {
        self.with(|a| a.doc_override = doc);
        self.emit_identity();
    }

    /// Moves the account (repo, blobs) to another PDS: the DID document points there, the old
    /// PDS answers `RepoDeactivated`, and old sessions stop working.
    pub fn move_to(&self, pds: &PdsHandle) {
        self.with(|a| {
            let old = std::mem::replace(&mut a.pds, pds.base.clone());
            if old != a.pds {
                a.former_pds.retain(|b| *b != a.pds);
                a.former_pds.push(old);
            }
        });
        self.emit_identity();
    }

    /// Overrides the PDS's cursor behaviour for this repo (`None`: the PDS's).
    pub fn set_cursor_mode(&self, mode: Option<CursorMode>) {
        self.with(|a| a.cursor_mode = mode);
    }

    /// Injects a fault limited to this repo.
    pub fn inject(&self, rule: FaultRule) -> FaultId {
        self.net.inject(rule.on_repo(self.did.as_str()))
    }

    // -----------------------------------------------------------------------------------------
    // Equivocation

    /// Equivocation: callers chosen by `selector` see a fork of the current repo, changed by
    /// `f` and signed with the account key at the **same rev** as the real head (a different
    /// commit CID for the same rev). Reads (`getLatestCommit`, `getRecord`, `listRecords`,
    /// `describeRepo`, `sync.getRecord`, `getRepo`, `getRepoStatus`) follow the selector;
    /// writes go to the real repo. Returns the fork's `(cid, rev)`.
    pub fn equivocate(
        &self,
        selector: ForkSelector,
        f: impl FnOnce(&mut RepoWriter<'_>),
    ) -> (Cid, Tid) {
        let (_, rev) = self.head();
        self.equivocate_at(selector, rev, f)
    }

    /// [`AccountHandle::equivocate`] with the fork at `rev`.
    pub fn equivocate_at(
        &self,
        selector: ForkSelector,
        rev: Tid,
        f: impl FnOnce(&mut RepoWriter<'_>),
    ) -> (Cid, Tid) {
        self.head();
        self.with(|a| {
            let mut fork = a.repo.clone();
            f(&mut RepoWriter { repo: &mut fork });
            let did = a.did.clone();
            let key = a.key.clone();
            fork.recommit(&did, &key, rev);
            let out = (fork.commit.cid, fork.commit.rev);
            a.fork = Some(Fork {
                repo: fork,
                selector,
                requests: 0,
            });
            out
        })
    }

    /// Ends equivocation.
    pub fn end_equivocation(&self) {
        self.with(|a| a.fork = None);
    }
}
