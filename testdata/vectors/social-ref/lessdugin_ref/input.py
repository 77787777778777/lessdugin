# SPDX-License-Identifier: CC0-1.0
"""Parsing and shape validation of the vector input: {fold: FoldInput, query?: Query}.

FoldInput mirrors social.md §1.1 exactly:

    {
      "decl":         {"uri": F, "cid": cid, "value": forum record},
      "records":      [{"uri", "cid", "value"}, …]      sorted strictly ascending by uri bytes
      "absent":       [uri, …]                           sorted strictly ascending
      "obs":          [{"uri", "cid", "us"}, …]          sorted strictly ascending by (uri, cid)
      "repoStatus":   {did: "fresh" | "stale" | "unavailable"}
      "now":          µs (integer)
      "algorithmIds": {"moderation", "karma", "ranking", "reactPalette"}   optional
    }

A malformed input raises InputError: a vector must never rely on a lenient reading.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .common import (FK_RE, InputError, coll_name, fk, split_uri, ubytes)
from .syntax import is_cid, is_did, is_nsid, is_rkey

import re

FOLD_KEYS_REQUIRED = ("decl", "records", "absent", "obs", "repoStatus", "now")
FOLD_KEYS_OPTIONAL = ("algorithmIds",)
REPO_STATUS = ("fresh", "stale", "unavailable")
ALG_KEYS = ("moderation", "karma", "ranking", "reactPalette")
RECORD_COLLECTIONS = ("membership", "post", "comment", "revision", "opinion", "modAction",
                      "wikiRevision", "sequence")


@dataclass(frozen=True)
class Record:
    uri: str
    cid: str
    value: dict

    @property
    def did(self) -> str:
        return split_uri(self.uri)[0]

    @property
    def coll(self) -> str:
        return coll_name(split_uri(self.uri)[1])

    @property
    def rkey(self) -> str:
        return split_uri(self.uri)[2]


@dataclass
class FoldInput:
    decl: Record
    records: dict          # uri -> Record, in uri byte order
    absent: tuple          # sorted uris
    obs: dict              # (uri, cid) -> µs
    repo_status: dict      # did -> "fresh" | "stale" | "unavailable"
    now: int
    algorithm_ids: dict | None = None
    raw: dict = field(default=None, repr=False)

    @property
    def forum(self) -> str:
        return self.decl.uri

    def status(self, did: str) -> str:
        """repoStatus[did]; a DID missing from the map is Unavailable (social.md §1.1)."""
        return self.repo_status.get(did, "unavailable")


def _is_int(x) -> bool:
    return isinstance(x, int) and not isinstance(x, bool)


def _record(obj, where: str) -> Record:
    if not isinstance(obj, dict) or set(obj) != {"uri", "cid", "value"}:
        raise InputError(f"{where}: expected exactly {{uri, cid, value}}")
    uri, cid, value = obj["uri"], obj["cid"], obj["value"]
    parts = split_uri(uri)
    if parts is None or not (is_did(parts[0]) and is_nsid(parts[1]) and is_rkey(parts[2])):
        raise InputError(f"{where}: not an at-uri of a record: {uri!r}")
    if not is_cid(cid):
        raise InputError(f"{where}: not a CID: {cid!r}")
    if not isinstance(value, dict):
        raise InputError(f"{where}: value is not an object")
    return Record(uri, cid, value)


def parse_fold_input(obj) -> FoldInput:
    if not isinstance(obj, dict):
        raise InputError("fold input is not an object")
    missing = [k for k in FOLD_KEYS_REQUIRED if k not in obj]
    extra = [k for k in obj if k not in FOLD_KEYS_REQUIRED + FOLD_KEYS_OPTIONAL]
    if missing or extra:
        raise InputError(f"fold input keys: missing {missing}, unknown {extra}")

    decl = _record(obj["decl"], "decl")
    if coll_name(split_uri(decl.uri)[1]) != "forum":
        raise InputError("decl.uri is not a forum uri")
    F = decl.uri
    FK = fk(F)

    if not isinstance(obj["records"], list):
        raise InputError("records is not an array")
    records = {}
    prev = None
    for i, r in enumerate(obj["records"]):
        rec = _record(r, f"records[{i}]")
        if prev is not None and ubytes(rec.uri) <= ubytes(prev):
            raise InputError(f"records[{i}]: not sorted strictly ascending by uri ({rec.uri})")
        prev = rec.uri
        short = coll_name(split_uri(rec.uri)[1])
        if short not in RECORD_COLLECTIONS:
            raise InputError(f"records[{i}]: collection is never a fold input: {rec.uri}")
        # §1.1: memberships with rkey fk; everything else in the {fk}- key range
        if short == "membership":
            if rec.rkey != FK:
                raise InputError(f"records[{i}]: membership rkey is not fk(F)")
        elif not (rec.rkey.startswith(FK + "-") and re.fullmatch(FK_RE, rec.rkey[:12])):
            raise InputError(f"records[{i}]: rkey outside the {{fk}}- range")
        records[rec.uri] = rec

    absent = obj["absent"]
    if not isinstance(absent, list) or not all(isinstance(u, str) for u in absent):
        raise InputError("absent is not an array of strings")
    if [ubytes(u) for u in absent] != sorted(set(ubytes(u) for u in absent)):
        raise InputError("absent is not sorted strictly ascending")
    subjects = set()
    for rec in records.values():
        if rec.coll == "opinion":
            s = rec.value.get("subject")
            if isinstance(s, dict) and isinstance(s.get("uri"), str):
                subjects.add(s["uri"])
    for u in absent:
        if u in records:
            raise InputError(f"absent uri is also in records: {u}")
        if u not in subjects:
            raise InputError(f"absent uri is not the subject of an opinion in records: {u}")

    if not isinstance(obj["obs"], list):
        raise InputError("obs is not an array")
    obs = {}
    prev = None
    for i, o in enumerate(obj["obs"]):
        if not isinstance(o, dict) or set(o) != {"uri", "cid", "us"}:
            raise InputError(f"obs[{i}]: expected exactly {{uri, cid, us}}")
        if not (isinstance(o["uri"], str) and is_cid(o["cid"]) and _is_int(o["us"])):
            raise InputError(f"obs[{i}]: bad types")
        key = (ubytes(o["uri"]), ubytes(o["cid"]))
        if prev is not None and key <= prev:
            raise InputError(f"obs[{i}]: not sorted strictly ascending by (uri, cid)")
        prev = key
        obs[(o["uri"], o["cid"])] = o["us"]

    rs = obj["repoStatus"]
    if not isinstance(rs, dict):
        raise InputError("repoStatus is not an object")
    for d, s in rs.items():
        if not is_did(d) or s not in REPO_STATUS:
            raise InputError(f"repoStatus[{d!r}] = {s!r}")

    now = obj["now"]
    if not _is_int(now):
        raise InputError("now is not an integer")

    algs = obj.get("algorithmIds")
    if algs is not None:
        if not isinstance(algs, dict) or set(algs) != set(ALG_KEYS) or \
                not all(isinstance(v, str) for v in algs.values()):
            raise InputError(f"algorithmIds must have exactly the keys {ALG_KEYS}")

    return FoldInput(decl=decl, records=records, absent=tuple(absent), obs=obs,
                     repo_status=dict(rs), now=now, algorithm_ids=algs, raw=obj)


def parse_vector_input(obj):
    """{fold, query?} -> (FoldInput, query dict or None)."""
    if not isinstance(obj, dict) or "fold" not in obj or set(obj) - {"fold", "query"}:
        raise InputError("vector input must be {fold, query?}")
    q = obj.get("query")
    if q is not None and not isinstance(q, dict):
        raise InputError("query is not an object")
    return parse_fold_input(obj["fold"]), q
