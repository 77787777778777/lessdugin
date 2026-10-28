// SPDX-License-Identifier: AGPL-3.0-or-later
//! Small cache rows (`sync.md` §10.5): blob metadata (`("blob", cid)`), blob references
//! (`("blob.ref", cid, fk)` and `("blob.forum", fk, cid)`), live-source cursors
//! (`("cur", source)`) and forum access times (`("forum", fk)`).
//!
//! The blob bytes live in the host's blob storage (§12.2). Their references are counted per
//! forum (§11.4): each build of a forum's model writes, with [`set_forum_blob_refs`], how many
//! of the forum's counted records reference each CID ([`crate::model::ForumModel::blob_refs`]),
//! once by CID and once by forum. A CID that loses its last reference in every forum is
//! returned, and the engine deletes its bytes (`threat-model.md` PRIV-2); eviction drops a
//! forum's references the same way ([`drop_forum_blob_refs`], §10.7). Every row is written
//! whole, never adjusted, so two processes updating the same forum converge.

use std::collections::BTreeMap;
use std::ops::Bound;

use serde::{Deserialize, Serialize};

use crate::host::{Dir, KeyRange, KvOp, KvStore};
use crate::store::key::{self, Part};
use crate::store::records::StoreError;
use crate::store::row::{self, ROW_VERSION};
use crate::store::schema::{self, tag};

/// The rows a reference scan reads at a time.
const REF_SCAN_PAGE: u32 = 1_000;
/// The ops of one reference write (even: a CID's two rows go in the same write).
const REF_WRITE_OPS: usize = 1_000;

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

/// `BlobRefRow`, at `("blob.ref", cid, fk)` and `("blob.forum", fk, cid)`: how many of forum
/// fk's counted records reference the CID (§11.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRefRow {
    /// Row version.
    pub v: u32,
    /// The forum's counted records that reference the CID (at least 1; a row is deleted at 0).
    pub n: u32,
}

/// The blob references of forum `fk` (§11.4): CID → how many of its counted records reference it.
pub async fn forum_blob_refs<K: KvStore>(
    kv: &K,
    fk: &str,
) -> Result<BTreeMap<String, u32>, StoreError> {
    let mut range = KeyRange::prefix(&schema::prefix(tag::BLOB_FORUM, &[fk]));
    let mut out = BTreeMap::new();
    loop {
        let page = kv.scan(range.clone(), Dir::Forward, REF_SCAN_PAGE).await?;
        for (k, v) in &page {
            let Ok(mut parts) = key::decode(k) else {
                continue;
            };
            let (Some(Part::Bytes(cid)), Ok(r)) = (parts.pop(), row::decode::<BlobRefRow>(v))
            else {
                continue;
            };
            if let Ok(cid) = String::from_utf8(cid)
                && r.v == ROW_VERSION
                && r.n > 0
            {
                out.insert(cid, r.n);
            }
        }
        match page.last() {
            Some((last, _)) if page.len() == REF_SCAN_PAGE as usize => {
                range.start = Bound::Excluded(last.clone());
            }
            _ => return Ok(out),
        }
    }
}

