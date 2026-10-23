// SPDX-License-Identifier: AGPL-3.0-or-later
//! Regression tests for the M2 engine review: the long-lived pump and `live_tail`, events
//! (accounts, the order of `Changed` and `done`, sync progress), opening a post link on a fresh
//! device, `forum_open`'s authority sync and declaration reuse, followed-forum seeding,
//! `parse_ref`, `recent_discussion` and the smaller query and write checks.

mod common;

use std::sync::{Arc, Mutex};

use common::block_on;
use futures::FutureExt;
use lessdugin_core::dto::{
    CommentDraft, DefaultTier, Draft, EditOpts, Event, ForumDraft, ForumRole, ListPostsQuery,
    MembershipMode, PostDraft, PostFilter, PostSort, Ref, RefContext, SyncOpts, ThreadOpts,
    ThreadRoot, Topic, WriteState, WriteTicket,
};
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig, ErrorCode};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::fake_pds::{AccountHandle, Fault};
use lessdugin_testkit::{FakeNet, MemHost};

const MINUTE: i64 = 60_000;

type Events = Arc<Mutex<Vec<Event>>>;

fn record(engine: &Engine<MemHost>) -> Events {
    let events: Events = Arc::default();
    let sink = Arc::clone(&events);
    engine.subscribe(move |e| sink.lock().unwrap().push(e));
    events
}

fn login(engine: &Engine<MemHost>, acct: &AccountHandle) {
    block_on(engine.add_account(AccountInfo::app_password(
        acct.did().as_str(),
        acct.pds_base(),
    )))
    .unwrap();
}

fn done_uri(engine: &Engine<MemHost>, t: &WriteTicket) -> String {
    match block_on(engine.write_status(&t.id)).unwrap().unwrap().state {
        WriteState::Done { uri, .. } => uri,
        other => panic!("write {} is {other:?}", t.id),
    }
}

/// A device of the shared network: its host and engine.
struct Device {
    host: MemHost,
    engine: Engine<MemHost>,
}

impl Device {
    fn new(net: &FakeNet, name: &str, seed: u64) -> Self {
        let host = MemHost::new();
        host.set_seed(seed);
        net.install_as(&host, name);
        let engine = Engine::new(host.clone(), EngineConfig::default());
        Self { host, engine }
    }

    fn flush(&self) {
        let r = block_on(self.engine.outbox_flush()).unwrap();
        assert_eq!((r.failed, r.conflicts), (0, 0));
    }
}

/// alice's open forum with a post by alice (on the network's own host).
struct Forum {
    net: FakeNet,
    alice: Device,
    alice_acct: AccountHandle,
    forum: String,
    post: String,
}

fn forum() -> Forum {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let alice_acct = net.add_account("alice.example.com");
    let alice = Device {
        engine: Engine::new(host.clone(), EngineConfig::default()),
        host,
    };
    login(&alice.engine, &alice_acct);
    let mut d = ForumDraft::new("meta", "Meta");
    d.membership = MembershipMode::Open;
    d.default_tier = DefaultTier::Frontpage;
    let t = block_on(alice.engine.forum_create(&d)).unwrap();
    alice.flush();
    let forum = done_uri(&alice.engine, &t);
    alice.host.advance_ms(MINUTE);
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "Hello", "Body")),
    )
    .unwrap();
    alice.flush();
    let post = done_uri(&alice.engine, &t);
    Forum {
        net,
        alice,
        alice_acct,
        forum,
        post,
    }
}

// ---------------------------------------------------------------------------------------------
// H2: the pump

#[test]
fn the_outbox_pump_is_long_lived_and_follows_the_accounts() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let acct = net.add_account("alice.example.com");
    let engine = Engine::new(host.clone(), EngineConfig::default());
    // sleeps wait for the test's clock, so the pump can be driven poll by poll
    host.park_sleeps(true);

    // started before login (as the SPA does at startup), it waits instead of returning
    let mut pump = Box::pin(engine.outbox_pump());
    assert!((&mut pump).now_or_never().is_none());
    login(&engine, &acct);
    let t = block_on(engine.forum_create(&ForumDraft::new("meta", "Meta"))).unwrap();
    // the account added after it started is pumped
    assert!((&mut pump).now_or_never().is_none());
    let forum = done_uri(&engine, &t);

    // a stop ends it; a pump started after the stop runs
    engine.outbox_stop();
    assert!(matches!((&mut pump).now_or_never(), Some(Ok(()))));
    let mut again = Box::pin(engine.outbox_pump());
    assert!((&mut again).now_or_never().is_none());
    let t = block_on(engine.publish(&PostDraft::article(&forum, "Hello", "Body"))).unwrap();
    assert!((&mut again).now_or_never().is_none());
    done_uri(&engine, &t);

    // a write that waits (a rate limit) runs once the clock reaches its time
    let pds = net.default_pds();
    pds.set_rate_limit(Some(lessdugin_testkit::fake_pds::RateLimit {
        limit: 0,
        window_ms: 10 * MINUTE,
    }));
    let t = block_on(engine.publish(&PostDraft::article(&forum, "Later", "Body"))).unwrap();
    assert!((&mut again).now_or_never().is_none());
    assert!(matches!(
        block_on(engine.write_status(&t.id)).unwrap().unwrap().state,
        WriteState::Pending { .. }
    ));
    pds.set_rate_limit(None);
    host.advance_ms(11 * MINUTE);
    assert!((&mut again).now_or_never().is_none());
    done_uri(&engine, &t);
    engine.outbox_stop();
    assert!(matches!((&mut again).now_or_never(), Some(Ok(()))));
    host.park_sleeps(false);
}

