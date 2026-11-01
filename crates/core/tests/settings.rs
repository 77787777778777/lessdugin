// SPDX-License-Identifier: AGPL-3.0-or-later
//! `settings()` / `set_setting()` (engine-api.md 3.6, M4) and the media allowlist (3.9).

mod common;

use lessdugin_core::engine::{
    AccountInfo, Engine, EngineConfig, EngineError, ErrorCode, ExportScope, SETTINGS, Sources,
};
use lessdugin_core::host::{Host, KvOp, KvStore};
use lessdugin_testkit::identity::{DOH, PLC, install_doc, install_nxdomain};
use lessdugin_testkit::{FakePds, MemHost, resp};
use serde_json::{Value, json};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

fn engine() -> Engine<MemHost> {
    Engine::new(MemHost::new(), EngineConfig::default())
}

/// An engine whose active account is alice (her identity and PDS installed on `host`).
fn with_account() -> (MemHost, FakePds, Engine<MemHost>) {
    let host = MemHost::new();
    let pds = FakePds::install(&host, common::PDS);
    let alice = common::alice();
    alice.install(&host, PLC, DOH);
    let e = Engine::new(host.clone(), EngineConfig::default());
    block_on(e.add_account(AccountInfo::app_password(alice.did.as_str(), common::PDS))).unwrap();
    (host, pds, e)
}

/// `set_setting` with a text value (a stored setting's form).
fn set(e: &Engine<MemHost>, key: &str, v: Option<&str>) -> Result<(), EngineError> {
    block_on(e.set_setting(key, v.map(Value::from).as_ref()))
}

#[test]
fn a_setting_round_trips_and_clearing_it_returns_to_the_default() {
    let e = engine();
    // the defaults, before anything is chosen
    let d = block_on(e.settings()).unwrap();
    assert_eq!(d.sources, lessdugin_core::dto::Sources::Assisted);
    assert_eq!(d.verify, lessdugin_core::dto::VerifyMode::Pds);
    assert!(d.media_autoload_hosts.is_empty());
    set(&e, "sources", Some("direct")).unwrap();
    assert_eq!(
        block_on(e.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Direct
    );
    // clearing it returns to the default
    set(&e, "sources", None).unwrap();
    assert_eq!(
        block_on(e.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Assisted
    );
}

#[test]
fn an_unknown_setting_is_refused_rather_than_stored() {
    let e = engine();
    let err = set(&e, "sorce", Some("direct")).unwrap_err();
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
        let err = set(&e, key, Some(bad)).unwrap_err();
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
        ("verify", "pds"),
        ("mediaAutoload", "on"),
        ("externalImages", "off"),
    ] {
        set(&e, key, Some(good)).unwrap();
    }
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.sources, lessdugin_core::dto::Sources::Assisted);
    assert_eq!(s.verify, lessdugin_core::dto::VerifyMode::Pds);
}

/// `verify = proof` (background verification) is Reserved (sync.md 2.3): nothing implements
/// it, so it is refused instead of being stored and shown as chosen (M4R-5).
#[test]
fn verify_proof_is_refused_rather_than_stored() {
    let e = engine();
    let err = set(&e, "verify", Some("proof")).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported, "{err:?}");
    assert_eq!(
        err.detail.as_ref().and_then(|d| d.get("milestone")),
        Some(&json!("P2"))
    );
    assert_eq!(
        block_on(e.settings()).unwrap().verify,
        lessdugin_core::dto::VerifyMode::Pds
    );
    let x = block_on(e.export_local(ExportScope::Everything)).unwrap();
    assert!(x.members.is_empty(), "nothing was stored: {:?}", x.members);
}

/// alice's handle answers neither DoH nor `.well-known`; only Slingshot (CORE+) knows it.
fn slingshot_only_handle(host: &MemHost) {
    let alice = common::alice();
    install_doc(host, PLC, &alice.did, &alice.doc());
    install_nxdomain(host, DOH, "alice.example.com");
    host.respond_get("https://alice.example.com/", resp::status(404));
    host.respond_get(
        &format!(
            "{}/xrpc/com.bad-example.identity.resolveMiniDoc",
            common::SLINGSHOT
        ),
        resp::ok_json(&json!({
            "did": alice.did.as_str(),
            "handle": alice.handle,
            "pds": alice.pds,
            "signing_key": alice.multikey(),
        })),
    );
}

