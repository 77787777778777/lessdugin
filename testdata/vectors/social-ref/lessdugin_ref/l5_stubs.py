# SPDX-License-Identifier: CC0-1.0
"""L5 stubs other than the author tier: reacts (§9.2), tags and wiki (§13), sequences (§14).

Each function is exact for inputs where its subject matter is absent and raises NotModelled
otherwise. The next stages replace them (keep the signatures; they may move into their own
modules, re-exported from here).

    compute_reacts(view) -> dict subject uri -> list of
        {"react", "quote": qkey string, "creator": did, "supporters": [did…], "opposers": [did…]}
        (valid reacts only; reads L0–L4: view.l3.E, view.l3.K, gates.passes)

    compute_tags(view) -> Tags
        Tags.canonical: dict slug -> wikiRevision uri | None   (§13.2, reads V1–V5 via
                         visibility.mod_state, L0–L4)
        Tags.exists(slug) / Tags.applicable(slug) -> bool
        Tags.rel(p, slug) -> int ; Tags.applies(p, slug) -> bool        (§13.3)

    compute_sequences(view) -> Sequences
        Sequences.library: [sequence uri…] in library order (§14)
        Sequences.entries(s) -> [post uri…] ; Sequences.displayable(u) -> bool
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .common import NotModelled


def compute_reacts(view) -> dict:
    if view.l3.counting:
        raise NotModelled("L5: react aggregation (social.md §9.2) is not filled yet")
    return {}


@dataclass(frozen=True)
class Tags:
    canonical: dict = field(default_factory=dict)

    def exists(self, slug: str) -> bool:
        return False

    def applicable(self, slug: str) -> bool:
        return False

    def rel(self, p: str, slug: str) -> int:
        return 0

    def applies(self, p: str, slug: str) -> bool:
        return False


def compute_tags(view) -> Tags:
    for uri, rec in view.input.records.items():
        if not view.validity[uri].valid:
            continue
        if rec.coll == "wikiRevision" or (rec.coll == "post" and rec.value.get("tags")):
            raise NotModelled("L5: tags and wiki (social.md §13) are not filled yet")
    return Tags()


@dataclass(frozen=True)
class Sequences:
    library: tuple = ()

    def entries(self, s: str) -> list:
        raise NotModelled("sequence entries")

    def displayable(self, u: str) -> bool:
        raise NotModelled("sequence entries")


def compute_sequences(view) -> Sequences:
    for uri, rec in view.input.records.items():
        if rec.coll == "sequence" and view.validity[uri].valid:
            raise NotModelled("L5: the sequence library (social.md §14) is not filled yet")
    return Sequences()
