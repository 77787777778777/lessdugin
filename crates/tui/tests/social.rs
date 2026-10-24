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
