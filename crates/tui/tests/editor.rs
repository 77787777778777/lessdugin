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

/// `:edit` opens `$EDITOR` on your comment's current text and sends the edit; the old version
/// is kept as a revision. Someone else's comment is refused.
#[test]
fn the_edit_command_edits_your_comment() {
    use lessdugin_core::dto::CommentDraft;
    let s = scenario();
    let carol = &s.w.carol;
    let t = block_on(
        carol
            .engine
            .comment(&CommentDraft::new(&s.welcome, None, "First version.")),
    )
    .unwrap();
    carol.flush();
    let mine = carol.done(&t);
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(&s, dir.path(), "Second version.");
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    let run = |t: &mut Headless<lessdugin_testkit::MemHost>, line: &str| {
        block_on(t.press(":"));
        for c in line.chars() {
            block_on(t.press(&c.to_string()));
        }
        block_on(t.press("Enter"));
    };
    run(&mut t, &format!("edit {mine}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let view = block_on(carol.engine.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    let c = view.comments.iter().find(|c| c.uri == mine).unwrap();
    // the editor's text above the template's: the new line, then the old text it was given
    assert!(
        c.body.text.starts_with("Second version."),
        "{:?}",
        c.body.text
    );
    assert!(c.badges.edited, "{:?}", c.badges);
    let h = block_on(carol.engine.history(&mine)).unwrap();
    assert!(!h.versions.is_empty(), "{h:?}");

    // bob's comment is not carol's to edit
    run(&mut t, &format!("edit {}", s.bob_c1));
    let st = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(st.contains("only your own"), "{st}");
}

/// Types `line` into the `:` overlay and runs it.
fn run(t: &mut Headless<lessdugin_testkit::MemHost>, line: &str) {
    block_on(t.press(":"));
    for c in line.chars() {
        block_on(t.press(&c.to_string()));
    }
    block_on(t.press("Enter"));
}

/// carol's post `uri` as the engine reads it back: `(title, body text)`.
fn post_of(s: &common::Scenario, uri: &str) -> (Option<String>, String) {
    let view = block_on(s.w.carol.engine.get_thread(uri, &ThreadOpts::default())).unwrap();
    let ThreadRoot::Post(p) = view.post else {
        panic!("missing")
    };
    (p.post.title, p.body.text)
}

/// `:edit` on a quick take edits its body: it has no title line (`data-model.md` PO4), so the
/// first line of the text stays in the body, and a one-line take is not emptied.
#[test]
fn the_edit_command_edits_a_quick_takes_body() {
    use lessdugin_core::dto::{BodyDraft, PostDraft, PostKind};
    let s = scenario();
    let carol = &s.w.carol;
    let draft = PostDraft {
        kind: PostKind::QuickTake,
        title: None,
        body: BodyDraft::text("A one-line take."),
        ..PostDraft::article(&s.forum, "", "")
    };
    let t = block_on(carol.engine.publish(&draft)).unwrap();
    carol.flush();
    let take = carol.done(&t);

    // the editor writes a new first line above the take
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(&s, dir.path(), "Sharper:");
    block_on(t.start(None));
    run(&mut t, &format!("edit {take}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let (title, body) = post_of(&s, &take);
    assert_eq!(title, None);
    // (the plain text of the markdown: the two lines are one paragraph)
    assert_eq!(body.trim(), "Sharper: A one-line take.", "{body:?}");

    // an editor that leaves the text as it is keeps the whole take
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_editor(&s, dir.path(), "");
    block_on(t.start(None));
    run(&mut t, &format!("edit {take}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let (title, body) = post_of(&s, &take);
    assert_eq!(title, None);
    assert_eq!(body.trim(), "Sharper: A one-line take.", "{body:?}");

    // and the editor is given the take as it is, with no (empty) title line above it
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(&s, dir.path(), "s/Sharper:/Sharper:/", true);
    block_on(t.start(None));
    run(&mut t, &format!("edit {take}"));
    let seen = std::fs::read_to_string(dir.path().join("seen.txt")).unwrap();
    assert!(seen.starts_with("Sharper:\n"), "{seen:?}");
    assert!(!seen.contains("the first line is the title"), "{seen}");
    assert!(!seen.contains("The first line is the title"), "{seen}");
}

/// carol's TUI with an `$EDITOR` that runs `sed -e <expr>` over the file (and, with `record`,
/// keeps a copy of what it was given in `seen.txt`).
fn tui_with_sed(
    s: &common::Scenario,
    dir: &std::path::Path,
    expr: &str,
    record: bool,
) -> Headless<lessdugin_testkit::MemHost> {
    let script = dir.join("sed-editor.sh");
    let keep = if record {
        format!("cp \"$1\" '{}'\n", dir.join("seen.txt").display())
    } else {
        String::new()
    };
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nset -e\n{keep}sed -e '{expr}' \"$1\" > \"$1.new\"\nmv \"$1.new\" \"$1\"\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    block_on(Headless::new(
        s.w.carol.engine.clone(),
        HeadlessOpts {
            editor: Some(editor(dir, &script)),
            ..HeadlessOpts::default()
        },
    ))
    .unwrap()
}

/// `:edit` on a post whose body spilled into a blob (over 64 KiB, `data-model.md` §5.3) edits
/// its source as the engine fetched it, and the new version spills again.
#[test]
fn the_edit_command_edits_a_spilled_body() {
    use lessdugin_core::dto::PostDraft;
    let s = scenario();
    let carol = &s.w.carol;
    let mut source = "The old opening.\n\n".to_owned();
    while source.len() <= 65_536 {
        source.push_str("More words in the long body. ");
    }
    let t = block_on(carol.engine.publish(&PostDraft::article(&s.forum, "Long", source))).unwrap();
    carol.flush();
    let post = carol.done(&t);
    let record = |uri: &str| {
        let u = lessdugin_atp::syntax::AtUri::parse(uri).unwrap();
        block_on(
            lessdugin_core::store::records::RecordStore::new(carol.engine.host().cache()).get(&u),
        )
        .unwrap()
        .unwrap()
        .value_json()
        .unwrap()
    };
    assert!(record(&post)["body"].get("textBlob").is_some(), "it spilled");

    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(&s, dir.path(), "s/The old opening./The new opening./", false);
    block_on(t.start(None));
    run(&mut t, &format!("edit {post}"));
    let st = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(!st.contains("blob"), "{st}");
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let (title, body) = post_of(&s, &post);
    assert_eq!(title.as_deref(), Some("Long"));
    assert!(body.starts_with("The new opening. More words"), "{}", &body[..80]);
    assert!(record(&post)["body"].get("textBlob").is_some(), "it spilled again");
}

/// `:edit` on an article keeps a title the user did not change as it was, a leading `#`
/// included, and takes the edited body.
#[test]
fn the_edit_command_keeps_an_unchanged_title() {
    use lessdugin_core::dto::PostDraft;
    let s = scenario();
    let carol = &s.w.carol;
    let t = block_on(carol.engine.publish(&PostDraft::article(
        &s.forum,
        "#1 rule: be kind",
        "The old body.",
    )))
    .unwrap();
    carol.flush();
    let post = carol.done(&t);

    // an editor that changes the body only
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("sed-editor.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nset -e\nsed 's/The old body./The new body./' \"$1\" > \"$1.new\"\nmv \"$1.new\" \"$1\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut t = block_on(Headless::new(
        carol.engine.clone(),
        HeadlessOpts {
            editor: Some(editor(dir.path(), &script)),
            ..HeadlessOpts::default()
        },
    ))
    .unwrap();
    block_on(t.start(None));
    run(&mut t, &format!("edit {post}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let (title, body) = post_of(&s, &post);
    assert_eq!(title.as_deref(), Some("#1 rule: be kind"));
    assert_eq!(body.trim(), "The new body.", "{body:?}");
}
