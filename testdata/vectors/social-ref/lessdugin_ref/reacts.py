# SPDX-License-Identifier: CC0-1.0
"""L5: react aggregation (social.md §9.2, karma/1; palette default/1 §9.1).

Interface (fold.py and projections.py call it; keep the names):

    compute_reacts(view) -> dict  subject uri -> tuple[ReactTally, ...]
        Only subjects with at least one valid react appear. Tallies are sorted by
        (react bytes, qkey bytes). Reads L0–L4 only: view.l3.counting (O1–O5), view.l3.E,
        view.l3.K, view.passes(u, g), view.l1 (restrict{react}, via opinions.axis_enabled),
        view.policy (voting must be `reacts`), the palette ids (docs/spec/reacts-default-1.json)
        and qkey (lfm-1.md §11). Opinions on absent subjects never count here (§5).

    ReactTally.react       palette id
    ReactTally.qkey        qkey of the quote ("" when the entry has no quote)
    ReactTally.creator     DID (the u ∈ C with the greatest K[u]; ties -> smallest DID)
    ReactTally.supporters  tuple of DIDs sorted by bytes, creator included
    ReactTally.opposers    tuple of DIDs sorted by bytes
    ReactTally.quote       the creator's #textQuote object, or None (display only)

    qkey(quote) -> str     lfm-1.md §11: norm(trim_ws(quote.exact)), "" without a quote, where
                           norm removes the §10.2 controls, applies NFC and replaces every
                           White_Space run with one U+0020 (NOTES.md 35)
    PALETTE                the default/1 ids, in palette order (checked against the JSON file)

Per counting opinion v with the reacts axis enabled (opinions.axis_enabled):
    entries(v) = [e ∈ v.reacts : e.react ∈ palette ∧ e.stance ∈ {for, against}]  (default for)
    grouped by (react, qkey): a group with both stances is dropped (contradiction); otherwise
    it yields one (react, q, stance). Same-stance duplicates collapse (NOTES.md 36).
Per subject and (react, q): Fv, Av (voters in E), C = {u ∈ Fv : passes(u, reactCreate)};
valid ⇔ C ≠ ∅; creator, supporters = {u ∈ Fv : passes(u, reactSecond)} ∪ {creator},
opposers = {u ∈ Av : passes(u, reactAnti)}.
"""

from __future__ import annotations

import json
import unicodedata
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

from . import opinions
from .common import ubytes

PALETTE_FILE = Path(__file__).resolve().parents[4] / "docs" / "spec" / "reacts-default-1.json"

# social.md §9.1, in this order; the JSON file must list exactly these ids.
PALETTE = (
    "agree", "disagree", "important", "insightful", "changed-mind", "crux", "not-a-crux",
    "locally-valid", "locally-invalid", "typo", "wrong", "misleading", "unclear",
    "dont-understand", "examples", "citation-needed", "concrete", "overconfident",
    "underconfident", "hits-the-mark", "missed-the-point", "strawman", "steelman",
    "scout-mindset", "soldier-mindset", "too-harsh", "good-point", "thanks", "seen",
    "will-reply-later", "laugh", "heart", "surprise", "empathy",
)
STANCES = ("for", "against")

# lfm-1.md §9.1: the 25 White_Space code points.
WHITE_SPACE = frozenset([*range(0x09, 0x0E), 0x20, 0x85, 0xA0, 0x1680, *range(0x2000, 0x200B),
                         0x2028, 0x2029, 0x202F, 0x205F, 0x3000])
# lfm-1.md §10.2 step 1: the bidi controls removed besides the non-White_Space Cc code points.
BIDI_CONTROLS = frozenset([0x061C, 0x200E, 0x200F, *range(0x202A, 0x202F), *range(0x2066, 0x206A)])
assert len(WHITE_SPACE) == 25 and len(BIDI_CONTROLS) == 12


