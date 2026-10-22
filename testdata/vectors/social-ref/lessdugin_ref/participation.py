# SPDX-License-Identifier: CC0-1.0
"""L2: participation and seeds (social.md §3): P, G, B, N, S, members and applicants.

Reads L0, L1 and membership records only. E and newcomers need L3 (karma.py).
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import ubytes


@dataclass(frozen=True)
class Participation:
    mode: str               # approval | open
    members: frozenset      # member(d): a valid membership with rkey fk and forum = F
    P: frozenset
    G: frozenset
    B: frozenset
    N: frozenset
    S: frozenset
    applicants: frozenset   # approval mode: member(d) ∧ access(d) = none ∧ d ∉ A (NOTES.md)


def sorted_dids(xs) -> list:
    return sorted(xs, key=ubytes)


def compute_participation(records, validity, auth, mod, policy, forum) -> Participation:
    members = set()
    for uri, rec in records.items():
        if rec.coll == "membership" and validity[uri].valid and rec.value.get("forum") == forum:
            members.add(rec.did)
    # Every DID that some resolution or membership mentions; DIDs outside it have access none.
    subjects = {T for (T, d, _) in mod.resolved_items() if d in ("access", "nullify")}
    access = {d: mod.effective(d, "access") for d in subjects}
    G = frozenset(d for d, a in access.items() if a == "member")
    B = frozenset(d for d, a in access.items() if a == "banned")
    N = frozenset(d for d in subjects if mod.effective(d, "nullify") is True)
    A = auth.A
    if policy.membership == "open":
        P = A | frozenset(members)
        applicants = frozenset()
    else:
        P = A | frozenset(d for d in members if access.get(d, "none") != "none")
        applicants = frozenset(d for d in members if access.get(d, "none") == "none" and d not in A)
    S = (A | G) - (B | N)
    return Participation(mode=policy.membership, members=frozenset(members), P=P, G=G, B=B, N=N,
                         S=S, applicants=applicants)
