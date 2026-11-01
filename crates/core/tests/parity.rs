// SPDX-License-Identifier: AGPL-3.0-or-later
//! Parity mode (`engine-api.md` §2.1): with `EngineConfig::parity_now_ms`, two devices that
//! admitted the same records at different times produce byte-identical `list_posts` and
//! `get_thread` JSON.
//!
//! bob's device clock runs 30 minutes ahead, so his post is future-dated for a device that sees
//! it at once (carol) and not for one that sees it an hour later (alice): without parity mode
//! their views differ (the `futureDated` badge and `tRankMs`); with it they are the same bytes,
//! also when the two queries run at different host times.

mod common;

use common::block_on;
use lessdugin_core::dto::{
    CommentDraft, DefaultTier, ForumDraft, ListPostsQuery, PostDraft, PostFilter, PostList,
    PostSort, SyncOpts, ThreadOpts, ThreadView, WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig};
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost};

const MINUTE: i64 = 60_000;

struct User {
    host: MemHost,
    engine: Engine<MemHost>,
    acct: AccountHandle,
}

impl User {
    fn new(net: &FakeNet, host: MemHost, handle: &str) -> Self {
        let acct = net.add_account(handle);
        let engine = Engine::new(host.clone(), EngineConfig::default());
        block_on(engine.add_account(AccountInfo::app_password(
            acct.did().as_str(),
            acct.pds_base(),
        )))
        .unwrap();
        Self { host, engine, acct }
    }

    /// An engine over this device's stores in parity mode at `now_ms`.
    fn parity(&self, now_ms: i64) -> Engine<MemHost> {
        Engine::new(
            self.host.clone(),
            EngineConfig {
                parity_now_ms: Some(now_ms),
                ..EngineConfig::default()
            },
        )
    }

    fn flush(&self) {
        let r = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!((r.failed, r.conflicts), (0, 0), "{}", self.acct.handle());
    }

    fn done(&self, t: &WriteTicket) -> String {
        match block_on(self.engine.write_status(&t.id))
            .unwrap()
            .unwrap()
            .state
        {
            WriteState::Done { uri, .. } => uri,
            other => panic!("write {} is {other:?}", t.id),
        }
    }

    fn sync(&self, forum: &str) {
        let r = block_on(self.engine.sync_forum(forum, &SyncOpts::default())).unwrap();
        assert!(!r.partial && r.errors.is_empty(), "{r:?}");
    }
}

fn views(e: &Engine<MemHost>, forum: &str, post: &str) -> (String, String) {
    let q = ListPostsQuery {
        sort: PostSort::Magic,
        filter: PostFilter::All,
        ..ListPostsQuery::new(forum)
    };
    let list: PostList = block_on(e.list_posts(&q)).unwrap();
    let thread: ThreadView = block_on(e.get_thread(post, &ThreadOpts::default())).unwrap();
    (
        serde_json::to_string(&list).unwrap(),
        serde_json::to_string(&thread).unwrap(),
    )
}

