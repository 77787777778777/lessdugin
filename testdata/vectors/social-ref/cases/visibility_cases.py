# SPDX-License-Identifier: CC0-1.0
"""visibility/visibility.json (kind "visibility"): social.md §10.1–§10.2 (L6) and §10.3 (L7).

Each "first-match" vector holds items that match two or more rules, so the first matching rule
of V1–V10 decides. `states` is the V1–V10 state of every candidate item, `displayed` the state
after the thread pass (§10.2). The lens (L7) is never asserted: every vector is also checked
here to come out unchanged when a lens muting every author is applied after the fold.
"""

from __future__ import annotations

import json

from lessdugin_ref.build import HOUR, MIN, T0, Forum, did, nsid, rec
from lessdugin_ref.fold import fold_vector_input
from lessdugin_ref.projections import project
from lessdugin_ref.visibility import lens

from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))
GATES_0 = {"authorModPersonal": 0, "authorModFrontpage": 0}
FIELDS = ("states", "displayed")


def F(*, gates=None, display=None, policy=None, **kw) -> Forum:
    kw.setdefault("moderators", MODS)
    kw.setdefault("membership", "open")
    pol = dict(policy or {})
    if gates is not None:
        pol["gates"] = dict(gates)
    if display is not None:
        pol["display"] = dict(display)
    return Forum(policy=pol or None, **kw)


def u(r):
    return r if isinstance(r, str) else r["uri"]


def mem(f, *names):
    return [f.membership(n) for n in names]


def grant(f, *names):
    return [f.modaction("owner", "access", did(n), level="member", t=T0 + MIN) for n in names]


VIS = {"state": "Visible"}
EXC = {"state": "Excluded"}


def hid(reason):
    return {"state": "Hidden", "reason": reason}


def col(reason):
    return {"state": "Collapsed", "reason": reason}


def removed(rank, actor, spam=False, reason=None, state="Hidden"):
    return {"state": state, "reason": "Removed",
            "removed": {"rank": rank, "actor": did(actor), "spam": spam, "reason": reason}}


def tomb(reason):
    return {"state": "Tombstone", "reason": reason}


def lens_check(v) -> None:
    """§10.3: the lens never changes a fold output. Apply it (muting every author) to a fresh
    fold of the vector's input and re-project every field: nothing may differ."""
    view = fold_vector_input(json.loads(json.dumps(v["input"])))
    before = project(view, "visibility", FIELDS)
    authors = frozenset(r.did for r in view.input.records.values())
    lensed = lens(view, view.displayed, mutes=authors)
    after = project(view, "visibility", FIELDS)
    assert before == after == v["expected"], v["name"]
    for x, s in lensed.items():
        want = "Collapsed" if view.displayed[x].listed else view.displayed[x].state
        assert s.state == want and (not view.displayed[x].listed or s.reason == "Muted"), (v["name"], x)


def V(name, f, records, note, *, fields=FIELDS, invalid=None, fi=None):
    v = vector("visibility", name, f.fold_input(records, **(fi or {})), fields=fields, note=note,
               invalid=invalid)
    lens_check(v)
    return v


def S(v, r):
    return v["expected"]["states"][u(r)]


def D(v, r):
    return v["expected"]["displayed"][u(r)]


def in_other_forum(f, r):
    """The same record with value.forum naming another forum (the key keeps this forum's fk)."""
    other = f"at://{did('other')}/{nsid('forum')}/elsewhere"
    return rec(r["uri"], {**r["value"], "forum": other})


