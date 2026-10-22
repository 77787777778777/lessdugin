// SPDX-License-Identifier: AGPL-3.0-or-later
//! The account's PDS as the outbox uses it (`data-model.md` §8, `sync.md` §7, ADR 0005).
//!
//! - **Fresh reads** go to the PDS bound at login, anonymously, with the transport's GET
//!   retries: `getLatestCommit` (the swap head is never cached), `getRecord` (admitted: strict
//!   JSON and the CID checked) and prefix listings of the own repo.
//! - **Writes** (`createRecord`, `putRecord`, `deleteRecord`, `applyWrites`, `uploadBlob`) go
//!   through [`Target::UserPds`] and are never retried by the transport; the protocols decide.
//!   Each write request is charged to the account's write budget before it is sent and refused
//!   (as [`CallError::RateLimited`]) when it does not fit. `429` and the `RateLimit-*` headers of
//!   write responses block further writes until the reset.
//! - **Errors** are classified for the protocols: `InvalidSwap` is the only conflict signal; a
//!   5xx or a dropped connection on a write is ambiguous (re-read before deciding, never resend
//!   blindly); everything else is a refusal.

use lessdugin_atp::syntax::{AtIdentifier, Did, Nsid, RecordKey};
use lessdugin_atp::xrpc::{CommitMeta, GetLatestCommit, GetRecord, XrpcError};
use serde_json::{Value, json};

use crate::engine::{EngineError, ErrorCode};
use crate::host::{Host, HostError, HttpRequest, HttpResponse, Target};
use crate::limits;
use crate::outbox::budget::{self, BudgetRow};
use crate::store::RecordStore;
use crate::store::admit::{self, Expect, NoValidator, RecordValidator};
use crate::sync::list::{ListError, ListTuning, Lister};
use crate::xrpc::{CallOpts, Service, Xrpc, XrpcCallError};

/// `EngineError.detail` key set on a read that failed in transport (offline, DNS, timeout).
pub const OFFLINE_DETAIL: &str = "offline";

/// `true` for a read error that failed in transport (see [`OFFLINE_DETAIL`]).
pub fn is_offline(e: &EngineError) -> bool {
    e.detail
        .as_ref()
        .and_then(|d| d.get(OFFLINE_DETAIL))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The account a pump writes for: its DID and the PDS bound at login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The account DID.
    pub did: Did,
    /// The PDS bound at login (`sync.md` §3.6).
    pub pds: url::Url,
}

impl Session {
    /// A session.
    pub fn new(did: Did, pds: url::Url) -> Self {
        Self { did, pds }
    }

    /// Parses `did` and `pds`.
    pub fn parse(did: &str, pds: &str) -> Result<Self, EngineError> {
        Ok(Self {
            did: Did::parse(did).map_err(|e| EngineError::invalid_ref(format!("{did:?}: {e}")))?,
            pds: url::Url::parse(pds)
                .map_err(|e| EngineError::invalid_input(format!("PDS {pds:?}: {e}")))?,
        })
    }

    /// The PDS origin (`scheme://host[:port]`), the key of its host row.
    pub fn origin(&self) -> String {
        self.pds.origin().ascii_serialization()
    }
}

/// A head: `{cid, rev}` of the latest commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    /// Commit CID.
    pub cid: String,
    /// Commit rev.
    pub rev: String,
}

impl From<CommitMeta> for Head {
    fn from(c: CommitMeta) -> Self {
        Self {
            cid: c.cid,
            rev: c.rev,
        }
    }
}

/// A current record, admitted (its value hashes to its CID).
#[derive(Clone, Debug, PartialEq)]
pub struct Current {
    /// The record key.
    pub rkey: String,
    /// The CID.
    pub cid: String,
    /// The value.
    pub value: Value,
    /// Its validity (listings with a validator only; otherwise valid).
    pub valid: bool,
}

/// `swapRecord` of a `putRecord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swap<'a> {
    /// No precondition.
    Unset,
    /// `null`: create only if absent (ADR 0005).
    Null,
    /// The record's current CID must be this.
    Cid(&'a str),
}

