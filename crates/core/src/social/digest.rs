// SPDX-License-Identifier: AGPL-3.0-or-later
//! `inputDigest` (`social.md` §1.2): the SHA-256 of the sorted `(uri, cid)` lines of `records`
//! and `absent`, the declaration CID, the effective behaviour IDs and the `PARAMS` values.
//!
//! ```text
//! lines = { u ‖ 0x00 ‖ cid ‖ 0x0A : records } ∪ { u ‖ 0x00 ‖ "-" ‖ 0x0A : absent }
//! D     = concat(sorted lines) ‖ "decl" 0x00 declCid 0x0A ‖ "algs" 0x00 algs 0x0A
//!         ‖ "params" 0x00 params 0x0A
//! inputDigest = lowercase hex of sha256(D)
//! ```

use lessdugin_lexicon::records::Forum;

use crate::social::input::FoldInput;
use crate::social::policy::EffectivePolicy;

/// The preimage D of `input`'s digest.
pub fn preimage(input: &FoldInput) -> Result<Vec<u8>, serde_json::Error> {
    let decl: Forum = serde_json::from_value(input.decl.value.clone())?;
    let policy = EffectivePolicy::of(&decl);
    let mut lines: Vec<Vec<u8>> = input
        .records
        .iter()
        .map(|(u, r)| line(u, &r.cid))
        .chain(input.absent.iter().map(|u| line(u, "-")))
        .collect();
    lines.sort();
    let mut d: Vec<u8> = lines.concat();
    for (tag, body) in [
        ("decl", input.decl.cid.clone()),
        ("algs", policy.algs_line()),
        ("params", policy.params_line()),
    ] {
        d.extend_from_slice(&line(tag, &body));
    }
    Ok(d)
}

fn line(a: &str, b: &str) -> Vec<u8> {
    let mut l = Vec::with_capacity(a.len() + b.len() + 2);
    l.extend_from_slice(a.as_bytes());
    l.push(0);
    l.extend_from_slice(b.as_bytes());
    l.push(b'\n');
    l
}

/// `inputDigest`: 64 lowercase hex digits.
pub fn input_digest(input: &FoldInput) -> Result<String, serde_json::Error> {
    let h = lessdugin_atp::cid::sha256(&preimage(input)?);
    Ok(h.iter().map(|b| format!("{b:02x}")).collect())
}
