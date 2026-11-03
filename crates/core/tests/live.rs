// SPDX-License-Identifier: AGPL-3.0-or-later

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lessdugin_atp::syntax::Did;
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, Sources};
use lessdugin_core::host::{
    BlobHandle, Dir, Host, HostError, HttpRequest, HttpResponse, KeyRange, KvOp, KvStore,
    WebSocket, WsMessage,
};
use lessdugin_core::live::source::{Cursor, Subscription};
use lessdugin_core::live::wire::{self, Wire};
use lessdugin_core::live::{
    Action, Context, EventFilter, FilterMode, Gap, LiveConfig, LiveSource, Operation, SourceStatus,
};
use lessdugin_core::store::mem::MemKv;
use lessdugin_core::store::schema;
use lessdugin_core::sync::queue::Action as QueueAction;
use lessdugin_core::sync::{DirtyItem, DirtyQueue, Priority, Reason};
use lessdugin_testkit::{FakeNet, MemHost};
use serde_json::json;

const DID: &str = "did:plc:known";
const OTHER: &str = "did:plc:other";
const NEW: &str = "did:plc:newcomer";
const FK: &str = "aaaaaaaaaaaa";
const REV: &str = "3mxyfycmgv32r";
const CID: &str = "bafyreiarbommdbe24mq6atxsq4ms6dzmkh2nlyagyn35bh7zdaardjmr2q";
const TIME: &str = "2025-10-09T08:53:20.000Z";
const TIME_US: i64 = 1_760_000_000_000_000;
/// When the test client received a frame: an hour after the frames' own time.
const RECEIVED_US: i64 = TIME_US + 3_600_000_000;

#[derive(Clone)]
struct FailOnceKv {
    inner: MemKv,
    fail_next: Arc<AtomicBool>,
    /// The next `get_many` is pending once before it reads (a caller can drop it there).
    yield_next_read: Arc<AtomicBool>,
    successful_writes: Arc<futures::lock::Mutex<()>>,
}

/// Pending exactly once, then ready.
struct YieldOnce(bool);

impl std::future::Future for YieldOnce {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

impl KvStore for FailOnceKv {
    async fn get_many(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, HostError> {
        if self.yield_next_read.swap(false, Ordering::SeqCst) {
            YieldOnce(false).await;
        }
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
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(HostError::Storage("injected live apply failure".into()));
        }
        let _guard = self.successful_writes.lock().await;
        self.inner.write(batch).await
    }
}

#[derive(Clone)]
struct FailHost {
    inner: MemHost,
    cache: FailOnceKv,
    state: FailOnceKv,
}

impl FailHost {
    fn new(inner: MemHost) -> Self {
        let cache = FailOnceKv {
            inner: inner.cache_kv().clone(),
            fail_next: Arc::new(AtomicBool::new(false)),
            yield_next_read: Arc::new(AtomicBool::new(false)),
            successful_writes: Arc::new(futures::lock::Mutex::new(())),
        };
        let state = FailOnceKv {
            inner: inner.state_kv().clone(),
            fail_next: Arc::new(AtomicBool::new(false)),
            yield_next_read: Arc::new(AtomicBool::new(false)),
            successful_writes: Arc::new(futures::lock::Mutex::new(())),
        };
        Self {
            inner,
            cache,
            state,
        }
    }

    fn fail_next_cache_write(&self) {
        self.cache.fail_next.store(true, Ordering::SeqCst);
    }

    fn allow_cache_writes(&self) {
        self.cache.fail_next.store(false, Ordering::SeqCst);
    }
}

impl Host for FailHost {
    type Kv = FailOnceKv;
    type Ws = <MemHost as Host>::Ws;
    type Lock = <MemHost as Host>::Lock;

    fn cache(&self) -> &Self::Kv {
        &self.cache
    }

    fn state(&self) -> &Self::Kv {
        &self.state
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
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

/// The frames of one browser-like socket. Like `BrowserWebSocket` in packages/engine/src/host.ts,
/// a `next()` call registers its waiter at once, and a frame goes to the oldest waiter even if
/// that receive was abandoned: the frame is then lost.
#[derive(Clone, Default)]
struct Feed {
    state: Arc<std::sync::Mutex<FeedState>>,
}

#[derive(Default)]
struct FeedState {
    waiters: std::collections::VecDeque<futures::channel::oneshot::Sender<WsMessage>>,
    queue: std::collections::VecDeque<WsMessage>,
    lost: usize,
    closed: bool,
}

impl Feed {
    fn push(&self, message: WsMessage) {
        let mut st = self.state.lock().unwrap();
        if st.closed {
            st.lost += 1;
        } else if let Some(waiter) = st.waiters.pop_front() {
            if waiter.send(message).is_err() {
                st.lost += 1;
            }
        } else {
            st.queue.push_back(message);
        }
    }

    fn lost(&self) -> usize {
        self.state.lock().unwrap().lost
    }

    fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
}

struct BrowserLikeWs {
    feed: Feed,
}

impl WebSocket for BrowserLikeWs {
    fn next(
        &mut self,
    ) -> impl std::future::Future<Output = Option<Result<WsMessage, HostError>>> + Send {
        let (tx, rx) = futures::channel::oneshot::channel();
        {
            let mut st = self.feed.state.lock().unwrap();
            if let Some(message) = st.queue.pop_front() {
                let _ = tx.send(message);
            } else if !st.closed {
                st.waiters.push_back(tx);
            }
        }
        async move { rx.await.ok().map(Ok) }
    }

    async fn send_text(&mut self, _text: String) -> Result<(), HostError> {
        Ok(())
    }

    async fn close(&mut self) {
        let mut st = self.feed.state.lock().unwrap();
        st.closed = true;
        st.waiters.clear();
    }
}

/// MemHost with browser-like sockets that stay open until closed: each connection gets its own
/// [`Feed`].
#[derive(Clone)]
struct BrowserLikeHost {
    inner: MemHost,
    sockets: Arc<std::sync::Mutex<Vec<(String, Feed)>>>,
}

impl BrowserLikeHost {
    fn new(inner: MemHost) -> Self {
        Self {
            inner,
            sockets: Arc::default(),
        }
    }

    fn urls(&self) -> Vec<String> {
        self.sockets
            .lock()
            .unwrap()
            .iter()
            .map(|(u, _)| u.clone())
            .collect()
    }

    /// The feed of the `i`-th connection.
    fn feed_at(&self, i: usize) -> Feed {
        self.sockets.lock().unwrap()[i].1.clone()
    }

    /// The feed of the last connection whose URL contains `pattern`.
    fn feed(&self, pattern: &str) -> Feed {
        self.sockets
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(u, _)| u.contains(pattern))
            .map(|(_, f)| f.clone())
            .expect("no such connection")
    }
}

impl Host for BrowserLikeHost {
    type Kv = MemKv;
    type Ws = BrowserLikeWs;
    type Lock = <MemHost as Host>::Lock;