def visibility_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ first match: V1 … V10
    f = F(membership="approval")
    pb = f.post("bob", T0 + HOUR, title=" ")
    hb = f.modaction("mona", "hide", pb, t=T0 + 2 * HOUR)
    q = in_other_forum(f, f.post("alice", T0 + 3 * HOUR))
    c = f.comment("carol", q, T0 + 4 * HOUR)
    pa = f.post("alice", T0 + 5 * HOUR)
    v = V("v1-before-v2-and-v3", f, [*mem(f, "alice", "carol"), *grant(f, "alice", "carol"), pb, hb, q, c, pa],
          "V1 first: bob's post is invalid (PO3: a title of U+2003 only) although bob is no participant and "
          "mona hid it; alice's other post names another forum in value.forum (G5/G9: invalid) and is "
          "Excluded. carol's valid comment on it is not: CO5 compares only a valid root (social.md §20.1 "
          "item 51), so the comment sits under a Missing root and is Visible",
          invalid={u(pb): "PO3", u(q): "G5"})
    assert [S(v, x) for x in (pb, q, c, pa)] == [EXC, EXC, VIS, VIS] and v["expected"]["displayed"] == \
        v["expected"]["states"]
    out.append(v)

    f = F(membership="approval")
    pb, pc, pd = (f.post(n, T0 + i * HOUR) for i, n in ((1, "bob"), (2, "carol"), (3, "dave")))
    hs = [f.modaction("mona", "hide", p, t=T0 + 4 * HOUR) for p in (pb, pc, pd)]
    v = V("v2-not-participant-before-v3", f, [*mem(f, "carol", "dave"), *grant(f, "dave"), pb, pc, pd, *hs],
          "approval mode: bob has no membership and carol is an applicant (membership, no grant), so both "
          "are outside P and V2 applies before mona's hides; dave is granted, so V3 hides his post")
    assert [S(v, x) for x in (pb, pc, pd)] == [hid("NotParticipant"), hid("NotParticipant"), removed(1, "mona")]
    out.append(v)

    f = F()
    pa, pb, pc, pm = (f.post(n, T0 + i * HOUR) for i, n in ((1, "alice"), (2, "bob"), (3, "carol"), (4, "mona")))
    hs = [f.modaction("mona", "hide", p, t=T0 + 5 * HOUR) for p in (pa, pb)]
    v = V("v2-moderation-unavailable-before-v3", f, [*mem(f, "alice", "bob"), *grant(f, "bob"), pa, pb, pc, pm, *hs],
          "mike's repo is Unavailable, so the view is degraded: alice participates (open mode) but is not "
          "in S, so Hidden(ModerationUnavailable) comes before mona's hide; bob is granted (in S) and his "
          "post is Removed; carol has no membership (the first V2 clause); mona is an authority",
          fi={"repo_status": {did("mike"): "unavailable"}})
    assert [S(v, x) for x in (pa, pb, pc, pm)] == [hid("ModerationUnavailable"), removed(1, "mona"),
                                                   hid("NotParticipant"), VIS]
    out.append(v)

    f = F()
    p1, p2 = (f.post("bob", T0 + i * HOUR) for i in (1, 2))
    ban = f.modaction("mona", "access", did("bob"), level="banned", t=T0 + 3 * HOUR)
    h = f.modaction("owner", "hide", p1, reason="doxxing", t=T0 + 4 * HOUR)
    v = V("v3-before-v4", f, [*mem(f, "bob"), p1, p2, ban, h],
          "bob is banned (rank 1); the owner also hid p1, and V3 (Removed, with the winner's rank, actor and "
          "reason) comes before V4; p2 is Hidden(Banned)")
    assert S(v, p1) == removed(3, "owner", reason="doxxing") and S(v, p2) == hid("Banned")
    out.append(v)

    f = F(membership="approval")
    p1, p2, p3 = (f.post("carol", T0 + i * HOUR) for i in (1, 2, 3))
    ban = f.modaction("mona", "access", did("carol"), level="banned", t=T0 + 4 * HOUR)
    rst = f.modaction("mike", "restrict", did("carol"), capabilities=["post"], t=T0 + 4 * HOUR)
    k2 = f.modaction("adam", "hide", p2, hidden=False, t=T0 + 5 * HOUR)
    v = V("v4-before-v5", f, [*mem(f, "carol"), p1, p2, p3, ban, rst, k2],
          "approval mode: carol (member, banned) participates; she is banned and restricted from posting at "
          "rank 1, and V4 comes first; adam's keep of p2 (rank 2) exempts it from both, so it falls through "
          "to V10 (V8 applies in open mode only; her score is 0 > collapseBelow −5)")
    assert [S(v, x) for x in (p1, p2, p3)] == [hid("Banned"), VIS, hid("Banned")]
    out.append(v)

    f = F(gates=GATES_0)
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    c1 = f.comment("dave", pa, T0 + 3 * HOUR)
    c2 = f.comment("dave", pb, T0 + 4 * HOUR)
    rst = f.modaction("mona", "restrict", did("dave"), capabilities=["comment"], t=T0 + 5 * HOUR)
    ah = f.modaction("alice", "hide", c1, scope="author", t=T0 + 5 * HOUR)
    v = V("v5-before-v6", f, [*mem(f, "alice", "bob", "dave"), *grant(f, "alice", "bob"), pa, pb, c1, c2, rst, ah],
          "dave is restricted from commenting (rule 4b) and alice also author-hid his comment in her thread: "
          "V5 (Restricted) comes before V6")
    assert S(v, c1) == hid("Restricted") and S(v, c2) == hid("Restricted")
    out.append(v)

    f = F(gates=GATES_0)
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("carol", p, T0 + 2 * HOUR)
    c2 = f.comment("dave", p, T0 + 3 * HOUR)
    lk = f.modaction("mona", "lock", p, allowed=[], t=T0 + 4 * HOUR)
    ah = f.modaction("alice", "hide", c1, scope="author", t=T0 + 4 * HOUR)
    v = V("v6-before-v7", f, [*mem(f, "alice", "carol", "dave"), *grant(f, "alice", "carol", "dave"), p, c1, c2, lk, ah],
          "mona's forum lock (allowed = []) covers both comments; alice's author hide of carol's comment "
          "still applies (the lock does not resolve `hidden`), and V6 comes before V7")
    assert S(v, c1) == hid("AuthorModerated") and S(v, c2) == hid("Locked")
    out.append(v)

    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    c1 = f.comment("dave", p1, T0 + 3 * HOUR)
    c2 = f.comment("dave", p2, T0 + 4 * HOUR)
    lk = f.modaction("mona", "lock", p1, allowed=[], t=T0 + 5 * HOUR)
    v = V("v7-before-v8", f, [*mem(f, "alice", "dave"), *grant(f, "alice"), p1, p2, c1, c2, lk],
          "dave is a newcomer (open mode, not in S ∪ E): his comment on the locked post is Hidden(Locked) "
          "(V7 first); on the other post it is Collapsed(NewUser)")
    assert S(v, c1) == hid("Locked") and S(v, c2) == col("NewUser")
    out.append(v)

    for name, users, want_e, note in (
            ("v8-before-v9", "collapse", col("NewUser"),
             "collapseBelow = 0. erin is a newcomer with score 0 (self(i) = 0 outside E): V8 "
             "(newUsers = collapse) comes before V9; alice is granted, her post scores self = 1 > 0"),
            ("v8-hide-new-users", "hide", hid("NewUser"),
             "newUsers = hide: a newcomer's item is Hidden(NewUser)"),
            ("v8-show-new-users-then-v9", "show", col("LowScore"),
             "newUsers = show: V8 does not apply, so erin's post reaches V9: score 0 ≤ collapseBelow 0")):
        f = F(display={"collapseBelow": 0, "newUsers": users})
        pa = f.post("alice", T0 + HOUR)
        pe = f.post("erin", T0 + 2 * HOUR)
        v = V(name, f, [*mem(f, "alice", "erin"), *grant(f, "alice"), pa, pe], note)
        assert S(v, pa) == VIS and S(v, pe) == want_e
        out.append(v)

    f = F(display={"collapseBelow": 1})
    pa = f.post("alice", T0 + HOUR)
    ca = f.comment("alice", pa, T0 + 2 * HOUR)
    v = V("v9-score-equal-to-collapseBelow", f, [*mem(f, "alice"), *grant(f, "alice"), pa, ca],
          "V9 is score ≤ collapseBelow: with collapseBelow = 1 the granted author's post and comment, which "
          "score self(i) = 1, are Collapsed(LowScore)")
    assert S(v, pa) == col("LowScore") and S(v, ca) == col("LowScore")
    out.append(v)

    f = F()
    voters = ("owner", "adam", "mona", "mike", "sam", "tom")
    p5 = f.post("alice", T0 + HOUR)
    p4 = f.post("alice", T0 + 2 * HOUR)
    c5 = f.comment("bob", p4, T0 + 3 * HOUR)
    ops = [f.opinion(w, p5, t=T0 + 4 * HOUR, karma="down") for w in voters]
    ops += [f.opinion(w, p4, t=T0 + 4 * HOUR, karma="down") for w in voters[:5]]
    ops += [f.opinion(w, c5, t=T0 + 4 * HOUR, karma="strongDown") for w in voters]
    v = V("v9-collapse-threshold-default", f,
          [*mem(f, "alice", "bob", "sam", "tom"), *grant(f, "alice", "bob", "sam", "tom"), p5, p4, c5, *ops],
          "collapseBelow = −5 (default). Six seeds with KP = 0 downvote p5 (power −1 each): score = self 1 − 6 "
          "= −5 ≤ −5, Collapsed(LowScore); five downvote p4: −4, Visible; six strongDown bob's comment: "
          "−strong(0) = −1 each, 1 − 6 = −5, Collapsed(LowScore)")
    view = fold_vector_input(v["input"])
    assert [view.l35[u(x)].score for x in (p5, p4, c5)] == [-5, -4, -5]
    assert [S(v, x) for x in (p5, p4, c5)] == [col("LowScore"), VIS, col("LowScore")]
    out.append(v)

    f = F(membership="approval", display={"collapseBelow": 0})
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    nul = f.modaction("mona", "restrict", did("alice"), nullifyVotes=True, t=T0 + 3 * HOUR)
    v = V("nullified-participant-scores-0", f, [*mem(f, "alice", "bob"), *grant(f, "alice", "bob"), pa, pb, nul],
          "approval mode, collapseBelow = 0: alice is granted but nullified, so she participates but is "
          "outside S and E; her post scores self(i) = 0 and is Collapsed(LowScore) (V8 is open-mode only); "
          "bob's scores 1")
    assert S(v, pa) == col("LowScore") and S(v, pb) == VIS
    out.append(v)

    # ------------------------------------------------------------------ newcomer collapse (V8)
    f = F()
    names = ("alice", "bob", "carol", "dave", "erin")
    posts = {n: f.post(n, T0 + i * HOUR) for i, n in enumerate(names, 1)}
    host = f.post("owner", T0 + 6 * HOUR)
    comments = {n: f.comment(n, host, T0 + (7 + i) * HOUR) for i, n in enumerate(names)}
    ups = [f.opinion(w, posts["bob"], t=T0 + 12 * HOUR, karma="up") for w in ("owner", "adam")]
    ups += [f.opinion("owner", posts["carol"], t=T0 + 12 * HOUR, karma="up")]
    ban = f.modaction("mona", "access", did("erin"), level="banned", t=T0 + 12 * HOUR)
    v = V("newcomer-collapse", f,
          [*mem(f, *names), *grant(f, "alice"), *posts.values(), host, *comments.values(), *ups, ban],
          "open mode, newUsers = collapse: alice is granted (a seed); bob is established (two seeds upvoted "
          "him: K1 = 2 ≥ establish 1, seedUp = 2 ≥ establishSeeds 2); carol has one seed upvote (seedUp 1 "
          "< 2) and dave none, so both are newcomers, Collapsed(NewUser); erin is banned (V4 first)")
    view = fold_vector_input(v["input"])
    assert did("bob") in view.l3.E and did("carol") not in view.l3.E
    want = {"alice": VIS, "bob": VIS, "carol": col("NewUser"), "dave": col("NewUser"), "erin": hid("Banned")}
    assert all(S(v, posts[n]) == want[n] and S(v, comments[n]) == want[n] for n in names)
    out.append(v)

    # ------------------------------------------------------------------ item kinds
    f = F()
    s1 = f.sequence("carol", T0 + HOUR)
    s2 = f.sequence("dave", T0 + 2 * HOUR)
    w1 = f.wiki("carol", T0 + 3 * HOUR, "epistemics")
    w2 = f.wiki("dave", T0 + 4 * HOUR, "decision-theory")
    w3 = f.wiki("alice", T0 + 5 * HOUR, "rationality")
    rst = f.modaction("mona", "restrict", did("carol"), capabilities=["wiki"], t=T0 + 6 * HOUR)
    h = f.modaction("mike", "hide", s2, t=T0 + 6 * HOUR)
    v = V("sequences-and-wiki-revisions", f,
          [*mem(f, "alice", "carol", "dave"), *grant(f, "alice", "carol"), s1, s2, w1, w2, w3, rst, h],
          "the rules apply to every candidate item: carol (granted) is restricted from `wiki`, which hides "
          "her wikiRevision but not her sequence; mike hid dave's sequence; dave's wikiRevision is a "
          "newcomer's (Collapsed(NewUser)); V6 and V7 concern comments only")
    assert [S(v, x) for x in (s1, s2, w1, w2, w3)] == [VIS, removed(1, "mike"), hid("Restricted"),
                                                      col("NewUser"), VIS]
    out.append(v)

    # ------------------------------------------------------------------ thread pass (§10.2)
    f = F()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("bob", p, T0 + 2 * HOUR)
    c2 = f.comment("carol", p, T0 + 3 * HOUR, parent=c1)
    c3 = f.comment("dave", p, T0 + 4 * HOUR)
    h1 = f.modaction("mona", "hide", c1, reason="personal attack", showReason=False, t=T0 + 5 * HOUR)
    h3 = f.modaction("mona", "hide", c3, reason="spam link", spam=True, t=T0 + 5 * HOUR)
    v = V("tombstone-for-a-hidden-comment-with-a-visible-reply", f,
          [*mem(f, "alice", "bob", "carol", "dave"), *grant(f, "alice", "bob", "carol", "dave"), p, c1, c2, c3, h1,
           h3],
          "c1 is Removed and has a listed reply, so the thread pass makes it Tombstone(Removed), keeping the "
          "removal details (its reason stays hidden: showReason = false); c3 is Removed with no reply and "
          "stays Hidden (not displayed)")
    assert S(v, c1) == removed(1, "mona") and D(v, c1) == removed(1, "mona", state="Tombstone")
    assert D(v, c3) == removed(1, "mona", spam=True, reason="spam link") and D(v, c2) == VIS
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("bob", p, T0 + 2 * HOUR)
    c2 = f.comment("carol", p, T0 + 3 * HOUR, parent=c1)
    c3 = f.comment("dave", p, T0 + 4 * HOUR, parent=c2)
    c4 = f.comment("bob", p, T0 + 5 * HOUR)
    c5 = f.comment("carol", p, T0 + 6 * HOUR, parent=c4)
    hs = [f.modaction("mona", "hide", c, t=T0 + 7 * HOUR) for c in (c1, c2, c4, c5)]
    v = V("tombstones-along-a-chain", f,
          [*mem(f, "alice", "bob", "carol", "dave"), *grant(f, "alice", "bob", "carol", "dave"), p, c1, c2, c3, c4,
           c5, *hs],
          "c1 → c2 → c3 with c1 and c2 hidden: c3 is a listed descendant of both, so both become "
          "Tombstones; c4 → c5 are both hidden with no listed descendant and stay Hidden")
    assert [D(v, c)["state"] for c in (c1, c2, c3, c4, c5)] == ["Tombstone", "Tombstone", "Visible", "Hidden",
                                                                "Hidden"]
    out.append(v)

    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    c1 = f.comment("bob", p1, T0 + 3 * HOUR)
    h1 = f.modaction("mona", "hide", p1, reason="wrong forum", t=T0 + 4 * HOUR)
    h2 = f.modaction("mona", "hide", p2, t=T0 + 4 * HOUR)
    v = V("tombstone-root-post", f, [*mem(f, "alice", "bob"), *grant(f, "alice", "bob"), p1, p2, c1, h1, h2],
          "a hidden post with a listed comment is the root of the placed tree and becomes "
          "Tombstone(Removed) (it stays unlisted); a hidden post without comments stays Hidden")
    assert D(v, p1) == removed(1, "mona", reason="wrong forum", state="Tombstone")
    assert D(v, p2) == removed(1, "mona") and D(v, c1) == VIS
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("bob", p, T0 + 2 * HOUR)
    c2 = f.comment("erin", p, T0 + 3 * HOUR, parent=c1)
    c3 = f.comment("bob", p, T0 + 4 * HOUR)
    c4 = f.comment("carol", p, T0 + 5 * HOUR, parent=c3)
    c4 = rec(c4["uri"], {k: x for k, x in c4["value"].items() if k != "body"})
    hs = [f.modaction("mona", "hide", c, t=T0 + 6 * HOUR) for c in (c1, c3)]
    v = V("collapsed-reply-keeps-a-tombstone-excluded-does-not", f,
          [*mem(f, "alice", "bob", "carol", "erin"), *grant(f, "alice", "bob", "carol"), p, c1, c2, c3, c4, *hs],
          "listed means Visible or Collapsed: erin's newcomer reply (Collapsed(NewUser)) keeps c1 as a "
          "Tombstone; carol's reply to c3 is invalid (G2: no body), so it is Excluded, is not a node of "
          "the placed tree, and c3 stays Hidden", invalid={u(c4): "G2"})
    assert D(v, c1)["state"] == "Tombstone" and D(v, c2) == col("NewUser")
    assert D(v, c3)["state"] == "Hidden" and D(v, c4) == EXC
    out.append(v)

    f = F(gates=GATES_0, display={"newUsers": "hide"})
    p = f.post("alice", T0 + HOUR)
    q = f.post("alice", T0 + 2 * HOUR)
    cb = f.comment("bob", p, T0 + 3 * HOUR)
    cb2 = f.comment("carol", p, T0 + 4 * HOUR, parent=cb)
    ca = f.comment("dave", p, T0 + 5 * HOUR)
    ca2 = f.comment("carol", p, T0 + 6 * HOUR, parent=ca)
    cn = f.comment("erin", p, T0 + 7 * HOUR)
    cn2 = f.comment("carol", p, T0 + 8 * HOUR, parent=cn)
    ql = f.comment("dave", q, T0 + 9 * HOUR)
    ql2 = f.comment("carol", q, T0 + 10 * HOUR, parent=ql)
    ban = f.modaction("mona", "access", did("bob"), level="banned", t=T0 + 11 * HOUR)
    ah = f.modaction("alice", "hide", ca, scope="author", t=T0 + 11 * HOUR)
    lk = f.modaction("mike", "lock", q, allowed=[ql2["uri"]], t=T0 + 11 * HOUR)
    v = V("tombstone-keeps-the-reason", f,
          [*mem(f, "alice", "bob", "carol", "dave", "erin"), *grant(f, "alice", "carol", "dave"), p, q, cb, cb2,
           ca, ca2, cn, cn2, ql, ql2, ban, ah, lk],
          "every Hidden reason becomes a Tombstone with the same reason when a reply is listed: bob is "
          "banned, dave's comment under p is author-hidden by alice, erin is a newcomer under newUsers = "
          "hide, and dave's comment under q is outside the forum lock's allowed list while carol's reply "
          "is in it")
    assert [D(v, x) for x in (cb, ca, cn, ql)] == [tomb("Banned"), tomb("AuthorModerated"), tomb("NewUser"),
                                                   tomb("Locked")]
    assert all(D(v, x) == VIS for x in (cb2, ca2, cn2, ql2))
    out.append(v)

    f = F()
    ghost = f.post("alice", T0 + HOUR)
    gone = f.comment_uri("bob", ghost["uri"], T0 + 2 * HOUR, 5)
    c1 = f.comment("carol", ghost, T0 + 3 * HOUR, parent=(gone, ghost["cid"]))
    c2 = f.comment("dave", ghost, T0 + 4 * HOUR, parent=c1)
    h = f.modaction("mona", "hide", c1, t=T0 + 5 * HOUR)
    v = V("tombstone-under-missing-placeholders", f,
          [*mem(f, "carol", "dave"), *grant(f, "carol", "dave"), c1, c2, h],
          "the root post is not in records (a Missing root) and c1's parent is missing (a Missing "
          "placeholder): placeholders are not items, but the thread pass still runs over the placed tree, "
          "so the hidden c1 with a listed reply becomes a Tombstone")
    assert D(v, c1)["state"] == "Tombstone" and D(v, c2) == VIS
    out.append(v)

    out += m3_visibility_vectors()
    return out


