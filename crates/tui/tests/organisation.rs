// SPDX-License-Identifier: AGPL-3.0-or-later
//! M5 local search, cached user pages, tags/wiki and sequences through the real TUI effects.

mod common;

use common::{MINUTE, block_on, scenario};
use lessdugin_core::dto::{
    BodyDraft, SequenceChapterDraft, SequenceDraft, WikiDraft, WriteState, WriteTicket,
};
use lessdugin_lexicon::collection::nsid;
use lessdugin_testkit::MemHost;
use lessdugin_tui::{Headless, HeadlessOpts};
use serde_json::json;

type Tui = Headless<MemHost>;

/// Types a command into the real command overlay and settles every resulting engine effect.
fn run_command(t: &mut Tui, line: &str) {
    block_on(t.press(":"));
    for c in line.chars() {
        block_on(t.press(&c.to_string()));
    }
    assert_eq!(t.state.command.as_deref(), Some(line));
    block_on(t.press("Enter"));
    assert!(t.state.command.is_none());
}

fn alice_tui(s: &common::Scenario, editor: Option<lessdugin_tui::EditorConfig>) -> Tui {
    block_on(Headless::new(
        s.w.alice.engine.clone(),
        HeadlessOpts {
            width: 100,
            height: 32,
            editor,
            ..HeadlessOpts::default()
        },
    ))
    .unwrap()
}

