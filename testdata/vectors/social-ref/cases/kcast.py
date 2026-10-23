# SPDX-License-Identifier: CC0-1.0
"""A cast in which K ≠ KP (social.md §6.2), shared by the karma, reacts, tags and author cases.

Every other cast has K1 = KP = K for the users that matter, so a fold that reads K where the
spec says KP (or the reverse) passes them. Here, in an approval-mode forum where every user is
granted (E = S) and pairCap = 25 (the default):

    user                 receives (on the user's own post)                 K1   KP   K
    boostera … boosterl  nothing                                            0    0    0
    tess                 up from boostera … boosterj (10)                   10   10   10
    sid                  up from boostera … boosterk (11), strongDown       10    9    9
                         from tess                          KP: 11 − strong(K1 tess 10 → 2)
                                                            K:  11 − strong(KP tess 10 → 2)
    vera                 up from boostera … boosterh (8), strongUp          9   10    9
                         from sid                           KP: 8 + strong(K1 sid 10 → 2)
                                                            K:  8 + strong(KP sid 9 → 1)
    wes                  strongUp from vera                                 1    1    2
                                                            KP: strong(K1 vera 9 → 1)
                                                            K:  strong(KP vera 10 → 2)
    abe                  up from vera                                       1    1    1

did(abe) < did(wes), so a tie on KP between them goes to abe by DID, and K separates them.
"""

from __future__ import annotations

from lessdugin_ref.build import MIN, T0, Forum, did

BOOSTERS = tuple("booster" + c for c in "abcdefghijkl")        # 12
USERS = ("tess", "sid", "vera", "wes", "abe")


def forum(*, policy=None, moderators=(), **kw) -> Forum:
    return Forum(membership="approval", policy=policy, moderators=moderators, **kw)


def cast(f: Forum, *, t0=T0) -> dict:
    """The cast's records: {"records": [...], "posts": {name: post record}}.

    Each user writes one post (t0 + i minutes); memberships and grants by the owner for everyone.
    """
    names = (*BOOSTERS, *USERS)
    recs = []
    for n in names:
        recs.append(f.membership(n))
        recs.append(f.modaction("owner", "access", did(n), level="member", t=t0 + MIN))
    posts = {}
    for i, n in enumerate(USERS, 2):
        posts[n] = f.post(n, t0 + i * MIN, title=f"{n}'s post")
    posts["boosterl"] = f.post("boosterl", t0 + 9 * MIN, title="boosterl's post")
    t = t0 + 10 * MIN
    for b in BOOSTERS[:10]:
        recs.append(f.opinion(b, posts["tess"], t=t, karma="up"))
    for b in BOOSTERS[:11]:
        recs.append(f.opinion(b, posts["sid"], t=t, karma="up"))
    recs.append(f.opinion("tess", posts["sid"], t=t, karma="strongDown"))
    for b in BOOSTERS[:8]:
        recs.append(f.opinion(b, posts["vera"], t=t, karma="up"))
    recs.append(f.opinion("sid", posts["vera"], t=t, karma="strongUp"))
    recs.append(f.opinion("vera", posts["wes"], t=t, karma="strongUp"))
    recs.append(f.opinion("vera", posts["abe"], t=t, karma="up"))
    return {"records": [*recs, *posts.values()], "posts": posts}


EXPECTED = {
    "K1": {"tess": 10, "sid": 10, "vera": 9, "wes": 1, "abe": 1},
    "KP": {"tess": 10, "sid": 9, "vera": 10, "wes": 1, "abe": 1},
    "K": {"tess": 10, "sid": 9, "vera": 9, "wes": 2, "abe": 1},
}


def check(view) -> None:
    """Assert the table above against a fold of a vector that contains the cast."""
    assert did("abe").encode() < did("wes").encode()
    for stage, want in EXPECTED.items():
        got = getattr(view.l3, stage)
        for n, k in want.items():
            assert got.get(did(n), 0) == k, (stage, n, got.get(did(n), 0), k)
        for b in BOOSTERS:
            assert got.get(did(b), 0) == 0, (stage, b)
