// SPDX-License-Identifier: AGPL-3.0-or-later
//! A native host wrapper which enforces the indexer's outbound politeness budget on every HTTP
//! request, including requests made inside core identity resolution and synchronisation.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{BlobHandle, Host, HostError, HttpRequest, HttpResponse, Target};
use lessdugin_native::host::{NativeHost, NativeLock, NativeWs};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

struct HostBudget {
    in_flight: Arc<Semaphore>,
    sent: Mutex<VecDeque<Instant>>,
}

/// Native host with at most five requests/second and four in flight per origin, and 200 in
/// flight globally. Blob methods deliberately remain unsupported: the indexer never stores blobs.
#[derive(Clone)]
pub struct PoliteHost {
    inner: NativeHost,
    global: Arc<Semaphore>,
    hosts: Arc<Mutex<HashMap<String, Arc<HostBudget>>>>,
}

impl PoliteHost {
    pub fn new(inner: NativeHost) -> Self {
        Self {
            inner,
            global: Arc::new(Semaphore::new(200)),
            hosts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn permit(
        &self,
        target: &Target,
    ) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), HostError> {
        let origin = match target {
            Target::Anon(url) => url.origin().ascii_serialization(),
            Target::UserPds { account, .. } => format!("userpds:{account}"),
        };
        let host = {
            let mut hosts = self.hosts.lock().await;
            Arc::clone(hosts.entry(origin).or_insert_with(|| {
                Arc::new(HostBudget {
                    in_flight: Arc::new(Semaphore::new(4)),
                    sent: Mutex::new(VecDeque::new()),
                })
            }))
        };
        // The per-origin permit and rate slot come first: requests queued on one slow origin
        // (PLC, a hanging PDS) must not hold global permits that every other origin needs.
        let local = Arc::clone(&host.in_flight)
            .acquire_owned()
            .await
            .map_err(|_| HostError::Other("indexer shut down".into()))?;
        loop {
            let wait = {
                let mut sent = host.sent.lock().await;
                let now = Instant::now();
                while sent
                    .front()
                    .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1))
                {
                    sent.pop_front();
                }
                if sent.len() < 5 {
                    sent.push_back(now);
                    None
                } else {
                    sent.front()
                        .map(|t| Duration::from_secs(1).saturating_sub(now.duration_since(*t)))
                }
            };
            if let Some(wait) = wait {
                tokio::time::sleep(wait).await;
            } else {
                break;
            }
        }
        let global = Arc::clone(&self.global)
            .acquire_owned()
            .await
            .map_err(|_| HostError::Other("indexer shut down".into()))?;
        Ok((global, local))
    }
}

impl Host for PoliteHost {
    type Kv = lessdugin_native::kv::SqliteKv;
    type Ws = NativeWs;
    type Lock = NativeLock;

    fn cache(&self) -> &Self::Kv {
        self.inner.cache()
    }
    fn state(&self) -> &Self::Kv {
        self.inner.state()
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        let _permits = self.permit(&req.target).await?;
        self.inner.http(req).await
    }

    async fn fetch_blob(
        &self,
        _pds: url::Url,
        _did: Did,
        _cid: String,
        _max_bytes: u64,
    ) -> Result<BlobHandle, HostError> {
        Err(HostError::Unsupported(
            "the indexer does not fetch or store blobs".into(),
        ))
    }

    async fn websocket(
        &self,
        url: url::Url,
        max_message_bytes: u32,
    ) -> Result<Self::Ws, HostError> {
        self.inner.websocket(url, max_message_bytes).await
    }

    async fn resolve_txt_system(&self, name: String) -> Result<Option<Vec<String>>, HostError> {
        self.inner.resolve_txt_system(name).await
    }

    async fn lock(&self, name: &str, wait: bool) -> Result<Option<Self::Lock>, HostError> {
        self.inner.lock(name, wait).await
    }

    async fn sleep(&self, ms: u64) {
        self.inner.sleep(ms).await
    }
    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }
    fn fill_random(&self, buf: &mut [u8]) {
        self.inner.fill_random(buf)
    }
}
