// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M2 engine end to end over the fake network: three users on three devices (hosts) share
//! one network; each acts through its own `Engine`.
//!
//! alice creates an approval-mode forum, grants bob and carol and posts a welcome; bob joins,
//! posts and comments; carol (granted) comments and replies; alice edits her post (a revision);
//! bob deletes a comment. After a sync every engine's lists and threads reflect it all. The
//! clock is the fake one, so `list_posts` / `get_thread` JSON is snapshotted byte for byte.

mod common;

use std::sync::{Arc, Mutex};

use common::{assert_send, block_on};
use lessdugin_core::dto::{
    BodyDraft, CommentDraft, CommentSort, DefaultTier, Draft, EditOpts, Event, ForumDraft,
    ForumRole, ItemState, ListPostsQuery, MembersQuery, MembershipMode, PostDraft, PostFilter,
    PostKind, PostSort, SyncOpts, ThreadOpts, ThreadRoot, Topic, VersionRef, WriteState,
    WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::AccountHandle;
use lessdugin_testkit::{FakeNet, MemHost};

const MINUTE: i64 = 60_000;

struct User {
    host: MemHost,
    engine: Engine<MemHost>,
    acct: AccountHandle,
    events: Arc<Mutex<Vec<Event>>>,
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
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let sink = Arc::clone(&events);
        engine.subscribe(move |e| sink.lock().unwrap().push(e));
        Self {
            host,
            engine,
            acct,
            events,
        }
    }

    fn did(&self) -> String {
        self.acct.did().to_string()
    }

    /// Runs the outbox until idle; every write must be done.
    fn flush(&self) {
        let r = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!(
            (r.failed, r.conflicts),
            (0, 0),
            "{}: {:?}",
            self.acct.handle(),
            block_on(self.engine.writes()).unwrap()
        );
    }

    /// The uri a done write created.
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
        assert!(!r.partial, "{r:?}");
        assert!(r.errors.is_empty(), "{r:?}");
    }

    fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

struct World {
    alice: User,
    bob: User,
    carol: User,
}

impl World {
    fn new() -> Self {
        let ha = MemHost::new();
        let net = FakeNet::on(&ha);
        let _ = net.default_pds();
        let hb = MemHost::new();
        hb.set_seed(2);
        net.install_as(&hb, "bob");
        let hc = MemHost::new();
        hc.set_seed(3);
        net.install_as(&hc, "carol");
        Self {
            alice: User::new(&net, ha, "alice.example.com"),
            bob: User::new(&net, hb, "bob.example.com"),
            carol: User::new(&net, hc, "carol.example.com"),
        }
    }

    /// Moves every device's clock (the network runs on alice's).
    fn tick(&self, ms: i64) {
        for u in [&self.alice, &self.bob, &self.carol] {
            u.host.advance_ms(ms);
        }
    }
}

struct Scenario {
    w: World,
    forum: String,
    welcome: String,
    bob_post: String,
    bob_c1: String,
    bob_c2: String,
    carol_c1: String,
    carol_c2: String,
}

