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
    // the TUI's own commands, not the CLI's
    assert!(screen.contains("`:open <owner>/<slug>`"), "{screen}");
    assert!(screen.contains("`:join`"), "{screen}");
    assert!(!screen.contains("lessdugin join"), "{screen}");
    insta::assert_snapshot!("home_empty_80x24", screen);
    // and so does `R`, which has nothing to sync
    block_on(t.press("R"));
    let st = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(st.contains("`:open <owner>/<slug>`"), "{st}");
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

/// The displayed time of a post or comment is its t_rank, never the createdAt its author claims
/// (`social.md` §2.3): a backdated item shows when it was first seen.
#[test]
fn the_displayed_time_is_t_rank_not_the_claimed_created_at() {
    use lessdugin_core::dto::{ListPostsQuery, ThreadOpts};
    use lessdugin_tui::app::ThreadData;
    use lessdugin_tui::text::{post_meta, time};
    let s = scenario();
    let list = block_on(
        s.w.carol
            .engine
            .list_posts(&ListPostsQuery::new(s.forum.clone())),
    )
    .unwrap();
    // three years before it was seen
    let backdated = 3 * 365 * 24 * 60 * MINUTE;
    let mut p = list.items[0].clone();
    p.created_at_ms = p.t_rank_ms - backdated;
    let meta: Vec<String> = post_meta(&p).iter().map(|t| t.to_plain_string()).collect();
    assert!(meta.contains(&time(p.t_rank_ms)), "{meta:?}");
    assert!(!meta.contains(&time(p.created_at_ms)), "{meta:?}");

    let mut view = block_on(
        s.w.carol
            .engine
            .get_thread(&s.welcome, &ThreadOpts::default()),
    )
    .unwrap();
    let c = &mut view.comments[0];
    c.created_at_ms = c.t_rank_ms - backdated;
    let (shown, claimed) = (time(c.t_rank_ms), time(c.created_at_ms));
    let data = ThreadData {
        view,
        bodies: Default::default(),
    };
    let doc: String = lessdugin_tui::thread::layout(&data, &Default::default(), 80)
        .lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(doc.contains(&shown), "{doc}");
    assert!(!doc.contains(&claimed), "{doc}");
}

/// A forum whose posts start in `personal` (the CLI's and the SPA's default) lists nothing on
/// the frontpage; `f` cycles the filter to all posts (and curated), `s` reaches the
/// recent-comments sort, and on a thread `s` sorts the comments (`social.md` §12.2-§12.4).
#[test]
fn filters_and_every_sort_are_reachable() {
    use lessdugin_core::dto::{CommentSort, ForumDraft, PostFilter, PostSort};
    let s = scenario();
    let (alice, carol) = (&s.w.alice, &s.w.carol);
    let t = block_on(
        alice
            .engine
            .forum_create(&ForumDraft::new("personal", "Personal")),
    )
    .unwrap();
    alice.flush();
    let forum = alice.done(&t);
    s.w.tick(MINUTE);
    let t = block_on(
        alice
            .engine
            .publish(&PostDraft::article(&forum, "A personal post", "Hi.")),
    )
    .unwrap();
    alice.flush();
    let post = alice.done(&t);
    s.w.tick(MINUTE);
    block_on(carol.engine.forum_follow(&forum, None)).unwrap();
    carol.sync(&forum);

    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(forum.clone())));
    let screen = t.render();
    assert!(screen.contains("No frontpage posts."), "{screen}");
    assert!(!screen.contains("A personal post"), "{screen}");
    block_on(t.press("f"));
    let screen = t.render();
    assert!(screen.contains("A personal post"), "{screen}");
    assert!(screen.contains("filter:  all"), "{screen}");
    let f = t.state.top_forum_screen().unwrap();
    assert_eq!(f.filter, PostFilter::All);
    assert_eq!(f.data.as_ref().unwrap().list.filter, PostFilter::All);
    block_on(t.press("f"));
    assert_eq!(
        t.state.top_forum_screen().unwrap().filter,
        PostFilter::Curated
    );
    // `:filter all` sets it directly
    block_on(t.press(":"));
    for c in "filter all".chars() {
        block_on(t.press(&c.to_string()));
    }
    block_on(t.press("Enter"));
    assert_eq!(t.state.top_forum_screen().unwrap().filter, PostFilter::All);
    // four `s` from magic: recent comments, and the list is in it
    block_on(t.keys(&["s", "s", "s", "s"]));
    let f = t.state.top_forum_screen().unwrap();
    assert_eq!(f.sort, PostSort::RecentComments);
    assert_eq!(f.data.as_ref().unwrap().list.sort, PostSort::RecentComments);
    assert!(t.render().contains("recent comments"), "{}", t.render());

    // a thread's comment sort
    block_on(t.open_thread(&post));
    block_on(t.press("s"));
    let th = t.state.top_thread().unwrap();
    assert_eq!(th.opts.sort, CommentSort::Top);
    assert_eq!(th.data.as_ref().unwrap().view.sort, CommentSort::Top);
    assert!(t.render().contains("comments: top"), "{}", t.render());
    block_on(t.press(":"));
    for c in "sort latest-reply".chars() {
        block_on(t.press(&c.to_string()));
    }
    block_on(t.press("Enter"));
    let th = t.state.top_thread().unwrap();
    assert_eq!(
        th.data.as_ref().unwrap().view.sort,
        CommentSort::LatestReply
    );
}

