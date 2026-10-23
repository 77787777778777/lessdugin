// SPDX-License-Identifier: AGPL-3.0-or-later
//! L6: visibility V8–V10 and the thread pass (`mod/1`, `social.md` §10.1, §10.2). V1–V7 are
//! [`crate::social::full::modstate`]'s.
//!
//! ```text
//! state(x) = v1_v7(x) if Some, else
//!            V8  mode = open ∧ author(x) ∉ S ∪ E: newUsers collapse → Collapsed(NewUser),
//!                hide → Hidden(NewUser), show → no match
//!            V9  score(x) ≤ display.collapseBelow → Collapsed(LowScore)
//!            V10 Visible
//! thread pass: a Hidden post or comment with a listed descendant in the placed tree
//!              (through Missing placeholders; Excluded comments are not nodes; judged on the
//!              states before the pass) becomes Tombstone, keeping its reason and Removed{…};
//!              every other state is unchanged; a placeholder is shown iff it has a listed
//!              descendant
//! ```
//!
//! The lens (§10.3, L7) is local presentation applied after the fold: [`lens`] reads the fold's
//! displayed states by reference and returns a separate map, so it cannot change any fold
//! output (scores, karma, gates, other viewers' states).
//!
//! ```text
//! lens(x) = Collapsed(Muted) if displayed(x) is listed ∧ author(x) ∈ muted (blocks count as mutes)
//!         = displayed(x)     otherwise
//! ```

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::values::{MembershipMode, NewUsers};

use crate::social::full::cx::Cx;
use crate::social::full::karma::Karma;
use crate::social::full::scores::Score;
use crate::social::full::view::{CollapseReason, HiddenReason, Participation, Stage, State};
use crate::social::input::split_uri;
use crate::social::threads::{Node, Placement};

/// L6 visibility results.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visibility {
    /// V1–V10 for every candidate item (Excluded ones included), before the thread pass.
    pub states: BTreeMap<String, State>,
    /// The same items after the thread pass (Tombstones).
    pub displayed: BTreeMap<String, State>,
    /// The displayed `Missing` placeholders (with a listed descendant), by thread root.
    pub shown_missing: BTreeMap<String, BTreeSet<String>>,
}

impl Visibility {
    /// The state of `uri` before the thread pass is `Visible`.
    pub fn visible(&self, uri: &str) -> bool {
        matches!(self.states.get(uri), Some(State::Visible))
    }

    /// The displayed state of `uri` is listed (`Visible` or `Collapsed`).
    pub fn listed(&self, uri: &str) -> bool {
        self.displayed.get(uri).is_some_and(State::listed)
    }

    /// `uri` is a displayed node of its thread: listed, or a `Tombstone` (§10.2).
    pub fn shown(&self, uri: &str) -> bool {
        matches!(
            self.displayed.get(uri),
            Some(State::Visible | State::Collapsed(_) | State::Tombstone(_))
        )
    }
}

/// For every node of `pl` (the root included), whether it has a listed descendant, given which
/// comments are listed.
pub fn listed_below(pl: &Placement, listed: impl Fn(&str) -> bool) -> BTreeMap<Node, bool> {
    let mut below: BTreeMap<Node, bool> = BTreeMap::new();
    let mut order = vec![Node::Root];
    order.extend(pl.top_down());
    // children come after their parent in `order`, so walk it backwards
    for node in order.iter().rev() {
        let any = pl.children_of(node).iter().any(|ch| {
            let own = matches!(ch, Node::Comment(c) if listed(c));
            own || below.get(ch).copied().unwrap_or(false)
        });
        below.insert(node.clone(), any);
    }
    below
}

