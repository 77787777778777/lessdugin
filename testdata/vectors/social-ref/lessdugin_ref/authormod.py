# SPDX-License-Identifier: CC0-1.0
"""L5: author-tier moderation (social.md §4.9). STUB — to be filled by the next stage.

Interface (visibility.py and fold.py call it):

    compute_author_tier(view) -> AuthorTier
        reads L0–L4 (view.l3.E, gates.passes) and L1 resolutions; never L6.

    AuthorTier.author_moderated(c) -> bool   AuthorModerated(c) of §4.9 (V6), comments only
    AuthorTier.thread_pin(c) -> bool | None  the author thread-pin value of c when
                                             resolve(c, pin.thread) = Default, else None
    AuthorTier.status(m_uri) -> (status, rank | None)
                                             mod-log status of an author-scope modAction:
                                             effective | expired | stale | unauthorised |
                                             reserved/unknown (rank only for overridden)

Until the stage lands this is exact only for inputs without an effect-capable author-scope
modAction (valid, forum = F, scope author, no MA2/MA7/MA9 no-effect flag): then no comment is
author-moderated, there are no author pins, and author-scope records without effect show as
reserved/unknown. Anything else raises NotModelled.
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import NotModelled


@dataclass(frozen=True)
class AuthorTier:
    moderated: frozenset
    pins: dict
    statuses: dict

    def author_moderated(self, c: str) -> bool:
        return c in self.moderated

    def thread_pin(self, c: str):
        return self.pins.get(c)

    def status(self, m_uri: str):
        return self.statuses[m_uri]


def compute_author_tier(view) -> AuthorTier:
    statuses = {}
    for uri, m in view.l1.mods.items():
        if m.scope != "author":
            continue
        if m.no_effect is None:
            raise NotModelled("L5: author-tier moderation (social.md §4.9) is not filled yet")
        statuses[uri] = ("reserved/unknown", None)
    return AuthorTier(moderated=frozenset(), pins={}, statuses=statuses)
