// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 social surface over a `TestBackend`: opinions (`synthesis-v1.md` §6.11), viewer caps,
//! reacts, moderation, notifications and the `:` command line.

mod common;

use common::{MINUTE, block_on, scenario, tui};
use lessdugin_core::dto::{AccessLevel, ModActionIntent, ModActionValue, ModScope, SyncOpts};

type Tui = lessdugin_tui::Headless<lessdugin_testkit::MemHost>;

/// The status line's message as plain text.
fn status(t: &Tui) -> String {
    t.state
        .status
        .as_ref()
        .map(|s| s.text.to_plain_string())
        .unwrap_or_default()
}

/// The points of `needle`'s row as the forum list renders them.
fn points_of(screen: &str, needle: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let Some(i) = lines.iter().position(|l| l.contains(needle)) else {
        return String::new();
    };
    let meta = lines.get(i + 1).copied().unwrap_or_default();
    meta.split('\u{b7}')
        .map(str::trim)
        .find(|p| p.contains("point"))
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn upvote_records_then_counts() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    // bob's post is first (magic, newest first); the author's self-vote scores it 1
    assert_eq!(points_of(&t.render(), "Hello from bob"), "1 point");

    // `u`: held for the undo window, the score is not counted yet
    block_on(t.press("u"));
    let st = status(&t);
    assert!(st.contains("Vote recorded"), "{st}");
    assert!(st.contains("Backspace undoes"), "{st}");
    assert_eq!(points_of(&t.render(), "Hello from bob"), "1 point");
    assert!(!t.state.op_writes.is_empty(), "the write is open");

    // the pump sends it (the host clock sleeps through the hold); the vote counts
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    assert_eq!(points_of(&t.render(), "Hello from bob"), "2 points");
}

#[test]
fn backspace_undoes_a_pending_vote() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("u"));
    block_on(t.press("Backspace"));
    assert!(status(&t).contains("Undone"), "{}", status(&t));
    assert!(t.state.op_writes.is_empty());
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    assert_eq!(points_of(&t.render(), "Hello from bob"), "1 point");
    // nothing left to undo
    block_on(t.press("Backspace"));
    assert!(status(&t).contains("Nothing to undo"), "{}", status(&t));
}

#[test]
fn a_banned_viewer_cannot_vote() {
    let s = scenario();
    let (alice, carol) = (&s.w.alice, &s.w.carol);
    block_on(alice.engine.mod_action(&ModActionIntent::Assert {
        forum: s.forum.clone(),
        scope: ModScope::Forum,
        subject: carol.did(),
        action: ModActionValue::Access {
            level: AccessLevel::Banned,
        },
        reason: None,
        until_ms: None,
    }))
    .unwrap();
    alice.flush();
    s.w.tick(MINUTE);
    block_on(carol.engine.sync_forum(&s.forum, &SyncOpts::default())).unwrap();

    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("u"));
    assert!(
        status(&t).contains("You cannot vote here"),
        "{}",
        status(&t)
    );
    assert!(t.state.op_writes.is_empty(), "nothing was queued");
    block_on(t.press("a"));
    assert!(
        status(&t).contains("You cannot vote here"),
        "{}",
        status(&t)
    );
}
use lessdugin_core::engine::{AccountInfo, Engine, EngineConfig};
use lessdugin_testkit::MemHost;
use lessdugin_testkit::fake_pds::Fault;

/// A fresh read-only device on the scenario's network (no account): sees the forum as a
/// reader who never listed a repo before.
fn reader(name: &str, s: &common::Scenario) -> Engine<MemHost> {
    let h = MemHost::new();
    h.set_seed(9);
    h.set_now_ms(s.w.alice.host.now());
    s.w.net.install_as(&h, name);
    Engine::new(h, EngineConfig::default())
}