fn scenario() -> Scenario {
    let w = World::new();
    let (alice, bob, carol) = (&w.alice, &w.bob, &w.carol);

    // alice: an approval-mode forum whose posts start on the frontpage; bob and carol granted
    let mut draft = ForumDraft::new("meta", "Meta");
    draft.description = Some("Talk about the forum.".into());
    draft.about = Some(BodyDraft::text("# Rules\n\nBe kind."));
    draft.default_tier = DefaultTier::Frontpage;
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    assert_eq!(forum, format!("at://{}/{}/meta", alice.did(), nsid::FORUM));
    block_on(alice.engine.grant(&forum, &[bob.did(), carol.did()])).unwrap();
    w.tick(MINUTE);
    let t = block_on(alice.engine.publish(&PostDraft::article(
        &forum,
        "Welcome",
        "Hello **everyone**.",
    )))
    .unwrap();
    alice.flush();
    let welcome = alice.done(&t);

    // bob: opens, joins, posts and comments twice on the welcome post
    w.tick(MINUTE);
    let view = block_on(bob.engine.forum_open(&forum)).unwrap();
    assert_eq!(view.forum.name, "Meta");
    assert_eq!(view.forum.membership, MembershipMode::Approval);
    assert_eq!(view.my_role, ForumRole::None);
    assert!(view.about.unwrap().html.contains("Be kind."));
    block_on(bob.engine.join(&forum, Some("hi"))).unwrap();
    bob.flush();
    bob.sync(&forum);
    w.tick(MINUTE);
    let t = block_on(bob.engine.publish(&PostDraft::article(
        &forum,
        "Hello from bob",
        "My first post, with a [link](https://example.com).",
    )))
    .unwrap();
    bob.flush();
    let bob_post = bob.done(&t);
    w.tick(MINUTE);
    let t = block_on(bob.engine.comment(&CommentDraft::new(
        &welcome,
        None,
        "Thanks for the forum!",
    )))
    .unwrap();
    bob.flush();
    let bob_c1 = bob.done(&t);
    w.tick(MINUTE);
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&welcome, None, "oops, wrong thread")),
    )
    .unwrap();
    bob.flush();
    let bob_c2 = bob.done(&t);

    // carol: never joins explicitly; her first comment carries her membership
    w.tick(MINUTE);
    carol.sync(&forum);
    let t = block_on(
        carol
            .engine
            .comment(&CommentDraft::new(&welcome, None, "Glad to be here.")),
    )
    .unwrap();
    carol.flush();
    let carol_c1 = carol.done(&t);
    w.tick(MINUTE);
    let t = block_on(carol.engine.comment(&CommentDraft::new(
        &welcome,
        Some(bob_c1.clone()),
        "Agreed!",
    )))
    .unwrap();
    carol.flush();
    let carol_c2 = carol.done(&t);

    // alice edits her post (with history: a revision of the old version)
    w.tick(MINUTE);
    alice.sync(&forum);
    let edit = Draft::Post {
        id: None,
        post: PostDraft::article(&forum, "Welcome!", "Hello **everyone**. Read the rules."),
    };
    let opts = EditOpts {
        history: true,
        summary: Some("clarify".into()),
    };
    block_on(alice.engine.edit(&welcome, &edit, &opts)).unwrap();
    alice.flush();

    // bob deletes his second comment
    w.tick(MINUTE);
    block_on(bob.engine.delete(&bob_c2)).unwrap();
    bob.flush();

    // bob's first sync saw carol (granted) before her membership existed: a granted repo is
    // synced without the membership gate, so the next explicit sync picks her up at once (no
    // REJECTED_RECHECK_MS back-off)
    for u in [alice, bob, carol] {
        u.sync(&forum);
    }
    Scenario {
        w,
        forum,
        welcome,
        bob_post,
        bob_c1,
        bob_c2,
        carol_c1,
        carol_c2,
    }
}

