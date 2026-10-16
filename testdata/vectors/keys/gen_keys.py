#!/usr/bin/env python3
# SPDX-License-Identifier: CC0-1.0
"""Generate the key-derivation vectors for fk/1 and sk/1.

Implements exactly docs/spec/data-model.md §3.1-§3.2:

    b32s(bytes)    base32, MSB-first 5-bit groups, alphabet "234567abcdefghijklmnopqrstuvwxyz", no padding
    H(tag, p1..pk) sha256(UTF8(tag) ‖ (0x1F ‖ UTF8(NFC(p_i)))*), rejecting any p_i that contains U+001F
    fk(forumUri)   b32s(H("lessdugin-fk/1", forumUri))[0..12]
    h16(name, p…)  b32s(H("lessdugin-sk/1", name, p…))[0..16]
    threadTid, stid, P, actionKey and the per-collection rkeys

Usage (from anywhere; writes next to this script):

    python3 -I testdata/vectors/keys/gen_keys.py          # (re)write fk.json, sk.json, rkeys.json
    python3 -I testdata/vectors/keys/gen_keys.py --check  # exit 1 if the files on disk differ

Only the Python standard library is used. The NFC step uses Python's `unicodedata`; every
non-ASCII input below is stable across Unicode versions >= 6.0, so the vectors do not depend
on the Unicode version of the generator or of a runner.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
import unicodedata
from pathlib import Path

NS = "online.7778777.lessdugin"
NS_TEMP = "online.7778777.lessdugin.temp"

ALPHABET = "234567abcdefghijklmnopqrstuvwxyz"
TAG_FK = "lessdugin-fk/1"
TAG_SK = "lessdugin-sk/1"

FK_RE = r"[2-7a-z]{12}"
TID_RE = r"[2-7a-j][2-7a-z]{12}"


class KeyError_(Exception):
    """A derivation that yields no key. `code` is the vector error code."""

    def __init__(self, code: str):
        super().__init__(code)
        self.code = code


# --------------------------------------------------------------------------- primitives


def b32s(data: bytes, n: int) -> str:
    """First n characters of b32s(data). Requires 5n <= 8 * len(data) (no partial group)."""
    if 5 * n > 8 * len(data):
        raise ValueError("b32s: n exceeds the complete 5-bit groups of the input")
    value = int.from_bytes(data, "big")
    total = 8 * len(data)
    out = []
    for i in range(n):
        shift = total - 5 * (i + 1)
        out.append(ALPHABET[(value >> shift) & 0x1F])
    return "".join(out)


def preimage(tag: str, parts: list[str]) -> bytes:
    buf = bytearray(tag.encode("utf-8"))
    for p in parts:
        if "\u001f" in p:
            raise KeyError_("UnitSeparator")
        buf += b"\x1f"
        buf += unicodedata.normalize("NFC", p).encode("utf-8")
    return bytes(buf)


def H(tag: str, parts: list[str]) -> bytes:
    return hashlib.sha256(preimage(tag, parts)).digest()


def fk(forum_uri: str) -> str:
    return b32s(H(TAG_FK, [forum_uri]), 12)


def h16(name: str, parts: list[str]) -> str:
    return b32s(H(TAG_SK, [name, *parts]), 16)


# --------------------------------------------------------------------------- uri helpers


def split_uri(uri: str) -> tuple[str, str, str]:
    """(did, collection, rkey) of an at-uri of the form at://DID/NSID/RKEY."""
    if not uri.startswith("at://"):
        raise KeyError_("Undefined")
    rest = uri[len("at://"):].split("/")
    if len(rest) != 3:
        raise KeyError_("Undefined")
    return rest[0], rest[1], rest[2]


def thread_tid(uri: str, ns: str) -> str:
    _, coll, rkey = split_uri(uri)
    if coll == ns + ".post":
        m = re.fullmatch(f"({FK_RE})-({TID_RE})", rkey)
        if m:
            return m.group(2)
    if coll == ns + ".comment":
        m = re.fullmatch(f"({FK_RE})-({TID_RE})-({TID_RE})", rkey)
        if m:
            return m.group(2)
    raise KeyError_("Undefined")


def stid(uri: str, ns: str) -> str:
    _, coll, _ = split_uri(uri)
    if coll in (ns + ".post", ns + ".comment"):
        return thread_tid(uri, ns)
    if coll in (ns + ".wikiRevision", ns + ".sequence"):
        return "~"
    raise KeyError_("Undefined")


