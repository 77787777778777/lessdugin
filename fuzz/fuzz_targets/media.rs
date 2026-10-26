// SPDX-License-Identifier: AGPL-3.0-or-later
//! The media pipeline's container walks (`lessdugin_core::media`, `threat-model.md` §5.9.1).
//!
//! Fuzzing matters here because every walker parses attacker-supplied framing - segment lengths,
//! chunk sizes, box sizes, sub-block chains - and a walk that loops, allocates without bound or
//! indexes out of range would hang or crash a client on a hostile attachment.
//!
//! Contract: never panics, never hangs, and
//! - the output of a successful strip is a FIXED POINT: stripping it again changes nothing, so a
//!   file is never degraded further by being re-prepared;
//! - `sniff` agrees with the format the bytes actually are (a strip never turns one format into
//!   another that sniffs differently);
//! - MP4 output is the same LENGTH as its input (`udta`/`meta` are overwritten in place, so no
//!   offset may ever shift - `data-model.md` §8);
//! - the result is never longer than the input plus a bounded allowance, so a small hostile input
//!   cannot expand into a large allocation;
//! - a container the walk cannot parse comes back byte-identical (the pipeline does not destroy
//!   what it does not understand).

#![no_main]

use lessdugin_core::media::{Format, MAX_SCAN_BYTES, sniff, strip_metadata};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_SCAN_BYTES {
        return;
    }
    let first = strip_metadata(data);
    let format = sniff(data);

    // The result is never wildly larger than the input: a rebuild only drops.
    assert!(
        first.bytes.len() <= data.len(),
        "the pipeline grew {} bytes into {}",
        data.len(),
        first.bytes.len()
    );

    if !first.changed {
        // Unparseable or nothing to drop: the bytes must come back EXACTLY as they came in.
        assert_eq!(first.bytes, data, "an unchanged strip must be byte-identical");
        return;
    }

    // MP4 is rewritten in place: the length must not move a single offset.
    if format == Some(Format::Iso) {
        assert_eq!(
            first.bytes.len(),
            data.len(),
            "an MP4 strip must not change the file's length"
        );
    }

    // A successful strip must not change what the file IS.
    assert_eq!(
        sniff(&first.bytes),
        format,
        "stripping changed the sniffed format"
    );

    // Idempotent: stripping the stripped output finds nothing left to drop.
    let second = strip_metadata(&first.bytes);
    assert!(
        !second.changed,
        "a second strip found something to drop ({} -> {})",
        first.bytes.len(),
        second.bytes.len()
    );
    assert_eq!(
        second.bytes, first.bytes,
        "a second strip must be byte-identical"
    );
});