// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: tags and wiki (`karma/1`, `social.md` §13): head selection, candidates and flags (§13.2),
//! relevance and application (§13.3). **STUB**: the layer agent replaces [`wiki`].
//!
//! ```text
//! page(slug) = valid wikiRevisions r with r.forum = F, r.slug = slug, not Hidden by V1–V5
//!              (> 1000: keep the greatest (TID, then smallest uri), flag partial)
//! par(r)     = { q.uri : q ∈ r.parents } ∩ uris(page)
//! genesisOK  = tagCreation = moderators ? author ∈ A : passes(author, createTag)
//! editOK     = passes(author, wikiEdit)
//! Cand       = least set: r ∈ page ∧ (genesisOK(r) ∨ (editOK(r) ∧ par(r) ∩ Cand ≠ ∅))
//! Edges      = { (r, q) : r ∈ Cand, q ∈ par(r) ∩ Cand }, cut per round: every cyclic SCC loses
//!              the edges from its greatest-uri member into the SCC
//! depth(r)   = 0 without outgoing edge, else 1 + max depth(q)
//! canonical  = the moderator head if it names a candidate with its current CID,
//!              else argmax (depth desc, K[author] desc, uri asc)
//! exists     ⇔ canonical ≠ none ∧ ¬effective(tag:slug, tag.removed);  applicable ⇔ exists ∧ isTag
//! rel(p, s)  = own + votes;  own = s ∈ p.tags ∧ author not restricted from tag ? max(1, small(max(KP, 0))) : 0
//!              votes = Σ counting v on p, voter ∈ E, tags axis, entry for s (OP3 then OP4): power(KP[voter], value)
//! applies    ⇔ applicable(s) ∧ rel(p, s) > 0
//! ```
//!
//! Inputs: `cx` (wikiRevision values via [`Cx::value`], post tags, CIDs, policy `tagCreation`),
//! `a` (L0), `l1` (`tag.removed`, `tag.head`, restrictions), `v1_v5` (the page rule), `karma`
//! (E, K, KP, counting opinions with the tags axis), `gates` (createTag, wikiEdit).

use std::collections::{BTreeMap, BTreeSet};

use crate::social::full::cx::Cx;
use crate::social::full::gates::Gates;
use crate::social::full::karma::Karma;
use crate::social::full::moderation::Moderation;
use crate::social::full::view::{Pending, Stage, State};

/// The pending marker of [`wiki`].
pub const PENDING: Pending = Pending {
    stub: "wiki::wiki",
    layer: "L5",
    section: "§13",
};

/// A wiki candidate (r ∈ Cand) and its display values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WikiCandidate {
    /// `depth(r)` over the edges left by the cycle cut.
    pub depth: u32,
    /// The edge targets after the cycle cut.
    pub parents: BTreeSet<String>,
    /// The edge targets whose stored `parents` CID differs from their current CID.
    pub altered: BTreeSet<String>,
    /// `Root(ParentUnavailable)`: a non-empty `parents` field with no candidate among them.
    pub parent_unavailable: bool,
}

/// One slug's page (§13.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WikiPage {
    /// The canonical revision.
    pub canonical: Option<String>,
    /// `exists(slug)`.
    pub exists: bool,
    /// `applicable(slug)`.
    pub applicable: bool,
    /// The page held more than 1,000 revisions.
    pub partial: bool,
    /// The history chain from `canonical`.
    pub history: Vec<String>,
    /// "N unmerged edits".
    pub unmerged: u32,
    /// Every candidate.
    pub candidates: BTreeMap<String, WikiCandidate>,
}

/// L5 tag and wiki results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wiki {
    /// Pages of every slug of a wikiRevision not Excluded by V1, and of every slug whose `tag:`
    /// subject has a forum-tier `tag.removed` or `tag.head` assertion.
    pub pages: BTreeMap<String, WikiPage>,
    /// The non-zero `rel(p, slug)` of every post not Excluded.
    pub rel: BTreeMap<String, BTreeMap<String, i64>>,
    /// `applies(p, slug)`.
    pub applies: BTreeMap<String, BTreeSet<String>>,
}

/// **STUB (L5, `social.md` §13).** Wiki pages, tag relevance and application.
pub fn wiki(
    _cx: &Cx<'_>,
    _a: &BTreeMap<String, u8>,
    _l1: &Moderation,
    _v1_v5: &BTreeMap<String, Option<State>>,
    karma: &Stage<Karma>,
    gates: &Stage<Gates>,
) -> Stage<Wiki> {
    let _karma = karma.as_ref().map_err(|p| *p)?;
    let _gates = gates.as_ref().map_err(|p| *p)?;
    Err(PENDING)
}
