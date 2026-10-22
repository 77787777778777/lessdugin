# SPDX-License-Identifier: CC0-1.0
"""L6: ranking, sorts, filters and Recent Discussion (social.md §12, magic/1). STUB.

Interface (to be filled by the ranking stage):

    hot(b: int, t: int, now: int, policy) -> float          §12.1, operations in exactly that order
    hot_bits(x: float) -> str                               16 lowercase hex digits of the binary64
    post_order(view, sort, filter, tag=None, kind=None, author=None) -> [post uri…]   §12.2–§12.3
    comment_order(view, p, sort) -> [uri…]                  §12.4, siblings sorted at every level,
                                                            pre-order, Missing placeholders last
    recent_discussion(view) -> [(post uri, [comment uri…])] §12.5

Python's math.pow may differ from libm::pow in the last bit; rank vectors therefore carry
"hotBitsSource": "python-math.pow" (testdata/vectors/README.md, social section).
"""

from __future__ import annotations

import struct

from .common import NotModelled


def hot_bits(x: float) -> str:
    return struct.pack(">d", x).hex()


def hot(b: int, t: int, now: int, policy) -> float:
    raise NotModelled("L6: hot (social.md §12.1) is not filled yet")


def post_order(view, sort: str, filter: str, tag=None, kind=None, author=None) -> list:
    raise NotModelled("L6: post sorts (social.md §12.2) are not filled yet")


def comment_order(view, p: str, sort: str) -> list:
    raise NotModelled("L6: comment sorts (social.md §12.4) are not filled yet")


def recent_discussion(view) -> list:
    raise NotModelled("L6: Recent Discussion (social.md §12.5) is not filled yet")
