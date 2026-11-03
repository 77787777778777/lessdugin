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

use lessdugin_atp::syntax::{AtUri, Did};
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
/// (`sync.md` §14.3) therefore reads it itself: a granted member's post hydrated from the
/// indexer is listed while the member's own repo cannot be listed at all.
#[test]
fn a_granted_members_hydrated_post_is_listed_before_their_repo_sync() {
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
        serve_indexer(&mem, &net, &[theirs.clone(), mine.clone()]);
        // The member's repo cannot be listed or described: only the cold path can read it.
        for m in [
            "com.atproto.repo.listRecords",
            "com.atproto.repo.describeRepo",
        ] {
            net.inject(Fault::status(500).on_repo(member.did().as_str()).on_nsid(m));
        }
        let engine = assisted(mem.clone());
        engine.forum_open(key.uri.as_str()).await.unwrap();
        let page = engine
            .list_posts_page(&all_posts(&key), &lessdugin_core::dto::SyncOpts::default())
            .await
            .unwrap();
        let listed: Vec<&str> = page.items.iter().map(|p| p.uri.as_str()).collect();
        assert_eq!(listed, [theirs.as_str(), mine.as_str()], "{page:?}");
        let item = &page.items[0];
        assert_eq!(item.title.as_deref(), Some("Member's post"));
        // The membership came with the page's proofs, not from a point fetch at the PDS.
        let fetched: Vec<String> = mem
            .requests()
            .into_iter()
            .map(|r| r.url)
            .filter(|u| u.contains("com.atproto.repo.getRecord") && u.contains(tail(member.did())))
            .collect();
        assert!(fetched.is_empty(), "{fetched:?}");
    });
}

/// `sync.md` §14.3 step 4 and §3.5: the identities and heads of the authors on screen are
/// resolved at P0, ahead of the forum sync's participant sweep that runs beside the cold page,
/// though the sweep reaches them last (in DID order) and started first. Deterministic: the fake
/// clock moves only when the test moves it, and only the order of requests is checked.
#[test]
fn the_authors_on_screen_are_resolved_ahead_of_the_participant_sweep() {
    const MEMBERS: usize = 24;
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
