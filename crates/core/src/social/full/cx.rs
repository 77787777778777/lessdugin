// SPDX-License-Identifier: AGPL-3.0-or-later
//! The prepared fold input ([`Cx`]): the contract form ([`FoldInput`], full values), its
//! skeleton form ([`ForumInput`]: validity, times, references), the typed declaration, the
//! effective policy (§1.6) and rule V1 (§10.1), which every layer reads.
//!
//! ```text
//! well-formed (§1.1): decl valid; records = memberships with rkey fk ∪ ranged collections in the
//!                     "{fk}-" range; absent ∩ records = ∅; every absent uri is the subject of an
//!                     opinion in records
//! V1 excluded(x)    ⇔ x invalid ∨ x.forum ≠ F ∨ CO5 (x is a comment whose root uri is in records,
//!                     valid or not, with value.forum ≠ x.forum; an absent forum fails)
//! ```
//!
//! Records are read from their JSON values with the namespace prefix `ns` (never the typed
//! structs, whose `$type` defaults are this build's), so the fold runs under any namespace.

use std::collections::BTreeSet;

use lessdugin_atp::syntax::AtUri;
use lessdugin_lexicon::Collection;
use lessdugin_lexicon::records::Forum;
use serde_json::Value;

use crate::social::fold::FoldError;
use crate::social::input::{FoldInput, ForumInput, Skel, SkelData, split_uri};
use crate::social::policy::EffectivePolicy;
use crate::store::repo::RepoStatus;

/// `data-model.md` §7 for one record: `(valid, first failing rule)`.
pub type Validity<'v> = &'v dyn Fn(&str, &Value) -> (bool, Option<String>);

/// Record validity with this build's lexicon (`lessdugin_lexicon::validity`), whose namespace is
/// [`lessdugin_lexicon::ns::NS`].
pub fn lexicon_validity(uri: &str, value: &Value) -> (bool, Option<String>) {
    let Ok(u) = AtUri::parse(uri) else {
        return (false, Some("G4".into()));
    };
    let v = lessdugin_lexicon::validity::check_uri(&u, value);
    (v.valid, v.first_rule().map(|r| r.to_string()))
}

/// The prepared input every layer reads.
pub struct Cx<'a> {
    /// The contract form (full values).
    pub input: &'a FoldInput,
    /// The skeleton form (validity, times, references).
    pub skel: ForumInput,
    /// The declaration.
    pub decl: Forum,
    /// The effective policy.
    pub policy: EffectivePolicy,
    /// V1: the candidate items that are Excluded.
    pub excluded: BTreeSet<String>,
}

fn err(e: impl core::fmt::Display) -> FoldError {
    FoldError(e.to_string())
}

impl<'a> Cx<'a> {
    /// Prepares `input` (namespace `ns`, record validity `validity`); rejects a malformed input
    /// (§1.1, §20.1 item 25).
    pub fn new(input: &'a FoldInput, ns: &str, validity: Validity<'_>) -> Result<Self, FoldError> {
        let (decl_ok, decl_rule) = validity(&input.decl.uri, &input.decl.value);
        if !decl_ok {
            return Err(FoldError(format!(
                "the declaration is invalid ({})",
                decl_rule.unwrap_or_default()
            )));
        }
        let skel = ForumInput::from_contract(input, ns, validity).map_err(err)?;
        let decl: Forum = serde_json::from_value(input.decl.value.clone()).map_err(err)?;
        let policy = EffectivePolicy::of(&decl);
        let mut cx = Self {
            input,
            skel,
            decl,
            policy,
            excluded: BTreeSet::new(),
        };
        cx.check_well_formed()?;
        cx.excluded = cx
            .items()
            .filter(|x| cx.v1_excludes(x))
            .map(|x| x.uri.clone())
            .collect();
        Ok(cx)
    }

