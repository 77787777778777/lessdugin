# SPDX-License-Identifier: CC0-1.0
"""Builders for vector inputs: genuine, valid records with canonical keys and real CIDs.

    did("alice")      -> did:plc:alice2222222222222222222  (readable; [a-z2-7]{24} after the prefix)
    dt(us)            -> RFC 3339 UTC datetime, millisecond precision (microseconds if needed)
    tid(us, clock)    -> the TID of (us << 10 | clock)
    cid(value)        -> CIDv1 dag-cbor sha256 of the value (base32)

Forum(...) builds a declaration and its records; Forum.fold_input(records, …) assembles the
FoldInput JSON of social.md §1.1 (records sorted by uri, obs defaulting to each version's
claimed time, repoStatus defaulting to "fresh" for every record owner and authority).
"""

from __future__ import annotations

import itertools
import re

from . import dagcbor
from .common import NS, B32S_ALPHABET, fk, h16, nsid, split_uri, stid, thread_tid, ubytes, action_key
from .syntax import datetime_us

T0 = 1_767_225_600_000_000          # 2026-01-01T00:00:00Z
SEC = 1_000_000
MIN = 60 * SEC
HOUR = 60 * MIN
DAY = 24 * HOUR
NOW = T0 + 30 * DAY                 # 2026-01-31T00:00:00Z


def did(name: str) -> str:
    if not re.fullmatch(r"[a-z2-7]{1,24}", name):
        raise ValueError(name)
    return "did:plc:" + name.ljust(24, "2")


def dt(us: int) -> str:
    days, rem = divmod(us, 86_400_000_000)
    # civil_from_days (H. Hinnant)
    z = days + 719468
    era = (z if z >= 0 else z - 146096) // 146097
    doe = z - era * 146097
    yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
    y = yoe + era * 400
    doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
    mp = (5 * doy + 2) // 153
    d = doy - (153 * mp + 2) // 5 + 1
    m = mp + 3 if mp < 10 else mp - 9
    y += m <= 2
    secs, micro = divmod(rem, 1_000_000)
    hh, mm, ss = secs // 3600, secs // 60 % 60, secs % 60
    frac = f"{micro // 1000:03d}" if micro % 1000 == 0 else f"{micro:06d}"
    s = f"{y:04d}-{m:02d}-{d:02d}T{hh:02d}:{mm:02d}:{ss:02d}.{frac}Z"
    assert datetime_us(s) == us, (s, us)
    return s


def tid(us: int, clock: int = 0) -> str:
    n = (us << 10) | clock
    if n >> 63:
        raise ValueError("TID out of range")
    return "".join(B32S_ALPHABET[(n >> (5 * i)) & 31] for i in range(12, -1, -1))


def cid(value) -> str:
    return dagcbor.cid_of(value)


def md(text: str) -> dict:
    return {"$type": nsid("markdown"), "text": text}


def rec(uri: str, value: dict) -> dict:
    return {"uri": uri, "cid": cid(value), "value": value}


def at(who: str, coll: str, rkey: str) -> str:
    return f"at://{who}/{nsid(coll)}/{rkey}"


def sref(r: dict) -> dict:
    return {"uri": r["uri"], "cid": r["cid"]}