def P(uri: str, ns: str) -> str:
    _, coll, rkey = split_uri(uri)
    if coll == ns + ".forum":
        return fk(uri)
    if coll in (ns + ".post", ns + ".comment", ns + ".sequence", ns + ".modAction"):
        return rkey
    raise KeyError_("Undefined")


def action_key(action: dict, ns: str) -> str:
    t = action["$type"]
    base = ns + ".modAction#"
    simple = {"hide": "hide", "access": "access", "restrict": "restrict", "tier": "tier",
              "lock": "lock", "tag": "tag"}
    if t.startswith(base):
        name = t[len(base):]
        if name in simple:
            return simple[name]
        if name == "pin":
            return "pin:" + action["scope"]
    return t


def check_tid(tid: str) -> str:
    if not re.fullmatch(TID_RE, tid):
        raise KeyError_("InvalidTid")
    return tid


def rkey(inp: dict) -> dict:
    """Derive the canonical rkey (and intermediates) for an `fn: rkey` vector input."""
    ns = inp["ns"]
    coll = inp["collection"]
    out: dict = {}
    if coll == "membership":
        out["fk"] = fk(inp["forum"])
        out["rkey"] = out["fk"]
    elif coll in ("post", "wikiRevision", "sequence"):
        out["fk"] = fk(inp["forum"])
        out["rkey"] = out["fk"] + "-" + check_tid(inp["tid"])
    elif coll == "comment":
        _, rcoll, rrkey = split_uri(inp["root"])
        if rcoll != ns + ".post" or not re.fullmatch(f"{FK_RE}-{TID_RE}", rrkey):
            raise KeyError_("Undefined")
        out["fk"] = fk(inp["forum"])
        out["threadTid"] = thread_tid(inp["root"], ns)
        out["rkey"] = out["fk"] + "-" + out["threadTid"] + "-" + check_tid(inp["tid"])
    elif coll == "revision":
        out["P"] = P(inp["subject"], ns)
        out["rkey"] = out["P"] + "-" + check_tid(inp["tid"])
    elif coll == "opinion":
        out["fk"] = fk(inp["forum"])
        out["stid"] = stid(inp["subject"], ns)
        out["h16"] = h16("opinion", [inp["subject"]])
        out["rkey"] = out["fk"] + "-" + out["stid"] + "-" + out["h16"]
    elif coll == "modAction":
        scope_eff = inp.get("scope", "forum")
        out["fk"] = fk(inp["forum"])
        out["scopeEff"] = scope_eff
        out["actionKey"] = action_key(inp["action"], ns)
        out["h16"] = h16("modAction", [scope_eff, inp["subject"], out["actionKey"]])
        out["rkey"] = out["fk"] + "-" + out["h16"]
    elif coll == "forum":
        raise KeyError_("Undefined")  # a forum key is its slug; there is nothing to derive
    elif coll == "profile":
        out["rkey"] = "self"
    else:
        raise KeyError_("Undefined")
    return out


# --------------------------------------------------------------------------- inputs

PLC_A = "did:plc:ewvi7nxzyoun6zhxrhs64oiz"
PLC_B = "did:plc:abcdefghijklmnopqrstuvwx"
PLC_C = "did:plc:7iza6de2dwap2sbkpav7c6c6"
WEB_A = "did:web:forum.example.com"
WEB_B = "did:web:xn--bcher-kva.example"  # IDN hostnames are stored in their ASCII (A-label) form

T1 = "3l2s5xxv2ze2c"
T2 = "3l2s6aaaaaaa2"
T3 = "3l2s7bbbbbbb2"
T4 = "3lz7rb6tpt22a"
T5 = "3m2abcdefgh22"


def at(did: str, coll: str, rk: str, ns: str = NS) -> str:
    return f"at://{did}/{ns}.{coll}/{rk}"


