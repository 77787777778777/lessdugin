// SPDX-License-Identifier: AGPL-3.0-or-later
//! Loopback mini-Jetstream for devnet tests. It serves signed commit hints, never record text:
//! clients must still read and verify each record from its author's PDS. The owned runtime
//! stops with this value; no socket task can outlive the test network.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

use futures::{SinkExt, StreamExt};
use lessdugin_atp::syntax::{Datetime, Did};
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

use super::fake_pds::{FakeNet, RepoEvent, RepoEventKind};

const MAX_FRAME_BYTES: usize = 1_200_000;
const MAX_EVENTS: usize = 10_000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which Jetstream protocol a listener serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    /// Legacy `/subscribe`, with `requireHello` and `options_update`.
    V1,
    /// `network.bsky.jetstream.subscribeEvents` envelopes.
    V2,
}

/// A received connection or v1 filter update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestLog {
    /// URL path and query.
    pub url: String,
    /// `options_update` payload, if received.
    pub options: Option<Value>,
}

/// A running, loopback-only mini-Jetstream.
pub struct Jetstream {
    base: String,
    runtime: Option<tokio::runtime::Runtime>,
    requests: Arc<Mutex<Vec<RequestLog>>>,
}

#[derive(Default)]
struct Filters {
    collections: BTreeSet<String>,
    dids: BTreeSet<String>,
}

impl Filters {
    fn matches(&self, did: &Did, collection: Option<&str>) -> bool {
        if !self.dids.is_empty() && !self.dids.contains(did.as_str()) {
            return false;
        }
        collection.is_none_or(|c| {
            self.collections.is_empty()
                || self.collections.iter().any(|p| {
                    p == c
                        || p.strip_suffix(".*").is_some_and(|prefix| {
                            c.strip_prefix(prefix)
                                .is_some_and(|rest| rest.starts_with('.'))
                        })
                })
        })
    }
}

impl Jetstream {
    /// Start a Jetstream backed by `net`'s event log. Only loopback TCP is used.
    pub fn start(net: &FakeNet, wire: Wire) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mini-Jetstream");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let base = format!("ws://{}", listener.local_addr().expect("local address"));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("mini-jetstream")
            .enable_all()
            .build()
            .expect("Jetstream runtime");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let net = net.clone();
        runtime.spawn(async move {
            let listener = TcpListener::from_std(listener).expect("tokio listener");
            while let Ok((tcp, _)) = listener.accept().await {
                let net = net.clone();
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    serve(tcp, net, log, wire).await;
                });
            }
        });
        Self {
            base,
            runtime: Some(runtime),
            requests,
        }
    }

    /// Base URL passed to a devnet engine.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Connection and filter requests observed by this service.
    pub fn requests(&self) -> Vec<RequestLog> {
        lock(&self.requests).clone()
    }
}

