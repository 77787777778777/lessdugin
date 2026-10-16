// SPDX-License-Identifier: MIT OR Apache-2.0
//! Normative (class N) limits of record validity and fold input (`data-model.md` §7.2, §7.4;
//! `limits.md` §4, §6). Field limits (string lengths, array sizes, blob `maxSize`) are not
//! repeated here: the validator reads them from the embedded lexicons.

use crate::collection::Collection;

/// Maximum DAG-CBOR size of a record value, in bytes (rule G6, `RECORD_CBOR_MAX_BYTES`).
pub const RECORD_CBOR_MAX_BYTES: usize = 1_000_000;

/// Maximum nesting depth of a record value; the root object is depth 1 (rule G10,
/// `NESTING_MAX_RECORD`).
pub const NESTING_MAX_RECORD: usize = 32;

/// Maximum length of a slug (`data-model.md` §2.1, `KNOWN_VALUE_MAX_BYTES`).
pub const SLUG_MAX_BYTES: usize = 64;

/// Normative fetch cap for the `opinion` collection: the first N records of each
/// `(repo, collection, fk)` range in **descending** rkey byte order (§7.4).
pub const FETCH_CAP_OPINION: usize = 100_000;

/// Normative fetch cap for the other ranged, forum-scoped collections (§7.4).
pub const FETCH_CAP_DEFAULT: usize = 20_000;

/// The §7.4 fold-input cap of a collection's `{fk}-` range, or `None` for the collections that
/// are not ranged (`forum`, `profile`) or hold a single record per forum (`membership`).
pub const fn fetch_cap(collection: Collection) -> Option<usize> {
    match collection {
        Collection::Opinion => Some(FETCH_CAP_OPINION),
        Collection::Post
        | Collection::Comment
        | Collection::Revision
        | Collection::ModAction
        | Collection::WikiRevision
        | Collection::Sequence => Some(FETCH_CAP_DEFAULT),
        Collection::Membership | Collection::Forum | Collection::Profile => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps() {
        assert_eq!(fetch_cap(Collection::Opinion), Some(100_000));
        for c in [
            Collection::Post,
            Collection::Comment,
            Collection::Revision,
            Collection::ModAction,
            Collection::WikiRevision,
            Collection::Sequence,
        ] {
            assert_eq!(fetch_cap(c), Some(20_000), "{c}");
        }
        for c in [
            Collection::Membership,
            Collection::Forum,
            Collection::Profile,
        ] {
            assert_eq!(fetch_cap(c), None, "{c}");
        }
    }
}