FORUM = at(PLC_A, "forum", "rationality")
FORUM_TEMP = at(PLC_A, "forum", "rationality", NS_TEMP)
FORUM_WEB = at(WEB_A, "forum", "general")
FK = fk(FORUM)
FK_TEMP = fk(FORUM_TEMP)
FK_WEB = fk(FORUM_WEB)
POST = at(PLC_A, "post", f"{FK}-{T1}")
POST_TEMP = at(PLC_A, "post", f"{FK_TEMP}-{T1}", NS_TEMP)
POST_WEB = at(PLC_C, "post", f"{FK_WEB}-{T4}")
COMMENT = at(PLC_B, "comment", f"{FK}-{T1}-{T2}")
COMMENT_WEB = at(WEB_B, "comment", f"{FK_WEB}-{T4}-{T5}")
WIKI = at(PLC_B, "wikiRevision", f"{FK}-{T3}")
SEQ = at(WEB_A, "sequence", f"{FK}-{T3}")
MODACTION = at(PLC_C, "modAction", f"{FK}-{h16('modAction', ['forum', POST, 'hide'])}")


# --------------------------------------------------------------------------- vector builders


def run(fn) -> dict:
    try:
        return fn()
    except KeyError_ as e:
        return {"error": e.code}


def fk_vectors() -> list[dict]:
    cases = [
        ("plc-owner-worked-example", FORUM, None, "data-model.md §3.4"),
        ("plc-owner-temp-namespace", FORUM_TEMP, None, "the .temp prefix changes every fk"),
        ("plc-owner-other-slug", at(PLC_A, "forum", "meta"), None, None),
        ("plc-owner-other-did", at(PLC_B, "forum", "rationality"), None, "same slug, different owner"),
        ("web-owner", FORUM_WEB, None, None),
        ("web-owner-subdomain-idn-a-label", at(WEB_B, "forum", "books"), None, None),
        ("slug-one-char", at(PLC_C, "forum", "a"), None, "shortest slug"),
        ("slug-64-bytes", at(PLC_C, "forum", "a" * 30 + "-" + "0123456789" * 3 + "-b2"), None,
         "longest slug (64 bytes)"),
        ("slug-digits-and-hyphens", at(WEB_A, "forum", "2024-ai-safety-camp"), None, None),
        ("nfc-composed", "at://did:web:example.com/online.7778777.lessdugin.forum/café", "raw",
         "not canonical (non-slug rkey); exercises NFC in H"),
        ("nfc-decomposed-equals-composed", "at://did:web:example.com/online.7778777.lessdugin.forum/café", "raw",
         "NFC(e + U+0301) = U+00E9, so the fk equals nfc-composed"),
        ("nfc-singleton-angstrom", "at://did:web:example.com/online.7778777.lessdugin.forum/Å", "raw",
         "U+212B ANGSTROM SIGN normalises to U+00C5 under NFC"),
        ("nfc-singleton-angstrom-equals", "at://did:web:example.com/online.7778777.lessdugin.forum/Å", "raw",
         "equals nfc-singleton-angstrom"),
        ("nfc-hangul-jamo", "at://did:web:example.com/online.7778777.lessdugin.forum/가", "raw",
         "conjoining jamo compose to U+AC00 under NFC"),
        ("nfc-hangul-precomposed", "at://did:web:example.com/online.7778777.lessdugin.forum/가", "raw",
         "equals nfc-hangul-jamo"),
        ("nfc-not-nfkc-ligature", "at://did:web:example.com/online.7778777.lessdugin.forum/ﬁle", "raw",
         "U+FB01 is unchanged by NFC (NFKC would give 'fi'); must differ from nfc-not-nfkc-plain"),
        ("nfc-not-nfkc-plain", "at://did:web:example.com/online.7778777.lessdugin.forum/file", "raw",
         "compare with nfc-not-nfkc-ligature"),
        ("astral-emoji", "at://did:web:example.com/online.7778777.lessdugin.forum/\U0001f980", "raw",
         "4-byte UTF-8"),
        ("invalid-unit-separator", "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.forum/a\u001fb",
         "raw", "a part containing U+001F is rejected: no key"),
        ("unit-separator-only", "\u001f", "raw", "rejected even when it is the whole part"),
        ("record-separator-accepted", "at://did:web:example.com/online.7778777.lessdugin.forum/a\u001eb", "raw",
         "only U+001F is rejected; U+001E is hashed like any other character"),
        ("empty-string", "", "raw", "the empty part hashes to tag ‖ 0x1F"),
    ]
    out = []
    for name, uri, raw, note in cases:
        def calc(uri=uri):
            d = H(TAG_FK, [uri])
            return {"fk": b32s(d, 12), "digest": d.hex()}
        v = {"name": name, "input": {"forumUri": uri}, "expected": run(calc)}
        if raw:
            v["raw"] = True
        if note:
            v["note"] = note
        out.append(v)
    return out


