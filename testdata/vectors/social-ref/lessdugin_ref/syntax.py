# SPDX-License-Identifier: CC0-1.0
"""atproto string syntaxes used by the fold: datetime, DID, NSID, record key, at-uri, CID, URI.

`datetime_us` is social.md §2.1 µs(datetime). The canonical at-uri form is data-model.md §2.3.
"""

from __future__ import annotations

import re

from .common import (FK_RE, H16_RE, SLUG_RE, TID_RE, coll_name, split_uri)

# --------------------------------------------------------------------------- datetime

# 1–20 fraction digits and at most 64 characters: the limits of the atproto reference
# implementations (Go indigo and TypeScript @atproto/syntax) and of crates/atp (NOTES.md 17).
# With at most 20 fraction digits the longest datetime has 46 characters, so the length limit
# never binds; it is checked anyway.
DATETIME_MAX_LEN = 64
DATETIME_MAX_FRACTION_DIGITS = 20
_DT_RE = re.compile(
    r"([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})"
    rf"(?:\.([0-9]{{1,{DATETIME_MAX_FRACTION_DIGITS}}}))?"
    r"(Z|[+-][0-9]{2}:[0-9]{2})"
)


def _days_from_civil(y: int, m: int, d: int) -> int:
    """Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm)."""
    y -= m <= 2
    era = (y if y >= 0 else y - 399) // 400
    yoe = y - era * 400
    doy = (153 * (m + (-3 if m > 2 else 9)) + 2) // 5 + d - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return era * 146097 + doe - 719468


def _days_in_month(y: int, m: int) -> int:
    if m == 2:
        leap = (y % 4 == 0 and y % 100 != 0) or y % 400 == 0
        return 29 if leap else 28
    return 30 if m in (4, 6, 9, 11) else 31


_YEAR0_US = _days_from_civil(0, 1, 1) * 86_400_000_000


def datetime_parts(s):
    """Parse an atproto datetime; return its UTC instant in µs (may be negative), or None.

    Accepted: YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM), at most 64 characters, uppercase T
    and Z, 1 to 20 fraction digits, real calendar dates, hour ≤ 23, minute and second ≤ 59
    (no leap second), offset hour ≤ 23 and minute ≤ 59, never "-00:00", and an instant not
    before 0000-01-01T00:00:00Z (testdata/interop/syntax/datetime_*.txt; NOTES.md 17).
    Fraction digits beyond the sixth are truncated.
    """
    if not isinstance(s, str) or len(s) > DATETIME_MAX_LEN:
        return None
    m = _DT_RE.fullmatch(s)
    if not m:
        return None
    y, mo, d, hh, mi, ss = (int(m.group(i)) for i in range(1, 7))
    frac, tz = m.group(7), m.group(8)
    if not (1 <= mo <= 12 and 1 <= d <= _days_in_month(y, mo)):
        return None
    if hh > 23 or mi > 59 or ss > 59:
        return None
    off = 0
    if tz != "Z":
        if tz == "-00:00":
            return None
        oh, om = int(tz[1:3]), int(tz[4:6])
        if oh > 23 or om > 59:
            return None
        off = (oh * 60 + om) * 60_000_000 * (1 if tz[0] == "+" else -1)
    micro = int((frac or "").ljust(6, "0")[:6])
    us = (_days_from_civil(y, mo, d) * 86_400 + hh * 3600 + mi * 60 + ss) * 1_000_000 + micro - off
    if us < _YEAR0_US:
        return None
    return us


def is_datetime(s) -> bool:
    return datetime_parts(s) is not None


def datetime_us(s):
    """µs(datetime) of social.md §2.1: None if it does not parse or is < 0."""
    us = datetime_parts(s)
    if us is None or us < 0:
        return None
    return us


# --------------------------------------------------------------------------- identifiers

