// SPDX-License-Identifier: MIT OR Apache-2.0
//! Request/response types for the small XRPC subset lessdugin uses. No HTTP client: callers build
//! the path with [`query_path`] (queries) or [`procedure_path`] (procedures, JSON body) and decode
//! responses with serde.
//!
//! Request parameters use validated syntax types. Response bodies keep server-provided
//! identifiers (`uri`, `cid`, `rev`, …) as plain strings so that one malformed entry does not fail
//! a whole page; callers validate each item (`AtUri::parse`, `cid::parse`, `Tid::parse`). Record
//! values stay as `serde_json::Value` (see [`crate::datamodel`]). Unknown response fields are
//! ignored. `Debug` output of credential-bearing types redacts secrets.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::syntax::{AtIdentifier, Did, Handle, Nsid, RecordKey};

/// An XRPC error body: `{"error": "...", "message": "..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpcError {
    /// Error name (e.g. `RecordNotFound`, `InvalidSwap`, `RateLimitExceeded`).
    pub error: String,
    /// Human-readable message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl core::fmt::Display for XrpcError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.message {
            Some(m) => write!(f, "{}: {}", self.error, m),
            None => f.write_str(&self.error),
        }
    }
}

/// XRPC method kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// HTTP GET with query parameters.
    Query,
    /// HTTP POST with a body.
    Procedure,
}

/// An XRPC endpoint: its NSID, method and output type.
pub trait Endpoint {
    /// The method NSID.
    const NSID: &'static str;
    /// Query or procedure.
    const METHOD: Method;
    /// The JSON output type (`()` for none, or a binary body).
    type Output;
}

/// Query parameters of an endpoint.
pub trait QueryParams {
    /// `(name, value)` pairs in a fixed order; absent optional parameters are omitted.
    fn query_pairs(&self) -> Vec<(&'static str, String)>;
}

/// Percent-encodes a query component: everything except RFC 3986 unreserved characters
/// (`A-Z a-z 0-9 - . _ ~`) becomes `%XX` (uppercase hex) of its UTF-8 bytes.
pub fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0x0f)]));
        }
    }
    out
}

/// Builds `name=value&…` with percent-encoding.
pub fn query_string<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(k.as_ref()));
        out.push('=');
        out.push_str(&percent_encode(v.as_ref()));
    }
    out
}

/// `/xrpc/<nsid>?<query>` for a query endpoint (no `?` when there are no parameters).
pub fn query_path<P: Endpoint + QueryParams>(params: &P) -> String {
    let qs = query_string(&params.query_pairs());
    if qs.is_empty() {
        format!("/xrpc/{}", P::NSID)
    } else {
        format!("/xrpc/{}?{qs}", P::NSID)
    }
}

/// `/xrpc/<nsid>` for a procedure endpoint.
pub fn procedure_path<P: Endpoint>() -> String {
    format!("/xrpc/{}", P::NSID)
}

fn push_opt<T: ToString>(out: &mut Vec<(&'static str, String)>, k: &'static str, v: &Option<T>) {
    if let Some(v) = v {
        out.push((k, v.to_string()));
    }
}

/// `{cid, rev}` of a commit, as returned by write procedures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitMeta {
    /// Commit CID.
    pub cid: String,
    /// Commit revision (TID).
    pub rev: String,
}

/// One record in a response: `{uri, cid, value}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordEntry {
    /// The record's AT URI.
    pub uri: String,
    /// The record CID.
    pub cid: String,
    /// The record value (atproto JSON).
    pub value: Value,
}

macro_rules! endpoint {
    ($ty:ty, $nsid:literal, $method:ident, $out:ty) => {
        impl Endpoint for $ty {
            const NSID: &'static str = $nsid;
            const METHOD: Method = Method::$method;
            type Output = $out;
        }
    };
}

// ------------------------------------------------------------------------------------------
// com.atproto.repo

