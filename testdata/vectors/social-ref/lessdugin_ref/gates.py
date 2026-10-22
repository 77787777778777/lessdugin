# SPDX-License-Identifier: CC0-1.0
"""L4: gates (social.md §8, karma/1). Reads K from L3 (karma.py) and A from L0.

    passes(view, u, g) ⇔ u ∈ A ∨ K[u] ≥ gateValue(g)
    gateValue(g)       = decl.policy.gates.g if present, else the karma/1 default (policy.py)
    g ∈ GATES (the §8 table order): reactCreate, reactSecond, reactAnti, authorModPersonal,
        authorModFrontpage, createTag, wikiEdit, sequence

The "weighted vote" gate (voter ∈ E) is not configurable and lives in karma.py and scores.py.
`tagCreation = moderators` (genesis wiki revisions by authorities only) is a §13.2 rule of
tags.py, not a gate value.

Vector projection fields (kind "karma"; testdata/vectors/README.md, social section). They are
registered here, into the one global field namespace of projections.py, because this stage
owns gates.py and not projections.py (NOTES.md 38):

    gateValues   {gate: int} for every gate in GATES (the effective values)
    passes       {did: {gate: bool}} for every DID of A and every DID that owns a record in
                 `records`
"""

from __future__ import annotations

from . import projections as _projections
from .common import split_uri, ubytes
from .policy import GATE_DEFAULTS

GATES = ("reactCreate", "reactSecond", "reactAnti", "authorModPersonal", "authorModFrontpage",
         "createTag", "wikiEdit", "sequence")
assert set(GATES) == set(GATE_DEFAULTS)


def passes(view, u: str, g: str) -> bool:
    if g not in GATE_DEFAULTS:
        raise ValueError(g)
    return u in view.l0.A or view.l3.K.get(u, 0) >= view.policy.gate(g)


def gate_values(view) -> dict:
    return {g: view.policy.gate(g) for g in GATES}


def gate_dids(view) -> list:
    """The DIDs the `passes` field lists: A ∪ { did(u) : u ∈ records }, sorted by bytes."""
    ds = set(view.l0.A)
    for u in view.input.records:
        ds.add(split_uri(u)[0])
    return sorted(ds, key=ubytes)


@_projections.field("karma", "gateValues")
def _gate_values_field(view):
    return gate_values(view)


@_projections.field("karma", "passes")
def _passes_field(view):
    return {d: {g: passes(view, d, g) for g in GATES} for d in gate_dids(view)}