class Forum:
    def __init__(self, owner="owner", slug="test", created=T0, membership="open", moderators=(),
                 policy=None, **extra):
        self.owner = did(owner)
        self.uri = at(self.owner, "forum", slug)
        self.fk = fk(self.uri)
        value = {"$type": nsid("forum"), "name": "Test forum", "createdAt": dt(created)}
        if membership is not None:
            value["membership"] = membership
        if moderators:
            value["moderators"] = [{"did": did(n), "role": r} for n, r in moderators]
        if policy is not None:
            value["policy"] = policy
        value.update(extra)
        self.decl = rec(self.uri, value)
        self._clock = itertools.count(1)
        self.obs_override = {}                  # (uri, cid) -> µs | None (None: no obs entry)
        self.extra_obs = {}                     # (uri, cid) -> µs, for earlier versions

    # ------------------------------------------------------------------ records
    def _clk(self, clock):
        return next(self._clock) % 1024 if clock is None else clock

    def membership(self, who, t=T0, forum=None, **extra):
        value = {"$type": nsid("membership"), "forum": forum or self.uri, "createdAt": dt(t), **extra}
        return rec(at(did(who), "membership", self.fk), value)

    def post(self, who, t, title="A post", clock=None, tid_us=None, created=None, **extra):
        rk = f"{self.fk}-{tid(t if tid_us is None else tid_us, self._clk(clock))}"
        value = {"$type": nsid("post"), "forum": self.uri, "createdAt": created or dt(t)}
        if title is not None:
            value["title"] = title
        value.update(extra)
        return rec(at(did(who), "post", rk), value)

    def comment(self, who, root, t, parent=None, clock=None, tid_us=None, created=None, text="A comment",
                **extra):
        """root, parent: a record dict or a (uri, cid) pair."""
        ru, rc = (root["uri"], root["cid"]) if isinstance(root, dict) else root
        rk = f"{self.fk}-{thread_tid(ru)}-{tid(t if tid_us is None else tid_us, self._clk(clock))}"
        value = {"$type": nsid("comment"), "root": {"uri": ru, "cid": rc}, "forum": self.uri,
                 "body": md(text), "createdAt": created or dt(t)}
        if parent is not None:
            pu, pc = (parent["uri"], parent["cid"]) if isinstance(parent, dict) else parent
            value["parent"] = {"uri": pu, "cid": pc}
        value.update(extra)
        return rec(at(did(who), "comment", rk), value)

    def comment_uri(self, who, root_uri, us, clock):
        return at(did(who), "comment", f"{self.fk}-{thread_tid(root_uri)}-{tid(us, clock)}")

    def wiki(self, who, t, slug, name=None, parents=(), clock=None, **extra):
        rk = f"{self.fk}-{tid(t, self._clk(clock))}"
        value = {"$type": nsid("wikiRevision"), "forum": self.uri, "slug": slug, "name": name or slug,
                 "createdAt": dt(t)}
        if parents:
            value["parents"] = [sref(p) if isinstance(p, dict) else {"uri": p[0], "cid": p[1]} for p in parents]
        value.update(extra)
        return rec(at(did(who), "wikiRevision", rk), value)

    def sequence(self, who, t, title="A sequence", chapters=((),), clock=None, **extra):
        rk = f"{self.fk}-{tid(t, self._clk(clock))}"
        value = {"$type": nsid("sequence"), "forum": self.uri, "title": title, "createdAt": dt(t),
                 "chapters": [{"posts": [p if isinstance(p, str) else p["uri"] for p in ch]} for ch in chapters]}
        value.update(extra)
        return rec(at(did(who), "sequence", rk), value)

    def modaction(self, actor, action, subject, *, scope=None, subject_cid=None, t=T0, until=None,
                  reason=None, adopts=None, rkey=None, updated=None, **fields):
        """action: short name (hide, access, …) or a full $type; subject: DID, uri, tag:slug or a record."""
        if isinstance(subject, dict):
            if subject_cid is None:
                subject_cid = subject["cid"]
            subject = subject["uri"]
        t_ = action if "." in action else f"{nsid('modAction')}#{action}"
        a = {"$type": t_, **fields}
        value = {"$type": nsid("modAction"), "forum": self.uri, "subject": subject, "action": a,
                 "createdAt": dt(t)}
        if scope is not None:
            value["scope"] = scope
        if subject_cid is not None and subject_cid is not False:
            value["subjectCid"] = subject_cid
        if until is not None:
            value["until"] = until if isinstance(until, str) else dt(until)
        if reason is not None:
            value["reason"] = reason
        if adopts is not None:
            value["adopts"] = sref(adopts)
        if updated is not None:
            value["updatedAt"] = dt(updated)
        if rkey is None:
            rkey = f"{self.fk}-{h16('modAction', [scope or 'forum', subject, action_key(a)])}"
        return rec(at(did(actor) if not actor.startswith("did:") else actor, "modAction", rkey), value)

    def revision(self, subject, op, t, record=None, clock=None):
        """A revision in the subject's repo; `record` is the prior value (its CID must be subject.cid)."""
        who = split_uri(subject["uri"])[0]
        from .common import P
        rk = f"{P(subject['uri'])}-{tid(t, self._clk(clock))}"
        value = {"$type": nsid("revision"), "subject": sref(subject), "op": op, "createdAt": dt(t)}
        if record is not None:
            value["record"] = record
        return rec(at(who, "revision", rk), value)

    def opinion(self, who, subject, t=T0, **axes):
        su = subject["uri"]
        rk = f"{self.fk}-{stid(su)}-{h16('opinion', [su])}"
        value = {"$type": nsid("opinion"), "subject": sref(subject), "forum": self.uri, "createdAt": dt(t),
                 **axes}
        return rec(at(did(who), "opinion", rk), value)

    # ------------------------------------------------------------------ input
    def fold_input(self, records, *, absent=(), now=NOW, repo_status=None, algorithm_ids=None,
                   decl=None, obs=None):
        decl = decl or self.decl
        recs = sorted(records, key=lambda r: ubytes(r["uri"]))
        uris = [r["uri"] for r in recs]
        assert len(set(uris)) == len(uris), "duplicate uri"
        if obs is None:
            o = {}
            for r in [decl, *recs]:
                k = (r["uri"], r["cid"])
                if k in self.obs_override:
                    if self.obs_override[k] is not None:
                        o[k] = self.obs_override[k]
                    continue
                v = r["value"]
                us = datetime_us(v.get("updatedAt") or v.get("createdAt"))
                if us is not None:
                    o[k] = us
            o.update(self.extra_obs)
        else:
            o = dict(obs)
        obs_list = [{"uri": u, "cid": c, "us": us}
                    for (u, c), us in sorted(o.items(), key=lambda kv: (ubytes(kv[0][0]), ubytes(kv[0][1])))]
        rs = {}
        for m in decl["value"].get("moderators", []):
            rs[m["did"]] = "fresh"
        rs[split_uri(decl["uri"])[0]] = "fresh"
        for r in recs:
            rs[split_uri(r["uri"])[0]] = "fresh"
        if repo_status:
            for k, v in repo_status.items():
                if v is None:
                    rs.pop(k, None)
                else:
                    rs[k] = v
        fi = {"decl": decl, "records": recs, "absent": sorted(absent, key=ubytes), "obs": obs_list,
              "repoStatus": dict(sorted(rs.items())), "now": now}
        if algorithm_ids is not None:
            fi["algorithmIds"] = algorithm_ids
        return fi
