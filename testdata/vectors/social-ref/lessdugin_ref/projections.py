# SPDX-License-Identifier: CC0-1.0
"""Vector projections of a ForumView (testdata/vectors/README.md, "Social fold vectors").

Every projection field has one name and one JSON shape across all kinds, so that a runner can
implement a single `project(view, field)`. A vector's `expected` lists a subset of the fields
of its file's kind; a runner compares each listed field with JSON equality. Fields are computed
on demand, so a vector that lists only L0–L2 fields never touches a stubbed layer.

Conventions (normative for the vectors):
- DID and uri sets are arrays sorted ascending by UTF-8 bytes.
- Integer maps keyed by DID (K1, seedUp, KP, K) list exactly the DIDs whose value is non-zero.
- Per-item maps are keyed by uri; each field says which items it lists.
- A State is {"state": S, "reason"?: R, "removed"?: {"rank", "actor", "spam", "reason"}}
  (see visibility.State.to_json); "removed.reason" is null when absent or not shown.
"""

from __future__ import annotations

from . import digest as _digest
from . import rank as _rank
from .common import ubytes
from .modfold import DIMENSIONS, Resolved
from .policy import algs_line, params_line


def _sorted(xs) -> list:
    return sorted(xs, key=ubytes)


def _nonzero(m: dict) -> dict:
    return {k: m[k] for k in _sorted(m) if m[k] != 0}


FIELDS: dict = {}          # name -> function(view) -> JSON value
FIELD_KINDS: dict = {}     # name -> the kind whose files may list it


def field(kind: str, name: str):
    def deco(fn):
        assert name not in FIELDS, name
        FIELDS[name] = fn
        FIELD_KINDS[name] = kind
        return fn
    return deco


# =========================================================================== shared (any kind)

@field("*", "degraded")
def _degraded(view):
    return view.degraded


@field("*", "newerRulesBanner")
def _banner(view):
    return view.policy.newer_rules_banner


# =========================================================================== kind "time"

@field("time", "tDecl")
def _t_decl(view):
    return view.times[0]


@field("time", "times")
def _times(view):
    _, it, tr = view.times
    return {u: {"tRank": tr[u], "futureDated": it[u].future,
                "inconsistentTimestamps": it[u].inconsistent} for u in _sorted(tr)}


# =========================================================================== kind "modfold"

@field("modfold", "authorities")
def _authorities(view):
    return {d: view.l0.ranks[d] for d in _sorted(view.l0.ranks)}


@field("modfold", "mode")
def _mode(view):
    return view.l2.mode


for _name in ("members", "P", "G", "B", "N", "S", "applicants"):
    field("modfold", _name)(lambda view, _n=_name: _sorted(getattr(view.l2, _n)))


@field("modfold", "newcomers")
def _newcomers(view):
    return _sorted(view.newcomers)


def _value_json(d, v):
    if d == "restrict":
        return _sorted(v)
    if d == "tag.head":
        return {"uri": v[0], "cid": v[1]}
    return v


@field("modfold", "resolved")
def _resolved(view):
    out = {}
    for T, d, res in view.l1.resolved_items():
        if not isinstance(res, Resolved):
            continue                                   # unordered conflict: Default
        e = {"value": _value_json(d, res.value), "rank": res.rank, "winner": res.winner}
        if d == "locked" and res.value is True:
            e["allowed"] = _sorted(view.l1.allowed(T))
        out.setdefault(T, {})[d] = e
    return out


@field("modfold", "tiers")
def _tiers(view):
    return {u: view.l1.effective(u, "tier") for u, r in view.items.items()
            if r.coll == "post" and not view.excluded[u]}


@field("modfold", "modStates")
def _mod_states(view):
    out = {}
    for u in view.items:
        s = view.mod_state(u)
        out[u] = None if s is None else s.to_json()
    return out


@field("modfold", "modLog")
def _mod_log(view):
    out = []
    for uri, (status, rank) in view.mod_log:
        e = {"uri": uri, "status": status}
        if rank is not None:
            e["rank"] = rank
        out.append(e)
    return out


@field("modfold", "modQueue")
def _mod_queue(view):
    key = {"stale": "uri", "conflict": "subject", "applicant": "did"}
    return [{"type": t, key[t]: k} for t, k in view.mod_queue]


# =========================================================================== kind "thread"

