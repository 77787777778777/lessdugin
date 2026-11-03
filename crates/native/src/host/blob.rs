// SPDX-License-Identifier: AGPL-3.0-or-later
//! Verified blob storage under `<cache>/blobs/<cid[-2..]>/<cid>` (`sync.md` §11.2, §12.2;
//! `threat-model.md` §5.6.1).
//!
//! `getBlob` is streamed into a 0600 temporary file in the blob directory while feeding
//! SHA-256; the file is renamed to its final name only when the digest equals the CID's.
//! On any failure the temporary file is removed and nothing is stored.
//!
//! One download runs per (PDS, DID, CID) at a time (`sync.md` §12.2): it is a detached task,
//! capped by `BLOB_TOTAL_TIMEOUT_MS`, that every `fetch` of that author's copy awaits, so a
//! caller that stops waiting (the engine's `BODY_BLOB_WAIT_MS` for a spilled body) neither
//! cancels it nor starts another, and the stored copy is there for the next caller. A fetch of
//! the same CID from another author or PDS does not join it: that copy may fail where this one
//! succeeds (review m3m4-continuation-1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::{BoxFuture, Shared};

use lessdugin_atp::cid as atp_cid;
use lessdugin_atp::syntax::Did;
use lessdugin_core::host::{BlobHandle, HostError};
use lessdugin_core::limits;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use url::Url;

use super::net::{Net, NetRequest, read_capped};
use crate::fsutil;

/// Largest read through [`BlobStore::read`] (1 MiB, `sync.md` §12.2).
pub const BLOB_READ_MAX: u32 = 1_048_576;
/// Cap on the body of an error response from `getBlob`.
const ERROR_BODY_MAX: u64 = 64 * 1024;

/// A download in flight, as every `fetch` of its copy awaits it: the detached task's result.
type Download = Shared<BoxFuture<'static, Result<BlobHandle, HostError>>>;

/// The copy a download fetches: (PDS, DID, CID).
type CopyKey = (String, String, String);

/// The blob directory, and the downloads in flight (shared by every clone).
#[derive(Clone)]
pub struct BlobStore {
    dir: PathBuf,
    inflight: Arc<Mutex<HashMap<CopyKey, Download>>>,
}

impl std::fmt::Debug for BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// Parses and checks a blob CID string: canonical CIDv1, codec raw, sha2-256. The canonical
/// form is lowercase base32, so the string is also safe as a file name.
fn parse_blob_cid(cid: &str) -> Result<lessdugin_atp::Cid, HostError> {
    let parsed = atp_cid::parse(cid)
        .map_err(|e| HostError::Unsupported(format!("blob CID {cid:?}: {e}")))?;
    if parsed.codec() != atp_cid::RAW
        || parsed.hash().code() != atp_cid::SHA2_256
        || parsed.hash().size() != 32
    {
        return Err(HostError::Unsupported(
            "blob CID must be CIDv1 with codec raw and sha2-256".into(),
        ));
    }
    Ok(parsed)
}

fn io(e: std::io::Error) -> HostError {
    HostError::Storage(e.to_string())
}

