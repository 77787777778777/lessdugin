// SPDX-License-Identifier: AGPL-3.0-or-later
//! L5: author-tier moderation (`mod/1`, `social.md` §4.9) and the mod-log status of
//! author-scope records (§4.12).
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
//! Author-tier values act at rank 0 and never enter forum-tier resolution (L1). They never
//! affect opinions, karma or participation (L3 cannot read L5). Records with no effect (MA2,
//! MA7, MA9) are `reserved/unknown` already ([`ModRecord::no_effect`]) and get no status here.

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;
use lessdugin_lexicon::values::{AccessLevel, ActionKind, Tier};

use crate::social::full::cx::Cx;
use crate::social::full::karma::Karma;
use crate::social::full::moderation::{ModRecord, ModScope, Moderation, filter_allowed};
use crate::social::full::view::{Dim, DimValue, ModStatus, Stage};
use crate::social::input::SkelData;

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

/// What an author-scope record asks for, once its subject passed the table's subject checks.
enum Act<'m> {
    /// `#hide{hidden: true}` of a comment.
    Hide,
    /// `#access{level: banned}` of a DID.
    Ban,
    /// `#pin{scope: thread, pinned}` of a comment.
    Pin(bool),
    /// `#lock{locked, allowed}` of a post.
    Lock(bool, &'m [String]),
}

/// What the author tier reads: the input, A, L1 and K.
struct AuthorCx<'c, 'a> {
    cx: &'c Cx<'a>,
    a: &'c BTreeMap<String, u8>,
    l1: &'c Moderation,
    karma: &'c Karma,
}

impl AuthorCx<'_, '_> {
    /// `authorGate(X, p)`.
    fn gate(&self, x: &str, p: &str) -> bool {
        if self.a.contains_key(x) {
            return true;
        }
        let g = if self.l1.tier(p) == Tier::Personal {
            self.cx.policy.author_mod_personal
        } else {
            self.cx.policy.author_mod_frontpage
        };
        self.karma.k_of(x) >= g
    }

    /// `root(c)` of a non-Excluded comment c whose root is a non-Excluded post.
    fn live_root(&self, c: &str) -> Option<&str> {
        let x = self.cx.skel(c)?;
        if x.coll != Some(Collection::Comment) || !self.cx.live(c) {
            return None;
        }
        let SkelData::Comment {
            root: Some(root), ..
        } = &x.data
        else {
            return None;
        };
        self.cx.live_post(&root.uri).then_some(root.uri.as_str())
    }

    /// `author(x)` of a record in `records`.
    fn author(&self, uri: &str) -> Option<&str> {
        self.cx.skel(uri).map(|x| x.did.as_str())
    }

    /// The table condition of §4.9 (subject, own thread or post, author gate, immunity, a live
    /// root): the act, or `None` (unauthorised).
    fn table<'m>(&self, m: &'m ModRecord) -> Option<Act<'m>> {
        let x = m.actor.as_str();
        match m.action? {
            ActionKind::Hide => {
                if m.value_of(Dim::Hidden) != Some(&DimValue::Bool(true)) {
                    return None;
                }
                let root = self.live_root(&m.subject)?;
                let ok = self.author(root) == Some(x)
                    && self.gate(x, root)
                    && self
                        .author(&m.subject)
                        .is_some_and(|d| !self.a.contains_key(d));
                ok.then_some(Act::Hide)
            }
            ActionKind::Access => {
                let banned =
                    m.value_of(Dim::Access) == Some(&DimValue::Access(AccessLevel::Banned));
                let did = m.subject.starts_with("did:");
                (banned && did && !self.a.contains_key(&m.subject)).then_some(Act::Ban)
            }
            ActionKind::Pin => {
                let Some(DimValue::Bool(pinned)) = m.value_of(Dim::PinThread) else {
                    return None;
                };
                let root = self.live_root(&m.subject)?;
                (self.author(root) == Some(x) && self.gate(x, root)).then_some(Act::Pin(*pinned))
            }
            ActionKind::Lock => {
                let Some(DimValue::Bool(locked)) = m.value_of(Dim::Locked) else {
                    return None;
                };
                let p = m.subject.as_str();
                (self.cx.live_post(p) && self.author(p) == Some(x) && self.gate(x, p))
                    .then_some(Act::Lock(*locked, &m.allowed))
            }
            _ => None,
        }
    }
}