/// One op of an `applyWrites` batch.
#[derive(Clone, Debug, PartialEq)]
pub enum BatchOp {
    /// Create at a fixed rkey.
    Create {
        /// Collection.
        collection: String,
        /// Record key.
        rkey: String,
        /// Value.
        value: Value,
    },
    /// Update.
    Update {
        /// Collection.
        collection: String,
        /// Record key.
        rkey: String,
        /// Value.
        value: Value,
    },
    /// Delete.
    Delete {
        /// Collection.
        collection: String,
        /// Record key.
        rkey: String,
    },
}

impl BatchOp {
    /// The record key.
    pub fn rkey(&self) -> &str {
        match self {
            Self::Create { rkey, .. } | Self::Update { rkey, .. } | Self::Delete { rkey, .. } => {
                rkey
            }
        }
    }

    /// The bytes this op adds to an `applyWrites` body (its JSON and a separator).
    pub fn body_bytes(&self) -> usize {
        serde_json::to_vec(&self.json()).map_or(usize::MAX, |b| b.len().saturating_add(1))
    }

    fn json(&self) -> Value {
        match self {
            Self::Create {
                collection,
                rkey,
                value,
            } => json!({
                "$type": "com.atproto.repo.applyWrites#create",
                "collection": collection, "rkey": rkey, "value": value,
            }),
            Self::Update {
                collection,
                rkey,
                value,
            } => json!({
                "$type": "com.atproto.repo.applyWrites#update",
                "collection": collection, "rkey": rkey, "value": value,
            }),
            Self::Delete { collection, rkey } => json!({
                "$type": "com.atproto.repo.applyWrites#delete",
                "collection": collection, "rkey": rkey,
            }),
        }
    }

    /// Write-budget points.
    pub fn cost(&self) -> u32 {
        match self {
            Self::Create { .. } => limits::WRITE_COST_CREATE,
            Self::Update { .. } => limits::WRITE_COST_UPDATE,
            Self::Delete { .. } => limits::WRITE_COST_DELETE,
        }
    }

    /// `(collection, rkey)`.
    pub fn target(&self) -> (&str, &str) {
        match self {
            Self::Create {
                collection, rkey, ..
            }
            | Self::Update {
                collection, rkey, ..
            }
            | Self::Delete { collection, rkey } => (collection, rkey),
        }
    }

    /// The value written, for creates and updates.
    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Create { value, .. } | Self::Update { value, .. } => Some(value),
            Self::Delete { .. } => None,
        }
    }
}

/// A successful write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// The record CID (`createRecord`, `putRecord`).
    pub cid: Option<String>,
    /// The commit (absent for no-op writes, ADR 0005).
    pub commit: Option<Head>,
    /// Per-op CIDs of an `applyWrites` (`None` for deletes), in order.
    pub results: Vec<Option<String>>,
    /// When the request was sent (ms).
    pub sent_at: i64,
}

/// How a call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum CallError {
    /// `400 InvalidSwap`: a `swapRecord` or `swapCommit` precondition failed.
    InvalidSwap,
    /// The outcome of a write is unknown (5xx, dropped connection, timeout): re-read before
    /// deciding.
    Ambiguous(EngineError),
    /// Rate limited (`429`, the write budget or a `RateLimit-*` block): try again at `until`.
    RateLimited {
        /// When to try again (ms).
        until: i64,
        /// The error.
        error: EngineError,
    },
    /// A write refused for good (a 4xx other than `InvalidSwap`).
    Rejected {
        /// HTTP status (0 for host refusals).
        status: u16,
        /// The XRPC error name.
        name: Option<String>,
        /// The error.
        error: EngineError,
    },
    /// A read failed after the transport's retries.
    Read(EngineError),
}

impl CallError {
    /// The engine error.
    pub fn engine_error(&self) -> EngineError {
        match self {
            Self::InvalidSwap => {
                EngineError::conflict("the record or the repo changed (InvalidSwap)")
            }
            Self::Ambiguous(e) | Self::Read(e) => e.clone(),
            Self::RateLimited { error, .. } | Self::Rejected { error, .. } => error.clone(),
        }
    }
}