    fn cache(&self) -> &MemKv {
        self.inner.cache()
    }

    fn state(&self) -> &MemKv {
        self.inner.state()
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, HostError> {
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

    async fn websocket(&self, url: url::Url, _max: u32) -> Result<BrowserLikeWs, HostError> {
        let feed = Feed::default();
        self.sockets
            .lock()
            .unwrap()
            .push((url.to_string(), feed.clone()));
        Ok(BrowserLikeWs { feed })
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

fn context() -> Context {
    Context {
        known_dids: BTreeSet::from([DID.to_owned()]),
        forums: BTreeSet::from([FK.to_owned()]),
        newcomer_forums: BTreeSet::from([FK.to_owned()]),
        declarations: BTreeSet::from([(DID.to_owned(), "forum".to_owned())]),
        ..Context::default()
    }
}

fn v2_commit(did: &str, collection: &str, rkey: &str, seq: u64) -> String {
    json!({
        "$type": "message",
        "payload": {
            "$type": "network.bsky.jetstream.subscribeEvents#commit",
            "did": did, "seq": seq, "time": TIME, "witnessedAt": TIME,
            "rev": REV, "operation": "create", "collection": collection,
            "rkey": rkey, "cid": CID,
            "record": {"$type": collection, "text": "UNTRUSTED HINT TEXT"}
        }
    })
    .to_string()
}

fn v1_commit(did: &str, collection: &str, rkey: &str) -> String {
    json!({
        "did": did, "time_us": TIME_US, "cursor": 42, "kind": "commit",
        "commit": {
            "rev": REV, "operation": "create", "collection": collection,
            "rkey": rkey, "cid": CID,
            "record": {"text": "also untrusted"}
        }
    })
    .to_string()
}

fn post() -> String {
    lessdugin_lexicon::collection::nsid::POST.to_owned()
}

fn membership() -> String {
    lessdugin_lexicon::collection::nsid::MEMBERSHIP.to_owned()
}

fn point(output: &lessdugin_core::live::FilterOutput) -> (String, Option<String>, i64) {
    match &output.actions[..] {
        [
            Action::PointFetch {
                uri,
                cid,
                first_seen_us,
                ..
            },
        ] => (uri.clone(), cid.clone(), *first_seen_us),
        other => panic!("expected point fetch, got {other:?}"),
    }
}

#[test]
fn v2_exact_filter_and_record_is_only_a_hint() {
    let cx = context();
    let mut filter = EventFilter::default();
    let event = wire::parse(Wire::V2, &v2_commit(DID, &post(), &format!("{FK}-3l"), 9))
        .unwrap()
        .unwrap();
    let out = filter.apply(Wire::V2, "one", event, &cx, RECEIVED_US);
    let (uri, cid, first) = point(&out);
    assert_eq!(uri, format!("at://{DID}/{}/{FK}-3l", post()));
    assert_eq!(cid.as_deref(), Some(CID));
    // first-seen is when this client received the frame, not the server's time (security-web-5)
    assert_eq!(first, RECEIVED_US);
    assert!(!format!("{:?}", out).contains("UNTRUSTED"));

    for bad_rkey in [format!("{FK}x-3l"), format!("{FK}x"), "other".into()] {
        let e = wire::parse(Wire::V2, &v2_commit(DID, &post(), &bad_rkey, 10))
            .unwrap()
            .unwrap();
        assert!(filter.apply(Wire::V2, "one", e, &cx, RECEIVED_US).actions.is_empty());
    }
    let spoof = format!("{}.spoof", post());
    let e = wire::parse(Wire::V2, &v2_commit(DID, &spoof, &format!("{FK}-3l"), 11))
        .unwrap()
        .unwrap();
    assert!(filter.apply(Wire::V2, "one", e, &cx, RECEIVED_US).actions.is_empty());
}

#[test]
fn shared_v1_v2_dedup_and_cursor() {
    let cx = context();
    let mut filter = EventFilter::default();
    let v1 = wire::parse(Wire::V1, &v1_commit(DID, &post(), &format!("{FK}-3l")))
        .unwrap()
        .unwrap();
    let first = filter.apply(Wire::V1, "old", v1, &cx, RECEIVED_US);
    assert_eq!(first.actions.len(), 1);
    assert_eq!(first.cursor.unwrap().time_us, TIME_US);
    filter.acknowledge();
    let v2 = wire::parse(Wire::V2, &v2_commit(DID, &post(), &format!("{FK}-3l"), 42))
        .unwrap()
        .unwrap();
    let replay = filter.apply(Wire::V2, "new", v2, &cx, RECEIVED_US);
    assert!(replay.duplicate);
    assert!(replay.actions.is_empty());
    assert_eq!(replay.cursor.unwrap().seq, Some(42));
}

#[test]
fn failed_actionable_frame_replays_instead_of_becoming_a_duplicate() {
    let cx = context();
    let mut filter = EventFilter::default();
    let event = || {
        wire::parse(
            Wire::V2,
            &v2_commit(DID, &post(), &format!("{FK}-replay"), 42),
        )
        .unwrap()
        .unwrap()
    };
    let first = filter.apply(Wire::V2, "one", event(), &cx, RECEIVED_US);
    assert_eq!(first.actions.len(), 1);
    assert!(filter.has_pending());
    // The integrator fails before committing effects: the wire replay cannot skip this work.
    assert!(
        filter
            .apply(Wire::V2, "one", event(), &cx, RECEIVED_US)
            .actions
            .is_empty()
    );
    let (_, _, _, _, retry) = filter.retry().unwrap();
    assert_eq!(retry.actions, first.actions);
    assert_eq!(retry.cursor.unwrap().time_us, TIME_US);
    assert!(!filter.acknowledge_for(Wire::V2, "two"));
    assert!(filter.has_pending());
    assert!(filter.acknowledge_for(Wire::V2, "one"));
    assert!(!filter.has_pending());
    let duplicate = filter.apply(Wire::V2, "one", event(), &cx, RECEIVED_US);
    assert!(duplicate.duplicate);
    assert!(duplicate.actions.is_empty());
}

#[test]
fn identity_account_sync_drop_network_noise() {
    let cx = context();
    let identity = json!({"$type":"message","payload":{
        "$type":"network.bsky.jetstream.subscribeEvents#identity", "did":OTHER,
        "seq":50,"witnessedAt":TIME,"identity":{"did":OTHER,"seq":8,"time":TIME}
    }})
    .to_string();
    let account = json!({"$type":"message","payload":{
        "$type":"network.bsky.jetstream.subscribeEvents#account", "did":DID,
        "seq":51,"witnessedAt":TIME,"account":{"did":DID,"seq":9,"time":TIME,"active":false,"status":"suspended"}
    }}).to_string();
    let sync = json!({"$type":"message","payload":{
        "$type":"network.bsky.jetstream.subscribeEvents#sync", "did":DID,
        "seq":52,"witnessedAt":TIME,"time":TIME
    }})
    .to_string();
    let mut filter = EventFilter::default();
    let noise = wire::parse(Wire::V2, &identity).unwrap().unwrap();
    assert!(filter.apply(Wire::V2, "i", noise, &cx, RECEIVED_US).actions.is_empty());
    let account = wire::parse(Wire::V2, &account).unwrap().unwrap();
    let out = filter.apply(Wire::V2, "i", account, &cx, RECEIVED_US);
    assert!(matches!(
        out.actions[..],
        [Action::AccountStatus { active: false, .. }]
    ));
    filter.acknowledge();
    let sync = wire::parse(Wire::V2, &sync).unwrap().unwrap();
    assert!(matches!(
        filter.apply(Wire::V2, "i", sync, &cx, RECEIVED_US).actions[..],
        [Action::Resync { .. }]
    ));
}

#[test]
fn malformed_events_are_errors_not_panics_or_actions() {
    let valid = v2_commit(DID, &post(), &format!("{FK}-3l"), 42);
    for bad in [
        "not json".into(),
        valid.replace(DID, "not-a-did"),
        valid.replace(REV, "bad-rev"),
        valid.replace(CID, "bad-cid"),
        valid.replace(TIME, "not-a-time"),
        valid.replace(&format!("\"rkey\":\"{FK}-3l\""), "\"rkey\":\"..\""),
        valid.replace("\"seq\":42", "\"seq\":\"42\""),
    ] {
        assert!(wire::parse(Wire::V2, &bad).is_err(), "accepted {bad}");
    }
    let unknown = json!({"$type":"message","payload":{"$type":"something.new#future"}});
    assert_eq!(wire::parse(Wire::V2, &unknown.to_string()).unwrap(), None);
}

#[test]
fn unknown_did_only_exact_f_new_membership_candidate() {
    let cx = context();
    let mut filter = EventFilter::default();
    let e = wire::parse(Wire::V2, &v2_commit(NEW, &membership(), FK, 1))
        .unwrap()
        .unwrap();
    let out = filter.apply(Wire::V2, "i", e, &cx, RECEIVED_US);
    assert!(matches!(
        out.actions[..],
        [Action::Candidate { ref did, ref fk, .. }] if did == NEW && fk == FK
    ));
    for (collection, rkey) in [(post(), FK.to_owned()), (membership(), format!("{FK}-x"))] {
        let e = wire::parse(Wire::V2, &v2_commit(NEW, &collection, &rkey, 2))
            .unwrap()
            .unwrap();
        assert!(filter.apply(Wire::V2, "i", e, &cx, RECEIVED_US).actions.is_empty());
    }
}

#[test]
fn info_actions_restart_cursor() {
    for (name, gap) in [
        ("OutdatedCursor", Gap::OutdatedCursor),
        ("FutureCursor", Gap::FutureCursor),
    ] {
        let text = json!({"$type":"message","payload":{
            "$type":"network.bsky.jetstream.subscribeEvents#info", "name":name
        }})
        .to_string();
        let e = wire::parse(Wire::V2, &text).unwrap().unwrap();
        let mut filter = EventFilter::default();
        let out = filter.apply(Wire::V2, "i", e, &context(), RECEIVED_US);
        if gap == Gap::OutdatedCursor {
            assert_eq!(out.actions, vec![Action::Reconcile(gap)]);
            let retry = filter.retry().unwrap().4;
            assert_eq!(retry, out);
            assert!(retry.restart_cursor);
            assert!(retry.cursor.is_none());
        } else {
            assert!(out.actions.is_empty());
        }
        assert!(out.restart_cursor);
        assert!(out.cursor.is_none());
    }
}

#[test]
fn subscription_modes_and_private_participant_filters() {
    let base = url::Url::parse("wss://jetstream.example/subscribe").unwrap();
    let ns = lessdugin_lexicon::ns::NS;
    let dids = |n| {
        (0..n)
            .map(|i| format!("did:plc:user{i}"))
            .collect::<Vec<_>>()
    };
    let native = Subscription::plan(&base, ns, false, true, &dids(200), None, TIME_US).unwrap();
    assert_eq!(native.mode, FilterMode::Wildcard);
    assert_eq!(
        native
            .url
            .query_pairs()
            .filter(|(k, _)| k == "dids")
            .count(),
        0
    );
    let early = Subscription::plan(&base, ns, true, false, &dids(2), None, TIME_US).unwrap();
    assert_eq!(early.mode, FilterMode::Wildcard);
    let small = Subscription::plan(&base, ns, true, true, &dids(150), None, TIME_US).unwrap();
    assert_eq!(small.mode, FilterMode::V2Dids);
    assert_eq!(
        small.url.query_pairs().filter(|(k, _)| k == "dids").count(),
        150
    );
    assert!(!small.url.as_str().contains("kinds=commit"));
    let large = Subscription::plan(&base, ns, true, true, &dids(151), None, TIME_US).unwrap();
    assert_eq!(large.mode, FilterMode::V1Dids);
    let hello: serde_json::Value = serde_json::from_str(large.hello.as_deref().unwrap()).unwrap();
    assert_eq!(
        hello["payload"]["wantedDids"].as_array().unwrap().len(),
        151
    );
    assert_eq!(
        large.url.query_pairs().filter(|(k, _)| k == "dids").count(),
        0
    );
    let huge = Subscription::plan(&base, ns, true, true, &dids(10_001), None, TIME_US).unwrap();
    assert_eq!(huge.mode, FilterMode::Wildcard);
}

#[test]
fn cursor_resume_inclusive_or_rewound() {
    let cursor = Cursor {
        wire: Wire::V2,
        instance: "wss://one:443".into(),
        seq: Some(42),
        time_us: TIME_US,
    };
    let base = url::Url::parse("wss://one/subscribe").unwrap();
    let same = Subscription::plan(
        &base,
        lessdugin_lexicon::ns::NS,
        false,
        false,
        &[],
        Some(&cursor),
        TIME_US,
    )
    .unwrap();
    assert!(
        same.url
            .query_pairs()
            .any(|(k, v)| k == "cursor" && v == "42")
    );
    let other = url::Url::parse("wss://two/subscribe").unwrap();
    let moved = Subscription::plan(
        &other,
        lessdugin_lexicon::ns::NS,
        false,
        false,
        &[],
        Some(&cursor),
        TIME_US,
    )
    .unwrap();
    assert!(
        moved
            .url
            .query_pairs()
            .any(|(k, v)| k == "cursor" && v == (TIME_US - 5_000_000).to_string())
    );
}

#[test]
fn live_actions_coalesce_in_existing_queue() {
    let mut queue = DirtyQueue::new();
    queue.enqueue(DirtyItem::point(DID, "at://did/a/1", Priority::P1, 10_000).with_min_rev(REV));
    queue.enqueue(DirtyItem::new(DID, Priority::P0, Reason::Resync, 5_000));
    assert_eq!(queue.len(), 1);
    let item = queue.get(DID).unwrap();
    assert_eq!(item.priority, Priority::P0);
    assert_eq!(item.ready_at, 5_000);
    assert_eq!(item.action(), QueueAction::Sync);
}

#[test]
fn memhost_v1_v2_source_and_persisted_cursor() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        let coll = post();
        host.script_websocket(
            "wss://jetstream.example/",
            vec![WsMessage::Text(v2_commit(
                DID,
                &coll,
                &format!("{FK}-3l"),
                42,
            ))],
        );
        let mut source = LiveSource::new(
            Arc::new(host.clone()),
            LiveConfig::new(
                vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                false,
            ),
        );
        let status = source.next(&context()).await.unwrap();
        assert!(matches!(status, SourceStatus::Actions(ref a) if a.len() == 1));
        assert!(source.cursor().is_none());
        source.acknowledge().await.unwrap();
        assert_eq!(source.cursor().unwrap().seq, Some(42));
        // A clean close flushes before the periodic 5s/1000-event deadline and schedules backoff.
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Waiting
        );
        assert!(source.retry_in_ms() > 0);
        let raw = host
            .cache()
            .get_many(vec![schema::cur_key("jetstream")])
            .await
            .unwrap()
            .pop()
            .flatten();
        assert!(raw.is_some());

        let v1_host = MemHost::new();
        v1_host.script_websocket(
            "ws://jetstream.test/",
            vec![WsMessage::Text(v1_commit(DID, &coll, &format!("{FK}-3l")))],
        );
        let mut cx = context();
        cx.participants_known = true;
        cx.filter_dids = (0..151).map(|i| format!("did:plc:user{i}")).collect();
        let mut v1 = LiveSource::new(
            Arc::new(v1_host.clone()),
            LiveConfig::new(
                vec![url::Url::parse("ws://jetstream.test/subscribe").unwrap()],
                true,
            ),
        );
        assert!(matches!(
            v1.next(&cx).await.unwrap(),
            SourceStatus::Actions(_)
        ));
        let sent = v1_host.websocket_sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("options_update"));
        assert!(!sent[0].contains(DID));
    });
}

#[test]
fn memhost_info_gap_is_bounded_and_malformed_does_not_move_cursor() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        let info = json!({"$type":"message","payload":{
            "$type":"network.bsky.jetstream.subscribeEvents#info","name":"OutdatedCursor"
        }})
        .to_string();
        host.script_websocket("wss://jetstream.example/", vec![WsMessage::Text(info)]);
        let mut source = LiveSource::new(
            Arc::new(host),
            LiveConfig::new(
                vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                false,
            ),
        );
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Actions(vec![Action::Reconcile(Gap::OutdatedCursor)])
        );
        assert!(source.cursor().is_none());
        source.acknowledge().await.unwrap();
        assert_eq!(source.cursor().unwrap().seq, None);
        assert_eq!(source.cursor().unwrap().time_us, TIME_US - 5_000_000);
        assert!(source.retry_in_ms() > 0);
        // One call does not spin reconnecting a scripted closed socket.
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Waiting
        );
    });
}

