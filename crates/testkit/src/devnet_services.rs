// SPDX-License-Identifier: AGPL-3.0-or-later
//! The CORE+ services of the node devnet (`packages/e2e/devnet.mjs`, `docs/plan.md` §4.1) for
//! the end-to-end tests, over a mirror of the devnet PDS's firehose
//! (`com.atproto.sync.subscribeRepos`, followed from sequence 0):
//!
//! - a mini-Jetstream serving both wires on one listener ([`Jetstream::start_both`]: v1 at
//!   `/subscribe`, v2 at `subscribeEvents`), with identity and account noise for unrelated DIDs
//!   ([`NOISE_DIDS`]) mixed into its log, as the public network has;
//! - a Constellation ([`Constellation`], `getBacklinks` and `getBacklinksCount`) whose pages are
//!   short and still carry a cursor, the last one to an empty final page;
//! - a relay answering `com.atproto.sync.listReposByCollection` the same way.
//!
//! Every request the services answer is logged ([`DevnetServices::log`], and `GET
//! /_devnet/log` for a harness in another process), so a test can tell which client contacted
//! which CORE+ service, and that a `sources = direct` client contacted none. Everything is
//! loopback. The services are hints only: Jetstream frames carry no record text, and the
//! mirrored records feed only the Constellation and relay indexes; clients still read and verify
//! every record at its author's PDS.

// A real process against a real network: wall-clock time for the noise and the log.
#![allow(clippy::disallowed_methods)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::StreamExt;
use lessdugin_atp::car::{CarLimits, read_car};
use lessdugin_atp::syntax::{Datetime, Did, Tid};
use lessdugin_atp::{Cid, Ipld, dagcbor, datamodel};
use lessdugin_core::host::{HttpRequest, Target};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::constellation::{BacklinkSource, Constellation};
use crate::fake_pds::{BacklinkSnapshot, OpAction, RepoEvent, RepoEventKind, RepoOp};
use crate::jetstream::{EventLog, Jetstream, RequestLog};

/// The unrelated DIDs of the identity and account noise. No devnet account has them.
pub const NOISE_DIDS: [&str; 3] = [
    "did:plc:noiseaaaaaaaaaaaaaaaaaab",
    "did:plc:noiseaaaaaaaaaaaaaaaaaac",
    "did:plc:noiseaaaaaaaaaaaaaaaaaad",
];

/// How often a noise event is added to the Jetstream log.
const NOISE_EVERY: Duration = Duration::from_millis(500);
/// Relay page size: short, so a client must follow the cursor.
const RELAY_PAGE: usize = 1;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
}

/// What the mirror holds: the Jetstream log (devnet events and noise, in one sequence) and the
/// current records of every active repo.
#[derive(Default)]
struct MirrorState {
    events: Vec<RepoEvent>,
    records: BTreeMap<Did, BTreeMap<(String, String), Value>>,
    inactive: BTreeSet<Did>,
    /// The last firehose sequence ingested (the resume cursor).
    upstream: i64,
    /// Firehose frames that could not be decoded (diagnostics).
    skipped: u64,
    noise: u64,
}

impl MirrorState {
    fn push(&mut self, did: Did, time_us: i64, kind: RepoEventKind) {
        let seq = self.events.last().map_or(1, |e| e.seq + 1);
        self.events.push(RepoEvent {
            seq,
            time_us,
            did,
            kind,
        });
    }
}

/// The devnet PDS's firehose, mirrored.
#[derive(Clone, Default)]
struct Mirror(Arc<Mutex<MirrorState>>);

impl EventLog for Mirror {
    fn events(&self) -> Vec<RepoEvent> {
        lock(&self.0).events.clone()
    }

    fn events_since(&self, after: u64) -> Vec<RepoEvent> {
        let st = lock(&self.0);
        let from = st.events.partition_point(|e| e.seq <= after);
        st.events[from..].to_vec()
    }
}

