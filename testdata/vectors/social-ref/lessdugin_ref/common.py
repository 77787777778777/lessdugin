# SPDX-License-Identifier: CC0-1.0
"""Shared primitives of the reference model: namespace, errors, key functions, integers.

The key functions (fk/1, sk/1) are not re-implemented: they are imported from
testdata/vectors/keys/gen_keys.py by path, so that the social vectors use exactly the
canonical keys of the keys vectors (docs/spec/data-model.md §3).
"""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

# The vectors always use the final namespace (data-model.md §1.3); runners map prefixes.
NS = "online.7778777.lessdugin"

# Short collection names of the record collections (data-model.md §1.2).
COLLECTIONS = (
    "forum", "membership", "post", "comment", "revision", "opinion", "modAction",
    "wikiRevision", "sequence", "profile",
)
ITEM_COLLECTIONS = ("post", "comment", "sequence", "wikiRevision")   # social.md §10.1 candidates
RANGED_COLLECTIONS = ("post", "comment", "revision", "opinion", "modAction", "wikiRevision", "sequence")

I64_MIN = -(2 ** 63)
I64_MAX = 2 ** 63 - 1


class NotModelled(Exception):
    """Raised whenever the reference model meets an input it does not model.

    The model never silently accepts something it does not implement: a vector generator
    that hits this must either avoid the input or extend the model.
    """


class InputError(Exception):
    """The vector input does not have the FoldInput shape of social.md §1.1."""


# --------------------------------------------------------------------------- keys (by path)

def _load_gen_keys():
    path = Path(__file__).resolve().parents[2] / "keys" / "gen_keys.py"
    name = "lessdugin_gen_keys"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


keys = _load_gen_keys()
KeyDerivationError = keys.KeyError_


def fk(forum_uri: str) -> str:
    return keys.fk(forum_uri)


def h16(name: str, parts: list[str]) -> str:
    return keys.h16(name, parts)


def thread_tid(uri: str) -> str:
    """threadTid (data-model.md §3.1); raises KeyDerivationError('Undefined')."""
    return keys.thread_tid(uri, NS)


def stid(uri: str) -> str:
    return keys.stid(uri, NS)


def P(uri: str) -> str:
    return keys.P(uri, NS)


def action_key(action: dict) -> str:
    return keys.action_key(action, NS)


# --------------------------------------------------------------------------- token grammars

FK_RE = r"[2-7a-z]{12}"
H16_RE = r"[2-7a-z]{16}"
TID_RE = r"[2-7a-j][2-7a-z]{12}"
SLUG_RE = r"[a-z0-9]+(?:-[a-z0-9]+)*"
B32S_ALPHABET = "234567abcdefghijklmnopqrstuvwxyz"


def is_slug(s) -> bool:
    return isinstance(s, str) and len(s.encode("utf-8")) <= 64 and re.fullmatch(SLUG_RE, s) is not None


def is_tid(s) -> bool:
    return isinstance(s, str) and re.fullmatch(TID_RE, s) is not None


def tid_int(tid: str) -> int:
    """The 64-bit integer value of a base32-sortable TID (13 characters)."""
    if not is_tid(tid):
        raise ValueError(f"not a TID: {tid!r}")
    n = 0
    for ch in tid:
        n = (n << 5) | B32S_ALPHABET.index(ch)
    return n


def coll_name(nsid: str):
    """Short collection name of an NS collection NSID, or None."""
    if nsid.startswith(NS + "."):
        rest = nsid[len(NS) + 1:]
        if rest in COLLECTIONS:
            return rest
    return None


def nsid(short: str) -> str:
    return f"{NS}.{short}"


def split_uri(uri: str):
    """(did, collection NSID, rkey) of a string of the form at://A/B/C, else None.

    No validity check; use syntax.is_canonical_at_uri for that.
    """
    if not isinstance(uri, str) or not uri.startswith("at://"):
        return None
    parts = uri[len("at://"):].split("/")
    if len(parts) != 3:
        return None
    return parts[0], parts[1], parts[2]


def did_of(uri: str) -> str:
    return split_uri(uri)[0]


def coll_of(uri: str):
    """Short NS collection name of a uri, or None."""
    s = split_uri(uri)
    return coll_name(s[1]) if s else None


def rkey_of(uri: str) -> str:
    return split_uri(uri)[2]


def fk_of_rkey(rkey: str) -> str:
    return rkey[:12]


# --------------------------------------------------------------------------- integers

def sat(x: int) -> int:
    """Saturate to signed 64-bit (social.md §0.4)."""
    return I64_MIN if x < I64_MIN else I64_MAX if x > I64_MAX else x


def sat_add(a: int, b: int) -> int:
    return sat(a + b)


def sat_mul(a: int, b: int) -> int:
    return sat(a * b)


def min_some(*xs):
    present = [x for x in xs if x is not None]
    return min(present) if present else None


def ubytes(s: str) -> bytes:
    """Sort key: strings compare by their UTF-8 bytes (social.md §0.4)."""
    return s.encode("utf-8")