/// `com.atproto.repo.getRecord` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetRecord {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
    /// Collection.
    pub collection: Nsid,
    /// Record key.
    pub rkey: RecordKey,
    /// Optional CID of a specific version.
    pub cid: Option<String>,
}
endpoint!(
    GetRecord,
    "com.atproto.repo.getRecord",
    Query,
    GetRecordOutput
);
impl QueryParams for GetRecord {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            ("repo", self.repo.to_string()),
            ("collection", self.collection.to_string()),
            ("rkey", self.rkey.to_string()),
        ];
        push_opt(&mut v, "cid", &self.cid);
        v
    }
}

/// `com.atproto.repo.getRecord` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetRecordOutput {
    /// Record URI.
    pub uri: String,
    /// Record CID (optional in the lexicon).
    #[serde(default)]
    pub cid: Option<String>,
    /// Record value.
    pub value: Value,
}

/// `com.atproto.repo.listRecords` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRecords {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
    /// Collection.
    pub collection: Nsid,
    /// Page size (1–100).
    pub limit: Option<u32>,
    /// Opaque cursor.
    pub cursor: Option<String>,
    /// Reverse order.
    pub reverse: Option<bool>,
}
endpoint!(
    ListRecords,
    "com.atproto.repo.listRecords",
    Query,
    ListRecordsOutput
);
impl QueryParams for ListRecords {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            ("repo", self.repo.to_string()),
            ("collection", self.collection.to_string()),
        ];
        push_opt(&mut v, "limit", &self.limit);
        push_opt(&mut v, "cursor", &self.cursor);
        push_opt(&mut v, "reverse", &self.reverse);
        v
    }
}

/// `com.atproto.repo.listRecords` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListRecordsOutput {
    /// Next-page cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Records.
    pub records: Vec<RecordEntry>,
}

/// `com.atproto.repo.describeRepo` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeRepo {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
}
endpoint!(
    DescribeRepo,
    "com.atproto.repo.describeRepo",
    Query,
    DescribeRepoOutput
);
impl QueryParams for DescribeRepo {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![("repo", self.repo.to_string())]
    }
}

/// `com.atproto.repo.describeRepo` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeRepoOutput {
    /// Handle.
    pub handle: String,
    /// DID.
    pub did: String,
    /// DID document.
    #[serde(default)]
    pub did_doc: Value,
    /// Collections present in the repo.
    #[serde(default)]
    pub collections: Vec<String>,
    /// Whether the handle resolves bidirectionally (per the PDS).
    #[serde(default)]
    pub handle_is_correct: bool,
}

/// `com.atproto.repo.createRecord` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecord {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
    /// Collection.
    pub collection: Nsid,
    /// Record key (server-generated TID if absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rkey: Option<RecordKey>,
    /// Lexicon validation flag (lessdugin leaves it unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
    /// The record.
    pub record: Value,
    /// Expected current commit CID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_commit: Option<String>,
}
endpoint!(
    CreateRecord,
    "com.atproto.repo.createRecord",
    Procedure,
    WriteRecordOutput
);

/// `com.atproto.repo.putRecord` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutRecord {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
    /// Collection.
    pub collection: Nsid,
    /// Record key.
    pub rkey: RecordKey,
    /// Lexicon validation flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
    /// The record.
    pub record: Value,
    /// Expected current record CID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_record: Option<String>,
    /// Expected current commit CID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_commit: Option<String>,
}
endpoint!(
    PutRecord,
    "com.atproto.repo.putRecord",
    Procedure,
    WriteRecordOutput
);

/// Output of `createRecord` / `putRecord`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteRecordOutput {
    /// Record URI.
    pub uri: String,
    /// Record CID.
    pub cid: String,
    /// The new commit.
    #[serde(default)]
    pub commit: Option<CommitMeta>,
    /// `valid` / `unknown`.
    #[serde(default)]
    pub validation_status: Option<String>,
}

/// `com.atproto.repo.deleteRecord` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRecord {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
    /// Collection.
    pub collection: Nsid,
    /// Record key.
    pub rkey: RecordKey,
    /// Expected current record CID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_record: Option<String>,
    /// Expected current commit CID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_commit: Option<String>,
}
endpoint!(
    DeleteRecord,
    "com.atproto.repo.deleteRecord",
    Procedure,
    DeleteRecordOutput
);

/// `com.atproto.repo.deleteRecord` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DeleteRecordOutput {
    /// The new commit (absent if nothing was deleted).
    #[serde(default)]
    pub commit: Option<CommitMeta>,
}