#[test]
fn malformed_source_frame_does_not_move_cursor_or_kill_connection() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        host.script_websocket(
            "wss://jetstream.example/",
            vec![
                WsMessage::Text("{malformed".into()),
                WsMessage::Text(v2_commit(DID, &post(), &format!("{FK}-3l"), 42)),
            ],
        );
        let mut source = LiveSource::new(
            Arc::new(host),
            LiveConfig::new(
                vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                false,
            ),
        );
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Waiting
        );
        assert!(source.cursor().is_none());
        assert!(matches!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Actions(_)
        ));
        assert!(source.cursor().is_none());
        source.acknowledge().await.unwrap();
        assert_eq!(source.cursor().unwrap().seq, Some(42));
    });
}

#[test]
fn dedup_is_bounded_lru() {
    let cx = context();
    let mut filter = EventFilter::with_dedup_capacity(2);
    for rkey in ["aaaaaaaaaaaa-1", "aaaaaaaaaaaa-2"] {
        let e = wire::parse(Wire::V2, &v2_commit(DID, &post(), rkey, 1))
            .unwrap()
            .unwrap();
        assert_eq!(filter.apply(Wire::V2, "i", e, &cx, RECEIVED_US).actions.len(), 1);
        filter.acknowledge();
    }
    // Promote the first key, then insert a third: the second is the least recently used.
    let first = wire::parse(Wire::V2, &v2_commit(DID, &post(), "aaaaaaaaaaaa-1", 1))
        .unwrap()
        .unwrap();
    assert!(filter.apply(Wire::V2, "i", first, &cx, RECEIVED_US).duplicate);
    let third = wire::parse(Wire::V2, &v2_commit(DID, &post(), "aaaaaaaaaaaa-3", 1))
        .unwrap()
        .unwrap();
    assert_eq!(filter.apply(Wire::V2, "i", third, &cx, RECEIVED_US).actions.len(), 1);
    filter.acknowledge();
    assert_eq!(filter.dedup().len(), 2);
    let second = wire::parse(Wire::V2, &v2_commit(DID, &post(), "aaaaaaaaaaaa-2", 1))
        .unwrap()
        .unwrap();
    assert_eq!(filter.apply(Wire::V2, "i", second, &cx, RECEIVED_US).actions.len(), 1);
}