def _thread(view):
    p = view.query.get("thread")
    if p not in view.threads:
        raise KeyError(f"query.thread {p!r} has no thread")
    return view.threads[p]


@field("thread", "rootMissing")
def _root_missing(view):
    return _thread(view).root_missing


@field("thread", "missing")
def _missing(view):
    return _sorted(_thread(view).missing)


@field("thread", "parent")
def _parent(view):
    th = _thread(view)
    return {c: th.parent[c] for c in _sorted(th.parent)}


@field("thread", "depth")
def _depth(view):
    th = _thread(view)
    return {n: th.depth[n] for n in _sorted(th.depth)}


@field("thread", "orphaned")
def _orphaned(view):
    return _sorted(_thread(view).orphaned)


@field("thread", "maxDepth")
def _max_depth(view):
    return max(_thread(view).depth.values(), default=0)


# =========================================================================== kind "digest"

@field("digest", "inputDigest")
def _input_digest(view):
    return view.input_digest


@field("digest", "digestPreimage")
def _preimage(view):
    return _digest.preimage(view.input, view.policy).decode("utf-8")


@field("digest", "algs")
def _algs(view):
    return algs_line(view.policy)


@field("digest", "params")
def _params(view):
    return params_line(view.policy)


# =========================================================================== kind "karma" (next stage)

@field("karma", "counting")
def _counting(view):
    return _sorted(view.l3.counting)


for _name in ("K1", "seedUp", "KP", "K"):
    field("karma", _name)(lambda view, _n=_name: _nonzero(getattr(view.l3, _n)))


@field("karma", "E")
def _e(view):
    return _sorted(view.l3.E)


@field("karma", "established")
def _established(view):
    return _sorted(view.l3.E - view.l2.S)


@field("karma", "scores")
def _scores(view):
    out = {}
    for u in _sorted(view.l35):
        s = view.l35[u]
        out[u] = {"score": s.score, "agreement": s.agreement, "unweighted": s.unweighted,
                  "earlier": s.earlier, "weighted": s.weighted}
    return out


# =========================================================================== kind "reacts" (next stage)

@field("reacts", "reacts")
def _reacts(view):
    out = {}
    for subj in _sorted(view.l5_reacts):
        out[subj] = [{"react": t.react, "qkey": t.qkey, "creator": t.creator,
                      "supporters": list(t.supporters), "opposers": list(t.opposers)}
                     for t in view.l5_reacts[subj]]
    return out


# =========================================================================== kind "visibility" (next stage)

@field("visibility", "states")
def _states(view):
    return {u: view.states[u].to_json() for u in view.items}


@field("visibility", "displayed")
def _displayed(view):
    return {u: view.displayed[u].to_json() for u in view.items}


# =========================================================================== kind "rank" (next stage)

@field("rank", "postOrder")
def _post_order(view):
    return list(_rank.post_order(view, view.query["posts"]))


@field("rank", "hotBits")
def _hot_bits(view):
    hv = _rank.hot_values(view, view.query["posts"])
    return {u: _rank.hot_bits(hv[u]) for u in _sorted(hv)}


@field("rank", "commentOrder")
def _comment_order(view):
    q = view.query["comments"]
    return list(_rank.comment_order(view, q["thread"], q["sort"]))


@field("rank", "recentDiscussion")
def _recent_discussion(view):
    return [{"post": p, "comments": list(cs)} for p, cs in _rank.recent_discussion(view)]


# =========================================================================== kinds

DEFAULT_FIELDS = {
    "time": ("tDecl", "times"),
    "modfold": ("resolved", "modStates", "modLog"),
    "thread": ("rootMissing", "missing", "parent", "depth", "orphaned"),
    "digest": ("inputDigest", "digestPreimage", "algs", "params"),
    "karma": ("K1", "seedUp", "E", "KP", "K", "scores"),
    "reacts": ("reacts",),
    "visibility": ("states", "displayed"),
    "rank": ("postOrder",),
}


def project(view, kind: str, fields=None) -> dict:
    names = fields if fields is not None else DEFAULT_FIELDS[kind]
    out = {}
    for n in names:
        if FIELD_KINDS[n] not in ("*", kind):
            raise ValueError(f"field {n} belongs to kind {FIELD_KINDS[n]}, not {kind}")
        out[n] = FIELDS[n](view)
    return out


__all__ = ["project", "FIELDS", "FIELD_KINDS", "DEFAULT_FIELDS", "DIMENSIONS"]
