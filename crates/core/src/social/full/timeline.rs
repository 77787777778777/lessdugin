// SPDX-License-Identifier: AGPL-3.0-or-later
//! Time (`magic/1`, `social.md` §2) and thread placement (`mod/1`, §11 steps 1–6). Both read
//! only validity (V1), `obs` and `decl`, so they sit outside the layer stack (§1.4).
//!
//! ```text
//! t_decl     = µs(decl.createdAt) or 0, clamped to t_obs(decl) when > 5 min after it
//! threads    = every non-Excluded post, and every root named by a non-Excluded comment;
//!              root present ⇔ it is a non-Excluded post (an Excluded root is a Missing root)
//! t_rank(x)  = max(lower(x), t_eff'(x)); lower = t_decl for posts, wikiRevisions, sequences and
//!              comments under the root (top-level, under a placeholder, re-attached, or in a
//!              Missing root's thread); else t_rank(parent comment); computed top-down
//! revisions  = valid op: update revisions per subject, in TID order (§11 step 6)
//! edited     ⇔ such a revision exists ∨ updatedAt later than createdAt
//! ```

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;

use crate::social::full::cx::Cx;
use crate::social::full::view::ItemTime;
use crate::social::input::SkelData;
use crate::social::threads::{self, Node, Placement, ThreadComment};
use crate::social::time;

/// Time and threads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    /// `t_decl` (µs).
    pub t_decl: i64,
    /// Time values of every candidate item not Excluded.
    pub times: BTreeMap<String, ItemTime>,
    /// Placed threads by root post uri.
    pub threads: BTreeMap<String, Placement>,
    /// Valid `op: update` revisions per subject uri, in TID order.
    pub revisions: BTreeMap<String, Vec<String>>,
    /// Items with the `[edited]` badge.
    pub edited: BTreeSet<String>,
}

/// Computes time and threads (§2.2, §11).
pub fn timeline(cx: &Cx<'_>) -> Timeline {
    let t_decl = time::t_decl(
        cx.input
            .decl
            .value
            .get("createdAt")
            .and_then(|v| v.as_str())
            .and_then(time::us),
        cx.skel.decl_obs,
    );

    // §11 step 6: revisions in TID order
    let mut revs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for x in cx.records() {
        if let SkelData::Revision {
            subject: Some(s),
            op: Some(op),
            ..
        } = &x.data
            && x.valid
            && op == "update"
        {
            let tid = time::rkey_tid(&x.rkey).unwrap_or_default().to_owned();
            revs.entry(s.uri.clone())
                .or_default()
                .push((tid, x.uri.clone()));
        }
    }
    let revisions: BTreeMap<String, Vec<String>> = revs
        .into_iter()
        .map(|(s, mut v)| {
            v.sort();
            (s, v.into_iter().map(|(_, u)| u).collect())
        })
        .collect();

    // t_rank of posts, wikiRevisions and sequences
    let mut t_rank: BTreeMap<String, i64> = BTreeMap::new();
    for x in cx.items() {
        if cx.live(&x.uri) && x.coll != Some(Collection::Comment) {
            t_rank.insert(x.uri.clone(), x.times.rank(t_decl));
        }
    }

    // §11: threads of every live post and every root named by a live comment
    let mut by_root: BTreeMap<String, Vec<ThreadComment>> = BTreeMap::new();
    for x in cx.items() {
        if !cx.live(&x.uri) {
            continue;
        }
        if x.coll == Some(Collection::Post) {
            by_root.entry(x.uri.clone()).or_default();
        }
        if let SkelData::Comment {
            root: Some(root),
            parent,
            ..
        } = &x.data
        {
            by_root
                .entry(root.uri.clone())
                .or_default()
                .push(ThreadComment {
                    uri: x.uri.clone(),
                    parent: parent.as_ref().map(|p| p.uri.clone()),
                });
        }
    }
    let mut placed: BTreeMap<String, Placement> = BTreeMap::new();
    for (root, comments) in by_root {
        let present = cx.live_post(&root);
        let placement = threads::place(&root, present, &comments);
        // §2.2, §11 step 5: top-down
        let root_rank = if present {
            t_rank.get(&root).copied().unwrap_or(t_decl)
        } else {
            t_decl
        };
        for node in placement.top_down() {
            let Node::Comment(c) = &node else {
                continue;
            };
            let lower = match placement.tree_parent(c) {
                Some(p) => t_rank.get(p).copied().unwrap_or(root_rank),
                None => root_rank,
            };
            if let Some(x) = cx.skel(c) {
                t_rank.insert(c.clone(), x.times.rank(lower));
            }
        }
        placed.insert(root, placement);
    }

    let mut times = BTreeMap::new();
    let mut edited = BTreeSet::new();
    for x in cx.items() {
        if let Some(t) = t_rank.get(&x.uri) {
            times.insert(
                x.uri.clone(),
                ItemTime {
                    t_rank: *t,
                    future: x.times.future(),
                    inconsistent: x.times.inconsistent(),
                },
            );
            if revisions.contains_key(&x.uri) || x.updated_later() {
                edited.insert(x.uri.clone());
            }
        }
    }
    Timeline {
        t_decl,
        times,
        threads: placed,
        revisions,
        edited,
    }
}