#[test]
fn two_oversized_frames_recover_once_with_reconcile() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        host.script_websocket(
            "wss://jetstream.example/",
            vec![WsMessage::Text("x".repeat(1_200_001))],
        );
        let mut source = LiveSource::new(
            Arc::new(host.clone()),
            LiveConfig::new(
                vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                false,
            ),
        );
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Waiting
        );
        host.advance_ms(source.retry_in_ms());
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Actions(vec![Action::Reconcile(Gap::OversizedMessage)])
        );
        assert!(source.cursor().is_none());
        source.acknowledge().await.unwrap();
        assert!(source.cursor().is_some());
        // A third call observes backoff instead of spinning on the scripted socket.
        assert_eq!(
            source.next(&context()).await.unwrap(),
            SourceStatus::Waiting
        );
    });
}

#[test]
fn live_tail_store_failure_replays_frame_before_advancing_cursor() {
    futures::executor::block_on(async {
        let mem = MemHost::new();
        let net = FakeNet::on(&mem);
        let account = net.add_account("alice.example.com");
        let host = FailHost::new(mem.clone());
        let endpoint = url::Url::parse("wss://jetstream.example/subscribe").unwrap();
        let engine = Engine::new(
            host.clone(),
            EngineConfig {
                jetstream: vec![endpoint],
                ..EngineConfig::default()
            },
        );
        engine
            .add_account(AccountInfo::app_password(
                account.did().as_str(),
                account.pds_base(),
            ))
            .await
            .unwrap();
        mem.script_websocket(
            "wss://jetstream.example/",
            vec![WsMessage::Text(v2_commit(
                account.did().as_str(),
                &post(),
                &format!("{FK}-3l"),
                42,
            ))],
        );
        mem.park_sleeps(true);
        host.fail_next_cache_write();
        let replay_write = host.cache.successful_writes.lock().await;

        let tail = engine.live_tail();
        futures::pin_mut!(tail);
        assert!(futures::poll!(tail.as_mut()).is_pending());
        assert!(
            !host.cache.fail_next.load(Ordering::SeqCst),
            "the actionable frame did not reach its cache write"
        );
        let key = schema::cur_key("jetstream");
        assert!(
            host.cache()
                .get_many(vec![key.clone()])
                .await
                .unwrap()
                .pop()
                .flatten()
                .is_none(),
            "a failed action batch must not persist the frame cursor"
        );

        // The next round receives LiveSource's buffered action, not a new socket frame. Move past
        // the periodic persistence deadline so a successful replay makes the cursor observable.
        host.allow_cache_writes();
        mem.advance_ms(5_001);
        drop(replay_write);
        assert!(futures::poll!(tail.as_mut()).is_pending());
        assert!(
            host.cache()
                .get_many(vec![key])
                .await
                .unwrap()
                .pop()
                .flatten()
                .is_some(),
            "the replayed action batch must be acknowledged after it applies"
        );
        engine.live_stop();
        tail.await.unwrap();
    });
}