/// The forum-tier dimension that governs an author-tier act (§4.9 precedence); `None` for a ban,
/// which no forum-tier value of the DID lifts.
fn governing_dim(act: &Act<'_>) -> Option<Dim> {
    match act {
        Act::Hide => Some(Dim::Hidden),
        Act::Ban => None,
        Act::Pin(_) => Some(Dim::PinThread),
        Act::Lock(..) => Some(Dim::Locked),
    }
}

/// L5 (`social.md` §4.9): author-tier effects and the statuses of author-scope records.
///
/// Needs E and K (L3) unless no listed author-scope record may have an effect, or
/// `policy.authorModeration` is false (then every such record is `unauthorised`).
pub fn author_tier(
    cx: &Cx<'_>,
    a: &BTreeMap<String, u8>,
    l1: &Moderation,
    karma: &Stage<Karma>,
) -> Stage<AuthorTier> {
    let records: Vec<&ModRecord> = l1
        .records
        .values()
        .filter(|m| m.scope == ModScope::Author && m.no_effect.is_none())
        .collect();
    let mut out = AuthorTier::default();
    if records.is_empty() {
        return Ok(out);
    }
    if !cx.policy.author_moderation {
        for m in records {
            out.status.insert(m.uri.clone(), ModStatus::Unauthorised);
        }
        return Ok(out);
    }
    let karma = karma.as_ref().map_err(|p| *p)?;
    let t = AuthorCx { cx, a, l1, karma };

    // statuses (§4.12) and the effective acts
    let mut hidden: BTreeSet<&str> = BTreeSet::new();
    let mut bans: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new(); // X → banned DIDs
    for m in records {
        let act = t.table(m).filter(|_| karma.e.contains(&m.actor));
        let Some(act) = act else {
            out.status.insert(m.uri.clone(), ModStatus::Unauthorised);
            continue;
        };
        if m.expired(cx.now()) {
            out.status.insert(m.uri.clone(), ModStatus::Expired);
            continue;
        }
        if !m.cid_ok(cx) {
            out.status.insert(m.uri.clone(), ModStatus::Stale);
            continue;
        }
        // authorEffective(m) holds from here on
        let forum = governing_dim(&act).and_then(|d| l1.get(&m.subject, d));
        let status = match forum {
            Some(r) => ModStatus::Overridden { rank: r.rank },
            None => ModStatus::Effective,
        };
        out.status.insert(m.uri.clone(), status);
        match act {
            // resolve(c, hidden) = Default is part of AuthorModerated (below)
            Act::Hide => {
                hidden.insert(&m.subject);
            }
            // judged per comment (below), whatever the forum tier says of the DID
            Act::Ban => {
                bans.entry(&m.actor).or_default().insert(&m.subject);
            }
            Act::Pin(pinned) if forum.is_none() => {
                out.thread_pins.insert(m.subject.clone(), pinned);
            }
            Act::Lock(true, allowed) if forum.is_none() => {
                out.locks
                    .insert(m.subject.clone(), filter_allowed(cx, &m.subject, allowed));
            }
            Act::Pin(_) | Act::Lock(..) => {}
        }
    }

    // AuthorModerated(c) for every non-Excluded comment with a live root
    for x in cx.items() {
        let Some(p) = t.live_root(&x.uri) else {
            continue;
        };
        if l1.get(&x.uri, Dim::Hidden).is_some() {
            continue; // the forum tier's hide or keep governs
        }
        let Some(owner) = t.author(p) else {
            continue;
        };
        let author = x.did.as_str();
        let author_hidden = hidden.contains(x.uri.as_str());
        let author_banned =
            bans.get(owner).is_some_and(|ds| ds.contains(author)) && t.gate(owner, p);
        let locked_out = out.locks.get(p).is_some_and(|allowed| {
            !allowed.contains(&x.uri) && !a.contains_key(author) && author != owner
        });
        if author_hidden || author_banned || locked_out {
            out.moderated.insert(x.uri.clone());
        }
    }
    Ok(out)
}