// ---------------------------------------------------------------------------------------------
// M1, M2, L1, L2: events

#[test]
fn account_changes_emit_changed_accounts() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let a = net.add_account("alice.example.com");
    let b = net.add_account("bob.example.com");
    let engine = Engine::new(host.clone(), EngineConfig::default());
    let events = record(&engine);
    login(&engine, &a);
    login(&engine, &b);
    block_on(engine.set_active_account(a.did().as_str())).unwrap();
    block_on(engine.remove_account(b.did().as_str(), true)).unwrap();
    let changed: Vec<Vec<Topic>> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Changed { topics } => Some(topics.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        changed,
        [
            vec![Topic::Accounts],
            vec![Topic::Accounts],
            vec![Topic::Accounts],
            vec![Topic::Outbox, Topic::Accounts],
        ]
    );
}

#[test]
fn changed_comes_after_the_write_is_done() {
    let f = forum();
    let events = record(&f.alice.engine);
    let t = block_on(
        f.alice
            .engine
            .publish(&PostDraft::article(&f.forum, "Second", "Body")),
    )
    .unwrap();
    f.alice.flush();
    let post = done_uri(&f.alice.engine, &t);
    let events = events.lock().unwrap().clone();
    let done_at = events
        .iter()
        .position(
            |e| matches!(e, Event::Write { id, state: WriteState::Done { .. } } if *id == t.id),
        )
        .expect("a done event");
    let thread_changed_at = events
        .iter()
        .position(|e| match e {
            Event::Changed { topics } => topics.contains(&Topic::Thread { uri: post.clone() }),
            _ => false,
        })
        .expect("a Changed for the new thread");
    assert!(done_at < thread_changed_at, "{events:#?}");
    // every write state change is also `Changed [Outbox]`
    let writes = events
        .iter()
        .filter(|e| matches!(e, Event::Write { .. }))
        .count();
    let outbox = events
        .iter()
        .filter(|e| matches!(e, Event::Changed { topics } if topics == &[Topic::Outbox]))
        .count();
    assert_eq!(writes, outbox);
    // a view re-queried on that Changed no longer shows the post as pending
    let l = block_on(f.alice.engine.list_posts(&ListPostsQuery {
        sort: PostSort::New,
        filter: PostFilter::All,
        ..ListPostsQuery::new(&f.forum)
    }))
    .unwrap();
    assert!(l.items.iter().all(|p| !p.badges.pending));
}

