// SPDX-License-Identifier: AGPL-3.0-or-later
//! Strict-enough, allocation-bounded-by-the-host Jetstream v1/v2 wire parsing.

use lessdugin_atp::cid;
use lessdugin_atp::syntax::{Datetime, Did, Nsid, RecordKey};
use lessdugin_atp::tid::Tid;
use serde::Deserialize;

use super::events::Operation;

/// The two Jetstream wire formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    /// Legacy `/subscribe` frames.
    V1,
    /// `network.bsky.jetstream.subscribeEvents` envelopes.
    V2,
}

/// A parsed event. `seq` is the outer v2 sequence, not the identity/account inner sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Repo DID (`None` for `#info`).
    pub did: Option<String>,
    /// Event body.
    pub kind: EventKind,
    /// Per-instance v2 sequence.
    pub seq: Option<u64>,
    /// Event/witness time in unix microseconds.
    pub time_us: i64,
}

/// Parsed event bodies. There is intentionally no record payload here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A commit hint.
    Commit {
        /// Collection NSID.
        collection: String,
        /// Record key.
        rkey: String,
        /// Repo revision.
        rev: String,
        /// Create/update/delete.
        operation: Operation,
        /// CID for create/update; deletes normally have none.
        cid: Option<String>,
    },
    /// Identity change; the inner sequence feeds deduplication.
    Identity { inner_seq: u64 },
    /// Account status change; the inner sequence feeds deduplication.
    Account {
        inner_seq: u64,
        active: bool,
        status: Option<String>,
    },
    /// Broken repo stream: re-list the repo.
    Sync,
    /// Cursor recovery instruction.
    Info(InfoKind),
}

/// Jetstream v2 `#info` variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoKind {
    /// Replay is no longer available.
    OutdatedCursor,
    /// The requested cursor is ahead of the server.
    FutureCursor,
}

#[derive(Debug, thiserror::Error)]
#[error("malformed Jetstream event: {0}")]
/// A malformed event. Callers log it and keep the connection alive.
pub struct ParseError(String);

fn bad<T>(why: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(why.into()))
}

fn required<'a>(v: &'a Option<String>, name: &str) -> Result<&'a str, ParseError> {
    v.as_deref()
        .ok_or_else(|| ParseError(format!("missing {name}")))
}

fn did(s: &str) -> Result<String, ParseError> {
    Did::parse(s)
        .map(|d| d.to_string())
        .map_err(|e| ParseError(format!("bad did: {e}")))
}

fn time(s: &str) -> Result<i64, ParseError> {
    Datetime::parse(s)
        .map(|t| t.timestamp_micros())
        .map_err(|e| ParseError(format!("bad timestamp: {e}")))
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "$type")]
    ty: String,
    payload: Payload,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "$type")]
    ty: String,
    did: Option<String>,
    seq: Option<u64>,
    #[serde(rename = "witnessedAt")]
    witnessed_at: Option<String>,
    operation: Option<String>,
    collection: Option<String>,
    rkey: Option<String>,
    rev: Option<String>,
    cid: Option<String>,
    identity: Option<Identity>,
    account: Option<Account>,
    name: Option<String>,
    message: Option<String>,
    info: Option<InfoBody>,
}

