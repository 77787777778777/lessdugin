// SPDX-License-Identifier: AGPL-3.0-or-later
//! The M3 social surface over a `TestBackend`: opinions (`synthesis-v1.md` §6.11), viewer caps,
//! reacts, moderation, notifications and the `:` command line.

mod common;

use common::{MINUTE, block_on, scenario, tui};
use lessdugin_core::dto::{
    AccessLevel, CommentDraft, ModActionIntent, ModActionValue, ModScope, SyncOpts,
};

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

/// Switching a held react's stance is ONE opinion effect: the runtime runs effects as separate
/// tasks, so a remove-then-add pair could land add-first and the remove would delete the react.
#[test]
fn a_stance_switch_is_one_add() {
    use lessdugin_core::dto::{OpinionIntent, Stance};
    use lessdugin_tui::{Action, Effect};
    let s = scenario();
    let (forum, _, _) = react_forum(&s);
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(forum.clone())));
    block_on(t.press("Enter"));
    // carol reacts "Agreed" for, and the write lands
    block_on(t.keys(&["x", "Enter"]));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    // the palette again, against, Enter: the effects of that one key
    block_on(t.keys(&["x", "Tab"]));
    let fx = t.state.update(Action::Key(
        lessdugin_tui::keys::parse_key("Enter").unwrap(),
    ));
    let opinions: Vec<&OpinionIntent> = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Opinion { intent, .. } => Some(intent),
            _ => None,
        })
        .collect();
    assert_eq!(
        opinions,
        [&OpinionIntent::React {
            id: "agree".into(),
            stance: Stance::Against,
            quote: None,
            remove: false,
        }],
        "{fx:?}"
    );
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

/// dave applies to the approval-mode forum of `s` (a membership without a grant) and alice
/// learns of it; returns his DID.
fn dave_applies(s: &common::Scenario) -> String {
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
    dave_acct.did().to_string()
}

/// alice's TUI on the Mod screen's queue of `s`'s forum.
fn alice_queue(s: &common::Scenario) -> Tui {
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
    t
}

#[test]
fn an_applicant_shows_in_the_queue_and_enter_approves() {
    let s = scenario();
    let dave = dave_applies(&s);
    // alice (the owner) opens the Mod screen: the queue lists dave as an applicant
    let mut t = alice_queue(&s);
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
        members.iter().any(|m| m.actor.did == dave && m.granted),
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

/// Types a command line into the `:` overlay (one key per character) and presses Enter.
fn run_command(t: &mut Tui, line: &str) {
    block_on(t.press(":"));
    assert!(t.state.command.is_some(), "the overlay opens on `:`");
    for c in line.chars() {
        block_on(t.press(&c.to_string()));
    }
    assert_eq!(t.state.command.as_deref(), Some(line), "typed verbatim");
    block_on(t.press("Enter"));
    assert!(t.state.command.is_none(), "Enter closes the overlay");
}

#[test]
fn the_command_line_votes_end_to_end() {
    let s = scenario();
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    assert_eq!(points_of(&t.render(), "Hello from bob"), "1 point");

    // `: vote up` -> the same Effect::Opinion the `u` key queues
    run_command(&mut t, "vote up");
    assert!(status(&t).contains("Vote recorded"), "{}", status(&t));
    assert!(!t.state.op_writes.is_empty(), "the write is open");

    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    assert_eq!(points_of(&t.render(), "Hello from bob"), "2 points");

    // `: agree for` goes through the agreement axis
    run_command(&mut t, "agree for");
    assert!(status(&t).contains("Vote recorded"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let screen = t.render();
    assert!(
        screen.contains("agree"),
        "the agreement shows in the meta line:\n{screen}"
    );

    // `: 0`-equivalent: the agreement axis clears
    run_command(&mut t, "vote clear");
    assert!(status(&t).contains("Vote recorded"), "{}", status(&t));
}

#[test]
fn the_command_line_moderates_end_to_end() {
    let s = scenario();
    let (forum, bob_post, _) = react_forum(&s);
    // carol is a moderator here: `: mod hide <uri> "reason"` issues the same mod_action as `m`
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(forum.clone())));
    run_command(&mut t, &format!("mod hide {bob_post} \"off topic\""));
    assert!(status(&t).contains("Mod action"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    // the write reached the engine: the thread view holds the hide with its reason
    use lessdugin_core::dto::{HiddenReason, ItemState, ThreadOpts};
    let tv = block_on(t.engine().get_thread(&bob_post, &ThreadOpts::default())).unwrap();
    let lessdugin_core::dto::ThreadRoot::Post(p) = &tv.post else {
        panic!("the post is gone");
    };
    assert!(
        matches!(
            p.post.state,
            ItemState::Hidden {
                reason: HiddenReason::Removed { .. }
            }
        ),
        "{:?}",
        p.post.state
    );
    // and the mod log records it with the reason the command line carried
    let log = block_on(t.engine().mod_log(&forum, None, 20)).unwrap();
    assert!(
        log.entries.iter().any(|e| e.action == "hide"
            && e.value.contains("hidden")
            && e.reason.as_deref() == Some("off topic")),
        "{:?}",
        log.entries
    );
}

#[test]
fn the_command_line_opens_activity_and_notifications() {
    let s = scenario();
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    // `: activity` switches the forum to its Activity tab
    run_command(&mut t, "activity");
    assert_eq!(
        t.state.top_forum_screen().unwrap().tab,
        lessdugin_tui::ForumTab::Activity
    );
    assert!(t.state.top_forum_screen().unwrap().activity.is_some());

    // `: notifications` opens the Notifications screen
    run_command(&mut t, "notifications");
    assert!(
        matches!(
            t.state.top().map(|s| &s.kind),
            Some(lessdugin_tui::app::ScreenKind::Notifications(_))
        ),
        "the notifications screen is on top"
    );
}

#[test]
fn the_command_line_reports_a_bad_verb() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    run_command(&mut t, "frobnicate");
    let st = status(&t);
    assert!(st.contains("Unknown command"), "{st}");
    assert!(t.state.op_writes.is_empty());
}

#[test]
fn the_command_line_overlay_renders() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press(":"));
    for c in "mod hide".chars() {
        block_on(t.press(&c.to_string()));
    }
    let screen = t.render();
    // the overlay draws at the bottom, with the text typed so far
    let last_line = screen.lines().last().unwrap_or_default();
    assert!(last_line.contains(":mod hide"), "{screen}");
    insta::assert_snapshot!("command_line_overlay_80x24", screen);

    // Esc closes it and leaves the screen as it was
    block_on(t.press("Esc"));
    assert!(t.state.command.is_none());
    let after = t.render();
    assert!(!after.contains(":mod hide"), "{after}");
}

