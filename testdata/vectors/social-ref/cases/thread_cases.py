# SPDX-License-Identifier: CC0-1.0
"""thread/thread.json (kind "thread"): thread placement, social.md §11 steps 1–4.

Written here because testdata/vectors/thread/ did not exist when this stage ran (the Rust engine
agent may own that directory later; see NOTES.md).
"""

from __future__ import annotations

from lessdugin_ref.build import HOUR, MIN, SEC, T0, Forum, cid, nsid, rec

from .util import file_doc, vector


def u(r):
    return r if isinstance(r, str) else r["uri"]


def V(name, f, records, p, note, *, fields=None, invalid=None, fi=None):
    return vector("thread", name, f.fold_input(records, **(fi or {})), fields=fields,
                  query={"thread": u(p)}, note=note, invalid=invalid)


def fake_cid(label: str) -> str:
    """A syntactically valid CID for a parent reference whose version does not matter (placement
    never depends on CIDs; a cycle cannot carry consistent CIDs)."""
    return cid({"$type": nsid("comment"), "label": label})


def thread_vectors() -> list:
    out = []
    f = Forum()

    def comment(who, root, t, parent=None, clock=1):
        par = None
        if parent is not None:
            par = (u(parent), parent["cid"] if isinstance(parent, dict) else fake_cid(u(parent)))
        return f.comment(who, root, t, parent=par, clock=clock)

    # ------------------------------------------------------------------ baseline
    p = f.post("alice", T0)
    c1 = comment("bob", p, T0 + HOUR)
    c2 = comment("carol", p, T0 + 2 * HOUR)
    c3 = comment("dave", p, T0 + 3 * HOUR, parent=c1)
    c4 = comment("erin", p, T0 + 4 * HOUR, parent=c3)
    v = V("flat-and-nested-replies", f, [p, c1, c2, c3, c4], p,
          "parent absent → the root (depth 1); parent a node of this thread → that comment", fields=(
              "rootMissing", "missing", "parent", "depth", "orphaned", "maxDepth"))
    assert v["expected"]["parent"] == {u(c1): u(p), u(c2): u(p), u(c3): u(c1), u(c4): u(c3)}
    assert v["expected"]["depth"] == {u(c1): 1, u(c2): 1, u(c3): 2, u(c4): 3} and v["expected"]["maxDepth"] == 3
    out.append(v)

    # ------------------------------------------------------------------ cycles (step 3)
    p = f.post("alice", T0)
    xb = f.comment_uri("bob", u(p), T0 + HOUR, 1)
    xc = f.comment_uri("carol", u(p), T0 + 2 * HOUR, 1)
    cb = comment("bob", p, T0 + HOUR, parent=xc)
    cc = comment("carol", p, T0 + 2 * HOUR, parent=xb)
    kid_b = comment("dave", p, T0 + 3 * HOUR, parent=cb)
    kid_c = comment("erin", p, T0 + 4 * HOUR, parent=cc)
    v = V("two-cycle", f, [p, cb, cc, kid_b, kid_c], p,
          "bob's and carol's comments name each other as parent (parent can be edited): the member with "
          "the greatest uri (carol's) is re-attached to the root and flagged Orphaned; bob's hangs under it")
    e = v["expected"]
    assert e["orphaned"] == [u(cc)] and e["parent"][u(cc)] == u(p) and e["parent"][u(cb)] == u(cc)
    assert e["depth"] == {u(cc): 1, u(cb): 2, u(kid_b): 3, u(kid_c): 2}
    out.append(v)

    p = f.post("alice", T0)
    xs = {n: f.comment_uri(n, u(p), T0 + i * HOUR, 1) for i, n in enumerate(("bob", "carol", "dave"), 1)}
    cx = comment("bob", p, T0 + HOUR, parent=xs["carol"])
    cy = comment("carol", p, T0 + 2 * HOUR, parent=xs["dave"])
    cz = comment("dave", p, T0 + 3 * HOUR, parent=xs["bob"])
    v = V("three-cycle", f, [p, cx, cy, cz], p,
          "bob → carol → dave → bob: dave's comment (greatest uri) is cut and re-attached to the root; the "
          "rest of the cycle continues below it")
    assert v["expected"]["orphaned"] == [u(cz)]
    assert v["expected"]["depth"] == {u(cz): 1, u(cy): 2, u(cx): 3}
    out.append(v)

    p = f.post("alice", T0)
    xs_ = f.comment_uri("bob", u(p), T0 + HOUR, 1)
    cs = comment("bob", p, T0 + HOUR, parent=xs_)
    kid = comment("carol", p, T0 + 2 * HOUR, parent=cs)
    v = V("self-parent", f, [p, cs, kid], p,
          "a comment that names itself as parent is a cycle of length 1: re-attached to the root, Orphaned")
    assert v["expected"]["orphaned"] == [u(cs)] and v["expected"]["depth"] == {u(cs): 1, u(kid): 2}
    out.append(v)

    p = f.post("alice", T0)
    xa = f.comment_uri("bob", u(p), T0 + HOUR, 1)
    xb2 = f.comment_uri("carol", u(p), T0 + 2 * HOUR, 1)
    ca = comment("bob", p, T0 + HOUR, parent=xb2)
    cb2 = comment("carol", p, T0 + 2 * HOUR, parent=xa)
    tail = comment("zed", p, T0 + 3 * HOUR, parent=ca)
    tail2 = comment("yves", p, T0 + 4 * HOUR, parent=tail)
    v = V("cycle-with-a-tail", f, [p, ca, cb2, tail, tail2], p,
          "zed's and yves's comments lead into the bob ↔ carol cycle but are not on it: only the cycle's "
          "greatest uri (carol's) is cut, although zed's uri is greater")
    assert u(tail) > u(cb2) and v["expected"]["orphaned"] == [u(cb2)]
    assert v["expected"]["depth"] == {u(cb2): 1, u(ca): 2, u(tail): 3, u(tail2): 4}
    out.append(v)

    p = f.post("alice", T0)
    a1 = f.comment_uri("bob", u(p), T0 + HOUR, 1)
    a2 = f.comment_uri("carol", u(p), T0 + 2 * HOUR, 1)
    b1 = f.comment_uri("dave", u(p), T0 + 3 * HOUR, 1)
    b2 = f.comment_uri("erin", u(p), T0 + 4 * HOUR, 1)
    recs = [comment("bob", p, T0 + HOUR, parent=a2), comment("carol", p, T0 + 2 * HOUR, parent=a1),
            comment("dave", p, T0 + 3 * HOUR, parent=b2), comment("erin", p, T0 + 4 * HOUR, parent=b1)]
    v = V("two-separate-cycles", f, [p, *recs], p,
          "each cycle is cut independently at its own greatest uri")
    assert v["expected"]["orphaned"] == [a2, b2]
    out.append(v)

    # ------------------------------------------------------------------ Missing placeholders (step 2)
    p = f.post("alice", T0)
    gx = f.comment_uri("zed", u(p), T0 + 5 * HOUR, 7)
    gy = f.comment_uri("yves", u(p), T0 + 6 * HOUR, 8)
    m1 = comment("bob", p, T0 + HOUR, parent=gx)
    m2 = comment("carol", p, T0 + 2 * HOUR, parent=gx)
    m3 = comment("dave", p, T0 + 3 * HOUR, parent=gy)
    m4 = comment("erin", p, T0 + 4 * HOUR, parent=m1)
    v = V("missing-parent-placeholders", f, [p, m1, m2, m3, m4], p,
          "parents that are not in records become Missing placeholders, one per distinct uri, each a "
          "child of the root (depth 1); their children have depth 2", fields=(
              "rootMissing", "missing", "parent", "depth", "orphaned", "maxDepth"))
    e = v["expected"]
    assert e["missing"] == sorted([gx, gy], key=str.encode)
    assert e["depth"] == {gx: 1, gy: 1, u(m1): 2, u(m2): 2, u(m3): 2, u(m4): 3}
    assert e["parent"] == {u(m1): gx, u(m2): gx, u(m3): gy, u(m4): u(m1)}
    out.append(v)

    p = f.post("alice", T0)
    q = f.post("bob", T0 + MIN)
    d = comment("carol", q, T0 + HOUR)
    c = comment("dave", p, T0 + 2 * HOUR, parent=d)
    v = V("cross-root-parent", f, [p, q, d, c], p,
          "dave's comment in p's thread names as parent a comment of another thread (q): in p's thread "
          "that parent is a Missing placeholder")
    assert v["expected"]["missing"] == [u(d)] and v["expected"]["depth"] == {u(d): 1, u(c): 2}
    out.append(v)

    p = f.post("alice", T0)
    bad = comment("bob", p, T0 + HOUR)
    bad = rec(bad["uri"], {**bad["value"], "body": {"$type": nsid("markdown")}})
    c = comment("carol", p, T0 + 2 * HOUR, parent=bad)
    v = V("excluded-parent-is-missing", f, [p, bad, c], p,
          "bob's comment is invalid (its body has neither text nor textBlob: G7) and therefore Excluded, "
          "so it is not a node; its reply hangs under a Missing placeholder for its uri",
          invalid={u(bad): "G7"})
    assert v["expected"]["missing"] == [u(bad)] and v["expected"]["parent"] == {u(c): u(bad)}
    out.append(v)

    # ------------------------------------------------------------------ root (step 1)
    p = f.post("alice", T0)
    c1 = comment("bob", p, T0 + HOUR)
    c2 = comment("carol", p, T0 + 2 * HOUR, parent=c1)
    v = V("root-missing", f, [c1, c2], p,
          "the root post is not in records: the thread's root is Missing(p) and the comments are placed "
          "under it as usual")
    assert v["expected"]["rootMissing"] is True and v["expected"]["missing"] == []
    assert v["expected"]["parent"] == {u(c1): u(p), u(c2): u(c1)}
    out.append(v)

    p = f.post("alice", T0, title="　")
    c1 = comment("bob", p, T0 + HOUR)
    v = V("root-excluded", f, [p, c1], p,
          "the root post's title is only White_Space (PO3), so the post is Excluded; an Excluded root is "
          "treated as not in records: Missing(p) root (NOTES.md)", invalid={u(p): "PO3"})
    assert v["expected"]["rootMissing"] is True
    out.append(v)

    return out