    fn check_well_formed(&self) -> Result<(), FoldError> {
        let range = format!("{}-", self.fk());
        for x in self.skel.records.values() {
            let ok = match x.coll {
                Some(Collection::Membership) => x.rkey == self.fk(),
                Some(c) => c.is_forum_scoped() && x.rkey.starts_with(&range),
                None => false,
            };
            if !ok {
                return Err(FoldError(format!("{} is outside the fold input's sets", x.uri)));
            }
        }
        let subjects: BTreeSet<&str> = self
            .skel
            .records
            .values()
            .filter(|x| x.coll == Some(Collection::Opinion))
            .filter_map(|x| x.value()?.get("subject")?.get("uri")?.as_str())
            .collect();
        for u in &self.skel.absent {
            if self.skel.records.contains_key(u) || !subjects.contains(u.as_str()) {
                return Err(FoldError(format!("absent uri {u} is malformed")));
            }
        }
        Ok(())
    }

    /// The NSID prefix.
    pub fn ns(&self) -> &str {
        &self.skel.ns
    }

    /// F.
    pub fn forum(&self) -> &str {
        &self.skel.forum
    }

    /// `did(F)`.
    pub fn owner(&self) -> &str {
        &self.skel.owner
    }

    /// `fk(F)`.
    pub fn fk(&self) -> &str {
        &self.skel.fk
    }

    /// `now` (µs).
    pub fn now(&self) -> i64 {
        self.skel.now
    }

    /// Every record skeleton, by uri.
    pub fn records(&self) -> impl Iterator<Item = &Skel> {
        self.skel.records.values()
    }

    /// The skeleton of `uri`.
    pub fn skel(&self, uri: &str) -> Option<&Skel> {
        self.skel.records.get(uri)
    }

    /// The full value of `uri`.
    pub fn value(&self, uri: &str) -> Option<&'a Value> {
        self.input.records.get(uri).map(|r| &r.value)
    }

    /// `uri ∈ absent`.
    pub fn is_absent(&self, uri: &str) -> bool {
        self.skel.absent.contains(uri)
    }

    /// The repo status of `did` (missing: `Unavailable`).
    pub fn status(&self, did: &str) -> RepoStatus {
        self.skel.status(did)
    }

    /// `coll(u)` of any at-uri under this namespace.
    pub fn coll_of(&self, uri: &str) -> Option<Collection> {
        let (_, c, _) = split_uri(uri)?;
        Collection::from_nsid_in(self.ns(), c)
    }

    /// The candidate items (post, comment, wikiRevision, sequence), Excluded ones included.
    pub fn items(&self) -> impl Iterator<Item = &Skel> {
        self.records().filter(|x| x.is_item())
    }

    /// A candidate item that V1 does not exclude.
    pub fn live(&self, uri: &str) -> bool {
        self.skel(uri)
            .is_some_and(|x| x.is_item() && !self.excluded.contains(uri))
    }

    /// A non-Excluded post.
    pub fn live_post(&self, uri: &str) -> bool {
        self.skel(uri)
            .is_some_and(|x| x.coll == Some(Collection::Post) && !self.excluded.contains(uri))
    }

    /// A valid record whose `forum` is F (the explicit forum check of §20.1 item 23).
    pub fn in_forum(&self, x: &Skel) -> bool {
        x.valid && x.forum.as_deref() == Some(self.forum())
    }

    /// V1 (§10.1) for a candidate item.
    fn v1_excludes(&self, x: &Skel) -> bool {
        if !self.in_forum(x) {
            return true;
        }
        // CO5: compared literally, even when the root is invalid (NOTES 20)
        if let SkelData::Comment {
            root: Some(root), ..
        } = &x.data
            && let Some(r) = self.skel(&root.uri)
            && r.forum != x.forum
        {
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validity_of_a_bad_uri() {
        let (ok, rule) = lexicon_validity("not a uri", &Value::Null);
        assert!(!ok);
        assert_eq!(rule.as_deref(), Some("G4"));
    }
}