impl BacklinkSource for Mirror {
    fn snapshot(&self) -> BacklinkSnapshot {
        let st = lock(&self.0);
        st.records
            .iter()
            .filter(|(did, _)| !st.inactive.contains(*did))
            .map(|(did, records)| {
                let records = records
                    .iter()
                    .map(|((c, r), v)| (c.clone(), r.clone(), v.clone()))
                    .collect();
                (did.clone(), records)
            })
            .collect()
    }
}

/// The length of the first DAG-CBOR item of `b` (a firehose frame is a header item followed by
/// a body item).
fn cbor_item_len(b: &[u8]) -> Option<usize> {
    fn arg(b: &[u8], info: u8) -> Option<(u64, usize)> {
        let n = match info {
            0..=23 => return Some((u64::from(info), 1)),
            24 => 1,
            25 => 2,
            26 => 4,
            27 => 8,
            _ => return None,
        };
        let bytes = b.get(1..=n)?;
        Some((
            bytes.iter().fold(0u64, |a, &x| (a << 8) | u64::from(x)),
            1 + n,
        ))
    }
    fn item(b: &[u8], depth: usize) -> Option<usize> {
        if depth > 64 {
            return None;
        }
        let first = *b.first()?;
        let (major, info) = (first >> 5, first & 0x1f);
        let (value, head) = arg(b, info)?;
        let n = usize::try_from(value).ok()?;
        match major {
            0 | 1 => Some(head),
            2 | 3 => head.checked_add(n).filter(|&end| end <= b.len()),
            4 | 5 => {
                let items = if major == 5 { n.checked_mul(2)? } else { n };
                let mut at = head;
                for _ in 0..items {
                    at += item(b.get(at..)?, depth + 1)?;
                }
                Some(at)
            }
            6 => Some(head + item(b.get(head..)?, depth + 1)?),
            7 => Some(head),
            _ => None,
        }
    }
    item(b, 0)
}

fn field<'a>(map: &'a BTreeMap<String, Ipld>, key: &str) -> Option<&'a Ipld> {
    map.get(key)
}

fn text<'a>(map: &'a BTreeMap<String, Ipld>, key: &str) -> Option<&'a str> {
    match field(map, key)? {
        Ipld::String(s) => Some(s),
        _ => None,
    }
}

fn int(map: &BTreeMap<String, Ipld>, key: &str) -> Option<i64> {
    match field(map, key)? {
        Ipld::Integer(n) => i64::try_from(*n).ok(),
        _ => None,
    }
}

