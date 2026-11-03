// SPDX-License-Identifier: AGPL-3.0-or-later
//! The indexer cold path (`sync.md` §14.3) next to the ordinary PDS sync it runs beside: head
//! checks that a sync in flight does not hold up, the membership of a granted author on a
//! hydrated page, the order in which the authors on screen are resolved, and a forum sync that
//! its budget cut short. Offline, over `FakeNet` on a `MemHost`, with a host that can hold
//! chosen requests (a slow PDS) until the test releases them.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use lessdugin_atp::syntax::Did;
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_core::host::{BlobHandle, Host, HostError, HttpRequest, HttpResponse};
use lessdugin_core::store::RecordStore;
use lessdugin_core::sync::{Priority, Reason, Stats, SyncConfig, SyncRequest, Syncer};
use lessdugin_lexicon::collection::nsid;
use lessdugin_lexicon::keys;
use lessdugin_lexicon::records::{Forum, Membership, Post};
use lessdugin_lexicon::values::MembershipMode;
use lessdugin_testkit::fake_pds::{AccountHandle, Fault};
use lessdugin_testkit::repo::tid_at_ms;
use lessdugin_testkit::{FakeNet, MemHost};

const HEAD: &str = "com.atproto.sync.getLatestCommit";

/// Requests held until released: a URL holding every one of `needles` waits.
#[derive(Default)]
struct Hold {
    needles: Mutex<Vec<Vec<String>>>,
    open: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
    held: Mutex<Vec<String>>,
}

impl Hold {
    fn matches(&self, url: &str) -> bool {
        self.needles
            .lock()
            .unwrap()
            .iter()
            .any(|n| n.iter().all(|s| url.contains(s.as_str())))
    }

    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        for w in std::mem::take(&mut *self.wakers.lock().unwrap()) {
            w.wake();
        }
    }
}

/// A `MemHost` whose matching requests wait for [`Hold::release`].
#[derive(Clone)]
struct HoldHost {
    inner: MemHost,
    hold: Arc<Hold>,
}

impl HoldHost {
    fn new(inner: &MemHost) -> Self {
        Self {
            inner: inner.clone(),
            hold: Arc::default(),
        }
    }

    /// Holds the requests whose URL contains every one of `needles` (a DID's method-specific
    /// part, since query strings encode its colons).
    fn hold(&self, needles: &[&str]) {
        self.hold
            .needles
            .lock()
            .unwrap()
            .push(needles.iter().map(|s| (*s).to_owned()).collect());
    }

    fn held(&self) -> Vec<String> {
        self.hold.held.lock().unwrap().clone()
    }
}

impl Host for HoldHost {
    type Kv = <MemHost as Host>::Kv;
    type Ws = <MemHost as Host>::Ws;
    type Lock = <MemHost as Host>::Lock;

    fn cache(&self) -> &Self::Kv {
        self.inner.cache()
    }