def sk_vectors() -> list[dict]:
    out: list[dict] = []

    def add(name, inp, calc, raw=False, note=None):
        v = {"name": name, "input": inp, "expected": run(calc)}
        if raw:
            v["raw"] = True
        if note:
            v["note"] = note
        out.append(v)

    # b32s
    for name, hexs, n, note in [
        ("b32s-empty", "", 0, None),
        ("b32s-zero-5-bytes", "0000000000", 8, "all-zero groups map to '2'"),
        ("b32s-ones-5-bytes", "ffffffffff", 8, "all-one groups map to 'z'"),
        ("b32s-ascending", "00443214c74254b635cf84653a56d7c675be77df", 32,
         "every 5-bit value 0..31 once, in order: the whole alphabet"),
        ("b32s-msb-first", "8000000000", 8, "the top bit lands in the first character"),
        ("b32s-sha256-prefix-51", hashlib.sha256(b"").hexdigest(), 51,
         "51 complete groups of a 32-byte digest; the partial 52nd group is never used"),
    ]:
        add(name, {"fn": "b32s", "hex": hexs, "n": n},
            lambda hexs=hexs, n=n: {"b32s": b32s(bytes.fromhex(hexs), n)}, note=note)

    # H
    for name, tag, parts, raw, note in [
        ("H-no-parts", TAG_SK, [], False, "the tag alone"),
        ("H-one-empty-part", TAG_SK, [""], False, "tag ‖ 0x1F"),
        ("H-two-parts", TAG_SK, ["a", "b"], False, None),
        ("H-joined-part-differs", TAG_SK, ["a\u001eb"], False,
         "differs from H-two-parts: parts are separated by 0x1F, not concatenated"),
        ("H-concatenation-differs", TAG_SK, ["ab"], False, "differs from H-two-parts"),
        ("H-nfc-composed", TAG_SK, ["café"], True, None),
        ("H-nfc-decomposed", TAG_SK, ["café"], True, "equals H-nfc-composed"),
        ("H-cjk", TAG_SK, ["日本語"], True, "3-byte UTF-8"),
        ("H-unit-separator-rejected", TAG_SK, ["opinion", "x\u001fy"], True,
         "any part containing U+001F: no digest, the input is invalid"),
    ]:
        def calc(tag=tag, parts=parts):
            pre = preimage(tag, parts)
            return {"preimage": pre.hex(), "digest": hashlib.sha256(pre).hexdigest()}
        add(name, {"fn": "H", "tag": tag, "parts": parts}, calc, raw, note)

    # h16, opinion form
    for name, parts, raw, note in [
        ("h16-opinion-post", [POST], False, "data-model.md §3.4"),
        ("h16-opinion-comment", [COMMENT], False, "data-model.md §3.4"),
        ("h16-opinion-wikiRevision", [WIKI], False, None),
        ("h16-opinion-sequence-web-author", [SEQ], False, None),
        ("h16-opinion-post-temp", [POST_TEMP], False, "the .temp prefix changes every h16"),
        ("h16-opinion-comment-web-author", [COMMENT_WEB], False, None),
        ("h16-opinion-nfc-composed", ["at://did:web:example.com/x.y.z/é"], True, None),
        ("h16-opinion-nfc-decomposed", ["at://did:web:example.com/x.y.z/é"], True,
         "equals h16-opinion-nfc-composed"),
        ("h16-opinion-unit-separator", [POST + "\u001f"], True, "rejected"),
    ]:
        def calc(parts=parts):
            d = H(TAG_SK, ["opinion", *parts])
            return {"h16": b32s(d, 16), "digest": d.hex()}
        add(name, {"fn": "h16", "name": "opinion", "parts": parts}, calc, raw, note)

    # h16, modAction form: parts = [scope_eff, subject, actionKey]
    for name, parts, raw, note in [
        ("h16-modAction-access-did", ["forum", PLC_B, "access"], False, "data-model.md §3.4"),
        ("h16-modAction-hide-post", ["forum", POST, "hide"], False, "data-model.md §3.4"),
        ("h16-modAction-pin-forum", ["forum", POST, "pin:forum"], False, "data-model.md §3.4"),
        ("h16-modAction-tag", ["forum", "tag:decision-theory", "tag"], False, "data-model.md §3.4"),
        ("h16-modAction-author-scope-hide", ["author", COMMENT, "hide"], False, None),
        ("h16-modAction-ban-web-did", ["forum", WEB_A, "access"], False, None),
        ("h16-modAction-restrict", ["forum", PLC_C, "restrict"], False, None),
        ("h16-modAction-unknown-action", ["forum", POST, NS + ".modAction#feature"], False,
         "an unknown action's key is its $type"),
        ("h16-modAction-scope-is-hashed", ["author", POST, "hide"], False,
         "differs from h16-modAction-hide-post only in scope"),
        ("h16-modAction-tag-nfc-composed", ["forum", "tag:résumé", "tag"], True,
         "not a valid slug; exercises NFC"),
        ("h16-modAction-tag-nfc-decomposed", ["forum", "tag:résumé", "tag"], True,
         "equals h16-modAction-tag-nfc-composed"),
        ("h16-modAction-unit-separator", ["forum", "tag:a\u001fb", "tag"], True, "rejected"),
    ]:
        def calc(parts=parts):
            d = H(TAG_SK, ["modAction", *parts])
            return {"h16": b32s(d, 16), "digest": d.hex()}
        add(name, {"fn": "h16", "name": "modAction", "parts": parts}, calc, raw, note)
    return out


