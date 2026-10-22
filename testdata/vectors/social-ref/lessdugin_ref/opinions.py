# SPDX-License-Identifier: CC0-1.0
"""L3: opinion validity (social.md §5, karma/1) and the per-axis rules of its table.

    counting(view) -> dict opinion uri -> Counted
        Every opinion that counts (O1–O5), in uri order. That includes opinions whose subject
        is in `absent` (the second branch of O2), whatever the subject's kind and however many
        voters it has: the deleted-subject rule only limits what such an opinion counts toward
        (karma only, per stage, post or comment subjects; karma.py), it does not stop it from
        counting (NOTES.md 30).
    reason(view, uri) -> None | "O1" | "O2" | "O3" | "O4" | "O5"
        The first of O1–O5 that an opinion fails (None if it counts). Diagnostics and case
        assertions only; never projected.
    axis_enabled(view, c, axis) -> bool
        The §5 axis table for axis ∈ karma | agreement | reacts | tags. The karma and agreement
        values themselves are already resolved on Counted (None when the axis is absent,
        unknown or disabled). reacts.py and tags.py read their entries from the record and
        apply their entry rules (OP3, OP4, §9.2) after checking axis_enabled.

Reads L0–L2 only: validity and V1 (view.excluded), view.l1 (spam, restrict), view.l2 (B, N),
view.policy (voting) and `absent`.
"""

from __future__ import annotations

from dataclasses import dataclass

from .common import coll_of, did_of

KARMA_VALUES = ("up", "down", "strongUp", "strongDown")
AGREEMENT_VALUES = ("agree", "disagree", "strongAgree", "strongDisagree")
AXES = ("karma", "agreement", "reacts", "tags")


@dataclass(frozen=True)
class Counted:
    """A counting opinion (§5), with its karma and agreement axes resolved."""
    uri: str
    voter: str                 # did(v.uri)
    subject: str               # v.subject.uri
    subject_cid: str           # v.subject.cid: drives "earlier version" only (§7), never counting
    absent: bool               # v.subject.uri ∈ absent
    karma: str | None          # up | down | strongUp | strongDown; None: absent, unknown (OP3) or
                               # disabled (voter restricted from `vote`)
    agreement: str | None      # agree | …; None: absent, unknown, restricted from `vote`, or
                               # policy.voting = karmaOnly

    @property
    def recipient(self) -> str:
        """recipient(v) = did(v.subject.uri) (§6.2)."""
        return did_of(self.subject)

    @property
    def subject_coll(self) -> str:
        return coll_of(self.subject)


def restricted(view, did: str, capability: str) -> bool:
    """capability ∈ effective(did, restrict) (§4.7)."""
    return capability in view.l1.effective(did, "restrict")


def _absent(view) -> frozenset:
    return frozenset(view.input.absent)


def reason(view, uri: str, absent: frozenset | None = None):
    """The first failing rule among O1–O5 for the opinion at uri, or None if it counts."""
    if absent is None:
        absent = _absent(view)
    rec = view.input.records[uri]
    if rec.coll != "opinion":
        raise ValueError(uri)
    v = rec.value
    # O1: valid (G1–G10, OP1, OP2) and v.forum = F
    if not view.validity[uri].valid or v.get("forum") != view.F:
        return "O1"
    s = v["subject"]["uri"]
    # O2: the subject is a non-Excluded record in F (valid, same forum, CO5), or s ∈ absent
    in_records = s in view.input.records and s in view.excluded and not view.excluded[s]
    if not (in_records or s in absent):
        return "O2"
    voter = rec.did
    if voter == did_of(s):                                   # O3: self-opinions
        return "O3"
    if voter in view.l2.B or voter in view.l2.N:             # O4
        return "O4"
    if view.l1.effective(s, "spam") is True:                 # O5 (on the uri, even if absent)
        return "O5"
    return None


def counting(view) -> dict:
    out = {}
    voting = view.policy.voting
    absent = _absent(view)
    for uri, rec in view.input.records.items():
        if rec.coll != "opinion" or reason(view, uri, absent) is not None:
            continue
        v = rec.value
        voter = rec.did
        no_vote = restricted(view, voter, "vote")
        k = v.get("karma")
        a = v.get("agreement")
        karma = k if (k in KARMA_VALUES and not no_vote) else None
        agreement = a if (a in AGREEMENT_VALUES and not no_vote and voting in ("twoAxis", "reacts")) else None
        s = v["subject"]
        out[uri] = Counted(uri=uri, voter=voter, subject=s["uri"], subject_cid=s["cid"],
                           absent=s["uri"] in absent, karma=karma, agreement=agreement)
    return out


def axis_enabled(view, c: Counted, axis: str) -> bool:
    """The §5 axis table, for a counting opinion c.

    karma       a known value, not restricted from `vote`               (c.karma is not None)
    agreement   a known value, not restricted from `vote`, voting ∈ {twoAxis, reacts}
    reacts      not restricted from `react`, voting = reacts; the subject is a record (opinions
                on absent subjects count only toward karma); palette and stance are entry rules
    tags        not restricted from `tag`; the subject is a post (not absent); OP4 entry rules
    """
    if axis == "karma":
        return c.karma is not None
    if axis == "agreement":
        return c.agreement is not None
    if axis == "reacts":
        return (not c.absent and view.policy.voting == "reacts"
                and not restricted(view, c.voter, "react"))
    if axis == "tags":
        return not c.absent and c.subject_coll == "post" and not restricted(view, c.voter, "tag")
    raise ValueError(axis)


__all__ = ["Counted", "counting", "reason", "axis_enabled", "restricted", "KARMA_VALUES",
           "AGREEMENT_VALUES", "AXES"]
