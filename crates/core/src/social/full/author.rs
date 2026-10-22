// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: author-tier moderation (`mod/1`, `social.md` §4.9) and the mod-log status of
//! author-scope records (§4.12). **STUB**: the layer agent replaces [`author_tier`].
//!
//! ```text
//! authorEffective(m) ⇔ valid ∧ scope_eff = author ∧ MA6 ∧ policy.authorModeration
//!                    ∧ X ∈ E ∧ (until absent ∨ µs(until) > now) ∧ (positive ⇒ CID check)
//!                    ∧ the table condition (a live root: c a non-Excluded comment whose root is
//!                      a non-Excluded post; #lock: a non-Excluded post)
//! authorGate(X, p)   ⇔ X ∈ A ∨ K[X] ≥ (tier(p) = personal ? authorModPersonal : authorModFrontpage)
//! #hide{hidden}      comment c: author(root(c)) = X, authorGate(X, root(c)), author(c) ∉ A
//! #access{banned}    DID d ∉ A: comments of d in threads of posts p with author(p) = X ∧ authorGate(X, p)
//! #pin{thread}       comment c: author(root(c)) = X, authorGate(X, root(c))
//! #lock{locked}      post p: author(p) = X, authorGate(X, p)
//! AuthorModerated(c) ⇔ resolve(c, hidden) = Default ∧ (authorHidden ∨ authorBanned
//!                      (keepRank(c) = 0) ∨ authorLockedOut (resolve(p, locked) = Default,
//!                      c ∉ aAllowed(p), author(c) ∉ A, author(c) ≠ X))
//! status (§4.12): unauthorised (authorModeration off, X ∉ E, table condition) → expired
//!                 → stale (positive pin CID) → overridden (forum tier Resolved for hidden /
//!                 locked / pin.thread, with its rank; never for a ban) → effective
//! ```
//!
//! Inputs: `cx` (records, V1 via [`Cx::live`], `now`, policy `authorModeration` and gates),
//! `a` (L0), `l1` (the parsed records with [`ModScope::Author`], forum-tier resolutions,
//! `keepRank`, tiers), `karma` (E and K). Records with no effect (MA2, MA7, MA9) are
//! `reserved/unknown` already and need no status here.

use std::collections::{BTreeMap, BTreeSet};

use crate::social::full::cx::Cx;
use crate::social::full::karma::Karma;
use crate::social::full::moderation::{ModScope, Moderation};
use crate::social::full::view::{ModStatus, Pending, Stage};

/// The pending marker of [`author_tier`].
pub const PENDING: Pending = Pending {
    stub: "author::author_tier",
    layer: "L5",
    section: "§4.9, §4.12",
};

/// L5 author-tier results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorTier {
    /// The mod-log status of every listed author-scope record that may have an effect
    /// (`no_effect` = `None`). A record missing here is left out of the mod log.
    pub status: BTreeMap<String, ModStatus>,
    /// The comments with `AuthorModerated(c)` (V6).
    pub moderated: BTreeSet<String>,
    /// Author thread pins that apply (`resolve(c, pin[thread]) = Default`): comment → pinned
    /// (§12.4).
    pub thread_pins: BTreeMap<String, bool>,
    /// Author locks that apply (`resolve(p, locked) = Default`): post → `aAllowed(p)` (MA9
    /// entries).
    pub locks: BTreeMap<String, BTreeSet<String>>,
}

/// **STUB (L5, `social.md` §4.9).** Author-tier effects and author-scope statuses.
///
/// Exact already when no listed author-scope record may have an effect: then nothing is
/// author-moderated, pinned or locked, and no status is due.
pub fn author_tier(
    _cx: &Cx<'_>,
    _a: &BTreeMap<String, u8>,
    l1: &Moderation,
    _karma: &Stage<Karma>,
) -> Stage<AuthorTier> {
    let any = l1
        .records
        .values()
        .any(|m| m.scope == ModScope::Author && m.no_effect.is_none());
    if !any {
        return Ok(AuthorTier::default());
    }
    Err(PENDING)
}
