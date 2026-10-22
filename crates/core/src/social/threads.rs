// SPDX-License-Identifier: AGPL-3.0-or-later
//! Thread placement (`mod/1`, `social.md` §11, steps 1–4): a pure function of the thread's
//! comments and their parent pointers.
//!
//! ```text
//! nodes     = the root p (Missing(p) if p is not a non-Excluded post) + every non-Excluded
//!             comment c with c.root.uri = p
//! parent(c) = Root                   if c.parent is absent
//!           = Comment(c.parent.uri)  if c.parent.uri is a node of this thread (itself included)
//!           = Missing(c.parent.uri)  otherwise: one placeholder per distinct uri, a child of the
//!                                    root
//! cycles    : each component holds at most one cycle; its member with the greatest uri is
//!             re-attached to the root and flagged Orphaned
//! depth     : the root's children have depth 1; a comment whose depth would exceed
//!             THREAD_DEPTH_MAX (1,000) is re-attached to the root, flagged Orphaned, and its
//!             subtree continues from depth 1
//! ```
//!
//! Placement never reads CIDs, times or validity: callers pass only the non-Excluded comments
//! (rule V1) and say whether the root is present. The result does not depend on the order of
//! the input (every walk is over sorted uris).

use std::collections::{BTreeMap, BTreeSet};

use crate::limits::THREAD_DEPTH_MAX;

/// Where a placed comment hangs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Parent {
    /// The thread root (the post, or the `Missing(p)` root).
    Root,
    /// Another comment of the thread.
    Comment(String),
    /// A `Missing(uri)` placeholder (a child of the root).
    Missing(String),
}

/// A node of a placed thread, as a parent key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Node {
    /// The root.
    Root,
    /// A comment.
    Comment(String),
    /// A `Missing(uri)` placeholder.
    Missing(String),
}

impl Node {
    /// The node's uri (`None` for the root).
    pub fn uri(&self) -> Option<&str> {
        match self {
            Self::Root => None,
            Self::Comment(u) | Self::Missing(u) => Some(u),
        }
    }
}

impl From<&Parent> for Node {
    fn from(p: &Parent) -> Self {
        match p {
            Parent::Root => Node::Root,
            Parent::Comment(u) => Node::Comment(u.clone()),
            Parent::Missing(u) => Node::Missing(u.clone()),
        }
    }
}

/// The placed tree of one thread (§11 steps 1–4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    /// The root post uri p.
    pub root: String,
    /// p is not a non-Excluded post of the fold input: the root is `Missing(p)`.
    pub root_missing: bool,
    /// The parent of each comment after the cycle cut and the depth cap.
    pub parent: BTreeMap<String, Parent>,
    /// The `Missing` placeholders (children of the root).
    pub missing: BTreeSet<String>,
    /// The depth of each comment and placeholder (the root's children have depth 1).
    pub depth: BTreeMap<String, u32>,
    /// Comments re-attached to the root by the cycle cut or the depth cap.
    pub orphaned: BTreeSet<String>,
    /// Children of each node, sorted by uri (placeholders listed separately from comments).
    pub children: BTreeMap<Node, Vec<Node>>,
}

impl Placement {
    /// The greatest depth (0 without comments).
    pub fn max_depth(&self) -> u32 {
        self.depth.values().copied().max().unwrap_or(0)
    }

    /// The children of `node`: comments first, then placeholders, each sorted by uri.
    pub fn children_of(&self, node: &Node) -> &[Node] {
        self.children.get(node).map_or(&[], Vec::as_slice)
    }

    /// Every node below the root, top-down (breadth-first, children in uri order).
    pub fn top_down(&self) -> Vec<Node> {
        let mut out = Vec::new();
        let mut i = 0;
        out.extend(self.children_of(&Node::Root).iter().cloned());
        while i < out.len() {
            let kids = self.children_of(&out[i]).to_vec();
            out.extend(kids);
            i += 1;
        }
        out
    }

    /// The tree parent of a comment for `t_rank` (§2.2 `treeParent`): its parent comment, or
    /// `None` for the root post (top-level comments, children of a placeholder, re-attached
    /// comments).
    pub fn tree_parent(&self, comment: &str) -> Option<&str> {
        match self.parent.get(comment)? {
            Parent::Comment(u) => Some(u),
            Parent::Root | Parent::Missing(_) => None,
        }
    }
}

/// One comment offered to [`place`]: its uri and `parent.uri`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadComment {
    /// The comment uri.
    pub uri: String,
    /// `parent.uri`, if present.
    pub parent: Option<String>,
}

