// SPDX-License-Identifier: AGPL-3.0-or-later
//! L6: ranking, sorts, filters and Recent Discussion (`magic/1`, `social.md` §12). **STUB**: the
//! layer agent replaces [`orderings`] (`crate::social::rank` has `hot`, `order_posts` and
//! `cmp_comments` to reuse).
//!
//! ```text
//! b(p)      = score(p) + (tier = curated ? curatedBonus : 0) + (tier ≥ frontpage ? frontpageBonus : 0)
//! posts     = listed posts (displayed Visible or Collapsed) passing the filter (frontpage: tier ≥
//!             frontpage ∧ kind ≠ quickTake; all; curated: tier = curated ∧ kind ≠ quickTake), the
//!             tag (applies), kind (PO2) and author filters, in the sort's order (§12.2); magic:
//!             pins first and the cutoff (pinned posts exempt); hotBits for b > 0
//! comments  = pre-order of the displayed nodes of query.comments.thread (listed or Tombstone
//!             comments, placeholders with a listed descendant); siblings: thread pins first
//!             (forum tier, else author tier), then the sort key, then uri; placeholders last
//! recent    = listed posts with ≥ 1 Visible comment in their placed thread, by (newest Visible
//!             comment t_rank desc, uri asc), each with its 3 newest Visible comments
//! ```
//!
//! Inputs: `cx` (post kinds, `now`, ranking policy), `query`, `l1` (tiers, forum pins, thread
//! pins), `times` (`t_rank`), `threads` (placements), `visibility` (states, displayed), `scores`,
//! `wiki` (`applies`, only for a tag filter), `author` (author thread pins).

use std::collections::BTreeMap;

use crate::social::full::author::AuthorTier;
use crate::social::full::cx::Cx;
use crate::social::full::moderation::Moderation;
use crate::social::full::query::Query;
use crate::social::full::scores::Score;
use crate::social::full::view::{ItemTime, Pending, Stage};
use crate::social::full::visibility::Visibility;
use crate::social::full::wiki::Wiki;
use crate::social::threads::Placement;

/// The pending marker of [`orderings`].
pub const PENDING: Pending = Pending {
    stub: "ranking::orderings",
    layer: "L6",
    section: "§12",
};

/// One Recent Discussion entry (§12.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentPost {
    /// The post uri.
    pub post: String,
    /// Its up to 3 newest `Visible` comments, by (t_rank desc, uri asc).
    pub comments: Vec<String>,
}

/// L6 orderings for one query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Orderings {
    /// The post list of `query.posts` (`None` without it).
    pub post_order: Option<Vec<String>>,
    /// `hot(b(p), t_rank(p))` bit patterns of the listed posts with b > 0 (`magic` only).
    pub hot_bits: BTreeMap<String, u64>,
    /// The comment order of `query.comments` (`None` without it).
    pub comment_order: Option<Vec<String>>,
    /// Recent Discussion (no limit).
    pub recent_discussion: Vec<RecentPost>,
}

/// **STUB (L6, `social.md` §12).** The orderings `query` asks for, and Recent Discussion.
#[allow(clippy::too_many_arguments)]
pub fn orderings(
    _cx: &Cx<'_>,
    _query: &Query,
    _l1: &Moderation,
    _times: &BTreeMap<String, ItemTime>,
    _threads: &BTreeMap<String, Placement>,
    visibility: &Stage<Visibility>,
    scores: &Stage<BTreeMap<String, Score>>,
    _wiki: &Stage<Wiki>,
    author: &Stage<AuthorTier>,
) -> Stage<Orderings> {
    let _visibility = visibility.as_ref().map_err(|p| *p)?;
    let _scores = scores.as_ref().map_err(|p| *p)?;
    let _author = author.as_ref().map_err(|p| *p)?;
    Err(PENDING)
}
