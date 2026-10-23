// SPDX-License-Identifier: AGPL-3.0-or-later
//! The screens over a `TestBackend` at 80×24 and 160×48, driven by keys against MemHost +
//! FakePds (`docs/plan.md` §4.1, CLI and TUI layer): Home, Forum, Thread, the help overlay,
//! navigation, folds and the live-update banner.

mod common;

use common::{MINUTE, block_on, scenario, tui};
use lessdugin_core::dto::{PostDraft, SyncOpts};
use lessdugin_tui::app::ScreenKind;

const SIZES: [(u16, u16); 2] = [(80, 24), (160, 48)];

#[test]
fn home_lists_followed_forums_with_their_sync_state() {
    let s = scenario();
    for (w, h) in SIZES {
        let mut t = tui(&s, w, h);
        block_on(t.start(None));
        let screen = t.render();
        assert!(screen.contains("Meta"), "{screen}");
        assert!(
            screen.contains("not logged in") || screen.contains("@carol"),
            "{screen}"
        );
        insta::assert_snapshot!(format!("home_{w}x{h}"), screen);
    }
}

#[test]
fn home_without_forums_says_how_to_join_one() {
    let s = scenario();
    block_on(s.w.carol.engine.forum_unfollow(&s.forum)).unwrap();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(None));
    let screen = t.render();
    assert!(screen.contains("You follow no forum yet."), "{screen}");
    insta::assert_snapshot!("home_empty_80x24", screen);
}

#[test]
fn forum_lists_posts_and_sorts() {
    let s = scenario();
    for (w, h) in SIZES {
        let mut t = tui(&s, w, h);
        block_on(t.start(Some(s.forum.clone())));
        let screen = t.render();
        let bob = screen.find("Hello from bob").expect("bob's post");
        let welcome = screen.find("Welcome").expect("the welcome post");
        assert!(
            bob < welcome,
            "newest first (magic, equal scores):\n{screen}"
        );
        insta::assert_snapshot!(format!("forum_{w}x{h}"), screen);
        // s cycles magic → top → new → old; three presses land on old
        block_on(t.keys(&["s", "s", "s"]));
        let screen = t.render();
        let bob = screen.rfind("Hello from bob").unwrap();
        let welcome = screen.rfind("Welcome").unwrap();
        assert!(welcome < bob, "oldest first:\n{screen}");
        if w == 80 {
            insta::assert_snapshot!("forum_sort_old_80x24", screen);
        }
    }
}

#[test]
fn thread_shows_the_post_and_its_comments() {
    let s = scenario();
    for (w, h) in SIZES {
        let mut t = tui(&s, w, h);
        block_on(t.start(Some(s.forum.clone())));
        // the welcome post is second (newest first)
        block_on(t.keys(&["j", "Enter"]));
        assert!(t.on_thread());
        let view = t.thread_view().expect("thread view").clone();
        assert_eq!(view.comments.len(), 1);
        assert_eq!(view.comments[0].uri, s.bob_c1);
        let screen = t.render();
        assert!(screen.contains("Welcome"), "{screen}");
        insta::assert_snapshot!(format!("thread_{w}x{h}"), screen);
        // the help overlay
        block_on(t.press("?"));
        let help = t.render();
        assert!(help.contains("Keys"), "{help}");
        insta::assert_snapshot!(format!("help_{w}x{h}"), help);
        block_on(t.press("x"));
        assert!(!t.state.help);
        // back to the forum, then home, then quit
        block_on(t.press("h"));
        assert!(matches!(
            t.state.top().map(|s| &s.kind),
            Some(ScreenKind::Forum(_))
        ));
        block_on(t.press("q"));
        block_on(t.press("q"));
        assert!(t.state.quit);
    }
}

