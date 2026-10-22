# SPDX-License-Identifier: CC0-1.0
"""L6: ranking, sorts, filters and Recent Discussion (social.md §12, magic/1).

Interface (projections.py calls it):

    hot(b: int, t: int, now: int, policy) -> float
        §12.1, operations in exactly that order (age = max(now − t, 0) as float; h = age /
        3_600_000_000.0; base = h + offsetHours; e = exponentMilli / 1000.0; b / pow(base, e)).
    hot_bits(x: float) -> str
        16 lowercase hex digits of the binary64 bit pattern.
    post_order(view, q: dict) -> tuple of post uris
        q is the vector query's "posts" member: {"sort": "magic" | "top" | "new" | "old" |
        "recentComments", "filter": "frontpage" | "all" | "curated", "tag"?: slug,
        "kind"?: effective kind, "author"?: did}. Listed posts only (§10.1: state Visible or
        Collapsed; a Tombstone is not listed), §12.2–§12.3.
    hot_values(view, q: dict) -> dict post uri -> float
        hot(b(p), t_rank(p)) of every post in post_order(view, q) with b(p) > 0 (magic only).
    comment_order(view, p: str, sort: str) -> tuple of node uris
        §12.4 for the placed tree of post p, in pre-order with the siblings of every level
        sorted: thread pins first (forum tier, else the author tier), then the sort key, then
        uri; Missing placeholders after all comment siblings, by uri. The displayed nodes are
        (§10.2): comments that are listed or Tombstone, and Missing placeholders with a listed
        descendant. sort is magic | top | new | old | latestReply.
    recent_discussion(view) -> tuple of (post uri, tuple of up to 3 comment uris)  §12.5

Python's math.pow may differ from libm::pow in the last bit; rank vectors therefore carry
"hotBitsSource": "python-math.pow" at file level (testdata/vectors/README.md, social section).

Reads L0–L5 (tiers and forum pins from L1, scores from L3.5, author thread pins from L5,
tag application from L5 tags only when a tag filter is asked), the L6 states (visibility.py),
thread placement and t_rank. Readings of the spec are in ../NOTES.md ("Comment order",
"Activity and Recent Discussion", "Rank queries").
"""

from __future__ import annotations

import math
import struct

from .common import did_of, sat_add, ubytes

DAY_US = 86_400_000_000
HOUR_US = 3_600_000_000.0
POST_SORTS = ("magic", "top", "new", "old", "recentComments")
COMMENT_SORTS = ("magic", "top", "new", "old", "latestReply")
FILTERS = ("frontpage", "all", "curated")
POST_KINDS = ("article", "link", "question", "quickTake")
TIER_LEVEL = {"personal": 0, "frontpage": 1, "curated": 2}
POST_QUERY_KEYS = {"sort", "filter", "tag", "kind", "author"}


def hot_bits(x: float) -> str:
    return struct.pack(">d", x).hex()


def total_key(x: float) -> int:
    """f64::total_cmp as an integer sort key (§1.3 rule 3)."""
    b = struct.unpack(">q", struct.pack(">d", x))[0]
    return b ^ 0x7FFF_FFFF_FFFF_FFFF if b < 0 else b


def hot(b: int, t: int, now: int, policy) -> float:
    age = float(max(now - t, 0))                    # exact: < 2^53
    h = age / HOUR_US
    base = h + float(policy.offsetHours)
    e = float(policy.exponentMilli) / 1000.0
    return float(b) / math.pow(base, e)


# --------------------------------------------------------------------------- per-item values

def t_rank(view, u: str) -> int:
    return view.times[2][u]


def score(view, u: str) -> int:
    return view.l35[u].score


def effective_kind(value: dict) -> str:
    k = value.get("kind")
    return k if k in POST_KINDS else "article"            # PO2


def bonus_score(view, p: str) -> int:
    """b(p) = score(p) + (tier = curated ? curatedBonus : 0) + (tier ≥ frontpage ? frontpageBonus : 0)."""
    pol = view.policy
    tier = view.l1.effective(p, "tier")
    b = score(view, p)
    if tier == "curated":
        b = sat_add(b, pol.curatedBonus)
    if TIER_LEVEL[tier] >= TIER_LEVEL["frontpage"]:
        b = sat_add(b, pol.frontpageBonus)
    return b


def forum_pinned(view, p: str) -> bool:
    return view.l1.effective(p, "pin.forum") is True


def thread_pinned(view, c: str) -> bool:
    """effective(c, pin[thread]), or the author-tier pin when the forum tier is Default (§12.4)."""
    if view.l1.resolve(c, "pin.thread") is not None:
        return view.l1.effective(c, "pin.thread") is True
    return view.l5_author.thread_pin(c) is True


def _hot_key(view, b: int, t: int):
    """Shared magic key after the pins: b > 0 first; hot desc for b > 0, else (b desc, t desc)."""
    if b > 0:
        return (0, -total_key(hot(b, t, view.input.now, view.policy)))
    return (1, -b, -t)


# --------------------------------------------------------------------------- posts (§12.2, §12.3)

def listed_posts(view) -> list:
    disp = view.displayed
    return [u for u, r in view.items.items() if r.coll == "post" and disp[u].listed]


def _check_post_query(q: dict) -> None:
    if not isinstance(q, dict) or set(q) - POST_QUERY_KEYS or "sort" not in q or "filter" not in q:
        raise ValueError(f"posts query must be {{sort, filter, tag?, kind?, author?}}: {q!r}")
    if q["sort"] not in POST_SORTS or q["filter"] not in FILTERS:
        raise ValueError(f"unknown sort or filter: {q!r}")
    if "kind" in q and q["kind"] not in POST_KINDS:
        raise ValueError(f"unknown kind: {q!r}")


