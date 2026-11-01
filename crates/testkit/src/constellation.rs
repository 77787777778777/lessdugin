// SPDX-License-Identifier: AGPL-3.0-or-later
//! A small Constellation index over the committed records of a [`FakeNet`].
//!
//! It answers only `getBacklinks` and `getBacklinksCount` for `NS.membership:forum` and
//! `NS.post:forum`. The results are discovery hints, not verified content: the core still reads
//! and confirms each candidate from its author's PDS. The service never reads pending helper
//! writes, and its short pages have stable, exclusive offset cursors.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lessdugin_atp::syntax::{AtUri, Did, Nsid};
use lessdugin_core::host::Target;
use lessdugin_core::host::{HttpRequest, HttpResponse};
use serde_json::{Value, json};

use super::fake_pds::FakeNet;
use super::memhost::{MemHost, target_url};
use super::resp;

const LINKS: &str = "blue.microcosm.links.getBacklinks";
const COUNT: &str = "blue.microcosm.links.getBacklinksCount";
const DEFAULT_BASE: &str = "https://constellation.example.com";
/// Test-only page size. The discovery client must keep paging even when a page is short.
const DEVNET_PAGE: usize = 1;
const MAX_PAGE: usize = 100;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type RecordKey = (Did, String, String);
type Observation = (u64, u64);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Link {
    did: Did,
    collection: String,
    rkey: String,
    time_us: u64,
    seq: u64,
}

/// A deterministic, local-only Constellation test service.
#[derive(Clone)]
pub struct Constellation {
    net: FakeNet,
    base: Arc<str>,
    seq: Arc<AtomicU64>,
    /// Per-record observation order. Updating a record retains its original event order.
    observed: Arc<Mutex<BTreeMap<RecordKey, Observation>>>,
}

/// An owned loopback Constellation listener for native devnet tests.
#[cfg(feature = "http")]
pub struct HttpConstellation {
    base: String,
    runtime: Option<tokio::runtime::Runtime>,
}

#[cfg(feature = "http")]
impl HttpConstellation {
    /// The base URL to configure in the native devnet profile.
    pub fn base_url(&self) -> &str {
        &self.base
    }
}

#[cfg(feature = "http")]
impl Drop for HttpConstellation {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Constellation {
    /// Install the service at the default test URL on `host`.
    pub fn install(net: &FakeNet, host: &MemHost) -> Self {
        Self::install_at(net, host, DEFAULT_BASE)
    }

    /// Install the service at `base`, on the same in-memory host as `net`.
    pub fn install_at(net: &FakeNet, host: &MemHost, base: &str) -> Self {
        let base = base.trim_end_matches('/');
        url::Url::parse(base).expect("Constellation base URL");
        let this = Self {
            net: net.clone(),
            base: Arc::from(base.to_owned()),
            seq: Arc::new(AtomicU64::new(0)),
            observed: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let list = this.clone();
        host.on_get(&format!("{base}/xrpc/{LINKS}"), move |req| {
            list.answer(req, false)
        });
        let count = this.clone();
        host.on_get(&format!("{base}/xrpc/{COUNT}"), move |req| {
            count.answer(req, true)
        });
        this
    }

    /// Serve the same index over loopback HTTP for native devnet tests. The service is bounded
    /// to one record per page so a successful sync proves short-page cursor handling.
    #[cfg(feature = "http")]
    pub fn serve_http(net: &FakeNet) -> HttpConstellation {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind Constellation");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("fake-constellation")
            .enable_all()
            .build()
            .expect("Constellation runtime");
        let service = Self {
            net: net.clone(),
            base: Arc::from(base.clone()),
            seq: Arc::new(AtomicU64::new(0)),
            observed: Arc::new(Mutex::new(BTreeMap::new())),
        };
        runtime.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
                let service = service.clone();
                async move {
                    let method = req.method().clone();
                    let uri = req.uri().to_string();
                    let request = HttpRequest::get(Target::Anon(
                        url::Url::parse(&format!("http://localhost{uri}")).expect("request URL"),
                    ));
                    if method != http::Method::GET {
                        return axum::response::Response::new(axum::body::Body::from("method"));
                    }
                    let response = service.answer(&request, uri.contains(&format!("/{COUNT}")));
                    match response {
                        Ok(response) => axum::response::Response::builder()
                            .status(response.status)
                            .header("content-type", "application/json")
                            .header("access-control-allow-origin", "*")
                            .body(axum::body::Body::from(response.body))
                            .expect("response"),
                        Err(error) => axum::response::Response::builder()
                            .status(500)
                            .body(axum::body::Body::from(error.to_string()))
                            .expect("error"),
                    }
                }
            });
            let _ = axum::serve(listener, app).await;
        });
        HttpConstellation {
            base,
            runtime: Some(runtime),
        }
    }

    /// The configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Take a consistent snapshot and flush pending writes before indexing it. This is the same
    /// commit barrier the fake PDS uses for requests, not a direct view of uncommitted writes.
    fn snapshot(&self) -> super::fake_pds::BacklinkSnapshot {
        self.net.commit_all();
        self.net.backlink_records()
    }

    fn answer(
        &self,
        req: &HttpRequest,
        count: bool,
    ) -> Result<HttpResponse, lessdugin_core::host::HostError> {
        let url = target_url(&req.target);
        let parsed = match url::Url::parse(&url) {
            Ok(url) => url,
            Err(_) => return Ok(bad_request("bad service URL")),
        };
        let params: BTreeMap<String, String> = parsed.query_pairs().into_owned().collect();
        let Some(subject) = params.get("subject") else {
            return Ok(bad_request("subject is required"));
        };
        let Some(source) = params.get("source") else {
            return Ok(bad_request("source is required"));
        };
        if AtUri::parse(subject).is_err() {
            return Ok(bad_request("subject must be an at-uri"));
        }
        let Some((collection, "forum")) = source.split_once(':') else {
            return Ok(bad_request("only :forum sources are supported"));
        };
        if Nsid::parse(collection).is_err() {
            return Ok(bad_request("invalid source collection"));
        }
        let exact_did = match params.get("did") {
            Some(d) => match Did::parse(d) {
                Ok(d) => Some(d),
                Err(_) => return Ok(bad_request("invalid did")),
            },
            None => None,
        };
        let limit = match params.get("limit") {
            Some(raw) => match raw.parse::<usize>() {
                Ok(n @ 1..=MAX_PAGE) => n,
                _ => return Ok(bad_request("limit must be between 1 and 100")),
            },
            None => MAX_PAGE,
        };
        #[cfg(feature = "http")]
        let limit = limit.min(DEVNET_PAGE);
        let offset = match params.get("cursor") {
            None => 0,
            Some(c) if c.is_empty() => 0,
            Some(c) => match c.strip_prefix("o:").and_then(|n| n.parse::<usize>().ok()) {
                Some(n) => n,
                None => return Ok(bad_request("invalid cursor")),
            },
        };
        let mut links = self.links(subject, collection, exact_did.as_ref());
        let total = links.len();
        if count {
            return Ok(resp::ok_json(&json!({ "total": total })));
        }
        let end = offset.saturating_add(limit).min(links.len());
        let page = if offset < links.len() {
            links.drain(offset..end).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let records = page
            .into_iter()
            .map(|l| {
                json!({
                    "did": l.did,
                    "collection": l.collection,
                    "rkey": l.rkey,
                })
            })
            .collect::<Vec<_>>();
        let cursor = if end < total {
            Value::String(format!("o:{end}"))
        } else {
            Value::Null
        };
        Ok(resp::ok_json(
            &json!({ "records": records, "cursor": cursor }),
        ))
    }

    fn links(&self, subject: &str, collection: &str, exact_did: Option<&Did>) -> Vec<Link> {
        let snapshot = self.snapshot();
        let mut observed = lock(&self.observed);
        let mut links = Vec::new();
        for (did, records) in snapshot {
            if exact_did.is_some_and(|wanted| wanted != &did) {
                continue;
            }
            for (coll, rkey, record) in records {
                if coll != collection
                    || record.get("forum").and_then(Value::as_str) != Some(subject)
                {
                    continue;
                }
                let key = (did.clone(), coll.clone(), rkey.clone());
                let next_seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
                let (time_us, seq) = *observed.entry(key).or_insert((next_seq, next_seq));
                links.push(Link {
                    did: did.clone(),
                    collection: coll,
                    rkey,
                    time_us,
                    seq,
                });
            }
        }
        links.sort_by(|a, b| {
            b.time_us
                .cmp(&a.time_us)
                .then_with(|| b.seq.cmp(&a.seq))
                .then_with(|| a.did.cmp(&b.did))
                .then_with(|| a.rkey.cmp(&b.rkey))
        });
        links
    }
}