def rkey_vectors() -> list[dict]:
    out: list[dict] = []

    def add(name, inp, calc, raw=False, note=None):
        v = {"name": name, "input": inp, "expected": run(calc)}
        if raw:
            v["raw"] = True
        if note:
            v["note"] = note
        out.append(v)

    def rk(name, raw=False, note=None, ns=NS, **fields):
        inp = {"fn": "rkey", "ns": ns, **fields}
        add(name, inp, lambda inp=inp: rkey(inp), raw, note)

    # threadTid / stid / P / actionKey
    for name, uri, ns, note in [
        ("threadTid-post", POST, NS, None),
        ("threadTid-comment", COMMENT, NS, "the FIRST TID segment"),
        ("threadTid-comment-web", COMMENT_WEB, NS, None),
        ("threadTid-temp-post-under-temp", POST_TEMP, NS_TEMP, None),
        ("threadTid-temp-post-under-release", POST_TEMP, NS, "a .temp collection is not NS.post in a release build"),
        ("threadTid-wikiRevision-undefined", WIKI, NS, None),
        ("threadTid-post-bad-grammar", at(PLC_A, "post", "3l2s5xxv2ze2c"), NS, "no fk segment"),
        ("threadTid-post-uppercase-tid", at(PLC_A, "post", f"{FK}-3L2S5XXV2ZE2C"), NS, "TID syntax is lowercase"),
        ("threadTid-post-tid-top-bit", at(PLC_A, "post", f"{FK}-zl2s5xxv2ze2c"), NS,
         "first TID character outside [2-7a-j]"),
        ("threadTid-comment-with-post-grammar", at(PLC_A, "comment", f"{FK}-{T1}"), NS, None),
    ]:
        add(name, {"fn": "threadTid", "ns": ns, "uri": uri},
            lambda uri=uri, ns=ns: {"threadTid": thread_tid(uri, ns)}, note=note)
    for name, uri in [("stid-post", POST), ("stid-comment", COMMENT), ("stid-wikiRevision", WIKI),
                      ("stid-sequence", SEQ), ("stid-forum-undefined", FORUM),
                      ("stid-modAction-undefined", MODACTION)]:
        add(name, {"fn": "stid", "ns": NS, "uri": uri}, lambda uri=uri: {"stid": stid(uri, NS)})
    for name, uri in [("P-forum", FORUM), ("P-forum-web", FORUM_WEB), ("P-post", POST), ("P-comment", COMMENT),
                      ("P-sequence", SEQ), ("P-modAction", MODACTION), ("P-wikiRevision-undefined", WIKI)]:
        add(name, {"fn": "P", "ns": NS, "uri": uri}, lambda uri=uri: {"P": P(uri, NS)})
    for name, action, ns in [
        ("actionKey-hide", {"$type": NS + ".modAction#hide", "hidden": True}, NS),
        ("actionKey-access", {"$type": NS + ".modAction#access", "level": "banned"}, NS),
        ("actionKey-restrict", {"$type": NS + ".modAction#restrict", "capabilities": ["vote"]}, NS),
        ("actionKey-tier", {"$type": NS + ".modAction#tier", "tier": "curated"}, NS),
        ("actionKey-pin-forum", {"$type": NS + ".modAction#pin", "scope": "forum"}, NS),
        ("actionKey-pin-thread", {"$type": NS + ".modAction#pin", "scope": "thread"}, NS),
        ("actionKey-pin-unknown-scope", {"$type": NS + ".modAction#pin", "scope": "global"}, NS),
        ("actionKey-lock", {"$type": NS + ".modAction#lock", "locked": True}, NS),
        ("actionKey-tag", {"$type": NS + ".modAction#tag", "removed": True}, NS),
        ("actionKey-unknown", {"$type": NS + ".modAction#feature"}, NS),
        ("actionKey-foreign-type", {"$type": "com.example.mod#hide"}, NS),
        ("actionKey-temp-hide", {"$type": NS_TEMP + ".modAction#hide"}, NS_TEMP),
        ("actionKey-temp-type-in-release", {"$type": NS_TEMP + ".modAction#hide"}, NS),
    ]:
        add(name, {"fn": "actionKey", "ns": ns, "action": action},
            lambda action=action, ns=ns: {"actionKey": action_key(action, ns)})

    # per-collection rkeys
    rk("membership", collection="membership", forum=FORUM)
    rk("membership-web-owner", collection="membership", forum=FORUM_WEB)
    rk("post-worked-example", collection="post", forum=FORUM, tid=T1, note="data-model.md §3.4")
    rk("post-temp", collection="post", forum=FORUM_TEMP, tid=T1, ns=NS_TEMP)
    rk("post-web-owner", collection="post", forum=FORUM_WEB, tid=T4)
    rk("post-invalid-tid", collection="post", forum=FORUM, tid="3l2s5xxv2ze2", note="12 characters")
    rk("comment-worked-example", collection="comment", forum=FORUM, root=POST, tid=T2,
       note="data-model.md §3.4")
    rk("comment-web", collection="comment", forum=FORUM_WEB, root=POST_WEB, tid=T5)
    rk("comment-root-is-comment", collection="comment", forum=FORUM, root=COMMENT, tid=T3,
       note="root must be an NS.post uri with ^FK-TID$")
    rk("revision-of-post", collection="revision", subject=POST, tid=T3)
    rk("revision-of-comment", collection="revision", subject=COMMENT, tid=T3, note="P is the comment rkey: 54 chars")
    rk("revision-of-forum", collection="revision", subject=FORUM, tid=T3, note="P = fk(forum uri)")
    rk("revision-of-sequence", collection="revision", subject=SEQ, tid=T4)
    rk("revision-of-modAction", collection="revision", subject=MODACTION, tid=T4)
    rk("revision-of-wikiRevision-undefined", collection="revision", subject=WIKI, tid=T4)
    rk("opinion-on-post", collection="opinion", forum=FORUM, subject=POST, note="data-model.md §3.4")
    rk("opinion-on-comment", collection="opinion", forum=FORUM, subject=COMMENT, note="data-model.md §3.4")
    rk("opinion-on-wikiRevision", collection="opinion", forum=FORUM, subject=WIKI, note="stid '~': 31 chars")
    rk("opinion-on-sequence", collection="opinion", forum=FORUM, subject=SEQ)
    rk("opinion-on-comment-web", collection="opinion", forum=FORUM_WEB, subject=COMMENT_WEB)
    rk("opinion-temp", collection="opinion", forum=FORUM_TEMP, subject=POST_TEMP, ns=NS_TEMP)
    rk("opinion-on-forum-undefined", collection="opinion", forum=FORUM, subject=FORUM)
    rk("modAction-access-worked-example", collection="modAction", forum=FORUM, subject=PLC_B,
       action={"$type": NS + ".modAction#access", "level": "banned"}, note="data-model.md §3.4; scope absent = forum")
    rk("modAction-access-explicit-forum-scope", collection="modAction", forum=FORUM, scope="forum", subject=PLC_B,
       action={"$type": NS + ".modAction#access", "level": "member"},
       note="same key as the worked example: level is not part of the key")
    rk("modAction-hide-worked-example", collection="modAction", forum=FORUM, subject=POST,
       action={"$type": NS + ".modAction#hide", "hidden": True, "spam": True}, note="data-model.md §3.4")
    rk("modAction-keep-same-key-as-hide", collection="modAction", forum=FORUM, subject=POST,
       action={"$type": NS + ".modAction#hide", "hidden": False}, note="keep and hide share a key")
    rk("modAction-pin-forum-worked-example", collection="modAction", forum=FORUM, subject=POST,
       action={"$type": NS + ".modAction#pin", "scope": "forum"}, note="data-model.md §3.4")
    rk("modAction-pin-thread-author-scope", collection="modAction", forum=FORUM, scope="author", subject=COMMENT,
       action={"$type": NS + ".modAction#pin", "scope": "thread"})
    rk("modAction-tag-worked-example", collection="modAction", forum=FORUM, subject="tag:decision-theory",
       action={"$type": NS + ".modAction#tag", "removed": False},
       note="data-model.md §3.4")
    rk("modAction-ban-web-did", collection="modAction", forum=FORUM_WEB, subject=WEB_B,
       action={"$type": NS + ".modAction#access", "level": "banned"})
    rk("modAction-unknown-scope", collection="modAction", forum=FORUM, scope="global", subject=POST,
       action={"$type": NS + ".modAction#hide"}, note="an unknown scope is hashed as written (MA7: no effect)")
    rk("modAction-unknown-action", collection="modAction", forum=FORUM, subject=POST,
       action={"$type": NS + ".modAction#feature"})
    rk("modAction-temp", collection="modAction", forum=FORUM_TEMP, subject=POST_TEMP, ns=NS_TEMP,
       action={"$type": NS_TEMP + ".modAction#hide"})
    rk("modAction-tag-nfc-composed", collection="modAction", forum=FORUM, subject="tag:café", raw=True,
       action={"$type": NS + ".modAction#tag"}, note="not a valid slug; exercises NFC")
    rk("modAction-tag-nfc-decomposed", collection="modAction", forum=FORUM, subject="tag:café", raw=True,
       action={"$type": NS + ".modAction#tag"}, note="equals modAction-tag-nfc-composed")
    rk("modAction-unit-separator", collection="modAction", forum=FORUM, subject="tag:a\u001fb", raw=True,
       action={"$type": NS + ".modAction#tag"}, note="rejected: no key")
    rk("wikiRevision", collection="wikiRevision", forum=FORUM, tid=T3)
    rk("sequence-web-owner", collection="sequence", forum=FORUM_WEB, tid=T5)
    rk("profile", collection="profile")
    rk("membership-nfc-forum", collection="membership", raw=True,
       forum="at://did:web:example.com/online.7778777.lessdugin.forum/Å",
       note="non-canonical forum uri; the fk equals fk.json nfc-singleton-angstrom")
    return out