#[test]
fn a_degraded_forum_shows_its_banner() {
    use lessdugin_core::dto::ModRole;
    use lessdugin_core::dto::{BodyDraft, DefaultTier, ForumDraft, ModeratorDraft, PostDraft};
    let s = scenario();
    let (alice, bob) = (&s.w.alice, &s.w.bob);
    // a forum with bob as a declared moderator: bob's repo then becomes an authority's repo
    let mut draft = ForumDraft::new("deg", "Degraded");
    draft.default_tier = DefaultTier::Frontpage;
    draft.about = Some(BodyDraft::text("Be kind."));
    draft.moderators = vec![ModeratorDraft {
        did: bob.did(),
        role: ModRole::Moderator,
        title: None,
    }];
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    s.w.tick(MINUTE);
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "Visible post", "Body.")),
    )
    .unwrap();
    alice.flush();
    let _post = alice.done(&t);
    s.w.tick(MINUTE);

    // bob's PDS stops answering, and enough time passes for his repo to count as unavailable
    s.w.net.inject(Fault::status(503).on_repo(&s.w.bob.did()));
    s.w.tick(3 * 24 * 60 * MINUTE);
    let late = reader("late", &s);
    let r = block_on(late.sync_forum(&forum, &SyncOpts::default())).unwrap();
    assert!(r.degraded || r.repos_unavailable > 0, "{r:?}");

    let mut t = block_on(lessdugin_tui::Headless::new(
        late,
        lessdugin_tui::HeadlessOpts {
            width: 80,
            height: 24,
            auto_sync: false,
            ..Default::default()
        },
    ))
    .unwrap();
    block_on(t.start(Some(forum.clone())));
    let screen = t.render();
    assert!(screen.contains("moderation data unavailable"), "{screen}");
    insta::assert_snapshot!("forum_degraded_banner_80x24", screen);

    // the thread screen shows the same banner
    block_on(t.open_thread(&_post));
    let doc = t.thread_document().unwrap();
    assert!(doc.contains("moderation data unavailable"), "{doc}");
}

/// An open-mode forum with a newcomer (nina): her vote is recorded but carries no weight yet
/// (`social.md` §5-§7: voters outside E), so every screen shows "1 vote not yet counted" and
/// nina's own shows "(not counted yet)".
#[test]
fn an_unestablished_voters_vote_shows_as_not_counted() {
    use lessdugin_core::dto::{
        BodyDraft, DefaultTier, ForumDraft, KarmaValue, MembershipMode, OpinionIntent, PostDraft,
    };
    let s = scenario();
    let (alice, bob, carol) = (&s.w.alice, &s.w.bob, &s.w.carol);
    // nina's device on the same network
    let nina_acct = s.w.net.add_account("nina.example.com");
    let h = MemHost::new();
    h.set_seed(7);
    s.w.net.install_as(&h, "nina");
    let nina = Engine::new(h, EngineConfig::default());
    block_on(nina.add_account(AccountInfo::app_password(
        nina_acct.did().as_str(),
        nina_acct.pds_base(),
    )))
    .unwrap();

    // an OPEN forum (approval mode has no newcomers: every granted member is a seed)
    let mut draft = ForumDraft::new("open", "Open forum");
    draft.membership = MembershipMode::Open;
    draft.default_tier = DefaultTier::Frontpage;
    draft.about = Some(BodyDraft::text("Be kind."));
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    s.w.tick(MINUTE);
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "Welcome aboard", "Hello.")),
    )
    .unwrap();
    alice.flush();
    let welcome = alice.done(&t);
    s.w.tick(MINUTE);
    // bob and carol join and are granted (seeds); nina just joins (a newcomer)
    block_on(bob.engine.forum_follow(&forum, None)).unwrap();
    block_on(bob.engine.join(&forum, None)).unwrap();
    bob.flush();
    block_on(carol.engine.forum_follow(&forum, None)).unwrap();
    block_on(carol.engine.join(&forum, None)).unwrap();
    carol.flush();
    block_on(alice.engine.grant(&forum, &[bob.did(), carol.did()])).unwrap();
    alice.flush();
    block_on(nina.forum_follow(&forum, None)).unwrap();
    block_on(nina.join(&forum, Some("new here"))).unwrap();
    block_on(nina.outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(nina.sync_forum(&forum, &SyncOpts::default())).unwrap();
    // nina comments and a seed (bob) replies: the reply pulls nina's repo into the closure
    // (`sync.md` §4.3) so other devices discover her records, her opinion included
    let tc = block_on(nina.comment(&lessdugin_core::dto::CommentDraft::new(
        &welcome,
        None,
        "New here!",
    )))
    .unwrap();
    block_on(nina.outbox_flush()).unwrap();
    let nina_c = match block_on(nina.write_status(&tc.id)).unwrap().unwrap().state {
        lessdugin_core::dto::WriteState::Done { uri, .. } => uri,
        other => panic!("{other:?}"),
    };
    s.w.tick(MINUTE);
    // bob point-syncs nina's comment (sync.md §4.2: an unknown author is confirmed first),
    // then replies: the reply pulls nina's repo into the closure for everyone else
    block_on(bob.engine.sync_thread(&nina_c, &SyncOpts::default())).unwrap();
    block_on(bob.engine.comment(&lessdugin_core::dto::CommentDraft::new(
        &welcome,
        Some(nina_c),
        "Welcome!",
    )))
    .unwrap();
    bob.flush();
    s.w.tick(MINUTE);
    // nina's upvote: counting (she is a participant) but unweighted (outside E)
    block_on(nina.opinion(
        &welcome,
        &OpinionIntent::Karma {
            value: Some(KarmaValue::Up),
        },
    ))
    .unwrap();
    s.w.tick(MINUTE);
    block_on(nina.outbox_flush()).unwrap();
    s.w.tick(MINUTE);

    // carol's TUI shows the note on the list
    let mut t = block_on(lessdugin_tui::Headless::new(
        carol.engine.clone(),
        lessdugin_tui::HeadlessOpts {
            width: 100,
            height: 24,
            auto_sync: true,
            ..Default::default()
        },
    ))
    .unwrap();
    block_on(t.start(Some(forum.clone())));
    let screen = t.render();
    assert!(screen.contains("1 vote not yet counted"), "{screen}");

    // nina's own TUI shows her vote as not counted yet
    let mut tn = block_on(lessdugin_tui::Headless::new(
        nina,
        lessdugin_tui::HeadlessOpts {
            width: 140,
            height: 24,
            auto_sync: true,
            ..Default::default()
        },
    ))
    .unwrap();
    block_on(tn.start(Some(forum.clone())));
    let screen = tn.render();
    assert!(screen.contains("your: up (not counted yet)"), "{screen}");
}

