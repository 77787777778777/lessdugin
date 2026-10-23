// SPDX-License-Identifier: AGPL-3.0-or-later
//! The parity hook (`docs/plan.md` §4.2): `capture_thread` returns the exact `ThreadView` the
//! Thread screen holds, as dto/1 JSON. It is byte-identical to `get_thread` on the same
//! engine, and, in parity mode (`engine-api.md` §2.1), across devices that synced at different
//! times.

mod common;

use common::{MINUTE, block_on, scenario, scenario_with};
use lessdugin_core::dto::{ThreadOpts, ThreadView};
use lessdugin_core::engine::EngineConfig;
use lessdugin_tui::{CaptureOpts, capture_thread};

#[test]
fn capture_holds_the_engine_view_byte_for_byte() {
    let s = scenario();
    let engine = &s.w.carol.engine;
    let cap = block_on(capture_thread(engine, &s.welcome, CaptureOpts::default())).unwrap();
    let direct = block_on(engine.get_thread(&s.welcome, &ThreadOpts::default())).unwrap();
    assert_eq!(cap.json, serde_json::to_string(&direct).unwrap());
    assert_eq!(
        cap.json_pretty,
        format!("{}\n", serde_json::to_string_pretty(&direct).unwrap())
    );
    // it parses back as the same dto/1 value
    let back: ThreadView = serde_json::from_str(&cap.json).unwrap();
    assert_eq!(back, cap.view);
    assert!(cap.json.starts_with("{\"dto\":1,"), "{}", cap.json);
    assert!(cap.screen.contains("Welcome"), "{}", cap.screen);
    assert!(
        cap.document.contains("Thanks for the forum!"),
        "{}",
        cap.document
    );
    insta::assert_snapshot!("capture_document_80", cap.document);

    // a comment uri opens its thread on that comment, with the same view
    let by_comment = block_on(capture_thread(engine, &s.bob_c1, CaptureOpts::default())).unwrap();
    assert_eq!(by_comment.json, cap.json);
}

#[test]
fn parity_mode_makes_devices_agree() {
    let t = 1_900_000_000_000;
    let s = scenario_with(EngineConfig {
        parity_now_ms: Some(t),
        ..EngineConfig::default()
    });
    // alice syncs later than carol did
    s.w.tick(10 * MINUTE);
    s.w.alice.sync(&s.forum);
    let opts = CaptureOpts {
        width: 100,
        height: 30,
        sync: false,
    };
    let a = block_on(capture_thread(&s.w.alice.engine, &s.welcome, opts)).unwrap();
    let c = block_on(capture_thread(&s.w.carol.engine, &s.welcome, opts)).unwrap();
    assert_eq!(a.json, c.json);
    assert_eq!(a.document, c.document);
}
