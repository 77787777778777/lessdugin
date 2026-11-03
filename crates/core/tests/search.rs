// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search (`sync.md` §13, `lfm-1.md` §12): snippets never show spoilers, and matching
//! costs one pass over hostile bodies.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::block_on;
use lessdugin_atp::syntax::Did;
use lessdugin_core::dto::{
    CommentDraft, ForumDraft, MembershipMode, PostDraft, SearchItem, SearchQuery, SearchResults,
    SyncOpts, WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig};
use lessdugin_core::host::{
    BlobHandle, Dir, Host, HostError, HttpRequest, HttpResponse, KeyRange, KvOp, KvStore,
};
use lessdugin_core::search::snippet;
use lessdugin_core::store::mem::MemKv;
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost};

const MINUTE: i64 = 60_000;

struct Device {
    host: MemHost,
    engine: Engine<MemHost>,
    account: AccountHandle,
}

impl Device {
    fn new(host: MemHost, account: AccountHandle) -> Self {
        let engine = Engine::new(host.clone(), EngineConfig::default());
        block_on(engine.add_account(AccountInfo::app_password(
            account.did().as_str(),
            account.pds_base(),
        )))
        .unwrap();
        Self {
            host,
            engine,
            account,
        }
    }

    fn flush(&self) {
        let report = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!((report.failed, report.conflicts), (0, 0));
    }

    fn done(&self, ticket: &WriteTicket) -> String {
        self.flush();
        match block_on(self.engine.write_status(&ticket.id))
            .unwrap()
            .unwrap()
            .state
        {
            WriteState::Done { uri, .. } => uri,
            state => panic!("write is {state:?}"),
        }
    }

    fn sync(&self, forum: &str) {
        let report = block_on(self.engine.sync_forum(forum, &SyncOpts::default())).unwrap();
        assert!(!report.partial, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
    }

    fn post(&self, forum: &str, title: &str, body: &str) -> String {
        let ticket =
            block_on(self.engine.publish(&PostDraft::article(forum, title, body))).unwrap();
        self.done(&ticket)
    }

    fn comment(&self, root: &str, body: &str) -> String {
        let ticket = block_on(self.engine.comment(&CommentDraft::new(root, None, body))).unwrap();
        self.done(&ticket)
    }

    fn search(&self, forum: &str, q: &str) -> SearchResults {
        block_on(self.engine.search(&SearchQuery {
            q: q.into(),
            forum: Some(forum.to_owned()),
            author: None,
            limit: 100,
        }))
        .unwrap()
    }
}

/// Alice owns an open forum that Bob joined.
struct World {
    alice: Device,
    bob: Device,
    forum: String,
}

impl World {
    fn new() -> Self {
        let host = MemHost::new();
        let net = FakeNet::on(&host);
        let _ = net.default_pds();
        let bob_host = MemHost::new();
        bob_host.set_seed(2);
        net.install_as(&bob_host, "bob");
        let alice = Device::new(host, net.add_account("alice.example.com"));
        let bob = Device::new(bob_host, net.add_account("bob.example.com"));
        let mut forum = ForumDraft::new("meta", "Meta");
        forum.membership = MembershipMode::Open;
        let ticket = block_on(alice.engine.forum_create(&forum)).unwrap();
        let forum = alice.done(&ticket);
        block_on(bob.engine.forum_open(&forum)).unwrap();
        let ticket = block_on(bob.engine.join(&forum, None)).unwrap();
        bob.done(&ticket);
        let world = Self { alice, bob, forum };
        world.tick();
        world
    }

