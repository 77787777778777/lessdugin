# SPDX-License-Identifier: CC0-1.0
"""Visibility (social.md §10.1–§10.2).

mod_state() is rules V1–V7: V1–V5 read only validity and L0–L2, V7 reads L1 and A, and V6
asks the L5 author tier (authormod.py). state() continues with V8–V10, which read E (L3) and
score (L3.5). thread_pass() turns hidden nodes with a listed descendant into tombstones.

lens() is L7 (§10.3): local presentation applied after the fold, from local state only. It
returns a new map and never changes a fold output; the vectors never assert it (the case
modules only check that applying it leaves every projected field unchanged).
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import ITEM_COLLECTIONS, NS

CAP = {"post": "post", "comment": "comment", "sequence": "sequence", "wikiRevision": "wiki"}


@dataclass(frozen=True)
class State:
    state: str                      # Visible | Collapsed | Hidden | Excluded | Tombstone
    reason: str | None = None       # NotParticipant | ModerationUnavailable | Removed | Banned |
                                    # Restricted | AuthorModerated | Locked | NewUser | LowScore
    removed: tuple | None = None    # Removed{rank, actor, spam, reason if showReason}

    @property
    def listed(self) -> bool:
        return self.state in ("Visible", "Collapsed")

    def to_json(self) -> dict:
        out = {"state": self.state}
        if self.reason is not None:
            out["reason"] = self.reason
        if self.removed is not None:
            rank, actor, spam, reason = self.removed
            out["removed"] = {"rank": rank, "actor": actor, "spam": spam, "reason": reason}
        return out


EXCLUDED = State("Excluded")


def v1_excluded(view, uri: str) -> bool:
    """V1: invalid, x.forum ≠ F, or CO5 (a valid root post in the input has the same forum).

    CO5 compares only a valid root (social.md §20.1 item 51, NOTES.md 20): an invalid root is
    a Missing root (NOTES.md 3), and its comments stay in the thread.
    """
    rec = view.input.records[uri]
    if not view.validity[uri].valid or rec.value.get("forum") != view.F:
        return True
    if rec.coll == "comment":
        r = rec.value["root"]["uri"]
        root = view.input.records.get(r)
        if root is not None and view.validity[r].valid and root.value.get("forum") != rec.value["forum"]:
            return True
    return False


def mod_state(view, uri: str):
    """The state given by the first matching rule among V1–V7, or None if none matches."""
    rec = view.input.records[uri]
    if rec.coll not in ITEM_COLLECTIONS:
        raise ValueError(uri)
    if view.excluded[uri]:
        return EXCLUDED
    a = rec.did
    l1, l2 = view.l1, view.l2
    if a not in l2.P:                                                         # V2
        return State("Hidden", "NotParticipant")
    if view.degraded and a not in l2.S:
        return State("Hidden", "ModerationUnavailable")
    res = l1.resolve(uri, "hidden")                                           # V3
    if res is not None and res.value is True:
        w = l1.mods[res.winner]
        action = w.value["action"]
        reason = w.value.get("reason") if action.get("showReason", True) else None
        spam = l1.effective(uri, "spam")                                      # NOTES.md 8
        return State("Hidden", "Removed", (res.rank, w.actor, spam, reason))
    keep = l1.keep_rank(uri)
    br = l1.ban_rank(a)                                                       # V4
    if br is not None and keep < br:
        return State("Hidden", "Banned")
    if CAP[rec.coll] in l1.effective(a, "restrict") and keep < l1.restrict_rank(a):   # V5
        return State("Hidden", "Restricted")
    if rec.coll == "comment":
        if view.l5_author.author_moderated(uri):                              # V6
            return State("Hidden", "AuthorModerated")
        p = rec.value["root"]["uri"]                                          # V7
        if l1.effective(p, "locked") is True and uri not in l1.allowed(p) \
                and a not in view.l0.A and keep < l1.lock_rank(p):
            return State("Hidden", "Locked")
    return None


def state(view, uri: str) -> State:
    """V1–V10 (before the thread pass)."""
    s = mod_state(view, uri)
    if s is not None:
        return s
    a = view.input.records[uri].did
    pol = view.policy
    if pol.membership == "open" and a not in (view.l2.S | view.l3.E):          # V8
        if pol.newUsers == "collapse":
            return State("Collapsed", "NewUser")
        if pol.newUsers == "hide":
            return State("Hidden", "NewUser")
    if view.l35[uri].score <= pol.collapseBelow:                               # V9
        return State("Collapsed", "LowScore")
    return State("Visible")                                                    # V10


def thread_pass(view, states: dict) -> dict:
    """§10.2: a Hidden post or comment with a listed descendant becomes Tombstone(reason)."""
    out = dict(states)
    for p, th in view.threads.items():
        listed_below = {}

        def has_listed(n):
            # iterative post-order over the placed tree
            order, stack = [], [n]
            while stack:
                x = stack.pop()
                order.append(x)
                stack.extend(th.children.get(x, []))
            for x in reversed(order):
                lb = False
                for c in th.children.get(x, []):
                    own = c not in th.missing and states[c].listed
                    lb = lb or own or listed_below[c]
                listed_below[x] = lb
            return listed_below[n]

        has_listed(p)
        for n, lb in listed_below.items():
            if n == p:
                if not th.root_missing and lb and states[p].state == "Hidden":
                    out[p] = State("Tombstone", states[p].reason, states[p].removed)
            elif n not in th.missing and lb and states[n].state == "Hidden":
                out[n] = State("Tombstone", states[n].reason, states[n].removed)
    return out


MUTED = State("Collapsed", "Muted")


def lens(view, displayed: dict, mutes=frozenset()) -> dict:
    """L7 mutes (§10.3): a listed item by a muted author becomes Collapsed(Muted).

    `mutes` is the viewer's local mute list (blocks treated as mutes are the same set). Pure:
    reads only the given displayed states and record authors, returns a new map.
    """
    out = {}
    for u, s in displayed.items():
        out[u] = MUTED if s.listed and view.input.records[u].did in mutes else s
    return out


__all__ = ["State", "mod_state", "state", "thread_pass", "v1_excluded", "lens", "NS"]
