// SPDX-License-Identifier: AGPL-3.0-or-later
//! Writing through `$EDITOR`: `r` replies to the selected comment, `c` comments on the post and
//! `P` publishes a post. `$EDITOR` is a script that writes text above the template.

mod common;

use common::{block_on, editor, editor_script, scenario};
use lessdugin_core::dto::{ThreadOpts, ThreadRoot};
use lessdugin_tui::{Headless, HeadlessOpts};

fn tui_with_editor(
    s: &common::Scenario,
    dir: &std::path::Path,
    text: &str,
) -> Headless<lessdugin_testkit::MemHost> {
    let script = editor_script(dir, text);
    block_on(Headless::new(
        s.w.carol.engine.clone(),
        HeadlessOpts {
            editor: Some(editor(dir, &script)),
            ..HeadlessOpts::default()
        },
    ))
    .unwrap()
}

#[test]
fn reply_in_the_editor_creates_a_comment() {
    let s = scenario();
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(&s, dir.path(), "Agreed with **bob**.");
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    // select bob's comment and reply
    block_on(t.press("j"));
    block_on(t.press("r"));

    // the comment was written, sent and is shown under bob's
    let view = block_on(
        s.w.carol
            .engine
            .get_thread(&s.welcome, &ThreadOpts::default()),
    )
    .unwrap();
    let reply = view
        .comments
        .iter()
        .find(|c| c.author.did == s.w.carol.did())
        .expect("carol's reply");
    assert_eq!(reply.parent.as_deref(), Some(s.bob_c1.as_str()));
    assert_eq!(reply.depth, 2);
    assert!(
        reply.body.text.contains("Agreed with bob."),
        "{:?}",
        reply.body
    );
    assert!(!reply.badges.pending, "the outbox was flushed");
    let held = t.thread_view().unwrap();
    assert!(held.comments.iter().any(|c| c.uri == reply.uri));
    let screen = t.render();
    assert!(screen.contains("Agreed with bob."), "{screen}");
    assert!(screen.contains("Sent."), "{screen}");
    // the template file is gone
    let left: Vec<_> = std::fs::read_dir(dir.path().join("templates"))
        .unwrap()
        .collect();
    assert!(left.is_empty(), "{left:?}");

    // others see it after a sync
    s.w.bob.sync(&s.forum);
    let bob_view = block_on(
        s.w.bob
            .engine
            .get_thread(&s.welcome, &ThreadOpts::default()),
    )
    .unwrap();
    assert!(bob_view.comments.iter().any(|c| c.uri == reply.uri));
}

#[test]
fn comment_on_the_post_and_cancel() {
    let s = scenario();
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(&s, dir.path(), "A top-level comment.");
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    // c on bob's comment still comments on the post
    block_on(t.keys(&["j", "c"]));
    let view = t.thread_view().unwrap().clone();
    let c = view
        .comments
        .iter()
        .find(|c| c.author.did == s.w.carol.did())
        .expect("carol's comment");
    assert_eq!(c.parent, None);
    assert_eq!(c.depth, 1);

    // an editor that writes nothing: nothing is sent
    let dir2 = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(&s, dir2.path(), "");
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    let before = t.thread_view().unwrap().comments.len();
    block_on(t.press("r"));
    assert_eq!(t.thread_view().unwrap().comments.len(), before);
    let screen = t.render();
    assert!(screen.contains("Nothing to send"), "{screen}");
}

#[test]
fn new_post_in_the_editor() {
    let s = scenario();
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(
        &s,
        dir.path(),
        "# From the TUI\n\nA post written in $EDITOR.",
    );
    block_on(t.start(Some(s.forum.clone())));
    block_on(t.press("P"));
    let screen = t.render();
    assert!(screen.contains("From the TUI"), "{screen}");
    let list = block_on(
        s.w.carol
            .engine
            .list_posts(&lessdugin_core::dto::ListPostsQuery {
                filter: lessdugin_core::dto::PostFilter::All,
                sort: lessdugin_core::dto::PostSort::New,
                ..lessdugin_core::dto::ListPostsQuery::new(&s.forum)
            }),
    )
    .unwrap();
    let p = &list.items[0];
    assert_eq!(p.title.as_deref(), Some("From the TUI"));
    assert_eq!(p.author.did, s.w.carol.did());
    let post = block_on(s.w.carol.engine.get_thread(&p.uri, &ThreadOpts::default())).unwrap();
    let ThreadRoot::Post(v) = post.post else {
        panic!("missing")
    };
    assert_eq!(v.body.text.trim(), "A post written in $EDITOR.");
}

#[test]
fn writing_needs_an_account() {
    let s = scenario();
    let dir = tempfile::tempdir().unwrap();
    let script = editor_script(dir.path(), "never sent");
    block_on(s.w.carol.engine.remove_account(&s.w.carol.did(), false)).unwrap();
    let mut t = block_on(Headless::new(
        s.w.carol.engine.clone(),
        HeadlessOpts {
            editor: Some(editor(dir.path(), &script)),
            ..HeadlessOpts::default()
        },
    ))
    .unwrap();
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    block_on(t.press("r"));
    let screen = t.render();
    assert!(screen.contains("Log in to write"), "{screen}");
    assert!(screen.contains("not logged in (read-only)"), "{screen}");
}
