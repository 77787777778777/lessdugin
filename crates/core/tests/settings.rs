// SPDX-License-Identifier: AGPL-3.0-or-later
//! `settings()` / `set_setting()` (engine-api.md 3.6, M4) and the media allowlist (3.9).

use lessdugin_core::engine::{Engine, EngineConfig, ErrorCode, SETTINGS};
use lessdugin_testkit::MemHost;

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

fn engine() -> Engine<MemHost> {
    Engine::new(MemHost::new(), EngineConfig::default())
}

#[test]
fn a_setting_round_trips_and_clearing_it_returns_to_the_default() {
    let e = engine();
    assert!(
        block_on(e.settings()).unwrap().is_empty(),
        "nothing set yet"
    );
    block_on(e.set_setting("sources", Some("direct"))).unwrap();
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.get("sources").map(String::as_str), Some("direct"));
    // clearing it leaves no row, so a client can tell "default" from "chosen"
    block_on(e.set_setting("sources", None)).unwrap();
    assert!(block_on(e.settings()).unwrap().is_empty());
}

#[test]
fn an_unknown_setting_is_refused_rather_than_stored() {
    let e = engine();
    let err = block_on(e.set_setting("sorce", Some("direct"))).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(
        err.detail.as_ref().and_then(|d| d.get("kind")),
        Some(&serde_json::json!("unknownSetting"))
    );
    assert!(
        block_on(e.settings()).unwrap().is_empty(),
        "a typo stores nothing"
    );
}

#[test]
fn a_value_the_setting_cannot_mean_is_refused() {
    let e = engine();
    for (key, bad) in [
        ("sources", "sometimes"),
        ("verify", "maybe"),
        ("mediaAutoload", "yes"),
        ("externalImages", "1"),
    ] {
        let err = block_on(e.set_setting(key, Some(bad))).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{key} = {bad}");
        assert!(
            err.message.contains(key),
            "the message must name the setting: {}",
            err.message
        );
    }
    // and the good values are accepted
    for (key, good) in [
        ("sources", "assisted"),
        ("verify", "proof"),
        ("mediaAutoload", "on"),
        ("externalImages", "off"),
    ] {
        block_on(e.set_setting(key, Some(good))).unwrap();
    }
    assert_eq!(block_on(e.settings()).unwrap().len(), 4);
}

#[test]
fn the_media_allowlist_parses_trimmed_and_nonempty() {
    let e = engine();
    assert!(
        block_on(e.media_hosts()).unwrap().is_empty(),
        "no allowlist yet"
    );
    block_on(e.set_setting("mediaHosts", Some("a.example, b.example ,,  ,c.example "))).unwrap();
    assert_eq!(
        block_on(e.media_hosts()).unwrap(),
        vec!["a.example", "b.example", "c.example"]
    );
    block_on(e.set_setting("mediaHosts", None)).unwrap();
    assert!(block_on(e.media_hosts()).unwrap().is_empty());
}

#[test]
fn every_documented_setting_is_known() {
    // synthesis-v1.md §6.12 names sources, verification, external images and media autoload hosts;
    // the list is the contract `set_setting` enforces, so it cannot silently shrink.
    for k in [
        "sources",
        "verify",
        "mediaAutoload",
        "mediaHosts",
        "externalImages",
    ] {
        assert!(SETTINGS.contains(&k), "{k} must be a known setting");
    }
    let e = engine();
    for k in SETTINGS {
        block_on(e.set_setting(
            k,
            Some(match k {
                "sources" => "assisted",
                "verify" => "pds",
                "mediaAutoload" | "externalImages" => "on",
                _ => "a.example",
            }),
        ))
        .unwrap();
    }
    assert_eq!(block_on(e.settings()).unwrap().len(), SETTINGS.len());
}

#[test]
fn drafts_round_trip_overwrite_and_delete() {
    // engine-api.md §3.6 (M4). The SPA's composer autosaves through these, so a no-op here would
    // silently lose every draft; there was no store at all before M4.
    use lessdugin_core::dto::{BodyDraft, Draft, PostDraft};
    let e = engine();
    assert!(
        block_on(e.drafts()).unwrap().is_empty(),
        "nothing saved yet"
    );

    let draft = Draft::Post {
        id: None,
        post: PostDraft {
            forum: "at://did:plc:alice/online.7778777.lessdugin.temp.forum/meta".into(),
            kind: lessdugin_core::dto::PostKind::Article,
            title: Some("A question".into()),
            body: BodyDraft::text("half-written"),
            tags: Vec::new(),
            labels: Vec::new(),
            link: None,
        },
    };
    let id = block_on(e.save_draft(&draft)).unwrap();
    assert!(!id.is_empty());
    let saved = block_on(e.drafts()).unwrap();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0].id, id);

    // a second save with the id OVERWRITES, which is what autosave does every second
    let updated = Draft::Post {
        id: Some(id.clone()),
        post: PostDraft {
            forum: "at://did:plc:alice/online.7778777.lessdugin.temp.forum/meta".into(),
            kind: lessdugin_core::dto::PostKind::Article,
            title: Some("A question".into()),
            body: BodyDraft::text("finished"),
            tags: Vec::new(),
            labels: Vec::new(),
            link: None,
        },
    };
    assert_eq!(block_on(e.save_draft(&updated)).unwrap(), id);
    assert_eq!(block_on(e.drafts()).unwrap().len(), 1, "one draft, not two");

    block_on(e.delete_draft(&id)).unwrap();
    assert!(block_on(e.drafts()).unwrap().is_empty());
    // deleting what is gone is not an error: the composer deletes on publish regardless
    block_on(e.delete_draft(&id)).unwrap();
}
