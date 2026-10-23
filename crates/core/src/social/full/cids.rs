// SPDX-License-Identifier: AGPL-3.0-or-later
//! CID comparison (`social.md` §0.4): a CID written in a record (`subjectCid`, `subject.cid`,
//! `root.cid`, `parent.cid`, `head.cid`, `parents[].cid`) names a record of the fold input when
//! both have the same canonical string form (CIDv1, base32 lowercase, multibase prefix `b`).
//!
//! ```text
//! canonical(s) = the canonical string of s, parsed as a CIDv1 in any multibase; None otherwise
//!                (a CIDv0 `Qm…`, or a string that does not decode)
//! same(w, c)   ⇔ w = c ∨ canonical(w) = canonical(c) ≠ None
//! ```
//!
//! A record's G2 `cid` format check accepts other spellings of a CID (another multibase, upper
//! case), so a positive assertion whose `subjectCid` spells the current CID in base32 upper case
//! still counts. The fold input's own CIDs are canonical (the sync layer computes them), so the
//! common cases are decided without decoding: equal strings match, and two canonical strings
//! that differ before their last character cannot decode to the same bytes (base32 is
//! positional; only the last character carries padding bits).

use lessdugin_atp::Cid;

/// The canonical string form of a CIDv1 written in any multibase (`None`: not a CIDv1).
pub fn canonical(s: &str) -> Option<String> {
    let c = Cid::try_from(s).ok()?;
    (u64::from(c.version()) == 1).then(|| c.to_string())
}

/// `b` followed by lowercase base32 only: the canonical alphabet (not necessarily a CID).
fn canonical_shape(s: &str) -> bool {
    s.strip_prefix('b').is_some_and(|b| {
        !b.is_empty()
            && b.bytes()
                .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c))
    })
}

/// `w` and `c` name the same CID (§0.4): equal strings, or equal canonical forms.
pub fn same(w: &str, c: &str) -> bool {
    if w == c {
        return true;
    }
    if canonical_shape(w) && canonical_shape(c) {
        let (a, b) = (w.as_bytes(), c.as_bytes());
        // different lengths decode to different byte lengths; a difference before the last
        // character is a difference in the decoded bytes
        if a.len() != b.len() || a[..a.len() - 1] != b[..b.len() - 1] {
            return false;
        }
    }
    match (canonical(w), canonical(c)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The canonical form of `s` when it is a CIDv1, else `s` unchanged (it then names no record).
pub fn normalise(s: &str) -> String {
    if canonical_shape(s) {
        return s.to_owned();
    }
    canonical(s).unwrap_or_else(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "bafyreibox5tmitcjfmeurxwythqcq5cazsbifbkr56hu4752nd7erb6vdu";

    fn upper(s: &str) -> String {
        format!("B{}", s[1..].to_ascii_uppercase())
    }

    #[test]
    fn spellings_of_one_cid() {
        assert!(same(C, C));
        assert!(same(&upper(C), C));
        assert!(same(C, &upper(C)));
        assert_eq!(canonical(&upper(C)).as_deref(), Some(C));
        assert_eq!(normalise(&upper(C)), C);
        // base16 (multibase prefix `f`)
        let hex: String = Cid::try_from(C)
            .unwrap()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(same(&format!("f{hex}"), C));
        assert_eq!(normalise(&format!("f{hex}")), C);
    }

    #[test]
    fn different_or_malformed() {
        let other = "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifviyp5byhrz2w4bpbq";
        assert!(!same(other, C));
        // a canonical-looking string that differs only in its last character decodes to
        // different bytes here (or does not decode)
        let mut last = C.to_owned();
        last.pop();
        last.push('a');
        assert!(!same(&last, C));
        // the same data bits with non-zero padding bits: not a canonical encoding, and the
        // decoder rejects it
        let mut padded = C.to_owned();
        padded.pop();
        padded.push('v');
        assert_eq!(canonical(&padded), None);
        assert!(!same(&padded, C));
        assert!(!same("not-a-cid", C));
        assert!(!same("", C));
        assert!(!same("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG", C));
        assert_eq!(
            canonical("QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG"),
            None
        );
        assert_eq!(normalise("not-a-cid"), "not-a-cid");
    }
}
