# SPDX-License-Identifier: CC0-1.0
"""L3.5: score, agreement, unweighted, earlier versions (social.md §7, karma/1).

Interface (keep it; visibility.py V9, rank.py, sequences.py and projections.py call it):

    compute_scores(view) -> dict uri -> Score
        For every candidate item (post, comment, wikiRevision, sequence) that V1 does not
        exclude, in uri order: Hidden items have scores too (§7). Reads L0–L3 only
        (view.l3.counting, .E, .KP; view.policy).

    Score.score       int        self(i) + Σ power(KP[voter], karma) over W(i); no pair cap
    Score.agreement   int | None None when policy.voting = karmaOnly
    Score.unweighted  int        counting opinions with a karma value whose voter ∉ E
    Score.earlier     int        N: |{v ∈ W(i) with a karma value : v.subject.cid ≠ cid(i)}|, CIDs
                                 compared by canonical form (§0.4)
    Score.weighted    int        M: |{v ∈ W(i) with a karma value}|

W(i) = { v : v counts (§5), v.subject.uri = i.uri, voter(v) ∈ E }. Opinions on absent
subjects never contribute here (they have no item). "A karma value" means the karma axis is
present and enabled (Counted.karma is not None), so a voter restricted from `vote` adds to
neither count (decision 11 of social.md §20.1).
"""

from __future__ import annotations

from dataclasses import dataclass

from . import cids
from .common import ITEM_COLLECTIONS, sat_add
from .karma import power, small, strong


@dataclass(frozen=True)
class Score:
    score: int
    agreement: int | None
    unweighted: int
    earlier: int
    weighted: int


def self_score(view, uri: str) -> int:
    """self(i) of §7."""
    rec = view.input.records[uri]
    a = rec.did
    l3 = view.l3
    if a not in l3.E:
        return 0
    kp = max(l3.KP.get(a, 0), 0)
    return strong(kp) if rec.coll == "post" else small(kp)


def compute_scores(view) -> dict:
    l3 = view.l3
    E, KP = l3.E, l3.KP
    by_subject = {}
    for c in l3.counting.values():
        if not c.absent:
            by_subject.setdefault(c.subject, []).append(c)
    karma_only = view.policy.voting == "karmaOnly"
    out = {}
    for uri, rec in view.input.records.items():
        if rec.coll not in ITEM_COLLECTIONS or view.excluded[uri]:
            continue
        score = self_score(view, uri)
        agreement = None if karma_only else 0
        unweighted = earlier = weighted = 0
        for c in by_subject.get(uri, ()):
            if c.voter in E:                                      # W(i)
                if c.karma is not None:
                    score = sat_add(score, power(KP.get(c.voter, 0), c.karma))
                    weighted += 1
                    if not cids.same(c.subject_cid, rec.cid):       # canonical form (§0.4)
                        earlier += 1
                if c.agreement is not None and agreement is not None:
                    agreement = sat_add(agreement, power(KP.get(c.voter, 0), c.agreement))
            elif c.karma is not None:
                unweighted += 1
        out[uri] = Score(score, agreement, unweighted, earlier, weighted)
    return out