const MEDIA_ERRORS: [&str; 3] = ["InvalidMimeType", "InvalidSize", "BlobNotFound"];

/// The outbox's view of one account's PDS. Holds the account's write budget while a write runs.
pub struct PdsClient<'h, H: Host> {
    host: &'h H,
    session: &'h Session,
    /// The account's write budget (persisted by the caller).
    pub budget: BudgetRow,
    /// A write request was sent (it may have reached the PDS).
    pub sent_any: bool,
    /// Requests made.
    pub requests: u32,
}

impl<'h, H: Host> PdsClient<'h, H> {
    /// A client for `session` charging `budget`.
    pub fn new(host: &'h H, session: &'h Session, budget: BudgetRow) -> Self {
        Self {
            host,
            session,
            budget,
            sent_any: false,
            requests: 0,
        }
    }

    /// The session.
    pub fn session(&self) -> &Session {
        self.session
    }

    fn service(&self) -> Service {
        Service::Anon(self.session.pds.clone())
    }

    fn read_error(&self, e: XrpcCallError) -> CallError {
        if let XrpcCallError::RateLimited { retry_after_ms } = &e {
            let wait = retry_after_ms.map_or(budget::DEFAULT_RATE_LIMIT_WAIT_MS, |ms| {
                i64::try_from(ms).unwrap_or(i64::MAX)
            });
            return CallError::RateLimited {
                until: self.host.now_ms().saturating_add(wait),
                error: EngineError::from(e),
            };
        }
        // a transport failure (offline, DNS, timeout): the write waits for the network
        let offline = matches!(e, XrpcCallError::Network(_) | XrpcCallError::Timeout);
        let error = EngineError::from(e).context("reading the own repo");
        CallError::Read(if offline {
            error.with_detail(OFFLINE_DETAIL, true)
        } else {
            error
        })
    }

    // -----------------------------------------------------------------------------------------
    // Reads

    /// `com.atproto.sync.getLatestCommit`, fresh.
    pub async fn head(&mut self) -> Result<Head, CallError> {
        self.requests += 1;
        let opts = CallOpts::capped(limits::SMALL_RESPONSE_MAX_BYTES);
        let params = GetLatestCommit {
            did: self.session.did.clone(),
        };
        Xrpc::new(self.host)
            .query_endpoint(&self.service(), &params, &opts)
            .await
            .map(Head::from)
            .map_err(|e| self.read_error(e))
    }

    /// `com.atproto.repo.getRecord`, fresh and admitted; `None` if absent.
    pub async fn get(
        &mut self,
        collection: &str,
        rkey: &str,
    ) -> Result<Option<Current>, CallError> {
        self.requests += 1;
        let (Ok(coll), Ok(key)) = (Nsid::parse(collection), RecordKey::parse(rkey)) else {
            return Err(CallError::Rejected {
                status: 0,
                name: None,
                error: EngineError::invalid_input(format!("bad record path {collection}/{rkey}")),
            });
        };
        let params = GetRecord {
            repo: AtIdentifier::Did(self.session.did.clone()),
            collection: coll,
            rkey: key,
            cid: None,
        };
        let path = lessdugin_atp::xrpc::query_path(&params);
        let opts = CallOpts::capped(limits::RECORD_RESPONSE_MAX_BYTES);
        let resp = match Xrpc::new(self.host)
            .get(&self.service(), &path, &opts)
            .await
        {
            Ok(r) => r,
            Err(XrpcCallError::RecordNotFound) => return Ok(None),
            Err(e) => return Err(self.read_error(e)),
        };
        let expect = Expect {
            did: &self.session.did,
            collection,
            rkey: Some(rkey),
        };
        let a = admit::admit_get_record(&resp.body, &expect, &NoValidator).map_err(|e| {
            CallError::Read(
                EngineError::new(
                    ErrorCode::Network,
                    format!("own PDS returned a bad record: {e}"),
                )
                .with_retryable(true),
            )
        })?;
        Ok(Some(Current {
            rkey: rkey.to_owned(),
            cid: a.cid.to_string(),
            value: a.value,
            valid: true,
        }))
    }

