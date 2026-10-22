# SPDX-License-Identifier: CC0-1.0
"""inputDigest (social.md §1.2)."""

from __future__ import annotations

import hashlib

from .policy import algs_line, params_line


def preimage(inp, policy) -> bytes:
    lines = set()
    for u, rec in inp.records.items():
        lines.add(u.encode("utf-8") + b"\x00" + rec.cid.encode("ascii") + b"\x0a")
    for u in inp.absent:
        lines.add(u.encode("utf-8") + b"\x00" + b"-" + b"\x0a")
    d = b"".join(sorted(lines))
    d += b"decl\x00" + inp.decl.cid.encode("ascii") + b"\x0a"
    d += b"algs\x00" + algs_line(policy).encode("ascii") + b"\x0a"
    d += b"params\x00" + params_line(policy).encode("ascii") + b"\x0a"
    return d


def input_digest(inp, policy) -> str:
    return hashlib.sha256(preimage(inp, policy)).hexdigest()