#[test]
fn three_users_on_three_devices() {
    let s = scenario();
    let (alice, bob, carol) = (&s.w.alice, &s.w.bob, &s.w.carol);

    for u in [alice, bob, carol] {
        // lists: both posts (the forum's default tier is frontpage), newest first
        let list = block_on(u.engine.list_posts(&ListPostsQuery::new(&s.forum))).unwrap();
        let uris: Vec<&str> = list.items.iter().map(|p| p.uri.as_str()).collect();
        assert_eq!(uris, [s.bob_post.as_str(), s.welcome.as_str()]);
        assert!(!list.completeness.partial, "{:?}", list.completeness);
        let welcome = &list.items[1];
        assert_eq!(welcome.title.as_deref(), Some("Welcome!"));
        assert_eq!(welcome.comment_count, 3);
        assert!(welcome.badges.edited);
        assert_eq!(welcome.state, ItemState::Visible);
        assert_eq!(welcome.excerpt, "Hello everyone. Read the rules.");

        // the thread: carol's reply under bob's comment, the deleted comment gone
        let th = block_on(u.engine.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
        let ThreadRoot::Post(p) = &th.post else {
            panic!("missing root")
        };
        assert_eq!(p.history.revisions, 1);
        assert!(p.body.html.contains("Read the rules."));
        let shape: Vec<(&str, u32, Option<&str>)> = th
            .comments
            .iter()
            .map(|c| (c.uri.as_str(), c.depth, c.parent.as_deref()))
            .collect();
        assert_eq!(
            shape,
            [
                (s.carol_c1.as_str(), 1, None),
                (s.bob_c1.as_str(), 1, None),
                (s.carol_c2.as_str(), 2, Some(s.bob_c1.as_str())),
            ],
            "comment sort magic: no score yet, so newest first"
        );
        // the comments made on the first version say so; the reply is on the current parent
        let versions: Vec<Option<VersionRef>> =
            th.comments.iter().map(|c| c.version.clone()).collect();
        assert_eq!(
            versions,
            [
                Some(VersionRef::Revision { revision: 1 }),
                Some(VersionRef::Revision { revision: 1 }),
                None
            ]
        );
        assert!(th.comments.iter().all(|c| c.state == ItemState::Visible));
        let old = block_on(u.engine.get_thread(
            &s.welcome,
            &ThreadOpts {
                sort: CommentSort::Old,
                unread_only: false,
            },
        ))
        .unwrap();
        assert_eq!(old.comments[0].uri, s.bob_c1);

        // history: two versions; the members; the deleted comment is a tombstone
        let h = block_on(u.engine.history(&s.welcome)).unwrap();
        assert_eq!(h.versions.len(), 2);
        assert_eq!(h.versions[1].summary.as_deref(), Some("clarify"));
        let members = block_on(u.engine.members(&s.forum, &MembersQuery::default())).unwrap();
        let mut roles: Vec<(String, ForumRole)> = members
            .iter()
            .map(|m| (m.actor.did.clone(), m.status))
            .collect();
        roles.sort();
        let mut want = vec![
            (alice.did(), ForumRole::Owner),
            (bob.did(), ForumRole::Member),
            (carol.did(), ForumRole::Member),
        ];
        want.sort();
        assert_eq!(roles, want);
        let raw = block_on(u.engine.raw(&s.bob_c2)).unwrap();
        assert!(raw.record.is_none() && raw.cid.is_none(), "{raw:?}");
        assert!(block_on(u.engine.get_post(&s.bob_post)).is_ok());
    }

    // filters and sorts
    let q = |sort, filter| ListPostsQuery {
        sort,
        filter,
        ..ListPostsQuery::new(&s.forum)
    };
    let old = block_on(carol.engine.list_posts(&q(PostSort::Old, PostFilter::All))).unwrap();
    assert_eq!(old.items[0].uri, s.welcome);
    let curated = block_on(
        carol
            .engine
            .list_posts(&q(PostSort::New, PostFilter::Curated)),
    )
    .unwrap();
    assert!(curated.items.is_empty());
    let by_bob = ListPostsQuery {
        author: Some(bob.did()),
        kind: Some(PostKind::Article),
        ..ListPostsQuery::new(&s.forum)
    };
    let by_bob = block_on(carol.engine.list_posts(&by_bob)).unwrap();
    assert_eq!(by_bob.items.len(), 1);
    let page = ListPostsQuery {
        limit: 1,
        ..ListPostsQuery::new(&s.forum)
    };
    let first = block_on(carol.engine.list_posts(&page)).unwrap();
    assert_eq!(first.cursor.as_deref(), Some("1"));
    let second = block_on(carol.engine.list_posts(&ListPostsQuery {
        cursor: first.cursor.clone(),
        ..page
    }))
    .unwrap();
    assert_eq!((second.items.len(), second.cursor.as_deref()), (1, None));

    // the followed list: alice created, bob joined, carol's comes from her membership record
    for u in [alice, bob, carol] {
        let f = block_on(u.engine.followed()).unwrap();
        assert_eq!(f.len(), 1, "{}", u.acct.handle());
        assert_eq!(f[0].uri, s.forum);
        assert_eq!(f[0].name, "Meta");
        assert!(f[0].member);
    }
    block_on(carol.engine.forum_unfollow(&s.forum)).unwrap();
    assert!(block_on(carol.engine.followed()).unwrap().is_empty());
    block_on(carol.engine.forum_follow(&s.forum, Some("meta"))).unwrap();
    assert_eq!(
        block_on(carol.engine.followed()).unwrap()[0]
            .alias
            .as_deref(),
        Some("meta")
    );

    // sync_thread: the post and its participants; a comment's uri opens its thread
    let r = block_on(carol.engine.sync_thread(&s.welcome, &SyncOpts::default())).unwrap();
    assert_eq!((r.repos_total, r.repos_complete), (3, 3), "{r:?}");
    assert!(r.errors.is_empty() && !r.partial, "{r:?}");
    let via_comment =
        block_on(carol.engine.get_thread(&s.carol_c2, &ThreadOpts::default())).unwrap();
    assert_eq!(via_comment.comments.len(), 3);
    let err = block_on(carol.engine.get_thread(&s.bob_c2, &ThreadOpts::default())).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // the forum header from bob's side
    let v = block_on(bob.engine.forum_open(&s.forum)).unwrap();
    assert_eq!(v.my_role, ForumRole::Member);
    assert!(v.forum.member && v.forum.followed);
}

#[test]
fn events_report_writes_syncs_and_changes() {
    let s = scenario();
    let bob = &s.w.bob;
    let events = bob.take_events();
    let json: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    // every write went pending → sending → done
    let delete_id = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Write { id, .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap();
    let states: Vec<&WriteState> = events
        .iter()
        .filter_map(|e| match e {
            Event::Write { id, state } if *id == delete_id => Some(state),
            _ => None,
        })
        .collect();
    assert!(matches!(states[0], WriteState::Pending { .. }), "{json:#?}");
    assert!(matches!(states.last().unwrap(), WriteState::Done { .. }));
    // the applied delete changed the forum and the thread
    let changed_after_delete = events.iter().any(|e| match e {
        Event::Changed { topics } => {
            topics.contains(&Topic::Forum {
                uri: s.forum.clone(),
            }) && topics.contains(&Topic::Thread {
                uri: s.welcome.clone(),
            })
        }
        _ => false,
    });
    assert!(changed_after_delete, "{json:#?}");
    assert!(events.iter().any(|e| matches!(e,
        Event::SyncProgress { forum, done, total } if *forum == s.forum && done == total && *total == 3
    )));

    // a callback may call back into the engine; a later subscription sees later events only
    let seen = Arc::new(Mutex::new(0u32));
    let (e2, n) = (bob.engine.clone(), Arc::clone(&seen));
    let id = bob.engine.subscribe(move |ev| {
        if let Event::Changed { .. } = ev {
            *n.lock().unwrap() += 1;
            // re-entrant use of the engine is fine: nothing is delivered until this returns
            e2.unsubscribe(lessdugin_core::engine::SubscriptionId(u64::MAX));
        }
    });
    block_on(bob.engine.forum_follow(&s.forum, None)).unwrap();
    assert_eq!(*seen.lock().unwrap(), 1);
    assert!(bob.engine.unsubscribe(id));
}

/// An engine over `u`'s device in parity mode (`engine-api.md` §2.1) at `now_ms`.
fn parity_engine(u: &User, now_ms: i64) -> Engine<MemHost> {
    Engine::new(
        u.host.clone(),
        EngineConfig {
            parity_now_ms: Some(now_ms),
            ..EngineConfig::default()
        },
    )
}

#[test]
fn snapshots_are_deterministic() {
    let s = scenario();
    // parity mode: the handle state (carol's own is verified on her device since her login)
    // and the clock do not depend on the device
    let fixed = s.w.alice.host.now() + MINUTE;
    let carol = parity_engine(&s.w.carol, fixed);
    let list = block_on(carol.list_posts(&ListPostsQuery::new(&s.forum))).unwrap();
    let thread = block_on(carol.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    insta::assert_json_snapshot!("list_posts", list);
    insta::assert_json_snapshot!("get_thread", thread);
    // the same bytes from another device after its own sync (cross-client parity)
    let alice = parity_engine(&s.w.alice, fixed);
    let a_list = block_on(alice.list_posts(&ListPostsQuery::new(&s.forum))).unwrap();
    let a_thread = block_on(alice.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    assert_eq!(
        serde_json::to_string(&a_list).unwrap(),
        serde_json::to_string(&list).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&a_thread).unwrap(),
        serde_json::to_string(&thread).unwrap()
    );
}

#[test]
fn writes_need_an_account_and_publish_media() {
    let host = MemHost::new();
    let _net = FakeNet::on(&host);
    let e = Engine::new(host.clone(), EngineConfig::default());
    let f = "at://did:plc:aaaaaaaaaaaaaaaaaaaaaaaa/online.7778777.lessdugin.forum/meta";
    let err = block_on(e.publish(&PostDraft::article(f, "t", "b"))).unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthRequired);
    let err = block_on(e.list_posts(&ListPostsQuery::new("not a forum"))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);

    let s = scenario();
    let bob = &s.w.bob;
    // M4: a body with media publishes now. The record carries a placeholder blob object for
    // each item, and the outbox substitutes what `uploadBlob` returns, failing the write when
    // the returned `ref.$link` is not the CID computed locally (data-model.md §8.8).
    // a real CID, from the pipeline, so the record validates (G2: links must be CIDs)
    let png: &[u8] =
        b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00";
    let prepared = lessdugin_core::media::prepare(png.to_vec()).unwrap();
    let cid = lessdugin_atp::cid::for_raw(&prepared).to_string();
    let mut d = PostDraft::article(
        &s.forum,
        "Post with an image",
        format!("See ![x](cid:{cid})"),
    );
    d.body.media.push(lessdugin_core::dto::MediaRef {
        id: cid.clone(),
        cid: cid.clone(),
        mime: "image/png".into(),
        size_bytes: 1,
        alt: Some("an image".into()),
        aspect_ratio: None,
        duration_ms: None,
        name: None,
    });
    let t = block_on(bob.engine.publish(&d)).unwrap();
    assert!(!t.id.is_empty());
    // the flusher uploads the blob first. FakePds holds no blob for this synthetic CID, so
    // the write ends `Failed` with `mediaRejected` - which is exactly the guard working: a
    // mismatched or missing blob refuses the write rather than storing a dangling ref.
    let r = block_on(bob.engine.outbox_flush()).unwrap();
    assert_eq!(r.failed, 1, "{r:?}");
    let tickets = block_on(bob.engine.writes()).unwrap();
    let failed = tickets
        .iter()
        .find(|w| w.id == t.id)
        .expect("the ticket is still listed");
    assert!(
        matches!(&failed.state, WriteState::Failed { .. }),
        "{:?}",
        failed.state
    );
    // W9: bob's engine cannot write to carol's repo
    let err = block_on(bob.engine.retract(&s.carol_c1)).unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthRequired);
    // a held write can be undone before it is sent
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&s.bob_post, None, "draft")),
    )
    .unwrap();
    let u = block_on(bob.engine.undo(&t.id)).unwrap();
    assert_eq!(u.state, WriteState::Undone);
    bob.flush();

    let q = ListPostsQuery::new(&s.forum);
    assert_send(&bob.engine.list_posts(&q));
    assert_send(&bob.engine.get_thread(&s.welcome, &ThreadOpts::default()));
    assert_send(&bob.engine.publish(&d));
    assert_send(&bob.engine.sync_forum(&s.forum, &SyncOpts::default()));
    assert_send(&bob.engine.outbox_pump());
    assert_send(&bob.engine);
}
