# SPDX-License-Identifier: CC0-1.0
"""CIDs written in records compare by their canonical form (social.md §0.4, §20.1 item 45).

    canon(s) -> str | None
        The canonical string form (CIDv1, multibase `b`, base32 lowercase, no padding) of a CID
        written in a record, or None when s is not a CIDv1 in any multibase: such a string names
        no version. A CIDv0 (`Qm…`, or the 0x12 0x20 binary under a multibase prefix) is not a
        CIDv1. Raises NotModelled for a multibase prefix this model does not decode, rather than
        guess.
    same(written, cid) -> bool
        canon(written) is not None and equals canon(cid). The fold input's own CIDs are already
        canonical (the sync layer computes them); they go through canon() too, which leaves a
        canonical CID unchanged.

Decoded multibases: base16 (f, F), base32 (b, B), base32pad (c, C), base32hex (v, V),
base32hexpad (t, T), base32z (h), base36 (k, K), base58btc (z), base58flickr (Z), base64 (m),
base64pad (M), base64url (u), base64urlpad (U). Decoding is strict: one letter case per
alphabet, no non-zero trailing bits, padding only where the base has it. The CID bytes must be
<varint 1><varint codec><varint hash code><varint digest length><digest>, with minimal varints
and nothing after the digest.
"""

from __future__ import annotations

from .common import NotModelled

B32_LOWER = "abcdefghijklmnopqrstuvwxyz234567"
B32HEX_LOWER = "0123456789abcdefghijklmnopqrstuv"
B32Z = "ybndrfg8ejkmcpqxot1uwisza345h769"
B16_LOWER = "0123456789abcdef"
B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
B64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
B58BTC = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
B58FLICKR = "123456789abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ"
B36_LOWER = "0123456789abcdefghijklmnopqrstuvwxyz"

# prefix -> (kind, alphabet, padded)
_BASES = {
    "f": ("bits", B16_LOWER, False), "F": ("bits", B16_LOWER.upper(), False),
    "b": ("bits", B32_LOWER, False), "B": ("bits", B32_LOWER.upper(), False),
    "c": ("bits", B32_LOWER, True), "C": ("bits", B32_LOWER.upper(), True),
    "v": ("bits", B32HEX_LOWER, False), "V": ("bits", B32HEX_LOWER.upper(), False),
    "t": ("bits", B32HEX_LOWER, True), "T": ("bits", B32HEX_LOWER.upper(), True),
    "h": ("bits", B32Z, False),
    "m": ("bits", B64, False), "M": ("bits", B64, True),
    "u": ("bits", B64URL, False), "U": ("bits", B64URL, True),
    "k": ("basex", B36_LOWER, False), "K": ("basex", B36_LOWER.upper(), False),
    "z": ("basex", B58BTC, False), "Z": ("basex", B58FLICKR, False),
}
# The other multibase prefixes (identity, base2, base8, base10, proquint, base45, base256emoji).
_NOT_DECODED = ("\x00", "0", "7", "9", "p", "R", "\U0001F680")


def _bits_decode(s: str, alphabet: str, padded: bool):
    width = {16: 4, 32: 5, 64: 6}[len(alphabet)]
    if padded:
        group = {4: 2, 5: 8, 6: 4}[width]           # characters per padded group
        if len(s) % group:
            return None
        s = s.rstrip("=")
    if "=" in s:
        return None
    acc, nbits, out = 0, 0, bytearray()
    for ch in s:
        i = alphabet.find(ch)
        if i < 0:
            return None
        acc = (acc << width) | i
        nbits += width
        if nbits >= 8:
            nbits -= 8
            out.append((acc >> nbits) & 0xFF)
    if nbits >= width or acc & ((1 << nbits) - 1):
        return None                                  # a dangling character or non-zero trailing bits
    if padded and len(_bits_encode(bytes(out), alphabet)) != len(s):
        return None
    return bytes(out)