    fn tick(&self) {
        for d in [&self.alice, &self.bob] {
            d.host.advance_ms(MINUTE);
        }
        for d in [&self.alice, &self.bob] {
            d.sync(&self.forum);
        }
    }
}

/// The hit of `uri` in `results`: its snippet and highlights.
fn hit_of(results: &SearchResults, uri: &str) -> Option<(String, Vec<(u32, u32)>)> {
    results.hits.iter().find_map(|h| {
        let item = match &h.item {
            SearchItem::Post(p) => &p.uri,
            SearchItem::Comment(c) => &c.uri,
        };
        (item == uri).then(|| (h.snippet.clone(), h.highlights.clone()))
    })
}

/// The highlighted text of a snippet.
fn highlighted(snippet: &str, highlights: &[(u32, u32)]) -> Vec<String> {
    let chars: Vec<char> = snippet.chars().collect();
    highlights
        .iter()
        .map(|&(s, e)| chars[s as usize..e as usize].iter().collect())
        .collect()
}

/// Snippets and highlights come from the title and P_x, never from spoiler content (O-1,
/// `lfm-1.md` §12; m5-acceptance-1). Matching still reads P, so a word found only inside a
/// spoiler gives the hit with the start of title + P_x and no highlight.
#[test]
fn snippets_never_show_spoiler_text() {
    let w = World::new();
    let only = w.alice.post(
        &w.forum,
        "Mystery",
        "> [!spoiler]\n> the butler zqspoiler did it\n\nvisible opening words\n",
    );
    let both = w.alice.post(
        &w.forum,
        "Plot",
        "> [!spoiler]\n> secret plotword ending\n\nThe visible plotword here.\n",
    );
    w.tick();
    let comment = w.bob.comment(
        &both,
        "> [!spoiler]\n> hidden twist zqcomment\n\nshown zqcomment text",
    );
    w.tick();

    for reader in [&w.alice, &w.bob] {
        let results = reader.search(&w.forum, "ZQSPOILER");
        let (text, highlights) = hit_of(&results, &only).expect("a spoiler-only match is a hit");
        assert_eq!(text, "Mystery visible opening words");
        assert!(highlights.is_empty(), "{highlights:?}");

        let results = reader.search(&w.forum, "plotword");
        let (text, highlights) = hit_of(&results, &both).expect("a visible match");
        assert_eq!(text, "Plot The visible plotword here.");
        assert!(!text.contains("secret"), "{text}");
        assert_eq!(highlighted(&text, &highlights), ["plotword"]);
        assert_eq!(highlights, [(17, 25)]);

        for q in ["butler", "secret", "twist"] {
            for h in &reader.search(&w.forum, q).hits {
                for word in ["butler", "secret", "twist", "hidden"] {
                    assert!(!h.snippet.contains(word), "{:?}", h.snippet);
                }
            }
        }
    }

    // A term only in the spoiler and one in the visible text: AND over P, snippet of P_x.
    let results = w.bob.search(&w.forum, "twist shown");
    let (text, highlights) = hit_of(&results, &comment).expect("both terms are in P");
    assert_eq!(text, "shown zqcomment text");
    assert_eq!(highlighted(&text, &highlights), ["shown"]);
    let results = w.bob.search(&w.forum, "zqcomment");
    let (text, highlights) = hit_of(&results, &comment).unwrap();
    assert_eq!(text, "shown zqcomment text");
    assert_eq!(highlights, [(6, 15)]);
    let results = w.bob.search(&w.forum, "twist");
    let (text, highlights) = hit_of(&results, &comment).expect("a spoiler-only comment match");
    assert_eq!(
        (text.as_str(), highlights.len()),
        ("shown zqcomment text", 0)
    );
}

/// A cache store that counts the keys read with `get_many` (record bodies are read so).
#[derive(Clone)]
struct CountingKv {
    inner: MemKv,
    keys: Arc<AtomicUsize>,
}

impl KvStore for CountingKv {
    async fn get_many(&self, keys: Vec<Vec<u8>>) -> Result<Vec<Option<Vec<u8>>>, HostError> {
        self.keys.fetch_add(keys.len(), Ordering::SeqCst);
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
        self.inner.write(batch).await
    }
}

/// A [`MemHost`] whose cache store counts its point reads.
#[derive(Clone)]
struct CountingHost {
    inner: MemHost,
    cache: CountingKv,
    state: CountingKv,
}

impl CountingHost {
    fn new(inner: &MemHost) -> Self {
        let kv = |kv: &MemKv| CountingKv {
            inner: kv.clone(),
            keys: Arc::default(),
        };
        Self {
            cache: kv(inner.cache_kv()),
            state: kv(inner.state_kv()),
            inner: inner.clone(),
        }
    }