#[derive(Deserialize)]
struct InfoBody {
    name: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct Identity {
    did: String,
    seq: u64,
    time: String,
}

#[derive(Deserialize)]
struct Account {
    did: String,
    seq: u64,
    time: String,
    active: bool,
    status: Option<String>,
}

#[derive(Deserialize)]
struct V1 {
    did: Option<String>,
    time_us: Option<i64>,
    kind: String,
    commit: Option<V1Commit>,
    identity: Option<Identity>,
    account: Option<Account>,
}

#[derive(Deserialize)]
struct V1Commit {
    rev: String,
    operation: String,
    collection: String,
    rkey: String,
    cid: Option<String>,
    // IgnoredAny ensures record text is neither retained nor exposed to filtering code.
    #[allow(dead_code)]
    record: Option<serde::de::IgnoredAny>,
}

fn operation(s: &str) -> Result<Operation, ParseError> {
    match s {
        "create" => Ok(Operation::Create),
        "update" => Ok(Operation::Update),
        "delete" => Ok(Operation::Delete),
        _ => bad("unknown commit operation"),
    }
}

fn commit(
    collection: &str,
    rkey: &str,
    rev: &str,
    op: &str,
    cid_s: Option<&str>,
) -> Result<EventKind, ParseError> {
    Nsid::parse(collection).map_err(|e| ParseError(format!("bad collection: {e}")))?;
    RecordKey::parse(rkey).map_err(|e| ParseError(format!("bad rkey: {e}")))?;
    Tid::parse(rev).map_err(|e| ParseError(format!("bad rev: {e}")))?;
    let operation = operation(op)?;
    let cid = match (cid_s, operation) {
        (Some(s), _) => {
            cid::parse(s).map_err(|e| ParseError(format!("bad cid: {e}")))?;
            Some(s.to_owned())
        }
        (None, Operation::Delete) => None,
        (None, _) => return bad("create/update has no cid"),
    };
    Ok(EventKind::Commit {
        collection: collection.to_owned(),
        rkey: rkey.to_owned(),
        rev: rev.to_owned(),
        operation,
        cid,
    })
}

fn nested_did(outer: &str, inner: &str) -> Result<String, ParseError> {
    let outer = did(outer)?;
    if did(inner)? != outer {
        return bad("outer and inner DIDs differ");
    }
    Ok(outer)
}

fn info(p: &Payload) -> Result<Event, ParseError> {
    let name = p
        .name
        .as_deref()
        .or(p.message.as_deref())
        .or(p.info.as_ref().and_then(|i| i.name.as_deref()))
        .or(p.info.as_ref().and_then(|i| i.message.as_deref()))
        .ok_or_else(|| ParseError("#info has no name".into()))?;
    let kind = match name {
        "OutdatedCursor" => InfoKind::OutdatedCursor,
        "FutureCursor" => InfoKind::FutureCursor,
        _ => return bad("unknown #info name"),
    };
    Ok(Event {
        did: None,
        kind: EventKind::Info(kind),
        seq: None,
        time_us: 0,
    })
}

/// Parses one v1 or v2 text frame. Unknown event/payload types return `Ok(None)`.
pub fn parse(wire: Wire, text: &str) -> Result<Option<Event>, ParseError> {
    match wire {
        Wire::V2 => {
            let envelope: Envelope =
                serde_json::from_str(text).map_err(|e| ParseError(e.to_string()))?;
            if envelope.ty != "message" {
                return Ok(None);
            }
            let p = envelope.payload;
            let short =
                p.ty.strip_prefix("network.bsky.jetstream.subscribeEvents#")
                    .or_else(|| p.ty.strip_prefix('#'));
            let Some(short) = short else { return Ok(None) };
            if short == "info" {
                return info(&p).map(Some);
            }
            let outer_did = did(required(&p.did, "did")?)?;
            let seq = p.seq.ok_or_else(|| ParseError("missing seq".into()))?;
            let time_us = time(required(&p.witnessed_at, "witnessedAt")?)?;
            let kind = match short {
                "commit" => commit(
                    required(&p.collection, "collection")?,
                    required(&p.rkey, "rkey")?,
                    required(&p.rev, "rev")?,
                    required(&p.operation, "operation")?,
                    p.cid.as_deref(),
                )?,
                "identity" => {
                    let i = p
                        .identity
                        .ok_or_else(|| ParseError("missing identity".into()))?;
                    nested_did(&outer_did, &i.did)?;
                    time(&i.time)?;
                    EventKind::Identity { inner_seq: i.seq }
                }
                "account" => {
                    let a = p
                        .account
                        .ok_or_else(|| ParseError("missing account".into()))?;
                    nested_did(&outer_did, &a.did)?;
                    time(&a.time)?;
                    EventKind::Account {
                        inner_seq: a.seq,
                        active: a.active,
                        status: a.status,
                    }
                }
                "sync" => EventKind::Sync,
                _ => return Ok(None),
            };
            Ok(Some(Event {
                did: Some(outer_did),
                kind,
                seq: Some(seq),
                time_us,
            }))
        }
        Wire::V1 => {
            let v: V1 = serde_json::from_str(text).map_err(|e| ParseError(e.to_string()))?;
            if !matches!(v.kind.as_str(), "commit" | "identity" | "account") {
                return Ok(None);
            }
            let time_us = v
                .time_us
                .ok_or_else(|| ParseError("missing time_us".into()))?;
            if time_us < 0 {
                return bad("negative time_us");
            }
            let outer = did(required(&v.did, "did")?)?;
            let kind = match v.kind.as_str() {
                "commit" => {
                    let c = v
                        .commit
                        .ok_or_else(|| ParseError("missing commit".into()))?;
                    commit(
                        &c.collection,
                        &c.rkey,
                        &c.rev,
                        &c.operation,
                        c.cid.as_deref(),
                    )?
                }
                "identity" => {
                    let i = v
                        .identity
                        .ok_or_else(|| ParseError("missing identity".into()))?;
                    nested_did(&outer, &i.did)?;
                    time(&i.time)?;
                    EventKind::Identity { inner_seq: i.seq }
                }
                "account" => {
                    let a = v
                        .account
                        .ok_or_else(|| ParseError("missing account".into()))?;
                    nested_did(&outer, &a.did)?;
                    time(&a.time)?;
                    EventKind::Account {
                        inner_seq: a.seq,
                        active: a.active,
                        status: a.status,
                    }
                }
                _ => return Ok(None),
            };
            Ok(Some(Event {
                did: Some(outer),
                kind,
                seq: None,
                time_us,
            }))
        }
    }
}