/// Places the thread of post `root` (§11 steps 1–4). `root_present` is false when p is not a
/// non-Excluded post of the fold input; `comments` are the non-Excluded comments whose
/// `root.uri` is p (duplicates are ignored).
pub fn place(root: &str, root_present: bool, comments: &[ThreadComment]) -> Placement {
    // 1. nodes
    let mut pointer: BTreeMap<String, Option<String>> = BTreeMap::new();
    for c in comments {
        pointer
            .entry(c.uri.clone())
            .or_insert_with(|| c.parent.clone());
    }

    // 2. parent pointers
    let mut parent: BTreeMap<String, Parent> = pointer
        .iter()
        .map(|(c, p)| {
            let pp = match p {
                None => Parent::Root,
                Some(p) if pointer.contains_key(p) => Parent::Comment(p.clone()),
                Some(p) => Parent::Missing(p.clone()),
            };
            (c.clone(), pp)
        })
        .collect();

    // 3. cycles: walk every chain once (white / on the path / done)
    let mut orphaned: BTreeSet<String> = BTreeSet::new();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let uris: Vec<String> = parent.keys().cloned().collect();
    for start in uris {
        if done.contains(&start) {
            continue;
        }
        let mut path: Vec<String> = Vec::new();
        let mut on_path: BTreeMap<String, usize> = BTreeMap::new();
        let mut cur = start;
        loop {
            if done.contains(&cur) {
                break;
            }
            if let Some(&i) = on_path.get(&cur) {
                // the cycle is path[i..]; cut its greatest uri
                if let Some(m) = path[i..].iter().max().cloned() {
                    parent.insert(m.clone(), Parent::Root);
                    orphaned.insert(m);
                }
                break;
            }
            on_path.insert(cur.clone(), path.len());
            path.push(cur.clone());
            match parent.get(&cur) {
                Some(Parent::Comment(next)) => cur = next.clone(),
                _ => break,
            }
        }
        done.extend(path);
    }

    // 4. depth, top-down from the root
    let mut kids: BTreeMap<Node, BTreeSet<Node>> = BTreeMap::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for (c, p) in &parent {
        if let Parent::Missing(m) = p {
            missing.insert(m.clone());
        }
        kids.entry(Node::from(p))
            .or_default()
            .insert(Node::Comment(c.clone()));
    }
    for m in &missing {
        kids.entry(Node::Root)
            .or_default()
            .insert(Node::Missing(m.clone()));
    }
    let mut depth: BTreeMap<String, u32> = BTreeMap::new();
    let mut queue: Vec<(Node, u32)> = vec![(Node::Root, 0)];
    let mut reattached: Vec<String> = Vec::new();
    let mut i = 0;
    while i < queue.len() {
        let (node, d) = queue[i].clone();
        i += 1;
        let Some(children) = kids.get(&node) else {
            continue;
        };
        for child in children {
            let mut cd = d + 1;
            if cd > THREAD_DEPTH_MAX
                && let Node::Comment(c) = child
            {
                reattached.push(c.clone());
                cd = 1;
            }
            if let Some(u) = child.uri() {
                depth.insert(u.to_owned(), cd);
            }
            queue.push((child.clone(), cd));
        }
    }
    for c in reattached {
        parent.insert(c.clone(), Parent::Root);
        orphaned.insert(c);
    }

    // the final children lists: comments, then placeholders, each by uri
    let mut children: BTreeMap<Node, Vec<Node>> = BTreeMap::new();
    for (c, p) in &parent {
        children
            .entry(Node::from(p))
            .or_default()
            .push(Node::Comment(c.clone()));
    }
    for list in children.values_mut() {
        list.sort();
    }
    for m in &missing {
        children
            .entry(Node::Root)
            .or_default()
            .push(Node::Missing(m.clone()));
    }

    Placement {
        root: root.to_owned(),
        root_missing: !root_present,
        parent,
        missing,
        depth,
        orphaned,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(uri: &str, parent: Option<&str>) -> ThreadComment {
        ThreadComment {
            uri: uri.into(),
            parent: parent.map(Into::into),
        }
    }

    #[test]
    fn flat_nested_missing_and_cycles() {
        let p = place(
            "p",
            true,
            &[
                c("a", None),
                c("b", Some("a")),
                c("c", Some("x")),
                c("d", Some("x")),
                c("e", Some("f")),
                c("f", Some("e")),
                c("g", Some("g")),
                c("h", Some("e")),
            ],
        );
        assert_eq!(p.parent["b"], Parent::Comment("a".into()));
        assert_eq!(p.parent["c"], Parent::Missing("x".into()));
        assert_eq!(p.missing, BTreeSet::from(["x".to_owned()]));
        assert_eq!(p.depth["x"], 1);
        assert_eq!(p.depth["d"], 2);
        // e ↔ f: f (greater) is cut; g names itself
        assert_eq!(p.parent["f"], Parent::Root);
        assert_eq!(p.parent["e"], Parent::Comment("f".into()));
        assert_eq!(p.parent["g"], Parent::Root);
        assert_eq!(p.depth["h"], 3);
        assert_eq!(p.orphaned, BTreeSet::from(["f".to_owned(), "g".to_owned()]));
        assert_eq!(p.max_depth(), 3);
        assert_eq!(p.tree_parent("d"), None);
        assert_eq!(p.tree_parent("h"), Some("e"));
        let order: Vec<_> = p
            .top_down()
            .iter()
            .filter_map(|n| n.uri().map(str::to_owned))
            .collect();
        assert_eq!(order[..4], ["a", "f", "g", "x"].map(String::from));
    }

    #[test]
    fn order_independent() {
        let input = vec![
            c("a", Some("c")),
            c("b", Some("a")),
            c("c", Some("b")),
            c("d", Some("c")),
        ];
        let mut rev = input.clone();
        rev.reverse();
        assert_eq!(place("p", false, &input), place("p", false, &rev));
        let p = place("p", false, &input);
        assert!(p.root_missing);
        assert_eq!(p.orphaned, BTreeSet::from(["c".to_owned()]));
    }

    #[test]
    fn depth_cap() {
        let n = THREAD_DEPTH_MAX as usize + 2;
        let uri = |i: usize| format!("c{i:05}");
        let comments: Vec<_> = (0..n)
            .map(|i| ThreadComment {
                uri: uri(i),
                parent: (i > 0).then(|| uri(i - 1)),
            })
            .collect();
        let p = place("p", true, &comments);
        assert_eq!(p.depth[&uri(999)], 1000);
        assert_eq!(p.depth[&uri(1000)], 1);
        assert_eq!(p.depth[&uri(1001)], 2);
        assert_eq!(p.orphaned, BTreeSet::from([uri(1000)]));
        assert_eq!(p.max_depth(), 1000);
    }
}
