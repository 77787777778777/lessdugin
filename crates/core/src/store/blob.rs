// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small cache rows (`sync.md` §10.5): blob metadata and reference counts (`("blob", cid)`),
//! live-source cursors (`("cur", source)`) and forum access times (`("forum", fk)`).
//!
//! The blob bytes live in the host's blob storage (§12.2); `refs` counts the stored, eligible,
//! non-hidden records that reference the CID and is maintained by the `ForumModel` (§11.4) through
//! [`super::records::Batch::adjust_blob_refs`]. At 0 the host deletes the bytes.

use serde::{Deserialize, Serialize};

use crate::store::row::ROW_VERSION;

/// `BlobRow`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobRow {
    /// Row version.
    pub v: u32,
    /// The DID whose PDS serves it.
    pub did: String,
    /// The declared MIME type.
    pub mime: String,
    /// The sniffed type, once read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sniff: Option<String>,
    /// Size in bytes.
    pub size: u64,
    /// The host holds the bytes.
    pub stored: bool,
    /// References from stored, eligible, non-hidden records.
    pub refs: i64,
    /// When the row was written (ms).
    pub at: i64,
    /// Last use (ms).
    pub used: i64,
}

impl BlobRow {
    /// A row for a blob not fetched yet.
    pub fn new(did: &str, mime: &str, size: u64, now_ms: i64) -> Self {
        Self {
            v: ROW_VERSION,
            did: did.to_owned(),
            mime: mime.to_owned(),
            sniff: None,
            size,
            stored: false,
            refs: 0,
            at: now_ms,
            used: now_ms,
        }
    }
}

/// Jetstream wire versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    /// `/subscribe` with `options_update`.
    V1,
    /// `network.bsky.jetstream.subscribeEvents`.
    V2,
}

/// `CursorRow` (§6.3, §6.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorRow {
    /// Row version.
    pub v: u32,
    /// Jetstream wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<Wire>,
    /// Jetstream instance host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// v2 `seq`, or an indexer cursor as an integer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    /// µs timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_us: Option<i64>,
    /// An opaque cursor string (indexer `getChanges`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// When it was persisted (ms).
    pub at: i64,
}

impl CursorRow {
    /// An empty cursor stamped `now`.
    pub fn new(now_ms: i64) -> Self {
        Self {
            v: ROW_VERSION,
            wire: None,
            instance: None,
            seq: None,
            time_us: None,
            cursor: None,
            at: now_ms,
        }
    }
}

/// `ForumRow`: the forum uri and when it was last viewed (eviction, §10.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumRow {
    /// Row version.
    pub v: u32,
    /// The forum at-uri.
    pub uri: String,
    /// Last view (ms).
    pub last_viewed: i64,
}

impl ForumRow {
    /// A row for `uri` viewed at `now`.
    pub fn new(uri: &str, now_ms: i64) -> Self {
        Self {
            v: ROW_VERSION,
            uri: uri.to_owned(),
            last_viewed: now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::row;

    #[test]
    fn rows_roundtrip() {
        let b = BlobRow::new("did:plc:x", "image/png", 5, 1);
        assert_eq!(
            row::decode::<BlobRow>(&row::encode(&b).unwrap()).unwrap(),
            b
        );
        let mut c = CursorRow::new(2);
        c.wire = Some(Wire::V2);
        c.seq = Some(-1);
        assert_eq!(
            row::decode::<CursorRow>(&row::encode(&c).unwrap()).unwrap(),
            c
        );
        let f = ForumRow::new("at://did:plc:x/c/s", 3);
        assert_eq!(
            row::decode::<ForumRow>(&row::encode(&f).unwrap()).unwrap(),
            f
        );
    }
}