    fn state(&self) -> &Self::Kv {
        self.inner.state()
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
        let url = lessdugin_testkit::memhost::target_url(&req.target);
        if self.hold.matches(&url) && !self.hold.open.load(Ordering::SeqCst) {
            self.hold.held.lock().unwrap().push(url);
            std::future::poll_fn(|cx: &mut Context<'_>| {
                self.hold.wakers.lock().unwrap().push(cx.waker().clone());
                if self.hold.open.load(Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        }
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
        self.inner.sleep(ms).await;
    }

    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }

    fn fill_random(&self, buf: &mut [u8]) {
        self.inner.fill_random(buf);
    }
}

/// The method-specific part of a DID, as it appears in a query string.
fn tail(did: &Did) -> &str {
    did.as_str().rsplit(':').next().unwrap()
}

/// Polls `f` once.
fn poll_once<F: Future + Unpin>(f: &mut F) -> Poll<F::Output> {
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    std::pin::Pin::new(f).poll(&mut cx)
}

fn dt(host: &MemHost) -> lessdugin_atp::syntax::Datetime {
    lessdugin_atp::syntax::Datetime::from_millis(host.now() - 3_600_000).unwrap()
}

/// A forum of `owner` in `mode`, the owner a member.
fn forum(host: &MemHost, owner: &AccountHandle, mode: MembershipMode) -> ForumKey {
    let key = ForumKey::new(owner.did(), "meta").unwrap();
    let f = Forum::create("meta", dt(host), mode);
    owner.put(nsid::FORUM, "meta", &serde_json::to_value(f).unwrap());
    join(host, owner, &key);
    key
}

fn join(host: &MemHost, a: &AccountHandle, key: &ForumKey) {
    let m = Membership::new(key.uri.clone(), dt(host));
    a.put(nsid::MEMBERSHIP, &key.fk, &serde_json::to_value(m).unwrap());
}

/// A post by `a` in the forum, `n` ms after an hour ago; its uri.
fn post(host: &MemHost, a: &AccountHandle, key: &ForumKey, title: &str, n: i64) -> String {
    let p = Post::article(key.uri.clone(), title, "Body", dt(host));
    let rkey = keys::post_rkey(
        key.uri.as_str(),
        tid_at_ms(host.now() - 3_600_000 + n).as_ref(),
    )
    .unwrap();
    a.put(nsid::POST, &rkey, &serde_json::to_value(p).unwrap());
    a.uri(nsid::POST, &rkey)
}

/// `sync.md` §14.3 step 4 and §10.6: a head check of an author on screen does not wait for a
/// sync of that author's repo that is in flight. The sync holds the repo for its read-diff-write;
/// the head check's observation is handed to it and recorded with its write, so neither loses
/// the other's state.
#[test]
fn a_head_check_does_not_wait_for_a_sync_in_flight() {
    let mem = MemHost::new();
    let net = FakeNet::on(&mem);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let key = forum(&mem, &owner, MembershipMode::Open);
    post(&mem, &owner, &key, "First", 1);
    let host = HoldHost::new(&mem);
    let engine = Engine::new(mem.clone(), EngineConfig::default());
    let sy = Syncer::new(
        Arc::new(host.clone()),
        SyncConfig::from_engine(engine.config()),
    );
    let s = sy.session(Priority::P1, Arc::new(Stats::capped(None)));
    let did = owner.did().clone();
    let req = SyncRequest::forum(did.clone(), &key.fk, Reason::Participant);
    let first = futures::executor::block_on(sy.sync_repo(&s, &req));
    assert!(first.error.is_none(), "{first:?}");
    let store = RecordStore::new(mem.cache());
    let synced = futures::executor::block_on(store.repo(did.as_str()))
        .unwrap()
        .unwrap();

    // The author posts; the next sync's range listings are slow, and it holds the repo while it
    // waits for them.
    post(&mem, &owner, &key, "Second", 2);
    host.hold(&["com.atproto.repo.listRecords", tail(&did)]);
    let mut sync = Box::pin(sy.sync_repo(&s, &req));
    assert!(poll_once(&mut sync).is_pending());
    assert!(
        !host.held().is_empty(),
        "the sync did not reach its listings"
    );

    // The author writes again meanwhile, and the cold path checks the head at P0. The sync's
    // own re-check of the head, after its listings, then fails: only the head check saw the
    // newest head.
    mem.advance_ms(1_000);
    post(&mem, &owner, &key, "Third", 3);
    let (head_cid, head_rev) = owner.head();
    let checked_at = mem.now();
    net.inject(
        Fault::status(500)
            .on_repo(did.as_str())
            .on_nsid(HEAD)
            .after(1),
    );
    let p0 = sy.session(Priority::P0, Arc::new(Stats::capped(None)));
    let mut check = Box::pin(sy.head_check(&p0, &did, false));
    let checked = match poll_once(&mut check) {
        Poll::Ready(r) => r.unwrap(),
        Poll::Pending => panic!("the head check waited for the sync in flight"),
    };
    // The head moved past the synced one: the repo is queued for a sync.
    assert_eq!(checked, Some(Priority::P1));
    assert_eq!(
        futures::executor::block_on(store.repo(did.as_str()))
            .unwrap()
            .unwrap(),
        synced,
        "the head check wrote the repo row while the sync held it"
    );

    host.hold.release();
    mem.advance_ms(1_000);
    let report = futures::executor::block_on(sync);
    assert!(
        report.error.is_some() || !report.issues.is_empty(),
        "{report:?}"
    );
    let row = futures::executor::block_on(store.repo(did.as_str()))
        .unwrap()
        .unwrap();
    // The sync's own write landed (its failed head re-check counts), and the head check's
    // newer observation is part of it.
    assert_eq!(row.fails, synced.fails + 1, "{row:?}");
    let seen = row.head_seen.expect("no head observation");
    assert_eq!(
        (seen.rev.as_str(), seen.cid, seen.sent_at),
        (head_rev.as_str(), head_cid, checked_at),
        "the head check's observation was lost"
    );
}