/// One firehose frame into the mirror. `None` when it is not a frame the mirror understands.
fn ingest(mirror: &Mirror, frame: &[u8]) -> Option<()> {
    let split = cbor_item_len(frame)?;
    let Ipld::Map(header) = dagcbor::decode(&frame[..split]).ok()? else {
        return None;
    };
    if int(&header, "op") != Some(1) {
        return None;
    }
    let Ipld::Map(body) = dagcbor::decode(&frame[split..]).ok()? else {
        return None;
    };
    let t = text(&header, "t")?;
    let seq = int(&body, "seq")?;
    let time_us = text(&body, "time")
        .and_then(|t| Datetime::parse(t).ok())
        .map_or_else(now_us, |t| t.timestamp_micros());
    let mut st = lock(&mirror.0);
    if seq <= st.upstream {
        // a replay after a reconnect
        return Some(());
    }
    match t {
        "#commit" => {
            let did = Did::parse(text(&body, "repo")?).ok()?;
            let rev = Tid::parse(text(&body, "rev")?).ok()?;
            let Some(Ipld::Link(commit)) = field(&body, "commit") else {
                return None;
            };
            let blocks = match field(&body, "blocks") {
                Some(Ipld::Bytes(b)) if !b.is_empty() => read_car(b, &CarLimits::default()).ok(),
                _ => None,
            };
            let Some(Ipld::List(raw_ops)) = field(&body, "ops") else {
                return None;
            };
            let mut ops = Vec::new();
            for op in raw_ops {
                let Ipld::Map(op) = op else { return None };
                let (collection, rkey) = text(op, "path")?.split_once('/')?;
                let action = match text(op, "action")? {
                    "create" => OpAction::Create,
                    "update" => OpAction::Update,
                    "delete" => OpAction::Delete,
                    _ => return None,
                };
                let cid: Option<Cid> = match field(op, "cid") {
                    Some(Ipld::Link(c)) => Some(*c),
                    _ => None,
                };
                let prev: Option<Cid> = match field(op, "prev") {
                    Some(Ipld::Link(c)) => Some(*c),
                    _ => None,
                };
                let record: Option<Arc<[u8]>> =
                    cid.and_then(|c| blocks.as_ref()?.get(&c).map(Arc::from));
                let key = (collection.to_owned(), rkey.to_owned());
                let repo = st.records.entry(did.clone()).or_default();
                match (&action, record.as_deref()) {
                    (OpAction::Delete, _) => {
                        repo.remove(&key);
                    }
                    (_, Some(bytes)) => {
                        if let Ok(value) = datamodel::record_cbor_to_json(bytes) {
                            repo.insert(key, value);
                        }
                    }
                    (_, None) => {}
                }
                ops.push(RepoOp {
                    action,
                    collection: collection.to_owned(),
                    rkey: rkey.to_owned(),
                    cid,
                    prev,
                    record,
                });
            }
            let commit = *commit;
            st.push(
                did,
                time_us,
                RepoEventKind::Commit {
                    rev,
                    cid: commit,
                    ops,
                },
            );
        }
        "#identity" => {
            let did = Did::parse(text(&body, "did")?).ok()?;
            let handle = text(&body, "handle").map(str::to_owned);
            st.push(did, time_us, RepoEventKind::Identity { handle });
        }
        "#account" => {
            let did = Did::parse(text(&body, "did")?).ok()?;
            let active = matches!(field(&body, "active"), Some(Ipld::Bool(true)));
            let status = text(&body, "status").map(str::to_owned);
            if active {
                st.inactive.remove(&did);
            } else {
                st.inactive.insert(did.clone());
            }
            st.push(did, time_us, RepoEventKind::Account { active, status });
        }
        // #sync, #info and anything newer: not needed by the hints these services give
        _ => {}
    }
    st.upstream = seq;
    Some(())
}