# --------------------------------------------------------------------------- self-check and output


def self_check() -> None:
    """The worked example of data-model.md §3.4 (and the superseded draft vectors' b32s)."""
    assert fk(FORUM) == "uetjixxr3a33", fk(FORUM)
    assert fk(FORUM_TEMP) == "2xbiijsnjtau", fk(FORUM_TEMP)
    assert POST.endswith("/uetjixxr3a33-3l2s5xxv2ze2c")
    assert h16("opinion", [POST]) == "wcz54x7liy3jsh3g"
    assert rkey({"ns": NS, "collection": "opinion", "forum": FORUM, "subject": POST})["rkey"] == \
        "uetjixxr3a33-3l2s5xxv2ze2c-wcz54x7liy3jsh3g"
    assert rkey({"ns": NS, "collection": "comment", "forum": FORUM, "root": POST, "tid": T2})["rkey"] == \
        "uetjixxr3a33-3l2s5xxv2ze2c-3l2s6aaaaaaa2"
    assert rkey({"ns": NS, "collection": "opinion", "forum": FORUM, "subject": COMMENT})["rkey"] == \
        "uetjixxr3a33-3l2s5xxv2ze2c-3mjfititlivah7tw"
    assert FK + "-" + h16("modAction", ["forum", PLC_B, "access"]) == "uetjixxr3a33-ckjbz7a2pjeplbao"
    assert FK + "-" + h16("modAction", ["forum", POST, "hide"]) == "uetjixxr3a33-jmsm5bwwzljcu2zo"
    assert FK + "-" + h16("modAction", ["forum", POST, "pin:forum"]) == "uetjixxr3a33-4vdjzbg6ohwpa323"
    assert FK + "-" + h16("modAction", ["forum", "tag:decision-theory", "tag"]) == "uetjixxr3a33-yext7l62fmg5ufrt"
    assert b32s(bytes.fromhex("00443214c74254b635cf84653a56d7c675be77df"), 32) == ALPHABET
    # NFC equalities the vectors claim
    assert fk("at://did:web:example.com/online.7778777.lessdugin.forum/café") == \
        fk("at://did:web:example.com/online.7778777.lessdugin.forum/café")
    assert fk("at://did:web:example.com/online.7778777.lessdugin.forum/ﬁle") != \
        fk("at://did:web:example.com/online.7778777.lessdugin.forum/file")


