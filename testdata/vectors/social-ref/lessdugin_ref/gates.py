# SPDX-License-Identifier: CC0-1.0
"""L4: gates (social.md §8). Complete; it reads K from L3 (karma.py).

    passes(view, u, g) ⇔ u ∈ A ∨ K[u] ≥ gateValue(g)
    g ∈ reactSecond, reactCreate, reactAnti, authorModPersonal, authorModFrontpage,
        createTag, wikiEdit, sequence
"""

from __future__ import annotations

from .policy import GATE_DEFAULTS


def passes(view, u: str, g: str) -> bool:
    if g not in GATE_DEFAULTS:
        raise ValueError(g)
    return u in view.l0.A or view.l3.K.get(u, 0) >= view.policy.gate(g)
