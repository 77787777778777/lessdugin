// SPDX-License-Identifier: AGPL-3.0-or-later
//! Resizing: the screens lay out again at every size, the selection stays on the same item,
//! nothing is drawn past the width, and a sequence of sizes and keys never panics (a small
//! deterministic resize fuzz).

mod common;

use common::{block_on, scenario, tui};
use unicode_width::UnicodeWidthStr;

fn selected(t: &lessdugin_tui::Headless<lessdugin_testkit::MemHost>) -> Option<String> {
    let th = t.state.top_thread()?;
    Some(th.layout.items.get(th.pos.selected)?.uri.clone())
}

fn assert_fits(screen: &str, w: u16, h: u16) {
    let rows: Vec<&str> = screen.split('\n').collect();
    assert_eq!(rows.len(), usize::from(h), "{screen}");
    for r in rows {
        assert!(r.width() <= usize::from(w), "{w}x{h}: {r:?}");
    }
}

#[test]
fn resizing_keeps_the_selection() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(None));
    block_on(t.open_thread(&s.welcome));
    block_on(t.press("j"));
    assert_eq!(selected(&t).as_deref(), Some(s.bob_c1.as_str()));
    for (w, h) in [(40, 12), (160, 48), (24, 6), (100, 30), (80, 24)] {
        block_on(t.resize(w, h));
        let screen = t.render();
        assert_fits(&screen, w, h);
        assert_eq!(selected(&t).as_deref(), Some(s.bob_c1.as_str()), "{w}x{h}");
        let th = t.state.top_thread().unwrap();
        assert_eq!(th.layout.width, w);
        let it = &th.layout.items[th.pos.selected];
        let content = usize::from(h.saturating_sub(3).max(1));
        assert!(
            it.start < th.pos.scroll + content && it.end > th.pos.scroll,
            "{w}x{h}: the selected item shows"
        );
    }
    insta::assert_snapshot!("thread_resized_back_80x24", t.render());
    block_on(t.resize(40, 12));
    insta::assert_snapshot!("thread_40x12", t.render());
}

#[test]
fn resize_fuzz() {
    let s = scenario();
    let mut t = tui(&s, 80, 24);
    block_on(t.start(Some(s.forum.clone())));
    // xorshift: deterministic sizes and keys
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let keys = [
        "j", "k", "g", "G", "Ctrl-d", "Ctrl-u", "Enter", "h", "Space", "p", "s", "?", "Esc",
    ];
    for _ in 0..300 {
        let n = next();
        if n % 3 == 0 {
            let w = u16::try_from(1 + n % 220).unwrap();
            let h = u16::try_from(1 + (n >> 8) % 70).unwrap();
            block_on(t.resize(w, h));
            let screen = t.render();
            assert_fits(&screen, w, h);
        } else {
            let k = keys[usize::try_from((n >> 16) % keys.len() as u64).unwrap()];
            block_on(t.press(k));
            let _ = t.render();
        }
        if t.state.quit {
            break;
        }
    }
}
