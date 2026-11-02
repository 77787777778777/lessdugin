// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search (`sync.md` §13, `lfm-1.md` §12): snippets never show spoilers, and matching
//! costs one pass over hostile bodies.

mod common;

use std::time::Duration;

use common::block_on;
use lessdugin_core::dto::{
    CommentDraft, ForumDraft, MembershipMode, PostDraft, SearchItem, SearchQuery, SearchResults,
    SyncOpts, WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig};
use lessdugin_core::search::snippet;
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
    assert!(!format!("{recent:?}").contains(&comment), "Recent Discussion omits it");
    assert!(hit_of(&w.bob.search(&w.forum, "orphanword"), &comment).is_none());
    assert!(!user().recent_comments.iter().any(|c| c.uri == comment));
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
