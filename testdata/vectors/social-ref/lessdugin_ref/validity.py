# SPDX-License-Identifier: CC0-1.0
"""Record validity (data-model.md §7): the per-record rules the fold depends on.

`check(uri, value)` returns a Validity. Implemented: G1–G10 and FO1, ME1, PO1–PO6, CO1–CO4,
RV1–RV5, OP1–OP2, MA1–MA12, WR1–WR2, SQ1–SQ3 (the *entry* rules FO2–FO4, PO7, PO8, CO6, OP3,
OP4, MA8, MA9-entries, WR2-duplicates and SQ2-display are applied by the fold where they
matter). CO5 is a cross-record rule (V1) and lives in the fold.

G2 is driven by the lexicon JSON files in lexicons/online/7778777/lessdugin/ (data-model.md
wins over them until M7; the two agree on every field used here). Anything the model cannot
decide exactly raises NotModelled (for example a maxGraphemes check that needs UAX #29
segmentation beyond ASCII).

`rule` is the first failing rule in the order G1, G2, G3, G4, G5, G6, G7, G8, G10, then the
collection's rules in numeric order. The vectors never assert rule IDs (NOTES.md 1).
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

from . import dagcbor
from .common import (FK_RE, NS, TID_RE, H16_RE, KeyDerivationError, NotModelled, P, action_key,
                     coll_name, fk, h16, is_slug, nsid, split_uri, stid, thread_tid)
from .syntax import (has_non_white_space, is_canonical_at_uri, is_canonical_did, is_cid, is_datetime,
                     is_did, is_uri, is_canonical_at_uri as canon)

LEXICON_DIR = Path(__file__).resolve().parents[4] / "lexicons" / "online" / "7778777" / "lessdugin"

RECORD_CBOR_MAX_BYTES = 1_000_000      # G6
NESTING_MAX_RECORD = 32                # G10

SPILLABLE = {("post", "body"), ("wikiRevision", "body"), ("forum", "about"), ("sequence", "body")}


@dataclass(frozen=True)
class Validity:
    valid: bool
    rule: str | None = None           # first failing rule, when invalid
    no_effect: str | None = None      # valid but without effect: "MA2" (reserved key), "MA7", "MA9"
    detail: str | None = None

    @staticmethod
    def ok(no_effect=None):
        return Validity(True, None, no_effect)


class _Invalid(Exception):
    def __init__(self, rule: str, detail: str = ""):
        super().__init__(f"{rule}: {detail}")
        self.rule = rule
        self.detail = detail


# --------------------------------------------------------------------------- lexicons (G2, G3)

_BUILTIN = {
    "com.atproto.repo.strongRef": {"defs": {"main": {
        "type": "object", "required": ["uri", "cid"],
        "properties": {"uri": {"type": "string", "format": "at-uri"},
                       "cid": {"type": "string", "format": "cid"}}}}},
    "com.atproto.label.defs": {"defs": {
        "selfLabels": {"type": "object", "required": ["values"],
                       "properties": {"values": {"type": "array", "items": {"type": "ref", "ref": "#selfLabel"},
                                                 "maxLength": 10}}},
        "selfLabel": {"type": "object", "required": ["val"],
                      "properties": {"val": {"type": "string", "maxLength": 128}}}}},
}


@lru_cache(maxsize=None)
def _lexicons() -> dict:
    docs = dict(_BUILTIN)
    for p in sorted(LEXICON_DIR.glob("*.json")):
        with open(p, encoding="utf-8") as f:
            d = json.load(f)
        docs[d["id"]] = d
    return docs


def _resolve(ref: str, doc_id: str):
    if ref.startswith("#"):
        target_doc, name = doc_id, ref[1:]
    elif "#" in ref:
        target_doc, name = ref.split("#", 1)
    else:
        target_doc, name = ref, "main"
    doc = _lexicons().get(target_doc)
    if doc is None or name not in doc["defs"]:
        raise NotModelled(f"lexicon ref not available: {ref} (from {doc_id})")
    return doc["defs"][name], target_doc, f"{target_doc}#{name}" if name != "main" else target_doc


def _graphemes_ok(s: str, max_g: int) -> bool:
    if len(s) <= max_g:
        return True                       # graphemes ≤ code points
    if all(ord(c) < 0x80 for c in s):
        return len(s) - s.count("\r\n") <= max_g
    raise NotModelled("maxGraphemes needs UAX #29 segmentation of non-ASCII text")


def _mime_ok(mime: str, accept) -> bool:
    for a in accept:
        if a == "*/*" or a == mime:
            return True
        if a.endswith("/*") and mime.startswith(a[:-1]):
            return True
    return False


_FORMATS = {
    "did": is_did, "cid": is_cid, "datetime": is_datetime, "uri": is_uri,
    # every at-uri field of the v1 lexicons is a reference field: G2 checks the general shape,
    # G4 the canonical form and the collection.
    "at-uri": lambda s: isinstance(s, str) and s.startswith("at://") and len(s.encode()) <= 8192,
}


def _is_int(x) -> bool:
    return isinstance(x, int) and not isinstance(x, bool)


def _validate(v, schema: dict, doc_id: str, path: str) -> None:
    t = schema["type"]
    if t == "ref":
        target, tdoc, _ = _resolve(schema["ref"], doc_id)
        return _validate(v, target, tdoc, path)
    if t == "object":
        if not isinstance(v, dict):
            raise _Invalid("G2", f"{path}: not an object")
        for k in schema.get("required", []):
            if k not in v:
                raise _Invalid("G2", f"{path}.{k}: required")
        nullable = set(schema.get("nullable", []))
        for k, sub in schema.get("properties", {}).items():
            if k in v:
                if v[k] is None:
                    if k not in nullable:
                        raise _Invalid("G2", f"{path}.{k}: null")
                    continue
                _validate(v[k], sub, doc_id, f"{path}.{k}")
        return
    if t == "string":
        if not isinstance(v, str):
            raise _Invalid("G2", f"{path}: not a string")
        n = len(v.encode("utf-8"))
        if "maxLength" in schema and n > schema["maxLength"]:
            raise _Invalid("G2", f"{path}: maxLength")
        if "minLength" in schema and n < schema["minLength"]:
            raise _Invalid("G2", f"{path}: minLength")
        if "maxGraphemes" in schema and not _graphemes_ok(v, schema["maxGraphemes"]):
            raise _Invalid("G2", f"{path}: maxGraphemes")
        if "minGraphemes" in schema:
            raise NotModelled("minGraphemes")
        fmt = schema.get("format")
        if fmt is not None:
            if fmt not in _FORMATS:
                raise NotModelled(f"string format {fmt}")
            if not _FORMATS[fmt](v):
                raise _Invalid("G2", f"{path}: format {fmt}")
        if "enum" in schema or "const" in schema:
            raise NotModelled("string enum/const")
        return                            # knownValues are not enforced (G2)
    if t == "integer":
        if not _is_int(v):
            raise _Invalid("G2", f"{path}: not an integer")
        if "minimum" in schema and v < schema["minimum"]:
            raise _Invalid("G2", f"{path}: minimum")
        if "maximum" in schema and v > schema["maximum"]:
            raise _Invalid("G2", f"{path}: maximum")
        return
    if t == "boolean":
        if not isinstance(v, bool):
            raise _Invalid("G2", f"{path}: not a boolean")
        return
    if t == "array":
        if not isinstance(v, list):
            raise _Invalid("G2", f"{path}: not an array")
        if "maxLength" in schema and len(v) > schema["maxLength"]:
            raise _Invalid("G2", f"{path}: maxLength")
        if "minLength" in schema and len(v) < schema["minLength"]:
            raise _Invalid("G2", f"{path}: minLength")
        for i, x in enumerate(v):
            _validate(x, schema["items"], doc_id, f"{path}[{i}]")
        return
    if t == "union":
        if not isinstance(v, dict) or not isinstance(v.get("$type"), str):
            raise _Invalid("G2", f"{path}: union member without $type")
        if schema.get("closed"):
            raise NotModelled("closed unions")
        for ref in schema["refs"]:
            target, tdoc, full = _resolve(ref, doc_id)
            if v["$type"] == full:
                return _validate(v, target, tdoc, path)
        return                            # open union: unknown members are left alone
    if t == "blob":
        if not (isinstance(v, dict) and v.get("$type") == "blob" and isinstance(v.get("ref"), dict)
                and set(v["ref"]) == {"$link"} and isinstance(v["ref"]["$link"], str)
                and isinstance(v.get("mimeType"), str) and _is_int(v.get("size"))):
            raise _Invalid("G3", f"{path}: not a typed blob")
        if "accept" in schema and not _mime_ok(v["mimeType"], schema["accept"]):
            raise _Invalid("G2", f"{path}: blob accept")
        if "maxSize" in schema and v["size"] > schema["maxSize"]:
            raise _Invalid("G2", f"{path}: blob maxSize")
        return
    if t == "unknown":
        if not isinstance(v, dict):
            raise _Invalid("G2", f"{path}: unknown must be an object")
        return
    raise NotModelled(f"lexicon type {t}")


def _g2(coll: str, value: dict) -> None:
    doc = _lexicons()[nsid(coll)]
    _validate(value, doc["defs"]["main"]["record"], nsid(coll), "$")


# --------------------------------------------------------------------------- G6, G10

def _depth(v) -> int:
    if isinstance(v, dict):
        if set(v) in ({"$link"}, {"$bytes"}):
            return 0
        return 1 + max((_depth(x) for x in v.values()), default=0)
    if isinstance(v, list):
        return 1 + max((_depth(x) for x in v), default=0)
    return 0


# --------------------------------------------------------------------------- md values (G4 links, G7)

def _md_values(coll: str, v: dict):
    """(field, md value, spillable) for every md value of a record (data-model.md §5.2)."""
    out = []

    def add(field, mdv):
        if isinstance(mdv, dict):
            out.append((field, mdv, (coll, field) in SPILLABLE))

    if coll in ("post", "comment", "wikiRevision", "sequence"):
        add("body", v.get("body"))
    if coll == "forum":
        add("about", v.get("about"))
    if coll == "sequence":
        for ch in v.get("chapters", []):
            add("chapter.body", ch.get("body"))
    if coll == "modAction":
        a = v.get("action", {})
        if a.get("$type") == f"{NS}.modAction#tier":
            add("notice", a.get("notice"))
    return [(f, m, s) for (f, m, s) in out if m.get("$type") == f"{NS}.markdown"]


# --------------------------------------------------------------------------- the check

def _modaction_known(v: dict):
    """(scope_eff, action short name or None) and whether MA7 makes the record effect-free."""
    scope = v.get("scope", "forum")
    a = v["action"]
    t = a["$type"]
    base = f"{NS}.modAction#"
    name = t[len(base):] if t.startswith(base) else None
    if name not in ("hide", "access", "restrict", "tier", "pin", "lock", "tag"):
        name = None
    unknown = scope not in ("forum", "author") or name is None
    if name == "access" and a.get("level") not in ("banned", "member"):
        unknown = True
    if name == "tier" and a.get("tier") not in ("personal", "frontpage", "curated"):
        unknown = True
    if name == "pin" and a.get("scope") not in ("forum", "thread"):
        unknown = True
    return scope, name, unknown


def is_positive(action: dict) -> bool:
    """Positive (CID-bound) record per social.md §4.2 / data-model.md §6.7 (known actions)."""
    t = action.get("$type", "")
    name = t.split("#", 1)[1] if t.startswith(f"{NS}.modAction#") else None
    if name == "hide":
        return action.get("hidden", True) is False
    if name == "tier":
        return action.get("tier") in ("frontpage", "curated")
    if name == "pin":
        return action.get("pinned", True) is True
    return False


def subject_form(s):
    """'did' | 'uri' | 'tag' | None (MA1)."""
    if not isinstance(s, str):
        return None
    if is_canonical_did(s):
        return "did"
    if s.startswith("tag:") and is_slug(s[4:]):
        return "tag"
    if canon(s):
        return "uri"
    return None


def _check(uri: str, value: dict) -> Validity:
    parts = split_uri(uri)
    repo_did, coll_nsid, rkey = parts
    coll = coll_name(coll_nsid)
    if coll is None:
        raise NotModelled(f"not an NS record collection: {uri}")
    v = value

    # G1
    if v.get("$type") != nsid(coll):
        raise _Invalid("G1", "$type")
    # G2, G3
    _g2(coll, v)

    # G4: reference fields
    def ref(u, allowed, what):
        if not is_canonical_at_uri(u, allowed):
            raise _Invalid("G4", what)

    if coll not in ("forum", "profile", "revision"):      # a revision has no forum field (NOTES.md 15)
        ref(v["forum"], {"forum"}, "forum")
    if coll == "comment":
        ref(v["root"]["uri"], {"post"}, "root")
        if "parent" in v:
            ref(v["parent"]["uri"], {"comment"}, "parent")
    if coll == "revision":
        ref(v["subject"]["uri"], {"post", "comment", "forum", "sequence", "modAction"}, "subject")
    if coll == "opinion":
        ref(v["subject"]["uri"], {"post", "comment", "wikiRevision", "sequence"}, "subject")
    if coll == "modAction":
        if "adopts" in v:
            ref(v["adopts"]["uri"], {"modAction"}, "adopts")
        a = v["action"]
        if a["$type"] == f"{NS}.modAction#lock":
            for u in a.get("allowed", []):
                ref(u, {"comment"}, "allowed")
        if a["$type"] == f"{NS}.modAction#tag" and "head" in a:
            ref(a["head"]["uri"], {"wikiRevision"}, "head")
    if coll == "wikiRevision":
        for p in v.get("parents", []):
            ref(p["uri"], {"wikiRevision"}, "parents")
    if coll == "sequence":
        for ch in v["chapters"]:
            for u in ch["posts"]:
                ref(u, {"post"}, "chapter.posts")
    if coll == "profile":
        for u in v.get("pinned", []):
            ref(u, {"post", "comment", "sequence"}, "pinned")
    mds = _md_values(coll, v)
    for field, mdv, _ in mds:
        for u in mdv.get("links", []):
            ref(u, None, f"{field}.links")

    # G5 (with G9): canonical rkey, data-model.md §3.2
    try:
        F = v.get("forum")
        if coll == "forum":
            ok = is_slug(rkey)
        elif coll == "profile":
            ok = rkey == "self"
        elif coll == "membership":
            ok = rkey == fk(F)
        elif coll in ("post", "wikiRevision", "sequence"):
            ok = re.fullmatch(rf"{FK_RE}-{TID_RE}", rkey) is not None and rkey[:12] == fk(F)
        elif coll == "comment":
            root = v["root"]["uri"]
            rparts = split_uri(root)
            ok = (re.fullmatch(rf"{FK_RE}-{TID_RE}-{TID_RE}", rkey) is not None and rkey[:12] == fk(F)
                  and re.fullmatch(rf"{FK_RE}-{TID_RE}", rparts[2]) is not None
                  and rparts[2][:12] == rkey[:12] and rkey.split("-")[1] == thread_tid(root))
        elif coll == "revision":
            sub = v["subject"]["uri"]
            m = re.fullmatch(rf"(.*)-({TID_RE})", rkey)
            ok = m is not None and m.group(1) == P(sub) and split_uri(sub)[0] == repo_did
        elif coll == "opinion":
            sub = v["subject"]["uri"]
            ok = (rkey == f"{fk(F)}-{stid(sub)}-{h16('opinion', [sub])}"
                  and split_uri(sub)[2][:12] == fk(F))
        elif coll == "modAction":
            scope_eff = v.get("scope", "forum")
            reserved = re.fullmatch(rf"{FK_RE}-{TID_RE}", rkey) is not None
            if reserved:
                ok = rkey[:12] == fk(F)
            else:
                ok = rkey == f"{fk(F)}-{h16('modAction', [scope_eff, v['subject'], action_key(v['action'])])}"
        else:
            raise NotModelled(coll)
    except KeyDerivationError as e:
        raise _Invalid("G5", f"key derivation: {e.code}")
    if not ok:
        raise _Invalid("G5", "rkey is not canonical")

    # G6
    if len(dagcbor.encode(v)) > RECORD_CBOR_MAX_BYTES:
        raise _Invalid("G6", "size")
    # G7
    for field, mdv, spillable in mds:
        if ("text" in mdv) == ("textBlob" in mdv):
            raise _Invalid("G7", f"{field}: exactly one of text, textBlob")
        if "textBlob" in mdv and not spillable:
            raise _Invalid("G7", f"{field}: inline only")
    # G8
    slugs = []
    if coll == "forum":
        slugs += v.get("featuredTags", [])
    if coll == "post":
        slugs += v.get("tags", [])
    if coll == "opinion":
        slugs += [r["react"] for r in v.get("reacts", [])] + [t["tag"] for t in v.get("tags", [])]
    if coll == "wikiRevision":
        slugs += [v["slug"]] + v.get("parentTags", [])
    for s in slugs:
        if not is_slug(s):
            raise _Invalid("G8", repr(s))
    # G10
    if _depth(v) > NESTING_MAX_RECORD:
        raise _Invalid("G10", "depth")

    # per-collection rules
    if coll == "post":
        kind = v.get("kind", "article")
        if kind not in ("article", "link", "question", "quickTake"):
            kind = "article"                                         # PO2
        if kind != "quickTake" and not (isinstance(v.get("title"), str) and has_non_white_space(v["title"])):
            raise _Invalid("PO3", "title")
        if kind == "quickTake" and "body" not in v:
            raise _Invalid("PO4", "body")
        if kind == "link" and "link" not in v:
            raise _Invalid("PO5", "link")
        b = v.get("body")
        if isinstance(b, dict) and b.get("$type") == f"{NS}.markdown" and "textBlob" in b and "excerpt" not in v:
            raise _Invalid("PO6", "excerpt")
    if coll == "revision":
        sub = v["subject"]
        scoll = coll_name(split_uri(sub["uri"])[1])
        if split_uri(sub["uri"])[0] != repo_did:
            raise _Invalid("RV1", "subject repo")
        if v["op"] not in ("update", "delete") or (v["op"] == "delete" and scoll != "modAction"):
            raise _Invalid("RV4", "op")
        if "record" in v:
            rec = v["record"]
            if not isinstance(rec, dict) or rec.get("$type") != nsid(scoll):
                raise _Invalid("RV5", "record $type")
            if dagcbor.cid_of(rec) != sub["cid"]:
                raise _Invalid("RV5", "record CID")
    if coll == "wikiRevision":
        for p in v.get("parents", []):
            if split_uri(p["uri"])[2][:12] != fk(v["forum"]):
                raise _Invalid("WR2", "parent fk")
    if coll == "modAction":
        return _check_modaction(v, rkey)
    return Validity.ok()


def _check_modaction(v: dict, rkey: str) -> Validity:
    subj = v["subject"]
    form = subject_form(subj)
    if form is None:
        raise _Invalid("MA1", "subject form")
    reserved_key = re.fullmatch(rf"{FK_RE}-{TID_RE}", rkey) is not None and \
        re.fullmatch(rf"{FK_RE}-{H16_RE}", rkey) is None
    scope, name, unknown = _modaction_known(v)
    a = v["action"]
    F = v["forum"]
    no_effect = "MA2" if reserved_key else None
    if unknown:
        no_effect = no_effect or "MA7"
    else:
        # MA3: an at-uri subject in a forum-scoped collection carries the forum's fk.
        # A forum-uri subject (#lock, reserved) has a slug key and is exempt (NOTES.md 12).
        if form == "uri":
            scoll = coll_name(split_uri(subj)[1])
            if scoll != "forum" and split_uri(subj)[2][:12] != fk(F):
                raise _Invalid("MA3", "subject fk")
        # MA4: subject form and collection per action
        scoll = coll_name(split_uri(subj)[1]) if form == "uri" else None
        allowed = {
            "hide": ("uri", {"post", "comment", "sequence", "wikiRevision"}),
            "access": ("did", None), "restrict": ("did", None),
            "tier": ("uri", {"post"}),
            "pin": ("uri", {"post", "sequence"} if a.get("scope") == "forum" else {"comment"}),
            "lock": ("uri", {"post", "forum"}),
            "tag": ("tag", None),
        }[name]
        if form != allowed[0] or (allowed[1] is not None and scoll not in allowed[1]):
            raise _Invalid("MA4", "subject not allowed for the action")
        # MA5
        if is_positive(a) and "subjectCid" not in v:
            raise _Invalid("MA5", "positive assertion without subjectCid")
        # MA6
        if scope == "author":
            ok = ((name == "hide" and a.get("hidden", True) is True and scoll == "comment")
                  or (name == "access" and a.get("level") == "banned")
                  or (name == "pin" and a.get("scope") == "thread")
                  or name == "lock")
            if not ok:
                raise _Invalid("MA6", "not allowed in author scope")
        # MA9
        if name == "lock" and scoll == "forum":
            no_effect = no_effect or "MA9"
    # MA10 (still checked when MA7 applies: MA7 skips only MA3–MA6)
    if name == "tag" and "head" in a:
        if split_uri(a["head"]["uri"])[2][:12] != fk(F):
            raise _Invalid("MA10", "head fk")
    # MA11, MA12: G4 and G7 above
    return Validity.ok(no_effect)


def check(uri: str, value: dict) -> Validity:
    try:
        return _check(uri, value)
    except _Invalid as e:
        return Validity(False, e.rule, None, e.detail)
    except (KeyError, TypeError, AttributeError, IndexError) as e:
        # A structural surprise after G2 means the model misses a rule: never guess.
        raise NotModelled(f"validity of {uri}: {e!r}")