/// An open forum carries the L7 label in its header (`threat-model.md` L7, SOC-13); an
/// approval forum, complete without third-party servers, does not.
#[test]
fn an_open_forum_says_it_relies_on_public_relays() {
    use lessdugin_core::dto::{DefaultTier, ForumDraft, MembershipMode};
    use lessdugin_tui::view::OPEN_MODE_LABEL;
    let s = scenario();
    let (alice, carol) = (&s.w.alice, &s.w.carol);
    let mut draft = ForumDraft::new("open", "Open");
    draft.membership = MembershipMode::Open;
    draft.default_tier = DefaultTier::Frontpage;
    let t = block_on(alice.engine.forum_create(&draft)).unwrap();
    alice.flush();
    let forum = alice.done(&t);
    s.w.tick(MINUTE);
    block_on(carol.engine.forum_follow(&forum, None)).unwrap();
    carol.sync(&forum);

    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(forum.clone())));
    let screen = t.render();
    // the engine's `relayDependent` banner, shown once
    assert_eq!(screen.matches(OPEN_MODE_LABEL).count(), 1, "{screen}");
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    let screen = t.render();
    assert!(!screen.contains(OPEN_MODE_LABEL), "{screen}");
}

/// Collapsed comments render collapsed, with their reason (`social.md` §10.1), and `Space`
/// expands them; the viewer's own `Collapsed(NewUser)` comments show expanded with the label
/// kept, since the newcomer collapse is for other readers (§10.3, decision O-4).
#[test]
fn collapsed_comments_fold_except_the_viewers_own_newcomer_ones() {
    use lessdugin_core::dto::{CollapseReason, ItemState, ThreadOpts};
    use lessdugin_tui::app::ThreadData;
    use lessdugin_tui::body::Body;
    use std::collections::BTreeSet;
    let s = scenario();
    let mut view = block_on(
        s.w.carol
            .engine
            .get_thread(&s.welcome, &ThreadOpts::default()),
    )
    .unwrap();
    let bob = s.w.bob.did();
    assert_eq!(view.comments[0].author.did, bob);
    let doc =
        |view: &lessdugin_core::dto::ThreadView, toggled: &BTreeSet<String>, me: Option<&str>| {
            let c = &view.comments[0];
            let data = ThreadData {
                bodies: [(c.uri.clone(), Body::from_record(None, &c.body))].into(),
                view: view.clone(),
            };
            lessdugin_tui::thread::layout_for(&data, toggled, 80, me, &Default::default())
                .lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
    let none = BTreeSet::new();
    let body = "Thanks for the forum!";
    for reason in [CollapseReason::NewUser, CollapseReason::LowScore] {
        view.comments[0].state = ItemState::Collapsed { reason };
        // another reader (carol), and a reader without an account
        for me in [Some(s.w.carol.did()), None] {
            let shown = doc(&view, &none, me.as_deref());
            assert!(shown.contains("collapsed: "), "{shown}");
            assert!(shown.contains("[+]"), "{shown}");
            assert!(
                !shown.contains(body),
                "{reason:?} folds for others:\n{shown}"
            );
            // Space expands it
            let toggled = BTreeSet::from([s.bob_c1.clone()]);
            let shown = doc(&view, &toggled, me.as_deref());
            assert!(shown.contains(body), "{shown}");
            assert!(!shown.contains("[+]"), "{shown}");
        }
    }
    // bob sees his own newcomer comment expanded, label kept
    view.comments[0].state = ItemState::Collapsed {
        reason: CollapseReason::NewUser,
    };
    let shown = doc(&view, &none, Some(&bob));
    assert!(shown.contains(body), "{shown}");
    assert!(shown.contains("collapsed: "), "{shown}");
    assert!(!shown.contains("[+]"), "{shown}");
    // a low-score collapse is not the newcomer one: it folds for its author too
    view.comments[0].state = ItemState::Collapsed {
        reason: CollapseReason::LowScore,
    };
    let shown = doc(&view, &none, Some(&bob));
    assert!(!shown.contains(body), "{shown}");
}

/// The same through the State: the Thread screen lays the thread out for the session's account,
/// so bob's own newcomer comment shows expanded in bob's TUI and folded in carol's (decision
/// O-4).
#[test]
fn the_thread_screen_shows_the_viewers_own_newcomer_comment_expanded() {
    use lessdugin_core::dto::{CollapseReason, ItemState, ThreadOpts};
    use lessdugin_tui::app::{Origin, ThreadData};
    use lessdugin_tui::{Action, Headless, HeadlessOpts};
    let s = scenario();
    let body = "Thanks for the forum!";
    let bob_tui = block_on(Headless::new(
        s.w.bob.engine.clone(),
        HeadlessOpts {
            width: 80,
            height: 30,
            ..HeadlessOpts::default()
        },
    ))
    .unwrap();
    for (mut t, own) in [(bob_tui, true), (tui(&s, 80, 30), false)] {
        block_on(t.start(None));
        block_on(t.open_thread(&s.welcome));
        let screen = t.state.top().unwrap().id;
        let mut view = block_on(t.engine().get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
        let c = view
            .comments
            .iter_mut()
            .find(|c| c.uri == s.bob_c1)
            .expect("bob's comment");
        c.state = ItemState::Collapsed {
            reason: CollapseReason::NewUser,
        };
        let bodies = t
            .state
            .top_thread()
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .bodies
            .clone();
        let _ = t.state.update(Action::ThreadLoaded {
            screen,
            origin: Origin::Live,
            result: Ok(Box::new(ThreadData { view, bodies })),
        });
        let shown = t.thread_document().unwrap();
        assert!(shown.contains("collapsed: new user"), "{shown}");
        assert_eq!(shown.contains(body), own, "own: {own}\n{shown}");
    }
}

/// A post over 64 KiB spills its body into a `textBlob` (`data-model.md` §5.3). The engine
/// fetches it for the view (m4-5); the Thread screen styles that source as it styles an inline
/// body (headings, emphasis), rather than showing the view's plain text.
#[test]
fn a_spilled_body_is_styled_from_its_fetched_source() {
    use lessdugin_core::dto::{BodyDraft, ThreadOpts, ThreadRoot};
    use lessdugin_tui::body::BodyDoc;
    let s = scenario();
    let bob = &s.w.bob;
    let mut source = "# Chapter one\n\nIt was a **dark** night.\n\n".to_owned();
    while source.len() <= 65_536 {
        source.push_str("More words in the long body. ");
    }
    let mut d = PostDraft::article(&s.forum, "Long", "");
    d.body = BodyDraft::text(source);
    let w = block_on(bob.engine.publish(&d)).unwrap();
    bob.flush();
    let post = bob.done(&w);
    s.w.carol.sync(&s.forum);
    // the record carries the blob, not the text
    let pv = block_on(s.w.carol.engine.get_thread(&post, &ThreadOpts::default())).unwrap();
    let ThreadRoot::Post(p) = pv.post else {
        panic!("no post")
    };
    assert!(p.body.notices.is_empty(), "{:?}", p.body.notices);
    assert!(
        p.body.text.starts_with("Chapter one"),
        "{}",
        &p.body.text[..80]
    );

    let mut t = tui(&s, 80, 40);
    block_on(t.start(None));
    block_on(t.open_thread(&post));
    let body = &t.state.top_thread().unwrap().data.as_ref().unwrap().bodies[&post];
    assert!(matches!(body.doc, BodyDoc::Styled(_)), "{:?}", body.doc);
    assert!(body.notices.is_empty(), "{:?}", body.notices);
    // the terminal projection: a heading line of its own (the view's plain text runs it into
    // the paragraph), and the emphasis markers are styling, not text
    let screen = t.render();
    assert!(screen.contains("# Chapter one\n"), "{screen}");
    assert!(screen.contains("It was a dark night."), "{screen}");
    assert!(!screen.contains("**dark**"), "{screen}");
}

/// m4-5 (`engine-api.md` §3.8 "Spilled bodies"): a spilled body the thread showed pending (the
/// author's PDS stalled) is fetched again by `live_tail`, which announces it; the open thread
/// re-queries and shows it.
#[test]
fn a_pending_spilled_body_shows_when_it_arrives() {
    use lessdugin_core::dto::PostDraft;
    use lessdugin_core::limits::BODY_BLOB_RETRY_MS;
    use lessdugin_core::model::render::NOTICE_SPILLED;
    let s = scenario();
    let (bob, carol) = (&s.w.bob, &s.w.carol);
    let mut source = "The opening.\n\n".to_owned();
    while source.len() <= 65_536 {
        source.push_str("More words in the long body. ");
    }
    source.push_str("\n\nThe last words.");
    let t = block_on(
        bob.engine
            .publish(&PostDraft::article(&s.forum, "Long", source)),
    )
    .unwrap();
    bob.flush();
    let post = bob.done(&t);
    s.w.tick(MINUTE);
    carol.host.hold_blob_fetches(true);
    carol.sync(&s.forum);

    let mut t = tui(&s, 100, 30);
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.open_thread(&post));
    let doc = t.thread_document().unwrap();
    assert!(doc.contains(NOTICE_SPILLED), "{doc}");
    assert!(!doc.contains("The last words."), "{doc}");

    // the PDS answers again; the round after the retry time fetches the body
    carol.host.hold_blob_fetches(false);
    s.w.tick(BODY_BLOB_RETRY_MS);
    block_on(t.engine().live_step()).unwrap();
    block_on(t.settle());
    let doc = t.thread_document().unwrap();
    assert!(!doc.contains(NOTICE_SPILLED), "{doc}");
    assert!(doc.contains("The last words."), "{doc}");
}
