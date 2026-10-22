# SPDX-License-Identifier: CC0-1.0
"""DAG-CBOR encoding of atproto data-model values (JSON form) and CIDv1 (dag-cbor, sha256).

Used for G6 (encoded size), RV5 (revision snapshot CID) and by the vector builders to give
every record a genuine CID. The fold itself treats CIDs as opaque strings (social.md §1.1).
Checked against testdata/interop/data-model/data-model-fixtures.json (see selftest()).
"""

from __future__ import annotations

import base64
import hashlib
import struct

from .common import NotModelled

CID_PREFIX = b"\x01\x71\x12\x20"    # CIDv1, dag-cbor, sha2-256, 32-byte digest


def _head(major: int, n: int) -> bytes:
    if n < 24:
        return bytes([(major << 5) | n])
    if n < 0x100:
        return bytes([(major << 5) | 24, n])
    if n < 0x10000:
        return bytes([(major << 5) | 25]) + struct.pack(">H", n)
    if n < 0x100000000:
        return bytes([(major << 5) | 26]) + struct.pack(">I", n)
    return bytes([(major << 5) | 27]) + struct.pack(">Q", n)


def cid_to_bytes(cid: str) -> bytes:
    if not cid.startswith("b"):
        raise NotModelled(f"only base32 multibase CIDs are modelled: {cid!r}")
    body = cid[1:].upper()
    body += "=" * (-len(body) % 8)
    return base64.b32decode(body)


def cid_from_bytes(raw: bytes) -> str:
    return "b" + base64.b32encode(raw).decode("ascii").lower().rstrip("=")


def encode(v) -> bytes:
    if v is None:
        return b"\xf6"
    if v is True:
        return b"\xf5"
    if v is False:
        return b"\xf4"
    if isinstance(v, int):
        return _head(0, v) if v >= 0 else _head(1, -1 - v)
    if isinstance(v, float):
        raise NotModelled("floats are not part of lessdugin records")
    if isinstance(v, str):
        b = v.encode("utf-8")
        return _head(3, len(b)) + b
    if isinstance(v, (bytes, bytearray)):
        return _head(2, len(v)) + bytes(v)
    if isinstance(v, list):
        return _head(4, len(v)) + b"".join(encode(x) for x in v)
    if isinstance(v, dict):
        if set(v) == {"$link"} and isinstance(v["$link"], str):
            raw = b"\x00" + cid_to_bytes(v["$link"])
            return b"\xd8\x2a" + _head(2, len(raw)) + raw
        if set(v) == {"$bytes"} and isinstance(v["$bytes"], str):
            s = v["$bytes"]
            return encode(base64.b64decode(s + "=" * (-len(s) % 4)))
        keys = sorted(v, key=lambda k: (len(k.encode("utf-8")), k.encode("utf-8")))
        out = [_head(5, len(v))]
        for k in keys:
            out.append(encode(k))
            out.append(encode(v[k]))
        return b"".join(out)
    raise NotModelled(f"not a data-model value: {type(v).__name__}")


def cid_of_bytes(data: bytes) -> str:
    return cid_from_bytes(CID_PREFIX + hashlib.sha256(data).digest())


def cid_of(value) -> str:
    """CIDv1 (dag-cbor, sha256), base32 lowercase with multibase prefix 'b'."""
    return cid_of_bytes(encode(value))


def selftest(fixtures_path) -> None:
    import json
    with open(fixtures_path, encoding="utf-8") as f:
        fixtures = json.load(f)
    for fx in fixtures:
        enc = encode(fx["json"])
        want = base64.b64decode(fx["cbor_base64"] + "=" * (-len(fx["cbor_base64"]) % 4))
        assert enc == want, "dag-cbor encoding mismatch"
        assert cid_of(fx["json"]) == fx["cid"], "cid mismatch"