#[test]
fn parity_mode_fixes_now_and_obs() {
    let ha = MemHost::new();
    let net = FakeNet::on(&ha);
    let _ = net.default_pds();
    let hb = MemHost::new();
    hb.set_seed(2);
    net.install_as(&hb, "bob");
    let hc = MemHost::new();
    hc.set_seed(3);
    net.install_as(&hc, "carol");
    let alice = User::new(&net, ha, "alice.example.com");
    let bob = User::new(&net, hb, "bob.example.com");
    let carol = User::new(&net, hc, "carol.example.com");
    let start = alice.host.now();

    // alice: an approval-mode forum whose posts start on the frontpage, bob granted, and a
    // welcome post
    let mut draft = ForumDraft::new("meta", "Meta");
    draft.default_tier = DefaultTier::Frontpage;
    draft.report_contact = Some("mailto:mods@example.com".into());
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    block_on(alice.engine.grant(&forum, &[bob.acct.did().to_string()])).unwrap();
    for u in [&alice, &bob, &carol] {
        u.host.advance_ms(MINUTE);
    }
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "Welcome", "Hello.")),
    )
    .unwrap();
    alice.flush();
    let welcome = alice.done(&t);

    // bob, whose clock is 30 minutes fast: a post and a comment from "the future"
    bob.host.advance_ms(31 * MINUTE);
    block_on(bob.engine.forum_open(&forum)).unwrap();
    let t = block_on(bob.engine.publish(&PostDraft::article(
        &forum,
        "Early bird",
        "Posted ahead.",
    )))
    .unwrap();
    bob.flush();
    let bob_post = bob.done(&t);
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&welcome, None, "First!")),
    )
    .unwrap();
    bob.flush();
    let bob_comment = bob.done(&t);

    // carol sees it all at once (bob's records are future-dated for her); alice an hour later
    carol.sync(&forum);
    for u in [&alice, &carol] {
        u.host.advance_ms(60 * MINUTE);
    }
    alice.sync(&forum);

    // without parity mode the two devices disagree
    let (c_list, c_thread) = views(&carol.engine, &forum, &welcome);
    let (a_list, a_thread) = views(&alice.engine, &forum, &welcome);
    assert_ne!(
        c_list, a_list,
        "the scenario must make the plain views differ"
    );
    assert!(c_list.contains("futureDated"), "{c_list}");
    assert!(!a_list.contains("futureDated"), "{a_list}");
    assert_ne!(c_thread, a_thread);

    // in parity mode they are the same bytes, whenever each device queries
    let fixed = start + 24 * 60 * MINUTE;
    let (c_list, c_thread) = views(&carol.parity(fixed), &forum, &welcome);
    carol.host.advance_ms(7 * MINUTE);
    alice.host.advance_ms(90 * MINUTE);
    let (a_list, a_thread) = views(&alice.parity(fixed), &forum, &welcome);
    assert_eq!(c_list, a_list);
    assert_eq!(c_thread, a_thread);
    assert!(!c_list.contains("futureDated"), "{c_list}");
    // shown authors' handles are verified by the query itself, not by `live_tail`
    assert!(!c_thread.contains("unverified"), "{c_thread}");
    assert!(c_list.contains(&bob_post) && c_thread.contains("First!"));
    // the DTO additions: a comment's thread root, the forum's report contact
    let raw = block_on(carol.engine.raw(&bob_comment)).unwrap();
    assert_eq!(raw.root.as_deref(), Some(welcome.as_str()));
    assert!(c_thread.contains(&format!(r#""root":"{welcome}""#)));
    let v = block_on(carol.engine.forum_open(&forum)).unwrap();
    assert_eq!(
        v.forum.report_contact.as_deref(),
        Some("mailto:mods@example.com")
    );
    // and again later on the same device (no host-clock dependence)
    carol.host.advance_ms(3 * 60 * MINUTE);
    assert_eq!(
        views(&carol.parity(fixed), &forum, &welcome),
        (c_list, c_thread)
    );
}

/// Parity mode ignores the device's read marks (`engine-api.md` §2.1, decision O-13): which
/// threads a device visited is not a function of the synced records, so a device that visited a
/// thread and one that did not must still produce the same bytes. Without parity mode the same
/// mark counts the unread comments as before.
#[test]
fn parity_mode_ignores_read_marks() {
    let ha = MemHost::new();
    let net = FakeNet::on(&ha);
    let _ = net.default_pds();
    let hb = MemHost::new();
    hb.set_seed(2);
    net.install_as(&hb, "bob");
    let alice = User::new(&net, ha, "alice.example.com");
    let bob = User::new(&net, hb, "bob.example.com");
    let start = alice.host.now();
    let tick = || {
        for u in [&alice, &bob] {
            u.host.advance_ms(MINUTE);
        }
    };

    // alice's forum and welcome post; bob granted, who comments
    let mut draft = ForumDraft::new("meta", "Meta");
    draft.default_tier = DefaultTier::Frontpage;
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    block_on(alice.engine.grant(&forum, &[bob.acct.did().to_string()])).unwrap();
    tick();
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "Welcome", "Hello.")),
    )
    .unwrap();
    alice.flush();
    let welcome = alice.done(&t);
    block_on(bob.engine.forum_open(&forum)).unwrap();
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&welcome, None, "First!")),
    )
    .unwrap();
    bob.flush();
    bob.done(&t);

    // alice visits the thread (her mark passes bob's first comment), then bob comments again
    tick();
    alice.sync(&forum);
    block_on(alice.engine.mark_read(&welcome)).unwrap();
    tick();
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&welcome, None, "Second.")),
    )
    .unwrap();
    bob.flush();
    let second = bob.done(&t);
    tick();
    alice.sync(&forum);
    bob.sync(&forum);

    // without parity mode alice's mark counts: one unread comment
    let q = ListPostsQuery {
        sort: PostSort::Magic,
        filter: PostFilter::All,
        ..ListPostsQuery::new(&forum)
    };
    let list: PostList = block_on(alice.engine.list_posts(&q)).unwrap();
    let item = list.items.iter().find(|p| p.uri == welcome).unwrap();
    assert_eq!(item.unread_comments, Some(1), "{item:?}");
    let thread: ThreadView =
        block_on(alice.engine.get_thread(&welcome, &ThreadOpts::default())).unwrap();
    let unread: Vec<&str> = thread
        .comments
        .iter()
        .filter(|c| c.unread)
        .map(|c| c.uri.as_str())
        .collect();
    assert_eq!(unread, vec![second.as_str()], "{thread:?}");

    // in parity mode the mark is ignored: the views are those of a device with no marks
    let fixed = start + 24 * 60 * MINUTE;
    let e = alice.parity(fixed);
    let list: PostList = block_on(e.list_posts(&q)).unwrap();
    let item = list.items.iter().find(|p| p.uri == welcome).unwrap();
    assert_eq!(item.unread_comments, None, "{item:?}");
    let thread: ThreadView = block_on(e.get_thread(&welcome, &ThreadOpts::default())).unwrap();
    assert!(thread.comments.len() == 2, "{thread:?}");
    assert!(thread.comments.iter().all(|c| !c.unread), "{thread:?}");
    let (a_list, a_thread) = views(&e, &forum, &welcome);
    assert!(!a_list.contains("unreadComments"), "{a_list}");
    assert!(!a_thread.contains("unreadComments"), "{a_thread}");
    // so alice, who visited the thread, and bob, who never did, agree byte for byte
    assert_eq!(
        (a_list, a_thread),
        views(&bob.parity(fixed), &forum, &welcome)
    );
    // and the mark itself is kept: it counts again outside parity mode
    let list: PostList = block_on(alice.engine.list_posts(&q)).unwrap();
    let item = list.items.iter().find(|p| p.uri == welcome).unwrap();
    assert_eq!(item.unread_comments, Some(1), "{item:?}");
}