/// Replaces forum `fk`'s blob references with `refs` (CID → count; a count of 0 is no
/// reference), writing only the rows that change. Returns the CIDs the forum referenced before
/// and no forum references now: their stored bytes are to be deleted (§11.4, PRIV-2).
pub async fn set_forum_blob_refs<K: KvStore>(
    kv: &K,
    fk: &str,
    refs: &BTreeMap<String, u32>,
) -> Result<Vec<String>, StoreError> {
    let old = forum_blob_refs(kv, fk).await?;
    let mut ops = Vec::new();
    for (cid, &n) in refs {
        if n > 0 && old.get(cid) != Some(&n) {
            let r = row::encode(&BlobRefRow { v: ROW_VERSION, n })?;
            ops.push(KvOp::Put(schema::blob_ref_key(cid, fk), r.clone()));
            ops.push(KvOp::Put(schema::blob_forum_key(fk, cid), r));
        }
    }
    let lost: Vec<&String> = old
        .keys()
        .filter(|c| refs.get(*c).is_none_or(|n| *n == 0))
        .collect();
    for cid in &lost {
        ops.push(KvOp::Delete(schema::blob_ref_key(cid, fk)));
        ops.push(KvOp::Delete(schema::blob_forum_key(fk, cid)));
    }
    let mut ops = ops.into_iter().peekable();
    while ops.peek().is_some() {
        kv.write(ops.by_ref().take(REF_WRITE_OPS).collect())
            .await?;
    }
    let mut gone = Vec::new();
    for cid in lost {
        let others = KeyRange::prefix(&schema::prefix(tag::BLOB_REF, &[cid]));
        if kv.scan(others, Dir::Forward, 1).await?.is_empty() {
            gone.push(cid.clone());
        }
    }
    Ok(gone)
}

/// Drops every blob reference of forum `fk`, the blob step of evicting it (§10.7). Returns the
/// CIDs no forum references now, as [`set_forum_blob_refs`] does.
pub async fn drop_forum_blob_refs<K: KvStore>(
    kv: &K,
    fk: &str,
) -> Result<Vec<String>, StoreError> {
    set_forum_blob_refs(kv, fk, &BTreeMap::new()).await
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
    use crate::store::mem::MemKv;
    use crate::store::row;

    fn refs(of: &[(&str, u32)]) -> BTreeMap<String, u32> {
        of.iter().map(|(c, n)| ((*c).to_owned(), *n)).collect()
    }

    #[test]
    fn a_blob_is_gone_once_no_forum_references_it() {
        futures::executor::block_on(async {
            let kv = MemKv::new();
            let (f, g) = ("aaaaaaaaaaaa", "bbbbbbbbbbbb");
            let set = |fk: &'static str, r: BTreeMap<String, u32>| {
                let kv = &kv;
                async move { set_forum_blob_refs(kv, fk, &r).await.unwrap() }
            };
            assert!(set(f, refs(&[("x", 2), ("y", 1)])).await.is_empty());
            assert!(set(g, refs(&[("x", 1)])).await.is_empty());
            assert_eq!(forum_blob_refs(&kv, f).await.unwrap(), refs(&[("x", 2), ("y", 1)]));
            // the same references again write nothing
            let before = kv.scan(KeyRange::prefix(b""), Dir::Forward, 100).await.unwrap();
            assert!(set(f, refs(&[("x", 2), ("y", 1)])).await.is_empty());
            let after = kv.scan(KeyRange::prefix(b""), Dir::Forward, 100).await.unwrap();
            assert_eq!(before, after);
            // f stops referencing both: y is gone, x is still g's
            assert_eq!(set(f, refs(&[("x", 0)])).await, ["y"]);
            assert_eq!(forum_blob_refs(&kv, f).await.unwrap(), refs(&[]));
            // evicting g lets x go
            assert_eq!(drop_forum_blob_refs(&kv, g).await.unwrap(), ["x"]);
            let left = kv.scan(KeyRange::prefix(b""), Dir::Forward, 100).await.unwrap();
            assert!(left.is_empty(), "{left:?}");
        });
    }

    #[test]
    fn many_references_are_listed_and_dropped_in_pages() {
        futures::executor::block_on(async {
            let kv = MemKv::new();
            let fk = "aaaaaaaaaaaa";
            let many: BTreeMap<String, u32> =
                (0..2_500).map(|i| (format!("bafk{i:05}"), 1)).collect();
            assert!(set_forum_blob_refs(&kv, fk, &many).await.unwrap().is_empty());
            assert_eq!(forum_blob_refs(&kv, fk).await.unwrap(), many);
            let gone = drop_forum_blob_refs(&kv, fk).await.unwrap();
            assert_eq!(gone.len(), 2_500);
        });
    }

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