_DID_RE = re.compile(r"did:[a-z]+:[a-zA-Z0-9._:%-]*[a-zA-Z0-9._-]")
_PLC_RE = re.compile(r"did:plc:[a-z2-7]{24}")
_HOST_LABEL = r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
_WEB_RE = re.compile(rf"did:web:{_HOST_LABEL}(?:\.{_HOST_LABEL})+")
_NSID_RE = re.compile(
    r"[a-zA-Z](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?"
    r"(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+"
    r"\.[a-zA-Z][a-zA-Z0-9]{0,62}"
)
_RKEY_RE = re.compile(r"[A-Za-z0-9._:~-]{1,512}")
_CID_RE = re.compile(r"[a-zA-Z0-9+=]{8,256}")
_URI_RE = re.compile(r"[a-zA-Z][a-zA-Z0-9+.-]*:[!-~]+")


def is_did(s) -> bool:
    """The generic atproto DID syntax (format `did`)."""
    return isinstance(s, str) and len(s) <= 2048 and _DID_RE.fullmatch(s) is not None


def is_canonical_did(s) -> bool:
    """data-model.md §2.3: did:plc:[a-z2-7]{24} or did:web:<lowercase hostname>."""
    if not isinstance(s, str):
        return False
    if _PLC_RE.fullmatch(s):
        return True
    m = _WEB_RE.fullmatch(s)
    return m is not None and len(s) - len("did:web:") <= 253


def is_nsid(s) -> bool:
    return isinstance(s, str) and len(s) <= 317 and _NSID_RE.fullmatch(s) is not None


def is_rkey(s) -> bool:
    return isinstance(s, str) and s not in (".", "..") and _RKEY_RE.fullmatch(s) is not None


def is_cid(s) -> bool:
    """The atproto `cid` string syntax; CIDv0 (`Qmb…`) is not allowed (interop fixtures)."""
    return isinstance(s, str) and _CID_RE.fullmatch(s) is not None and not s.startswith("Qmb")


def is_uri(s) -> bool:
    return isinstance(s, str) and len(s.encode("utf-8")) <= 3000 and _URI_RE.fullmatch(s) is not None


# Per-collection rkey grammars (data-model.md §3.2). The revision grammar is `^P-TID$` with
# P ∈ {FK, FK-TID, FK-TID-TID, FK-H16}; as a reference target only the shape can be checked.
RKEY_GRAMMAR = {
    "forum": rf"{SLUG_RE}",
    "membership": rf"{FK_RE}",
    "post": rf"{FK_RE}-{TID_RE}",
    "comment": rf"{FK_RE}-{TID_RE}-{TID_RE}",
    "revision": rf"{FK_RE}(?:-{TID_RE}|-{TID_RE}-{TID_RE}|-{H16_RE})?-{TID_RE}",
    "opinion": rf"{FK_RE}-(?:{TID_RE}|~)-{H16_RE}",
    "modAction": rf"{FK_RE}-(?:{H16_RE}|{TID_RE})",
    "wikiRevision": rf"{FK_RE}-{TID_RE}",
    "sequence": rf"{FK_RE}-{TID_RE}",
    "profile": r"self",
}


def is_canonical_at_uri(u, allowed=None) -> bool:
    """data-model.md §2.3 canonical at-uri; `allowed` is a set of short NS collection names
    (None = any collection, with the NS grammar still enforced for NS collections)."""
    if not isinstance(u, str) or len(u.encode("utf-8")) > 3000:
        return False
    parts = split_uri(u)
    if parts is None:
        return False
    did, coll, rkey = parts
    if not (is_canonical_did(did) and is_nsid(coll) and is_rkey(rkey)):
        return False
    short = coll_name(coll)
    if allowed is not None and short not in allowed:
        return False
    if short is not None and re.fullmatch(RKEY_GRAMMAR[short], rkey) is None:
        return False
    return True


# --------------------------------------------------------------------------- White_Space

# Unicode White_Space property (PropList.txt); stable since Unicode 6.3.
WHITE_SPACE = frozenset(
    [0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0x85, 0xA0, 0x1680, *range(0x2000, 0x200B),
     0x2028, 0x2029, 0x202F, 0x205F, 0x3000]
)


def has_non_white_space(s: str) -> bool:
    return any(ord(c) not in WHITE_SPACE for c in s)