fn done_in(writes: &[WriteTicket], collection: &str) -> String {
    writes
        .iter()
        .find_map(|ticket| match &ticket.state {
            WriteState::Done { uri, .. } if uri.contains(&format!("/{collection}/")) => {
                Some(uri.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no done {collection} write: {writes:?}"))
}

/// Decision O-7: in the search results `n` and `N` step to the next and previous hit (round
/// the ends), `/` opens the command line on `search `, and `N` stays Notifications elsewhere,
/// a result panel that is not a search included (it closes, as any key does).
#[test]
fn search_results_step_with_n_and_shift_n() {
    let s = scenario();
    let mut t = common::tui(&s, 100, 32);
    block_on(t.start(Some(s.forum.clone())));

    block_on(t.press("/"));
    assert_eq!(t.state.command.as_deref(), Some("search "));
    for c in "Hello --limit 10".chars() {
        block_on(t.press(&c.to_string()));
    }
    block_on(t.press("Enter"));
    let hits = t.state.panel.as_ref().expect("search panel").items.len();
    assert!(hits >= 2, "{}", t.render());
    insta::assert_snapshot!("search_results_100x32", t.render());
    block_on(t.press("n"));
    assert_eq!(t.state.panel.as_ref().unwrap().selected, 1);
    insta::assert_snapshot!("search_results_next_100x32", t.render());
    block_on(t.press("N"));
    assert_eq!(t.state.panel.as_ref().unwrap().selected, 0);
    // round the ends: N on the first hit is the last, n on the last the first
    block_on(t.press("N"));
    assert_eq!(t.state.panel.as_ref().unwrap().selected, hits - 1);
    block_on(t.press("n"));
    assert_eq!(t.state.panel.as_ref().unwrap().selected, 0);
    // `/` from the results searches again
    block_on(t.press("/"));
    assert!(t.state.panel.is_none());
    assert_eq!(t.state.command.as_deref(), Some("search "));
    block_on(t.press("Esc"));
    assert!(t.state.command.is_none());

    // another result panel: N closes it and moves nothing
    run_command(&mut t, "user bob.example.com");
    assert!(!t.state.panel.as_ref().unwrap().search);
    block_on(t.press("N"));
    assert!(t.state.panel.is_none(), "{}", t.render());
    assert!(t.state.top_forum_screen().is_some(), "{}", t.render());
    // and on a screen, N is Notifications
    block_on(t.press("N"));
    assert!(
        matches!(
            t.state.top().map(|s| &s.kind),
            Some(lessdugin_tui::app::ScreenKind::Notifications(_))
        ),
        "{}",
        t.render()
    );
}

#[test]
fn search_and_organisation_reads_render_engine_results_and_open_stable_refs() {
    let s = scenario();
    let wiki = block_on(s.w.alice.engine.wiki_edit(
        &s.forum,
        "rust",
        WikiDraft {
            name: "Rust".into(),
            description: Some("Systems programming".into()),
            body: Some(BodyDraft::text("About **Rust** and fearless concurrency.")),
            is_tag: true,
            parent_tags: vec![],
            summary: Some("Initial page".into()),
            base: None,
        },
    ))
    .unwrap();
    s.w.alice.flush();
    let _wiki_uri = s.w.alice.done(&wiki);
    let sequence = SequenceDraft {
        forum: s.forum.clone(),
        title: "Reading list".into(),
        body: Some(BodyDraft::text("Start here.")),
        chapters: vec![SequenceChapterDraft {
            title: Some("Chapter One".into()),
            body: Some(BodyDraft::text("The first chapter.")),
            posts: vec![s.welcome.clone()],
        }],
    };
    let created = block_on(s.w.alice.engine.seq_create(&sequence)).unwrap();
    s.w.alice.flush();
    let sequence_uri = s.w.alice.done(&created);
    s.w.tick(MINUTE);

    let mut t = common::tui(&s, 100, 32);
    block_on(t.start(Some(s.forum.clone())));
    let forum_cursor = t.state.top_forum_screen().unwrap().pos.cursor;

    // Search is an engine query over synced content, not a separate TUI index. Its panel keeps
    // the snippets and canonical refs visible; j/k changes only the panel's selection.
    run_command(&mut t, "search Hello --limit 10");
    let panel = t.state.panel.as_ref().expect("search panel");
    assert!(panel.items.len() >= 2, "{panel:?}");
    assert!(
        panel
            .lines
            .iter()
            .any(|line| line.to_plain_string().contains("Hello"))
    );
    let search_screen = t.render();
    assert!(search_screen.contains("Local search"), "{search_screen}");
    assert!(search_screen.contains("result 1/"), "{search_screen}");
    assert!(search_screen.contains("snippet") || search_screen.contains("Hello"));
    block_on(t.press("j"));
    let panel = t.state.panel.as_ref().unwrap();
    assert_eq!(panel.selected, 1);
    let target = panel.items[panel.selected].target.clone();
    block_on(t.press("Enter"));
    assert!(t.on_thread(), "{}", t.render());
    assert_eq!(t.state.top_thread().unwrap().uri, target);
    block_on(t.press("h"));
    assert_eq!(t.state.top_forum_screen().unwrap().pos.cursor, forum_cursor);

    run_command(&mut t, "user bob.example.com");
    let user_screen = t.render();
    assert!(user_screen.contains("User"), "{user_screen}");
    assert!(user_screen.contains("bob.example.com"), "{user_screen}");
    assert!(user_screen.contains("Recent posts"), "{user_screen}");
    assert!(user_screen.contains("Hello from bob"), "{user_screen}");
    assert!(t.state.panel.as_ref().unwrap().items.len() >= 2);
    block_on(t.press("Esc"));

    run_command(&mut t, "tags");
    let tags_screen = t.render();
    assert!(tags_screen.contains("#rust"), "{tags_screen}");
    assert!(tags_screen.contains("Systems programming"), "{tags_screen}");
    block_on(t.press("Esc"));
    run_command(&mut t, "tags rust");
    let tag_screen = t.render();
    assert!(
        tag_screen.contains("About Rust and fearless concurrency."),
        "{tag_screen}"
    );
    assert!(tag_screen.contains("Revision:"), "{tag_screen}");
    block_on(t.press("Esc"));

    run_command(&mut t, "sequences");
    let library_screen = t.render();
    assert!(library_screen.contains("Reading list"), "{library_screen}");
    assert!(
        t.state
            .panel
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .any(|line| line.to_plain_string() == sequence_uri),
        "{library_screen}"
    );
    assert_eq!(
        t.state.panel.as_ref().unwrap().items[0].target,
        sequence_uri
    );
    block_on(t.press("Enter"));
    let sequence_screen = t.render();
    assert!(sequence_screen.contains("Sequence"), "{sequence_screen}");
    assert!(sequence_screen.contains("Chapter One"), "{sequence_screen}");
    assert!(
        sequence_screen.contains("The first chapter."),
        "{sequence_screen}"
    );
    assert!(sequence_screen.contains("Welcome"), "{sequence_screen}");
    let chapter_post = t.state.panel.as_ref().unwrap().items[0].target.clone();
    assert_eq!(chapter_post, s.welcome);
    block_on(t.press("Enter"));
    assert!(t.on_thread());
    assert_eq!(t.state.top_thread().unwrap().uri, s.welcome);
}

#[test]
fn wiki_and_every_sequence_write_go_through_the_real_outbox() {
    let s = scenario();
    let dir = tempfile::tempdir().unwrap();
    let script = common::editor_script(dir.path(), "About **Rust** from the TUI editor.");
    let editor = common::editor(dir.path(), &script);
    let mut t = alice_tui(&s, Some(editor));
    block_on(t.start(Some(s.forum.clone())));

    run_command(
        &mut t,
        "wiki edit rust --name \"Rust Language\" --description \"Systems programming\" --summary \"Written in TUI\" --parent-tag programming",
    );
    let writes = block_on(t.engine().writes()).unwrap();
    let wiki_uri = done_in(&writes, nsid::WIKI_REVISION);
    let wiki_rkey = wiki_uri.rsplit('/').next().unwrap();
    let wiki = s.w.alice.acct.get(nsid::WIKI_REVISION, wiki_rkey).unwrap();
    assert_eq!(wiki["name"], "Rust Language");
    assert_eq!(wiki["description"], "Systems programming");
    assert_eq!(wiki["summary"], "Written in TUI");
    assert_eq!(wiki["parentTags"], json!(["programming"]));
    assert_eq!(wiki["body"]["text"], "About **Rust** from the TUI editor.");

    let draft = |title: &str| {
        json!({
            "forum": ".",
            "title": title,
            "body": {"markdown": "Introduction"},
            "chapters": [
                {"title": "One", "body": {"markdown": "Chapter body"}, "posts": [s.welcome]},
                {"title": "Two", "posts": []}
            ]
        })
        .to_string()
    };
    run_command(
        &mut t,
        &format!("sequence create {}", draft("Reading list")),
    );
    let writes = block_on(t.engine().writes()).unwrap();
    let sequence_uri = done_in(&writes, nsid::SEQUENCE);
    let sequence_rkey = sequence_uri.rsplit('/').next().unwrap();
    let created = s.w.alice.acct.get(nsid::SEQUENCE, sequence_rkey).unwrap();
    assert_eq!(created["title"], "Reading list");
    assert_eq!(created["chapters"][0]["posts"], json!([s.welcome]));

    run_command(
        &mut t,
        &format!(
            "sequence edit {sequence_uri} {}",
            draft("Edited reading list")
        ),
    );
    run_command(
        &mut t,
        &format!(
            "sequence add {sequence_uri} {} --chapter 0 --index 1",
            s.bob_post
        ),
    );
    run_command(
        &mut t,
        &format!("sequence move {sequence_uri} --chapter 0 --index 0 --to-chapter 1 --to-index 0"),
    );
    run_command(
        &mut t,
        &format!("sequence remove {sequence_uri} {}", s.bob_post),
    );

    let writes = block_on(t.engine().writes()).unwrap();
    assert_eq!(
        writes
            .iter()
            .filter(|ticket| matches!(ticket.state, WriteState::Done { ref uri, .. } if uri == &sequence_uri))
            .count(),
        5,
        "create, edit, add, move and remove all completed: {writes:?}"
    );
    let record = s.w.alice.acct.get(nsid::SEQUENCE, sequence_rkey).unwrap();
    assert_eq!(record["title"], "Edited reading list");
    assert_eq!(record["chapters"][0]["posts"], json!([]));
    assert_eq!(record["chapters"][1]["posts"], json!([s.welcome]));
}

/// An `$EDITOR` stand-in that saves the template it was given to `seen` and replaces the whole
/// file with `text`.
fn replacing_editor(dir: &std::path::Path, text: &str) -> lessdugin_tui::EditorConfig {
    let reply = dir.join("reply.txt");
    std::fs::write(&reply, text).unwrap();
    let script = dir.join("replace.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nset -e\ncat \"$1\" > '{}'\ncat '{}' > \"$1\"\n",
            dir.join("seen.txt").display(),
            reply.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    common::editor(dir, &script)
}

fn wiki_revision(s: &common::Scenario, slug: &str, text: &str, base: Option<String>) -> String {
    let t = block_on(s.w.alice.engine.wiki_edit(
        &s.forum,
        slug,
        WikiDraft {
            name: "Rust".into(),
            description: None,
            body: Some(BodyDraft::text(text)),
            is_tag: true,
            parent_tags: vec![],
            summary: None,
            base,
        },
    ))
    .unwrap();
    s.w.alice.flush();
    s.w.alice.done(&t)
}

fn wiki_parents(s: &common::Scenario, revision: &str) -> Vec<String> {
    let rkey = revision.rsplit('/').next().unwrap();
    let rec = s.w.alice.acct.get(nsid::WIKI_REVISION, rkey).unwrap();
    rec["parents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["uri"].as_str().unwrap().to_owned())
        .collect()
}

/// Decision O-2 (`engine-api.md` §3.5 `wiki_edit`) in the TUI: `:wiki edit` loads the page,
/// opens the editor on its text and names the head it loaded as the edit's base. When the page
/// moves on while the editor is open, the edit is refused instead of replacing the newer
/// revision: the text is kept, the status line says the page changed, and the next `:wiki
/// edit` reloads the page, offering the kept text with the page as it is now quoted under it,
/// and writes the redo on the new head.
#[test]
fn a_wiki_edit_whose_page_moved_is_refused_and_redone_on_the_new_head() {
    use lessdugin_tui::exec::{ExecOpts, run};
    use lessdugin_tui::{Action, Effect};
    let s = scenario();
    let r1 = wiki_revision(&s, "rust", "The first text.", None);
    let dir = tempfile::tempdir().unwrap();
    let mut t = alice_tui(&s, Some(replacing_editor(dir.path(), "My redo text.")));
    block_on(t.start(Some(s.forum.clone())));

    // the command loads the page first, then opens the editor on its text, based on r1
    block_on(t.press(":"));
    for c in "wiki edit rust --name Rust".chars() {
        block_on(t.press(&c.to_string()));
    }
    let mut fx = t.state.update(Action::Key(
        lessdugin_tui::keys::parse_key("Enter").unwrap(),
    ));
    assert!(
        matches!(fx.as_slice(), [Effect::StartWiki { slug, .. }] if slug == "rust"),
        "{fx:?}"
    );
    let ready = block_on(run(t.engine(), fx.remove(0), ExecOpts::default())).unwrap();
    let mut fx = t.state.update(ready);
    let Some(Effect::Editor(request)) = fx.pop() else {
        panic!("no editor session: {fx:?}");
    };
    let lessdugin_tui::app::Compose::Wiki { draft, .. } = &request.compose else {
        panic!("{:?}", request.compose);
    };
    assert_eq!(draft.base.as_deref(), Some(r1.as_str()));
    assert!(
        request.template.starts_with("The first text.\n"),
        "{}",
        request.template
    );

    // while the editor is open, the page moves on to r2
    let r2 = wiki_revision(&s, "rust", "The second text.", Some(r1.clone()));
    assert_eq!(wiki_parents(&s, &r2), std::slice::from_ref(&r1));
    t.act(Action::Edited {
        compose: request.compose,
        result: Ok("My edit of the first text.".into()),
    });
    block_on(t.settle());
    let status = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(status.contains("changed since it was loaded"), "{status}");
    assert!(status.contains(":wiki edit rust"), "{status}");
    assert_eq!(s.w.alice.acct.list(nsid::WIKI_REVISION).len(), 2);

    // `:wiki edit` again reloads the page: the kept text, the page as it is now under it, and
    // the redo is written on r2
    run_command(&mut t, "wiki edit rust --name Rust");
    let seen = std::fs::read_to_string(dir.path().join("seen.txt")).unwrap();
    assert!(seen.starts_with("My edit of the first text.\n"), "{seen}");
    assert!(seen.contains("# > The second text."), "{seen}");
    let writes = block_on(t.engine().writes()).unwrap();
    let r3 = writes
        .iter()
        .filter_map(|w| match &w.state {
            WriteState::Done { uri, .. } if uri.contains(nsid::WIKI_REVISION) => Some(uri.clone()),
            _ => None,
        })
        .find(|u| *u != r1 && *u != r2)
        .unwrap_or_else(|| panic!("no redo written: {writes:?}"));
    assert_eq!(wiki_parents(&s, &r3), [r2]);
    let rec =
        s.w.alice
            .acct
            .get(nsid::WIKI_REVISION, r3.rsplit('/').next().unwrap())
            .unwrap();
    assert_eq!(rec["body"]["text"], "My redo text.");
}

/// `social.md` §14, `engine-api.md` §3.4 `get_post(uri, sequence?)` (review m5-acceptance-8) in
/// the TUI: a post opened from a sequence's panel is read in that sequence, and `]` and `[`
/// step along it (each step read in it too, in place of the thread on top). For a post in two
/// sequences, next and previous are the ones of the sequence the reader came from; the
/// sequence's panel then offers "Continue reading" first, opened in it.
#[test]
fn posts_opened_from_a_sequence_are_read_in_it() {
    let s = scenario();
    let alice = &s.w.alice;
    let publish = |title: &str| {
        let t = block_on(
            alice
                .engine
                .publish(&lessdugin_core::dto::PostDraft::article(
                    &s.forum,
                    title,
                    format!("About {title}."),
                )),
        )
        .unwrap();
        alice.flush();
        alice.done(&t)
    };
    let (p1, p2, p3) = (publish("One"), publish("Two"), publish("Three"));
    let sequence = |title: &str, posts: Vec<String>| {
        let t = block_on(alice.engine.seq_create(&SequenceDraft {
            forum: s.forum.clone(),
            title: title.into(),
            body: None,
            chapters: vec![SequenceChapterDraft {
                title: Some("One".into()),
                body: None,
                posts,
            }],
        }))
        .unwrap();
        alice.flush();
        alice.done(&t)
    };
    let forward = sequence("Forward", vec![p1.clone(), p2.clone(), p3.clone()]);
    let backward = sequence("Backward", vec![p3.clone(), p2.clone(), p1.clone()]);
    s.w.tick(MINUTE);
    s.w.carol.sync(&s.forum);

    let mut t = common::tui(&s, 100, 32);
    block_on(t.start(Some(s.forum.clone())));
    let depth = t.state.stack.len();
    let nav = |t: &Tui| {
        let n = &t.thread_view().expect("a thread").post;
        let lessdugin_core::dto::ThreadRoot::Post(p) = n else {
            panic!("{n:?}");
        };
        let n = p.sequences.first().expect("a sequence nav");
        (
            p.post.uri.clone(),
            n.sequence.clone(),
            n.prev.clone(),
            n.next.clone(),
        )
    };

    // the backward sequence's second post, opened from its panel
    run_command(&mut t, &format!("sequence {backward}"));
    let panel = t.state.panel.as_ref().expect("the sequence panel");
    assert!(
        !panel
            .lines
            .iter()
            .any(|l| l.to_plain_string().contains("Continue reading")),
        "{panel:?}"
    );
    block_on(t.press("j"));
    block_on(t.press("Enter"));
    assert_eq!(
        t.state.top_thread().unwrap().sequence.as_deref(),
        Some(backward.as_str())
    );
    assert_eq!(
        nav(&t),
        (
            p2.clone(),
            backward.clone(),
            Some(p3.clone()),
            Some(p1.clone())
        )
    );
    let screen = t.render();
    assert!(
        screen.contains("sequence: Backward  [ previous  ] next"),
        "{screen}"
    );

    // `]` steps to One, in the same sequence, in place of the thread
    block_on(t.press("]"));
    assert_eq!(t.state.stack.len(), depth + 1);
    assert_eq!(
        nav(&t),
        (p1.clone(), backward.clone(), Some(p2.clone()), None)
    );
    block_on(t.press("]"));
    let status = t.state.status.as_ref().unwrap().text.to_plain_string();
    assert!(status.contains("last post of Backward"), "{status}");
    block_on(t.press("["));
    assert_eq!(nav(&t).0, p2);

    // the sequence continues there: offered first, and opened in it
    block_on(t.press("h"));
    run_command(&mut t, &format!("sequence {backward}"));
    let panel = t.state.panel.as_ref().expect("the sequence panel");
    assert!(
        panel
            .lines
            .iter()
            .any(|l| l.to_plain_string() == "Continue reading: Two"),
        "{panel:?}"
    );
    assert_eq!(panel.items[0].target, p2);
    block_on(t.press("Enter"));
    assert_eq!(nav(&t).1, backward);

    // from the forward sequence, the same post is between One and Three
    block_on(t.press("h"));
    run_command(&mut t, &format!("sequence {forward}"));
    block_on(t.press("j"));
    block_on(t.press("Enter"));
    assert_eq!(nav(&t), (p2, forward, Some(p1), Some(p3)));
}

/// The new optional request fields and arguments (`request`) and output fields (`output`) that
/// `engine-api.md` §1 "After the freeze (`milestone`)" names, as `Type.field` (an argument
/// `op(a, b?)` as `op.b`).
fn freeze_fields(milestone: &str) -> std::collections::BTreeMap<String, &'static str> {
    let spec = include_str!("../../../docs/spec/engine-api.md");
    let head = format!("- **After the freeze ({milestone}).**");
    let start = spec.find(&head).unwrap_or_else(|| panic!("no {head:?}"));
    let rest = &spec[start + head.len()..];
    let para = &rest[..rest.find("\n- **").unwrap_or(rest.len())];
    let para = para.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = std::collections::BTreeMap::new();
    for (marker, kind) in [
        ("ew optional request field", "request"),
        ("ew optional request argument", "request"),
        ("ew optional output field", "output"),
    ] {
        for (i, _) in para.match_indices(marker) {
            let Some(code) = para[i + marker.len()..].split('`').nth(1) else {
                continue;
            };
            match code.split_once('(') {
                Some((op, args)) => {
                    for a in args.trim_end_matches(')').split(',') {
                        if let Some(a) = a.trim().strip_suffix('?') {
                            out.insert(format!("{op}.{a}"), kind);
                        }
                    }
                }
                None => {
                    out.insert(code.to_owned(), kind);
                }
            }
        }
    }
    out
}

/// `engine-api.md` §1 "After the freeze (M5)": `command::FIELD_REACH` names every operation
/// argument and field that M5 adds to dto/1, and nothing else, so a field the spec adds there
/// fails here until the TUI reaches it. Each request entry, run on alice's welcome thread (the
/// first post of her sequence), queues what sends its field: the wiki edit's start, whose
/// loaded draft is based on the page's head; the thread load read in the sequence; the edit's
/// start with the link card dropped.
#[test]
fn every_new_field_has_a_way_in() {
    use lessdugin_tui::command::{FIELD_REACH, Reach};
    use lessdugin_tui::exec::{ExecOpts, run};
    use lessdugin_tui::{Action, Effect};
    let spec = freeze_fields("M5");
    let have: std::collections::BTreeSet<String> =
        FIELD_REACH.iter().map(|(f, _)| (*f).to_owned()).collect();
    assert_eq!(FIELD_REACH.len(), have.len(), "a field is listed twice");
    let want: std::collections::BTreeSet<String> = spec.keys().cloned().collect();
    assert!(want.len() >= 6, "the paragraph was not read: {spec:?}");
    assert_eq!(
        have, want,
        "the M5 fields the TUI reaches, against engine-api.md"
    );

    let s = scenario();
    wiki_revision(&s, "rust", "About Rust.", None);
    let t = block_on(s.w.alice.engine.seq_create(&SequenceDraft {
        forum: s.forum.clone(),
        title: "Start here".into(),
        body: None,
        chapters: vec![SequenceChapterDraft {
            title: None,
            body: None,
            posts: vec![s.welcome.clone(), s.bob_post.clone()],
        }],
    }))
    .unwrap();
    s.w.alice.flush();
    s.w.alice.done(&t);
    let mut t = alice_tui(&s, None);
    block_on(t.start(Some(s.forum.clone())));
    for (field, r) in FIELD_REACH {
        if spec[*field] == "output" {
            assert!(
                matches!(r, Reach::Automatic(how) if !how.is_empty()),
                "{field}: say where it shows"
            );
            continue;
        }
        block_on(t.open_thread(&s.welcome));
        let keys: Vec<String> = match r {
            Reach::Command(line) => std::iter::once(":".to_owned())
                .chain(line.chars().map(String::from))
                .chain(std::iter::once("Enter".to_owned()))
                .collect(),
            Reach::Keys(keys) => keys.iter().map(|k| (*k).to_owned()).collect(),
            other => panic!("{field}: {other:?} is not a way in"),
        };
        let (last, first) = keys.split_last().unwrap();
        for k in first {
            block_on(t.press(k));
        }
        let fx = t
            .state
            .update(Action::Key(lessdugin_tui::keys::parse_key(last).unwrap()));
        let sent = fx.iter().any(|e| match (*field, e) {
            ("WikiDraft.base", Effect::StartWiki { .. }) => {
                match block_on(run(t.engine(), e.clone(), ExecOpts::default())) {
                    Some(Action::WikiReady(Ok(start))) => start.draft.base.is_some(),
                    other => panic!("{other:?}"),
                }
            }
            ("get_post.sequence", Effect::LoadThread { sequence, .. }) => sequence.is_some(),
            ("EditOpts.dropLinkCard", Effect::StartEdit { drop_link_card, .. }) => *drop_link_card,
            _ => false,
        });
        assert!(sent, "{field} via {r:?}: {fx:?}");
    }
}
