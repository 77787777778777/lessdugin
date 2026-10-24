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
