# SPDX-License-Identifier: CC0-1.0
"""Versions and the edited badge (social.md §11 step 6 and "Edited"; §20.1 items 45, 46).

    versions(view) -> dict comment uri -> {"root"?: n | "unavailable", "parent"?: n | "unavailable"}
        For every comment that V1 does not exclude, each reference (`root`, `parent`) whose CID
        names another version than the target's current one. The target (the root post, the
        parent comment) must be an item of the matching collection in `records` that V1 does not
        exclude; a reference to an absent or Excluded target shows no version (a Missing node).
        Versions of a target: its valid `op: update` revisions R1 … Rk in (TID of the rkey, uri)
        order; version i is the snapshot with CID Ri.subject.cid, version k + 1 the current one.
        n is the greatest i whose CID equals the referenced one; none -> "unavailable". CIDs
        compare by canonical form (§0.4, cids.py): a reference spelled differently but naming the
        current version is not listed; one that is not a CIDv1 is "unavailable".
        A parent comment that is in `records` and not Excluded but under another root (a Missing
        placeholder in this thread, §11 step 2) is not modelled (NotModelled): the spec does not
        say whether such a reference shows a version.
    edited(view) -> sorted uris
        Every candidate item that V1 does not exclude and has a valid `op: update` revision, or
        µs(updatedAt) later than µs(createdAt). A createdAt without µs (before 1970) counts as
        earlier than any updatedAt that has one; an updatedAt without µs never makes an item
        edited.
"""

from __future__ import annotations

from . import cids
from .common import NotModelled, tid_int, ubytes
from .syntax import datetime_us


def _updates(view) -> dict:
    """target uri -> [snapshot CIDs] in version order (valid op: update revisions)."""
    by = {}
    for u, r in view.input.records.items():
        if r.coll == "revision" and view.validity[u].valid and r.value["op"] == "update":
            s = r.value["subject"]
            by.setdefault(s["uri"], []).append((tid_int(r.rkey.split("-")[-1]), ubytes(u), s["cid"]))
    return {t: [c for _, _, c in sorted(xs)] for t, xs in by.items()}


def _version(view, target: str, ref_cid, updates: dict):
    """None when the reference names the current version, else n or "unavailable"."""
    cur = view.input.records[target].cid
    if cids.same(ref_cid, cur):
        return None
    c = cids.canon(ref_cid)
    n = None
    for i, snap in enumerate(updates.get(target, []), 1):
        if c is not None and cids.same(snap, c):
            n = i
    return "unavailable" if n is None else n


def versions(view) -> dict:
    updates = _updates(view)
    items = view.items
    out = {}
    for c, r in items.items():
        if r.coll != "comment" or view.excluded[c]:
            continue
        e = {}
        root = r.value["root"]
        p = root["uri"]
        if p in items and items[p].coll == "post" and not view.excluded[p]:
            n = _version(view, p, root["cid"], updates)
            if n is not None:
                e["root"] = n
        par = r.value.get("parent")
        if par is not None:
            q = par["uri"]
            if q in items and items[q].coll == "comment" and not view.excluded[q]:
                if items[q].value["root"]["uri"] != p:
                    raise NotModelled(f"{c}: parent {q} is under another root")
                n = _version(view, q, par["cid"], updates)
                if n is not None:
                    e["parent"] = n
        if e:
            out[c] = e
    return {c: out[c] for c in sorted(out, key=ubytes)}


def edited(view) -> list:
    revised = set(_updates(view))
    out = []
    for u, r in view.items.items():
        if view.excluded[u]:
            continue
        if u in revised:
            out.append(u)
            continue
        v = r.value
        if "updatedAt" not in v:
            continue
        up, cr = datetime_us(v.get("updatedAt")), datetime_us(v.get("createdAt"))
        if up is not None and (cr is None or up > cr):
            out.append(u)
    return sorted(out, key=ubytes)