/// Removes the temporary file on drop unless it was renamed into place.
struct TempFile {
    path: PathBuf,
    keep: bool,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl BlobStore {
    /// A store rooted at `dir` (created on first use, mode 0700).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The blob directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where a (checked) CID is stored: `<dir>/<last two characters>/<cid>`.
    fn path_for(&self, cid: &str) -> PathBuf {
        let shard = &cid[cid.len().saturating_sub(2)..];
        self.dir.join(shard).join(cid)
    }

    /// The handle of a stored blob, if present.
    pub async fn open(&self, cid: &str) -> Result<Option<BlobHandle>, HostError> {
        parse_blob_cid(cid)?;
        let path = self.path_for(cid);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_file() => Ok(Some(BlobHandle {
                cid: cid.to_owned(),
                size: meta.len(),
                locator: path.to_string_lossy().into_owned(),
            })),
            Ok(_) => Err(HostError::Storage(format!(
                "{} is not a regular file",
                path.display()
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    /// Reads at most `len` (≤ 1 MiB) bytes of a stored blob from `offset`; fewer at the end.
    pub async fn read(&self, cid: &str, offset: u64, len: u32) -> Result<Vec<u8>, HostError> {
        parse_blob_cid(cid)?;
        let len = len.min(BLOB_READ_MAX) as usize;
        let mut file = tokio::fs::File::open(self.path_for(cid))
            .await
            .map_err(io)?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(io)?;
        let mut buf = Vec::with_capacity(len);
        file.take(len as u64)
            .read_to_end(&mut buf)
            .await
            .map_err(io)?;
        Ok(buf)
    }

    /// Deletes a stored blob (no error if absent).
    pub async fn delete(&self, cid: &str) -> Result<(), HostError> {
        parse_blob_cid(cid)?;
        match tokio::fs::remove_file(self.path_for(cid)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }

    /// `Host::fetch_blob`: download, verify and store, or return the stored copy. A copy (PDS,
    /// DID, CID) being downloaded is awaited, not fetched again; the download is a detached task
    /// (see the module docs), so dropping this future does not stop it. The first caller's
    /// `max_bytes` caps the download; the core holds a stored copy to its own declaration.
    pub async fn fetch(
        &self,
        net: &Net,
        pds: &Url,
        did: &Did,
        cid: &str,
        max_bytes: u64,
    ) -> Result<BlobHandle, HostError> {
        let parsed = parse_blob_cid(cid)?;
        if let Some(handle) = self.open(cid).await? {
            return Ok(handle);
        }
        let key: CopyKey = (pds.to_string(), did.to_string(), cid.to_owned());
        let download = {
            let mut inflight = self.inflight.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(d) = inflight.get(&key) {
                d.clone()
            } else {
                let max = max_bytes.min(limits::BLOB_HARD_MAX_BYTES);
                let mut url = pds
                    .join("/xrpc/com.atproto.sync.getBlob")
                    .map_err(|_| HostError::Blocked("invalid PDS URL".into()))?;
                url.query_pairs_mut()
                    .append_pair("did", did.as_str())
                    .append_pair("cid", cid);
                let req = NetRequest {
                    max_bytes: max,
                    timeout_ms: limits::BLOB_TOTAL_TIMEOUT_MS,
                    ..NetRequest::get(url)
                };
                let total = Duration::from_millis(u64::from(limits::BLOB_TOTAL_TIMEOUT_MS));
                let (store, net, cid_owned, key_owned) =
                    (self.clone(), net.clone(), cid.to_owned(), key.clone());
                let digest = parsed.hash().digest().to_vec();
                let task = tokio::spawn(async move {
                    let r = tokio::time::timeout(
                        total,
                        store.download(&net, req, &digest, &cid_owned, max),
                    )
                    .await
                    .unwrap_or(Err(HostError::Timeout));
                    // stored (or failed) before the entry goes: a caller that misses it
                    // finds the file
                    store
                        .inflight
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&key_owned);
                    r
                });
                let d: Download = async move {
                    task.await.unwrap_or_else(|e| {
                        Err(HostError::Other(format!("the blob download ended: {e}")))
                    })
                }
                .boxed()
                .shared();
                inflight.insert(key, d.clone());
                d
            }
        };
        download.await
    }

    async fn download(
        &self,
        net: &Net,
        req: NetRequest,
        digest: &[u8],
        cid: &str,
        max: u64,
    ) -> Result<BlobHandle, HostError> {
        let mut resp = net.open(req).await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let error = read_capped(resp, ERROR_BODY_MAX).await.ok().and_then(|r| {
                let v: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
                v.get("error")?.as_str().map(str::to_owned)
            });
            return Err(HostError::HttpStatus(status, error));
        }

        let tmp_dir = self.dir.join(".tmp");
        fsutil::ensure_private_dir(&tmp_dir).map_err(io)?;
        let mut rnd = [0u8; 12];
        getrandom::fill(&mut rnd).map_err(|e| HostError::Other(e.to_string()))?;
        let name: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
        let mut tmp = TempFile {
            path: tmp_dir.join(format!("{name}.part")),
            keep: false,
        };
        let std_file = fsutil::create_new_private(&tmp.path).map_err(io)?;
        let mut file = tokio::fs::File::from_std(std_file);

        let idle = Duration::from_millis(u64::from(limits::BLOB_IDLE_TIMEOUT_MS));
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        loop {
            let chunk = tokio::time::timeout(idle, resp.chunk())
                .await
                .map_err(|_| HostError::Timeout)?
                .map_err(super::net::map_reqwest_error)?;
            let Some(chunk) = chunk else { break };
            size += chunk.len() as u64;
            if size > max {
                return Err(HostError::TooLarge);
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(io)?;
        }
        if hasher.finalize()[..] != digest[..] {
            tracing::warn!(cid, "blob does not match its CID");
            return Err(HostError::CidMismatch);
        }
        file.flush().await.map_err(io)?;
        file.sync_all().await.map_err(io)?;
        drop(file);

        let dest = self.path_for(cid);
        if let Some(shard) = dest.parent() {
            fsutil::ensure_private_dir(shard).map_err(io)?;
        }
        tokio::fs::rename(&tmp.path, &dest).await.map_err(io)?;
        tmp.keep = true;
        Ok(BlobHandle {
            cid: cid.to_owned(),
            size,
            locator: dest.to_string_lossy().into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_cids_are_checked() {
        let raw = atp_cid::for_raw(b"hello").to_string();
        assert!(parse_blob_cid(&raw).is_ok());
        let dag = atp_cid::for_dag_cbor(b"\xa0").to_string();
        assert!(matches!(
            parse_blob_cid(&dag),
            Err(HostError::Unsupported(_))
        ));
        for bad in [
            "",
            "../../etc/passwd",
            "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG",
            "bafy/x",
        ] {
            assert!(parse_blob_cid(bad).is_err(), "{bad}");
        }
        let store = BlobStore::new("/c/blobs");
        let p = store.path_for(&raw);
        assert_eq!(
            p.parent().unwrap().file_name().unwrap().to_str().unwrap(),
            &raw[raw.len() - 2..]
        );
    }

    /// A local `getBlob` that trickles `bytes` in `chunks` pieces, `gap` apart, counting hits.
    async fn trickling_pds(
        bytes: Vec<u8>,
        chunks: usize,
        gap: Duration,
    ) -> (Url, Arc<std::sync::atomic::AtomicUsize>) {
        use axum::body::{Body, Bytes};
        use axum::routing::get;
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = Arc::clone(&hits);
        let app = axum::Router::new().route(
            "/xrpc/com.atproto.sync.getBlob",
            get(move || {
                let (bytes, served) = (bytes.clone(), Arc::clone(&served));
                async move {
                    served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let step = bytes.len().div_ceil(chunks);
                    let stream = futures::stream::unfold(0usize, move |at| {
                        let bytes = bytes.clone();
                        async move {
                            if at >= bytes.len() {
                                return None;
                            }
                            tokio::time::sleep(gap).await;
                            let end = (at + step).min(bytes.len());
                            let chunk = Bytes::copy_from_slice(&bytes[at..end]);
                            Some((Ok::<_, std::io::Error>(chunk), end))
                        }
                    });
                    Body::from_stream(stream)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = Url::parse(&format!("http://127.0.0.1:{}", addr.port())).unwrap();
        (base, hits)
    }

    #[tokio::test]
    async fn a_download_outlives_a_caller_that_stops_waiting_and_runs_once_per_cid() {
        // review robustness-4: `fetch` was a plain future, so a caller that stopped waiting
        // (the engine after BODY_BLOB_WAIT_MS) aborted the download and deleted the part file,
        // and the next caller started it again: a body slower than the wait never arrived
        use super::super::guard::{FixedDns, NetPolicy};
        let bytes: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let cid = atp_cid::for_raw(&bytes).to_string();
        let (base, hits) = trickling_pds(bytes.clone(), 5, Duration::from_millis(80)).await;
        let net = Net::with_dns(NetPolicy::devnet([&base]), Arc::new(FixedDns::new())).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().join("blobs"));
        let did = Did::parse("did:plc:alice").unwrap();
        let wait = Duration::from_millis(100);

        // the caller stops waiting: the download runs on, and a later caller joins it
        assert!(
            tokio::time::timeout(wait, store.fetch(&net, &base, &did, &cid, 1 << 20))
                .await
                .is_err()
        );
        assert_eq!(store.inflight.lock().unwrap().len(), 1);
        let h = store.fetch(&net, &base, &did, &cid, 1 << 20).await.unwrap();
        assert_eq!(h.size, bytes.len() as u64);
        assert_eq!(std::fs::read(&h.locator).unwrap(), bytes);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(store.inflight.lock().unwrap().is_empty());

        // with no later caller the download still completes and is stored
        let other: Vec<u8> = (0..20_000u32).map(|i| (i % 241) as u8).collect();
        let cid2 = atp_cid::for_raw(&other).to_string();
        let (base2, hits2) = trickling_pds(other.clone(), 5, Duration::from_millis(80)).await;
        let net2 = Net::with_dns(NetPolicy::devnet([&base2]), Arc::new(FixedDns::new())).unwrap();
        assert!(
            tokio::time::timeout(wait, store.fetch(&net2, &base2, &did, &cid2, 1 << 20))
                .await
                .is_err()
        );
        let mut stored = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stored = store.open(&cid2).await.unwrap();
            if stored.is_some() {
                break;
            }
        }
        let stored = stored.expect("the detached download stored the blob");
        assert_eq!(std::fs::read(&stored.locator).unwrap(), other);
        assert_eq!(hits2.load(std::sync::atomic::Ordering::SeqCst), 1);
        // and a fetch now is the stored copy, without a request
        assert_eq!(
            store.fetch(&net2, &base2, &did, &cid2, 10).await.unwrap(),
            stored
        );
        assert_eq!(hits2.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(store.inflight.lock().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(store.dir().join(".tmp")).unwrap().count(),
            0,
            "no temporary files left"
        );
    }

    /// A local `getBlob` that answers HTTP 400 after `delay`, counting hits.
    async fn failing_pds(delay: Duration) -> (Url, Arc<std::sync::atomic::AtomicUsize>) {
        use axum::routing::get;
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = Arc::clone(&hits);
        let app = axum::Router::new().route(
            "/xrpc/com.atproto.sync.getBlob",
            get(move || {
                let served = Arc::clone(&served);
                async move {
                    served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        r#"{"error":"InvalidRequest","message":"Blob not found"}"#,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = Url::parse(&format!("http://127.0.0.1:{}", addr.port())).unwrap();
        (base, hits)
    }

    #[tokio::test]
    async fn a_fetch_of_another_authors_copy_does_not_join_a_failing_download() {
        // review m3m4-continuation-1: downloads were joined by CID alone, so a fetch of bob's
        // body from bob's PDS joined mallory's download of the same CID from hers and got her
        // failure (HTTP 400, a CID mismatch or a stall)
        use super::super::guard::{FixedDns, NetPolicy};
        let bytes: Vec<u8> = (0..2_000u32).map(|i| (i % 251) as u8).collect();
        let cid = atp_cid::for_raw(&bytes).to_string();
        let (bob_pds, bob_hits) = trickling_pds(bytes.clone(), 1, Duration::from_millis(1)).await;
        let (mallory_pds, mallory_hits) = failing_pds(Duration::from_millis(300)).await;
        let net = Net::with_dns(
            NetPolicy::devnet([&bob_pds, &mallory_pds]),
            Arc::new(FixedDns::new()),
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = BlobStore::new(tmp.path().join("blobs"));
        let (bob, mallory) = (
            Did::parse("did:plc:bob").unwrap(),
            Did::parse("did:plc:mallory").unwrap(),
        );
        let failing = {
            let (store, net, pds, did, cid) = (
                store.clone(),
                net.clone(),
                mallory_pds.clone(),
                mallory.clone(),
                cid.clone(),
            );
            tokio::spawn(async move { store.fetch(&net, &pds, &did, &cid, 1 << 20).await })
        };
        // mallory's download is in flight
        for _ in 0..50 {
            if mallory_hits.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(store.inflight.lock().unwrap().len(), 1);
        let h = store
            .fetch(&net, &bob_pds, &bob, &cid, 1 << 20)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&h.locator).unwrap(), bytes);
        assert_eq!(bob_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(
            failing.await.unwrap(),
            Err(HostError::HttpStatus(400, _))
        ));
        assert_eq!(mallory_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(store.inflight.lock().unwrap().is_empty());
    }
}