#[test]
fn account_status_failure_replays_before_advancing_cursor() {
    futures::executor::block_on(async {
        let mem = MemHost::new();
        let net = FakeNet::on(&mem);
        let account = net.add_account("alice.example.com");
        let engine = Engine::new(
            mem.clone(),
            EngineConfig {
                jetstream: vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                ..EngineConfig::default()
            },
        );
        engine
            .add_account(AccountInfo::app_password(
                account.did().as_str(),
                account.pds_base(),
            ))
            .await
            .unwrap();
        let event = json!({"$type":"message","payload":{
            "$type":"network.bsky.jetstream.subscribeEvents#account", "did":account.did(),
            "seq":42,"witnessedAt":TIME,"account":{"did":account.did(),"seq":9,"time":TIME,
            "active":false,"status":"deactivated"}
        }})
        .to_string();
        mem.script_websocket("wss://jetstream.example/", vec![WsMessage::Text(event)]);
        mem.park_sleeps(true);
        net.inject(
            lessdugin_testkit::fake_pds::Fault::disconnect()
                .on_repo(account.did().as_str())
                .on_nsid("com.atproto.sync.getRepoStatus")
                .once(),
        );
        let tail = engine.live_tail();
        futures::pin_mut!(tail);
        assert!(futures::poll!(tail.as_mut()).is_pending());
        let key = schema::cur_key("jetstream");
        assert!(
            mem.cache()
                .get_many(vec![key.clone()])
                .await
                .unwrap()
                .pop()
                .flatten()
                .is_none(),
            "a failed getRepoStatus must leave the live frame unacknowledged"
        );
        mem.advance_ms(5_001);
        assert!(futures::poll!(tail.as_mut()).is_pending());
        assert!(
            mem.cache()
                .get_many(vec![key])
                .await
                .unwrap()
                .pop()
                .flatten()
                .is_some(),
            "a successful status refresh acknowledges the replayed frame"
        );
        engine.live_stop();
        tail.await.unwrap();
    });
}