def depth_vectors() -> list:
    out = []
    f = Forum()
    p = f.post("alice", T0)
    chain, prev = [], None
    for i in range(1002):
        c = f.comment("bob", p, T0 + (i + 1) * SEC, parent=None if prev is None else (prev["uri"], prev["cid"]),
                      clock=0, text="")
        chain.append(c)
        prev = c
    v = V("depth-1001-is-re-attached", f, [p, *chain], p,
          "a chain of 1,002 comments: the 1,000th has depth 1000 and stays; the 1,001st would exceed "
          "THREAD_DEPTH_MAX = 1000, so it is re-attached to the root (Orphaned, depth 1) and its child "
          "continues at depth 2 (were it not, the 1,002nd would be re-attached too). obs is empty: "
          "placement does not read it", fields=("rootMissing", "missing", "orphaned", "maxDepth"),
          fi={"obs": {}})
    assert v["expected"]["orphaned"] == [u(chain[1000])] and v["expected"]["maxDepth"] == 1000
    out.append(v)
    return out


FILES = [
    ("thread/thread.json", lambda: file_doc(
        "mod/1", "thread",
        "social.md §11 thread placement, steps 1–4: nodes (non-Excluded comments with root.uri = p), "
        "parent pointers with Missing placeholders, the cycle cut at the greatest uri, and the depth "
        "cap. input: {fold: FoldInput, query: {thread: p}}. expected: rootMissing, missing, parent, "
        "depth, orphaned, maxDepth for the thread of p (testdata/vectors/README.md, social section).",
        thread_vectors())),
    ("thread/depth.json", lambda: file_doc(
        "mod/1", "thread",
        "social.md §11 step 4, the depth cap THREAD_DEPTH_MAX = 1000 (limits.md), in a file of its own "
        "because its input is large. Same kind and shapes as thread/thread.json.",
        depth_vectors())),
]
