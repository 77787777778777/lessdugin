# SPDX-License-Identifier: CC0-1.0
"""Thread placement (social.md §11, steps 1–4). Reads only validity (V1) and records.

place_thread(p, comments, root_present) places the non-Excluded comments whose root.uri = p:
parent pointers with Missing placeholders, the cycle cut (greatest uri re-attached to the root,
flagged Orphaned) and the depth cap (a comment whose depth would exceed 1,000 is re-attached to
the root, flagged Orphaned; its subtree continues from depth 1).
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import ubytes

THREAD_DEPTH_MAX = 1000


@dataclass
class Thread:
    root: str               # the post uri p
    root_missing: bool      # True: Missing(p) root (p is not a non-Excluded post in records)
    parent: dict            # comment uri -> p | comment uri | placeholder uri (after steps 3–4)
    depth: dict             # comment and placeholder uri -> depth (root children have depth 1)
    orphaned: frozenset     # comment uris re-attached to the root by step 3 or step 4
    missing: frozenset      # Missing placeholder uris (children of the root)
    children: dict          # node uri (p, comment, placeholder) -> [child uris] sorted by uri

    def tree_parent(self, c: str):
        """treeParent(c) (social.md §2.2): a comment uri, or None for the root post."""
        q = self.parent[c]
        return None if q == self.root or q in self.missing else q

    def preorder(self):
        """Comment and placeholder uris in pre-order, children by uri (a convenience order)."""
        out, stack = [], list(reversed(self.children.get(self.root, [])))
        while stack:
            n = stack.pop()
            out.append(n)
            stack.extend(reversed(self.children.get(n, [])))
        return out


def place_thread(p: str, comments: dict, root_present: bool) -> Thread:
    """comments: uri -> value of every non-Excluded comment with root.uri = p."""
    nodes = set(comments)
    ptr, missing = {}, set()
    for c in sorted(nodes, key=ubytes):
        par = comments[c].get("parent")
        if par is None:
            ptr[c] = p
        elif par["uri"] in nodes:
            ptr[c] = par["uri"]
        else:
            ptr[c] = par["uri"]
            missing.add(par["uri"])

    # Step 3: cut every cycle at its greatest-uri member. Pointers form a functional graph,
    # so each component has at most one cycle; colouring finds each exactly once.
    orphaned = set()
    state = {}                                    # 1 = on the current path, 2 = done
    for start in sorted(nodes, key=ubytes):
        path, n = [], start
        while n in nodes and state.get(n) is None:
            state[n] = 1
            path.append(n)
            n = ptr[n]
        if n in nodes and state.get(n) == 1:      # closed a new cycle at n
            cyc = path[path.index(n):]
            m = max(cyc, key=ubytes)
            ptr[m] = p
            orphaned.add(m)
        for x in path:
            state[x] = 2

    # Step 4: depths top-down; re-attach beyond THREAD_DEPTH_MAX.
    kids = {}
    for c, q in ptr.items():
        kids.setdefault(q, []).append(c)
    for u in missing:
        kids.setdefault(p, []).append(u)
    depth = {}
    stack = [(u, 1) for u in kids.get(p, [])]
    while stack:
        n, d = stack.pop()
        if d > THREAD_DEPTH_MAX:
            ptr[n] = p
            orphaned.add(n)
            d = 1
        depth[n] = d
        for c in kids.get(n, []):
            stack.append((c, d + 1))

    children = {}
    for c, q in ptr.items():
        children.setdefault(q, []).append(c)
    for u in missing:
        children.setdefault(p, []).append(u)
    for k in children:
        children[k].sort(key=ubytes)
    assert set(depth) == nodes | missing, "every node is reachable after the cycle cut"
    return Thread(root=p, root_missing=not root_present, parent=ptr, depth=depth,
                  orphaned=frozenset(orphaned), missing=frozenset(missing), children=children)