/// An open forum where carol is a declared moderator (an authority passes every react gate,
/// so no karma is needed to start a react) and bob is a granted seed. Returns the forum uri,
/// bob's post and bob's comment on it.
fn react_forum(s: &common::Scenario) -> (String, String, String) {
    use lessdugin_core::dto::{
        BodyDraft, CommentDraft, DefaultTier, ForumDraft, ModRole, ModeratorDraft, PostDraft,
    };
    let (alice, bob, carol) = (&s.w.alice, &s.w.bob, &s.w.carol);
    let mut draft = ForumDraft::new("react", "React forum");
    draft.default_tier = DefaultTier::Frontpage;
    draft.about = Some(BodyDraft::text("Be kind."));
    draft.moderators = vec![ModeratorDraft {
        did: carol.did(),
        role: ModRole::Moderator,
        title: None,
    }];
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    s.w.tick(MINUTE);
    let t = block_on(alice.engine.publish(&PostDraft::article(
        &forum,
        "Seed post",
        "A first post.",
    )))
    .unwrap();
    alice.flush();
    let welcome = alice.done(&t);
    s.w.tick(MINUTE);
    block_on(bob.engine.forum_follow(&forum, None)).unwrap();
    block_on(bob.engine.join(&forum, None)).unwrap();
    bob.flush();
    block_on(carol.engine.forum_follow(&forum, None)).unwrap();
    block_on(carol.engine.join(&forum, None)).unwrap();
    carol.flush();
    block_on(alice.engine.grant(&forum, &[bob.did(), carol.did()])).unwrap();
    alice.flush();
    s.w.tick(MINUTE);
    let t = block_on(
        bob.engine
            .publish(&PostDraft::article(&forum, "Bob's post", "Bob writes.")),
    )
    .unwrap();
    bob.flush();
    let bob_post = bob.done(&t);
    bob.sync(&forum);
    s.w.tick(MINUTE);
    let t = block_on(
        bob.engine
            .comment(&CommentDraft::new(&welcome, None, "A comment.")),
    )
    .unwrap();
    bob.flush();
    let bob_c = bob.done(&t);
    s.w.tick(MINUTE);
    carol.sync(&forum);
    (forum, bob_post, bob_c)
}

#[test]
fn the_react_palette_opens_and_lists_the_palette() {
    let s = scenario();
    let (forum, _, _) = react_forum(&s);
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(forum.clone())));
    block_on(t.press("Enter"));
    assert!(t.on_thread(), "bob's post opened");
    block_on(t.press("x"));
    let screen = t.render();
    assert!(screen.contains("React"), "{screen}");
    assert!(screen.contains("Agreed"), "{screen}");
    insta::assert_snapshot!("react_palette_80x24", screen);
    // Tab switches the stance; Esc closes the palette
    block_on(t.press("Tab"));
    assert_eq!(
        t.state.react.as_ref().map(|o| o.stance),
        Some(lessdugin_core::dto::Stance::Against)
    );
    block_on(t.press("Esc"));
    assert!(t.state.react.is_none(), "the palette closed");
}