#[test]
fn the_command_line_sets_and_lists_moderators() {
    let s = scenario();
    let (forum, _, _) = react_forum(&s);
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(forum.clone())));
    // carol is a moderator here, not the owner: `mods add` is queued and the engine decides
    run_command(&mut t, &format!("mods add {}", s.w.bob.did()));
    assert!(
        status(&t).contains("moderator list") || status(&t).contains("refused"),
        "{}",
        status(&t)
    );

    // `mods` lists what the loaded forum view declares (react_forum declares carol)
    run_command(&mut t, "mods");
    let st = status(&t);
    assert!(st.contains("Moderators:"), "the listing reports: {st}");
    assert!(st.contains("carol"), "{st}");

    // and `mods add` with no actor hands back the usage error, not a panic
    run_command(&mut t, "mods add");
    assert!(status(&t).contains("needs a DID"), "{}", status(&t));
}

#[test]
fn the_command_line_adds_a_moderator_end_to_end() {
    let s = scenario();
    let (forum, _, _) = react_forum(&s);
    // alice owns the forum: her `mods add` writes the declaration
    let mut t = block_on(lessdugin_tui::Headless::new(
        s.w.alice.engine.clone(),
        lessdugin_tui::HeadlessOpts {
            width: 100,
            height: 30,
            auto_sync: false,
            ..Default::default()
        },
    ))
    .unwrap();
    block_on(t.start(Some(forum.clone())));
    run_command(&mut t, &format!("mods add {} admin", s.w.bob.did()));
    assert!(
        status(&t).contains("Updating the moderator list")
            || status(&t).contains("Moderator list updated"),
        "{}",
        status(&t)
    );
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    // the declaration now names bob
    let view = block_on(t.engine().forum_open(&forum)).unwrap();
    assert!(
        view.moderators.iter().any(|m| m.actor.did == s.w.bob.did()),
        "{:?}",
        view.moderators
    );
}

// --- scope 5: the Notifications screen (§6.11; social.md §16) -------------------------------

/// carol comments on alice's welcome post; alice's device then has a notification for it.
///
/// The FIRST computation for an (account, forum) only records a baseline (`notify.rs`: a new
/// device does not replay history), so this first query is what lets the second one notify.
fn alice_with_a_notification(s: &common::Scenario) -> String {
    let (alice, carol) = (&s.w.alice, &s.w.carol);
    // the baseline is recorded BEFORE the comment exists, so the comment is news
    let _baseline = block_on(alice.engine.notifications(&Default::default())).unwrap();
    let t = block_on(carol.engine.comment(&CommentDraft::new(
        &s.welcome,
        None,
        "Thanks @alice.example.com! Carol answers from the TUI.",
    )))
    .unwrap();
    carol.flush();
    let comment = carol.done(&t);
    s.w.tick(MINUTE);
    // the author's device folds the comment in through a forum sync, which raises the note
    alice.sync(&s.forum);
    comment
}