def m3_visibility_vectors() -> list:
    """Cases from the M3 fold reviews and the CO5 decision (social.md §10.2, §20.1 item 51)."""
    out = []

    def m(k):
        return T0 + k * MIN

    # CO5 compares only a valid root (social.md §20.1 item 51)
    f = F(display={"newUsers": "collapse"})
    p = in_other_forum(f, f.post("xavier", m(10)))
    c4 = f.comment("maria", p, m(20))
    c2 = f.comment("mona", p, m(21))
    ups = [f.opinion("owner", c4, t=m(30), karma="up"), f.opinion("adam", c2, t=m(30), karma="up")]
    v = V("co5-invalid-root-is-a-missing-root", f, [*mem(f, "xavier", "maria"), *grant(f, "maria"), p, c4, c2, *ups],
          "open mode, newUsers = collapse. xavier (a membership only) wrote the root post with a forum field "
          "naming another forum, as an edit of the post could (invalid: G5, the G9 part). CO5 compares only "
          "a valid root (social.md §20.1 item 51), so the post is Excluded but the comments are not: they "
          "sit under a Missing root and keep their own states (maria is granted, mona is a moderator)",
          invalid={u(p): "G5"})
    assert [S(v, x) for x in (p, c4, c2)] == [EXC, VIS, VIS]
    view = fold_vector_input(v["input"])
    assert view.threads[u(p)].root_missing
    out.append(v)

    # A hidden root post becomes a Tombstone through a Missing placeholder (§10.2)
    f = F(membership="approval")
    p = f.post("adam", m(10))
    gone = f.comment_uri("zoe", u(p), m(15), 9)
    c = f.comment("mona", p, m(20), parent=(gone, p["cid"]))
    hide = f.modaction("owner", "hide", p, t=m(30))
    v = V("hidden-root-tombstone-through-placeholder", f, [p, c, hide],
          "approval mode, authorities only. The owner hid adam's post; mona's comment names a parent comment "
          "that is no record, so it sits under a Missing placeholder (not an item). The thread pass walks "
          "through the placeholder: the post has a listed descendant and becomes Tombstone(Removed)")
    assert S(v, p) == removed(3, "owner") and D(v, p) == removed(3, "owner", state="Tombstone")
    assert S(v, c) == VIS and D(v, c) == VIS
    out.append(v)

    # A re-attached (cycle-cut) hidden comment with a listed child becomes a Tombstone (§10.2)
    f = F(membership="approval")
    p = f.post("owner", m(5))
    xa = f.comment_uri("adam", u(p), m(10), 1)
    uy = f.comment("mona", p, m(11), parent=(xa, p["cid"]), clock=1)
    ux = f.comment("adam", p, m(10), parent=uy, clock=1)
    hide = f.modaction("owner", "hide", uy, t=m(20))
    v = V("reattached-hidden-comment-tombstone", f, [p, ux, uy, hide],
          "adam's ux and mona's uy name each other as parent; uy (the greater uri) is re-attached to the root "
          "(Orphaned) and ux becomes its child. The owner hid uy; its listed child makes it a "
          "Tombstone(Removed)")
    view = fold_vector_input(v["input"])
    assert u(ux) == xa and view.threads[u(p)].orphaned == frozenset([u(uy)])
    assert D(v, uy) == removed(3, "owner", state="Tombstone") and D(v, ux) == VIS
    out.append(v)
    return out


FILES = [
    ("visibility/visibility.json", lambda: file_doc(
        "mod/1", "visibility",
        "social.md §10.1 rules V1–V10 (the first matching rule wins), collapse thresholds, newcomer "
        "collapse, and the §10.2 thread pass (Tombstone(reason) for a Hidden post or comment with a "
        "listed descendant in the placed tree). input: {fold: FoldInput}. expected: states (V1–V10, "
        "before the pass) and displayed (after the pass), {uri: State} for every candidate item "
        "(testdata/vectors/README.md, social section). The lens (§10.3, L7) is local and never "
        "asserted.",
        visibility_vectors())),
]