#[test]
fn a_react_shows_in_the_palette_and_under_the_item() {
    use lessdugin_core::dto::OpinionIntent;
    let s = scenario();
    let (forum, _, bob_c) = react_forum(&s);
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(forum.clone())));
    block_on(t.press("Enter"));
    // `x` on the post, `Enter` on the first entry ("Agreed")
    block_on(t.press("x"));
    block_on(t.press("Enter"));
    assert!(t.state.react.is_none(), "the palette closed");
    let st = status(&t);
    assert!(st.contains("React recorded"), "{st}");
    assert!(!t.state.op_writes.is_empty(), "the write is open");
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let doc = t.thread_document().unwrap();
    assert!(doc.contains("Agreed"), "{doc}");
    assert!(doc.contains("1 for") || doc.contains("yours"), "{doc}");
    insta::assert_snapshot!("react_under_item_80x24", t.render());

    // the same react is offered as the viewer's own, and picking it again takes it back
    block_on(t.press("x"));
    let o = t.state.react.as_ref().expect("the palette opened");
    let first = o.tallies.get("agree").expect("carol's tally");
    assert_eq!(first.mine, Some(lessdugin_core::dto::Stance::For));
    block_on(t.press("Enter"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let doc = t.thread_document().unwrap();
    assert!(!doc.contains("Agreed"), "the react is gone:\n{doc}");

    // a react on a comment (bob's comment under alice's post)
    let mut t2 = tui(&s, 80, 24);
    block_on(t2.start(Some(forum.clone())));
    block_on(t2.open_thread(&bob_c));
    // select bob's comment, then react
    while t2.state.top_thread().unwrap().layout.items[t2.state.top_thread().unwrap().pos.selected]
        .uri
        != bob_c
    {
        block_on(t2.press("j"));
    }
    block_on(t2.press("x"));
    block_on(t2.press("j"));
    block_on(t2.press("Enter"));
    let st = status(&t2);
    assert!(st.contains("React recorded"), "{st}");
    let _ = OpinionIntent::Karma { value: None };
}

#[test]
fn the_mod_menu_hides_an_item_with_a_reason() {
    use lessdugin_core::dto::{ItemState, ThreadOpts};
    let s = scenario();
    let (forum, bob_post, _) = react_forum(&s); // carol is a moderator there
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(forum.clone())));
    block_on(t.press("Enter")); // bob's post
    block_on(t.press("m"));
    let menu = t.state.mod_menu.as_ref().expect("the menu opened");
    assert!(
        menu.entries.iter().any(|e| e.label == "hide this item"),
        "{menu:?}"
    );
    let screen = t.render();
    assert!(screen.contains("Moderate"), "{screen}");
    insta::assert_snapshot!("mod_menu_80x24", t.render());
    // Enter: the reason prompt; type a reason and Enter: the mod_action is issued
    block_on(t.press("Enter"));
    assert!(t.state.prompt.is_some(), "the reason prompt opened");
    for k in ["p", "i", "l", "l", "s"] {
        block_on(t.press(k));
    }
    assert_eq!(
        t.state.prompt.as_ref().map(|p| p.text.as_str()),
        Some("pills")
    );
    block_on(t.press("Enter"));
    let st = status(&t);
    assert!(st.contains("Mod action recorded"), "{st}");
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    s.w.tick(MINUTE);
    // the post is hidden in bob's thread view (carol is an authority, so she sees the reason)
    let tv = block_on(t.engine().get_thread(&bob_post, &ThreadOpts::default())).unwrap();
    let lessdugin_core::dto::ThreadRoot::Post(p) = &tv.post else {
        panic!("the post is gone");
    };
    assert!(
        matches!(
            p.post.state,
            ItemState::Hidden {
                reason: lessdugin_core::dto::HiddenReason::Removed { .. }
            }
        ),
        "{:?}",
        p.post.state
    );
    // the mod log shows the action with its status and reason
    let log = block_on(t.engine().mod_log(&forum, None, 20)).unwrap();
    assert!(
        log.entries.iter().any(|e| e.action == "hide"
            && e.value.contains("hidden")
            && e.reason.as_deref() == Some("pills")),
        "{:?}",
        log.entries
    );
}