#[test]
fn the_notifications_screen_lists_unread_reasons_and_marks_them_read() {
    let s = scenario();
    let _comment = alice_with_a_notification(&s);
    // alice drives this test: the notifications belong to the welcome post's author
    let mut t = block_on(lessdugin_tui::Headless::new(
        s.w.alice.engine.clone(),
        lessdugin_tui::HeadlessOpts {
            width: 80,
            height: 24,
            ..lessdugin_tui::HeadlessOpts::default()
        },
    ))
    .unwrap();
    block_on(t.start(None));

    // the header carries the badge before the screen is even opened: two replies, both unread
    let badge = t.render();
    assert!(badge.contains("[N 2]"), "{badge}");
    block_on(t.press("N"));
    let screen = t.render();
    // the reason kind, the actors (as verified-or-claimed handles) and the preview
    assert!(screen.contains("reply"), "{screen}");
    assert!(screen.contains("bob.example.com"), "{screen}");
    assert!(screen.contains("carol.example.com"), "{screen}");
    assert!(screen.contains("2 unread"), "{screen}");
    insta::assert_snapshot!("notifications_screen_80x24", t.render());

    // `M` marks them read: the unread counter drains and the rows lose their `*`
    assert!(t.state.unread.as_ref().is_some_and(|u| u.notifications > 0));
    block_on(t.press("M"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    assert_eq!(
        t.state.unread.as_ref().map(|u| u.notifications),
        Some(0),
        "{:?}",
        t.state.unread
    );
    let screen = t.render();
    // nothing is marked unread any more
    assert!(screen.contains("0 unread"), "{screen}");
    assert!(!screen.starts_with("* "), "{screen}");
}

#[test]
fn mark_all_read_outside_the_screen_says_where_it_works() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("M"));
    assert!(
        status(&t).contains("notifications screen (N)"),
        "{}",
        status(&t)
    );
}

#[test]
fn versions_panel_shows_the_history_and_says_when_it_was_redacted() {
    // `E` (M4: "history view and redaction"). The panel hangs under the thread, so it never
    // displaces the item being read, and a redacted item says so rather than showing nothing.
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    // the welcome post is second (newest first), and it is the one with a revision
    block_on(t.keys(&["j", "Enter"]));
    assert!(
        t.on_thread(),
        "the test must be on a thread for its history"
    );
    block_on(t.press("E"));
    let screen = t.render();
    assert!(
        screen.contains("History"),
        "the panel must be visible:\n{screen}"
    );
    // a second `E` closes it again
    block_on(t.press("E"));
    let screen = t.render();
    assert!(
        !screen.contains("History"),
        "the panel must close:\n{screen}"
    );
}

/// alice's TUI (headless, syncing as the TUI does).
fn alice_tui(s: &common::Scenario, width: u16, height: u16) -> Tui {
    block_on(lessdugin_tui::Headless::new(
        s.w.alice.engine.clone(),
        lessdugin_tui::HeadlessOpts {
            width,
            height,
            ..lessdugin_tui::HeadlessOpts::default()
        },
    ))
    .unwrap()
}

/// Viewing a thread sets its read mark (`social.md` §16): the notifications about it are read
/// once it has been opened, and the badge drops.
#[test]
fn opening_a_thread_marks_it_read() {
    let s = scenario();
    let _comment = alice_with_a_notification(&s);
    let mut t = alice_tui(&s, 80, 24);
    block_on(t.start(None));
    assert!(t.render().contains("[N 2]"), "{}", t.render());
    block_on(t.open_thread(&s.welcome));
    let n = block_on(t.engine().notifications(&Default::default())).unwrap();
    assert_eq!(n.unread, 0, "{n:?}");
    let screen = t.render();
    assert!(!screen.contains("[N "), "{screen}");
}

/// `:read` marks the selected notification read, not every listed one.
#[test]
fn the_read_command_marks_only_the_selected_notification() {
    let s = scenario();
    let _comment = alice_with_a_notification(&s);
    let mut t = alice_tui(&s, 80, 24);
    block_on(t.start(None));
    block_on(t.press("N"));
    run_command(&mut t, "read");
    let n = block_on(t.engine().notifications(&Default::default())).unwrap();
    assert_eq!(n.unread, 1, "one of the two is read: {n:?}");
    // the screen follows
    assert!(t.render().contains("1 unread"), "{}", t.render());
    // `read-all` takes the rest
    run_command(&mut t, "read-all");
    let n = block_on(t.engine().notifications(&Default::default())).unwrap();
    assert_eq!(n.unread, 0, "{n:?}");
}

/// Types `line` into the `:` overlay and returns the effects of its Enter (not run).
fn command_effects(t: &mut Tui, line: &str) -> Vec<lessdugin_tui::Effect> {
    block_on(t.press(":"));
    for c in line.chars() {
        block_on(t.press(&c.to_string()));
    }
    t.state.update(lessdugin_tui::Action::Key(
        lessdugin_tui::keys::parse_key("Enter").unwrap(),
    ))
}

/// The opinion subjects of `fx`.
fn opinion_subjects(fx: &[lessdugin_tui::Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            lessdugin_tui::Effect::Opinion { subject, .. } => Some(subject.clone()),
            _ => None,
        })
        .collect()
}