DESCRIPTIONS = {
    "fk": (
        "fk/1 vectors (docs/spec/data-model.md §3.1). input.forumUri is passed to fk() as a string; "
        "expected is {fk, digest} (digest = hex of H(\"lessdugin-fk/1\", forumUri)) or {error}. "
        "error \"UnitSeparator\" = a hash part contains U+001F, so there is no key. "
        "raw: true marks inputs that are not canonical v1 values (they exercise NFC and U+001F in H); "
        "runners MUST evaluate them with the string-level function, without at-uri validation. "
        "Generated by gen_keys.py; do not edit by hand."
    ),
    "sk": (
        "sk/1 hash vectors (docs/spec/data-model.md §3.1): the shared primitives b32s and H, and h16 in its "
        "opinion and modAction forms. input.fn selects the function: "
        "b32s {hex, n} -> {b32s} (first n characters; 5n <= 8*bytes); "
        "H {tag, parts} -> {preimage, digest} (hex) or {error}; "
        "h16 {name, parts} -> {h16, digest} or {error}, where the hashed parts are [name, ...parts]. "
        "error \"UnitSeparator\" = a part contains U+001F. raw: true as in fk.json. "
        "Generated by gen_keys.py; do not edit by hand."
    ),
    "rkey": (
        "sk/1 rkey vectors (docs/spec/data-model.md §3.1-§3.2; uses fk/1). input.ns is the build's NSID "
        "prefix (online.7778777.lessdugin or online.7778777.lessdugin.temp); collection names are short names "
        "resolved as ns + \".\" + name. input.fn selects the function: "
        "threadTid {uri} -> {threadTid}; stid {uri} -> {stid}; P {uri} -> {P}; actionKey {action} -> {actionKey}; "
        "rkey {collection, forum?, tid?, root?, subject?, scope?, action?} -> {rkey, ...intermediates}. "
        "Runners MUST compare rkey (or the named result) or error; the other expected keys (fk, threadTid, stid, "
        "h16, P, scopeEff, actionKey) are intermediates that runners SHOULD compare when they expose them. "
        "Errors: \"Undefined\" (threadTid/stid/P undefined for the input, a comment root that is not an NS.post "
        "uri with ^FK-TID$, or a collection whose key is not derived), \"InvalidTid\" (the tid input is not TID "
        "syntax), \"UnitSeparator\" (a hash part contains U+001F). Derivation does not check record validity "
        "(G4/G5/OP2/MA*); those are lexicon vectors. raw: true as in fk.json. "
        "Generated by gen_keys.py; do not edit by hand."
    ),
}