#[test]
fn the_mod_queue_and_log_screens() {
    use lessdugin_core::dto::{ModActionValue, ModScope};
    let s = scenario();
    let (forum, bob_post, _) = react_forum(&s);
    // alice (the owner, rank 3) hides bob's post, carol (a moderator, rank 1) keeps it: the
    // log then shows carol's keep as overridden by the higher rank (mod/1 resolution)
    block_on(s.w.alice.engine.mod_action(&ModActionIntent::Assert {
        forum: forum.clone(),
        scope: ModScope::Forum,
        subject: bob_post.clone(),
        action: ModActionValue::Hide {
            hidden: true,
            spam: false,
            show_reason: true,
        },
        reason: None,
        until_ms: None,
    }))
    .unwrap();
    s.w.alice.flush();
    block_on(s.w.carol.engine.mod_action(&ModActionIntent::Assert {
        forum: forum.clone(),
        scope: ModScope::Forum,
        subject: bob_post.clone(),
        action: ModActionValue::Hide {
            hidden: false,
            spam: false,
            show_reason: true,
        },
        reason: Some("keep".into()),
        until_ms: None,
    }))
    .unwrap();
    s.w.tick(MINUTE);
    s.w.carol.flush();
    s.w.tick(MINUTE);
    // carol (a moderator) opens the queue through the menu
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(forum.clone())));
    block_on(t.press("m"));
    // go to the "mod queue" entry
    let n = t
        .state
        .mod_menu
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .position(|e| e.queue)
        .expect("a queue entry");
    for _ in 0..n {
        block_on(t.press("j"));
    }
    block_on(t.press("Enter"));
    let screen = t.render();
    assert!(screen.contains("queue"), "{screen}");
    insta::assert_snapshot!("mod_queue_screen_80x24", t.render());
    // Tab switches to the log: carol's keep shows as overridden by alice's rank-3 hide,
    // with its status and reason
    block_on(t.press("Tab"));
    let screen = t.render();
    assert!(screen.contains("log"), "{screen}");
    assert!(screen.contains("overridden"), "{screen}");
    assert!(screen.contains("hidden"), "{screen}");
    assert!(screen.contains("keep"), "{screen}");
    insta::assert_snapshot!("mod_log_screen_80x24", t.render());
}

#[test]
fn the_mod_queue_is_blocked_for_a_plain_member() {
    let s = scenario();
    // in the default (approval) scenario forum carol is a plain member: `m` is refused
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("m"));
    assert!(t.state.mod_menu.is_none(), "no menu for a plain member");
    assert!(status(&t).contains("cannot moderate"), "{}", status(&t));
}

#[test]
fn an_applicant_shows_in_the_queue_and_enter_approves() {
    let s = scenario();
    // dave applies to the approval-mode forum (a membership without a grant)
    let dave_acct = s.w.net.add_account("dave.example.com");
    let h = MemHost::new();
    h.set_seed(11);
    s.w.net.install_as(&h, "dave");
    let dave = Engine::new(h, EngineConfig::default());
    block_on(dave.add_account(AccountInfo::app_password(
        dave_acct.did().as_str(),
        dave_acct.pds_base(),
    )))
    .unwrap();
    block_on(dave.forum_follow(&s.forum, None)).unwrap();
    block_on(dave.join(&s.forum, Some("let me in"))).unwrap();
    block_on(dave.outbox_flush()).unwrap();
    block_on(dave.sync_forum(&s.forum, &SyncOpts::default())).unwrap();
    s.w.tick(MINUTE);
    // dave comments; bob (a seed) replies, which pulls dave's repo into the closure
    // (sync.md §4.3) so alice's next sync discovers dave's membership (the application)
    let td = block_on(dave.comment(&lessdugin_core::dto::CommentDraft::new(
        &s.welcome,
        None,
        "Let me in!",
    )))
    .unwrap();
    s.w.tick(MINUTE);
    block_on(dave.outbox_flush()).unwrap();
    let dave_c = match block_on(dave.write_status(&td.id)).unwrap().unwrap().state {
        lessdugin_core::dto::WriteState::Done { uri, .. } => uri,
        other => panic!("{other:?}"),
    };
    s.w.tick(MINUTE);
    // alice point-syncs dave's comment (sync.md §4.2: the unknown author is confirmed),
    // which caches dave's membership: his application shows in her queue
    block_on(s.w.alice.engine.sync_thread(&dave_c, &SyncOpts::default())).unwrap();
    s.w.tick(MINUTE);
    s.w.alice.sync(&s.forum);

    // alice (the owner) opens the Mod screen: the queue lists dave as an applicant
    let mut t = block_on(lessdugin_tui::Headless::new(
        s.w.alice.engine.clone(),
        lessdugin_tui::HeadlessOpts {
            width: 100,
            height: 24,
            ..Default::default()
        },
    ))
    .unwrap();
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("m"));
    let n = t
        .state
        .mod_menu
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .position(|e| e.queue)
        .expect("a queue entry");
    for _ in 0..n {
        block_on(t.press("j"));
    }
    block_on(t.press("Enter"));
    let screen = t.render();
    assert!(screen.contains("applicant"), "{screen}");
    assert!(screen.contains("dave"), "{screen}");
    insta::assert_snapshot!("mod_queue_applicant_80x24", t.render());

    // Enter approves (engine.grant); the queue empties once the write lands
    block_on(t.press("Enter"));
    assert!(status(&t).contains("approved"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.press("R"));
    let screen = t.render();
    assert!(screen.contains("queue is empty"), "{screen}");
    // dave is granted now
    let members = block_on(
        s.w.alice
            .engine
            .members(&s.forum, &lessdugin_core::dto::MembersQuery::default()),
    )
    .unwrap();
    assert!(
        members
            .iter()
            .any(|m| m.actor.did == dave_acct.did().to_string() && m.granted),
        "{members:?}"
    );
}