/// `:vote`, `:agree`, `:react` and `:unreact` act on the subject the command names, not on
/// the selection; and they work where nothing is selected (the Notifications screen).
#[test]
fn opinion_commands_act_on_the_named_subject() {
    use lessdugin_core::dto::{KarmaValue, ThreadOpts, ThreadRoot};
    let s = scenario();
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    // bob's post is selected (the first); the command names the welcome post
    assert_eq!(
        t.state.top_forum_screen().unwrap().selected().unwrap().uri,
        s.bob_post
    );
    let fx = command_effects(&mut t, &format!("vote down {}", s.welcome));
    assert_eq!(
        opinion_subjects(&fx),
        std::slice::from_ref(&s.welcome),
        "{fx:?}"
    );
    for line in [
        format!("agree for {}", s.welcome),
        format!("react agree for {}", s.welcome),
        format!("unreact agree {}", s.welcome),
    ] {
        let fx = command_effects(&mut t, &line);
        assert_eq!(
            opinion_subjects(&fx),
            std::slice::from_ref(&s.welcome),
            "{line}: {fx:?}"
        );
    }

    // end to end, from a screen with no selection
    block_on(t.press("N"));
    run_command(&mut t, &format!("vote up {}", s.bob_c1));
    assert!(status(&t).contains("Vote recorded"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let tv = block_on(t.engine().get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    let c = tv.comments.iter().find(|c| c.uri == s.bob_c1).unwrap();
    assert_eq!(
        c.score.mine.as_ref().and_then(|m| m.karma),
        Some(KarmaValue::Up),
        "{:?}",
        c.score
    );
    let ThreadRoot::Post(p) = &tv.post else {
        panic!("no post")
    };
    assert_eq!(p.post.score.mine.as_ref().and_then(|m| m.karma), None);
}

/// `:mod keep` asserts `#hide{hidden: false}`: the owner overrides a moderator's hide from the
/// TUI (it was a removal of the owner's own, non-existent hide, refused with `not_found`).
#[test]
fn the_owner_keeps_an_item_a_moderator_hid() {
    use lessdugin_core::dto::{ItemState, ThreadOpts, ThreadRoot};
    let s = scenario();
    let (forum, bob_post, _) = react_forum(&s); // carol moderates, alice owns
    block_on(s.w.carol.engine.mod_action(&ModActionIntent::Assert {
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
    s.w.carol.flush();
    s.w.tick(MINUTE);
    s.w.alice.sync(&forum);
    let state = |t: &Tui| {
        let tv = block_on(t.engine().get_thread(&bob_post, &ThreadOpts::default())).unwrap();
        let ThreadRoot::Post(p) = tv.post else {
            panic!("no post")
        };
        p.post.state
    };
    let mut t = alice_tui(&s, 100, 30);
    block_on(t.start(Some(forum.clone())));
    assert!(
        matches!(state(&t), ItemState::Hidden { .. }),
        "{:?}",
        state(&t)
    );
    run_command(&mut t, &format!("mod keep {bob_post} \"on topic\""));
    assert!(status(&t).contains("Mod action recorded"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    assert_eq!(state(&t), ItemState::Visible);
}

/// A nullification keeps the restriction the same moderator asserted before it, and the other
/// way round: each sets one dimension of the one `#restrict` record (`social.md` §4.2).
#[test]
fn restrict_then_nullify_keeps_both() {
    use lessdugin_core::dto::MembersQuery;
    let s = scenario();
    let bob = s.w.bob.did();
    let mut t = alice_tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    let member = |t: &Tui| {
        let mut m = block_on(t.engine().members(&s.forum, &MembersQuery::default()))
            .unwrap()
            .into_iter()
            .find(|m| m.actor.did == bob)
            .unwrap();
        m.restricted.sort();
        m
    };
    run_command(&mut t, &format!("mod restrict post,comment {bob}"));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    let m = member(&t);
    assert_eq!(m.restricted, ["comment", "post"], "{m:?}");
    run_command(&mut t, &format!("mod nullify {bob}"));
    assert!(status(&t).contains("Mod action recorded"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    let m = member(&t);
    assert!(m.nullified, "{m:?}");
    assert_eq!(
        m.restricted,
        ["comment", "post"],
        "the restriction stays: {m:?}"
    );
    // and a restriction keeps the nullification
    run_command(&mut t, &format!("mod restrict vote {bob}"));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    let m = member(&t);
    assert!(m.nullified, "the nullification stays: {m:?}");
    assert_eq!(m.restricted, ["vote"], "{m:?}");
}

/// bob's membership as alice's engine reads it, its restrictions sorted.
fn bob_member(t: &Tui, s: &common::Scenario) -> lessdugin_core::dto::MemberView {
    let mut m = block_on(t.engine().members(&s.forum, &Default::default()))
        .unwrap()
        .into_iter()
        .find(|m| m.actor.did == s.w.bob.did())
        .unwrap();
    m.restricted.sort();
    m
}

/// The same, when the first write is still in the outbox (not sent, or offline): the second
/// is completed from the pending write, not from the cache that holds confirmed writes only.
#[test]
fn restrict_then_nullify_before_the_first_is_sent_keeps_both() {
    let s = scenario();
    let bob = s.w.bob.did();
    let mut t = alice_tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    run_command(&mut t, &format!("mod restrict post,comment {bob}"));
    run_command(&mut t, &format!("mod nullify {bob}"));
    assert!(status(&t).contains("Mod action recorded"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    let m = bob_member(&t, &s);
    assert!(m.nullified, "{m:?}");
    assert_eq!(m.restricted, ["comment", "post"], "{m:?}");
    // and a retraction still in the outbox leaves nothing to keep
    run_command(&mut t, &format!("mod unrestrict {bob}"));
    run_command(&mut t, &format!("mod restrict vote {bob}"));
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    let m = bob_member(&t, &s);
    assert!(!m.nullified, "{m:?}");
    assert_eq!(m.restricted, ["vote"], "{m:?}");
}

/// The mod menu's capability presets add up: "voting, reacting" after "posting" restricts
/// both (each states the record's whole set, `social.md` §4.7), even before the first is sent.
#[test]
fn the_menu_restrictions_add_up() {
    let s = scenario();
    let mut t = alice_tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.open_thread(&s.bob_post));
    let pick = |t: &mut Tui, label: &str| {
        block_on(t.press("m"));
        let menu = t.state.mod_menu.as_ref().expect("the menu opened");
        let n = menu
            .entries
            .iter()
            .position(|e| e.label == label)
            .unwrap_or_else(|| panic!("no {label:?} in {menu:?}"));
        for _ in 0..n {
            block_on(t.press("j"));
        }
        block_on(t.press("Enter"));
        assert!(t.state.prompt.is_some(), "the reason prompt opened");
        block_on(t.press("Enter"));
        assert!(status(t).contains("Mod action recorded"), "{}", status(t));
    };
    pick(&mut t, "restrict the author (posting)");
    pick(&mut t, "restrict the author (voting, reacting)");
    block_on(t.engine().outbox_flush()).unwrap();
    s.w.tick(MINUTE);
    block_on(t.settle());
    let m = bob_member(&t, &s);
    assert_eq!(m.restricted, ["comment", "post", "react", "vote"], "{m:?}");
}

/// The "this is public" interstitial (`social.md` §17, `threat-model.md` PRIV-9,
/// `engine-api.md` §3.6): the first public action of each kind waits behind it; `n` drops it,
/// `y` sends it and records the acknowledgement, which every client on the device shares.
#[test]
fn the_first_public_action_of_each_kind_asks_first() {
    use lessdugin_core::dto::PublicActionKind;
    let s = common::scenario_unacknowledged();
    let engine = s.w.carol.engine.clone();
    let acked = |k| block_on(engine.public_action_acknowledged(k)).unwrap();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    let writes = |t: &Tui| block_on(t.engine().writes()).unwrap().len();

    // `u`: nothing is written yet
    block_on(t.press("u"));
    let n = t
        .state
        .public_notice
        .as_ref()
        .expect("the interstitial opens");
    assert_eq!(n.kinds, [PublicActionKind::Vote]);
    let screen = t.render();
    assert!(screen.contains("This is public"), "{screen}");
    assert!(screen.contains("y go on"), "{screen}");
    insta::assert_snapshot!("public_notice_vote_80x24", screen);
    assert_eq!(writes(&t), 0, "the vote waits");
    // keys go to the interstitial, not to the screen below
    block_on(t.press("j"));
    assert!(t.state.public_notice.is_some());
    assert_eq!(writes(&t), 0);
    // nor does Enter go past it (it queued most of the held writes: a second press, or a key
    // repeat, must not publish unread)
    block_on(t.press("Enter"));
    assert!(t.state.public_notice.is_some(), "Enter is not a yes");
    assert_eq!(writes(&t), 0);
    assert!(!acked(PublicActionKind::Vote));

    // `n` cancels: nothing written, nothing acknowledged
    block_on(t.press("n"));
    assert!(t.state.public_notice.is_none());
    assert!(
        status(&t).contains("nothing was published"),
        "{}",
        status(&t)
    );
    assert_eq!(writes(&t), 0);
    assert!(!acked(PublicActionKind::Vote));

    // `u` asks again; `y` sends the vote and records the acknowledgement
    block_on(t.press("u"));
    assert!(t.state.public_notice.is_some());
    block_on(t.press("y"));
    assert!(t.state.public_notice.is_none());
    assert!(status(&t).contains("Vote recorded"), "{}", status(&t));
    assert_eq!(writes(&t), 1);
    assert!(acked(PublicActionKind::Vote));
    // the next vote does not ask, nor does `0` (two intents, one kind)
    block_on(t.press("d"));
    assert!(t.state.public_notice.is_none());
    block_on(t.press("0"));
    assert!(t.state.public_notice.is_none());

    // a react is another kind, and so is a mod action, whichever key or command queues it
    block_on(t.press("x"));
    block_on(t.press("Enter"));
    let n = t.state.public_notice.as_ref().expect("the react asks");
    assert_eq!(n.kinds, [PublicActionKind::React]);
    block_on(t.press("Esc"));
    assert!(!acked(PublicActionKind::React));
    let fx = command_effects(&mut t, &format!("mod hide {}", s.bob_post));
    assert!(
        !fx.iter()
            .any(|e| matches!(e, lessdugin_tui::Effect::ModAction { .. })),
        "{fx:?}"
    );
    let n = t.state.public_notice.as_ref().expect("the mod action asks");
    assert_eq!(n.kinds, [PublicActionKind::ModAction]);
    assert_eq!(n.held.len(), 1);
    block_on(t.press("n"));

    // another client on the device acknowledges: this session follows
    block_on(engine.acknowledge_public_action(PublicActionKind::React)).unwrap();
    block_on(t.settle());
    block_on(t.press("x"));
    block_on(t.press("Enter"));
    assert!(t.state.public_notice.is_none(), "{}", t.render());
    assert!(status(&t).contains("React recorded"), "{}", status(&t));

    // a new session reads the acknowledgements back
    let mut t2 = tui(&s, 80, 24);
    block_on(t2.start(Some(s.forum.clone())));
    block_on(t2.press("u"));
    assert!(t2.state.public_notice.is_none());
    assert!(status(&t2).contains("Vote recorded"), "{}", status(&t2));
}

/// A join (`:join`) and an approval from the mod queue (Enter) are public actions too: each
/// waits behind the interstitial of its kind.
#[test]
fn a_join_and_a_queue_approval_ask_first() {
    use lessdugin_core::dto::PublicActionKind;
    use lessdugin_tui::Effect;
    let s = common::scenario_unacknowledged();
    // carol is granted but has not joined
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    let fx = command_effects(&mut t, "join");
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::Join { .. })),
        "{fx:?}"
    );
    let n = t.state.public_notice.as_ref().expect("the join asks");
    assert_eq!(n.kinds, [PublicActionKind::Join]);
    assert!(matches!(n.held[..], [Effect::Join { .. }]), "{n:?}");
    block_on(t.press("n"));
    assert!(block_on(t.engine().writes()).unwrap().is_empty());

    // Enter on an applicant in alice's queue: the grant waits
    let dave = dave_applies(&s);
    let mut t = alice_queue(&s);
    assert!(t.render().contains("applicant"), "{}", t.render());
    let before = block_on(t.engine().writes()).unwrap().len();
    let fx = t.state.update(lessdugin_tui::Action::Key(
        lessdugin_tui::keys::parse_key("Enter").unwrap(),
    ));
    assert!(
        !fx.iter().any(|e| e.operation() == Some("grant")),
        "{fx:?}"
    );
    let n = t.state.public_notice.as_ref().expect("the approval asks");
    assert_eq!(n.kinds, [PublicActionKind::ModAction]);
    assert!(
        n.held.iter().all(|e| e.operation() == Some("grant")),
        "{n:?}"
    );
    // `y` sends it
    block_on(t.press("y"));
    assert!(t.state.public_notice.is_none());
    assert!(block_on(t.engine().writes()).unwrap().len() > before);
    block_on(t.engine().outbox_flush()).unwrap();
    let members = block_on(
        s.w.alice
            .engine
            .members(&s.forum, &lessdugin_core::dto::MembersQuery::default()),
    )
    .unwrap();
    assert!(
        members.iter().any(|m| m.actor.did == dave && m.granted),
        "{members:?}"
    );
}

/// The acknowledgements are the active account's: when another client on the device switches
/// to an account that has not seen the interstitial, the next public action asks again.
#[test]
fn another_active_account_asks_again() {
    use lessdugin_core::dto::PublicActionKind;
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("u"));
    assert!(t.state.public_notice.is_none(), "carol acknowledged votes");
    // erin logs in on carol's device (and is the active account now)
    let erin = s.w.net.add_account("erin.example.com");
    block_on(t.engine().add_account(AccountInfo::app_password(
        erin.did().as_str(),
        erin.pds_base(),
    )))
    .unwrap();
    block_on(t.settle());
    block_on(t.press("d"));
    let n = t.state.public_notice.as_ref().expect("erin's first vote asks");
    assert_eq!(n.kinds, [PublicActionKind::Vote]);
}

/// The operations of `engine-api.md` §3.2 (forums and membership), §3.5 (writes) and §3.6
/// (local state), by name, from the spec's tables.
fn spec_operations() -> std::collections::BTreeSet<String> {
    let spec = include_str!("../../../docs/spec/engine-api.md");
    let mut ops = std::collections::BTreeSet::new();
    let mut on = false;
    for line in spec.lines() {
        if line.starts_with("### ") {
            on = ["### 3.2 ", "### 3.5 ", "### 3.6 "]
                .iter()
                .any(|h| line.starts_with(h));
            continue;
        }
        if !on || !line.starts_with("| `") {
            continue;
        }
        let first = line.split('|').nth(1).unwrap_or_default();
        for code in first.split('`').skip(1).step_by(2) {
            let name: String = code
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                ops.insert(name);
            }
        }
    }
    ops
}

/// `docs/plan.md` §M3: "every MVP action is reachable in all three clients (in the TUI at least
/// through the `:` command line)". The TUI's `REACH` table names every operation of the spec's
/// tables, and nothing else; so an operation the spec adds fails here until the TUI reaches it
/// (or says why it does not).
#[test]
fn every_engine_operation_has_a_way_in() {
    use lessdugin_tui::command::{REACH, Reach};
    let spec = spec_operations();
    assert!(spec.len() > 40, "the spec's tables were not read: {spec:?}");
    let reach: std::collections::BTreeSet<String> =
        REACH.iter().map(|(op, _)| (*op).to_owned()).collect();
    assert_eq!(REACH.len(), reach.len(), "an operation is listed twice");
    let missing: Vec<&String> = spec.difference(&reach).collect();
    assert!(
        missing.is_empty(),
        "not reachable from the TUI: {missing:?}"
    );
    let stale: Vec<&String> = reach.difference(&spec).collect();
    assert!(stale.is_empty(), "not operations of the spec: {stale:?}");
    // every operation an M3 client must offer is a key or a command
    for (op, r) in REACH {
        if let Reach::NotHere(why) = r {
            assert!(
                why.starts_with("M5")
                    || op.contains("draft")
                    || op.contains("_local")
                    || *op == "prepare_media",
                "{op} must be reachable: {why}"
            );
        }
    }
}

/// Each key and command of `REACH` leads to its operation: run on alice's welcome thread with
/// bob's comment selected (and `read`/`read-all` on her Notifications screen), the effects
/// it queues (or holds behind a confirmation or the public-action interstitial) include one
/// that calls it.
#[test]
fn every_key_and_command_reaches_its_operation() {
    use lessdugin_tui::command::{REACH, Reach};
    let s = scenario();
    let _comment = alice_with_a_notification(&s);
    let mut t = alice_tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.open_thread(&s.welcome));
    let on_comment = |t: &Tui| {
        t.state
            .top_thread()
            .and_then(|th| th.layout.items.get(th.pos.selected))
            .is_some_and(|i| i.uri == s.bob_c1)
    };
    for _ in 0..4 {
        if !on_comment(&t) {
            block_on(t.press("j"));
        }
    }
    assert!(on_comment(&t), "{}", t.render());
    // an opinion in its undo window, for `undo`
    block_on(t.press("u"));
    assert!(status(&t).contains("Vote recorded"), "{}", status(&t));
    // opening the thread (and the vote's reload) read its notifications: a new one, for
    // `read` and `read-all`
    let _again = alice_with_a_notification(&s);
    block_on(t.settle());

    let notifications = ["mark_notifications_read", "mark_all_notifications_read"];
    for (op, r) in REACH {
        let fx = match r {
            Reach::Command(line) => {
                if notifications.contains(op) {
                    block_on(t.press("N"));
                }
                command_effects(&mut t, line)
            }
            Reach::Keys(keys) => {
                let (last, first) = keys.split_last().unwrap();
                for k in first {
                    block_on(t.press(k));
                }
                t.state.update(lessdugin_tui::Action::Key(
                    lessdugin_tui::keys::parse_key(last).unwrap(),
                ))
            }
            Reach::Automatic(_) | Reach::NotHere(_) => continue,
        };
        let mut all = fx;
        if let Some(c) = t.state.confirm.take() {
            all.extend(c.effects);
        }
        if let Some(n) = t.state.public_notice.take() {
            all.extend(n.held);
        }
        t.state.panel = None;
        let ops: Vec<Option<&str>> = all.iter().map(|e| e.operation()).collect();
        assert!(
            ops.contains(&Some(*op)),
            "{op} via {r:?}: {ops:?}, status {}",
            status(&t)
        );
        // back where it started
        if notifications.contains(op) {
            block_on(t.press("h"));
        }
        assert!(on_comment(&t), "{op} moved away:\n{}", t.render());
    }
}

/// Membership, own items and local state from the `:` line, end to end: carol joins with a
/// note, alice sees her in `:members`, carol deletes her comment after confirming, reads an
/// item's raw view, bookmarks, mutes, and alice renames the forum.
#[test]
fn the_command_line_reaches_membership_items_and_local_state() {
    use lessdugin_core::dto::{MembersQuery, OpinionIntent, ThreadOpts};
    let s = scenario();
    let (alice, carol) = (&s.w.alice, &s.w.carol);
    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));

    // `:join` writes the membership, with its note
    run_command(&mut t, "join Carol, from the TUI");
    assert!(status(&t).contains("Join: queued"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    alice.sync(&s.forum);
    let members = block_on(alice.engine.members(&s.forum, &MembersQuery::default())).unwrap();
    let c = members.iter().find(|m| m.actor.did == carol.did()).unwrap();
    assert_eq!(c.membership_note.as_deref(), Some("Carol, from the TUI"));
    // `:members` shows them in a panel; any key closes it
    run_command(&mut t, "members");
    block_on(t.settle());
    let panel = t.render();
    assert!(panel.contains("Members"), "{panel}");
    assert!(panel.contains("bob.example.com"), "{panel}");
    block_on(t.press("x"));
    assert!(t.state.panel.is_none());

    // `:react ... --quote` anchors the react to the passage
    block_on(t.open_thread(&s.welcome));
    let fx = command_effects(
        &mut t,
        &format!("react agree for {} --quote Thanks for the forum", s.bob_c1),
    );
    let quoted = fx.iter().any(|e| {
        matches!(e, lessdugin_tui::Effect::Opinion {
            intent: OpinionIntent::React { quote: Some(q), .. }, ..
        } if q.exact == "Thanks for the forum")
    });
    assert!(quoted, "{fx:?}");

    // `:raw` shows the record and the mod actions naming it
    run_command(&mut t, &format!("raw {}", s.bob_c1));
    block_on(t.settle());
    let panel = t.render();
    assert!(panel.contains("Raw"), "{panel}");
    assert!(panel.contains("mod actions name it"), "{panel}");
    assert!(panel.contains("Thanks for the forum!"), "{panel}");
    block_on(t.press("q"));

    // `:bookmark` and `:mute` are local state
    run_command(&mut t, "bookmark");
    assert!(block_on(carol.engine.bookmarked(&s.welcome)).unwrap());
    run_command(&mut t, &format!("mute {}", s.w.bob.did()));
    let mutes = block_on(carol.engine.mutes()).unwrap();
    assert!(mutes.iter().any(|m| m.did == s.w.bob.did()), "{mutes:?}");

    // `:set` takes a word as the text it is and a list as JSON; `:unset` resets. Each shows
    // the settings in a panel, which a key closes
    let set = |t: &mut Tui, line: &str| {
        run_command(t, line);
        block_on(t.settle());
        assert!(t.state.panel.is_some(), "{line}: {}", status(t));
        block_on(t.press("q"));
    };
    set(&mut t, "set sources direct");
    set(&mut t, r#"set mediaAutoloadHosts ["cdn.example.com"]"#);
    let st = block_on(carol.engine.settings()).unwrap();
    assert_eq!(
        serde_json::to_value(st.sources).unwrap(),
        "direct",
        "{}",
        status(&t)
    );
    assert_eq!(st.media_autoload_hosts, ["cdn.example.com"], "{}", status(&t));
    set(&mut t, "unset mediaAutoloadHosts");
    let st = block_on(carol.engine.settings()).unwrap();
    assert!(st.media_autoload_hosts.is_empty(), "{}", status(&t));
    set(&mut t, "unset sources");

    // `:delete` asks first: `n` keeps the comment, `y` deletes it
    let t2 = block_on(
        carol
            .engine
            .comment(&lessdugin_core::dto::CommentDraft::new(
                &s.welcome,
                None,
                "To be deleted.",
            )),
    )
    .unwrap();
    carol.flush();
    let mine = carol.done(&t2);
    run_command(&mut t, &format!("delete {mine}"));
    assert!(t.state.confirm.is_some());
    assert!(t.render().contains("Confirm"), "{}", t.render());
    block_on(t.press("n"));
    assert!(t.state.confirm.is_none());
    assert_eq!(
        block_on(t.engine().writes())
            .unwrap()
            .iter()
            .filter(|w| !matches!(w.state, lessdugin_core::dto::WriteState::Done { .. }))
            .count(),
        0
    );
    run_command(&mut t, &format!("delete {mine}"));
    block_on(t.press("y"));
    assert!(status(&t).contains("Delete: queued"), "{}", status(&t));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let view = block_on(carol.engine.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    assert!(view.comments.iter().all(|c| c.uri != mine), "{view:?}");

    // `:open` takes a forum as owner/slug, and a post by its link
    run_command(&mut t, "open alice.example.com/meta");
    assert!(
        t.state.top_forum_screen().is_some_and(|f| f.uri == s.forum),
        "{}\n{}",
        status(&t),
        t.render()
    );
    run_command(&mut t, &format!("open {}", s.bob_post));
    assert_eq!(
        t.state.top_thread().map(|th| th.uri.clone()),
        Some(s.bob_post.clone())
    );

    // alice renames her forum with `:forum edit`
    let mut ta = alice_tui(&s, 100, 30);
    block_on(ta.start(Some(s.forum.clone())));
    run_command(&mut ta, "forum edit --name Meta, renamed --description");
    assert!(
        status(&ta).contains("Forum edit: queued"),
        "{}",
        status(&ta)
    );
    block_on(ta.engine().outbox_flush()).unwrap();
    let f = block_on(alice.engine.forum_open(&s.forum)).unwrap();
    assert_eq!(f.forum.name, "Meta, renamed");
    assert_eq!(f.forum.description, None);
    assert!(!status(&ta).contains("relies on"), "{}", status(&ta));

    // an open forum, created or switched to, carries the L7 label (threat-model.md L7)
    let label = lessdugin_tui::view::OPEN_MODE_LABEL;
    run_command(&mut ta, "forum edit --mode open");
    assert!(status(&ta).contains(label), "{}", status(&ta));
    run_command(&mut ta, "forum create cafe The cafe --open");
    assert!(status(&ta).contains("New open forum"), "{}", status(&ta));
    assert!(status(&ta).contains(label), "{}", status(&ta));
    run_command(&mut ta, "forum create desk The desk");
    assert!(status(&ta).contains("New forum"), "{}", status(&ta));
    assert!(!status(&ta).contains(label), "{}", status(&ta));
}
