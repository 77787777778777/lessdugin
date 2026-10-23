// SPDX-License-Identifier: AGPL-3.0-or-later
//! `POST /bridge/kv`: the SPA's `state` store, kept in the native `state.sqlite`
//! (`threat-model.md` §7.4), so that drafts, the outbox and read marks never live in the
//! browser's storage for the bridge's origin.
//!
//! Only keys whose first tuple element (`sync.md` §10.3–§10.4) is `out`, `out.budget`,
//! `out.meta`, `out.blob`, `loc`, `notif` or `meta` are reachable (the outbox keeps its rows
//! under the four `out*` elements, `crates/core/src/outbox/model.rs`). Sessions are never stored in `state`, so they are not
//! reachable here. A request is one operation (keys and values are standard base64, RFC 4648
//! §4, with padding):
//!
//! ```text
//! {"op": "get_many", "keys": [k…]}                        → {"values": [v | null…]}
//! {"op": "scan", "start": B, "end": B, "dir": "forward" | "reverse", "limit": n}
//!                                                           → {"entries": [[k, v]…]}
//! {"op": "write", "batch": [{"put": [k, v]} | {"delete": k} | {"deleteRange": [B, B]}…]}
//!                                                           → {}
//! B = {"included": k} | {"excluded": k}
//! ```
//!
//! Some keys can be read but not written or deleted here, because only the native client
//! writes them:
//! - the authorization-server pins, `("loc", "as_pin", did)` (`sync.md` §3.6), so a script in
//!   the tab cannot clear a pin to make the next login trust a new authorization server;
//! - the account rows and the active account, `("loc", "account", did)` and
//!   `("loc", "active_account")`, so a script in the tab cannot switch the account the CLI
//!   writes as, forge or delete account rows, or purge an account's outbox through
//!   `remove_account`;
//! - `("meta", …)`, the `state` schema version, so the tab cannot make the native client
//!   migrate or reset its store.
//!
//! A key outside the scope, or a range not inside one scope, refuses the whole request (403);
//! a batch is applied atomically or not at all.

use std::ops::Bound;

