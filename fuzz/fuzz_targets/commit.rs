// SPDX-License-Identifier: AGPL-3.0-or-later
//! Commit decoding and verification (`lessdugin_atp::commit`, `sync.md` §8.4) on raw commit
//! blocks.
//!
//! Contract: never panics. Beyond that, the unsigned bytes [`Commit::decode`] reconstructs are
//! exactly what a signer signs: a decoded commit re-signed over them with a known key verifies
//! whenever its `version`, `did` and `rev` do, so the signature check sees the bytes as signed
//! (field order, `prev` presence and unknown fields included).

#![no_main]

use lessdugin_atp::commit::{Commit, CommitError, VERSION};
use lessdugin_atp::dagcbor::validate_with;
use lessdugin_atp::datamodel::Limits;
use lessdugin_atp::syntax::Did;
use lessdugin_fuzz::common::NOW;
use lessdugin_fuzz::{FUZZ_DID, FUZZ_KEY, LIVE, sign};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(mut commit) = Commit::decode(data) else {
        return;
    };
    assert_eq!(
        validate_with(commit.unsigned_bytes(), &Limits::STRICT),
        Ok(())
    );
    let _ = commit.rev_tid();
    let _ = commit.verify(&FUZZ_DID, &FUZZ_KEY, NOW);
    let _ = commit.verify(&LIVE.0, &LIVE.3, NOW);
    let _ = commit.verify(&FUZZ_DID, &FUZZ_KEY, i64::MIN);
    let _ = commit.verify(&FUZZ_DID, &FUZZ_KEY, i64::MAX);

    // Re-sign the reconstructed unsigned bytes: verification must then pass the signature check.
    commit.sig = sign(commit.unsigned_bytes());
    let did = Did::parse(&commit.did).ok();
    let rev = commit.rev_tid();
    let now = rev.as_ref().map_or(NOW, |t| t.timestamp_micros() as i64);
    let result = commit.verify(did.as_ref().unwrap_or(&*FUZZ_DID), &FUZZ_KEY, now);
    assert!(
        !matches!(result, Err(CommitError::Signature(_))),
        "a commit re-signed over its unsigned bytes failed the signature check: {result:?}"
    );
    if commit.version == VERSION
        && let (Some(_), Some(rev)) = (did, rev)
    {
        assert_eq!(
            result,
            Ok(rev),
            "a well-formed re-signed commit must verify"
        );
    }
});