@lru_cache(maxsize=None)
def palette() -> frozenset:
    with open(PALETTE_FILE, encoding="utf-8") as f:
        doc = json.load(f)
    ids = tuple(r["id"] for r in doc["reacts"])
    assert doc["id"] == "default/1" and ids == PALETTE, "reacts-default-1.json differs from social.md §9.1"
    return frozenset(ids)


def _is_removed_control(ch: str) -> bool:
    cp = ord(ch)
    return (unicodedata.category(ch) == "Cc" and cp not in WHITE_SPACE) or cp in BIDI_CONTROLS


def trim_ws(s: str) -> str:
    i, j = 0, len(s)
    while i < j and ord(s[i]) in WHITE_SPACE:
        i += 1
    while j > i and ord(s[j - 1]) in WHITE_SPACE:
        j -= 1
    return s[i:j]


def norm(s: str) -> str:
    """lfm-1.md §11 norm(x): remove the §10.2 controls, NFC, then White_Space runs -> U+0020."""
    s = "".join(ch for ch in s if not _is_removed_control(ch))
    s = unicodedata.normalize("NFC", s)
    out, in_ws = [], False
    for ch in s:
        if ord(ch) in WHITE_SPACE:
            if not in_ws:
                out.append(" ")
            in_ws = True
        else:
            out.append(ch)
            in_ws = False
    return "".join(out)


def qkey(quote) -> str:
    """qkey(quote) of lfm-1.md §11: norm(trim_ws(quote.exact)), or "" without a quote."""
    if quote is None:
        return ""
    return norm(trim_ws(quote["exact"]))


@dataclass(frozen=True)
class ReactTally:
    react: str
    qkey: str
    creator: str
    supporters: tuple
    opposers: tuple
    quote: object = None


def yields(value: dict) -> dict:
    """The entries one opinion yields: (react, qkey) -> (stance, quote of the first entry).

    entries(v) keeps palette ids with a known stance (absent = for); a (react, qkey) group with
    both stances is dropped; same-stance duplicates collapse to one.
    """
    pal = palette()
    groups = {}
    for e in value.get("reacts", []):
        stance = e.get("stance", "for")
        if e["react"] not in pal or stance not in STANCES:
            continue                                            # OP3: entry discarded
        key = (e["react"], qkey(e.get("quote")))
        g = groups.setdefault(key, {"stances": set(), "quote": e.get("quote")})
        g["stances"].add(stance)
    return {k: (next(iter(g["stances"])), g["quote"]) for k, g in groups.items() if len(g["stances"]) == 1}


def compute_reacts(view) -> dict:
    l3 = view.l3
    E, K = l3.E, l3.K
    per = {}                    # (subject, react, q) -> {"for": {voter: quote}, "against": set()}
    for c in l3.counting.values():
        if not opinions.axis_enabled(view, c, "reacts") or c.voter not in E:
            continue
        for (react, q), (stance, quote) in yields(view.input.records[c.uri].value).items():
            g = per.setdefault((c.subject, react, q), {"for": {}, "against": set()})
            if stance == "for":
                g["for"][c.voter] = quote
            else:
                g["against"].add(c.voter)
    out = {}
    for (subject, react, q) in sorted(per, key=lambda k: (ubytes(k[0]), ubytes(k[1]), ubytes(k[2]))):
        g = per[(subject, react, q)]
        Fv, Av = g["for"], g["against"]
        C = [u for u in Fv if view.passes(u, "reactCreate")]
        if not C:
            continue
        creator = min(C, key=lambda u: (-K.get(u, 0), ubytes(u)))
        supporters = {u for u in Fv if view.passes(u, "reactSecond")} | {creator}
        opposers = {u for u in Av if view.passes(u, "reactAnti")}
        t = ReactTally(react=react, qkey=q, creator=creator,
                       supporters=tuple(sorted(supporters, key=ubytes)),
                       opposers=tuple(sorted(opposers, key=ubytes)), quote=Fv[creator])
        out.setdefault(subject, []).append(t)
    return {s: tuple(ts) for s, ts in out.items()}
