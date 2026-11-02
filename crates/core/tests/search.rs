// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local search (`sync.md` §13, `lfm-1.md` §12): matching cost on hostile bodies.

use std::time::Duration;

use lessdugin_core::search::snippet;

/// Matching and snippets are linear in the body (security-web-1, robustness-7). A body made of
/// one short word repeated used to cost a count of the whole folded prefix per match, so k
/// matches cost O(k · |P|) (78 ms for a 100 kB comment, 7.8 s for a 1 MB post). Eight times the
/// input must cost about eight times as much, not 64 (bound generous for noise).
#[test]
#[allow(clippy::disallowed_methods)] // timing a test is fine; the crate itself never reads clocks
fn matching_a_repeated_word_is_linear() {
    let time = |times: usize| {
        let body = "a ".repeat(times);
        (0..3)
            .map(|_| {
                let t = std::time::Instant::now();
                let (text, highlights) = snippet(&body, "A").expect("a hit");
                let el = t.elapsed();
                assert_eq!(text.chars().count(), 161);
                assert_eq!(highlights.len(), 80);
                el
            })
            .min()
            .expect("three runs")
    };
    let (small, large) = (time(50_000), time(400_000));
    assert!(
        large < small * 24 + Duration::from_millis(250),
        "100 kB took {small:?}, 800 kB took {large:?}"
    );
}