/// L6 (`social.md` §10.1 V8–V10, §10.2): visibility states and the thread pass.
pub fn visibility(
    cx: &Cx<'_>,
    l2: &Participation,
    v1_v7: &Stage<BTreeMap<String, Option<State>>>,
    karma: &Stage<Karma>,
    scores: &Stage<BTreeMap<String, Score>>,
    threads: &BTreeMap<String, Placement>,
) -> Stage<Visibility> {
    let v1_v7 = v1_v7.as_ref().map_err(|p| *p)?;
    let karma = karma.as_ref().map_err(|p| *p)?;
    let scores = scores.as_ref().map_err(|p| *p)?;
    let policy = &cx.policy;

    // §10.1
    let mut states: BTreeMap<String, State> = BTreeMap::new();
    for x in cx.items() {
        let state = match v1_v7.get(&x.uri) {
            Some(Some(s)) => s.clone(),
            _ => {
                let author = x.did.as_str();
                let newcomer = l2.mode == MembershipMode::Open
                    && !l2.s.contains(author)
                    && !karma.e.contains(author);
                let score = scores.get(&x.uri).map_or(0, |s| s.score);
                match (newcomer, policy.new_users) {
                    (true, NewUsers::Collapse) => State::Collapsed(CollapseReason::NewUser),
                    (true, NewUsers::Hide) => State::Hidden(HiddenReason::NewUser),
                    _ if score <= policy.collapse_below => {
                        State::Collapsed(CollapseReason::LowScore)
                    }
                    _ => State::Visible,
                }
            }
        };
        states.insert(x.uri.clone(), state);
    }

    // §10.2
    let listed = |u: &str| states.get(u).is_some_and(State::listed);
    let mut displayed = states.clone();
    let mut shown_missing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (root, pl) in threads {
        let below = listed_below(pl, listed);
        for (node, any) in &below {
            if !*any {
                continue;
            }
            let uri = match node {
                Node::Root if !pl.root_missing => root.as_str(),
                Node::Comment(c) => c.as_str(),
                Node::Missing(m) => {
                    shown_missing
                        .entry(root.clone())
                        .or_default()
                        .insert(m.clone());
                    continue;
                }
                Node::Root => continue,
            };
            if let Some(st) = displayed.get_mut(uri)
                && let State::Hidden(r) = st
            {
                *st = State::Tombstone(r.clone());
            }
        }
    }

    Ok(Visibility {
        states,
        displayed,
        shown_missing,
    })
}

/// A displayed state after the lens (§10.3, L7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lensed {
    /// The fold's displayed state, unchanged.
    Fold(State),
    /// `Collapsed(Muted)`: a listed item by a muted (or blocked) author.
    Muted,
}

/// The lens (§10.3, L7): mutes (and blocks treated as mutes) collapse the listed items of
/// `muted` authors. It reads the fold's displayed states and never changes them.
pub fn lens(
    displayed: &BTreeMap<String, State>,
    muted: &BTreeSet<String>,
) -> BTreeMap<String, Lensed> {
    displayed
        .iter()
        .map(|(u, st)| {
            let author = split_uri(u).map(|(d, _, _)| d);
            let l = if st.listed() && author.is_some_and(|d| muted.contains(d)) {
                Lensed::Muted
            } else {
                Lensed::Fold(st.clone())
            };
            (u.clone(), l)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::threads::{ThreadComment, place};

    fn c(uri: &str, parent: Option<&str>) -> ThreadComment {
        ThreadComment {
            uri: uri.into(),
            parent: parent.map(Into::into),
        }
    }

    #[test]
    fn listed_descendants_through_placeholders() {
        // a → b → d; x (missing) → e; f alone
        let pl = place(
            "p",
            false,
            &[
                c("a", None),
                c("b", Some("a")),
                c("d", Some("b")),
                c("e", Some("x")),
                c("f", None),
            ],
        );
        let below = listed_below(&pl, |u| u == "d" || u == "e");
        assert!(below[&Node::Root]);
        assert!(below[&Node::Comment("a".into())]);
        assert!(below[&Node::Comment("b".into())]);
        assert!(!below[&Node::Comment("d".into())]);
        assert!(below[&Node::Missing("x".into())]);
        assert!(!below[&Node::Comment("f".into())]);
        let none = listed_below(&pl, |_| false);
        assert!(none.values().all(|b| !b));
    }

    #[test]
    fn the_lens_never_changes_the_fold() {
        let p = "at://did:plc:m/x.post/k".to_owned();
        let q = "at://did:plc:n/x.post/k".to_owned();
        let h = "at://did:plc:m/x.comment/k".to_owned();
        let displayed = BTreeMap::from([
            (p.clone(), State::Visible),
            (q.clone(), State::Collapsed(CollapseReason::LowScore)),
            (h.clone(), State::Hidden(HiddenReason::Banned)),
        ]);
        let before = displayed.clone();
        let muted = BTreeSet::from(["did:plc:m".to_owned()]);
        let l = lens(&displayed, &muted);
        assert_eq!(displayed, before);
        assert_eq!(l[&p], Lensed::Muted);
        assert_eq!(
            l[&q],
            Lensed::Fold(State::Collapsed(CollapseReason::LowScore))
        );
        assert_eq!(l[&h], Lensed::Fold(State::Hidden(HiddenReason::Banned)));
    }
}
