# SPDX-License-Identifier: CC0-1.0
"""L3.5: score, agreement, unweighted, earlier versions (social.md §7). STUB.

Interface:

    compute_scores(view) -> dict uri -> Score
        for every valid item i in F (post, comment, wikiRevision, sequence; V1 not Excluded);
        reads L0–L3 only (view.l3 for counting opinions, E and KP).

    Score.score       int  (self(i) + Σ power(KP[voter], karma) over W(i); no pair cap)
    Score.agreement   int | None  (None when policy.voting = karmaOnly)
    Score.unweighted  int  (counting karma opinions of voters outside E)
    Score.earlier     int  (N of "N of M votes were cast on an earlier version")
    Score.total       int  (M)

Until the stage lands this is exact only when L3 reports no counting opinion: then
score(i) = self(i) (1 for an author in E, whose KP is 0, else 0) and every count is 0.
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import ITEM_COLLECTIONS, NotModelled
from .karma import small, strong


@dataclass(frozen=True)
class Score:
    score: int
    agreement: int | None
    unweighted: int
    earlier: int
    total: int


def compute_scores(view) -> dict:
    l3 = view.l3
    if l3.counting:
        raise NotModelled("L3.5: scores with counting opinions (social.md §7) are not filled yet")
    out = {}
    for uri, rec in view.input.records.items():
        if rec.coll not in ITEM_COLLECTIONS or view.excluded[uri]:
            continue
        a = rec.did
        kp = max(l3.KP.get(a, 0), 0)
        self_ = (strong(kp) if rec.coll == "post" else small(kp)) if a in l3.E else 0
        agreement = None if view.policy.voting == "karmaOnly" else 0
        out[uri] = Score(self_, agreement, 0, 0, 0)
    return out
