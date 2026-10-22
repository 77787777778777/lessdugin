# SPDX-License-Identifier: CC0-1.0
"""L1: forum-tier moderation (social.md §4.2–§4.8, §4.10–§4.12).

compute_modfold() classifies every modAction, resolves every (subject, dimension) and keeps
the statuses the mod log needs. It reads L0, modActions, and the existence and CIDs of
records (and `now`, for `until`), nothing else.

Dimension names used throughout the model and the vector projections:
    hidden, spam, access, restrict, nullify, tier, pin.forum, pin.thread, locked,
    tag.removed, tag.head
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .common import NS, KeyDerivationError, coll_of, did_of, fk, split_uri, thread_tid, ubytes
from .syntax import datetime_us
from .validity import is_positive

DIMENSIONS = ("hidden", "spam", "access", "restrict", "nullify", "tier", "pin.forum",
              "pin.thread", "locked", "tag.removed", "tag.head")

# Ordered dimensions: values from safest to least safe (social.md §4.2 "Order" column).
ORDER = {
    "hidden": (True, False),
    "spam": (True, False),
    "access": ("banned", "member"),
    "nullify": (True, False),
    "tier": ("personal", "frontpage", "curated"),
    "pin.forum": (False, True),
    "pin.thread": (False, True),
    "locked": (True, False),
    "tag.removed": (True, False),
}
KNOWN_CAPS = ("post", "comment", "vote", "react", "tag", "wiki", "sequence")


@dataclass(frozen=True)
class Resolved:
    value: object          # bool | str | frozenset (restrict) | (uri, cid) (tag.head)
    rank: int
    winner: str            # modAction uri


@dataclass
class ModRecord:
    """A valid modAction with forum = F, as classified by L1."""
    uri: str
    actor: str
    scope: str             # scope_eff (as written when unknown)
    action: str | None     # hide | access | restrict | tier | pin | lock | tag | None (unknown)
    subject: str
    value: dict
    no_effect: str | None  # "MA2" | "MA7" | "MA9" | None
    status: str | None = None          # forum scope: lapsed | unauthorised | expired | stale | None (counts)
    dims: dict = field(default_factory=dict)   # counting forum-scope records: dim -> value
    stale_head: bool = False           # #tag whose head fails the §4.5 check
    rank: int = 0


def _until_ok(value: dict, now: int) -> bool:
    if "until" not in value:
        return True
    us = datetime_us(value["until"])
    return us is not None and us > now


def _dims_of(action: str, a: dict) -> dict:
    if action == "hide":
        hidden = a.get("hidden", True)
        return {"hidden": hidden, "spam": bool(hidden and a.get("spam", False))}
    if action == "access":
        return {"access": a["level"]}
    if action == "restrict":
        caps = frozenset(c for c in a.get("capabilities", []) if c in KNOWN_CAPS)     # MA8
        return {"restrict": caps, "nullify": a.get("nullifyVotes", False)}
    if action == "tier":
        return {"tier": a["tier"]}
    if action == "pin":
        return {"pin." + a["scope"]: a.get("pinned", True)}
    if action == "lock":
        return {"locked": a.get("locked", True)}
    if action == "tag":
        d = {"tag.removed": a.get("removed", False)}
        if "head" in a:
            d["tag.head"] = (a["head"]["uri"], a["head"]["cid"])
        return d
    raise AssertionError(action)


class ModFold:
    def __init__(self, records: dict, validity: dict, auth, policy, forum: str, now: int):
        self.auth = auth
        self.policy = policy
        self.F = forum
        self.fk = fk(forum)
        self.now = now
        self.records = records
        self.mods: dict[str, ModRecord] = {}
        self._by: dict = {}                      # (subject, dim) -> [(rank, value, uri)]
        self._resolved: dict = {}                # (subject, dim) -> Resolved | "conflict"
        for uri, rec in records.items():
            if rec.coll != "modAction":
                continue
            val = validity[uri]
            if not val.valid or rec.value.get("forum") != forum:
                continue
            self._classify(rec, val)
        for key, xs in self._by.items():
            self._resolved[key] = self._resolve_list(key[1], xs)

    # ------------------------------------------------------------------ classification

    def _classify(self, rec, val) -> None:
        v = rec.value
        a = v["action"]
        t = a["$type"]
        base = f"{NS}.modAction#"
        name = t[len(base):] if t.startswith(base) else None
        if name not in ("hide", "access", "restrict", "tier", "pin", "lock", "tag"):
            name = None
        m = ModRecord(uri=rec.uri, actor=rec.did, scope=v.get("scope", "forum"), action=name,
                      subject=v["subject"], value=v, no_effect=val.no_effect)
        self.mods[rec.uri] = m
        if m.no_effect is not None or m.scope != "forum":
            return                               # reserved/unknown, or author scope (L5)
        X, T = m.actor, m.subject
        auth = self.auth
        m.rank = auth.rank(X)
        if X not in auth.ranks:                  # §4.3 (2); §4.10 lapse
            m.status = "lapsed"
            return
        rank_ok = X == auth.owner or X == auth.subject_author(T) or auth.rank(X) > auth.subject_rank(T)
        role_ok = not (name == "tier" and a["tier"] == "curated" and auth.rank(X) < 2)
        if not (rank_ok and role_ok):            # §4.3 (3), (4)
            m.status = "unauthorised"
            return
        if not _until_ok(v, self.now):           # §4.3 (5)
            m.status = "expired"
            return
        if is_positive(a) and not self._cid_bound_ok(T, v.get("subjectCid")):   # §4.3 (6), §4.5
            m.status = "stale"
            return
        dims = _dims_of(name, a)
        if "tag.head" in dims:
            huri, hcid = dims["tag.head"]
            if not self._cid_bound_ok(huri, hcid):                             # §4.5 #tag.head
                del dims["tag.head"]
                m.stale_head = True
        m.dims = dims
        for d, value in dims.items():
            self._by.setdefault((T, d), []).append((m.rank, value, m.uri))

    def _cid_bound_ok(self, uri: str, cid) -> bool:
        r = self.records.get(uri)
        return r is not None and cid == r.cid

    # ------------------------------------------------------------------ resolution (§4.4)

    @staticmethod
    def _resolve_list(d: str, xs: list):
        r = max(x[0] for x in xs)
        top = [x for x in xs if x[0] == r]
        vs = [x[1] for x in top]
        if d == "restrict":
            v = frozenset().union(*vs)
            winner = min((x[2] for x in top), key=ubytes)
            return Resolved(v, r, winner)
        if d == "tag.head":
            if any(x != vs[0] for x in vs):
                return "conflict"
            return Resolved(vs[0], r, min((x[2] for x in top), key=ubytes))
        order = ORDER[d]
        v = min(vs, key=order.index)
        winner = min((x[2] for x in top if x[1] == v), key=ubytes)
        return Resolved(v, r, winner)

    def resolve(self, T: str, d: str):
        """Resolved, or None for Default (including an unordered conflict)."""
        res = self._resolved.get((T, d))
        return res if isinstance(res, Resolved) else None

    def conflict(self, T: str, d: str) -> bool:
        return self._resolved.get((T, d)) == "conflict"

    def default(self, d: str):
        return {"hidden": False, "spam": False, "access": "none", "restrict": frozenset(),
                "nullify": False, "tier": self.policy.defaultTier, "pin.forum": False,
                "pin.thread": False, "locked": False, "tag.removed": False, "tag.head": None}[d]

    def effective(self, T: str, d: str):
        res = self.resolve(T, d)
        return res.value if res is not None else self.default(d)

    def resolved_items(self):
        """Every (subject, dim, Resolved | "conflict"), sorted by (subject bytes, dim)."""
        keys = sorted(self._resolved, key=lambda k: (ubytes(k[0]), DIMENSIONS.index(k[1])))
        return [(k[0], k[1], self._resolved[k]) for k in keys]

    def assertions(self, T: str, d: str) -> list:
        return list(self._by.get((T, d), []))

    # ------------------------------------------------------------------ derived values

    def keep_rank(self, uri: str) -> int:
        res = self.resolve(uri, "hidden")
        return res.rank if res is not None and res.value is False else 0

    def ban_rank(self, did: str):
        res = self.resolve(did, "access")
        return res.rank if res is not None and res.value == "banned" else None

    def restrict_rank(self, did: str) -> int:
        res = self.resolve(did, "restrict")
        return res.rank if res is not None else 0

    def lock_rank(self, p: str) -> int:
        res = self.resolve(p, "locked")
        return res.rank if res is not None else 0

    def allowed(self, p: str):
        """allowed(p) (§4.4), with MA9 entry filtering; None unless effective(p, locked)."""
        res = self.resolve(p, "locked")
        if res is None or res.value is not True:
            return None
        try:
            prefix = f"{self.fk}-{thread_tid(p)}-"
        except KeyDerivationError:
            prefix = None
        sets = []
        for (rank, value, uri) in self._by[(p, "locked")]:
            if rank == res.rank and value is True:
                entries = self.mods[uri].value["action"].get("allowed", [])
                sets.append(frozenset(u for u in entries
                                      if prefix is not None and split_uri(u)[2].startswith(prefix)))
        return frozenset.intersection(*sets)

    # ------------------------------------------------------------------ mod log (§4.12), forum scope

    def forum_status(self, m: ModRecord):
        """(status, rank) of a forum-scope record for the mod log; rank only for `overridden`."""
        if m.no_effect is not None:
            return ("reserved/unknown", None)
        if m.status is not None:                 # lapsed | unauthorised | expired | stale
            return (m.status, None)
        if m.stale_head:                         # NOTES.md: a stale #tag.head shows `stale`
            return ("stale", None)
        # A counting record is `effective` if one of its dimensions (other than the derived
        # `spam`) is in effect for it: resolved at its own rank, with its value (any value for
        # the set dimension `restrict`). Otherwise it is `overridden` by the highest resolved rank.
        ranks = []
        for d in m.dims:
            if d == "spam":
                continue
            res = self._resolved.get((m.subject, d))
            if not isinstance(res, Resolved):
                continue
            ranks.append(res.rank)
            if res.rank == m.rank and (d == "restrict" or res.value == m.dims[d]):
                return ("effective", None)
        return ("overridden", max(ranks))


def mod_time(rec) -> int | None:
    """Mod log time: µs(updatedAt) ?? µs(createdAt) for a modAction, µs(createdAt) for a revision."""
    v = rec.value
    if rec.coll == "modAction":
        t = datetime_us(v.get("updatedAt")) if "updatedAt" in v else None
        return t if t is not None else datetime_us(v.get("createdAt"))
    return datetime_us(v.get("createdAt"))


def degraded(auth, inp) -> bool:
    """§4.11: some authority's repo is Unavailable (a missing entry is Unavailable)."""
    return any(inp.status(a) == "unavailable" for a in sorted(auth.ranks))


def subject_coll(subject: str):
    return coll_of(subject) if subject.startswith("at://") else None


__all__ = ["ModFold", "ModRecord", "Resolved", "DIMENSIONS", "mod_time", "degraded", "did_of",
           "subject_coll"]