    /// Every record of `collection` whose rkey starts with `prefix` (which ends in `-`), fresh
    /// and admitted, rkey descending. Uses the sync listing (`sync::list::Lister`, `sync.md`
    /// §5.3, ADR 0004): the cursor-range probe with the host's cached cursor capability, the
    /// `FULL` fallback and the oversized-page retry (limits 100, 10, 1). Without a fetch cap. A
    /// listing that is not complete (a cursor that does not advance, a safety cap) is a
    /// non-retryable error: a delete, a redaction or an adopt never works from a partial list.
    pub async fn list_prefix(
        &mut self,
        collection: &str,
        prefix: &str,
        validator: Option<&dyn RecordValidator>,
    ) -> Result<Vec<Current>, CallError> {
        let Some(stem) = prefix.strip_suffix('-') else {
            return Err(CallError::Read(EngineError::internal(format!(
                "listing prefix {prefix:?} does not end in '-'"
            ))));
        };
        let coll = Nsid::parse(collection).map_err(|e| {
            CallError::Read(EngineError::invalid_input(format!("{collection:?}: {e}")))
        })?;
        let service = self.service();
        let origin = self.session.origin();
        let caps = RecordStore::new(self.host.cache())
            .host_caps(&origin)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let now = self.host.now_ms();
        let tuning = ListTuning {
            fetch_cap: Some(u32::MAX),
            ..ListTuning::default()
        };
        let xrpc = Xrpc::new(self.host);
        let lister = Lister {
            xrpc: &xrpc,
            service: &service,
            origin: &origin,
            did: &self.session.did,
            validator: validator.unwrap_or(&NoValidator),
            tuning: &tuning,
            keep_values: true,
        };
        let res = lister
            .list_range(&coll, stem, caps.use_range(now), caps.short_page_ends)
            .await;
        let listing = match res {
            Ok(l) => l,
            Err(ListError::Xrpc(e)) => return Err(self.read_error(e)),
            Err(e) => {
                return Err(CallError::Read(
                    EngineError::new(
                        ErrorCode::Network,
                        format!("own PDS returned a bad listing: {e}"),
                    )
                    .with_retryable(true),
                ));
            }
        };
        self.requests += listing.requests;
        if !listing.complete || listing.capped {
            let why = listing
                .stop
                .clone()
                .unwrap_or_else(|| "the listing did not complete".to_owned());
            // the PDS's own behaviour (a cursor that does not advance, a safety cap): failing
            // is better than acting on a partial list or retrying for ever
            return Err(CallError::Read(
                EngineError::new(
                    ErrorCode::Network,
                    format!("incomplete listing of {collection} {prefix}*: {why}"),
                )
                .with_retryable(false),
            ));
        }
        Ok(listing
            .entries
            .into_iter()
            .filter_map(|e| {
                let a = e.record.ok()?;
                Some(Current {
                    rkey: e.rkey,
                    cid: a.cid.to_string(),
                    valid: a.validity.is_valid(),
                    value: a.value,
                })
            })
            .collect())
    }

    // -----------------------------------------------------------------------------------------
    // Writes