def _bits_encode(b: bytes, alphabet: str) -> str:
    width = {16: 4, 32: 5, 64: 6}[len(alphabet)]
    acc, nbits, out = 0, 0, []
    for byte in b:
        acc = (acc << 8) | byte
        nbits += 8
        while nbits >= width:
            nbits -= width
            out.append(alphabet[(acc >> nbits) & ((1 << width) - 1)])
    if nbits:
        out.append(alphabet[(acc << (width - nbits)) & ((1 << width) - 1)])
    return "".join(out)


def _basex_decode(s: str, alphabet: str):
    if not s:
        return None
    n = 0
    for ch in s:
        i = alphabet.find(ch)
        if i < 0:
            return None
        n = n * len(alphabet) + i
    zeros = len(s) - len(s.lstrip(alphabet[0]))
    body = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    return b"\x00" * zeros + body


def _varint(b: bytes, i: int):
    """(value, next index) of a minimal unsigned varint at b[i:], or None."""
    n, shift, start = 0, 0, i
    while True:
        if i >= len(b) or i - start >= 9:
            return None
        x = b[i]
        n |= (x & 0x7F) << shift
        i += 1
        if not x & 0x80:
            break
        shift += 7
    if i - start > 1 and b[i - 1] == 0:
        return None                                  # not minimal
    return n, i


def _cidv1_bytes_ok(b: bytes) -> bool:
    i = 0
    for _ in range(3):                               # version, codec, multihash code
        r = _varint(b, i)
        if r is None:
            return False
        if _ == 0 and r[0] != 1:
            return False
        i = r[1]
    r = _varint(b, i)
    if r is None:
        return False
    length, i = r
    return len(b) - i == length


def canon(s):
    if not isinstance(s, str) or not s:
        return None
    prefix, body = s[0], s[1:]
    if prefix in _NOT_DECODED:
        raise NotModelled(f"multibase prefix {prefix!r} is not decoded by the model: {s!r}")
    base = _BASES.get(prefix)
    if base is None:
        return None                                  # not a multibase string (e.g. a CIDv0 "Qm…")
    kind, alphabet, padded = base
    raw = _bits_decode(body, alphabet, padded) if kind == "bits" else _basex_decode(body, alphabet)
    if raw is None or not _cidv1_bytes_ok(raw):
        return None
    return "b" + _bits_encode(raw, B32_LOWER)


def same(written, cid) -> bool:
    c = canon(written)
    return c is not None and c == canon(cid)


def upper_spelling(cid: str) -> str:
    """The base32 upper-case spelling of a canonical CID (multibase `B`), for vectors."""
    assert canon(cid) == cid, cid
    return "B" + cid[1:].upper()


def base58btc_spelling(cid: str) -> str:
    """The base58btc spelling of a canonical CID (multibase `z`), for vectors."""
    assert canon(cid) == cid, cid
    z = "z" + _basex_encode(_bits_decode(cid[1:], B32_LOWER, False), B58BTC)
    assert canon(z) == cid
    return z


def selftest() -> None:
    from .dagcbor import cid_of
    c = cid_of({"a": 1})
    raw = _bits_decode(c[1:], B32_LOWER, False)
    assert raw is not None and _cidv1_bytes_ok(raw) and canon(c) == c
    assert canon(upper_spelling(c)) == c
    assert canon("f" + raw.hex()) == c and canon("F" + raw.hex().upper()) == c
    assert canon("z" + _basex_encode(raw, B58BTC)) == c
    assert canon("m" + _bits_encode(raw, B64)) == c
    assert canon("c" + c[1:] + "=" * (-len(c[1:]) % 8)) == c
    assert canon("Qmb" + c[1:]) is None and canon("b" + c[1:-1]) is None and canon("bafy") is None
    assert canon(c + "a") is None and canon("x" + c[1:]) is None
    # CIDv0 bytes (0x12 0x20 …) under a multibase prefix are not a CIDv1
    v0 = bytes([0x12, 0x20]) + raw[-32:]
    assert canon("b" + _bits_encode(v0, B32_LOWER)) is None


def _basex_encode(b: bytes, alphabet: str) -> str:
    n = int.from_bytes(b, "big")
    out = []
    while n:
        n, r = divmod(n, len(alphabet))
        out.append(alphabet[r])
    zeros = len(b) - len(b.lstrip(b"\x00"))
    return alphabet[0] * zeros + "".join(reversed(out))
