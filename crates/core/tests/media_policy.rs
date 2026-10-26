// SPDX-License-Identifier: AGPL-3.0-or-later
//! The autoload policy (synthesis-v1.md 3.9, threat-model.md F29).

use lessdugin_core::media::policy::{
    AUTOLOAD_MAX_BYTES, Autoload, HOST_SHARED_BY_PARTICIPANTS, LoadReason, MediaCandidate,
    autoload, host_of, is_bluesky_host,
};

fn item<'a>(host: &'a str, labels: &'a [String], size: Option<u64>) -> MediaCandidate<'a> {
    MediaCandidate {
        author: "did:plc:alice",
        visible: true,
        labels,
        host,
        size_bytes: size,
    }
}

fn participants(n: usize) -> Vec<String> {
    (0..n).map(|_| "pds.example.com".to_owned()).collect()
}

#[test]
fn every_condition_must_hold_for_an_automatic_load() {
    let none: [String; 0] = [];
    let v = autoload(
        &item("pds.example.com", &none, None),
        true,
        &participants(5),
        &[],
    );
    assert_eq!(v, Autoload::Yes);
}

#[test]
fn an_unknown_author_always_asks_first() {
    let none: [String; 0] = [];
    let v = autoload(&item("pds.host.bsky.network", &none, None), false, &[], &[]);
    assert_eq!(
        v,
        Autoload::ClickThrough {
            reason: LoadReason::AuthorNotEstablished,
            host: "pds.host.bsky.network".into()
        }
    );
}

#[test]
fn a_hidden_item_is_never_fetched() {
    let none: [String; 0] = [];
    let mut it = item("pds.example.com", &none, None);
    it.visible = false;
    let v = autoload(&it, true, &participants(5), &[]);
    assert!(matches!(
        v,
        Autoload::ClickThrough {
            reason: LoadReason::NotVisible,
            ..
        }
    ));
}

#[test]
fn a_labelled_item_asks_first() {
    for label in ["sexual", "nudity", "porn", "graphic-media"] {
        let labels = vec![label.to_string()];
        let v = autoload(
            &item("pds.example.com", &labels, None),
            true,
            &participants(9),
            &[],
        );
        assert!(
            matches!(
                v,
                Autoload::ClickThrough {
                    reason: LoadReason::Label,
                    ..
                }
            ),
            "{label} must ask first, got {v:?}"
        );
    }
    // an unrelated label does not
    let labels = vec!["spoiler".to_string()];
    assert_eq!(
        autoload(
            &item("pds.example.com", &labels, None),
            true,
            &participants(5),
            &[]
        ),
        Autoload::Yes
    );
}

#[test]
fn a_host_is_known_when_shared_or_bluesky_or_allowlisted() {
    let none: [String; 0] = [];
    // 4 participants is one short of the threshold
    let v = autoload(
        &item("pds.example.com", &none, None),
        true,
        &participants(4),
        &[],
    );
    assert!(matches!(
        v,
        Autoload::ClickThrough {
            reason: LoadReason::HostUnknown,
            ..
        }
    ));
    // exactly the threshold is enough
    assert_eq!(
        autoload(
            &item("pds.example.com", &none, None),
            true,
            &participants(5),
            &[]
        ),
        Autoload::Yes
    );
    // a Bluesky host needs no participants at all
    assert_eq!(
        autoload(&item("pds.host.bsky.network", &none, None), true, &[], &[]),
        Autoload::Yes
    );
    // nor does an allowlisted one
    let allow = vec!["pds.example.com".to_string()];
    assert_eq!(
        autoload(&item("pds.example.com", &none, None), true, &[], &allow),
        Autoload::Yes
    );
}

#[test]
fn a_big_image_is_click_to_load() {
    let none: [String; 0] = [];
    let at = autoload(
        &item("pds.example.com", &none, Some(AUTOLOAD_MAX_BYTES)),
        true,
        &participants(5),
        &[],
    );
    assert_eq!(at, Autoload::Yes, "exactly the limit still autoloads");
    let over = autoload(
        &item("pds.example.com", &none, Some(AUTOLOAD_MAX_BYTES + 1)),
        true,
        &participants(5),
        &[],
    );
    assert!(matches!(
        over,
        Autoload::ClickThrough {
            reason: LoadReason::TooLarge,
            ..
        }
    ));
    // an unknown size is not treated as too large
    assert_eq!(
        autoload(
            &item("pds.example.com", &none, None),
            true,
            &participants(5),
            &[]
        ),
        Autoload::Yes
    );
}

#[test]
fn the_reason_names_the_first_condition_that_fails() {
    let labels = vec!["porn".to_string()];
    // not established wins over the label, which wins over the host
    let v = autoload(&item("unknown.example", &labels, None), false, &[], &[]);
    assert!(matches!(
        v,
        Autoload::ClickThrough {
            reason: LoadReason::AuthorNotEstablished,
            ..
        }
    ));
    let v = autoload(&item("unknown.example", &labels, None), true, &[], &[]);
    assert!(matches!(
        v,
        Autoload::ClickThrough {
            reason: LoadReason::Label,
            ..
        }
    ));
}

#[test]
fn hosts_parse_and_bluesky_hosts_are_recognised() {
    assert_eq!(host_of("https://pds.example.com"), "pds.example.com");
    assert_eq!(
        host_of("https://pds.example.com/some/path"),
        "pds.example.com"
    );
    assert_eq!(host_of("not a url"), "");
    assert!(is_bluesky_host("pds.host.bsky.network"));
    assert!(is_bluesky_host("bsky.network"));
    assert!(!is_bluesky_host("bsky.network.example.com"));
    assert!(!is_bluesky_host("evil-bsky.network"));
    assert_eq!(HOST_SHARED_BY_PARTICIPANTS, 5);
}