#[test]
fn discovery_completion_controls_private_participant_filters() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        host.park_sleeps(true);
        let candidate = "did:plc:candidate".to_owned();
        for complete in [false, true] {
            host.script_websocket(
                "wss://jetstream.example/",
                vec![WsMessage::Text(v2_commit(DID, &post(), "unrelated", 1))],
            );
            let mut source = LiveSource::new(
                Arc::new(host.clone()),
                LiveConfig::new(
                    vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                    true,
                ),
            );
            let context = Context {
                participants_known: complete,
                filter_dids: if complete {
                    vec![candidate.clone()]
                } else {
                    Vec::new()
                },
                ..context()
            };
            // Drive exactly one connection and one scripted frame; no engine tail or sleep.
            let status = source.next(&context).await.unwrap();
            assert!(matches!(
                status,
                SourceStatus::Actions(_) | SourceStatus::Waiting
            ));
            source.stop().await;
        }

        // sync.md §6.2: filtered mode with F_new non-empty adds the membership side connection.
        let (side, urls): (Vec<String>, Vec<String>) = host
            .websocket_urls()
            .into_iter()
            .partition(|u| u.contains("kinds=commit"));
        assert_eq!(side.len(), 1, "only the filtered source opens the side connection");
        assert_eq!(urls.len(), 2);
        let filtered = |url: &str| {
            url::Url::parse(url)
                .unwrap()
                .query_pairs()
                .any(|(k, v)| k == "dids" && v == candidate)
        };
        assert!(!filtered(&urls[0]), "discovery was not complete");
        assert!(filtered(&urls[1]), "completed discovery must filter DIDs");
    });
}

#[test]
fn direct_engine_takes_no_live_leader_or_socket() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        host.park_sleeps(true);
        let engine = Engine::new(
            host.clone(),
            EngineConfig {
                sources: Sources::Direct,
                sources_flag: Some(Sources::Direct),
                ..EngineConfig::default()
            },
        );
        let tail = engine.live_tail();
        futures::pin_mut!(tail);
        assert!(futures::poll!(tail.as_mut()).is_pending());
        // Assisted LiveSource takes this lock before Host::websocket. Direct mode leaves it free.
        assert!(host.lock("sync-leader", false).await.unwrap().is_some());
        host.park_sleeps(false);
        engine.live_stop();
        tail.await.unwrap();
        assert!(host.websocket_sent().is_empty());
    });
}

#[test]
fn did_parser_used_by_filter_rejects_bad_own_did() {
    assert!(Did::parse("did:plc:valid").is_ok());
    assert!(Did::parse("did:PLC:invalid").is_err());
    assert_eq!(Operation::Create, Operation::Create);
}

#[test]
fn browser_filter_reconnect_is_rate_limited_and_catches_up_new_dids() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        host.script_websocket(
            "wss://jetstream.example/",
            vec![
                WsMessage::Text(v2_commit(OTHER, &post(), "unrelated", 1)),
                WsMessage::Text(v2_commit(OTHER, &post(), "unrelated", 2)),
            ],
        );
        let mut source = LiveSource::new(
            Arc::new(host.clone()),
            LiveConfig::new(
                vec![url::Url::parse("wss://jetstream.example/subscribe").unwrap()],
                true,
            ),
        );
        let mut cx = context();
        cx.participants_known = true;
        cx.filter_dids = vec![DID.to_owned()];
        // sync.md §6.2: the membership side connection (F_new is not empty) is not counted.
        let main_urls = || {
            host.websocket_urls()
                .into_iter()
                .filter(|u| !u.contains("kinds=commit"))
                .collect::<Vec<_>>()
        };
        assert_eq!(source.next(&cx).await.unwrap(), SourceStatus::Waiting);
        assert_eq!(main_urls().len(), 1);

        cx.filter_dids.push(OTHER.to_owned());
        assert_eq!(source.next(&cx).await.unwrap(), SourceStatus::Waiting);
        assert_eq!(main_urls().len(), 1, "changed D is rate-limited");

        host.advance_ms(600_000);
        assert_eq!(
            source.next(&cx).await.unwrap(),
            SourceStatus::Actions(vec![Action::CatchUp {
                did: OTHER.to_owned()
            }])
        );
        let urls = main_urls();
        assert_eq!(urls.len(), 2);
        assert!(urls[1].contains("dids=did%3Aplc%3Aother"));
        assert!(!urls[1].contains("kinds=commit"));
    });
}

