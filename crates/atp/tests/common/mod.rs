// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared helpers for integration tests.
//!
//! The interop tests also run on wasm32 under node (`cargo xtask test-wasm`), where there is no
//! filesystem: there the fixtures are embedded at compile time (`EMBEDDED`), natively they are
//! read from `testdata/interop`.
#![allow(dead_code)]

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

/// `testdata/interop/<rel>`.
#[cfg(not(target_arch = "wasm32"))]
pub fn interop(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/interop")
        .join(rel)
}

/// Reads a file under `testdata/interop`.
#[cfg(not(target_arch = "wasm32"))]
pub fn read(rel: &str) -> Vec<u8> {
    let p = interop(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

/// Reads a file under `testdata/interop` (the copy embedded at compile time).
#[cfg(target_arch = "wasm32")]
pub fn read(rel: &str) -> Vec<u8> {
    EMBEDDED
        .iter()
        .find(|(p, _)| *p == rel)
        .unwrap_or_else(|| panic!("testdata/interop/{rel} is not embedded (tests/common/mod.rs)"))
        .1
        .to_vec()
}

/// Every fixture the tests read, embedded for wasm32 (missing entries fail with a clear panic).
#[cfg(target_arch = "wasm32")]
macro_rules! embed {
    ($($rel:literal),* $(,)?) => {
        &[$(($rel, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/interop/", $rel)))),*]
    };
}

#[cfg(target_arch = "wasm32")]
static EMBEDDED: &[(&str, &[u8])] = embed![
    "crypto/signature-fixtures.json",
    "crypto/w3c_didkey_K256.json",
    "crypto/w3c_didkey_P256.json",
    "data-model/data-model-fixtures.json",
    "data-model/data-model-invalid.json",
    "data-model/data-model-valid.json",
    "firehose/commit-proof-fixtures.json",
    "live/bsky.app-did.json",
    "live/bsky.app-profile-absent.car",
    "live/bsky.app-profile-self.car",
    "live/bsky.app-profile-self.record.json",
    "mst/common_prefix.json",
    "mst/example_keys.txt",
    "mst/key_heights.json",
    "syntax/atidentifier_syntax_invalid.txt",
    "syntax/atidentifier_syntax_valid.txt",
    "syntax/aturi_syntax_invalid.txt",
    "syntax/aturi_syntax_valid.txt",
    "syntax/cid_syntax_invalid.txt",
    "syntax/cid_syntax_valid.txt",
    "syntax/datetime_parse_invalid.txt",
    "syntax/datetime_syntax_invalid.txt",
    "syntax/datetime_syntax_valid.txt",
    "syntax/did_syntax_invalid.txt",
    "syntax/did_syntax_valid.txt",
    "syntax/handle_syntax_invalid.txt",
    "syntax/handle_syntax_valid.txt",
    "syntax/nsid_syntax_invalid.txt",
    "syntax/nsid_syntax_valid.txt",
    "syntax/recordkey_syntax_invalid.txt",
    "syntax/recordkey_syntax_valid.txt",
    "syntax/tid_syntax_invalid.txt",
    "syntax/tid_syntax_valid.txt",
];

/// Reads a JSON file under `testdata/interop`.
pub fn json(rel: &str) -> serde_json::Value {
    serde_json::from_slice(&read(rel)).unwrap()
}

/// Test lines of an interop `.txt` list: skips blank lines and `#` comments, keeps whitespace.
pub fn lines(rel: &str) -> Vec<String> {
    let text = String::from_utf8(read(rel)).unwrap();
    text.split('\n')
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Decodes standard base64 with or without padding.
pub fn b64(s: &str) -> Vec<u8> {
    lessdugin_atp::datamodel::decode_base64(s).unwrap()
}
