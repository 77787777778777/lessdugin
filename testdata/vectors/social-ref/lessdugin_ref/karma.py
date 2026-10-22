# SPDX-License-Identifier: CC0-1.0
"""L3: opinion validity (social.md §5) and karma/1 (§6). STUB — to be filled by the karma stage.

Interface (keep it; fold.py, visibility.py and scores.py call it):

    compute_karma(view) -> Karma
        reads view.input, view.validity, view.policy, view.F, view.l0, view.l1, view.l2
        (never L3.5 or above).

    Karma.counting   frozenset of opinion uris that count (§5, O1–O5), every axis
    Karma.K1, .seedUp, .KP, .K   dict did -> int (a missing key reads as 0; use .get(d, 0))
    Karma.E          frozenset of eligible voters (S ⊆ E)

The power helpers small/strong/power (§6.1) are implemented here already.

Until the stage lands, compute_karma() is exact only for inputs without any valid opinion in
F: then nothing counts, K1 = KP = K = 0 for everyone and E = S (seedUp = 0 < establishSeeds).
Any other input raises NotModelled.
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import NotModelled

LADDER = ((500000, 16), (250000, 15), (175000, 14), (100000, 13), (75000, 12), (50000, 11),
          (25000, 10), (10000, 9), (5000, 8), (2500, 7), (1000, 6), (500, 5), (250, 4), (100, 3),
          (10, 2))


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


@dataclass(frozen=True)
class Karma:
    counting: frozenset
    K1: dict
    seedUp: dict
    E: frozenset
    KP: dict
    K: dict


def compute_karma(view) -> Karma:
    for uri, rec in view.input.records.items():
        if rec.coll == "opinion" and view.validity[uri].valid and rec.value.get("forum") == view.F:
            raise NotModelled("L3: opinion validity and karma/1 (social.md §5–§6) is not filled yet")
    return Karma(counting=frozenset(), K1={}, seedUp={}, E=view.l2.S, KP={}, K={})
