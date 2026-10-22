# SPDX-License-Identifier: CC0-1.0
"""L0: authorities and ranks (social.md §4.1), from the current declaration only."""

from __future__ import annotations

from dataclasses import dataclass

from .common import split_uri
from .syntax import is_canonical_did

ROLE_RANK = {"admin": 2, "moderator": 1}


@dataclass(frozen=True)
class Authorities:
    owner: str
    ranks: dict          # did -> 3 | 2 | 1, every authority

    @property
    def A(self) -> frozenset:
        return frozenset(self.ranks)

    def rank(self, did) -> int:
        return self.ranks.get(did, 0)

    def subject_author(self, subject: str):
        """subjectAuthor(T): the DID itself, did(T) of an at-uri, None for "tag:" slugs."""
        if subject.startswith("tag:"):
            return None
        if subject.startswith("at://"):
            return split_uri(subject)[0]
        return subject

    def subject_rank(self, subject: str) -> int:
        a = self.subject_author(subject)
        return 0 if a is None else self.rank(a)


def compute_authorities(decl_uri: str, decl_value: dict) -> Authorities:
    owner = split_uri(decl_uri)[0]
    ranks = {owner: 3}
    seen = set()
    for m in decl_value.get("moderators", []):
        d = m["did"]
        if d in seen:                 # FO2: the first entry for a DID wins, even an ignored one
            continue
        seen.add(d)
        if d == owner or m["role"] not in ROLE_RANK:
            continue                  # FO2: the owner's own entry and unknown roles are ignored
        ranks[d] = ROLE_RANK[m["role"]]
    return Authorities(owner=owner, ranks=ranks)


def role_of(auth: Authorities, did: str):
    return {3: "owner", 2: "admin", 1: "moderator"}.get(auth.rank(did))


__all__ = ["Authorities", "compute_authorities", "role_of", "is_canonical_did"]