    async fn send_write(
        &mut self,
        nsid: &str,
        body: Vec<u8>,
        content_type: &str,
        cost: u32,
    ) -> Result<(HttpResponse, i64), CallError> {
        let now = self.host.now_ms();
        if !self.budget.fits(now, cost) {
            let until = self.budget.next_fit(now, cost);
            return Err(CallError::RateLimited {
                until,
                error: EngineError::new(ErrorCode::RateLimited, "write budget exhausted")
                    .with_detail("untilMs", until),
            });
        }
        self.budget.charge(now, cost);
        self.sent_any = true;
        self.requests += 1;
        let target = Target::UserPds {
            account: self.session.did.clone(),
            path_and_query: format!("/xrpc/{nsid}"),
        };
        let mut req = HttpRequest::post_json(target, body)
            .with_max_bytes(limits::DEFAULT_RESPONSE_MAX_BYTES)
            .with_timeout_ms(limits::REQUEST_TIMEOUT_MS);
        req.headers = vec![("content-type".into(), content_type.into())];
        let resp = match self.host.http(req).await {
            Ok(r) => r,
            Err(HostError::AuthRequired(m)) => {
                return Err(CallError::Rejected {
                    status: 401,
                    name: None,
                    error: EngineError::new(ErrorCode::AuthRequired, m).with_retryable(true),
                });
            }
            Err(
                e @ (HostError::Blocked(_)
                | HostError::Unsupported(_)
                | HostError::Cors
                | HostError::Storage(_)),
            ) => {
                return Err(CallError::Rejected {
                    status: 0,
                    name: None,
                    error: EngineError::from(e),
                });
            }
            // network errors, timeouts, an oversized answer: the request may have been applied
            Err(e) => return Err(CallError::Ambiguous(EngineError::from(e))),
        };
        let now = self.host.now_ms();
        if (200..300).contains(&resp.status) {
            if let Some(until) = budget::exhausted_until(&resp, now) {
                self.budget.block_until(until);
            }
            return Ok((resp, now));
        }
        if resp.status == 429 {
            let until = budget::rate_limited_until(&resp, now);
            self.budget.block_until(until);
            return Err(CallError::RateLimited {
                until,
                error: EngineError::new(ErrorCode::RateLimited, "rate limited by the PDS")
                    .with_detail("untilMs", until),
            });
        }
        let name = serde_json::from_slice::<XrpcError>(&resp.body)
            .ok()
            .map(|e| e.error);
        if name.as_deref() == Some("InvalidSwap") {
            return Err(CallError::InvalidSwap);
        }
        let err = EngineError::from(XrpcCallError::from_response(&resp, now));
        if resp.status >= 500 {
            return Err(CallError::Ambiguous(err.with_retryable(true)));
        }
        let err = match name.as_deref() {
            Some(n) if MEDIA_ERRORS.contains(&n) => err.with_detail("kind", "mediaRejected"),
            _ if resp.status == 413 => EngineError::new(ErrorCode::TooLarge, err.message),
            _ => err,
        };
        Err(CallError::Rejected {
            status: resp.status,
            name,
            error: err,
        })
    }

    async fn write_json(
        &mut self,
        nsid: &str,
        body: &Value,
        cost: u32,
    ) -> Result<(Value, i64), CallError> {
        let bytes = serde_json::to_vec(body).map_err(|e| CallError::Rejected {
            status: 0,
            name: None,
            error: EngineError::internal(format!("request JSON: {e}")),
        })?;
        if bytes.len() > limits::WRITE_BODY_MAX_BYTES {
            return Err(CallError::Rejected {
                status: 0,
                name: None,
                error: EngineError::new(
                    ErrorCode::TooLarge,
                    "the write is larger than 900,000 bytes",
                )
                .with_detail("bytes", bytes.len()),
            });
        }
        let (resp, at) = self
            .send_write(nsid, bytes, "application/json", cost)
            .await?;
        let v = crate::xrpc::decode_json::<Value>(&resp.body).unwrap_or(Value::Null);
        Ok((v, at))
    }

    /// `putRecord`.
    pub async fn put_record(
        &mut self,
        collection: &str,
        rkey: &str,
        value: &Value,
        swap: Swap<'_>,
    ) -> Result<Written, CallError> {
        let mut body = json!({
            "repo": self.session.did.as_str(),
            "collection": collection,
            "rkey": rkey,
            "record": value,
        });
        match swap {
            Swap::Unset => {}
            Swap::Null => body["swapRecord"] = Value::Null,
            Swap::Cid(c) => body["swapRecord"] = json!(c),
        }
        let sent_at = self.host.now_ms();
        let (out, _) = self
            .write_json(
                "com.atproto.repo.putRecord",
                &body,
                limits::WRITE_COST_UPDATE,
            )
            .await?;
        Ok(written(&out, sent_at))
    }

