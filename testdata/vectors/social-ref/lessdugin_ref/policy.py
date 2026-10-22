# SPDX-License-Identifier: CC0-1.0
"""Effective policy (social.md §1.6) from a valid declaration.

Absent fields take their lexicon default (data-model.md §6.1 and lexicons/…/forum.json); an
absent `#gates` field takes the karma/1 default (social.md §8). An unknown knownValue takes the
data-model.md §13.3 fallback and raises the banner "this forum uses rules newer than your
client". Out-of-range integers make the declaration invalid (G2/FO4), so they never reach here.
"""

from __future__ import annotations

from dataclasses import dataclass

# The fixed PARAMS list of social.md §1.2, in order.
PARAMS = (
    "membership", "defaultTier", "voting", "authorModeration", "tagCreation", "pairCap",
    "establish", "establishSeeds", "offsetHours", "exponentMilli", "curatedBonus",
    "frontpageBonus", "cutoffDays", "reactSecond", "reactCreate", "reactAnti",
    "authorModPersonal", "authorModFrontpage", "createTag", "wikiEdit", "sequence",
    "collapseBelow", "newUsers",
)

# karma/1 gate defaults (social.md §8, "field absent" column).
GATE_DEFAULTS = {
    "reactSecond": 5, "reactCreate": 10, "reactAnti": 20, "authorModPersonal": 50,
    "authorModFrontpage": 2000, "createTag": 10, "wikiEdit": 5, "sequence": 1000,
}

# Known behaviour IDs (social.md §0.3); v1 knows exactly one of each, which is the default.
ALG_DEFAULTS = {"moderation": "mod/1", "karma": "karma/1", "ranking": "magic/1",
                "reactPalette": "default/1"}


@dataclass(frozen=True)
class Policy:
    membership: str          # approval | open
    defaultTier: str         # personal | frontpage
    voting: str              # karmaOnly | twoAxis | reacts
    authorModeration: bool
    tagCreation: str         # open | moderators
    pairCap: int
    establish: int
    establishSeeds: int
    offsetHours: int
    exponentMilli: int
    curatedBonus: int
    frontpageBonus: int
    cutoffDays: int
    reactSecond: int
    reactCreate: int
    reactAnti: int
    authorModPersonal: int
    authorModFrontpage: int
    createTag: int
    wikiEdit: int
    sequence: int
    collapseBelow: int
    newUsers: str            # show | collapse | hide
    algorithm_ids: tuple     # ((name, id), …) in ALG_DEFAULTS order
    newer_rules_banner: bool
    fallbacks: tuple         # field paths that fell back from an unknown knownValue

    def gate(self, g: str) -> int:
        """gateValue(g) (social.md §8)."""
        return getattr(self, g)

    @property
    def algs(self) -> dict:
        return dict(self.algorithm_ids)


def _kv(value, known, default, path, fallbacks):
    if value is None:
        return default
    if value in known:
        return value
    fallbacks.append(path)
    return default


def effective_policy(decl_value: dict) -> Policy:
    fb: list = []
    pol = decl_value.get("policy") or {}
    karma = pol.get("karma") or {}
    rank = pol.get("ranking") or {}
    gates = pol.get("gates") or {}
    display = pol.get("display") or {}

    algs = (
        ("moderation", _kv(pol.get("moderation"), ("mod/1",), "mod/1", "policy.moderation", fb)),
        ("karma", _kv(karma.get("algorithm"), ("karma/1",), "karma/1", "policy.karma.algorithm", fb)),
        ("ranking", _kv(rank.get("algorithm"), ("magic/1",), "magic/1", "policy.ranking.algorithm", fb)),
        ("reactPalette", _kv(pol.get("reactPalette"), ("default/1",), "default/1", "policy.reactPalette", fb)),
    )
    vals = dict(
        membership=_kv(decl_value.get("membership"), ("open", "approval"), "approval", "membership", fb),
        defaultTier=_kv(decl_value.get("defaultTier"), ("personal", "frontpage"), "personal", "defaultTier", fb),
        voting=_kv(pol.get("voting"), ("karmaOnly", "twoAxis", "reacts"), "reacts", "policy.voting", fb),
        authorModeration=pol.get("authorModeration", True),
        tagCreation=_kv(pol.get("tagCreation"), ("open", "moderators"), "open", "policy.tagCreation", fb),
        pairCap=karma.get("pairCap", 25),
        establish=karma.get("establish", 1),
        establishSeeds=karma.get("establishSeeds", 2),
        offsetHours=rank.get("offsetHours", 2),
        exponentMilli=rank.get("exponentMilli", 1150),
        curatedBonus=rank.get("curatedBonus", 10),
        frontpageBonus=rank.get("frontpageBonus", 0),
        cutoffDays=rank.get("cutoffDays", 90),
        collapseBelow=display.get("collapseBelow", -5),
        newUsers=_kv(display.get("newUsers"), ("show", "collapse", "hide"), "collapse", "policy.display.newUsers", fb),
    )
    for g, d in GATE_DEFAULTS.items():
        vals[g] = gates.get(g, d)
    return Policy(**vals, algorithm_ids=algs, newer_rules_banner=bool(fb), fallbacks=tuple(fb))


def encode_param(v) -> str:
    """social.md §1.2 value encoding: shortest decimal, true/false, knownValue strings."""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    return v


def params_line(p: Policy) -> str:
    return ",".join(f"{name}={encode_param(getattr(p, name))}" for name in PARAMS)


def algs_line(p: Policy) -> str:
    a = p.algs
    return ",".join(a[k] for k in ALG_DEFAULTS)
