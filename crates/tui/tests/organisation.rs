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
