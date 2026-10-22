# SPDX-License-Identifier: CC0-1.0
"""Time (social.md §2, magic/1): µs(datetime), tidTime, t_obs, t_eff, t_decl and t_rank.

Reads only validity, obs, decl and thread placement. t_rank is computed for the items that
are not Excluded (V1): posts and other items against t_decl, comments top-down through the
placed tree (NOTES.md 3).
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import min_some, split_uri, tid_int
from .syntax import datetime_us

FUTURE_SKEW_US = 300_000_000             # FUTURE_SKEW_MS = 300,000 (limits.md §9)
TIMESTAMP_INCONSISTENT_US = 3_600_000_000


def tid_time(tid: str) -> int:
    """tidTime(tid) = the TID's 64-bit integer value >> 10 (µs since the epoch)."""
    return tid_int(tid) >> 10


def own_tid(uri: str) -> str:
    """The item's own TID: the rkey TID; for a comment, the last TID segment."""
    return split_uri(uri)[2].split("-")[-1]


def t_obs(uri: str, obs: dict):
    xs = [us for (u, _c), us in obs.items() if u == uri]
    return min(xs) if xs else None


@dataclass(frozen=True)
class ItemTime:
    t_claim: object        # int | None
    t_tid: int
    t_obs: object          # int | None
    t_eff: int
    future: bool
    t_eff2: int            # t_eff'
    inconsistent: bool


def item_time(uri: str, value: dict, obs: dict) -> ItemTime:
    tc = datetime_us(value.get("createdAt"))
    tt = tid_time(own_tid(uri))
    to = t_obs(uri, obs)
    te = min_some(tc, tt)
    fut = to is not None and te > to + FUTURE_SKEW_US
    incons = tc is not None and abs(tc - tt) > TIMESTAMP_INCONSISTENT_US
    return ItemTime(tc, tt, to, te, fut, to if fut else te, incons)


def t_decl(decl, obs: dict) -> int:
    c = datetime_us(decl.value.get("createdAt"))
    c = 0 if c is None else c
    o = t_obs(decl.uri, obs)
    return o if (o is not None and c > o + FUTURE_SKEW_US) else c


def compute_times(items: dict, threads: dict, decl, obs: dict):
    """items: uri -> Record of every non-Excluded item; threads: post uri -> Thread.

    Returns (t_decl, {uri: ItemTime}, {uri: t_rank}).
    """
    td = t_decl(decl, obs)
    it = {u: item_time(u, r.value, obs) for u, r in items.items()}
    tr = {}
    for u, r in items.items():                       # posts, wikiRevisions, sequences first
        if r.coll != "comment":
            tr[u] = max(td, it[u].t_eff2)
    for p, th in threads.items():                    # then comments, top-down
        root_rank = td if th.root_missing else tr[p]
        for n in th.preorder():
            if n in th.missing:
                continue
            tp = th.tree_parent(n)
            lower = root_rank if tp is None else tr[tp]
            tr[n] = max(lower, it[n].t_eff2)
    return td, it, tr