#[test]
fn sync_progress_is_reported_while_the_sync_runs() {
    let f = forum();
    let bob_acct = f.net.add_account("bob.example.com");
    let bob = Device::new(&f.net, "bob", 2);
    login(&bob.engine, &bob_acct);
    block_on(bob.engine.join(&f.forum, None)).unwrap();
    bob.flush();
    let t = block_on(
        bob.engine
            .publish(&PostDraft::article(&f.forum, "Bob's", "Body")),
    )
    .unwrap();
    bob.flush();
    done_uri(&bob.engine, &t);

    let carol = Device::new(&f.net, "carol", 3);
    let events = record(&carol.engine);
    // bob's post is on screen (an open forum finds newcomers from hints, M5)
    let opts = lessdugin_core::discovery::SyncOpts {
        on_screen: vec![bob_acct.did().to_string()],
        ..Default::default()
    };
    let r = block_on(carol.engine.sync_forum(&f.forum, &opts)).unwrap();
    assert!(!r.partial, "{r:?}");
    let progress: Vec<(u32, u32)> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::SyncProgress { forum, done, total } if *forum == f.forum => {
                Some((*done, *total))
            }
            _ => None,
        })
        .collect();
    // reported as repos finish, never backwards, closed by done == total
    assert!(progress.len() >= 3, "{progress:?}");
    assert!(
        progress.windows(2).all(|w| w[0].0 <= w[1].0),
        "{progress:?}"
    );
    assert!(progress.iter().any(|(d, t)| d < t), "{progress:?}");
    assert_eq!(progress.last(), Some(&(2, 2)));

    // a sync that fails part-way still closes its progress and reports the change (the point
    // fetch stored a tombstone)
    events.lock().unwrap().clear();
    let rkey = lessdugin_lexicon::keys::comment_rkey(
        lessdugin_lexicon::ns::NS,
        &f.forum,
        &f.post,
        "2222222222222",
    )
    .unwrap();
    let gone = format!("at://{}/{}/{rkey}", bob_acct.did(), nsid::COMMENT);
    assert!(block_on(carol.engine.sync_thread(&gone, &SyncOpts::default())).is_err());
    let events = events.lock().unwrap().clone();
    assert!(
        events.iter().any(|e| matches!(e,
            Event::SyncProgress { forum, done, total } if *forum == f.forum && done == total
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e,
            Event::Changed { topics } if topics.contains(&Topic::Forum { uri: f.forum.clone() })
        )),
        "{events:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// H1, L4, L11: a post link on a fresh device

#[test]
fn a_post_link_opens_on_a_device_that_never_opened_its_forum() {
    let f = forum();
    // bob replies on his device, so the thread has a comment
    let bob_acct = f.net.add_account("bob.example.com");
    let bob = Device::new(&f.net, "bob", 2);
    login(&bob.engine, &bob_acct);
    block_on(bob.engine.sync_thread(&f.post, &SyncOpts::default())).unwrap();
    let t = block_on(bob.engine.comment(&CommentDraft::new(&f.post, None, "Hi"))).unwrap();
    bob.flush();
    let comment = done_uri(&bob.engine, &t);

    // carol's fresh device opens the links directly (`#/p/…`, `show <uri>`)
    let carol = Device::new(&f.net, "carol", 3);
    let th = block_on(carol.engine.get_thread(&f.post, &ThreadOpts::default())).unwrap();
    assert!(matches!(&th.post, ThreadRoot::Post(p) if p.post.forum == f.forum));
    let fresh = Device::new(&f.net, "dave", 4);
    let p = block_on(fresh.engine.get_post(&f.post)).unwrap();
    assert_eq!(p.post.uri, f.post);
    // sync_thread takes a comment's uri (its thread), syncs the authorities, and reports the
    // forum's state after the sync
    let other = Device::new(&f.net, "erin", 5);
    let r = block_on(other.engine.sync_thread(&comment, &SyncOpts::default())).unwrap();
    assert!(!r.degraded, "{r:?}");
    assert!(r.errors.is_empty() && !r.partial, "{r:?}");
    let th = block_on(other.engine.get_thread(&comment, &ThreadOpts::default())).unwrap();
    match &th.post {
        ThreadRoot::Post(p) => assert_eq!(p.post.title.as_deref(), Some("Hello")),
        other => panic!("{other:?}"),
    }
    assert_eq!(th.comments.len(), 1);

    // a record that is not a post or comment is not a thread
    let membership = format!(
        "at://{}/{}/{}",
        f.alice_acct.did(),
        nsid::MEMBERSHIP,
        f.forum.rsplit('/').next().unwrap()
    );
    let err = block_on(other.engine.get_thread(&membership, &ThreadOpts::default())).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);
    let err = block_on(other.engine.sync_thread(&membership, &SyncOpts::default())).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);
    // a post that does not exist
    let gone = f
        .post
        .replace(f.post.rsplit('-').next().unwrap(), "2222222222222");
    let err = block_on(other.engine.get_post(&gone)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
}

// ---------------------------------------------------------------------------------------------
// M4: forum_open

#[test]
fn forum_open_syncs_the_authorities_and_reuses_the_declaration() {
    let f = forum();
    // the owner right after creating it
    let v = block_on(f.alice.engine.forum_open(&f.forum)).unwrap();
    assert!(!v.completeness.degraded, "{v:?}");
    // a fresh device
    let bob = Device::new(&f.net, "bob", 2);
    let v = block_on(bob.engine.forum_open(&f.forum)).unwrap();
    assert!(!v.completeness.degraded, "{v:?}");
    assert!(v.banners.is_empty(), "{:?}", v.banners);
    // opening it again within the reopen time costs no request
    let n = bob.host.requests().len();
    block_on(bob.engine.forum_open(&f.forum)).unwrap();
    assert_eq!(bob.host.requests().len(), n);
    // later, the declaration is fetched again
    bob.host
        .advance_ms(lessdugin_core::limits::FORUM_REOPEN_MS + 1);
    block_on(bob.engine.forum_open(&f.forum)).unwrap();
    assert!(bob.host.requests().len() > n);
}

