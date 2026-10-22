# SPDX-License-Identifier: CC0-1.0
"""Vector projections of a ForumView, one per vector kind (testdata/vectors/README.md, "social").

Each projection is a dict of named fields; a vector's `expected` lists a subset of them and a
runner compares each listed field with JSON equality. Fields are computed on demand, so a
vector that lists only L0–L2 fields never touches a stubbed layer.

DID and uri lists are sorted by UTF-8 bytes; maps are JSON objects (key order is irrelevant).
"""

from __future__ import annotations

from .common import ubytes
from .modfold import DIMENSIONS, Resolved
from .participation import sorted_dids
from .policy import algs_line, params_line
from . import digest as _digest


def _sorted(xs):
    return sorted(xs, key=ubytes)


# --------------------------------------------------------------------------- kind "time"

def time_fields(view) -> dict:
    def t_decl():
        return view.times[0]

    def items():
        _, it, tr = view.times
        return {u: {"tRank": tr[u], "futureDated": it[u].future,
                    "inconsistentTimestamps": it[u].inconsistent} for u in tr}
    return {"tDecl": t_decl, "items": items}


# --------------------------------------------------------------------------- kind "modfold"

def _value_json(d, v):
    if d == "restrict":
        return _sorted(v)
    if d == "tag.head":
        return {"uri": v[0], "cid": v[1]}
    return v


def resolved_json(view) -> dict:
    out = {}
    for T, d, res in view.l1.resolved_items():
        if not isinstance(res, Resolved):
            continue                                   # unordered conflict: Default
        e = {"value": _value_json(d, res.value), "rank": res.rank, "winner": res.winner}
        if d == "locked" and res.value is True:
            e["allowed"] = _sorted(view.l1.allowed(T))
        out.setdefault(T, {})[d] = e
    return out


def modfold_fields(view) -> dict:
    def items():
        out = {}
        for u in view.items:
            s = view.mod_state(u)
            if s is None:
                out[u] = {"mod": None}
            else:
                e = {"mod": "Excluded" if s.state == "Excluded" else s.reason}
                if s.removed is not None:
                    rank, actor, spam, reason = s.removed
                    e["removed"] = {"rank": rank, "actor": actor, "spam": spam, "reason": reason}
                out[u] = e
        return out

    def mod_log():
        out = []
        for uri, (status, rank) in view.mod_log:
            e = {"uri": uri, "status": status}
            if rank is not None:
                e["rank"] = rank
            out.append(e)
        return out

    def mod_queue():
        key = {"stale": "uri", "conflict": "subject", "applicant": "did"}
        return [{"type": t, key[t]: k} for t, k in view.mod_queue]

    return {
        "authorities": lambda: dict(sorted(view.l0.ranks.items(), key=lambda kv: ubytes(kv[0]))),
        "degraded": lambda: view.degraded,
        "mode": lambda: view.l2.mode,
        "members": lambda: sorted_dids(view.l2.members),
        "P": lambda: sorted_dids(view.l2.P),
        "G": lambda: sorted_dids(view.l2.G),
        "B": lambda: sorted_dids(view.l2.B),
        "N": lambda: sorted_dids(view.l2.N),
        "S": lambda: sorted_dids(view.l2.S),
        "applicants": lambda: sorted_dids(view.l2.applicants),
        "newcomers": lambda: sorted_dids(view.newcomers),
        "resolved": lambda: resolved_json(view),
        "items": items,
        "modLog": mod_log,
        "modQueue": mod_queue,
    }


# --------------------------------------------------------------------------- kind "thread"

def thread_fields(view) -> dict:
    p = view.query.get("thread")

    def th():
        if p not in view.threads:
            raise KeyError(f"query.thread {p!r} has no thread")
        return view.threads[p]

    return {
        "rootMissing": lambda: th().root_missing,
        "missing": lambda: _sorted(th().missing),
        "parent": lambda: {c: q for c, q in th().parent.items()},
        "depth": lambda: dict(th().depth),
        "orphaned": lambda: _sorted(th().orphaned),
        "maxDepth": lambda: max(th().depth.values(), default=0),
    }


# --------------------------------------------------------------------------- kind "inputDigest"

def digest_fields(view) -> dict:
    return {
        "inputDigest": lambda: view.input_digest,
        "algs": lambda: algs_line(view.policy),
        "params": lambda: params_line(view.policy),
        "preimage": lambda: _digest.preimage(view.input, view.policy).decode("utf-8"),
        "newerRules": lambda: view.policy.newer_rules_banner,
    }


KINDS = {
    "time": time_fields,
    "modfold": modfold_fields,
    "thread": thread_fields,
    "inputDigest": digest_fields,
}

DEFAULT_FIELDS = {
    "time": ("tDecl", "items"),
    "modfold": ("resolved", "items", "modLog"),
    "thread": ("rootMissing", "missing", "parent", "depth", "orphaned"),
    "inputDigest": ("inputDigest", "algs", "params"),
}


def project(view, kind: str, fields=None) -> dict:
    table = KINDS[kind](view)
    names = fields if fields is not None else DEFAULT_FIELDS[kind]
    return {n: table[n]() for n in names}


__all__ = ["project", "KINDS", "DEFAULT_FIELDS", "DIMENSIONS"]
