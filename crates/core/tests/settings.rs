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
    // the defaults, before anything is chosen
    let d = block_on(e.settings()).unwrap();
    assert_eq!(d.sources, lessdugin_core::dto::Sources::Assisted);
    assert_eq!(d.verify, lessdugin_core::dto::VerifyMode::Pds);
    assert!(d.media_autoload_hosts.is_empty());
    block_on(e.set_setting("sources", Some("direct"))).unwrap();
    assert_eq!(
        block_on(e.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Direct
    );
    // clearing it returns to the default
    block_on(e.set_setting("sources", None)).unwrap();
    assert_eq!(
        block_on(e.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Assisted
    );
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
    assert_eq!(
        block_on(e.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Assisted,
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
    // four good values were accepted; the settings (a) report them
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.sources, lessdugin_core::dto::Sources::Assisted);
    assert_eq!(s.verify, lessdugin_core::dto::VerifyMode::Proof);
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
    // every documented setting is accepted and visible through the object
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.sources, lessdugin_core::dto::Sources::Assisted);
    assert_eq!(s.verify, lessdugin_core::dto::VerifyMode::Pds);
    assert_eq!(s.media_autoload_hosts, vec!["a.example".to_owned()]);
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

#[test]
fn settings_is_an_object_with_the_clients_list_fields() {
    // The CI-only e2e failure was `TypeError: Cannot read properties of undefined (reading
    // 'map')` on the settings page: `settings()` returned a flat key/value map while the client
    // declares an OBJECT with four list fields and calls `indexers.map(...)`. The client's type is
    // the contract (engine-api.md §3.6 lists `settings()` as M4, and `types.ts` mirrors the DTOs),
    // so this pins the shape that makes that page render.
    let e = engine();
    let s = block_on(e.settings()).unwrap();
    // the four list fields must exist as LISTS, even when empty: `undefined.map` is the bug
    assert!(s.indexers.is_empty());
    assert!(s.external_images_forums.is_empty());
    assert!(s.media_autoload_hosts.is_empty());
    assert!(s.acknowledged_public_actions.is_empty());
    // and the scalar fields report their documented defaults
    assert_eq!(s.sources, lessdugin_core::dto::Sources::Assisted);
    assert_eq!(s.verify, lessdugin_core::dto::VerifyMode::Pds);
    assert_eq!(
        s.karma_notifications,
        lessdugin_core::dto::KarmaCadence::Daily
    );

    // a chosen value shows through the same object
    block_on(e.set_setting("sources", Some("direct"))).unwrap();
    block_on(e.set_setting("mediaHosts", Some("a.example,b.example"))).unwrap();
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.sources, lessdugin_core::dto::Sources::Direct);
    assert_eq!(
        s.media_autoload_hosts,
        vec!["a.example".to_owned(), "b.example".to_owned()]
    );
}