use axum::http::StatusCode;
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use lessdugin_core::host::{Dir, Host as _, KeyRange, KvOp, KvStore as _, prefix_successor};
use lessdugin_core::store::key::{Part, encode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::server::{Web, engine_error, error, json_response};

/// The first tuple elements the SPA may reach.
pub const SCOPES: [&str; 7] = [
    "out",
    "out.budget",
    "out.meta",
    "out.blob",
    "loc",
    "notif",
    "meta",
];

/// The encoded one-element prefix of each scope.
fn scope_prefixes() -> Vec<Vec<u8>> {
    SCOPES.iter().map(|s| encode(&[Part::from(*s)])).collect()
}

/// True if `key` starts with an allowed first element.
pub fn key_allowed(key: &[u8]) -> bool {
    scope_prefixes().iter().any(|p| key.starts_with(p))
}

/// True if every key of the range lies inside one scope: `p ≤ start` and `end ≤ succ(p)`
/// (exclusive end) or `end < succ(p)` (inclusive end). Unbounded ends are refused.
pub fn range_allowed(range: &KeyRange) -> bool {
    let start = match &range.start {
        Bound::Included(s) | Bound::Excluded(s) => s,
        Bound::Unbounded => return false,
    };
    scope_prefixes().iter().any(|p| {
        let Some(succ) = prefix_successor(p) else {
            return false;
        };
        let end_ok = match &range.end {
            Bound::Excluded(e) => e.as_slice() <= succ.as_slice(),
            Bound::Included(e) => e.as_slice() < succ.as_slice(),
            Bound::Unbounded => false,
        };
        start.as_slice() >= p.as_slice() && end_ok
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireBound {
    Included(String),
    Excluded(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireDir {
    Forward,
    Reverse,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum WireOp {
    Put(String, String),
    Delete(String),
    DeleteRange(WireBound, WireBound),
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    GetMany {
        keys: Vec<String>,
    },
    Scan {
        start: WireBound,
        end: WireBound,
        dir: WireDir,
        limit: u32,
    },
    Write {
        batch: Vec<WireOp>,
    },
}

/// A refused or malformed request.
enum Refusal {
    /// Not base64, or not the expected shape.
    Malformed(&'static str),
    /// Outside the scope.
    OutOfScope,
    /// A write to the read-only keys.
    ReadOnly,
}

/// The encoded prefixes of the keys that are read-only here.
fn read_only_prefixes() -> [Vec<u8>; 4] {
    [
        encode(&[Part::from("loc"), Part::from("as_pin")]),
        encode(&[Part::from("loc"), Part::from("account")]),
        encode(&[Part::from("loc"), Part::from("active_account")]),
        encode(&[Part::from("meta")]),
    ]
}

/// True if some key with prefix `p` may lie in the range (conservative).
fn intersects(range: &KeyRange, p: &[u8]) -> bool {
    let Some(succ) = prefix_successor(p) else {
        return true;
    };
    let starts_before_end = match &range.start {
        Bound::Included(s) | Bound::Excluded(s) => s.as_slice() < succ.as_slice(),
        Bound::Unbounded => true,
    };
    let ends_after_start = match &range.end {
        Bound::Excluded(e) => e.as_slice() > p,
        Bound::Included(e) => e.as_slice() >= p,
        Bound::Unbounded => true,
    };
    starts_before_end && ends_after_start
}

/// True if a write may touch `key`.
pub fn write_key_allowed(key: &[u8]) -> bool {
    key_allowed(key) && !read_only_prefixes().iter().any(|p| key.starts_with(p))
}

/// True if a `deleteRange` may cover `range`.
pub fn write_range_allowed(range: &KeyRange) -> bool {
    range_allowed(range) && !read_only_prefixes().iter().any(|p| intersects(range, p))
}

fn b64(s: &str) -> Result<Vec<u8>, Refusal> {
    STANDARD
        .decode(s)
        .map_err(|_| Refusal::Malformed("keys and values are standard base64"))
}

fn key(s: &str) -> Result<Vec<u8>, Refusal> {
    let k = b64(s)?;
    if key_allowed(&k) {
        Ok(k)
    } else {
        Err(Refusal::OutOfScope)
    }
}

fn bound(b: &WireBound) -> Result<Bound<Vec<u8>>, Refusal> {
    Ok(match b {
        WireBound::Included(k) => Bound::Included(b64(k)?),
        WireBound::Excluded(k) => Bound::Excluded(b64(k)?),
    })
}

fn write_key(s: &str) -> Result<Vec<u8>, Refusal> {
    let k = key(s)?;
    if write_key_allowed(&k) {
        Ok(k)
    } else {
        Err(Refusal::ReadOnly)
    }
}

fn write_range(start: &WireBound, end: &WireBound) -> Result<KeyRange, Refusal> {
    let r = range(start, end)?;
    if write_range_allowed(&r) {
        Ok(r)
    } else {
        Err(Refusal::ReadOnly)
    }
}

fn range(start: &WireBound, end: &WireBound) -> Result<KeyRange, Refusal> {
    let r = KeyRange {
        start: bound(start)?,
        end: bound(end)?,
    };
    if range_allowed(&r) {
        Ok(r)
    } else {
        Err(Refusal::OutOfScope)
    }
}

fn refused(r: Refusal) -> Response {
    match r {
        Refusal::Malformed(m) => error(StatusCode::BAD_REQUEST, "invalid_input", m),
        Refusal::ReadOnly => error(
            StatusCode::FORBIDDEN,
            "blocked",
            "accounts, the active account, authorization-server pins and the schema are written \
             only by the native client",
        ),
        Refusal::OutOfScope => error(
            StatusCode::FORBIDDEN,
            "blocked",
            "the bridge store reaches only out, out.budget, out.meta, out.blob, loc, notif and \
             meta keys",
        ),
    }
}

/// Handles one `/bridge/kv` request (it already passed the §7.3 checks).
pub async fn handle(web: &Web, body: &[u8]) -> Response {
    let req: Request = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "expected {\"op\": \"get_many\" | \"scan\" | \"write\", …}",
            );
        }
    };
    let state = web.engine.host().state();
    match req {
        Request::GetMany { keys } => {
            let keys = match keys.iter().map(|k| key(k)).collect::<Result<Vec<_>, _>>() {
                Ok(k) => k,
                Err(r) => return refused(r),
            };
            match state.get_many(keys).await {
                Ok(values) => {
                    let values: Vec<Value> = values
                        .into_iter()
                        .map(|v| v.map_or(Value::Null, |v| Value::String(STANDARD.encode(v))))
                        .collect();
                    json_response(StatusCode::OK, &json!({"values": values}))
                }
                Err(e) => engine_error(&e.into()),
            }
        }
        Request::Scan {
            start,
            end,
            dir,
            limit,
        } => {
            let r = match range(&start, &end) {
                Ok(r) => r,
                Err(r) => return refused(r),
            };
            let dir = match dir {
                WireDir::Forward => Dir::Forward,
                WireDir::Reverse => Dir::Reverse,
            };
            match state.scan(r, dir, limit).await {
                Ok(rows) => {
                    let entries: Vec<Value> = rows
                        .into_iter()
                        .map(|(k, v)| json!([STANDARD.encode(k), STANDARD.encode(v)]))
                        .collect();
                    json_response(StatusCode::OK, &json!({"entries": entries}))
                }
                Err(e) => engine_error(&e.into()),
            }
        }
        Request::Write { batch } => {
            let mut ops = Vec::with_capacity(batch.len());
            for op in &batch {
                let op = match op {
                    WireOp::Put(k, v) => write_key(k).and_then(|k| Ok(KvOp::Put(k, b64(v)?))),
                    WireOp::Delete(k) => write_key(k).map(KvOp::Delete),
                    WireOp::DeleteRange(s, e) => write_range(s, e).map(KvOp::DeleteRange),
                };
                match op {
                    Ok(op) => ops.push(op),
                    Err(r) => return refused(r),
                }
            }
            match state.write(ops).await {
                Ok(()) => json_response(StatusCode::OK, &json!({})),
                Err(e) => engine_error(&e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(parts: &[&str]) -> Vec<u8> {
        encode(&parts.iter().map(|p| Part::from(*p)).collect::<Vec<_>>())
    }

    #[test]
    fn keys_in_scope() {
        assert!(key_allowed(&k(&["out", "did:plc:x"])));
        assert!(key_allowed(&k(&["out.budget", "did:plc:x"])));
        assert!(key_allowed(&k(&["out.meta", "did:plc:x"])));
        assert!(key_allowed(&k(&["out.blob", "did:plc:x"])));
        assert!(key_allowed(&k(&["loc", "draft", "1"])));
        assert!(key_allowed(&k(&["notif", "did:plc:x"])));
        assert!(key_allowed(&k(&["meta", "schema"])));
        assert!(key_allowed(&k(&["meta"])));
        for bad in [
            k(&["rec", "x"]),
            k(&["outx", "a"]),
            k(&["out.budgetx"]),
            k(&["out.metax"]),
            k(&["out.blo"]),
            k(&["session", "did:plc:x"]),
            k(&["lo"]),
            Vec::new(),
            b"out".to_vec(),
        ] {
            assert!(!key_allowed(&bad), "{bad:?}");
        }
    }

    /// The keys every outbox row lives under (`crates/core/src/outbox/model.rs`) are writable:
    /// without them every write from a tab fails.
    #[test]
    fn outbox_rows_are_writable() {
        use lessdugin_core::outbox::model;
        let did = "did:plc:x";
        for key in [
            model::write_key(did, 7),
            model::budget_key(did),
            model::meta_key(did),
            model::blob_chunk_key(did, 7, 0, 0),
        ] {
            assert!(write_key_allowed(&key), "{key:?}");
        }
        for prefix in [model::writes_prefix(did), model::blob_prefix(did, 7)] {
            assert!(range_allowed(&KeyRange::prefix(&prefix)));
            assert!(write_range_allowed(&KeyRange::prefix(&prefix)));
        }
    }

    #[test]
    fn native_only_keys_are_read_only() {
        for key in [
            k(&["loc", "as_pin", "did:plc:x"]),
            k(&["loc", "account", "did:plc:x"]),
            k(&["loc", "active_account"]),
            k(&["meta", "schema"]),
            k(&["meta"]),
        ] {
            assert!(key_allowed(&key), "{key:?}");
            assert!(!write_key_allowed(&key), "{key:?}");
        }
        for range in [
            KeyRange::prefix(&k(&["loc", "account"])),
            KeyRange::prefix(&k(&["meta"])),
        ] {
            assert!(range_allowed(&range));
            assert!(!write_range_allowed(&range));
        }
        assert!(write_key_allowed(&k(&["loc", "accountx"])));
        assert!(write_key_allowed(&k(&["loc", "seeded", "did:plc:x"])));
        assert!(write_key_allowed(&k(&["loc", "follow", "at://x"])));
    }

    #[test]
    fn pins_are_read_only() {
        let pin = k(&["loc", "as_pin", "did:plc:x"]);
        assert!(key_allowed(&pin));
        assert!(!write_key_allowed(&pin));
        assert!(write_key_allowed(&k(&["loc", "draft", "1"])));
        assert!(write_key_allowed(&k(&["loc", "as_pinx"])));
        assert!(!write_range_allowed(&KeyRange::prefix(&k(&["loc"]))));
        assert!(!write_range_allowed(&KeyRange::prefix(&k(&[
            "loc", "as_pin"
        ]))));
        assert!(write_range_allowed(&KeyRange::prefix(&k(&[
            "loc", "draft"
        ]))));
        assert!(write_range_allowed(&KeyRange::prefix(&k(&["out"]))));
        // A range that ends exactly where the pins start does not reach them.
        assert!(write_range_allowed(&KeyRange {
            start: Bound::Included(k(&["loc", "ad"])),
            end: Bound::Excluded(k(&["loc", "as_pin"])),
        }));
        assert!(!write_range_allowed(&KeyRange {
            start: Bound::Included(k(&["loc", "ad"])),
            end: Bound::Included(k(&["loc", "as_pin"])),
        }));
        // From "a" on, the range covers the account rows.
        assert!(!write_range_allowed(&KeyRange {
            start: Bound::Included(k(&["loc", "a"])),
            end: Bound::Excluded(k(&["loc", "as_pin"])),
        }));
    }

    #[test]
    fn ranges_in_scope() {
        assert!(range_allowed(&KeyRange::prefix(&k(&["out"]))));
        assert!(range_allowed(&KeyRange::prefix(&k(&["out", "did:plc:x"]))));
        assert!(range_allowed(&KeyRange::prefix(&k(&["loc", "draft"]))));
        assert!(range_allowed(&KeyRange {
            start: Bound::Excluded(k(&["notif", "a"])),
            end: Bound::Included(k(&["notif", "b"])),
        }));
        // Spanning two scopes, or outside every scope.
        assert!(!range_allowed(&KeyRange {
            start: Bound::Included(k(&["loc"])),
            end: Bound::Excluded(k(&["out", "z"])),
        }));
        assert!(!range_allowed(&KeyRange::prefix(&k(&["rec"]))));
        assert!(!range_allowed(&KeyRange::prefix(b"")));
        assert!(!range_allowed(&KeyRange::prefix(b"\x02o")));
        assert!(!range_allowed(&KeyRange {
            start: Bound::Unbounded,
            end: Bound::Excluded(k(&["out", "z"])),
        }));
        assert!(!range_allowed(&KeyRange {
            start: Bound::Included(k(&["out"])),
            end: Bound::Unbounded,
        }));
        // The inclusive end at the successor itself is one key too far.
        let p = k(&["out"]);
        let succ = prefix_successor(&p).unwrap();
        assert!(range_allowed(&KeyRange {
            start: Bound::Included(p.clone()),
            end: Bound::Excluded(succ.clone()),
        }));
        assert!(!range_allowed(&KeyRange {
            start: Bound::Included(p),
            end: Bound::Included(succ),
        }));
    }
}