/// O-3 / M4R-5: the stored `sources` setting is what the engine does, in the running engine and
/// in the next one over the same store: in `direct` no CORE+ endpoint (here Slingshot) is
/// contacted. A flag of the invocation wins over it.
#[test]
fn the_sources_setting_applies_at_once_and_after_a_restart() {
    let host = MemHost::new();
    slingshot_only_handle(&host);
    let e = Engine::new(host.clone(), EngineConfig::default());
    set(&e, "sources", Some("direct")).unwrap();
    assert_eq!(
        block_on(e.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Direct
    );
    assert!(block_on(e.resolve_actor("alice.example.com")).is_err());
    assert!(
        host.requests_to(common::SLINGSHOT).is_empty(),
        "direct: Slingshot must not be asked"
    );
    // a new engine over the same store (a reload, the next CLI run) keeps it
    let again = Engine::new(host.clone(), EngineConfig::default());
    assert_eq!(block_on(again.sources()).unwrap(), Sources::Direct);
    assert!(block_on(again.resolve_actor("alice.example.com")).is_err());
    assert!(host.requests_to(common::SLINGSHOT).is_empty());

    // the same stored setting under a flag of the invocation: the flag wins
    let host = MemHost::new();
    slingshot_only_handle(&host);
    set(
        &Engine::new(host.clone(), EngineConfig::default()),
        "sources",
        Some("direct"),
    )
    .unwrap();
    let flagged = Engine::new(
        host.clone(),
        EngineConfig {
            sources_flag: Some(Sources::Assisted),
            ..EngineConfig::default()
        },
    );
    assert_eq!(
        block_on(flagged.settings()).unwrap().sources,
        lessdugin_core::dto::Sources::Assisted
    );
    let a = block_on(flagged.resolve_actor("alice.example.com")).unwrap();
    assert_eq!(a.did, common::alice().did.as_str());
    assert!(!host.requests_to(common::SLINGSHOT).is_empty());
}

#[test]
fn the_media_allowlist_parses_trimmed_and_nonempty() {
    let e = engine();
    assert!(
        block_on(e.media_hosts()).unwrap().is_empty(),
        "no allowlist yet"
    );
    set(
        &e,
        "mediaHosts",
        Some("a.example, b.example ,,  ,c.example "),
    )
    .unwrap();
    assert_eq!(
        block_on(e.media_hosts()).unwrap(),
        vec!["a.example", "b.example", "c.example"]
    );
    set(&e, "mediaHosts", None).unwrap();
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
        let v = match k {
            "sources" => json!("assisted"),
            "verify" => json!("pds"),
            "mediaAutoload" | "externalImages" => json!("on"),
            "externalImagesForums" | "indexers" => json!([]),
            _ => json!("a.example"),
        };
        block_on(e.set_setting(k, Some(&v))).unwrap();
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
    let (_host, _pds, e) = with_account();
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
    set(&e, "sources", Some("direct")).unwrap();
    set(&e, "mediaHosts", Some("a.example,b.example")).unwrap();
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.sources, lessdugin_core::dto::Sources::Direct);
    assert_eq!(
        s.media_autoload_hosts,
        vec!["a.example".to_owned(), "b.example".to_owned()]
    );
}

/// The client's `setSetting(key, value)` (`types.ts`: a field of `Settings` with a value of its
/// type, engine-api.md 3.6): every field persists and `settings()` reports it; `null` returns
/// it to its default.
#[test]
fn every_settings_field_persists_through_set_setting() {
    use lessdugin_core::dto::{KarmaCadence, PublicActionKind, Sources, VerifyMode};
    let (_host, _pds, e) = with_account();
    let forum = "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.temp.forum/meta";
    for (k, v) in [
        ("sources", json!("direct")),
        ("verify", json!("pds")),
        (
            "indexers",
            json!([{ "url": "https://indexer.example.com", "grantedAtMs": 5 }]),
        ),
        ("externalImagesForums", json!([forum])),
        ("mediaAutoloadHosts", json!(["cdn.example", "b.example"])),
        ("karmaNotifications", json!("weekly")),
        ("acknowledgedPublicActions", json!(["vote", "join"])),
    ] {
        block_on(e.set_setting(k, Some(&v))).unwrap_or_else(|err| panic!("{k}: {err:?}"));
    }
    let s = block_on(e.settings()).unwrap();
    assert_eq!((s.sources, s.verify), (Sources::Direct, VerifyMode::Pds));
    assert_eq!(s.indexers.len(), 1);
    assert_eq!(s.indexers[0].url, "https://indexer.example.com");
    assert_eq!(s.external_images_forums, vec![forum.to_owned()]);
    assert_eq!(s.media_autoload_hosts, vec!["cdn.example", "b.example"]);
    assert_eq!(s.karma_notifications, KarmaCadence::Weekly);
    assert_eq!(
        s.acknowledged_public_actions,
        vec![PublicActionKind::Vote, PublicActionKind::Join]
    );
    // a list replaces the one before; `null` is the default
    block_on(e.set_setting("acknowledgedPublicActions", Some(&json!(["react"])))).unwrap();
    for k in ["indexers", "mediaAutoloadHosts", "karmaNotifications"] {
        block_on(e.set_setting(k, Some(&Value::Null))).unwrap();
    }
    let s = block_on(e.settings()).unwrap();
    assert_eq!(s.acknowledged_public_actions, vec![PublicActionKind::React]);
    assert!(s.indexers.is_empty() && s.media_autoload_hosts.is_empty());
    assert_eq!(s.karma_notifications, KarmaCadence::Daily);
    // values of the wrong shape are refused
    for (k, v) in [
        (
            "indexers",
            json!([{ "url": "http://plain.example", "grantedAtMs": 1 }]),
        ),
        (
            "indexers",
            json!([{ "url": "https://index.example.com:444", "grantedAtMs": 1 }]),
        ),
        (
            "indexers",
            json!([{ "url": "https://user@index.example.com", "grantedAtMs": 1 }]),
        ),
        (
            "indexers",
            json!([{ "url": "https://index.example.com/?query=unexpected", "grantedAtMs": 1 }]),
        ),
        ("mediaAutoloadHosts", json!(["https://cdn.example/x"])),
        ("externalImagesForums", json!(["not a uri"])),
        ("karmaNotifications", json!("hourly")),
        ("sources", json!(["direct"])),
    ] {
        let err = block_on(e.set_setting(k, Some(&v))).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{k} = {v}");
    }
}

