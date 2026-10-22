# SPDX-License-Identifier: CC0-1.0
"""L5: tags and wiki (social.md §13, karma/1). STUB.

Interface (keep the names):

    compute_tags(view) -> Tags
        Reads L0–L4 and the V1–V5 states (visibility.mod_state(view, uri) restricted to rules
        V1–V5: a wikiRevision whose state is Excluded or Hidden(NotParticipant |
        ModerationUnavailable | Removed | Banned | Restricted) is not on any page).

    Tags.canonical          dict slug -> wikiRevision uri | None   (§13.2 head, for every slug
                            with a valid wikiRevision in F or a "tag:" modAction subject)
    Tags.candidates(slug)   frozenset of wikiRevision uris (Cand)
    Tags.flags(uri)         frozenset of display flags of a revision: "ParentUnavailable"
                            (Root(ParentUnavailable)), "AncestorAltered", "PagePartial"
    Tags.exists(slug)       bool (§13.2)
    Tags.applicable(slug)   bool (§13.2)
    Tags.rel(p, slug)       int  (§13.3)
    Tags.applies(p, slug)   bool (§13.3)

Until the stage lands this is exact only for inputs without valid wikiRevisions or post tags
(and without counting tag votes); anything else raises NotModelled.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .common import NotModelled


@dataclass(frozen=True)
class Tags:
    canonical: dict = field(default_factory=dict)

    def candidates(self, slug: str) -> frozenset:
        return frozenset()

    def flags(self, uri: str) -> frozenset:
        return frozenset()

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
    if view.l3.counting:
        raise NotModelled("L5: tag votes (social.md §13.3) are not filled yet")
    return Tags()