/// Follows the firehose of `pds` (an `http://` devnet origin) for as long as the runtime runs,
/// reconnecting from the last sequence ingested.
async fn follow(pds: url::Url, mirror: Mirror) {
    loop {
        let mut url = pds.clone();
        let scheme = if pds.scheme() == "https" { "wss" } else { "ws" };
        if url.set_scheme(scheme).is_ok() {
            url.set_path("/xrpc/com.atproto.sync.subscribeRepos");
            let cursor = lock(&mirror.0).upstream;
            url.set_query(Some(&format!("cursor={cursor}")));
            if let Ok((mut ws, _)) = tokio_tungstenite::connect_async(url.as_str()).await {
                while let Some(Ok(message)) = ws.next().await {
                    match message {
                        Message::Binary(frame) => {
                            if ingest(&mirror, &frame).is_none() {
                                lock(&mirror.0).skipped += 1;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Adds identity and account events for the [`NOISE_DIDS`], one every [`NOISE_EVERY`].
async fn noise(mirror: Mirror) {
    let dids: Vec<Did> = NOISE_DIDS
        .iter()
        .map(|d| Did::parse(d).expect("noise DID"))
        .collect();
    loop {
        {
            let mut st = lock(&mirror.0);
            let n = st.noise;
            st.noise += 1;
            let did = dids[usize::try_from(n).unwrap_or(0) % dids.len()].clone();
            let kind = if n.is_multiple_of(2) {
                RepoEventKind::Identity {
                    handle: Some(format!("noise-{n}.invalid")),
                }
            } else {
                let active = n % 4 == 1;
                RepoEventKind::Account {
                    active,
                    status: (!active).then(|| "deactivated".to_owned()),
                }
            };
            st.push(did, now_us(), kind);
        }
        tokio::time::sleep(NOISE_EVERY).await;
    }
}

/// One HTTP request the Constellation or the relay answered.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ServedRequest {
    /// `constellation` or `relay`.
    pub service: &'static str,
    /// The XRPC method.
    pub nsid: String,
    /// Path and query.
    pub url: String,
    /// The response status.
    pub status: u16,
    /// Arrival (ms since the epoch).
    pub at_ms: i64,
}

struct Http {
    mirror: Mirror,
    constellation: Constellation,
    jetstream: Arc<Mutex<Option<Jetstream>>>,
    log: Mutex<Vec<ServedRequest>>,
}

/// A running set of devnet CORE+ services (stopped when dropped).
pub struct DevnetServices {
    http_base: String,
    jetstream_base: String,
    http: Arc<Http>,
    jetstream: Arc<Mutex<Option<Jetstream>>>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Drop for DevnetServices {
    fn drop(&mut self) {
        lock(&self.jetstream).take();
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

fn response(status: u16, body: Value) -> axum::response::Response {
    axum::response::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("access-control-allow-origin", "*")
        .header("cache-control", "no-store")
        .body(axum::body::Body::from(body.to_string()))
        .expect("response")
}

fn xrpc_error(status: u16, message: &str) -> axum::response::Response {
    response(
        status,
        json!({"error": "InvalidRequest", "message": message}),
    )
}

impl Http {
    fn relay(&self, params: &BTreeMap<String, String>) -> (u16, Value) {
        let Some(collection) = params.get("collection") else {
            return (
                400,
                json!({"error": "InvalidRequest", "message": "collection"}),
            );
        };
        let limit = params
            .get("limit")
            .and_then(|l| l.parse::<usize>().ok())
            .unwrap_or(500)
            .clamp(1, RELAY_PAGE);
        let after = params.get("cursor").filter(|c| !c.is_empty());
        let mut dids: Vec<String> = self
            .mirror
            .snapshot()
            .into_iter()
            .filter(|(_, records)| records.iter().any(|(c, _, _)| c == collection))
            .map(|(did, _)| did.to_string())
            .filter(|d| after.is_none_or(|a| d.as_str() > a.as_str()))
            .collect();
        dids.sort();
        let page: Vec<String> = dids.iter().take(limit).cloned().collect();
        // the last non-empty page still carries a cursor; the empty page after it has none
        let mut out = json!({"repos": page.iter().map(|d| json!({"did": d})).collect::<Vec<_>>()});
        if let Some(last) = page.last() {
            out["cursor"] = json!(last);
        }
        (200, out)
    }

    fn log_json(&self) -> Value {
        let jetstream: Vec<Value> = lock(&self.jetstream)
            .as_ref()
            .map(Jetstream::requests)
            .unwrap_or_default()
            .iter()
            .map(jetstream_entry)
            .collect();
        let st = lock(&self.mirror.0);
        json!({
            "http": *lock(&self.log),
            "jetstream": jetstream,
            "mirror": {
                "events": st.events.len(),
                "upstreamSeq": st.upstream,
                "skipped": st.skipped,
                "noise": st.noise,
            },
        })
    }

    async fn handle(self: Arc<Self>, req: axum::extract::Request) -> axum::response::Response {
        let method = req.method().clone();
        let uri = req.uri().clone();
        let path = uri.path().to_owned();
        let url = uri.to_string();
        let params: BTreeMap<String, String> =
            url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                .into_owned()
                .collect();
        if method == http::Method::OPTIONS {
            return axum::response::Response::builder()
                .status(204)
                .header("access-control-allow-origin", "*")
                .header("access-control-allow-methods", "GET")
                .header("access-control-allow-headers", "*")
                .body(axum::body::Body::empty())
                .expect("response");
        }
        if method != http::Method::GET {
            return xrpc_error(405, "only GET");
        }
        match path.as_str() {
            "/_devnet/log" => return response(200, self.log_json()),
            "/_devnet/record" => {
                let found = params.get("uri").and_then(|u| {
                    let at = lessdugin_atp::syntax::AtUri::parse(u).ok()?;
                    let key = (at.collection().to_string(), at.rkey().to_string());
                    lock(&self.mirror.0)
                        .records
                        .get(at.did())?
                        .get(&key)
                        .cloned()
                });
                return match found {
                    Some(_) => response(200, json!({"present": true})),
                    None => response(404, json!({"present": false})),
                };
            }
            _ => {}
        }
        let nsid = path.strip_prefix("/xrpc/").unwrap_or(&path).to_owned();
        let (service, status, body) = match nsid.as_str() {
            "blue.microcosm.links.getBacklinks" | "blue.microcosm.links.getBacklinksCount" => {
                let request = HttpRequest::get(Target::Anon(
                    url::Url::parse(&format!("{}{url}", self.constellation.base_url()))
                        .expect("request URL"),
                ));
                let count = nsid.ends_with("Count");
                match self.constellation.answer(&request, count) {
                    Ok(r) => (
                        "constellation",
                        r.status,
                        serde_json::from_slice(&r.body).unwrap_or(Value::Null),
                    ),
                    Err(e) => (
                        "constellation",
                        500,
                        json!({"error": "InternalServerError", "message": e.to_string()}),
                    ),
                }
            }
            "com.atproto.sync.listReposByCollection" => {
                let (status, body) = self.relay(&params);
                ("relay", status, body)
            }
            _ => (
                "unknown",
                501,
                json!({"error": "MethodNotImplemented", "message": "Method Not Implemented"}),
            ),
        };
        lock(&self.log).push(ServedRequest {
            service,
            nsid,
            url,
            status,
            at_ms: now_us() / 1000,
        });
        response(status, body)
    }
}

fn jetstream_entry(r: &RequestLog) -> Value {
    let noise = r
        .sent
        .iter()
        .filter(|f| NOISE_DIDS.iter().any(|d| f.contains(d)))
        .count();
    json!({
        "url": r.url,
        "wire": match r.wire {
            crate::jetstream::Wire::V1 => "v1",
            crate::jetstream::Wire::V2 => "v2",
        },
        "options": r.options,
        "frames": r.sent.len(),
        "noiseFrames": noise,
    })
}

impl DevnetServices {
    /// Starts the services against the devnet PDS at `pds` (e.g. `http://localhost:2583`).
    ///
    /// # Panics
    /// When `pds` is not a URL or a loopback listener cannot be bound.
    pub fn start(pds: &str) -> Self {
        let pds = url::Url::parse(pds).expect("the devnet PDS URL");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("devnet-services")
            .enable_all()
            .build()
            .expect("devnet services runtime");
        let mirror = Mirror::default();
        runtime.spawn(follow(pds, mirror.clone()));
        runtime.spawn(noise(mirror.clone()));
        let jetstream = Jetstream::start_both(mirror.clone());
        let jetstream_base = jetstream.base_url().to_owned();
        let jetstream = Arc::new(Mutex::new(Some(jetstream)));
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind the devnet services");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let http_base = format!("http://{}", listener.local_addr().expect("local address"));
        let http = Arc::new(Http {
            constellation: Constellation::over(Arc::new(mirror.clone()), &http_base),
            mirror,
            jetstream: Arc::clone(&jetstream),
            log: Mutex::new(Vec::new()),
        });
        let served = Arc::clone(&http);
        runtime.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            let app = axum::Router::new()
                .fallback(move |req: axum::extract::Request| Arc::clone(&served).handle(req));
            let _ = axum::serve(listener, app).await;
        });
        Self {
            http_base,
            jetstream_base,
            http,
            jetstream,
            runtime: Some(runtime),
        }
    }

    /// The Jetstream base URL (`ws://127.0.0.1:<port>`; both wires).
    pub fn jetstream_url(&self) -> &str {
        &self.jetstream_base
    }

    /// The Constellation base URL.
    pub fn constellation_url(&self) -> &str {
        &self.http_base
    }

    /// The relay base URL (the same listener as the Constellation).
    pub fn relay_url(&self) -> &str {
        &self.http_base
    }

    /// The HTTP requests answered so far.
    pub fn requests(&self) -> Vec<ServedRequest> {
        lock(&self.http.log).clone()
    }

    /// The Jetstream connections so far.
    pub fn jetstream_requests(&self) -> Vec<RequestLog> {
        lock(&self.jetstream)
            .as_ref()
            .map(Jetstream::requests)
            .unwrap_or_default()
    }

    /// Everything as the `/_devnet/log` JSON: `http`, `jetstream` (with frame and noise
    /// counts) and the `mirror`'s progress.
    pub fn log(&self) -> Value {
        self.http.log_json()
    }

    /// True once the mirror holds the record `uri` (its create reached the firehose).
    pub fn has_record(&self, uri: &str) -> bool {
        let Ok(at) = lessdugin_atp::syntax::AtUri::parse(uri) else {
            return false;
        };
        let key = (at.collection().to_string(), at.rkey().to_string());
        lock(&self.http.mirror.0)
            .records
            .get(at.did())
            .is_some_and(|r| r.contains_key(&key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_cbor_item_of_a_frame_is_measured_exactly() {
        let header = dagcbor::encode(&Ipld::Map(BTreeMap::from([
            ("op".to_owned(), Ipld::Integer(1)),
            ("t".to_owned(), Ipld::String("#identity".into())),
        ])))
        .unwrap();
        let body = dagcbor::encode(&Ipld::Map(BTreeMap::from([
            ("did".to_owned(), Ipld::String(NOISE_DIDS[0].into())),
            ("seq".to_owned(), Ipld::Integer(7)),
            (
                "time".to_owned(),
                Ipld::String("2026-11-03T00:00:00.000Z".into()),
            ),
            ("handle".to_owned(), Ipld::String("a.test".into())),
            ("blob".to_owned(), Ipld::Bytes(vec![0; 300])),
            (
                "list".to_owned(),
                Ipld::List(vec![Ipld::Null, Ipld::Bool(true)]),
            ),
        ])))
        .unwrap();
        let frame = [header.clone(), body].concat();
        assert_eq!(cbor_item_len(&frame), Some(header.len()));
        let mirror = Mirror::default();
        assert_eq!(ingest(&mirror, &frame), Some(()));
        let events = mirror.events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].kind,
            RepoEventKind::Identity { handle: Some(h) } if h == "a.test"
        ));
        // a replayed sequence is not ingested twice
        assert_eq!(ingest(&mirror, &frame), Some(()));
        assert_eq!(mirror.events().len(), 1);
        assert_eq!(cbor_item_len(&frame[..header.len() - 1]), None);
    }

    #[test]
    fn the_relay_pages_are_short_and_end_with_an_empty_page() {
        let mirror = Mirror::default();
        {
            let mut st = lock(&mirror.0);
            for (i, d) in NOISE_DIDS.iter().enumerate() {
                let records = st.records.entry(Did::parse(d).unwrap()).or_default();
                records.insert(
                    (
                        "online.7778777.lessdugin.temp.membership".into(),
                        format!("k{i}"),
                    ),
                    json!({}),
                );
            }
            st.inactive.insert(Did::parse(NOISE_DIDS[2]).unwrap());
        }
        let http = Http {
            constellation: Constellation::over(Arc::new(mirror.clone()), "http://127.0.0.1:1"),
            mirror,
            jetstream: Arc::new(Mutex::new(None)),
            log: Mutex::new(Vec::new()),
        };
        let mut params = BTreeMap::from([(
            "collection".to_owned(),
            "online.7778777.lessdugin.temp.membership".to_owned(),
        )]);
        let mut seen = Vec::new();
        for _ in 0..5 {
            let (status, page) = http.relay(&params);
            assert_eq!(status, 200);
            let repos = page["repos"].as_array().unwrap();
            assert!(repos.len() <= RELAY_PAGE);
            seen.extend(repos.iter().map(|r| r["did"].as_str().unwrap().to_owned()));
            match page.get("cursor").and_then(Value::as_str) {
                Some(c) => {
                    params.insert("cursor".into(), c.into());
                }
                None => {
                    assert!(repos.is_empty(), "the final page is the empty one");
                    break;
                }
            }
        }
        // the deactivated repo is not listed
        assert_eq!(seen, [NOISE_DIDS[0], NOISE_DIDS[1]]);
    }
}