// ---------------------------------------------------------------------------------------------
// L3: followed-forum seeding

#[test]
fn the_followed_list_is_seeded_once_per_login() {
    let f = forum();
    let lists = |h: &MemHost| {
        h.requests()
            .iter()
            .filter(|r| r.url.contains("listRecords") && r.url.contains(nsid::MEMBERSHIP))
            .count()
    };
    // alice logs in on a second device: her forum is followed, and counts as joined before
    // any sync
    let laptop = Device::new(&f.net, "laptop", 7);
    login(&laptop.engine, &f.alice_acct);
    let followed = block_on(laptop.engine.followed()).unwrap();
    assert_eq!(followed.len(), 1);
    assert_eq!(followed[0].uri, f.forum);
    assert!(followed[0].member);
    let seeding = lists(&laptop.host);
    assert!(seeding > 0);
    // the next run of the client (a new engine) does not read the memberships again
    let again = Engine::new(laptop.host.clone(), EngineConfig::default());
    assert_eq!(block_on(again.followed()).unwrap().len(), 1);
    assert_eq!(lists(&laptop.host), seeding);
    // a new login does
    login(&again, &f.alice_acct);
    block_on(again.followed()).unwrap();
    assert_eq!(lists(&laptop.host), 2 * seeding);
}

// ---------------------------------------------------------------------------------------------
// L13: live_tail, handle verification

#[test]
fn live_tail_keeps_followed_forums_fresh_and_verifies_handles() {
    let f = forum();
    let bob_acct = f.net.add_account("bob.example.com");
    let bob = Device::new(&f.net, "bob", 2);
    login(&bob.engine, &bob_acct);
    block_on(bob.engine.join(&f.forum, None)).unwrap();
    bob.flush();
    let alice = &f.alice;
    let opts = lessdugin_core::discovery::SyncOpts {
        on_screen: vec![bob_acct.did().to_string()],
        ..Default::default()
    };
    block_on(alice.engine.sync_forum(&f.forum, &opts)).unwrap();

    // views show authors from the identity cache, and queue them for verification
    let q = ListPostsQuery::new(&f.forum);
    let l = block_on(alice.engine.list_posts(&q)).unwrap();
    assert_eq!(l.items.len(), 1);
    let events = record(&alice.engine);
    let round = block_on(alice.engine.live_step()).unwrap();
    assert!(round.verified >= 1, "{round:?}");
    let l = block_on(alice.engine.list_posts(&q)).unwrap();
    assert_eq!(
        l.items[0].author.handle.as_deref(),
        Some("alice.example.com")
    );

    // bob posts; alice's live tail polls his head, queues and syncs his repo
    let t = block_on(
        bob.engine
            .publish(&PostDraft::article(&f.forum, "From bob", "Body")),
    )
    .unwrap();
    bob.flush();
    let bob_post = done_uri(&bob.engine, &t);
    let mut seen = false;
    for _ in 0..10 {
        alice.host.advance_ms(13 * 60 * MINUTE);
        bob.host.advance_ms(13 * 60 * MINUTE);
        let r = block_on(alice.engine.live_step()).unwrap();
        assert!(r.next_ms >= 1 && r.next_ms <= lessdugin_core::limits::LIVE_TAIL_IDLE_MS);
        let l = block_on(alice.engine.list_posts(&ListPostsQuery {
            filter: PostFilter::All,
            ..ListPostsQuery::new(&f.forum)
        }))
        .unwrap();
        if l.items.iter().any(|p| p.uri == bob_post) {
            seen = true;
            break;
        }
    }
    assert!(seen, "bob's post never arrived");
    assert!(events.lock().unwrap().iter().any(|e| matches!(e,
        Event::Changed { topics } if topics.contains(&Topic::Forum { uri: f.forum.clone() })
    )));

    // the long-lived form waits between rounds and ends on live_stop
    alice.host.park_sleeps(true);
    let mut tail = Box::pin(alice.engine.live_tail());
    assert!((&mut tail).now_or_never().is_none());
    alice.engine.live_stop();
    assert!(matches!((&mut tail).now_or_never(), Some(Ok(()))));
    alice.host.park_sleeps(false);
    // background_verify has nothing to do in v1
    block_on(alice.engine.background_verify()).unwrap();
}

// ---------------------------------------------------------------------------------------------
// L13: parse_ref, recent_discussion