fn bad_request(message: &str) -> HttpResponse {
    resp::xrpc_error(400, "InvalidRequest", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lessdugin_core::host::Host;

    async fn get(host: &MemHost, path: &str) -> Value {
        let req = HttpRequest::get(Target::Anon(url::Url::parse(path).unwrap()));
        let response = host.http(req).await.unwrap();
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        serde_json::from_slice(&response.body).unwrap()
    }

    #[tokio::test]
    async fn exact_links_short_pages_and_count() {
        let host = MemHost::new();
        let net = FakeNet::on(&host);
        let owner = net.add_account("owner.test");
        let forum = owner.uri("online.7778777.lessdugin.forum", "self");
        let a = net.add_account("a.test");
        let b = net.add_account("b.test");
        let c = net.add_account("c.test");
        for account in [&a, &b, &c] {
            account.put(
                "online.7778777.lessdugin.membership",
                "forum-3l2s5xxv2zaa2",
                &json!({
                    "forum": forum, "createdAt": "2025-10-09T08:53:20Z"
                }),
            );
            account.head();
        }
        let service = Constellation::install(&net, &host);
        let base = format!(
            "{}/xrpc/{LINKS}?subject={forum}&source=online.7778777.lessdugin.membership:forum",
            service.base_url()
        );
        let first = get(&host, &format!("{base}&limit=1")).await;
        assert_eq!(first["records"].as_array().unwrap().len(), 1);
        assert_eq!(first["cursor"], "o:1");
        let second = get(&host, &format!("{base}&limit=1&cursor=o:1")).await;
        assert_ne!(second["records"][0]["did"], first["records"][0]["did"]);
        let last = get(&host, &format!("{base}&limit=1&cursor=o:2")).await;
        assert_eq!(last["cursor"], Value::Null);
        let count = get(
            &host,
            &format!(
                "{}/xrpc/{COUNT}?subject={forum}&source=online.7778777.lessdugin.membership:forum",
                service.base_url()
            ),
        )
        .await;
        assert_eq!(count["total"], 3);
        let filtered = get(&host, &format!("{base}&did={}", a.did())).await;
        assert_eq!(filtered["records"].as_array().unwrap().len(), 1);
        assert_eq!(filtered["records"][0]["did"], a.did().as_str());
        b.delete("online.7778777.lessdugin.membership", "forum-3l2s5xxv2zaa2");
        b.head();
        let count = get(
            &host,
            &format!(
                "{}/xrpc/{COUNT}?subject={forum}&source=online.7778777.lessdugin.membership:forum",
                service.base_url()
            ),
        )
        .await;
        assert_eq!(count["total"], 2);
    }
}
