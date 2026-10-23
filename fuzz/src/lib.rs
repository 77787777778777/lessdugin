// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared helpers of the lessdugin fuzz targets (see `README.md`).
//!
//! Every target is panic-free by contract: errors are fine; a panic, a stack overflow, a timeout
//! or an allocation above the limits `cargo xtask fuzz` runs with is a bug in the crate under test
//! (fixed there, with a regression test, and the minimized input kept in `regressions/`).

pub mod common;

use std::sync::LazyLock;

use lessdugin_atp::crypto::PublicKey;
use lessdugin_atp::syntax::{Did, Nsid, RecordKey};

/// The stack the parsers and the renderer must fit in: wasm32's default 1 MiB (the props tests
/// check the same bound for the deep-nesting cases, lfm-1.md §8.4).
pub const STACK_BYTES: usize = 1 << 20;

/// Runs `f` on a thread with a [`STACK_BYTES`] stack. A panic there aborts the process through
/// libFuzzer's panic hook, so the crash is attributed to the current input.
pub fn on_small_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn_scoped(s, f)
            .expect("spawn the fuzz thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e))
    })
}

/// The signing key of generated repos ([`common::FUZZ_SECRET`]).
pub static SIGNING_KEY: LazyLock<k256::ecdsa::SigningKey> = LazyLock::new(|| {
    k256::ecdsa::SigningKey::from_slice(&common::FUZZ_SECRET).expect("fixed secret")
});

/// The public key of [`SIGNING_KEY`].
pub static FUZZ_KEY: LazyLock<PublicKey> =
    LazyLock::new(|| PublicKey::Secp256k1(*SIGNING_KEY.verifying_key()));

/// The DID of generated repos.
pub static FUZZ_DID: LazyLock<Did> =
    LazyLock::new(|| Did::parse(common::FUZZ_DID).expect("fixed DID"));

/// The live proof's parameters: DID, collection, record key and `#atproto` key.
pub static LIVE: LazyLock<(Did, Nsid, RecordKey, PublicKey)> = LazyLock::new(|| {
    (
        Did::parse(common::LIVE_DID).expect("live DID"),
        Nsid::parse(common::LIVE_COLLECTION).expect("live collection"),
        RecordKey::parse(common::LIVE_RKEY).expect("live rkey"),
        PublicKey::from_multikey(common::LIVE_KEY).expect("live key"),
    )
});

/// Signs `msg` with [`SIGNING_KEY`] (64-byte compact, low-S).
pub fn sign(msg: &[u8]) -> Vec<u8> {
    use k256::ecdsa::signature::Signer as _;
    let s: k256::ecdsa::Signature = SIGNING_KEY.sign(msg);
    s.to_bytes().to_vec()
}

/// A cheap deterministic hash (FNV-1a) for picking variants from an input.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Characters a terminal must never receive from content (lfm-1.md §9.1): C0, DEL, C1,
/// U+2028/U+2029 and the bidi controls.
pub fn is_terminal_forbidden(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}' | '\u{2028}' | '\u{2029}' | '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}