#[test]
fn parse_ref_accepts_uris_links_and_actors() {
    let f = forum();
    let e = &f.alice.engine;
    let did = f.alice_acct.did().to_string();
    let ctx = RefContext::default();
    let rkey = f.post.rsplit('/').next().unwrap().to_owned();
    let p = |s: &str| block_on(e.parse_ref(s, &ctx));
    assert_eq!(
        p(&f.post).unwrap(),
        Ref::Post {
            uri: f.post.clone()
        }
    );
    assert_eq!(
        p(&f.forum).unwrap(),
        Ref::Forum {
            uri: f.forum.clone()
        }
    );
    assert_eq!(
        p(&format!("https://app.example/#/p/alice.example.com/{rkey}")).unwrap(),
        Ref::Post {
            uri: f.post.clone()
        }
    );
    assert_eq!(
        p("#/f/alice.example.com/meta?x=1").unwrap(),
        Ref::Forum {
            uri: f.forum.clone()
        }
    );
    assert_eq!(
        p(&format!("#/at/{did}/{}/{rkey}", nsid::POST)).unwrap(),
        Ref::Post {
            uri: f.post.clone()
        }
    );
    assert_eq!(
        p("@alice.example.com").unwrap(),
        Ref::Actor { did: did.clone() }
    );
    assert_eq!(p(&did).unwrap(), Ref::Actor { did: did.clone() });
    assert_eq!(
        p("#/u/alice.example.com").unwrap(),
        Ref::Actor { did: did.clone() }
    );
    assert_eq!(
        p("at://alice.example.com").unwrap(),
        Ref::Actor { did: did.clone() }
    );
    // handle/rkey: a post of the current forum
    let in_forum = RefContext {
        forum: Some(f.forum.clone()),
    };
    assert_eq!(
        block_on(e.parse_ref(&format!("alice.example.com/{rkey}"), &in_forum)).unwrap(),
        Ref::Post {
            uri: f.post.clone()
        }
    );
    for bad in [
        "",
        "not a ref",
        &format!("alice.example.com/{rkey}"),
        "#/x/y",
    ] {
        assert_eq!(p(bad).unwrap_err().code, ErrorCode::InvalidRef, "{bad:?}");
    }
}

#[test]
fn recent_discussion_lists_commented_posts_latest_first() {
    let f = forum();
    let e = &f.alice.engine;
    let t = block_on(e.publish(&PostDraft::article(&f.forum, "Quiet", "No comments"))).unwrap();
    f.alice.flush();
    let quiet = done_uri(e, &t);
    let mut comments = Vec::new();
    for i in 0..4 {
        f.alice.host.advance_ms(MINUTE);
        let t = block_on(e.comment(&CommentDraft::new(&f.post, None, format!("c{i}")))).unwrap();
        f.alice.flush();
        comments.push(done_uri(e, &t));
    }
    let d = block_on(e.recent_discussion(&f.forum, None, 0)).unwrap();
    assert_eq!(d.items.len(), 1);
    assert_eq!(d.items[0].post.uri, f.post);
    assert_ne!(d.items[0].post.uri, quiet);
    let shown: Vec<&str> = d.items[0].comments.iter().map(|c| c.uri.as_str()).collect();
    let latest: Vec<&str> = comments.iter().rev().take(3).map(String::as_str).collect();
    assert_eq!(shown, latest);
    assert!(d.cursor.is_none());
}

// ---------------------------------------------------------------------------------------------
// L9, L10: history and comment checks

#[test]
fn comments_and_history_checks() {
    let f = forum();
    let e = &f.alice.engine;
    // a parent from another thread is refused
    let t = block_on(e.publish(&PostDraft::article(&f.forum, "Other", "Body"))).unwrap();
    f.alice.flush();
    let other = done_uri(e, &t);
    let t = block_on(e.comment(&CommentDraft::new(&other, None, "there"))).unwrap();
    f.alice.flush();
    let elsewhere = done_uri(e, &t);
    let err =
        block_on(e.comment(&CommentDraft::new(&f.post, Some(elsewhere.clone()), "x"))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);

    // an edit without history dates the current version by its updatedAt
    f.alice.host.advance_ms(60 * MINUTE);
    let edit_at = f.alice.host.now();
    let t = block_on(e.edit(
        &f.post,
        &Draft::Post {
            id: None,
            post: PostDraft::article(&f.forum, "Hello", "Edited"),
        },
        &EditOpts {
            history: false,
            summary: None,
        },
    ))
    .unwrap();
    f.alice.flush();
    done_uri(e, &t);
    let h = block_on(e.history(&f.post)).unwrap();
    assert_eq!(h.versions.len(), 1);
    assert_eq!(h.versions[0].created_at_ms, edit_at);
}

// ---------------------------------------------------------------------------------------------
// Missing tests named by the review: membership writes, undo after send, retry, the pending
// badge, a deleted comment with replies, an approval-mode applicant

