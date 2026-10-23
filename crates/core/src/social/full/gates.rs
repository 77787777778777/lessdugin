// SPDX-License-Identifier: AGPL-3.0-or-later
//! L4: gates (`karma/1`, `social.md` §8).
//!
//! ```text
//! gateValue(g) = decl.policy.gates.g if present, else the karma/1 default
//!                (reactCreate 10, reactSecond 5, reactAnti 20, authorModPersonal 50,
//!                 authorModFrontpage 2000, createTag 10, wikiEdit 5, sequence 1000)
//! passes(u, g) ⇔ u ∈ A ∨ K[u] ≥ gateValue(g)       (authorities pass even when banned or nullified)
//! ```
//!
//! Inputs: `cx` (policy, record owners), `a` (L0), `karma` (K).

use std::collections::{BTreeMap, BTreeSet};

use crate::social::full::cx::Cx;
use crate::social::full::karma::Karma;
use crate::social::full::view::Stage;
use crate::social::policy::EffectivePolicy;

/// The eight configurable gates of §8.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Gate {
    /// A react is valid (§9.2).
    ReactCreate,
    /// A react supporter (§9.2).
    ReactSecond,
    /// A react opposer (§9.2).
    ReactAnti,
    /// Author moderation on a `personal` post (§4.9).
    AuthorModPersonal,
    /// Author moderation on a `frontpage` or `curated` post (§4.9).
    AuthorModFrontpage,
    /// A genesis wiki revision (§13.2).
    CreateTag,
    /// A non-genesis wiki revision (§13.2).
    WikiEdit,
    /// The sequence library (§14).
    Sequence,
}

impl Gate {
    /// Every gate.
    pub const ALL: [Gate; 8] = [
        Gate::ReactCreate,
        Gate::ReactSecond,
        Gate::ReactAnti,
        Gate::AuthorModPersonal,
        Gate::AuthorModFrontpage,
        Gate::CreateTag,
        Gate::WikiEdit,
        Gate::Sequence,
    ];

    /// The `#gates` field name.
    pub const fn name(self) -> &'static str {
        match self {
            Gate::ReactCreate => "reactCreate",
            Gate::ReactSecond => "reactSecond",
            Gate::ReactAnti => "reactAnti",
            Gate::AuthorModPersonal => "authorModPersonal",
            Gate::AuthorModFrontpage => "authorModFrontpage",
            Gate::CreateTag => "createTag",
            Gate::WikiEdit => "wikiEdit",
            Gate::Sequence => "sequence",
        }
    }

    /// `gateValue(g)`: the declared value, or the `karma/1` default (§8).
    pub fn value(self, p: &EffectivePolicy) -> i64 {
        match self {
            Gate::ReactCreate => p.react_create,
            Gate::ReactSecond => p.react_second,
            Gate::ReactAnti => p.react_anti,
            Gate::AuthorModPersonal => p.author_mod_personal,
            Gate::AuthorModFrontpage => p.author_mod_frontpage,
            Gate::CreateTag => p.create_tag,
            Gate::WikiEdit => p.wiki_edit,
            Gate::Sequence => p.sequence,
        }
    }
}

/// `passes(u, g) ⇔ u ∈ A ∨ K[u] ≥ gateValue(g)` (§8), for any DID.
pub fn passes(
    p: &EffectivePolicy,
    a: &BTreeMap<String, u8>,
    karma: &Karma,
    did: &str,
    g: Gate,
) -> bool {
    a.contains_key(did) || karma.k_of(did) >= g.value(p)
}

/// L4 results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    /// `passes(u, g)` for every DID of A and every DID that owns a record in `records`.
    pub passes: BTreeMap<String, BTreeMap<Gate, bool>>,
}

impl Gates {
    /// `passes(u, g)` for a DID of [`Gates::passes`] (`false` for any other DID; use [`passes`]
    /// for those).
    pub fn pass(&self, did: &str, g: Gate) -> bool {
        self.passes
            .get(did)
            .and_then(|m| m.get(&g))
            .copied()
            .unwrap_or(false)
    }
}

/// L4 (`social.md` §8): `passes(u, g)` for every DID of A and every record owner.
pub fn gates(cx: &Cx<'_>, a: &BTreeMap<String, u8>, karma: &Stage<Karma>) -> Stage<Gates> {
    let k = karma.as_ref().map_err(|p| *p)?;
    let dids: BTreeSet<&str> = a
        .keys()
        .map(String::as_str)
        .chain(cx.records().map(|x| x.did.as_str()))
        .filter(|d| !d.is_empty())
        .collect();
    let passes = dids
        .into_iter()
        .map(|d| {
            let gs = Gate::ALL
                .iter()
                .map(|g| (*g, passes(&cx.policy, a, k, d, *g)))
                .collect();
            (d.to_owned(), gs)
        })
        .collect();
    Ok(Gates { passes })
}
