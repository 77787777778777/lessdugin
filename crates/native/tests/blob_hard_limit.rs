// SPDX-License-Identifier: AGPL-3.0-or-later
//! `docs/plan.md` M4 acceptance: "a blob at the hard limit (300 MiB) served by FakePds is fetched
//! and CID-checked by the CLI without being held in memory" (`limits.md` §5
//! `BLOB_HARD_MAX_BYTES`, `threat-model.md` INFRA-6).
//!
//! FakePds serves the blob over real HTTP from a generator, a chunk at a time; the CLI's host
//! (`NativeHost`) streams it to a file while hashing it. A counting global allocator measures
//! the peak heap of the whole process - client and server - during the fetch, so a regression
//! that buffers the body on either side fails here. This binary holds this one test alone, so
//! nothing else allocates while it measures.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lessdugin_core::host::{BLOB_READ_MAX_BYTES, Host, HostError};
use lessdugin_core::limits::BLOB_HARD_MAX_BYTES;
use lessdugin_core::store::mem::MemKv;
use lessdugin_native::host::{NativeHost, Net, NetPolicy};
use lessdugin_testkit::FakeNet;
use lessdugin_testkit::fake_pds::{Generated, ManualClock};
use sha2::{Digest, Sha256};
use url::Url;

/// The system allocator, counting the bytes in use and their peak.
struct Counting;

static IN_USE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded to `System` unchanged; the counters only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is `System`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = IN_USE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) };
        IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                let grow = new_size - layout.size();
                let now = IN_USE.fetch_add(grow, Ordering::Relaxed) + grow;
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                IN_USE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// The heap the fetch may use on top of what was in use before it: buffers of a few chunks on
/// each side, far below the blob (a buffering implementation needs at least 300 MiB).
const PEAK_BUDGET: usize = 32 * 1024 * 1024;

#[test]
fn a_blob_at_the_hard_limit_is_streamed_to_disk_and_cid_checked() {
    let net = FakeNet::serve_http(Arc::new(ManualClock::new(1_700_000_000_000)));
    let alice = net.add_account("alice.test");
    let size = BLOB_HARD_MAX_BYTES;
    let cid = alice.serve_generated_blob(size, 0x5EED, "video/mp4").to_string();
    let pds = Url::parse(&alice.pds_base()).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let host = NativeHost::with_parts(
        MemKv::new(),
        MemKv::new(),
        tmp.path().join("blobs"),
        Net::new(NetPolicy::devnet([&pds])).unwrap(),
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let before = IN_USE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let handle = rt
        .block_on(host.fetch_blob(pds.clone(), alice.did().clone(), cid.clone(), size))
        .expect("the blob at the hard limit is fetched");
    let peak = PEAK.load(Ordering::Relaxed) - before;

    assert_eq!(handle.cid, cid);
    assert_eq!(handle.size, size);
    assert!(
        peak < PEAK_BUDGET,
        "the fetch used {peak} bytes of heap for a {size}-byte blob: it was buffered"
    );
    // the file is the blob: re-hash it from disk, and read it back through the core's window
    let mut file = std::fs::File::open(&handle.locator).unwrap();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut read = 0u64;
    loop {
        let n = std::io::Read::read(&mut file, &mut buf).unwrap();
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        read += n as u64;
    }
    assert_eq!(read, size);
    let digest: [u8; 32] = h.finalize().into();
    assert_eq!(
        lessdugin_atp::cid::from_sha256_digest(lessdugin_atp::cid::RAW, digest).to_string(),
        cid
    );
    let g = Generated {
        size,
        seed: 0x5EED,
    };
    let last = g.chunks() - 1;
    let tail = rt
        .block_on(host.blob_read(&cid, last * Generated::CHUNK as u64, BLOB_READ_MAX_BYTES))
        .unwrap();
    assert_eq!(tail, g.chunk(last));

    // the cap is the declared size: one byte less and the download is aborted, nothing kept
    rt.block_on(host.blob_delete(&cid)).unwrap();
    let short = rt.block_on(host.fetch_blob(pds, alice.did().clone(), cid.clone(), size - 1));
    assert_eq!(short.unwrap_err(), HostError::TooLarge);
    assert_eq!(rt.block_on(host.blob_open(&cid)).unwrap(), None);
}
