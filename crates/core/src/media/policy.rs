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
//! Everything here is a pure function of what the fold already knows. The engine computes the
//! verdict while it builds a view (`MediaView.autoload`, `sync.md` §11.3), so the CLI, the TUI
//! and the SPA show the same thing.

use url::Url;

/// Labels that always ask for a click before fetching bytes (F29).
pub const CLICK_THROUGH_LABELS: [&str; 4] = ["sexual", "nudity", "porn", "graphic-media"];

/// Images at or under this size load by themselves once the other conditions hold.
pub const AUTOLOAD_MAX_BYTES: u64 = crate::limits::BLOB_AUTOLOAD_MAX_BYTES;

/// The inline image types (`sync.md` §11.2) - the only ones that ever load without a click.
/// AVIF is inline in the SPA only and never uploaded, so it is not among them.
pub const AUTOLOAD_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

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
    /// One of CLICK_THROUGH_LABELS is on the item (or, for a comment, on its root post).
    Label,
    /// The host serves too few participants, is not a Bluesky host, and is not allowlisted.
    HostUnknown,
    /// The file is not an inline image (video, audio and the rest are always click-to-load).
    NotAnImage,
    /// The file is larger than AUTOLOAD_MAX_BYTES, or the record does not state its size.
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
    /// For a comment, its root post's self-labels, which count too (§11.3); empty otherwise.
    pub root_labels: &'a [String],
    /// The PDS host that would serve the bytes, as recorded on the author.
    pub host: &'a str,
    /// The blob's declared `mimeType`.
    pub mime: &'a str,
    /// The blob size, when the record states it (an unknown size never autoloads).
    pub size_bytes: Option<u64>,
}

/// The host of a PDS URL, or "" when it cannot be parsed.
pub fn host_of(pds: &str) -> String {
    match Url::parse(pds) {
        Ok(u) => u.host_str().unwrap_or_default().to_owned(),
        Err(_) => String::new(),
    }
}

/// True when the host is a Bluesky PDS (`*.host.bsky.network`), which 3.9 always treats as
/// known. `bsky.network` itself is the relay, not a PDS host.
pub fn is_bluesky_host(host: &str) -> bool {
    host.ends_with(".host.bsky.network")
}

/// The 3.9 autoload verdict. Conditions are checked in the order the spec lists them, so the
/// reason names the first one that fails.
///
/// established: the author is in S E, as the fold computed it for the forum.
/// shared: how many of the forum's participants the item's host serves (by the PDS in their
/// identity rows). allowlist: hosts the reader added by hand.
pub fn autoload(
    item: &MediaCandidate<'_>,
    established: bool,
    shared: usize,
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
        .chain(item.root_labels)
        .any(|l| CLICK_THROUGH_LABELS.contains(&l.as_str()))
    {
        return Autoload::ClickThrough {
            reason: LoadReason::Label,
            host,
        };
    }
    let known = !host.is_empty()
        && (is_bluesky_host(&host)
            || allowlist.iter().any(|a| a.eq_ignore_ascii_case(&host))
            || shared >= HOST_SHARED_BY_PARTICIPANTS);
    if !known {
        return Autoload::ClickThrough {
            reason: LoadReason::HostUnknown,
            host,
        };
    }
    let mime = item.mime.split(';').next().unwrap_or_default().trim();
    if !AUTOLOAD_TYPES.iter().any(|t| t.eq_ignore_ascii_case(mime)) {
        return Autoload::ClickThrough {
            reason: LoadReason::NotAnImage,
            host,
        };
    }
    if item.size_bytes.is_none_or(|n| n > AUTOLOAD_MAX_BYTES) {
        return Autoload::ClickThrough {
            reason: LoadReason::TooLarge,
            host,
        };
    }
    Autoload::Yes
}
