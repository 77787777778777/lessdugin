# SPDX-License-Identifier: CC0-1.0
"""L3: vote power and karma/1 (social.md §6), on top of opinion validity (§5, opinions.py).

Interface (fold.py, visibility.py, scores.py, gates.py, reacts.py, tags.py and projections.py
call it; keep it):

    compute_karma(view) -> Karma
        Reads view.input (records, absent), view.validity, view.excluded (V1), view.policy,
        view.F, view.l1 (spam, restrict), view.l2 (P, B, N, S). Never L3.5 or above.

    Karma.counting   dict opinion uri -> Counted (opinions.py), for every opinion that counts
                     (§5 O1–O5, including the absent-subject branch of O2), in uri order
    Karma.V          tuple of opinion uris in V (§6.2): counting, karma axis present and
                     enabled, subject collection post or comment (absent subjects included)
    Karma.K1, .seedUp, .KP, .K
                     dict did -> int; a missing key reads as 0 (use .get(d, 0)); entries
                     whose value is 0 may be present
    Karma.E          frozenset of eligible voters (S ⊆ E)

    Counted.voter        did(v.uri)
    Counted.subject      v.subject.uri
    Counted.subject_cid  v.subject.cid (drives "earlier version" only, never counting)
    Counted.absent       v.subject.uri ∈ absent: counts toward K1/KP/K only, and only under the
                         per-stage 2-voter rule of §6.2 (included(v, Elig))
    Counted.karma        up | down | strongUp | strongDown, or None when the karma axis is
                         absent, unknown (OP3) or disabled (voter restricted from `vote`)
    Counted.agreement    agree | disagree | strongAgree | strongDisagree, or None when absent,
                         unknown, disabled (restricted from `vote`) or policy.voting = karmaOnly
    The reacts and tags axes are read from the record by reacts.py / tags.py, which check
    opinions.axis_enabled(view, c, "reacts" | "tags") and then apply their own entry rules
    (palette and stance, OP3/OP4, post subjects only).

pairsum() keeps every per-pair net sum and cap (Stage.pairs) so that the property checks of
§6.3 (P5) can inspect them; the vectors project only K1, seedUp, E, KP and K.
"""

from __future__ import annotations

from dataclasses import dataclass

from . import opinions
from .common import sat, sat_add, sat_mul, ubytes
from .opinions import Counted

LADDER = ((500000, 16), (250000, 15), (175000, 14), (100000, 13), (75000, 12), (50000, 11),
          (25000, 10), (10000, 9), (5000, 8), (2500, 7), (1000, 6), (500, 5), (250, 4), (100, 3),
          (10, 2))
KARMA_COLLECTIONS = ("post", "comment")     # votes on wikiRevisions and sequences earn no karma


def small(k: int) -> int:
    return 2 if k >= 1000 else 1


def strong(k: int) -> int:
    for t, p in LADDER:
        if k >= t:
            return p
    return 1


def power(k: int, x: str) -> int:
    k = max(k, 0)
    if x in ("up", "agree"):
        return small(k)
    if x in ("down", "disagree"):
        return -small(k)
    if x in ("strongUp", "strongAgree"):
        return strong(k)
    if x in ("strongDown", "strongDisagree"):
        return -strong(k)
    raise ValueError(x)


def sign(x: str) -> int:
    """sign(x) of §6.1: +1 for up, strongUp; −1 for down, strongDown."""
    if x in ("up", "strongUp"):
        return 1
    if x in ("down", "strongDown"):
        return -1
    raise ValueError(x)


def clamp(x: int, lo: int, hi: int) -> int:
    return max(lo, min(hi, x))


@dataclass(frozen=True)
class Stage:
    """One pairsum stage: out[r] and, per (voter, recipient), the net sum x and the cap."""
    out: dict             # did -> int
    pairs: dict           # (voter, recipient) -> (x, cap); in (voter, recipient) byte order


@dataclass(frozen=True)
class Karma:
    counting: dict
    K1: dict
    seedUp: dict
    E: frozenset
    KP: dict
    K: dict
    V: tuple = ()
    stages: tuple = ()    # (K1 stage, KP stage, K stage): Stage objects, for property checks


def included_fn(V: list, elig: frozenset):
    """included(v, Elig) of §6.2, for the opinions in V (a list of Counted)."""
    voters = {}
    for v in V:
        if v.absent and v.voter in elig:
            voters.setdefault(v.subject, set()).add(v.voter)

    def included(v: Counted) -> bool:
        return v.voter in elig and (not v.absent or len(voters.get(v.subject, ())) >= 2)
    return included


def pairsum(V: list, basis, elig: frozenset, pair_cap: int) -> Stage:
    """pairsum(basis, Elig) of §6.2; basis None (stage 1) or a dict did -> int."""
    inc = included_fn(V, elig)
    acc = {}
    for v in V:
        if not inc(v):
            continue
        w = sign(v.karma) if basis is None else power(basis.get(v.voter, 0), v.karma)
        key = (v.voter, v.recipient)
        acc[key] = sat_add(acc.get(key, 0), w)
    out, pairs = {}, {}
    for (u, r) in sorted(acc, key=lambda k: (ubytes(k[0]), ubytes(k[1]))):
        x = acc[(u, r)]
        cap = pair_cap if basis is None else sat_mul(pair_cap, strong(max(basis.get(u, 0), 0)))
        out[r] = sat_add(out.get(r, 0), clamp(x, -cap, cap))
        pairs[(u, r)] = (x, cap)
    return Stage(out=out, pairs=pairs)


def seed_up(V: list, S: frozenset) -> dict:
    """seedUp[u] = |{ s ∈ S : Σ sign(v.karma) over included(v, S), voter s, recipient u > 0 }|."""
    inc = included_fn(V, S)
    net = {}
    for v in V:
        if inc(v):
            key = (v.voter, v.recipient)
            net[key] = sat_add(net.get(key, 0), sign(v.karma))
    out = {}
    for (s, u), x in net.items():
        if x > 0:
            out[u] = out.get(u, 0) + 1
    return out


def compute_karma(view) -> Karma:
    counting = opinions.counting(view)
    pol = view.policy
    l2 = view.l2
    V = [c for c in counting.values() if c.karma is not None and c.subject_coll in KARMA_COLLECTIONS]
    S = l2.S
    st1 = pairsum(V, None, S, pol.pairCap)
    K1 = st1.out
    su = seed_up(V, S)
    BN = l2.B | l2.N
    E = S | frozenset(u for u in l2.P - S
                      if u not in BN and K1.get(u, 0) >= pol.establish and su.get(u, 0) >= pol.establishSeeds)
    st2 = pairsum(V, K1, E, pol.pairCap)
    KP = st2.out
    st3 = pairsum(V, KP, E, pol.pairCap)
    K = st3.out
    return Karma(counting=counting, K1=K1, seedUp=su, E=E, KP=KP, K=K, V=tuple(v.uri for v in V),
                 stages=(st1, st2, st3))


def vote_power(view, u: str) -> tuple:
    """Displayed vote power of user u (§6.1): (normal, strong) = (small, strong)(max(KP[u], 0))."""
    kp = max(view.l3.KP.get(u, 0), 0)
    return small(kp), strong(kp)


__all__ = ["LADDER", "small", "strong", "power", "sign", "clamp", "Counted", "Karma", "Stage",
           "compute_karma", "pairsum", "seed_up", "included_fn", "vote_power", "sat"]