#[test]
fn leave_undo_retry_and_the_pending_badge() {
    let f = forum();
    let bob_acct = f.net.add_account("bob.example.com");
    let bob = Device::new(&f.net, "bob", 2);
    login(&bob.engine, &bob_acct);
    block_on(bob.engine.join(&f.forum, None)).unwrap();
    bob.flush();
    let role = |e: &Engine<MemHost>| block_on(e.forum_open(&f.forum)).unwrap().my_role;
    // open mode, no grant: a participant who is neither a seed nor established
    assert_eq!(role(&bob.engine), ForumRole::Newcomer);

    // a sent post cannot be undone
    let t = block_on(
        bob.engine
            .publish(&PostDraft::article(&f.forum, "Bob's", "Body")),
    )
    .unwrap();
    bob.flush();
    let post = done_uri(&bob.engine, &t);
    let err = block_on(bob.engine.undo(&t.id)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // the pending badge: an edit waiting in the outbox marks the post until it is done
    let t = block_on(bob.engine.edit(
        &post,
        &Draft::Post {
            id: None,
            post: PostDraft::article(&f.forum, "Bob's", "Edited"),
        },
        &EditOpts::default(),
    ))
    .unwrap();
    let badge = || {
        block_on(bob.engine.get_post(&post))
            .unwrap()
            .post
            .badges
            .pending
    };
    assert!(badge());
    bob.flush();
    done_uri(&bob.engine, &t);
    assert!(!badge());

    // a refused write fails; retry_write sends it again
    f.net.inject(
        Fault::error(400, "InvalidRequest", "refused")
            .on_nsid("com.atproto.repo.createRecord")
            .for_caller("bob")
            .once(),
    );
    let t = block_on(
        bob.engine
            .publish(&PostDraft::article(&f.forum, "Again", "Body")),
    )
    .unwrap();
    block_on(bob.engine.outbox_flush()).unwrap();
    assert!(matches!(
        block_on(bob.engine.write_status(&t.id))
            .unwrap()
            .unwrap()
            .state,
        WriteState::Failed { .. }
    ));
    block_on(bob.engine.retry_write(&t.id)).unwrap();
    bob.flush();
    done_uri(&bob.engine, &t);

    // leave deletes the membership: bob is no longer a member
    let t = block_on(bob.engine.leave(&f.forum)).unwrap();
    bob.flush();
    done_uri(&bob.engine, &t);
    assert_eq!(role(&bob.engine), ForumRole::None);
}

#[test]
fn a_deleted_comment_with_replies_becomes_a_placeholder_elsewhere() {
    let f = forum();
    let bob_acct = f.net.add_account("bob.example.com");
    let bob = Device::new(&f.net, "bob", 2);
    login(&bob.engine, &bob_acct);
    block_on(bob.engine.join(&f.forum, None)).unwrap();
    bob.flush();
    block_on(bob.engine.sync_thread(&f.post, &SyncOpts::default())).unwrap();
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&f.post, None, "first")),
    )
    .unwrap();
    bob.flush();
    let c1 = done_uri(&bob.engine, &t);
    // alice answers bob's comment (bob is on screen: an open forum finds newcomers from hints,
    // M5)
    let alice = &f.alice.engine;
    let opts = lessdugin_core::discovery::SyncOpts {
        on_screen: vec![bob_acct.did().to_string()],
        ..Default::default()
    };
    block_on(alice.sync_forum(&f.forum, &opts)).unwrap();
    let t =
        block_on(alice.comment(&CommentDraft::new(&f.post, Some(c1.clone()), "reply"))).unwrap();
    f.alice.flush();
    let c2 = done_uri(alice, &t);
    // carol's device has seen both
    let carol = Device::new(&f.net, "carol", 3);
    block_on(carol.engine.sync_forum(&f.forum, &opts)).unwrap();
    let th = block_on(carol.engine.get_thread(&f.post, &ThreadOpts::default())).unwrap();
    assert_eq!(th.comments.len(), 2);
    assert!(th.comments.iter().all(|c| !c.missing));
    // bob deletes his comment; carol's next sync shows a placeholder above alice's reply
    let t = block_on(bob.engine.delete(&c1)).unwrap();
    bob.flush();
    done_uri(&bob.engine, &t);
    carol.host.advance_ms(MINUTE);
    block_on(carol.engine.sync_thread(&f.post, &SyncOpts::default())).unwrap();
    let th = block_on(carol.engine.get_thread(&f.post, &ThreadOpts::default())).unwrap();
    let shape: Vec<(&str, bool, Option<&str>)> = th
        .comments
        .iter()
        .map(|c| (c.uri.as_str(), c.missing, c.parent.as_deref()))
        .collect();
    assert!(shape.contains(&(c1.as_str(), true, None)), "{shape:?}");
    assert!(
        shape.contains(&(c2.as_str(), false, Some(c1.as_str()))),
        "{shape:?}"
    );
}

