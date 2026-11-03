// SPDX-License-Identifier: AGPL-3.0-or-later
//! The indexer cold path (`sync.md` §14.3) next to the ordinary PDS sync it runs beside: head
//! checks that a sync in flight does not hold up, the membership of a granted author on a
//! hydrated page, the order in which the authors on screen are resolved, a proof admitted while
//! a sync of its author commits, a forum sync that its budget cut short, and a forum sync
//! started beside a cold page, which goes after it for a bounded time. Offline, over
//! `FakeNet` on a `MemHost`, with a host that can hold chosen requests (a slow PDS) and a chosen
//! write to its cache (a slow store) until the test releases them.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use lessdugin_atp::syntax::{AtUri, Did};
use lessdugin_core::discovery::ForumKey;
use lessdugin_core::engine::{Engine, EngineConfig};
use lessdugin_core::host::{
    BlobHandle, Dir, Host, HostError, HttpRequest, HttpResponse, KeyRange, KvOp, KvStore,
};
use lessdugin_core::store::mem::MemKv;
use lessdugin_core::store::{RecKey, RecordStore};
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

/// A `MemKv` whose next write of a chosen uri's slot waits for [`HoldKv::release`]: a store
/// that is slow to take a batch's commit (IndexedDB and SQLite answer asynchronously).
#[derive(Clone)]
struct HoldKv {
    inner: MemKv,
    hold: Arc<KvHold>,
}