/// One `applyWrites` operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum WriteOp {
    /// Create a record.
    #[serde(rename = "com.atproto.repo.applyWrites#create")]
    Create {
        /// Collection.
        collection: Nsid,
        /// Record key (server-generated if absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rkey: Option<RecordKey>,
        /// The record.
        value: Value,
    },
    /// Update (or create) a record.
    #[serde(rename = "com.atproto.repo.applyWrites#update")]
    Update {
        /// Collection.
        collection: Nsid,
        /// Record key.
        rkey: RecordKey,
        /// The record.
        value: Value,
    },
    /// Delete a record.
    #[serde(rename = "com.atproto.repo.applyWrites#delete")]
    Delete {
        /// Collection.
        collection: Nsid,
        /// Record key.
        rkey: RecordKey,
    },
}

/// `com.atproto.repo.applyWrites` input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyWrites {
    /// Repo (DID or handle).
    pub repo: AtIdentifier,
    /// Lexicon validation flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validate: Option<bool>,
    /// Operations (≤ 200 per call on the reference PDS).
    pub writes: Vec<WriteOp>,
    /// Expected current commit CID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_commit: Option<String>,
}
endpoint!(
    ApplyWrites,
    "com.atproto.repo.applyWrites",
    Procedure,
    ApplyWritesOutput
);

/// One `applyWrites` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum WriteResult {
    /// A created record.
    #[serde(
        rename = "com.atproto.repo.applyWrites#createResult",
        rename_all = "camelCase"
    )]
    Create {
        /// Record URI.
        uri: String,
        /// Record CID.
        cid: String,
        /// Validation status.
        #[serde(default)]
        validation_status: Option<String>,
    },
    /// An updated record.
    #[serde(
        rename = "com.atproto.repo.applyWrites#updateResult",
        rename_all = "camelCase"
    )]
    Update {
        /// Record URI.
        uri: String,
        /// Record CID.
        cid: String,
        /// Validation status.
        #[serde(default)]
        validation_status: Option<String>,
    },
    /// A deleted record.
    #[serde(rename = "com.atproto.repo.applyWrites#deleteResult")]
    Delete {},
    /// A result type this crate does not know.
    #[serde(other)]
    Unknown,
}

/// `com.atproto.repo.applyWrites` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ApplyWritesOutput {
    /// The new commit.
    #[serde(default)]
    pub commit: Option<CommitMeta>,
    /// Per-operation results, in order.
    #[serde(default)]
    pub results: Vec<WriteResult>,
}

/// `com.atproto.repo.uploadBlob` (the input is the raw blob body with its `Content-Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UploadBlob;
endpoint!(
    UploadBlob,
    "com.atproto.repo.uploadBlob",
    Procedure,
    UploadBlobOutput
);

/// `com.atproto.repo.uploadBlob` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadBlobOutput {
    /// The blob object, to be embedded verbatim in a record
    /// (`{$type: "blob", ref: {$link}, mimeType, size}`).
    pub blob: Value,
}

// ------------------------------------------------------------------------------------------
// com.atproto.sync

/// `com.atproto.sync.getLatestCommit` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetLatestCommit {
    /// Repo DID.
    pub did: Did,
}
endpoint!(
    GetLatestCommit,
    "com.atproto.sync.getLatestCommit",
    Query,
    CommitMeta
);
impl QueryParams for GetLatestCommit {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![("did", self.did.to_string())]
    }
}

/// `com.atproto.sync.getRecord` parameters (the output is a CAR, `application/vnd.ipld.car`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncGetRecord {
    /// Repo DID.
    pub did: Did,
    /// Collection.
    pub collection: Nsid,
    /// Record key.
    pub rkey: RecordKey,
}
endpoint!(SyncGetRecord, "com.atproto.sync.getRecord", Query, Vec<u8>);
impl QueryParams for SyncGetRecord {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("did", self.did.to_string()),
            ("collection", self.collection.to_string()),
            ("rkey", self.rkey.to_string()),
        ]
    }
}