fn url_cursor(url: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "cursor")
        .map(|(_, v)| v.into_owned())
        .expect("every subscription carries a cursor")
}

fn jetstream_source<H: Host>(host: H, base: &str, browser: bool) -> LiveSource<H> {
    LiveSource::new(
        Arc::new(host),
        LiveConfig::new(vec![url::Url::parse(base).unwrap()], browser),
    )
}

/// Runs one source until its first frame is acknowledged and the scripted socket has closed, so
/// the cursor of that frame is persisted (a close always flushes it).
async fn persist_seq_42(host: &MemHost) {
    host.script_websocket(
        "wss://jetstream.example/",
        vec![WsMessage::Text(v2_commit(DID, &post(), &format!("{FK}-3l"), 42))],
    );
    let mut first = jetstream_source(host.clone(), "wss://jetstream.example/subscribe", false);
    assert!(matches!(
        first.next(&context()).await.unwrap(),
        SourceStatus::Actions(_)
    ));
    first.acknowledge().await.unwrap();
    assert_eq!(first.next(&context()).await.unwrap(), SourceStatus::Waiting);
    first.stop().await;
}

#[test]
fn persisted_cursor_is_resumed_after_a_restart() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        persist_seq_42(&host).await;

        // Same instance, v2: the inclusive seq.
        host.script_websocket("wss://jetstream.example/", Vec::new());
        let mut same = jetstream_source(host.clone(), "wss://jetstream.example/subscribe", false);
        assert_eq!(same.next(&context()).await.unwrap(), SourceStatus::Waiting);
        assert_eq!(same.cursor().map(|c| c.seq), Some(Some(42)));
        let urls = host.websocket_urls();
        assert_eq!(url_cursor(&urls[1]), "42", "restart must resume at the stored seq");
        same.stop().await;

        // Another instance an hour later: the stored time rewound 5 s, not now - 5 s.
        host.advance_ms(3_600_000);
        host.script_websocket("wss://other.example/", Vec::new());
        let mut other = jetstream_source(host.clone(), "wss://other.example/subscribe", false);
        assert_eq!(other.next(&context()).await.unwrap(), SourceStatus::Waiting);
        let urls = host.websocket_urls();
        assert_eq!(
            url_cursor(&urls[2]),
            (TIME_US - 5_000_000).to_string(),
            "a restart must replay the hour the client was closed"
        );
        other.stop().await;

        // More than 36 h later the stored cursor is a gap: a reconcile first, then near now.
        host.advance_ms(36 * 3_600_000);
        let mut late = jetstream_source(host.clone(), "wss://other.example/subscribe", false);
        assert_eq!(
            late.next(&context()).await.unwrap(),
            SourceStatus::Actions(vec![Action::Reconcile(Gap::OutdatedCursor)])
        );
    });
}

#[test]
fn a_dropped_first_call_still_loads_the_persisted_cursor() {
    futures::executor::block_on(async {
        let mem = MemHost::new();
        persist_seq_42(&mem).await;
        let host = FailHost::new(mem.clone());
        mem.script_websocket("wss://jetstream.example/", Vec::new());
        let mut source = jetstream_source(host.clone(), "wss://jetstream.example/subscribe", false);
        host.cache.yield_next_read.store(true, Ordering::SeqCst);
        // The caller's timer wins while the cursor row is being read.
        assert!(
            futures::FutureExt::now_or_never(source.next(&context())).is_none(),
            "the first call must be pending inside the cursor read"
        );
        assert_eq!(source.next(&context()).await.unwrap(), SourceStatus::Waiting);
        let urls = mem.websocket_urls();
        assert_eq!(url_cursor(urls.last().unwrap()), "42");
    });
}

fn info(name: &str) -> String {
    json!({"$type":"message","payload":{
        "$type":"network.bsky.jetstream.subscribeEvents#info","name":name
    }})
    .to_string()
}

#[test]
fn a_dropped_next_never_loses_a_browser_frame() {
    futures::executor::block_on(async {
        use futures::FutureExt;
        let host = BrowserLikeHost::new(MemHost::new());
        let mut source = jetstream_source(host.clone(), "wss://jetstream.example/subscribe", true);
        // live_tail's timer wins while the source waits for a frame: the next() is dropped.
        assert!(source.next(&context()).now_or_never().is_none());
        let main = host.feed("subscribeEvents");
        main.push(WsMessage::Text(v2_commit(DID, &post(), &format!("{FK}-3l"), 42)));
        assert_eq!(main.lost(), 0, "the frame went to an abandoned receive");
        assert!(matches!(
            source.next(&context()).now_or_never(),
            Some(Ok(SourceStatus::Actions(ref a))) if a.len() == 1
        ));
        source.acknowledge().await.unwrap();
        // Many abandoned rounds still leave exactly one receive in flight.
        for _ in 0..5 {
            assert!(source.next(&context()).now_or_never().is_none());
        }
        assert_eq!(main.state.lock().unwrap().waiters.len(), 1);
        source.stop().await;
        assert!(main.closed(), "stop closes the socket");
        assert_eq!(host.urls().len(), 1, "no socket was opened twice");
    });
}

#[test]
fn a_frame_time_never_becomes_first_seen() {
    futures::executor::block_on(async {
        let host = MemHost::new();
        // A Jetstream that claims a commit is a day older (or newer) than it is.
        let day = 86_400_000;
        for shift in [-day, day] {
            let frame_us = (host.now() + shift) * 1_000;
            let time = lessdugin_atp::syntax::Datetime::from_micros(frame_us)
                .unwrap()
                .to_string();
            host.script_websocket(
                "wss://jetstream.example/",
                vec![WsMessage::Text(
                    v2_commit(DID, &post(), &format!("{FK}-3l"), 42).replace(TIME, &time),
                )],
            );
            let mut source =
                jetstream_source(host.clone(), "wss://jetstream.example/subscribe", false);
            let SourceStatus::Actions(actions) = source.next(&context()).await.unwrap() else {
                panic!("no action");
            };
            let [Action::PointFetch { first_seen_us, .. }] = &actions[..] else {
                panic!("expected a point fetch, got {actions:?}");
            };
            assert_eq!(*first_seen_us, host.now() * 1_000, "first seen is the receive time");
            source.acknowledge().await.unwrap();
            assert_eq!(source.cursor().unwrap().time_us, frame_us, "the cursor keeps it");
            source.stop().await;
        }
    });
}

