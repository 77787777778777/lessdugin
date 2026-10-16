// SPDX-License-Identifier: AGPL-3.0-or-later
//! Verified blob storage under `<cache>/blobs/<cid[-2..]>/<cid>` (`sync.md` §11.2, §12.2;
//! `threat-model.md` §5.6.1).
//!
//! `getBlob` is streamed into a 0600 temporary file in the blob directory while feeding
//! SHA-256; the file is renamed to its final name only when the digest equals the CID's.
//! On any failure the temporary file is removed and nothing is stored.

use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// The blob directory.
#[derive(Clone, Debug)]
pub struct BlobStore {
    dir: PathBuf,
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
        Self { dir: dir.into() }
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

    /// `Host::fetch_blob`: download, verify and store, or return the stored copy.
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
        tokio::time::timeout(
            total,
            self.download(net, req, parsed.hash().digest(), cid, max),
        )
        .await
        .map_err(|_| HostError::Timeout)?
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
}
