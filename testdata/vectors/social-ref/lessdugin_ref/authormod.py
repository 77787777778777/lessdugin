# SPDX-License-Identifier: CC0-1.0
"""L5: author-tier moderation (social.md §4.9, mod/1).

    compute_author_tier(view) -> AuthorTier
        Reads L0–L4 only: view.l0 (A), view.l1 (forum-tier resolve, keep_rank, effective tier,
        and l1.mods for the author-scope ModRecords: scope "author", no_effect None),
        view.l3.E, view.passes(u, g) (authorGate), view.policy (authorModeration), records and
        CIDs (§4.5 CID check), view.input.now (until) and V1 (view.excluded: validity only).
        Never L6.

    AuthorTier.author_moderated(c) -> bool
        AuthorModerated(c) of §4.9 for a comment uri c (False for every other uri).
    AuthorTier.thread_pin(c) -> bool | None
        The author thread-pin value of comment c when it applies (resolve(c, pin.thread) is
        Default and an effective author #pin{scope: thread} exists), else None.
    AuthorTier.status(m_uri) -> (status, rank | None)
        Mod-log status of an author-scope modAction (scope_eff = author, no no-effect flag):
        "effective" | "overridden" (rank = the forum-tier rank that governs) | "expired" |
        "stale" | "unauthorised" (authorModeration, E, gate, subject or table condition).
        Records with a no-effect flag (MA2, MA7, MA9) are `reserved/unknown` (modfold.py).

The rules, as modelled (readings in ../NOTES.md, "Author tier"):

    authorEffective(m) ⇔ valid ∧ forum = F ∧ scope_eff = author ∧ MA6 (validity.py)
                         ∧ policy.authorModeration ∧ X ∈ E ∧ table condition      → else unauthorised
                         ∧ (until absent ∨ µs(until) > now)                       → else expired
                         ∧ (m positive ⇒ T ∈ records ∧ subjectCid = cid(T))       → else stale
    root(c)  = c.root.uri when c is a non-Excluded comment and its root is a non-Excluded post
               in records; no author-tier effect on c otherwise
    table    #hide{hidden: true} c:  author(root(c)) = X ∧ authorGate(X, root(c)) ∧ author(c) ∉ A
             #access{banned} d:      d ∉ A          (authorGate is applied per thread, below)
             #pin{thread, pinned} c: author(root(c)) = X ∧ authorGate(X, root(c))
             #lock{locked, allowed} p: p a non-Excluded post ∧ author(p) = X ∧ authorGate(X, p)

    authorHidden(c)    ⇔ an effective author #hide on c (its actor is author(root(c)))
    authorBanned(c)    ⇔ an effective author #access{banned} by X = author(root(c)) on author(c)
                         ∧ authorGate(X, root(c)) ∧ keepRank(c) = 0
    authorLockedOut(c) ⇔ an effective author #lock{locked: true} on p = root(c)
                         ∧ resolve(p, locked) = Default ∧ c ∉ aAllowed(p) ∧ author(c) ∉ A
                         ∧ author(c) ≠ X
    AuthorModerated(c) ⇔ resolve(c, hidden) = Default ∧ (authorHidden ∨ authorBanned ∨ authorLockedOut)

Status of an effective record: `overridden` (with the forum rank) when the forum tier has a
Resolved value for the same subject and dimension (hide: hidden; lock: locked; pin:
pin.thread), else `effective`. An author ban is never `overridden`: forum keeps act per comment.
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import KeyDerivationError, did_of, split_uri, thread_tid
from .syntax import datetime_us
from .validity import is_positive


@dataclass(frozen=True)
class AuthorTier:
    moderated: frozenset        # comment uris with AuthorModerated(c)
    pins: dict                  # comment uri -> bool, author thread pins that apply
    statuses: dict              # author-scope modAction uri -> (status, rank | None)
    hidden: frozenset = frozenset()       # comments with authorHidden(c)
    banned: frozenset = frozenset()       # comments with authorBanned(c)
    locked_out: frozenset = frozenset()   # comments with authorLockedOut(c)

    def author_moderated(self, c: str) -> bool:
        return c in self.moderated

    def thread_pin(self, c: str):
        return self.pins.get(c)

    def status(self, m_uri: str):
        return self.statuses[m_uri]


def _until_ok(value: dict, now: int) -> bool:
    if "until" not in value:
        return True
    us = datetime_us(value["until"])
    return us is not None and us > now


def _allowed_entries(fk: str, p: str, entries) -> frozenset:
    """MA9 entry filter: keep entries whose rkey starts with {fk}-{threadTid(p)}- (NOTES.md 24)."""
    try:
        prefix = f"{fk}-{thread_tid(p)}-"
    except KeyDerivationError:
        return frozenset()
    return frozenset(u for u in entries if split_uri(u)[2].startswith(prefix))


def compute_author_tier(view) -> AuthorTier:
    l1 = view.l1
    records = view.input.records
    mods = [m for m in l1.mods.values() if m.scope == "author" and m.no_effect is None]
    if not mods:
        return AuthorTier(moderated=frozenset(), pins={}, statuses={})

    A = view.l0.A
    E = view.l3.E
    pol = view.policy
    now = view.input.now

    def live_post(p: str) -> bool:
        r = view.items.get(p)
        return r is not None and r.coll == "post" and not view.excluded[p]

    def root_of(c: str):
        r = view.items.get(c)
        if r is None or r.coll != "comment" or view.excluded[c]:
            return None
        p = r.value["root"]["uri"]
        return p if live_post(p) else None

    def gate(X: str, p: str) -> bool:
        """authorGate(X, p): X ∈ A ∨ K[X] ≥ the gate of p's effective tier."""
        g = "authorModPersonal" if l1.effective(p, "tier") == "personal" else "authorModFrontpage"
        return view.passes(X, g)

    statuses = {}
    hides = set()               # comment uris with an effective author hide (actor = root author)
    bans = {}                   # actor X -> set of banned DIDs (effective author bans)
    locks = {}                  # post uri -> aAllowed(p), effective author #lock{locked: true}
    pins = {}                   # comment uri -> pinned, effective author pins
    for m in sorted(mods, key=lambda m: m.uri.encode("utf-8")):
        X, T, a = m.actor, m.subject, m.value["action"]
        name = m.action
        # table condition (MA6 already restricts action, value and subject collection)
        if name == "hide":
            p = root_of(T)
            cond = p is not None and did_of(p) == X and gate(X, p) and did_of(T) not in A
        elif name == "access":
            cond = T not in A
        elif name == "pin":
            p = root_of(T)
            cond = p is not None and did_of(p) == X and gate(X, p)
        elif name == "lock":
            cond = live_post(T) and did_of(T) == X and gate(X, T)
        else:                                       # unreachable: MA6 makes others invalid
            cond = False
        if not (pol.authorModeration and X in E and cond):
            statuses[m.uri] = ("unauthorised", None)
            continue
        if not _until_ok(m.value, now):
            statuses[m.uri] = ("expired", None)
            continue
        if is_positive(a):
            r = records.get(T)
            if r is None or m.value.get("subjectCid") != r.cid:
                statuses[m.uri] = ("stale", None)
                continue
        dim = {"hide": "hidden", "lock": "locked", "pin": "pin.thread"}.get(name)
        res = l1.resolve(T, dim) if dim is not None else None
        statuses[m.uri] = ("overridden", res.rank) if res is not None else ("effective", None)
        if name == "hide":
            hides.add(T)
        elif name == "access":
            bans.setdefault(X, set()).add(T)
        elif name == "lock":
            if a.get("locked", True) is True:
                locks[T] = _allowed_entries(view.fk, T, a.get("allowed", []))
        elif name == "pin":
            if res is None:
                pins[T] = a.get("pinned", True)

    hidden, banned, locked_out = set(), set(), set()
    for c, r in view.items.items():
        p = root_of(c)
        if p is None or l1.resolve(c, "hidden") is not None:
            continue
        X, d = did_of(p), r.did
        if c in hides:
            hidden.add(c)
        if d in bans.get(X, ()) and gate(X, p) and l1.keep_rank(c) == 0:
            banned.add(c)
        if p in locks and l1.resolve(p, "locked") is None and c not in locks[p] \
                and d not in A and d != X:
            locked_out.add(c)
    return AuthorTier(moderated=frozenset(hidden | banned | locked_out), pins=pins,
                      statuses=statuses, hidden=frozenset(hidden), banned=frozenset(banned),
                      locked_out=frozenset(locked_out))