#[test]
fn a_gap_on_every_connection_backs_off_and_reconciles_once() {
    // An endpoint that answers every connection with OutdatedCursor, at once or after a frame.
    let scripts = [
        vec![WsMessage::Text(info("OutdatedCursor"))],
        vec![
            WsMessage::Text(v2_commit(OTHER, &post(), "unrelated", 1)),
            WsMessage::Text(info("OutdatedCursor")),
        ],
    ];
    for script in scripts {
        futures::executor::block_on(async {
            let host = MemHost::new();
            host.script_websocket("wss://jetstream.example/", script);
            let mut source =
                jetstream_source(host.clone(), "wss://jetstream.example/subscribe", false);
            let start = host.now();
            let mut reconciles = 0;
            while host.now() - start < 60_000 {
                match source.next(&context()).await.unwrap() {
                    SourceStatus::Actions(actions) => {
                        reconciles += actions
                            .iter()
                            .filter(|a| **a == Action::Reconcile(Gap::OutdatedCursor))
                            .count();
                        source.acknowledge().await.unwrap();
                    }
                    SourceStatus::Waiting => host.advance_ms(source.retry_in_ms().max(1)),
                    other => panic!("unexpected {other:?}"),
                }
            }
            let connections = host.websocket_urls().len();
            assert_eq!(reconciles, 1, "one reconcile per gap streak");
            assert!(
                connections <= 8,
                "{connections} connections in 60 s: the reconnects must back off"
            );
            assert!(source.polling_enabled() || connections > 1);
        });
    }
}

#[test]
fn the_first_switch_to_filtered_is_immediate_and_later_changes_wait() {
    futures::executor::block_on(async {
        use futures::FutureExt;
        let host = BrowserLikeHost::new(MemHost::new());
        let mut source = jetstream_source(host.clone(), "wss://jetstream.example/subscribe", true);
        let mut cx = Context {
            newcomer_forums: BTreeSet::new(),
            ..context()
        };
        assert!(source.next(&cx).now_or_never().is_none());
        assert_eq!(host.urls().len(), 1);
        assert!(!host.urls()[0].contains("dids="), "participants are not known yet");

        cx.participants_known = true;
        cx.filter_dids = vec![DID.to_owned()];
        // Pending, not a CatchUp: the wildcard connection missed nothing.
        assert!(source.next(&cx).now_or_never().is_none());
        let urls = host.urls();
        assert_eq!(urls.len(), 2, "known participants switch to filtered at once");
        assert!(urls[1].contains("dids=did%3Aplc%3Aknown"));
        assert!(host.feed_at(0).closed(), "the wildcard socket is closed");
        assert!(!host.feed_at(1).closed());

        cx.filter_dids.push(OTHER.to_owned());
        assert!(source.next(&cx).now_or_never().is_none());
        assert_eq!(host.urls().len(), 2, "a later change of D is rate-limited");
    });
}

#[test]
fn browser_filtered_mode_drives_a_membership_side_connection() {
    futures::executor::block_on(async {
        use futures::FutureExt;
        let host = BrowserLikeHost::new(MemHost::new());
        let mut source = jetstream_source(host.clone(), "wss://jetstream.example/subscribe", true);
        let mut cx = context();
        cx.participants_known = true;
        cx.filter_dids = vec![DID.to_owned()];
        assert!(source.next(&cx).now_or_never().is_none());
        let urls = host.urls();
        assert_eq!(urls.len(), 2, "{urls:?}");
        let ns = lessdugin_lexicon::ns::NS;
        let side_url = url::Url::parse(&urls[1]).unwrap();
        let pairs: Vec<(String, String)> = side_url.query_pairs().into_owned().collect();
        assert!(pairs.contains(&("collections".into(), format!("{ns}.membership"))));
        assert!(pairs.contains(&("kinds".into(), "commit".into())));
        assert!(!pairs.iter().any(|(k, _)| k == "dids"), "no DID filter");
        assert!(urls[0].contains("dids=did%3Aplc%3Aknown"));

        let main = host.feed("dids=");
        let side = host.feed("kinds=commit");
        // A newcomer's membership arrives only on the side connection.
        side.push(WsMessage::Text(v2_commit(NEW, &membership(), FK, 8)));
        assert!(matches!(
            source.next(&cx).now_or_never(),
            Some(Ok(SourceStatus::Actions(ref a)))
                if matches!(&a[..], [Action::Candidate { did, .. }] if did == NEW)
        ));
        source.acknowledge().await.unwrap();
        // One commit seen on both connections is handled once (the shared LRU).
        let known = v2_commit(DID, &membership(), FK, 9);
        main.push(WsMessage::Text(known.clone()));
        assert!(matches!(
            source.next(&cx).now_or_never(),
            Some(Ok(SourceStatus::Actions(ref a))) if a.len() == 1
        ));
        source.acknowledge().await.unwrap();
        side.push(WsMessage::Text(known));
        assert_eq!(
            source.next(&cx).now_or_never().map(Result::unwrap),
            Some(SourceStatus::Waiting)
        );
        // The main cursor is the main connection's only.
        assert_eq!(source.cursor().unwrap().seq, Some(9));

        // Without newcomer forums the side connection is closed.
        cx.newcomer_forums.clear();
        assert!(source.next(&cx).now_or_never().is_none());
        assert!(side.closed());
        assert!(!main.closed());
        assert_eq!(host.urls().len(), 2);
    });
}