fn post_draft(text: &str) -> lessdugin_core::dto::Draft {
    lessdugin_core::dto::Draft::Post {
        id: None,
        post: lessdugin_core::dto::PostDraft {
            forum: "at://did:plc:alice/online.7778777.lessdugin.temp.forum/meta".into(),
            kind: lessdugin_core::dto::PostKind::Article,
            title: Some("A question".into()),
            body: lessdugin_core::dto::BodyDraft::text(text),
            tags: Vec::new(),
            labels: Vec::new(),
            link: None,
        },
    }
}

/// M4R-4: a draft belongs to the account that saved it. Another account on the device does not
/// list it, cannot delete it and does not export it with `--mine`; without an account there is
/// no draft store.
#[test]
fn drafts_belong_to_the_account_that_saved_them() {
    let (host, _pds, e) = with_account();
    let alice = common::alice().did.to_string();
    let id = block_on(e.save_draft(&post_draft("alice's unpublished text"))).unwrap();
    let bob = common::bob();
    bob.install(&host, PLC, DOH);
    block_on(e.add_account(AccountInfo::app_password(bob.did.as_str(), common::PDS))).unwrap();
    assert!(
        block_on(e.drafts()).unwrap().is_empty(),
        "bob sees no draft"
    );
    block_on(e.delete_draft(&id)).unwrap();
    let mine = block_on(e.export_local(ExportScope::Mine)).unwrap();
    assert!(
        !mine
            .members
            .iter()
            .any(|m| m.parts.get(1).is_some_and(|t| t == "draft")),
        "bob's export carries no draft: {:?}",
        mine.members
    );
    block_on(e.set_active_account(&alice)).unwrap();
    let saved = block_on(e.drafts()).unwrap();
    assert_eq!(saved.len(), 1, "bob's delete did not reach alice's draft");
    assert_eq!(saved[0].id, id);

    let err = block_on(engine().save_draft(&post_draft("x"))).unwrap_err();
    assert_eq!(err.code, ErrorCode::AuthRequired);
    assert!(block_on(engine().drafts()).unwrap().is_empty());
}

/// M4R-8: a draft has a size cap, two new drafts saved in the same millisecond get two ids, and
/// the cap on the number of drafts counts the rows that do not decode too.
#[test]
fn drafts_are_capped_in_size_and_number_and_never_share_an_id() {
    use lessdugin_core::engine::{DRAFT_MAX_BYTES, MAX_DRAFTS};
    let (host, _pds, e) = with_account();
    let err = block_on(e.save_draft(&post_draft(&"x".repeat(DRAFT_MAX_BYTES)))).unwrap_err();
    assert_eq!(err.code, ErrorCode::TooLarge, "{err:?}");

    // the clock does not move between the two saves
    let a = block_on(e.save_draft(&post_draft("a post"))).unwrap();
    let b = block_on(e.save_draft(&post_draft("another"))).unwrap();
    assert_ne!(a, b);
    assert_eq!(block_on(e.drafts()).unwrap().len(), 2);

    // rows an import (or a newer version) left that do not decode still count
    let alice = common::alice().did.to_string();
    let junk: Vec<KvOp> = (0..5)
        .map(|i| {
            let id = format!("junk{i}");
            KvOp::Put(
                lessdugin_core::store::key::encode(&[
                    "loc".into(),
                    "draft".into(),
                    alice.as_str().into(),
                    id.as_str().into(),
                ]),
                b"not json".to_vec(),
            )
        })
        .collect();
    block_on(host.state().write(junk)).unwrap();
    for i in 0..MAX_DRAFTS {
        host.advance_ms(1);
        block_on(e.save_draft(&post_draft(&format!("draft {i}")))).unwrap();
    }
    let rows = block_on(host.state().scan(
        lessdugin_core::host::KeyRange::prefix(&lessdugin_core::store::key::encode(&[
            "loc".into(),
            "draft".into(),
            alice.as_str().into(),
        ])),
        lessdugin_core::host::Dir::Forward,
        1000,
    ))
    .unwrap();
    assert_eq!(rows.len(), MAX_DRAFTS, "every row counts toward the cap");
    assert!(
        rows.iter().all(|(_, v)| v.as_slice() != b"not json"),
        "the junk went first"
    );
}