#[derive(Default)]
struct KvHold {
    /// The keys whose next write waits (that write takes them).
    keys: Mutex<Vec<Vec<u8>>>,
    held: AtomicBool,
    open: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl HoldKv {
    fn new(inner: &MemKv) -> Self {
        Self {
            inner: inner.clone(),
            hold: Arc::default(),
        }
    }

    /// Holds the next write of `uri`'s record or tombstone.
    fn hold_next_write(&self, uri: &AtUri) {
        let k = RecKey::of_uri(uri);
        *self.hold.keys.lock().unwrap() = vec![k.rec(), k.tomb()];
    }

    fn held(&self) -> bool {
        self.hold.held.load(Ordering::SeqCst)
    }

    fn release(&self) {
        self.hold.open.store(true, Ordering::SeqCst);
        for w in std::mem::take(&mut *self.hold.wakers.lock().unwrap()) {
            w.wake();
        }
    }
}

impl KvStore for HoldKv {
    async fn get_many(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, HostError> {
        self.inner.get_many(keys).await
    }

    async fn scan(
        &self,
        range: KeyRange,
        dir: Dir,
        limit: u32,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, HostError> {
        self.inner.scan(range, dir, limit).await
    }

    async fn write(&self, batch: Vec<KvOp>) -> Result<(), HostError> {
        let hit = {
            let mut keys = self.hold.keys.lock().unwrap();
            let hit = batch
                .iter()
                .any(|op| matches!(op, KvOp::Put(k, _) | KvOp::Delete(k) if keys.contains(k)));
            if hit {
                keys.clear();
            }
            hit
        };
        if hit {
            self.hold.held.store(true, Ordering::SeqCst);
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
        self.inner.write(batch).await
    }
}

/// A `MemHost` whose matching requests wait for [`Hold::release`], over a cache that can hold a
/// write ([`HoldKv`]).
#[derive(Clone)]
struct HoldHost {
    inner: MemHost,
    hold: Arc<Hold>,
    cache: HoldKv,
    state: HoldKv,
}

impl HoldHost {
    fn new(inner: &MemHost) -> Self {
        Self {
            inner: inner.clone(),
            hold: Arc::default(),
            cache: HoldKv::new(inner.cache_kv()),
            state: HoldKv::new(inner.state_kv()),
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
    type Kv = HoldKv;
    type Ws = <MemHost as Host>::Ws;
    type Lock = <MemHost as Host>::Lock;

    fn cache(&self) -> &Self::Kv {
        &self.cache
    }

    fn state(&self) -> &Self::Kv {
        &self.state
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

const INDEX: &str = "https://index.example.com";

fn endpoint(method: &str) -> String {
    format!(
        "{INDEX}/xrpc/{}",
        lessdugin_core::indexer::contract::method_nsid(method)
    )
}

fn as_of() -> serde_json::Value {
    serde_json::json!({
        "asOf": {"seq": 0, "timeUs": lessdugin_testkit::memhost::DEFAULT_NOW_MS * 1000, "lagMs": 0},
        "indexer": {"did": "did:web:index.example.com", "coreVersion": "test"}
    })
}

/// A consented indexer at [`INDEX`] serving `listPosts` with `posts` (uris, in order) and
/// `getRecordProofs` with proofs fetched from the authors' PDSes, as a real indexer caches them.
fn serve_indexer(mem: &MemHost, net: &FakeNet, posts: &[String]) {
    let mut describe = serde_json::json!({
        "did": "did:web:index.example.com", "version": "1", "coreVersion": "test",
        "policies": [], "features": ["listPosts", "getRecordProofs"], "forums": []
    });
    describe
        .as_object_mut()
        .unwrap()
        .extend(as_of().as_object().unwrap().clone());
    mem.respond_get(
        &endpoint("describeIndexer"),
        lessdugin_testkit::resp::ok_json(&describe),
    );
    let mut page = serde_json::json!({
        "posts": posts.iter().map(|u| serde_json::json!({
            "ref": {"uri": u}, "effectiveAt": "2025-01-01T00:00:00Z", "state": "visible"
        })).collect::<Vec<_>>(),
        "policy": {"karma": "karma/1", "ranking": "magic/1"}, "inputDigest": "00"
    });
    page.as_object_mut()
        .unwrap()
        .extend(as_of().as_object().unwrap().clone());
    mem.respond_get(
        &endpoint("listPosts"),
        lessdugin_testkit::resp::ok_json(&page),
    );
    let net = net.clone();
    mem.on_get(&endpoint("getRecordProofs"), move |req| {
        let url = url::Url::parse(&lessdugin_testkit::memhost::target_url(&req.target)).unwrap();
        let uris: Vec<String> = url
            .query_pairs()
            .filter(|(k, _)| k == "uris")
            .map(|(_, v)| v.into_owned())
            .collect();
        Ok(lessdugin_testkit::resp::car(proof_bundle(&net, &uris)))
    });
}

/// A `getRecordProofs` bundle (`sync.md` §8.6) of the records' current proofs at their PDSes.
fn proof_bundle(net: &FakeNet, uris: &[String]) -> Vec<u8> {
    use lessdugin_atp::Ipld;
    use std::collections::BTreeMap;
    let mut items = Vec::new();
    let mut blocks = BTreeMap::new();
    for u in uris {
        let uri = AtUri::parse(u).unwrap();
        let account = net.account(uri.did().as_str()).unwrap();
        let qs = lessdugin_atp::xrpc::query_string(&[
            ("did", uri.did().as_str()),
            ("collection", uri.collection().as_str()),
            ("rkey", uri.rkey().as_str()),
        ]);
        let url = format!(
            "{}/xrpc/com.atproto.sync.getRecord?{qs}",
            account.pds_base()
        );
        let req = HttpRequest::get(lessdugin_core::host::Target::Anon(
            url::Url::parse(&url).unwrap(),
        ));
        let resp = net.request(&req, "indexer").unwrap();
        assert_eq!(resp.status, 200, "no proof for {u}");
        let car = lessdugin_atp::car::read_car(&resp.body, &lessdugin_atp::car::CarLimits::PROOF)
            .unwrap();
        let mut item = BTreeMap::new();
        item.insert("uri".to_owned(), Ipld::String(u.clone()));
        item.insert(
            "cid".to_owned(),
            account
                .cid(uri.collection().as_str(), uri.rkey().as_str())
                .map_or(Ipld::Null, Ipld::Link),
        );
        item.insert("commit".to_owned(), Ipld::Link(car.roots[0]));
        items.push(Ipld::Map(item));
        blocks.extend(car.blocks);
    }
    let mut manifest = BTreeMap::new();
    manifest.insert("v".to_owned(), Ipld::Integer(1));
    manifest.insert("items".to_owned(), Ipld::List(items));
    let manifest = lessdugin_atp::dagcbor::encode(&Ipld::Map(manifest)).unwrap();
    let root = lessdugin_atp::cid::for_dag_cbor(&manifest);
    blocks.insert(root, manifest);
    lessdugin_atp::car::write_car(&[root], blocks.iter().map(|(c, b)| (c, b.as_slice())))
}

fn assisted<H: Host>(host: H) -> Engine<H> {
    Engine::new(
        host,
        EngineConfig {
            indexers: vec![lessdugin_core::engine::IndexerConsent {
                url: url::Url::parse(INDEX).unwrap(),
            }],
            ..EngineConfig::default()
        },
    )
}

/// A grant of `#access member` to `member`, by the owner.
fn grant(host: &MemHost, owner: &AccountHandle, key: &ForumKey, member: &Did) {
    use lessdugin_lexicon::records::{AccessAction, Action, ModAction};
    let m = ModAction::new(
        key.uri.clone(),
        member.as_str(),
        Action::Access(AccessAction::new(
            lessdugin_lexicon::values::AccessLevel::Member,
        )),
        dt(host),
    );
    let r = lessdugin_core::outbox::records::mod_action(&m).unwrap();
    owner.put(nsid::MOD_ACTION, &r.rkey, &r.value);
}

fn all_posts(forum: &ForumKey) -> lessdugin_core::dto::ListPostsQuery {
    let mut q = lessdugin_core::dto::ListPostsQuery::new(forum.uri.to_string());
    q.sort = lessdugin_core::dto::PostSort::New;
    q.filter = lessdugin_core::dto::PostFilter::All;
    q
}

/// `social.md` §3.1 makes an author a participant only once their membership record is held,
/// and §4.2.1 reads a granted member's membership only inside their repo sync. The cold page
/// (`sync.md` §14.3 step 4) therefore reads it itself, at the member's PDS: a granted member's
/// post hydrated from the indexer is listed while a sync of the member's repo is held at its
/// listings, without waiting for it (§10.6).
#[test]
fn a_granted_members_hydrated_post_is_listed_before_their_repo_sync() {
    let mem = MemHost::new();
    let net = FakeNet::on(&mem);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let key = forum(&mem, &owner, MembershipMode::Approval);
    let member = net.add_account("member.example.com");
    join(&mem, &member, &key);
    grant(&mem, &owner, &key, member.did());
    let mine = post(&mem, &owner, &key, "Owner's post", 1);
    let theirs = post(&mem, &member, &key, "Member's post", 2);
    serve_indexer(&mem, &net, &[theirs.clone(), mine.clone()]);
    let host = HoldHost::new(&mem);
    let engine = assisted(host.clone());
    futures::executor::block_on(engine.forum_open(key.uri.as_str())).unwrap();
    // The page's forum sync is listing the member's repo, and its listings are slow.
    host.hold(&["com.atproto.repo.listRecords", tail(member.did())]);
    let sy = engine.syncer();
    let s = sy.session(Priority::P1, Arc::new(Stats::capped(None)));
    let req = SyncRequest::forum(member.did().clone(), &key.fk, Reason::Participant);
    let mut sync = Box::pin(sy.sync_repo(&s, &req));
    assert!(poll_once(&mut sync).is_pending());
    assert!(
        !host.held().is_empty(),
        "the sync did not reach its listings"
    );

    let query = all_posts(&key);
    let fallback = lessdugin_core::dto::SyncOpts::default();
    let mut page = Box::pin(engine.list_posts_page(&query, &fallback));
    let page = (0..1_000)
        .find_map(|_| match poll_once(&mut page) {
            Poll::Ready(r) => Some(r.unwrap()),
            Poll::Pending => None,
        })
        .expect("the cold page waited for the member's sync");
    let listed: Vec<&str> = page.items.iter().map(|p| p.uri.as_str()).collect();
    assert_eq!(listed, [theirs.as_str(), mine.as_str()], "{page:?}");
    assert_eq!(page.items[0].title.as_deref(), Some("Member's post"));
    // The membership was read at the member's PDS; the member's repo is still not listed.
    let membership = AtUri::parse(&member.uri(nsid::MEMBERSHIP, &key.fk)).unwrap();
    let row = futures::executor::block_on(RecordStore::new(mem.cache()).get(&membership))
        .unwrap()
        .expect("the membership is not held");
    assert_eq!(row.level, lessdugin_core::store::Level::Pds, "{row:?}");
    assert!(
        futures::executor::block_on(RecordStore::new(mem.cache()).repo(member.did().as_str()))
            .unwrap()
            .is_none_or(|r| r.listed.is_empty()),
        "the member's repo was listed"
    );

    host.hold.release();
    let report = futures::executor::block_on(sync);
    assert!(report.error.is_none(), "{report:?}");
}

/// `sync.md` §10.6: the cold page admits a proof without the repo lock, while a sync of the
/// author is between the reads of its batch and its commit (a slow store). Each holds the write
/// lock of the author's record slots for its read-diff-write, so the two never interleave: the
/// slot ends as their merge (the newer of the record and the tombstone by order key, with the
/// uri's earliest first-seen time), not as the stale state of whichever committed last.
#[test]
fn a_proof_admitted_while_a_sync_commits_is_merged_with_it() {
    let mem = MemHost::new();
    let net = FakeNet::on(&mem);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let key = forum(&mem, &owner, MembershipMode::Open);
    let member = net.add_account("member.example.com");
    join(&mem, &member, &key);
    let doomed = post(&mem, &member, &key, "Doomed", 1);
    let uri = AtUri::parse(&doomed).unwrap();
    let rkey = uri.rkey().as_str();
    let cid = member.cid(nsid::POST, rkey).unwrap();
    serve_indexer(&mem, &net, std::slice::from_ref(&doomed));
    let host = HoldHost::new(&mem);
    let engine = assisted(host.clone());
    futures::executor::block_on(engine.forum_open(key.uri.as_str())).unwrap();
    let store = RecordStore::new(mem.cache());
    assert_eq!(futures::executor::block_on(store.get(&uri)).unwrap(), None);

    // A sync of the member lists the post, and the store is slow to take its batch.
    host.cache.hold_next_write(&uri);
    let sy = engine.syncer();
    let s = sy.session(Priority::P1, Arc::new(Stats::capped(None)));
    let req = SyncRequest::forum(member.did().clone(), &key.fk, Reason::Participant);
    let mut sync = Box::pin(sy.sync_repo(&s, &req));
    assert!(poll_once(&mut sync).is_pending());
    assert!(host.cache.held(), "the sync did not reach its commit");
    let listed_at = mem.now();

    // The member deletes the post meanwhile, and the cold page admits its proof of absence.
    mem.advance_ms(1_000);
    assert!(member.delete(nsid::POST, rkey));
    let (_, deleted) = member.head();
    let uris = [uri.clone()];
    let mut page = Box::pin(engine.hydrate_indexer_records(key.uri.as_str(), &uris));
    // (under the write lock it waits here for the sync's commit; without it, it finishes and the
    // sync's batch, read before it, commits after it)
    let hydrated = (0..100).find_map(|_| match poll_once(&mut page) {
        Poll::Ready(r) => Some(r),
        Poll::Pending => None,
    });
    host.cache.release();
    let report = futures::executor::block_on(sync);
    assert!(report.error.is_none(), "{report:?}");
    let proofs = hydrated
        .unwrap_or_else(|| futures::executor::block_on(page))
        .unwrap()
        .expect("the indexer was not used");
    assert_eq!(proofs.absent, 1, "{proofs:?}");

    assert_eq!(
        futures::executor::block_on(store.get(&uri))
            .unwrap()
            .map(|r| r.cid),
        None,
        "the sync's stale batch put the deleted post back beside its tombstone"
    );
    let tomb = futures::executor::block_on(store.tombstone_of(&uri))
        .unwrap()
        .expect("the post is not tombstoned");
    assert_eq!(tomb.rev_lo, deleted.as_str(), "{tomb:?}");
    assert_eq!(
        (tomb.first_cid, tomb.first_seen_uri),
        (Some(cid), Some(listed_at * 1000)),
        "the post's first-seen time was lost: {tomb:?}"
    );
}

/// `sync.md` §14.3 step 4 and §3.5: the identities and heads of the authors on screen are
/// resolved at P0, ahead of the forum sync's participant sweep that runs beside the cold page,
/// though the sweep reaches them last (in DID order) and started first. Deterministic: the fake
/// clock moves only when the test moves it, and only the order of requests is checked.
#[test]
fn the_authors_on_screen_are_resolved_ahead_of_the_participant_sweep() {
    // more members than PLC pacing admits in one second (20, O-16), so the sweep is still
    // waiting for its next slots when the page starts
    const MEMBERS: usize = 50;
    const ON_SCREEN: usize = 6;
    let mem = MemHost::new();
    let net = FakeNet::on(&mem);
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let key = forum(&mem, &owner, MembershipMode::Approval);
    let mut members: Vec<AccountHandle> = (0..MEMBERS)
        .map(|i| net.add_account(&format!("member-{i:02}.example.com")))
        .collect();
    members.sort_by(|a, b| a.did().as_str().cmp(b.did().as_str()));
    let mut posts = Vec::new();
    for (i, m) in members.iter().enumerate() {
        join(&mem, m, &key);
        grant(&mem, &owner, &key, m.did());
        posts.push(post(&mem, m, &key, &format!("Post {i}"), i as i64 + 1));
    }
    // The page shows the posts of the members the sweep reaches last.
    let on_screen: Vec<String> = members[MEMBERS - ON_SCREEN..]
        .iter()
        .map(|m| m.did().to_string())
        .collect();
    let page_posts: Vec<String> = posts[MEMBERS - ON_SCREEN..].iter().rev().cloned().collect();
    serve_indexer(&mem, &net, &page_posts);
    let engine = assisted(mem.clone());
    futures::executor::block_on(engine.forum_open(key.uri.as_str())).unwrap();
    mem.clear_requests();

    let plc = net.plc();
    let plc_dids = |from: usize| -> Vec<(usize, String)> {
        mem.requests()
            .into_iter()
            .enumerate()
            .skip(from)
            .filter_map(|(i, r)| {
                r.url
                    .strip_prefix(&format!("{plc}/"))
                    .map(|d| (i, d.to_owned()))
            })
            .collect()
    };
    mem.park_sleeps(true);
    // The page's forum sync (as the SPA starts it) is resolving participants…
    let opts = lessdugin_core::dto::SyncOpts {
        budget_ms: Some(8_000),
        max_requests: Some(400),
        full: false,
    };
    let mut sweep = Box::pin(engine.sync_forum(key.uri.as_str(), &opts));
    let mut sweep_done = None;
    for _ in 0..1_000 {
        if plc_dids(0).len() >= 2 {
            break;
        }
        if let Poll::Ready(r) = poll_once(&mut sweep) {
            sweep_done = Some((mem.now(), r));
            break;
        }
        mem.advance_ms(10);
    }
    assert!(
        sweep_done.is_none(),
        "the sweep ended before the page started"
    );
    assert!(
        plc_dids(0).iter().all(|(_, d)| !on_screen.contains(d)),
        "the sweep reached an author on screen first"
    );
    // … when the cold page lists its posts.
    let started = mem.requests().len();
    let query = all_posts(&key);
    let fallback = lessdugin_core::dto::SyncOpts::default();
    let mut page = Box::pin(engine.list_posts_page(&query, &fallback));
    let mut page_done = None;
    for _ in 0..10_000 {
        if page_done.is_none()
            && let Poll::Ready(r) = poll_once(&mut page)
        {
            page_done = Some((mem.now(), r));
        }
        if sweep_done.is_none()
            && let Poll::Ready(r) = poll_once(&mut sweep)
        {
            sweep_done = Some((mem.now(), r));
        }
        if page_done.is_some() && sweep_done.is_some() {
            break;
        }
        mem.advance_ms(10);
    }
    mem.park_sleeps(false);
    let (page_at, page) = page_done.expect("the cold page did not finish");
    let (sweep_at, sweep) = sweep_done.expect("the sweep did not finish");
    let page = page.unwrap();
    sweep.unwrap();
    // (posts of the members the sweep has synced by then are listed too)
    let listed: Vec<&str> = page.items.iter().map(|p| p.uri.as_str()).collect();
    for p in &page_posts {
        assert!(
            listed.contains(&p.as_str()),
            "{p} not on the cold page: {listed:?}"
        );
    }
    assert!(
        page_at < sweep_at,
        "the cold page ({page_at}) waited for the sweep ({sweep_at})"
    );

    // Once the page asked: every document request goes to an author on screen until each has
    // its document, and each of their heads is checked before the sweep resolves anyone else.
    // One document per author (at most 1 h old) serves both its head check and its proofs. (The
    // sweep's own requests for these authors, queued earlier, come after its other ones.)
    let after = plc_dids(started);
    let next_sweep_doc = after
        .iter()
        .find(|(_, d)| !on_screen.contains(d))
        .map_or(usize::MAX, |(i, _)| *i);
    for did in &on_screen {
        let n = after
            .iter()
            .filter(|(i, d)| d == did && *i < next_sweep_doc)
            .count();
        assert_eq!(
            n, 1,
            "{did}: {n} documents before the sweep's next: {after:?}"
        );
    }
    let requests = mem.requests();
    for did in &on_screen {
        let head = requests
            .iter()
            .enumerate()
            .skip(started)
            .position(|(_, r)| {
                r.url.contains(HEAD) && r.url.contains(did.rsplit(':').next().unwrap())
            });
        let head = head
            .map(|p| p + started)
            .expect("an author on screen was not head-checked");
        assert!(
            head < next_sweep_doc,
            "{did}: head check at {head}, the sweep's next document at {next_sweep_doc}"
        );
    }
}

/// `sync.md` §5.2 and §6.1: a page's forum sync that its time budget cut short (a participant's
/// range listings held past it, as a slow PDS does) leaves that repo's unfinished pass to the
/// dirty queue with the forum's scope, and `live_tail` finishes it: the page converges without
/// another explicit sync, also for a forum the viewer does not follow.
#[test]
fn a_forum_sync_cut_short_by_its_budget_converges_in_the_background() {
    futures::executor::block_on(async {
        let mem = MemHost::new();
        let net = FakeNet::on(&mem);
        let _ = net.default_pds();
        let owner = net.add_account("owner.example.com");
        let key = forum(&mem, &owner, MembershipMode::Approval);
        let member = net.add_account("member.example.com");
        join(&mem, &member, &key);
        grant(&mem, &owner, &key, member.did());
        let mine = post(&mem, &owner, &key, "Owner's post", 1);
        let theirs = post(&mem, &member, &key, "Member's post", 2);
        let engine = Engine::new(
            mem.clone(),
            EngineConfig {
                sources: lessdugin_core::engine::Sources::Direct,
                ..EngineConfig::default()
            },
        );
        engine.forum_open(key.uri.as_str()).await.unwrap();
        // The member's listings answer only after the page's sync budget (8 s) is spent.
        let held = net.inject(
            Fault::latency(20_000)
                .on_repo(member.did().as_str())
                .on_nsid("com.atproto.repo.listRecords"),
        );
        let opts = lessdugin_core::dto::SyncOpts {
            budget_ms: Some(8_000),
            max_requests: Some(400),
            full: false,
        };
        let report = engine.sync_forum(key.uri.as_str(), &opts).await.unwrap();
        assert!(report.partial, "{report:?}");
        let listed = |page: &lessdugin_core::dto::PostList| -> Vec<String> {
            page.items.iter().map(|p| p.uri.clone()).collect()
        };
        let page = engine.list_posts(&all_posts(&key)).await.unwrap();
        assert_eq!(listed(&page), std::slice::from_ref(&mine), "{page:?}");

        // The PDS answers again. Nothing but live_tail's rounds run from here.
        net.remove_fault(held);
        for _ in 0..20 {
            let round = engine.live_step().await.unwrap();
            mem.advance_ms(round.next_ms.max(1_000));
        }
        let page = engine.list_posts(&all_posts(&key)).await.unwrap();
        assert!(
            listed(&page).contains(&theirs),
            "the forum did not converge: {page:?}"
        );
        assert!(!page.completeness.partial, "{:?}", page.completeness);
    });
}

/// An approval forum of `members` granted members, each with one post, served by a consented
/// indexer whose first page shows the posts of the last `on_screen` members in DID order (the
/// ones a participant sweep reaches last). Returns the page's posts and the members not on it.
fn crowded_forum(
    mem: &MemHost,
    net: &FakeNet,
    members: usize,
    on_screen: usize,
) -> (ForumKey, Vec<String>, Vec<Did>) {
    let _ = net.default_pds();
    let owner = net.add_account("owner.example.com");
    let key = forum(mem, &owner, MembershipMode::Approval);
    let mut accounts: Vec<AccountHandle> = (0..members)
        .map(|i| net.add_account(&format!("member-{i:02}.example.com")))
        .collect();
    accounts.sort_by(|a, b| a.did().as_str().cmp(b.did().as_str()));
    let mut posts = Vec::new();
    for (i, m) in accounts.iter().enumerate() {
        join(mem, m, &key);
        grant(mem, &owner, &key, m.did());
        posts.push(post(mem, m, &key, &format!("Post {i}"), i as i64 + 1));
    }
    let page: Vec<String> = posts[members - on_screen..].iter().rev().cloned().collect();
    serve_indexer(mem, net, &page);
    let off_screen = accounts[..members - on_screen]
        .iter()
        .map(|a| a.did().clone())
        .collect();
    (key, page, off_screen)
}

/// Whether a request so far names one of `dids` (a document, a head check, a listing).
fn names_any(mem: &MemHost, dids: &[Did]) -> bool {
    mem.requests()
        .iter()
        .any(|r| dids.iter().any(|d| r.url.contains(tail(d))))
}

/// The posts of `page` missing from `list`.
fn unlisted<'a>(list: &lessdugin_core::dto::PostList, page: &'a [String]) -> Vec<&'a String> {
    page.iter()
        .filter(|p| !list.items.iter().any(|i| &i.uri == *p))
        .collect()
}

/// [`crowded_forum`] opened by an assisted engine whose host holds the indexer's proofs (a
/// slow indexer) until the test releases them: `(host, engine, forum, page posts, members not
/// on the page)`.
fn page_then_sync(mem: &MemHost) -> (HoldHost, Engine<HoldHost>, ForumKey, Vec<String>, Vec<Did>) {
    let net = FakeNet::on(mem);
    let (key, page, off_screen) = crowded_forum(mem, &net, 30, 6);
    let host = HoldHost::new(mem);
    let engine = assisted(host.clone());
    futures::executor::block_on(engine.forum_open(key.uri.as_str())).unwrap();
    mem.clear_requests();
    host.hold(&["getRecordProofs"]);
    (host, engine, key, page, off_screen)
}

const PAGE_SYNC: lessdugin_core::dto::SyncOpts = lessdugin_core::dto::SyncOpts {
    budget_ms: Some(8_000),
    max_requests: Some(400),
    full: false,
};

/// `sync.md` §14.3 step 4: the forum sync that the SPA starts beside the cold page it is
/// reading goes after the page. While the page waits for its proofs, and until it is listed,
/// the sweep makes no request for a participant, so its work does not hold the page up.
/// Deterministic: the fake clock moves only when the test moves it.
#[test]
fn a_forum_sync_started_beside_a_cold_page_goes_after_it() {
    let mem = MemHost::new();
    let (host, engine, key, posts, off_screen) = page_then_sync(&mem);
    mem.park_sleeps(true);
    let query = all_posts(&key);
    let fallback = lessdugin_core::dto::SyncOpts::default();
    let mut page = Box::pin(engine.list_posts_page(&query, &fallback));
    assert!(poll_once(&mut page).is_pending());
    let mut sweep = Box::pin(engine.sync_forum(key.uri.as_str(), &PAGE_SYNC));
    // a second of a slow indexer, well within the hold
    for _ in 0..100 {
        assert!(poll_once(&mut page).is_pending(), "the held page finished");
        assert!(poll_once(&mut sweep).is_pending(), "the sweep finished");
        mem.advance_ms(10);
    }
    assert!(!host.held().is_empty(), "the page's proofs were not held");
    assert!(
        !names_any(&mem, &off_screen),
        "the sweep reached a participant while the page was reading"
    );
    host.hold.release();
    let (mut page_done, mut sweep_done) = (None, None);
    for _ in 0..10_000 {
        if page_done.is_none()
            && let Poll::Ready(r) = poll_once(&mut page)
        {
            assert!(
                !names_any(&mem, &off_screen),
                "the sweep reached a participant before the page was listed"
            );
            page_done = Some(r);
        }
        if sweep_done.is_none()
            && let Poll::Ready(r) = poll_once(&mut sweep)
        {
            sweep_done = Some(r);
        }
        if page_done.is_some() && sweep_done.is_some() {
            break;
        }
        mem.advance_ms(10);
    }
    mem.park_sleeps(false);
    let page = page_done.expect("the cold page did not finish").unwrap();
    assert_eq!(unlisted(&page, &posts), Vec::<&String>::new());
    sweep_done.expect("the sweep did not finish").unwrap();
    assert!(names_any(&mem, &off_screen), "the sweep never ran");
}

/// The hold is bounded: a cold page that is not listed within `COLD_PAGE_HOLD_MS` (its proofs
/// held, as by a stalled indexer) holds the forum sync beside it no longer.
#[test]
fn a_cold_page_holds_the_forum_sync_beside_it_for_a_bounded_time() {
    let mem = MemHost::new();
    let (host, engine, key, posts, off_screen) = page_then_sync(&mem);
    mem.park_sleeps(true);
    let query = all_posts(&key);
    let fallback = lessdugin_core::dto::SyncOpts::default();
    let mut page = Box::pin(engine.list_posts_page(&query, &fallback));
    assert!(poll_once(&mut page).is_pending());
    let started = mem.now();
    let mut sweep = Box::pin(engine.sync_forum(key.uri.as_str(), &PAGE_SYNC));
    let mut reached = None;
    for _ in 0..1_000 {
        assert!(poll_once(&mut page).is_pending(), "the held page finished");
        let _ = poll_once(&mut sweep);
        if names_any(&mem, &off_screen) {
            reached = Some(mem.now());
            break;
        }
        mem.advance_ms(10);
    }
    let held = reached.expect("the sweep stayed held by the page") - started;
    let bound = lessdugin_core::limits::COLD_PAGE_HOLD_MS as i64;
    assert!(
        (bound..bound + 1_000).contains(&held),
        "the sweep started {held} ms after it was asked (hold {bound} ms)"
    );
    assert!(!host.held().is_empty(), "the page's proofs were not held");
    host.hold.release();
    mem.park_sleeps(false);
    let page = futures::executor::block_on(page).unwrap();
    assert_eq!(unlisted(&page, &posts), Vec::<&String>::new());
    futures::executor::block_on(sweep).unwrap();
}
