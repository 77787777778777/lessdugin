// SPDX-License-Identifier: AGPL-3.0-or-later
//! The media display and autoload policy (synthesis-v1.md 3.9, threat-model.md F29).
//!
//! Fetching media tells the author's PDS, and whichever host serves the bytes, that this
//! device read the item - and from which address. 3.9 therefore makes automatic loading a
//! decision rather than a default: media loads by itself only when the author is a
//! participant this device already trusts, the item is visible, it carries none of the
//! labels that ask for a click, and its host is one the forum already talks to (or one the
//! reader allowlisted).
//!
//! Everything here is a pure function of what the fold already knows, so the CLI, the TUI
//! and the SPA reach the same verdict without asking the engine.

use url::Url;

/// Labels that always ask for a click before fetching bytes (F29).
pub const CLICK_THROUGH_LABELS: [&str; 4] = ["sexual", "nudity", "porn", "graphic-media"];

/// Images at or under this size load by themselves once the other conditions hold.
pub const AUTOLOAD_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How many participants a PDS must serve before its host counts as "already talked to".
pub const HOST_SHARED_BY_PARTICIPANTS: usize = 5;

/// What a client should do with one media item, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Autoload {
    /// Load it now.
    Yes,
    /// Ask first: the notice is "loads from <host>, which will see your IP" (3.9).
    ClickThrough {
        /// Why this item is not loaded automatically.
        reason: LoadReason,
        /// The host that would see the reader's address.
        host: String,
    },
}

/// Why an item is not loaded automatically (3.9's four conditions, in the order it lists them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadReason {
    /// The author is not a participant this device trusts (S E): an unknown author could be
    /// anyone, and fetching tells them the reader looked.
    AuthorNotEstablished,
    /// The item is not visible (hidden, collapsed, tombstoned, missing).
    NotVisible,
    /// One of CLICK_THROUGH_LABELS is on the item.
    Label,
    /// The host serves too few participants, is not a Bluesky host, and is not allowlisted.
    HostUnknown,
    /// The file is larger than AUTOLOAD_MAX_BYTES.
    TooLarge,
}

/// What a client knows about a candidate media item, so the verdict is one pure call.
#[derive(Clone, Debug)]
pub struct MediaCandidate<'a> {
    /// The author's DID.
    pub author: &'a str,
    /// The item's state in the fold.
    pub visible: bool,
    /// The item's self-labels (social.md 17).
    pub labels: &'a [String],
    /// The PDS host that would serve the bytes, as recorded on the author.
    pub host: &'a str,
    /// The blob size, when the record states it.
    pub size_bytes: Option<u64>,
}

/// The host of a PDS URL, or "" when it cannot be parsed.
pub fn host_of(pds: &str) -> String {
    match Url::parse(pds) {
        Ok(u) => u.host_str().unwrap_or_default().to_owned(),
        Err(_) => String::new(),
    }
}

/// True when the host is a Bluesky PDS, which 3.9 always treats as known.
pub fn is_bluesky_host(host: &str) -> bool {
    host == "bsky.network" || host.ends_with(".host.bsky.network")
}

/// The 3.9 autoload verdict. Conditions are checked in the order the spec lists them, so the
/// reason names the first one that fails.
///
/// established: the author is in S E, as the fold computed it for the forum.
/// hosts: the PDS host of every participant, so a host "serves >= 5 participants" can be
/// counted. allowlist: hosts the reader added by hand.
pub fn autoload(
    item: &MediaCandidate<'_>,
    established: bool,
    hosts: &[String],
    allowlist: &[String],
) -> Autoload {
    let host = item.host.to_owned();
    if !established {
        return Autoload::ClickThrough {
            reason: LoadReason::AuthorNotEstablished,
            host,
        };
    }
    if !item.visible {
        return Autoload::ClickThrough {
            reason: LoadReason::NotVisible,
            host,
        };
    }
    if item
        .labels
        .iter()
        .any(|l| CLICK_THROUGH_LABELS.contains(&l.as_str()))
    {
        return Autoload::ClickThrough {
            reason: LoadReason::Label,
            host,
        };
    }
    let known = is_bluesky_host(&host)
        || allowlist.iter().any(|a| a == &host)
        || hosts.iter().filter(|h| *h == &host).count() >= HOST_SHARED_BY_PARTICIPANTS;
    if !known {
        return Autoload::ClickThrough {
            reason: LoadReason::HostUnknown,
            host,
        };
    }
    if item.size_bytes.is_some_and(|n| n > AUTOLOAD_MAX_BYTES) {
        return Autoload::ClickThrough {
            reason: LoadReason::TooLarge,
            host,
        };
    }
    Autoload::Yes
}
