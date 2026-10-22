# SPDX-License-Identifier: CC0-1.0
"""L5: the sequence library (social.md §14, karma/1). STUB.

Interface (keep the names):

    compute_sequences(view) -> Sequences
        Reads L0–L4, the V1–V5 states (visibility.mod_state restricted to V1–V5), score
        (view.l35) and t_rank (time, outside the layer stack). "displayable" needs the L6
        listed state of posts (§10.1); it is evaluated lazily, after visibility.

    Sequences.library                    tuple of sequence uris in library order (§14)
    Sequences.eligible(s) -> bool
    Sequences.entries(s) -> tuple        post uris, chapters concatenated in order
    Sequences.displayable(u) -> bool
    Sequences.context(p, came_from=None) -> sequence uri | None
    Sequences.neighbours(p, s) -> (prev uri | None, next uri | None)

Until the stage lands this is exact only for inputs without valid sequences; anything else
raises NotModelled.
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import NotModelled


@dataclass(frozen=True)
class Sequences:
    library: tuple = ()

    def eligible(self, s: str) -> bool:
        raise NotModelled("sequences")

    def entries(self, s: str) -> tuple:
        raise NotModelled("sequences")

    def displayable(self, u: str) -> bool:
        raise NotModelled("sequences")

    def context(self, p: str, came_from=None):
        return None

    def neighbours(self, p: str, s: str):
        raise NotModelled("sequences")


def compute_sequences(view) -> Sequences:
    for uri, rec in view.input.records.items():
        if rec.coll == "sequence" and view.validity[uri].valid:
            raise NotModelled("L5: the sequence library (social.md §14) is not filled yet")
    return Sequences()
