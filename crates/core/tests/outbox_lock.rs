// SPDX-License-Identifier: AGPL-3.0-or-later
//! One write in flight per account across engines (`sync.md` §7.1): two outboxes over one host
//! (two engines, or two processes on one data directory) pump the same account's writes
//! concurrently. `Host::lock("outbox:<did>")` keeps every write's send exclusive, so no write is
//! picked by both and no two are in flight at once. A control run with a host whose lock does
//! not exclude shows the test catches the race.

mod common;

use std::sync::{Arc, Mutex};
use std::task::Poll;

use common::block_on;
use lessdugin_atp::syntax::{AtUri, Datetime, Did};
use lessdugin_core::dto::{WriteState, WriteTicket};
use lessdugin_core::host::{
    BlobHandle, Host, HostError, HttpRequest, HttpResponse, ProcessLock, ProcessLocks,
};
use lessdugin_core::outbox::{ContentOpts, Outbox, OutboxListener, Session};
use lessdugin_core::store::mem::MemKv;
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::records::Post;
use lessdugin_testkit::{FakeNet, MemHost};

/// A `MemHost` whose requests yield once, so concurrent pumps interleave at every request.
/// With `exclusive: false` its lock never excludes (the control).
struct YieldHost {
    inner: MemHost,
    exclusive: bool,
}

async fn yield_once() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

impl Host for YieldHost {
    type Kv = MemKv;
    type Ws = <MemHost as Host>::Ws;
    type Lock = ProcessLock;

    fn cache(&self) -> &MemKv {
        self.inner.cache()
    }

    fn state(&self) -> &MemKv {
        self.inner.state()
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        yield_once().await;
        self.inner.http(req).await
    }

    async fn fetch_blob(
        &self,
        pds: url::Url,
        did: Did,
        cid: String,
        max_bytes: u64,
    ) -> Result<BlobHandle, HostError> {
        self.inner.fetch_blob(pds, did, cid, max_bytes).await
    }

    async fn websocket(&self, url: url::Url, max: u32) -> Result<Self::Ws, HostError> {
        self.inner.websocket(url, max).await
    }

    async fn resolve_txt_system(&self, name: String) -> Result<Option<Vec<String>>, HostError> {
        self.inner.resolve_txt_system(name).await
    }

    async fn lock(&self, name: &str, wait: bool) -> Result<Option<ProcessLock>, HostError> {
        if self.exclusive {
            self.inner.lock(name, wait).await
        } else {
            Ok(ProcessLocks::new().lock(name, wait).await)
        }
    }

    async fn sleep(&self, ms: u64) {
        self.inner.sleep(ms).await;
    }

    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }

    fn fill_random(&self, buf: &mut [u8]) {
        self.inner.fill_random(buf);
    }
}

/// Write states as both outboxes report them, in order.
struct Log(Arc<Mutex<Vec<(String, WriteState)>>>);

impl OutboxListener for Log {
    fn on_write(&self, t: &WriteTicket) {
        self.0.lock().unwrap().push((t.id.clone(), t.state.clone()));
    }
}

/// Runs two outboxes over one host on the same account's six writes; returns the merged log.
fn race(exclusive: bool) -> Vec<(String, WriteState)> {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let alice = net.add_account("alice.example.com");
    let bob = net.add_account("bob.example.com");
    let session = Session::parse(alice.did().as_str(), &alice.pds_base()).unwrap();
    let forum = AtUri::parse(&format!("at://{}/{}/meta", bob.did(), nsid::FORUM)).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let outbox = || {
        Outbox::with_listener(
            Arc::new(YieldHost {
                inner: host.clone(),
                exclusive,
            }),
            Arc::new(Log(Arc::clone(&log))),
        )
    };
    let (a, b) = (outbox(), outbox());
    let now = Datetime::from_millis(host.now()).unwrap();
    for (i, ob) in [&a, &b, &a, &b, &a, &b].into_iter().enumerate() {
        let post = Post::article(forum.clone(), format!("post {i}"), "body", now.clone());
        block_on(ob.create_post(&session, &post, ContentOpts::default())).unwrap();
    }
    log.lock().unwrap().clear();
    let (ra, rb) = block_on(futures::future::join(
        a.run_until_idle(&session),
        b.run_until_idle(&session),
    ));
    ra.unwrap();
    rb.unwrap();
    log.lock().unwrap().clone()
}

/// The writes in flight at once (most), and the writes picked for sending more than once.
fn check(log: &[(String, WriteState)]) -> (usize, Vec<String>) {
    let mut in_flight: Vec<&str> = Vec::new();
    let mut most = 0;
    let mut sent: Vec<&str> = Vec::new();
    let mut twice = Vec::new();
    for (id, st) in log {
        match st {
            WriteState::Sending => {
                if sent.contains(&id.as_str()) {
                    twice.push(id.clone());
                }
                sent.push(id);
                in_flight.push(id);
                most = most.max(in_flight.len());
            }
            _ => in_flight.retain(|x| *x != id.as_str()),
        }
    }
    (most, twice)
}

#[test]
fn one_write_in_flight_per_account_across_outboxes() {
    let log = race(true);
    let (most, twice) = check(&log);
    assert_eq!(most, 1, "{log:#?}");
    assert!(twice.is_empty(), "{twice:?}");
    let done = log
        .iter()
        .filter(|(_, s)| matches!(s, WriteState::Done { .. }))
        .count();
    assert_eq!(done, 6, "{log:#?}");
}

#[test]
fn without_the_host_lock_the_outboxes_race() {
    // the control: a lock that does not exclude lets both outboxes pick the same write
    let log = race(false);
    let (most, twice) = check(&log);
    assert!(most > 1 || !twice.is_empty(), "{log:#?}");
}