    /// Keys read since the last call.
    fn take_keys(&self) -> usize {
        self.cache.keys.swap(0, Ordering::SeqCst)
    }
}

impl Host for CountingHost {
    type Kv = CountingKv;
    type Ws = <MemHost as Host>::Ws;
    type Lock = <MemHost as Host>::Lock;

    fn cache(&self) -> &CountingKv {
        &self.cache
    }

    fn state(&self) -> &CountingKv {
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

/// Each forum's in-memory inverted index (`sync.md` §13; m5-acceptance-14) is built once per
/// model from every listed body; a search then reads and renders only its candidates, and a new
/// model (here after a sync) gets a new index.
#[test]
fn search_reads_only_the_candidates_of_the_forum_index() {
    let w = World::new();
    for i in 0..40 {
        w.alice.post(
            &w.forum,
            &format!("Post {i}"),
            &format!("Common words {i}."),
        );
    }
    let rare = w.alice.post(&w.forum, "Rare", "A zqrareword appears once.");
    w.tick();
    let host = CountingHost::new(&w.alice.host);
    let engine = Engine::new(host.clone(), EngineConfig::default());
    let search = |q: &str| {
        let results = block_on(engine.search(&SearchQuery {
            q: q.into(),
            forum: Some(w.forum.clone()),
            author: None,
            limit: 100,
        }))
        .unwrap();
        let mut uris: Vec<String> = results
            .hits
            .iter()
            .map(|h| match &h.item {
                SearchItem::Post(p) => p.uri.clone(),
                SearchItem::Comment(c) => c.uri.clone(),
            })
            .collect();
        uris.sort();
        (uris, host.take_keys())
    };

    let (hits, built) = search("zqrareword");
    assert_eq!(hits, std::slice::from_ref(&rare));
    assert!(
        built > 41,
        "the index reads every listed body once ({built} keys)"
    );
    let (hits, read) = search("ZQRAREWORD");
    assert_eq!(hits, std::slice::from_ref(&rare));
    assert!(
        read < 10,
        "only the candidate is read and rendered ({read} keys)"
    );
    let (hits, read) = search("zqnowhere");
    assert!(hits.is_empty());
    assert!(read < 10, "no candidate, no body ({read} keys)");
    let (hits, read) = search("common");
    assert_eq!(hits.len(), 40);
    assert!((40..60).contains(&read), "{read} keys");

    let second = w.alice.post(&w.forum, "More", "Another zqrareword.");
    block_on(engine.sync_forum(&w.forum, &SyncOpts::default())).unwrap();
    host.take_keys();
    let (hits, rebuilt) = search("zqrareword");
    let mut expected = vec![rare, second];
    expected.sort();
    assert_eq!(hits, expected);
    assert!(
        rebuilt > 42,
        "the new model is indexed again ({rebuilt} keys)"
    );
}

/// Search and the user page leave out comments under a Missing root, as lists and Recent
/// Discussion do (`social.md` §11; security-web-4): here, after the root post is deleted.
#[test]
fn comments_under_a_missing_root_are_neither_found_nor_listed() {
    let w = World::new();
    let root = w.alice.post(&w.forum, "Root", "The root post.");
    w.tick();
    let comment = w.bob.comment(&root, "An orphanword reply.");
    w.tick();
    let bob = w.bob.account.did().to_string();
    let user = || block_on(w.bob.engine.user(Some(&w.forum), &bob)).unwrap();
    assert!(hit_of(&w.bob.search(&w.forum, "orphanword"), &comment).is_some());
    assert!(user().recent_comments.iter().any(|c| c.uri == comment));

    let ticket = block_on(w.alice.engine.delete(&root)).unwrap();
    w.alice.done(&ticket);
    w.tick();
    let recent = block_on(w.bob.engine.recent_discussion(&w.forum, None, 100)).unwrap();
    assert!(
        !format!("{recent:?}").contains(&comment),
        "Recent Discussion omits it"
    );
    assert!(hit_of(&w.bob.search(&w.forum, "orphanword"), &comment).is_none());
    assert!(!user().recent_comments.iter().any(|c| c.uri == comment));
}

/// A spilled body whose source is not in memory (not fetched yet, or not read by a view since)
/// is matched and shown by the record's own excerpt, as lists show it, and by the source's P and
/// P_x once a view has read it (`lfm-1.md` §12; the verifier found the text said the excerpt
/// was cut from P_x).
#[test]
fn a_pending_spilled_body_is_searched_by_its_excerpt() {
    use lessdugin_core::limits::BODY_BLOB_RETRY_MS;
    use lessdugin_lexicon::collection::nsid;
    let w = World::new();
    let at = w.bob.host.now_ms();
    let blob = w.bob.account.upload_blob(
        b"# Whole body\n\nOnly the zqsource word.\n".to_vec(),
        "text/markdown",
    );
    let created = lessdugin_atp::syntax::Datetime::from_millis(at).unwrap();
    let value = serde_json::json!({
        "$type": nsid::POST, "forum": w.forum, "title": "Long",
        "body": {"$type": nsid::MARKDOWN, "profile": "lfm/1", "textBlob": blob},
        "excerpt": "The zqexcerpt words", "createdAt": created.as_str(),
    });
    let tid = lessdugin_testkit::repo::tid_at_ms(at);
    let rkey = lessdugin_lexicon::keys::post_rkey(&w.forum, tid.as_str()).unwrap();
    w.bob.account.put(nsid::POST, &rkey, &value);
    let uri = w.bob.account.uri(nsid::POST, &rkey);
    w.tick();

    let alice = &w.alice;
    alice.host.hold_blob_fetches(true);
    let (text, highlights) =
        hit_of(&alice.search(&w.forum, "zqexcerpt"), &uri).expect("the excerpt is matched");
    assert_eq!(text, "Long The zqexcerpt words");
    assert_eq!(highlighted(&text, &highlights), ["zqexcerpt"]);
    assert!(hit_of(&alice.search(&w.forum, "zqsource"), &uri).is_none());

    alice.host.hold_blob_fetches(false);
    alice.host.advance_ms(BODY_BLOB_RETRY_MS + 1);
    // search reads no blob itself: a view does
    assert!(hit_of(&alice.search(&w.forum, "zqsource"), &uri).is_none());
    let post = block_on(alice.engine.get_post(&uri)).unwrap();
    assert_eq!(post.body.text, "Whole body Only the zqsource word.");
    let (text, highlights) =
        hit_of(&alice.search(&w.forum, "zqsource"), &uri).expect("the fetched source");
    assert_eq!(text, "Long Whole body Only the zqsource word.");
    assert_eq!(highlighted(&text, &highlights), ["zqsource"]);
    assert!(hit_of(&alice.search(&w.forum, "zqexcerpt"), &uri).is_none());
}

/// Matching and snippets are linear in the body (security-web-1, robustness-7). A body made of
/// one short word repeated used to cost a count of the whole folded prefix per match, so k
/// matches cost O(k · |P|) (78 ms for a 100 kB comment, 7.8 s for a 1 MB post). Eight times the
/// input must cost about eight times as much, not 64 (bound generous for noise).
#[test]
#[allow(clippy::disallowed_methods)] // timing a test is fine; the crate itself never reads clocks
fn matching_a_repeated_word_is_linear() {
    let time = |times: usize| {
        let body = "a ".repeat(times);
        (0..3)
            .map(|_| {
                let t = std::time::Instant::now();
                let (text, highlights) = snippet(&body, "A").expect("a hit");
                let el = t.elapsed();
                assert_eq!(text.chars().count(), 161);
                assert_eq!(highlights.len(), 80);
                el
            })
            .min()
            .expect("three runs")
    };
    let (small, large) = (time(50_000), time(400_000));
    assert!(
        large < small * 24 + Duration::from_millis(250),
        "100 kB took {small:?}, 800 kB took {large:?}"
    );
}
