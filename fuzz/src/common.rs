// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fixed parameters shared by the fuzz targets and the seed generator (`seeds/`, which includes
//! this file by path), so that generated seeds verify under the parameters the targets use.

/// "Now" for every verification (µs since the Unix epoch, 2030-03-17): later than every commit in
/// the seeds, including the captured live proof.
pub const NOW: i64 = 1_900_000_000_000_000;

/// The commit `rev` time of generated repos (µs, 2025-10-09).
pub const REV_MICROS: u64 = 1_760_000_000_000_000;

/// The repo DID of generated repos.
pub const FUZZ_DID: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

/// Another valid DID (commit `did` mismatches).
pub const OTHER_DID: &str = "did:plc:abcdefghijklmnopqrstuvwx";

/// The secp256k1 secret key that signs generated commits.
pub const FUZZ_SECRET: [u8; 32] = [42u8; 32];

/// The captured `getRecord` proof in `testdata/interop/live` (repo, record and `#atproto` key).
pub const LIVE_DID: &str = "did:plc:z72i7hdynmk6r22z27h6tvur";
/// Collection of the live record.
pub const LIVE_COLLECTION: &str = "app.bsky.actor.profile";
/// Record key of the live record.
pub const LIVE_RKEY: &str = "self";
/// The live repo's `#atproto` signing key (Multikey).
pub const LIVE_KEY: &str = "zQ3shQo6TF2moaqMTrUZEM1jeuYRQXeHEx4evX9751y2qPqRA";

/// Collections of generated repo keys (valid NSIDs).
pub const COLLECTIONS: &[&str] = &[
    "app.bsky.feed.post",
    "online.7778777.lessdugin.temp.post",
    "online.7778777.lessdugin.temp.comment",
    "app.bsky.actor.profile",
    "a.b.c",
];

/// The record-key alphabet (`[A-Za-z0-9._:~-]`).
pub const RKEY_ALPHABET: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:~-";

/// Maps fuzz bytes to a record key of 1–64 characters that is never `.` or `..`.
pub fn rkey_from(bytes: &[u8]) -> String {
    let mut s: String = bytes
        .iter()
        .take(64)
        .map(|b| RKEY_ALPHABET[usize::from(*b) % RKEY_ALPHABET.len()] as char)
        .collect();
    if s.is_empty() || s == "." || s == ".." {
        s.push('k');
    }
    s
}
