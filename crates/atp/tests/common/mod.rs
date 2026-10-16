// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared helpers for integration tests.
#![allow(dead_code)]

use std::path::PathBuf;

/// `testdata/interop/<rel>`.
pub fn interop(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/interop")
        .join(rel)
}

/// Reads a file under `testdata/interop`.
pub fn read(rel: &str) -> Vec<u8> {
    let p = interop(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

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