#[test]
fn thread_navigation_folds_and_parents() {
    let s = scenario();
    let (bob, carol) = (&s.w.bob, &s.w.carol);
    // a reply to bob's comment, and a reply to that, by bob
    let t1 = block_on(
        carol
            .engine
            .comment(&lessdugin_core::dto::CommentDraft::new(
                &s.welcome,
                Some(s.bob_c1.clone()),
                "Agreed.",
            )),
    )
    .unwrap();
    carol.flush();
    let c2 = carol.done(&t1);
    s.w.tick(MINUTE);
    bob.sync(&s.forum);
    let t2 = block_on(bob.engine.comment(&lessdugin_core::dto::CommentDraft::new(
        &s.welcome,
        Some(c2.clone()),
        "Thanks, carol.",
    )))
    .unwrap();
    bob.flush();
    let c3 = bob.done(&t2);
    s.w.tick(MINUTE);
    carol.sync(&s.forum);

    let mut t = tui(&s, 80, 24);
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    let uris: Vec<String> = t
        .thread_view()
        .unwrap()
        .comments
        .iter()
        .map(|c| c.uri.clone())
        .collect();
    assert_eq!(uris, [s.bob_c1.clone(), c2.clone(), c3.clone()]);
    let selected = |t: &lessdugin_tui::Headless<_>| {
        let th = t.state.top_thread().unwrap();
        th.layout.items[th.pos.selected].uri.clone()
    };
    // G: the last comment; p: its parent, twice; then the post
    block_on(t.press("G"));
    assert_eq!(selected(&t), c3);
    block_on(t.press("p"));
    assert_eq!(selected(&t), c2);
    block_on(t.press("p"));
    assert_eq!(selected(&t), s.bob_c1);
    block_on(t.press("p"));
    assert_eq!(selected(&t), s.welcome);
    // Space on bob's comment folds its two replies
    block_on(t.keys(&["g", "j"]));
    while selected(&t) != s.bob_c1 {
        block_on(t.press("j"));
    }
    block_on(t.press("Space"));
    let screen = t.render();
    assert!(screen.contains("[+2]"), "{screen}");
    assert!(!screen.contains("Thanks, carol."), "{screen}");
    insta::assert_snapshot!("thread_folded_80x24", screen);
    // j stays on the fold's last item; Space unfolds
    block_on(t.press("j"));
    assert_eq!(selected(&t), s.bob_c1);
    block_on(t.press("Space"));
    let doc = t.thread_document().unwrap();
    assert!(doc.contains("Thanks, carol."), "{doc}");
    assert!(!doc.contains("[+2]"), "{doc}");
    // Ctrl-d / Ctrl-u / g
    block_on(t.press("Ctrl-d"));
    block_on(t.press("Ctrl-u"));
    block_on(t.press("g"));
    assert_eq!(selected(&t), s.welcome);
    assert_eq!(t.state.top_thread().unwrap().pos.scroll, 0);
}

#[test]
fn a_live_update_shows_a_banner_and_never_moves_the_cursor() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    // select the welcome post (the second)
    block_on(t.press("j"));
    let cursor = |t: &lessdugin_tui::Headless<_>| match &t.state.top().unwrap().kind {
        ScreenKind::Forum(f) => f.selected().unwrap().uri.clone(),
        _ => panic!("not on the forum"),
    };
    assert_eq!(cursor(&t), s.welcome);

    // bob posts; carol's engine syncs in the background (as live_tail would)
    s.w.tick(MINUTE);
    let p = block_on(
        s.w.bob
            .engine
            .publish(&PostDraft::article(&s.forum, "Another one", "More.")),
    )
    .unwrap();
    s.w.bob.flush();
    let another = s.w.bob.done(&p);
    s.w.tick(MINUTE);
    block_on(t.engine().sync_forum(&s.forum, &SyncOpts::default())).unwrap();
    block_on(t.settle());

    let screen = t.render();
    assert!(screen.contains("1 new \u{2014} R"), "{screen}");
    assert!(!screen.contains("Another one"), "{screen}");
    assert_eq!(cursor(&t), s.welcome, "the cursor stays");
    insta::assert_snapshot!("forum_banner_80x24", screen);

    // R shows it; the cursor stays on the same post
    block_on(t.press("R"));
    let screen = t.render();
    assert!(screen.contains("Another one"), "{screen}");
    assert!(!screen.contains("new \u{2014} R"), "{screen}");
    assert_eq!(cursor(&t), s.welcome);
    let ScreenKind::Forum(f) = &t.state.top().unwrap().kind else {
        panic!()
    };
    assert_eq!(f.data.as_ref().unwrap().list.items[0].uri, another);
}

#[test]
fn a_new_comment_shows_a_banner_on_the_thread() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    block_on(t.press("j"));
    let selected = |t: &lessdugin_tui::Headless<_>| {
        let th = t.state.top_thread().unwrap();
        (th.layout.items[th.pos.selected].uri.clone(), th.pos.scroll)
    };
    let before = selected(&t);
    assert_eq!(before.0, s.bob_c1);

    // bob comments again; carol's engine syncs in the background
    s.w.tick(MINUTE);
    block_on(
        s.w.bob
            .engine
            .comment(&lessdugin_core::dto::CommentDraft::new(
                &s.welcome,
                None,
                "One more thing.",
            )),
    )
    .unwrap();
    s.w.bob.flush();
    s.w.tick(MINUTE);
    block_on(t.engine().sync_forum(&s.forum, &SyncOpts::default())).unwrap();
    block_on(t.settle());
    let screen = t.render();
    assert!(screen.contains("1 new \u{2014} R"), "{screen}");
    assert!(!t.thread_document().unwrap().contains("One more thing."));
    assert_eq!(selected(&t), before, "nothing moved");

    // R shows it, on the same comment
    block_on(t.press("R"));
    assert!(t.thread_document().unwrap().contains("One more thing."));
    assert_eq!(selected(&t).0, s.bob_c1);
    assert!(!t.render().contains("new \u{2014} R"));
}