/// `com.atproto.sync.getBlob` parameters (the output is the blob bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetBlob {
    /// Repo DID.
    pub did: Did,
    /// Blob CID.
    pub cid: String,
}
endpoint!(GetBlob, "com.atproto.sync.getBlob", Query, Vec<u8>);
impl QueryParams for GetBlob {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![("did", self.did.to_string()), ("cid", self.cid.clone())]
    }
}

/// `com.atproto.sync.getRepoStatus` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetRepoStatus {
    /// Repo DID.
    pub did: Did,
}
endpoint!(
    GetRepoStatus,
    "com.atproto.sync.getRepoStatus",
    Query,
    GetRepoStatusOutput
);
impl QueryParams for GetRepoStatus {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![("did", self.did.to_string())]
    }
}

/// `com.atproto.sync.getRepoStatus` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetRepoStatusOutput {
    /// Repo DID.
    pub did: String,
    /// Whether the account is active.
    pub active: bool,
    /// Why it is inactive (`takendown`, `suspended`, `deleted`, `deactivated`, …).
    #[serde(default)]
    pub status: Option<String>,
    /// Current revision, if active.
    #[serde(default)]
    pub rev: Option<String>,
}

/// `com.atproto.sync.listReposByCollection` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReposByCollection {
    /// Collection.
    pub collection: Nsid,
    /// Page size.
    pub limit: Option<u32>,
    /// Opaque cursor.
    pub cursor: Option<String>,
}
endpoint!(
    ListReposByCollection,
    "com.atproto.sync.listReposByCollection",
    Query,
    ListReposByCollectionOutput
);
impl QueryParams for ListReposByCollection {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![("collection", self.collection.to_string())];
        push_opt(&mut v, "limit", &self.limit);
        push_opt(&mut v, "cursor", &self.cursor);
        v
    }
}

/// One repo in a `listReposByCollection` page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    /// Repo DID.
    pub did: String,
}

/// `com.atproto.sync.listReposByCollection` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListReposByCollectionOutput {
    /// Next-page cursor.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Repos.
    pub repos: Vec<RepoRef>,
}

// ------------------------------------------------------------------------------------------
// com.atproto.server

/// `com.atproto.server.createSession` input. `Debug` redacts the password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSession {
    /// Handle, DID or email.
    pub identifier: String,
    /// App password.
    pub password: String,
    /// Email 2FA token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_factor_token: Option<String>,
    /// Allow a takendown account to log in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_takendown: Option<bool>,
}
endpoint!(
    CreateSession,
    "com.atproto.server.createSession",
    Procedure,
    Session
);

impl core::fmt::Debug for CreateSession {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CreateSession")
            .field("identifier", &self.identifier)
            .field("password", &"<redacted>")
            .field(
                "auth_factor_token",
                &self.auth_factor_token.as_ref().map(|_| "<redacted>"),
            )
            .field("allow_takendown", &self.allow_takendown)
            .finish()
    }
}

/// `com.atproto.server.refreshSession` (no input; the refresh JWT is the bearer token).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RefreshSession;
endpoint!(
    RefreshSession,
    "com.atproto.server.refreshSession",
    Procedure,
    Session
);

/// Output of `createSession` / `refreshSession`. `Debug` redacts the tokens.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Access JWT.
    pub access_jwt: String,
    /// Refresh JWT.
    pub refresh_jwt: String,
    /// Handle.
    pub handle: String,
    /// DID.
    pub did: String,
    /// DID document.
    #[serde(default)]
    pub did_doc: Option<Value>,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Email confirmed.
    #[serde(default)]
    pub email_confirmed: Option<bool>,
    /// Email 2FA enabled.
    #[serde(default)]
    pub email_auth_factor: Option<bool>,
    /// Account active.
    #[serde(default)]
    pub active: Option<bool>,
    /// Inactive status.
    #[serde(default)]
    pub status: Option<String>,
}