#[test]
fn the_activity_tab_lists_recent_discussion() {
    let s = scenario();
    // bob's post and his comment on the welcome both carry comments/activity; give his post a
    // comment so the Activity tab shows a post *with* its newest comment under it
    let t = block_on(
        s.w.carol
            .engine
            .comment(&lessdugin_core::dto::CommentDraft::new(
                &s.bob_post,
                None,
                "Nice first post, bob.",
            )),
    )
    .unwrap();
    s.w.carol.flush();
    s.w.tick(MINUTE);
    let _carol_c = s.w.carol.done(&t);

    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    // Posts is the default tab: the sort tabs and the post list show
    let screen = t.render();
    assert!(screen.contains("Hello from bob"), "{screen}");
    assert!(screen.contains("posts"), "{screen}");

    // Tab: the Activity tab (engine.recent_discussion)
    block_on(t.press("Tab"));
    let screen = t.render();
    assert!(screen.contains("activity"), "{screen}");
    assert!(screen.contains("Hello from bob"), "{screen}");
    assert!(
        screen.contains("Nice first post, bob."),
        "the newest comment under the post:\n{screen}"
    );
    insta::assert_snapshot!("forum_activity_tab_100x30", t.render());

    // the cursor belongs to the tab: j moves it, and Tab back keeps the posts cursor where it was
    let posts_cursor = t.state.top_forum_screen().unwrap().pos.cursor;
    block_on(t.press("j"));
    assert_eq!(t.state.top_forum_screen().unwrap().activity_pos.cursor, 1);
    assert_eq!(t.state.top_forum_screen().unwrap().pos.cursor, posts_cursor);
    block_on(t.press("k"));
    assert_eq!(t.state.top_forum_screen().unwrap().activity_pos.cursor, 0);

    // Tab back to Posts
    block_on(t.press("Tab"));
    assert_eq!(
        t.state.top_forum_screen().unwrap().tab,
        lessdugin_tui::ForumTab::Posts
    );
}

#[test]
fn the_activity_tab_calls_recent_discussion() {
    // the same data the engine returns, so the tab is not inventing it
    let s = scenario();
    let list = block_on(s.w.carol.engine.recent_discussion(&s.forum, None, 50)).unwrap();
    assert!(!list.items.is_empty(), "the scenario has listed posts");

    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("Tab"));
    let screen = t.render();
    for item in &list.items {
        // a post with no title shows its excerpt's first line (post_title); assert on whichever
        let shown = item.post.title.as_deref().unwrap_or(&item.post.excerpt);
        let first = shown.lines().next().unwrap_or_default().trim();
        assert!(
            !first.is_empty() && screen.contains(first),
            "the tab lists {first:?}:\n{screen}"
        );
        for c in &item.comments {
            // each comment shows the first line of its body (the tab renders bodies from the
            // records it fetched, falling back to the summary excerpt, which collapses lines)
            let excerpt = c.body.excerpt.trim();
            let prefix: String = excerpt.chars().take(20).collect();
            let prefix = prefix.trim();
            assert!(
                !prefix.is_empty() && screen.contains(prefix),
                "the comment's text {prefix:?} shows:\n{screen}"
            );
        }
    }
}
