// SPDX-License-Identifier: AGPL-3.0-or-later
//! L0: authorities and ranks (`mod/1`, `social.md` §4.1), and the declaration banner (§1.6).
//!
//! ```text
//! owner = did(F) rank 3;  admins rank 2;  mods rank 1        // decl.moderators read per FO2
//! A     = {owner} ∪ admins ∪ mods;   rank(d) = 0 for d ∉ A
//! subjectAuthor(T) = T (DID) | did(T) (at-uri) | none ("tag:" slug);  subjectRank = rank of it, or 0
//! banner ⇔ an effective-policy fallback (nine fields) ∨ an unknown moderators[].role in any entry
//! ```

use std::collections::BTreeMap;

use lessdugin_atp::syntax::Did;
use lessdugin_lexicon::values::Role;

use crate::discovery::roster;
use crate::social::FoldError;
use crate::social::full::cx::Cx;
use crate::social::input::split_uri;

/// A with ranks (§4.1): the owner (3) and the effective `decl.moderators` entries (FO2).
pub fn authorities(cx: &Cx<'_>) -> Result<BTreeMap<String, u8>, FoldError> {
    let owner = Did::parse(cx.owner()).map_err(|e| FoldError(e.to_string()))?;
    Ok(roster::authorities(&owner, &cx.decl))
}

/// `rank(d)`: 3, 2 or 1 for d ∈ A, else 0.
pub fn rank(a: &BTreeMap<String, u8>, did: &str) -> u8 {
    a.get(did).copied().unwrap_or(0)
}

/// `subjectAuthor(T)`: the DID itself, `did(T)` for an at-uri, `None` for a `tag:` subject.
pub fn subject_author(subject: &str) -> Option<&str> {
    if subject.starts_with("did:") {
        Some(subject)
    } else if subject.starts_with("at://") {
        split_uri(subject).map(|(d, _, _)| d)
    } else {
        None
    }
}

/// `subjectRank(T)` (`tag:` subjects rank 0).
pub fn subject_rank(a: &BTreeMap<String, u8>, subject: &str) -> u8 {
    subject_author(subject).map_or(0, |d| rank(a, d))
}

/// The "this forum uses rules newer than your client" banner (§1.6): one of the ten
/// knownValue fields of the declaration fell back. `moderators[].role` counts in any entry,
/// including one FO2 ignores for another reason.
pub fn newer_rules_banner(cx: &Cx<'_>) -> bool {
    cx.policy.newer_rules
        || cx
            .decl
            .moderators
            .iter()
            .flatten()
            .any(|m| Role::parse(&m.role).is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects() {
        let a = BTreeMap::from([("did:plc:o".to_owned(), 3u8), ("did:plc:m".to_owned(), 1)]);
        assert_eq!(subject_rank(&a, "did:plc:m"), 1);
        assert_eq!(subject_rank(&a, "at://did:plc:o/x.post/k"), 3);
        assert_eq!(subject_rank(&a, "tag:ai"), 0);
        assert_eq!(subject_author("tag:ai"), None);
        assert_eq!(rank(&a, "did:plc:z"), 0);
    }
}