impl core::fmt::Debug for Session {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session")
            .field("access_jwt", &"<redacted>")
            .field("refresh_jwt", &"<redacted>")
            .field("handle", &self.handle)
            .field("did", &self.did)
            .field("active", &self.active)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// `com.atproto.server.getSession` (no parameters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GetSession;
endpoint!(
    GetSession,
    "com.atproto.server.getSession",
    Query,
    GetSessionOutput
);
impl QueryParams for GetSession {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}

/// `com.atproto.server.getSession` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionOutput {
    /// Handle.
    pub handle: String,
    /// DID.
    pub did: String,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Email confirmed.
    #[serde(default)]
    pub email_confirmed: Option<bool>,
    /// Email 2FA enabled.
    #[serde(default)]
    pub email_auth_factor: Option<bool>,
    /// DID document.
    #[serde(default)]
    pub did_doc: Option<Value>,
    /// Account active.
    #[serde(default)]
    pub active: Option<bool>,
    /// Inactive status.
    #[serde(default)]
    pub status: Option<String>,
}

/// `com.atproto.server.describeServer` (no parameters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DescribeServer;
endpoint!(
    DescribeServer,
    "com.atproto.server.describeServer",
    Query,
    DescribeServerOutput
);
impl QueryParams for DescribeServer {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}

/// `com.atproto.server.describeServer` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DescribeServerOutput {
    /// Server DID.
    pub did: String,
    /// Invite code required for signup.
    #[serde(default)]
    pub invite_code_required: Option<bool>,
    /// Phone verification required for signup.
    #[serde(default)]
    pub phone_verification_required: Option<bool>,
    /// Handle domains offered.
    #[serde(default)]
    pub available_user_domains: Vec<String>,
    /// Policy links.
    #[serde(default)]
    pub links: Option<ServerLinks>,
    /// Contact.
    #[serde(default)]
    pub contact: Option<ServerContact>,
}

/// `describeServer#links`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLinks {
    /// Privacy policy URL.
    #[serde(default)]
    pub privacy_policy: Option<String>,
    /// Terms of service URL.
    #[serde(default)]
    pub terms_of_service: Option<String>,
}

/// `describeServer#contact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerContact {
    /// Contact email.
    #[serde(default)]
    pub email: Option<String>,
}

// ------------------------------------------------------------------------------------------
// com.atproto.identity

/// `com.atproto.identity.resolveHandle` parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveHandle {
    /// Handle to resolve.
    pub handle: Handle,
}
endpoint!(
    ResolveHandle,
    "com.atproto.identity.resolveHandle",
    Query,
    ResolveHandleOutput
);
impl QueryParams for ResolveHandle {
    fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![("handle", self.handle.to_string())]
    }
}

