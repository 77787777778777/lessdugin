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
//! versions   = per non-Excluded comment whose root.cid or parent.cid names another CID than its
//!              non-Excluded target's current one: "on revision n" (version i is the snapshot
//!              R_i.subject.cid of the target's i-th revision; the greatest i whose CID matches),
//!              or "on an unavailable version" (none matches); CIDs compare canonically (§0.4)
//! edited     ⇔ such a revision exists ∨ µs(updatedAt) later than µs(createdAt) (a createdAt
//!              without µs, before 1970, is earlier than any updatedAt)
//! ```

use std::collections::{BTreeMap, BTreeSet};

use lessdugin_lexicon::Collection;

use crate::social::full::cids;
use crate::social::full::cx::Cx;
use crate::social::full::view::ItemTime;
use crate::social::input::{RefSkel, SkelData};
use crate::social::threads::{self, Node, Placement, ThreadComment};
use crate::social::time;

/// The version of its target a comment's `root` or `parent` reference names (§11 step 6), when
/// it is not the target's current version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// "on revision n": the snapshot of the target's n-th `op: update` revision (1-based, TID
    /// order).
    Revision(u32),
    /// "on an unavailable version": no revision snapshot has the referenced CID.
    Unavailable,
}

/// The versions a comment's references name (§11 step 6); `None` for the current version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommentVersions {
    /// `root.cid`.
    pub root: Option<Version>,
    /// `parent.cid`.
    pub parent: Option<Version>,
}

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
    /// Comments whose `root` or `parent` names an earlier or unavailable version of a
    /// non-Excluded target (§11 step 6).
    pub versions: BTreeMap<String, CommentVersions>,
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

    // §11 step 6: revisions in TID order (then uri), with their snapshots' CIDs
    let mut revs: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
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
                .push((tid, x.uri.clone(), s.cid.clone()));
        }
    }
    for v in revs.values_mut() {
        v.sort();
    }
    let revisions: BTreeMap<String, Vec<String>> = revs
        .iter()
        .map(|(s, v)| (s.clone(), v.iter().map(|(_, u, _)| u.clone()).collect()))
        .collect();
    let version = |r: &RefSkel| -> Option<Version> {
        if !cx.live(&r.uri) {
            return None;
        }
        let target = cx.skel(&r.uri)?;
        if cids::same(&r.cid, &target.cid) {
            return None;
        }
        let n = revs
            .get(&r.uri)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, (_, _, c))| cids::same(&r.cid, c))
            .map(|(i, _)| i + 1)
            .last();
        Some(n.map_or(Version::Unavailable, |n| {
            Version::Revision(u32::try_from(n).unwrap_or(u32::MAX))
        }))
    };
    let mut versions = BTreeMap::new();
    for x in cx.items().filter(|x| cx.live(&x.uri)) {
        if let SkelData::Comment { root, parent, .. } = &x.data {
            let v = CommentVersions {
                root: root.as_ref().and_then(version),
                parent: parent.as_ref().and_then(version),
            };
            if v != CommentVersions::default() {
                versions.insert(x.uri.clone(), v);
            }
        }
    }

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
        versions,
        edited,
    }
}