#[test]
fn an_applicants_post_is_hidden_in_an_approval_forum() {
    let host = MemHost::new();
    let net = FakeNet::on(&host);
    let _ = net.default_pds();
    let alice_acct = net.add_account("alice.example.com");
    let alice = Device {
        engine: Engine::new(host.clone(), EngineConfig::default()),
        host,
    };
    login(&alice.engine, &alice_acct);
    let mut d = ForumDraft::new("club", "Club");
    d.membership = MembershipMode::Approval;
    let t = block_on(alice.engine.forum_create(&d)).unwrap();
    alice.flush();
    let forum = done_uri(&alice.engine, &t);
    // eve applies (joins without a grant) and posts
    let eve_acct = net.add_account("eve.example.com");
    let eve = Device::new(&net, "eve", 9);
    login(&eve.engine, &eve_acct);
    block_on(eve.engine.join(&forum, Some("let me in"))).unwrap();
    eve.flush();
    let t = block_on(
        eve.engine
            .publish(&PostDraft::article(&forum, "BUY PILLS", "spam")),
    )
    .unwrap();
    eve.flush();
    let spam = done_uri(&eve.engine, &t);
    // alice's sync sees eve's post (on screen): an applicant's post is hidden, without content
    let opts = lessdugin_core::discovery::SyncOpts {
        on_screen: vec![eve_acct.did().to_string()],
        ..Default::default()
    };
    block_on(alice.engine.sync_forum(&forum, &opts)).unwrap();
    let p = block_on(alice.engine.get_post(&spam)).unwrap();
    assert!(
        matches!(
            p.post.state,
            lessdugin_core::dto::ItemState::Hidden {
                reason: lessdugin_core::dto::HiddenReason::NotParticipant
            }
        ),
        "{:?}",
        p.post.state
    );
    assert_eq!(p.post.title, None);
    // once a participant answers it, it stays in the thread as a tombstone, still without its
    // content
    let t = block_on(
        alice
            .engine
            .comment(&CommentDraft::new(&spam, None, "Removed")),
    )
    .unwrap();
    alice.flush();
    done_uri(&alice.engine, &t);
    let p = block_on(alice.engine.get_post(&spam)).unwrap();
    assert!(
        matches!(
            p.post.state,
            lessdugin_core::dto::ItemState::Tombstone { .. }
        ),
        "{:?}",
        p.post.state
    );
    assert_eq!(p.post.title, None);
    assert!(p.body.html.is_empty());
    let l = block_on(alice.engine.list_posts(&ListPostsQuery {
        filter: PostFilter::All,
        ..ListPostsQuery::new(&forum)
    }))
    .unwrap();
    assert!(l.items.iter().all(|i| i.uri != spam));
}

// ---------------------------------------------------------------------------------------------
// M2 client review (robustness H1, M3): a refused session holds the writes until the next
// login; "send now" ends a back-off

const WRITE_NSIDS: [&str; 4] = [
    "com.atproto.repo.applyWrites",
    "com.atproto.repo.createRecord",
    "com.atproto.repo.putRecord",
    "com.atproto.repo.deleteRecord",
];

fn write_requests(net: &FakeNet) -> usize {
    WRITE_NSIDS.iter().map(|n| net.count(n)).sum()
}

#[test]
fn a_refused_session_holds_writes_until_the_account_logs_in_again() {
    let f = forum();
    let engine = &f.alice.engine;
    let events = record(engine);
    for n in WRITE_NSIDS {
        f.net
            .inject(Fault::error(401, "ExpiredToken", "Token has expired").on_nsid(n));
    }
    let t = block_on(engine.publish(&PostDraft::article(&f.forum, "Later", "Body"))).unwrap();
    let r = block_on(engine.outbox_flush()).unwrap();
    assert_eq!((r.failed, r.needs_login), (0, 1), "{r:?}");
    let state = block_on(engine.write_status(&t.id)).unwrap().unwrap().state;
    let WriteState::Pending {
        needs_login: true,
        throttled: false,
        not_before_ms: None,
        error: Some(e),
        ..
    } = &state
    else {
        panic!("{state:?}")
    };
    assert_eq!(e.code, ErrorCode::AuthRequired);
    let did = f.alice_acct.did().to_string();
    assert!(
        events.lock().unwrap().iter().any(|e| matches!(
            e,
            Event::Auth { did: d, state: lessdugin_core::dto::AuthState::Expired } if *d == did
        )),
        "Auth Expired is emitted"
    );
    // Nothing is retried while the session is refused, however often the outbox runs, and
    // the hold is not a failed attempt.
    let sent = write_requests(&f.net);
    for _ in 0..3 {
        f.alice.host.advance_ms(60 * MINUTE);
        let r = block_on(engine.outbox_flush()).unwrap();
        assert_eq!(r.needs_login, 1);
    }
    assert_eq!(write_requests(&f.net), sent);
    // A new login (the session works again) releases the hold, and the write is sent.
    f.net.clear_faults();
    login(engine, &f.alice_acct);
    assert!(matches!(
        block_on(engine.write_status(&t.id)).unwrap().unwrap().state,
        WriteState::Pending {
            needs_login: false,
            ..
        }
    ));
    let r = block_on(engine.outbox_flush()).unwrap();
    assert_eq!((r.done, r.needs_login), (1, 0), "{r:?}");
    done_uri(engine, &t);
}