/// `com.atproto.identity.resolveHandle` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveHandleOutput {
    /// The DID (unvalidated string).
    pub did: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn did() -> Did {
        Did::parse("did:plc:ewvi7nxzyoun6zhxrhs64oiz").unwrap()
    }

    #[test]
    fn paths() {
        let q = ListRecords {
            repo: AtIdentifier::Did(did()),
            collection: Nsid::parse("online.7778777.lessdugin.post").unwrap(),
            limit: Some(100),
            cursor: Some("uetjixxr3a33.".into()),
            reverse: None,
        };
        assert_eq!(
            query_path(&q),
            "/xrpc/com.atproto.repo.listRecords?repo=did%3Aplc%3Aewvi7nxzyoun6zhxrhs64oiz&collection=online.7778777.lessdugin.post&limit=100&cursor=uetjixxr3a33."
        );
        assert_eq!(
            query_path(&GetSession),
            "/xrpc/com.atproto.server.getSession"
        );
        assert_eq!(
            procedure_path::<ApplyWrites>(),
            "/xrpc/com.atproto.repo.applyWrites"
        );
        assert_eq!(percent_encode("a b/é&=+~"), "a%20b%2F%C3%A9%26%3D%2B~");
        assert_eq!(query_string(&[("k", "v"), ("x y", "")]), "k=v&x%20y=");
        let r = SyncGetRecord {
            did: did(),
            collection: Nsid::parse("app.bsky.actor.profile").unwrap(),
            rkey: RecordKey::parse("self").unwrap(),
        };
        assert!(query_path(&r).ends_with("collection=app.bsky.actor.profile&rkey=self"));
        assert_eq!(<SyncGetRecord as Endpoint>::METHOD, Method::Query);
    }

    #[test]
    fn apply_writes_json() {
        let a = ApplyWrites {
            repo: AtIdentifier::Did(did()),
            validate: None,
            writes: vec![
                WriteOp::Create {
                    collection: Nsid::parse("a.b.c").unwrap(),
                    rkey: Some(RecordKey::parse("k").unwrap()),
                    value: json!({"$type": "a.b.c"}),
                },
                WriteOp::Delete {
                    collection: Nsid::parse("a.b.c").unwrap(),
                    rkey: RecordKey::parse("z").unwrap(),
                },
            ],
            swap_commit: Some("bafy".into()),
        };
        let j = serde_json::to_value(&a).unwrap();
        assert_eq!(
            j,
            json!({
                "repo": "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
                "writes": [
                    {"$type": "com.atproto.repo.applyWrites#create", "collection": "a.b.c", "rkey": "k", "value": {"$type": "a.b.c"}},
                    {"$type": "com.atproto.repo.applyWrites#delete", "collection": "a.b.c", "rkey": "z"}
                ],
                "swapCommit": "bafy"
            })
        );
        let out: ApplyWritesOutput = serde_json::from_value(json!({
            "commit": {"cid": "bafyc", "rev": "3l2s5xxv2ze2c"},
            "results": [
                {"$type": "com.atproto.repo.applyWrites#createResult", "uri": "at://x/a.b.c/k", "cid": "bafyr", "validationStatus": "unknown"},
                {"$type": "com.atproto.repo.applyWrites#deleteResult"},
                {"$type": "com.atproto.repo.applyWrites#somethingNew", "x": 1}
            ]
        }))
        .unwrap();
        assert_eq!(out.results.len(), 3);
        assert!(
            matches!(&out.results[0], WriteResult::Create { validation_status: Some(s), .. } if s == "unknown")
        );
        assert_eq!(out.results[1], WriteResult::Delete {});
        assert_eq!(out.results[2], WriteResult::Unknown);
    }

    #[test]
    fn responses_and_redaction() {
        let e: XrpcError =
            serde_json::from_str(r#"{"error":"RecordNotFound","message":"nope"}"#).unwrap();
        assert_eq!(e.to_string(), "RecordNotFound: nope");
        let e: XrpcError = serde_json::from_str(r#"{"error":"X"}"#).unwrap();
        assert_eq!(e.to_string(), "X");
        let s: Session = serde_json::from_value(json!({
            "accessJwt": "secret-a", "refreshJwt": "secret-r", "handle": "a.test", "did": "did:plc:x", "extra": 1
        }))
        .unwrap();
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("secret"));
        let c = CreateSession {
            identifier: "a.test".into(),
            password: "xxxx-xxxx-xxxx-xxxx".into(),
            auth_factor_token: None,
            allow_takendown: None,
        };
        assert!(!format!("{c:?}").contains("xxxx"));
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"identifier": "a.test", "password": "xxxx-xxxx-xxxx-xxxx"})
        );
        let st: GetRepoStatusOutput =
            serde_json::from_str(r#"{"did":"did:plc:x","active":false,"status":"takendown"}"#)
                .unwrap();
        assert_eq!(st.status.as_deref(), Some("takendown"));
        let lr: ListRecordsOutput = serde_json::from_str(
            r#"{"records":[{"uri":"at://a/b.c.d/e","cid":"bafy","value":{"x":1}}]}"#,
        )
        .unwrap();
        assert!(lr.cursor.is_none());
        let p = PutRecord {
            repo: AtIdentifier::Did(did()),
            collection: Nsid::parse("a.b.c").unwrap(),
            rkey: RecordKey::parse("self").unwrap(),
            validate: None,
            record: json!({}),
            swap_record: Some("bafyold".into()),
            swap_commit: None,
        };
        assert_eq!(serde_json::to_value(&p).unwrap()["swapRecord"], "bafyold");
        let ds: DescribeServerOutput = serde_json::from_str(r#"{"did":"did:web:pds.test","availableUserDomains":[".test"],"links":{"privacyPolicy":"https://x"}}"#).unwrap();
        assert_eq!(ds.available_user_domains, vec![".test".to_owned()]);
    }
}