    /// `createRecord` at a fixed rkey.
    pub async fn create_record(
        &mut self,
        collection: &str,
        rkey: &str,
        value: &Value,
    ) -> Result<Written, CallError> {
        let body = json!({
            "repo": self.session.did.as_str(),
            "collection": collection,
            "rkey": rkey,
            "record": value,
        });
        let sent_at = self.host.now_ms();
        let (out, _) = self
            .write_json(
                "com.atproto.repo.createRecord",
                &body,
                limits::WRITE_COST_CREATE,
            )
            .await?;
        Ok(written(&out, sent_at))
    }

    /// `deleteRecord`, with an optional `swapRecord`.
    pub async fn delete_record(
        &mut self,
        collection: &str,
        rkey: &str,
        swap: Option<&str>,
    ) -> Result<Written, CallError> {
        let mut body = json!({
            "repo": self.session.did.as_str(),
            "collection": collection,
            "rkey": rkey,
        });
        if let Some(c) = swap {
            body["swapRecord"] = json!(c);
        }
        let sent_at = self.host.now_ms();
        let (out, _) = self
            .write_json(
                "com.atproto.repo.deleteRecord",
                &body,
                limits::WRITE_COST_DELETE,
            )
            .await?;
        Ok(written(&out, sent_at))
    }

    /// The exact request body of an `applyWrites` (for the size check, §8.9).
    pub fn apply_writes_body(&self, ops: &[BatchOp], swap_commit: Option<&str>) -> Value {
        let mut body = json!({
            "repo": self.session.did.as_str(),
            "writes": ops.iter().map(BatchOp::json).collect::<Vec<_>>(),
        });
        if let Some(c) = swap_commit {
            body["swapCommit"] = json!(c);
        }
        body
    }

    /// `applyWrites` (≤ 200 ops), with an optional `swapCommit`.
    pub async fn apply_writes(
        &mut self,
        ops: &[BatchOp],
        swap_commit: Option<&str>,
    ) -> Result<Written, CallError> {
        let body = self.apply_writes_body(ops, swap_commit);
        let cost = ops.iter().map(BatchOp::cost).sum();
        let sent_at = self.host.now_ms();
        let (out, _) = self
            .write_json("com.atproto.repo.applyWrites", &body, cost)
            .await?;
        let mut w = written(&out, sent_at);
        w.results = out
            .get("results")
            .and_then(Value::as_array)
            .map(|rs| {
                rs.iter()
                    .map(|r| r.get("cid").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(w)
    }

    /// `uploadBlob`: the returned blob object, verbatim.
    pub async fn upload_blob(&mut self, bytes: Vec<u8>, mime: &str) -> Result<Value, CallError> {
        let (resp, _) = self
            .send_write("com.atproto.repo.uploadBlob", bytes, mime, 0)
            .await?;
        let v: Value = crate::xrpc::decode_json(&resp.body).map_err(|e| {
            CallError::Ambiguous(EngineError::new(
                ErrorCode::Network,
                format!("uploadBlob answer: {e}"),
            ))
        })?;
        match v.get("blob") {
            Some(b @ Value::Object(_)) => Ok(b.clone()),
            _ => Err(CallError::Ambiguous(EngineError::new(
                ErrorCode::Network,
                "uploadBlob answered without a blob",
            ))),
        }
    }
}

fn written(out: &Value, sent_at: i64) -> Written {
    let commit = out.get("commit").and_then(|c| {
        Some(Head {
            cid: c.get("cid")?.as_str()?.to_owned(),
            rev: c.get("rev")?.as_str()?.to_owned(),
        })
    });
    Written {
        cid: out.get("cid").and_then(Value::as_str).map(str::to_owned),
        commit,
        results: Vec::new(),
        sent_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        let w = written(
            &json!({"uri": "at://x", "cid": "bafy", "commit": {"cid": "c", "rev": "r"}}),
            5,
        );
        assert_eq!(w.cid.as_deref(), Some("bafy"));
        assert_eq!(w.commit.unwrap().rev, "r");
        assert_eq!(written(&json!({}), 0).commit, None);
    }
}