impl Drop for Jetstream {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

async fn serve(
    tcp: tokio::net::TcpStream,
    net: FakeNet,
    requests: Arc<Mutex<Vec<RequestLog>>>,
    wire: Wire,
) {
    let log = Arc::clone(&requests);
    let (url_tx, mut url_rx) = tokio::sync::mpsc::unbounded_channel();
    // tokio-tungstenite's Callback trait requires this concrete, large error type; boxing it
    // would change the callback signature and no longer implement the trait.
    #[allow(clippy::result_large_err)]
    let callback = move |req: &Request, mut resp: Response| {
        resp.headers_mut().insert(
            "sec-websocket-protocol",
            "xrpc.v1.json".parse().expect("static header"),
        );
        let url = req.uri().to_string();
        lock(&log).push(RequestLog {
            url: url.clone(),
            options: None,
        });
        let _ = url_tx.send(url);
        Ok(resp)
    };
    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
        return;
    };
    let Some(first_url) = url_rx.recv().await else {
        return;
    };
    let Ok(parsed) = url::Url::parse(&format!("ws://localhost{}", first_url)) else {
        return;
    };
    let params: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
    let params_map: BTreeMap<_, _> = params.iter().cloned().collect();
    let hello = params_map.get("requireHello").is_some_and(|v| v == "true");
    let options = if wire == Wire::V1 && hello {
        match tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    return;
                };
                if value.get("type").and_then(Value::as_str) != Some("options_update") {
                    return;
                }
                let options = value.get("payload").cloned().unwrap_or(Value::Null);
                lock(&requests).push(RequestLog {
                    url: first_url,
                    options: Some(options.clone()),
                });
                options
            }
            _ => return,
        }
    } else {
        Value::Null
    };
    let wanted = filters(&params, &options, wire);
    let cursor = params_map
        .get("cursor")
        .and_then(|c| c.parse::<u64>().ok())
        .unwrap_or(0);
    let mut sent = 0;
    let mut last_seq = 0;
    // v2 sequence cursors are inclusive. Timestamp cursors (v1, or a v2 instance change)
    // include the rewind boundary; core deduplication handles that deliberate overlap.
    let timestamp_cursor = cursor > net.events().last().map_or(0, |e| e.seq);
    let initial = if timestamp_cursor {
        net.events()
            .into_iter()
            .filter(|e| e.time_us >= i64::try_from(cursor).unwrap_or(i64::MAX))
            .collect()
    } else {
        net.events_since(cursor.saturating_sub(1))
    };
    let mut pending = initial;
    loop {
        for event in std::mem::take(&mut pending) {
            last_seq = last_seq.max(event.seq);
            for frame in frames(&event, wire, &wanted) {
                if frame.len() > MAX_FRAME_BYTES
                    || ws.send(Message::Text(frame.into())).await.is_err()
                {
                    return;
                }
                sent += 1;
                if sent >= MAX_EVENTS {
                    return;
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                pending = net.events_since(last_seq);
            }
            message = ws.next() => {
                match message {
                    Some(Ok(Message::Close(_) | Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return,
                }
            }
        }
    }
}

fn filters(params: &[(String, String)], options: &Value, wire: Wire) -> Filters {
    let mut wanted = Filters::default();
    if wire == Wire::V1 {
        for (key, field) in [("wantedCollections", "collections"), ("wantedDids", "dids")] {
            if let Some(values) = options.get(key).and_then(Value::as_array) {
                let target = if field == "collections" {
                    &mut wanted.collections
                } else {
                    &mut wanted.dids
                };
                target.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        if wanted.collections.is_empty() {
            wanted.collections.extend(
                params
                    .iter()
                    .filter(|(k, _)| k == "wantedCollections")
                    .map(|(_, v)| v.clone()),
            );
        }
    } else {
        for (k, v) in params {
            match k.as_str() {
                "collections" => {
                    wanted.collections.insert(v.clone());
                }
                "dids" => {
                    wanted.dids.insert(v.clone());
                }
                _ => {}
            }
        }
    }
    wanted
}

fn extend(target: &mut Value, source: Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        target.extend(source.clone());
    }
}

fn frames(event: &RepoEvent, wire: Wire, wanted: &Filters) -> Vec<String> {
    let Ok(time) = Datetime::from_micros(event.time_us) else {
        return Vec::new();
    };
    let time = time.to_string();
    let mut payloads = Vec::new();
    match &event.kind {
        RepoEventKind::Commit { rev, ops, .. } => {
            for op in ops {
                if !wanted.matches(&event.did, Some(&op.collection)) {
                    continue;
                }
                let mut p = Map::new();
                p.insert("rev".into(), json!(rev.to_string()));
                p.insert("operation".into(), json!(op.action.as_str()));
                p.insert("collection".into(), json!(op.collection));
                p.insert("rkey".into(), json!(op.rkey));
                if let Some(cid) = op.cid {
                    p.insert("cid".into(), json!(cid.to_string()));
                }
                payloads.push(("commit", Value::Object(p)));
            }
        }
        RepoEventKind::Identity { handle } => {
            if wanted.matches(&event.did, None) {
                payloads.push((
                    "identity",
                    json!({"identity": {"did": event.did, "seq": event.seq, "time": time, "handle": handle}}),
                ));
            }
        }
        RepoEventKind::Account { active, status } => {
            if wanted.matches(&event.did, None) {
                payloads.push((
                    "account",
                    json!({"account": {"did": event.did, "seq": event.seq, "time": time, "active": active, "status": status}}),
                ));
            }
        }
    }
    payloads
        .into_iter()
        .map(|(kind, body)| {
            if wire == Wire::V1 {
                let mut out = json!({"did": event.did, "time_us": event.time_us, "kind": kind});
                extend(&mut out, body);
                out.to_string()
            } else {
                let mut payload = json!({
                    "$type": format!("network.bsky.jetstream.subscribeEvents#{kind}"),
                    "did": event.did, "seq": event.seq, "witnessedAt": time,
                });
                extend(&mut payload, body);
                json!({"$type": "message", "payload": payload}).to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_atp::syntax::Tid;

    #[test]
    fn both_wires_emit_commit_hints_not_record_text() {
        let did = Did::parse("did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let event = RepoEvent {
            seq: 7,
            time_us: 1_760_000_000_000_000,
            did,
            kind: RepoEventKind::Commit {
                rev: Tid::from_parts(1_760_000_000_000_000, 0).unwrap(),
                cid: lessdugin_atp::cid::for_raw(&[1]),
                ops: vec![super::super::fake_pds::RepoOp {
                    action: super::super::fake_pds::OpAction::Create,
                    collection: "online.7778777.lessdugin.post".into(),
                    rkey: "f-3l2s5xxv2zaa2".into(),
                    cid: Some(lessdugin_atp::cid::for_raw(&[1])),
                    prev: None,
                    record: Some(Arc::from(&b"private record text"[..])),
                }],
            },
        };
        let wanted = Filters::default();
        for wire in [Wire::V1, Wire::V2] {
            let frames = frames(&event, wire, &wanted);
            assert_eq!(frames.len(), 1);
            assert!(frames[0].contains("online.7778777.lessdugin.post"));
            assert!(!frames[0].contains("private record text"));
        }
    }

    #[tokio::test]
    async fn serves_real_v1_and_v2_websockets() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let net = FakeNet::serve_http(crate::fake_pds::SystemClock);
        let alice = net.add_account("alice.test");
        alice.put(
            "online.7778777.lessdugin.post",
            "f-3l2s5xxv2zaa2",
            &json!({
                "forum": alice.uri("online.7778777.lessdugin.forum", "self"),
                "title": "test", "createdAt": "2025-10-09T08:53:20Z"
            }),
        );
        alice.head();
        for wire in [Wire::V1, Wire::V2] {
            let server = Jetstream::start(&net, wire);
            let path = if wire == Wire::V1 {
                "/subscribe?requireHello=true&wantedCollections=online.7778777.lessdugin.*"
            } else {
                "/xrpc/network.bsky.jetstream.subscribeEvents?collections=online.7778777.lessdugin.*"
            };
            let mut request = format!("{}{}", server.base_url(), path)
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert("sec-websocket-protocol", "xrpc.v1.json".parse().unwrap());
            let (mut socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
            assert_eq!(
                response.headers().get("sec-websocket-protocol").unwrap(),
                "xrpc.v1.json"
            );
            if wire == Wire::V1 {
                socket
                    .send(Message::Text(
                        json!({"type": "options_update", "payload": {
                            "wantedCollections": ["online.7778777.lessdugin.*"],
                            "wantedDids": [alice.did().as_str()],
                        }})
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
            let mut text = String::new();
            for _ in 0..5 {
                let message =
                    tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                let Message::Text(frame) = message else {
                    panic!("expected text")
                };
                text = frame.to_string();
                if text.contains("online.7778777.lessdugin.post") {
                    break;
                }
            }
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(text.contains("online.7778777.lessdugin.post"), "{text}");
            assert!(value.get("record").is_none());
            let logs = server.requests();
            assert!(logs.iter().any(|r| r.url == path));
            if wire == Wire::V1 {
                assert!(logs.iter().any(|r| r.options.is_some()));
            }
        }
    }

    #[test]
    fn repeated_v2_dids_and_collections_are_not_lost() {
        let params = vec![
            ("dids".into(), "did:plc:a".into()),
            ("dids".into(), "did:plc:b".into()),
            ("collections".into(), "online.7778777.lessdugin.*".into()),
        ];
        let wanted = filters(&params, &Value::Null, Wire::V2);
        assert_eq!(wanted.dids.len(), 2);
        assert_eq!(wanted.collections.len(), 1);
    }
}