def documents() -> dict[str, dict]:
    return {
        "fk.json": {"behaviour": "fk/1", "kind": "fk", "description": DESCRIPTIONS["fk"], "vectors": fk_vectors()},
        "sk.json": {"behaviour": "sk/1", "kind": "hash", "description": DESCRIPTIONS["sk"], "vectors": sk_vectors()},
        "rkeys.json": {"behaviour": "sk/1", "kind": "rkey", "description": DESCRIPTIONS["rkey"],
                       "vectors": rkey_vectors()},
    }


def main() -> int:
    self_check()
    here = Path(__file__).resolve().parent
    check = "--check" in sys.argv[1:]
    status = 0
    for fname, doc in documents().items():
        names = [v["name"] for v in doc["vectors"]]
        assert len(names) == len(set(names)), f"duplicate vector names in {fname}"
        text = json.dumps(doc, ensure_ascii=True, indent=2) + "\n"
        path = here / fname
        if check:
            if not path.exists() or path.read_text(encoding="utf-8") != text:
                print(f"{fname}: out of date", file=sys.stderr)
                status = 1
        else:
            path.write_text(text, encoding="utf-8")
            print(f"wrote {path} ({len(doc['vectors'])} vectors)")
    return status


if __name__ == "__main__":
    sys.exit(main())
