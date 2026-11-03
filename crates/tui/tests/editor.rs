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
    let t = block_on(
        carol
            .engine
            .publish(&PostDraft::article(&s.forum, "Long", source)),
    )
    .unwrap();
    carol.flush();
    let post = carol.done(&t);
    let record = |uri: &str| {
        use lessdugin_core::host::Host as _;
        let u = lessdugin_atp::syntax::AtUri::parse(uri).unwrap();
        block_on(
            lessdugin_core::store::records::RecordStore::new(carol.engine.host().cache()).get(&u),
        )
        .unwrap()
        .unwrap()
        .value_json()
        .unwrap()
    };
    assert!(
        record(&post)["body"].get("textBlob").is_some(),
        "it spilled"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(
        &s,
        dir.path(),
        "s/The old opening./The new opening./",
        false,
    );
    block_on(t.start(None));
    run(&mut t, &format!("edit {post}"));
    let st = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(!st.contains("blob"), "{st}");
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let (title, body) = post_of(&s, &post);
    assert_eq!(title.as_deref(), Some("Long"));
    assert!(
        body.starts_with("The new opening. More words"),
        "{}",
        &body[..80]
    );
    assert!(
        record(&post)["body"].get("textBlob").is_some(),
        "it spilled again"
    );
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

/// sec-7 (`social.md` §4.5, `threat-model.md` SOC-3): `:edit` on an item a moderator pinned
/// says the edit suspends the pin and asks first; `n` opens no editor and sends nothing, `y`
/// edits.
#[test]
fn the_edit_command_asks_before_suspending_a_pin() {
    use lessdugin_core::dto::{ModActionIntent, ModActionValue, ModScope, PinScope, PostDraft};
    let s = scenario();
    let (alice, carol) = (&s.w.alice, &s.w.carol);
    let t = block_on(carol.engine.publish(&PostDraft::article(
        &s.forum,
        "Pinned",
        "The old body.",
    )))
    .unwrap();
    carol.flush();
    let post = carol.done(&t);
    alice.sync(&s.forum);
    block_on(alice.engine.mod_action(&ModActionIntent::Assert {
        forum: s.forum.clone(),
        scope: ModScope::Forum,
        subject: post.clone(),
        action: ModActionValue::Pin {
            scope: PinScope::Forum,
            pinned: true,
        },
        reason: None,
        until_ms: None,
    }))
    .unwrap();
    alice.flush();
    s.w.tick(common::MINUTE);
    carol.sync(&s.forum);

    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(&s, dir.path(), "s/The old body./The new body./", true);
    block_on(t.start(None));
    run(&mut t, &format!("edit {post}"));
    let c = t.state.confirm.as_ref().expect("a confirmation");
    assert!(
        c.text.starts_with("Editing suspends its pin:"),
        "{}",
        c.text
    );
    assert!(t.render().contains("Editing suspends its pin"));
    block_on(t.press("n"));
    assert!(t.state.confirm.is_none());
    assert!(!dir.path().join("seen.txt").exists(), "no editor ran");
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    assert_eq!(post_of(&s, &post).1.trim(), "The old body.");

    run(&mut t, &format!("edit {post}"));
    assert!(t.state.confirm.is_some());
    block_on(t.press("y"));
    assert!(dir.path().join("seen.txt").exists(), "the editor ran");
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    assert_eq!(post_of(&s, &post).1.trim(), "The new body.");
}

/// A user's cached record `uri` as JSON (what the engine wrote back once the write was done).
fn record(u: &common::User, uri: &str) -> serde_json::Value {
    use lessdugin_core::host::Host as _;
    let u_ = lessdugin_atp::syntax::AtUri::parse(uri).unwrap();
    block_on(lessdugin_core::store::records::RecordStore::new(u.engine.host().cache()).get(&u_))
        .unwrap()
        .unwrap()
        .value_json()
        .unwrap()
}

/// `prepare_media` of a tiny PNG with alt text `alt`.
fn picture(u: &common::User, seed: u8, alt: &str) -> lessdugin_core::dto::MediaRef {
    block_on(u.engine.prepare_media(
        lessdugin_testkit::fake_pds::tiny_png(seed),
        &lessdugin_core::dto::MediaInput {
            declared_mime: Some("image/png".into()),
            alt: Some(alt.into()),
            name: None,
            aspect_ratio: None,
        },
    ))
    .unwrap()
}

/// robustness-1 (`data-model.md` §8.3, §8.8): `:edit` keeps the body's media. The edit sends
/// the cached items again as references, so the engine keeps their blob objects and every
/// `cid:` image of the text still resolves; before, the new version had no `media` at all.
#[test]
fn the_edit_command_keeps_the_bodys_media() {
    use lessdugin_core::dto::{CommentDraft, PostDraft};
    let s = scenario();
    let carol = &s.w.carol;
    let pic = picture(carol, 1, "a picture");
    let cid = pic.cid.clone();
    let mut d = PostDraft::article(
        &s.forum,
        "With a picture",
        format!("Look:\n\n![a picture](cid:{cid})"),
    );
    d.body.media.push(pic);
    let t = block_on(carol.engine.publish(&d)).unwrap();
    carol.flush();
    let post = carol.done(&t);
    // and a comment with another one
    let pic2 = picture(carol, 2, "another");
    let cid2 = pic2.cid.clone();
    let mut c = CommentDraft::new(
        &s.welcome,
        None,
        format!("Mine too: ![another](cid:{cid2})"),
    );
    c.body.media.push(pic2);
    let t = block_on(carol.engine.comment(&c)).unwrap();
    carol.flush();
    let comment = carol.done(&t);
    let media_of = |v: &serde_json::Value| -> Vec<(String, String)> {
        v["body"]["media"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| {
                        (
                            m["blob"]["ref"]["$link"].as_str().unwrap_or("").to_owned(),
                            m["alt"].as_str().unwrap_or("").to_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        media_of(&record(carol, &post)),
        [(cid.clone(), "a picture".to_owned())]
    );

    // a typo fix in the post's body
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(&s, dir.path(), "s/Look:/Look here:/", false);
    block_on(t.start(None));
    run(&mut t, &format!("edit {post}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let v = record(carol, &post);
    assert!(
        v["body"]["text"].as_str().unwrap().contains("Look here:"),
        "{v}"
    );
    assert_eq!(media_of(&v), [(cid.clone(), "a picture".to_owned())], "{v}");
    let view = block_on(carol.engine.get_thread(&post, &ThreadOpts::default())).unwrap();
    let ThreadRoot::Post(p) = &view.post else {
        panic!("no post")
    };
    assert_eq!(p.body.media.len(), 1, "{:?}", p.body.media);
    assert_eq!(p.body.media[0].cid, cid);

    // and in the comment
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(&s, dir.path(), "s/Mine too:/Mine as well:/", false);
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    run(&mut t, &format!("edit {comment}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let v = record(carol, &comment);
    assert!(
        v["body"]["text"]
            .as_str()
            .unwrap()
            .contains("Mine as well:"),
        "{v}"
    );
    assert_eq!(media_of(&v), [(cid2, "another".to_owned())], "{v}");
}

/// robustness-7 (`data-model.md` §8.3): `:edit` of a link post keeps the card's title and
/// description, which the TUI cannot change, with the uri; before, the new version's card was
/// the bare uri.
#[test]
fn the_edit_command_keeps_a_link_posts_card() {
    use lessdugin_core::dto::{ExternalLinkDraft, PostDraft, PostKind};
    let s = scenario();
    let carol = &s.w.carol;
    let mut d = PostDraft::article(&s.forum, "A link", "Why it matters.");
    d.kind = PostKind::Link;
    d.link = Some(ExternalLinkDraft {
        uri: "https://example.com/article".into(),
        title: Some("The article".into()),
        description: Some("What it says.".into()),
    });
    let t = block_on(carol.engine.publish(&d)).unwrap();
    carol.flush();
    let post = carol.done(&t);
    let card = |v: &serde_json::Value| {
        (
            v["link"]["uri"].as_str().map(str::to_owned),
            v["link"]["title"].as_str().map(str::to_owned),
            v["link"]["description"].as_str().map(str::to_owned),
        )
    };
    let before = card(&record(carol, &post));
    assert_eq!(before.1.as_deref(), Some("The article"));

    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(
        &s,
        dir.path(),
        "s/Why it matters./Why it still matters./",
        false,
    );
    block_on(t.start(None));
    run(&mut t, &format!("edit {post}"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let v = record(carol, &post);
    assert_eq!(post_of(&s, &post).1.trim(), "Why it still matters.");
    assert_eq!(card(&v), before, "{v}");
}

/// `:edit --drop-link-card` (`EditOpts.dropLinkCard`, `engine-api.md` §3.5 `edit`, decision
/// O-14): the edit drops a link post's card (title, description, image) and keeps the link,
/// which `:edit` alone keeps; the editor says so. A comment has no card to drop.
#[test]
fn the_edit_command_can_drop_a_link_posts_card() {
    use lessdugin_core::dto::{ExternalLinkDraft, PostDraft, PostKind};
    let s = scenario();
    let carol = &s.w.carol;
    let mut d = PostDraft::article(&s.forum, "A link", "Why it matters.");
    d.kind = PostKind::Link;
    d.link = Some(ExternalLinkDraft {
        uri: "https://example.com/article".into(),
        title: Some("The article".into()),
        description: Some("A misleading description.".into()),
    });
    let t = block_on(carol.engine.publish(&d)).unwrap();
    carol.flush();
    let post = carol.done(&t);

    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(
        &s,
        dir.path(),
        "s/Why it matters./Why it still matters./",
        true,
    );
    block_on(t.start(None));
    run(&mut t, &format!("edit {post} --drop-link-card"));
    block_on(t.engine().outbox_flush()).unwrap();
    block_on(t.settle());
    let seen = std::fs::read_to_string(dir.path().join("seen.txt")).unwrap();
    assert!(
        seen.contains("link card (title, description, image) is dropped"),
        "{seen}"
    );
    let v = record(carol, &post);
    assert_eq!(
        v["link"],
        serde_json::json!({ "uri": "https://example.com/article" }),
        "{v}"
    );
    assert_eq!(post_of(&s, &post).1.trim(), "Why it still matters.");

    // a comment has no card: refused before the editor opens
    let c = block_on(
        carol
            .engine
            .comment(&lessdugin_core::dto::CommentDraft::new(
                &post,
                None,
                "A comment.",
            )),
    )
    .unwrap();
    carol.flush();
    let comment = carol.done(&c);
    std::fs::remove_file(dir.path().join("seen.txt")).unwrap();
    run(&mut t, &format!("edit {comment} --drop-link-card"));
    assert!(!dir.path().join("seen.txt").exists(), "no editor ran");
    let status = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(status.contains("no link card"), "{status}");
}

/// sec-7, the other half: when `edit_preflight` cannot run, `:edit` is not refused. The TUI
/// says what it could not check, and why, and asks the same question; `n` opens no editor, `y`
/// edits unchecked.
#[test]
fn an_edit_whose_preflight_failed_asks_before_editing_unchecked() {
    use lessdugin_core::dto::{CommentDraft, Draft};
    use lessdugin_tui::app::{Action, Compose, EditStart, EditorRequest, Effect};
    let s = scenario();
    let dir = tempfile::tempdir().unwrap();
    let mut t = tui_with_sed(&s, dir.path(), "s/x/y/", true);
    block_on(t.start(None));
    let request = EditorRequest {
        compose: Compose::Edit {
            uri: s.bob_c1.clone(),
            base: Box::new(Draft::Comment {
                id: None,
                comment: CommentDraft::new(&s.welcome, None, "x"),
            }),
            drop_link_card: false,
        },
        template: "x\n".into(),
    };
    let unchecked = || {
        Action::EditReady(Ok(EditStart {
            request: request.clone(),
            notices: Vec::new(),
            unchecked: Some("the forum's view is not loaded".into()),
        }))
    };
    let fx = t.state.update(unchecked());
    assert!(fx.is_empty(), "no editor before the user answers: {fx:?}");
    let c = t.state.confirm.as_ref().expect("asked first");
    assert!(
        c.text
            .contains("could not be checked (the forum's view is not loaded)"),
        "{}",
        c.text
    );
    assert!(c.text.ends_with("Edit it anyway?"), "{}", c.text);
    assert!(
        t.render().contains("could not be checked"),
        "{}",
        t.render()
    );
    block_on(t.press("n"));
    assert!(t.state.confirm.is_none());
    assert!(!dir.path().join("seen.txt").exists(), "no editor ran");

    let fx = t.state.update(unchecked());
    assert!(fx.is_empty());
    let y = lessdugin_tui::keys::parse_key("y").unwrap();
    let fx = t.state.update(Action::Key(y));
    assert_eq!(fx, [Effect::Editor(request)]);
    assert!(t.state.confirm.is_none());
}