#[test]
fn send_now_ends_a_back_off_but_not_a_rate_limit() {
    let f = forum();
    let engine = &f.alice.engine;
    // An ambiguous failure (a 500) backs the write off.
    let fault = f.net.inject(
        Fault::status(500)
            .on_nsid("com.atproto.repo.createRecord")
            .once(),
    );
    let fault2 = f.net.inject(
        Fault::status(500)
            .on_nsid("com.atproto.repo.putRecord")
            .once(),
    );
    let fault3 = f.net.inject(
        Fault::status(500)
            .on_nsid("com.atproto.repo.applyWrites")
            .once(),
    );
    let t = block_on(engine.publish(&PostDraft::article(&f.forum, "Retry", "Body"))).unwrap();
    let step = lessdugin_core::outbox::Session::new(
        f.alice_acct.did().clone(),
        url::Url::parse(&f.alice_acct.pds_base()).unwrap(),
    );
    let r = block_on(engine.outbox().run_due(&step)).unwrap();
    assert_eq!(r.throttled, 1, "{r:?}");
    let WriteState::Pending {
        throttled: true,
        not_before_ms: Some(_),
        error: Some(cause),
        ..
    } = block_on(engine.write_status(&t.id)).unwrap().unwrap().state
    else {
        panic!("backing off")
    };
    assert_ne!(
        cause.code,
        ErrorCode::RateLimited,
        "the pending state says why"
    );
    for id in [fault, fault2, fault3] {
        f.net.remove_fault(id);
    }
    // Without "send now" the next run waits for the back-off.
    let r = block_on(engine.outbox().run_due(&step)).unwrap();
    assert_eq!(r.processed, 0);
    assert_eq!(block_on(engine.outbox_expedite()).unwrap(), 1);
    let r = block_on(engine.outbox().run_due(&step)).unwrap();
    assert_eq!(r.done, 1, "{r:?}");
    done_uri(engine, &t);

    // `retry` on a backing-off write is "send now" too; a PDS rate limit is left alone.
    let pds = f.net.default_pds();
    pds.set_rate_limit(Some(lessdugin_testkit::fake_pds::RateLimit {
        limit: 0,
        window_ms: 10 * MINUTE,
    }));
    let t = block_on(engine.publish(&PostDraft::article(&f.forum, "Limited", "Body"))).unwrap();
    let r = block_on(engine.outbox().run_due(&step)).unwrap();
    assert_eq!(r.throttled, 1);
    assert_eq!(block_on(engine.outbox_expedite()).unwrap(), 0);
    let e = block_on(engine.retry_write(&t.id)).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidInput);
    pds.set_rate_limit(None);
}

// ---------------------------------------------------------------------------------------------
// M2 client review (robustness H2): a hung owner PDS does not block a view of cached content

#[test]
fn a_hung_pds_does_not_block_opening_a_cached_forum() {
    let f = forum();
    let engine = &f.alice.engine;
    block_on(engine.sync_forum(&f.forum, &SyncOpts::default())).unwrap();
    block_on(engine.forum_open(&f.forum)).unwrap();
    // Past the reopen window, the PDS accepts requests but answers after two minutes.
    f.alice.host.advance_ms(10 * MINUTE);
    f.net
        .inject(Fault::latency(120_000).on_host(&f.alice_acct.pds_base()));
    let started = f.alice.host.now();
    let view = block_on(engine.forum_open(&f.forum)).unwrap();
    let took = f.alice.host.now() - started;
    assert_eq!(view.forum.uri, f.forum);
    assert!(
        took <= lessdugin_core::limits::FORUM_DECL_CACHED_BUDGET_MS
            + lessdugin_core::limits::FORUM_OPEN_SYNC_BUDGET_MS,
        "forum_open took {took} ms of (fake) time with a hung PDS"
    );
    // The cached thread is still readable.
    let thread = block_on(engine.get_thread(&f.post, &ThreadOpts::default())).unwrap();
    assert!(matches!(&thread.post, ThreadRoot::Post(p) if p.post.uri == f.post));
}