def _passes_filter(view, p: str, q: dict) -> bool:
    tier = view.l1.effective(p, "tier")
    kind = effective_kind(view.items[p].value)
    f = q["filter"]
    if f == "frontpage" and not (TIER_LEVEL[tier] >= TIER_LEVEL["frontpage"] and kind != "quickTake"):
        return False
    if f == "curated" and not (tier == "curated" and kind != "quickTake"):
        return False
    if "kind" in q and kind != q["kind"]:
        return False
    if "author" in q and did_of(p) != q["author"]:
        return False
    if "tag" in q and not view.l5_tags.applies(p, q["tag"]):
        return False
    return True


def activity(view, p: str) -> int:
    """max({t_rank(p)} ∪ {t_rank(c) : c in p's thread, state Visible})."""
    a = t_rank(view, p)
    th = view.threads.get(p)
    if th is not None:
        for c in th.parent:
            if view.states[c].state == "Visible":
                a = max(a, t_rank(view, c))
    return a


def post_order(view, q: dict) -> tuple:
    _check_post_query(q)
    posts = [p for p in listed_posts(view) if _passes_filter(view, p, q)]
    sort = q["sort"]
    if sort == "magic":
        cutoff = view.policy.cutoffDays * DAY_US
        now = view.input.now
        posts = [p for p in posts if forum_pinned(view, p) or now - t_rank(view, p) <= cutoff]

        def key(p):
            return (not forum_pinned(view, p), _hot_key(view, bonus_score(view, p), t_rank(view, p)),
                    ubytes(p))
    elif sort == "top":
        def key(p):
            return (-score(view, p), -t_rank(view, p), ubytes(p))
    elif sort == "new":
        def key(p):
            return (-t_rank(view, p), ubytes(p))
    elif sort == "old":
        def key(p):
            return (t_rank(view, p), ubytes(p))
    else:                                                       # recentComments
        def key(p):
            return (-activity(view, p), ubytes(p))
    return tuple(sorted(posts, key=key))


def hot_values(view, q: dict) -> dict:
    if q.get("sort") != "magic":
        raise ValueError("hot values are defined for the magic post sort only")
    out = {}
    for p in post_order(view, q):
        b = bonus_score(view, p)
        if b > 0:
            out[p] = hot(b, t_rank(view, p), view.input.now, view.policy)
    return out


# --------------------------------------------------------------------------- comments (§12.4)

def _displayed_nodes(view, th) -> set:
    """§10.2: listed or Tombstone comments, and Missing placeholders with a listed descendant."""
    disp = view.displayed
    below = {}
    order, stack = [], [th.root]
    while stack:
        x = stack.pop()
        order.append(x)
        stack.extend(th.children.get(x, []))
    for x in reversed(order):
        below[x] = any((c not in th.missing and disp[c].listed) or below[c]
                       for c in th.children.get(x, []))
    out = set()
    for n in th.depth:
        if n in th.missing:
            if below[n]:
                out.add(n)
        elif disp[n].listed or disp[n].state == "Tombstone":
            out.add(n)
    return out


def comment_order(view, p: str, sort: str) -> tuple:
    if sort not in COMMENT_SORTS:
        raise ValueError(f"unknown comment sort: {sort!r}")
    if p not in view.threads:
        raise KeyError(f"no thread for {p!r}")
    th = view.threads[p]
    shown = _displayed_nodes(view, th)

    # Bottom-up over the placed tree: vis_below[n] = max t_rank of n's Visible descendants.
    order, stack = [], [th.root]
    while stack:
        x = stack.pop()
        order.append(x)
        stack.extend(th.children.get(x, []))
    vis_below: dict = {}
    for x in reversed(order):
        best = None
        for d in th.children.get(x, []):
            own = d not in th.missing and view.states[d].state == "Visible"
            for cand in (vis_below[d], t_rank(view, d) if own else None):
                if cand is not None:
                    best = cand if best is None else max(best, cand)
        vis_below[x] = best

    def key(c):
        if c in th.missing:
            return (1, ubytes(c))
        t = t_rank(view, c)
        if sort == "magic":
            k = _hot_key(view, score(view, c), t)
        elif sort == "top":
            k = (-score(view, c), -t)
        elif sort == "new":
            k = (-t,)
        elif sort == "old":
            k = (t,)
        else:                                                   # latestReply
            d = vis_below[c]
            k = (-(t if d is None else max(t, d)),)
        return (0, not thread_pinned(view, c), k, ubytes(c))

    # Pre-order with sorted siblings (iterative: the placed tree is up to 1,000 deep).
    out = []
    stack = [th.root]
    while stack:
        n = stack.pop()
        if n != th.root:
            out.append(n)
        kids = sorted((c for c in th.children.get(n, []) if c in shown), key=key)
        stack.extend(reversed(kids))
    return tuple(out)


# --------------------------------------------------------------------------- Recent Discussion (§12.5)

def recent_discussion(view) -> tuple:
    rows = []
    for p in listed_posts(view):
        th = view.threads.get(p)
        if th is None:
            continue
        vis = [c for c in th.parent if view.states[c].state == "Visible"]
        if not vis:
            continue
        vis.sort(key=lambda c: (-t_rank(view, c), ubytes(c)))
        rows.append((p, tuple(vis[:3]), t_rank(view, vis[0])))
    rows.sort(key=lambda r: (-r[2], ubytes(r[0])))
    return tuple((p, cs) for p, cs, _ in rows)
