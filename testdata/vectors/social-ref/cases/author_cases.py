# SPDX-License-Identifier: CC0-1.0
"""modfold/author.json (kind "modfold"): author-tier moderation, social.md §4.9 (L5).

The actor of an author-scope record must be in E; with no counting opinions E = S, so the
post authors here are granted (`#access member`) unless a vector says otherwise. K = 0 for
everyone in the vectors without opinions, so they set the author gates explicitly (0 passes,
1 fails); the vector `author-gate-49-50-personal-and-frontpage` uses the karma/1 defaults
(50 and 2000) with real karma from seed votes.
"""

from __future__ import annotations

from lessdugin_ref.build import DAY, HOUR, MIN, NOW, T0, Forum, did, dt, rec
from lessdugin_ref.common import split_uri
from lessdugin_ref.fold import fold_vector_input

from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))
GATES_0 = {"authorModPersonal": 0, "authorModFrontpage": 0}


def F(gates=GATES_0, policy=None, **kw) -> Forum:
    kw.setdefault("moderators", MODS)
    kw.setdefault("membership", "open")
    pol = dict(policy or {})
    if gates is not None:
        pol["gates"] = dict(gates)
    return Forum(policy=pol or None, **kw)


def _view(v):
    """Re-fold a vector's input (for hand checks of values the modfold kind does not project)."""
    return fold_vector_input(v["input"])


def V(name, f, records, note, *, extra=(), fields=None, invalid=None, fi=None):
    return vector("modfold", name, f.fold_input(records, **(fi or {})), fields=fields,
                  extra_fields=extra, note=note, invalid=invalid)


def u(r):
    return r if isinstance(r, str) else r["uri"]


def mem(f, *names):
    return [f.membership(n) for n in names]


def grant(f, *names):
    return [f.modaction("owner", "access", did(n), level="member", t=T0 + MIN) for n in names]


def am(f, actor, action, subject, **kw):
    """An author-scope modAction."""
    return f.modaction(actor, action, subject, scope="author", **kw)


def hidden(reason):
    return {"state": "Hidden", "reason": reason}


AM = hidden("AuthorModerated")


def removed(rank, actor, spam=False, reason=None):
    return {"state": "Hidden", "reason": "Removed",
            "removed": {"rank": rank, "actor": did(actor), "spam": spam, "reason": reason}}


def st(v, r):
    return v["expected"]["modStates"][u(r)]


def status(v, r):
    for e in v["expected"]["modLog"]:
        if e["uri"] == u(r):
            return (e["status"], e.get("rank"))
    return None


def edited(r, t, **changes):
    value = dict(r["value"])
    value.update(changes)
    value["updatedAt"] = dt(t)
    return rec(r["uri"], value)


def author_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ the basic effect
    f = F()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("carol", p, T0 + 2 * HOUR)
    c2 = f.comment("dave", p, T0 + 3 * HOUR)
    h = am(f, "alice", "hide", c1, spam=True, reason="off-topic", t=T0 + 4 * HOUR)
    v = V("author-hide-in-own-thread", f, [*mem(f, "alice", "carol", "dave"), *grant(f, "alice"), p, c1,
                                           c2, h],
          "alice (granted, so in E = S) author-hides carol's comment in her own thread (personal post; "
          "authorModPersonal = 0 ≤ K = 0): Hidden(AuthorModerated) (V6). Author-tier values act at rank 0 "
          "and never enter forum-tier resolution, so `resolved` has nothing for the comment and the "
          "record's spam = true is ignored (data-model.md §6.7)")
    assert st(v, c1) == AM and st(v, c2) is None and st(v, p) is None
    assert u(c1) not in v["expected"]["resolved"] and status(v, h) == ("effective", None)
    out.append(v)

    f = F()
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    ca = f.comment("dave", pa, T0 + 3 * HOUR)
    cb = f.comment("dave", pb, T0 + 4 * HOUR)
    h1 = am(f, "alice", "hide", cb, t=T0 + 5 * HOUR)
    h2 = am(f, "bob", "hide", ca, t=T0 + 5 * HOUR)
    h3 = am(f, "carol", "hide", ca, t=T0 + 6 * HOUR)
    v = V("author-hide-only-in-own-thread", f,
          [*mem(f, "alice", "bob", "carol", "dave"), *grant(f, "alice", "bob", "carol"), pa, pb, ca, cb, h1,
           h2, h3],
          "the table condition author(root(c)) = X fails for a hide in someone else's thread: alice's "
          "hide of a comment under bob's post, and bob's and carol's hides under alice's post, are "
          "unauthorised and have no effect")
    assert st(v, ca) is None and st(v, cb) is None
    assert all(status(v, r) == ("unauthorised", None) for r in (h1, h2, h3))
    out.append(v)

    # ------------------------------------------------------------------ gates (authorGate)
    for name, gates, ok_personal in (
            ("author-gate-personal-0-frontpage-1-at-K-0",
             {"authorModPersonal": 0, "authorModFrontpage": 1}, True),
            ("author-gate-personal-1-frontpage-0-at-K-0",
             {"authorModPersonal": 1, "authorModFrontpage": 0}, False)):
        f = F(gates=gates)
        p1 = f.post("alice", T0 + HOUR)
        p2 = f.post("alice", T0 + 2 * HOUR)
        p3 = f.post("alice", T0 + 3 * HOUR)
        t2 = f.modaction("mona", "tier", p2, tier="frontpage", t=T0 + 4 * HOUR)
        t3 = f.modaction("adam", "tier", p3, tier="curated", t=T0 + 4 * HOUR)
        cs = [f.comment("carol", p, T0 + (5 + i) * HOUR) for i, p in enumerate((p1, p2, p3))]
        hs = [am(f, "alice", "hide", c, t=T0 + 9 * HOUR) for c in cs]
        v = V(name, f, [*mem(f, "alice", "carol"), *grant(f, "alice"), p1, p2, p3, t2, t3, *cs, *hs],
              f"authorGate(X, p) ⇔ X ∈ A ∨ K[X] ≥ the gate of p's effective tier: authorModPersonal "
              f"({gates['authorModPersonal']}) for personal p1, authorModFrontpage "
              f"({gates['authorModFrontpage']}) for frontpage p2 and curated p3. alice has K = 0 (no "
              f"opinions), so the hide works exactly where the gate is 0", extra=("tiers",))
        want = (AM if ok_personal else None, None if ok_personal else AM, None if ok_personal else AM)
        assert tuple(st(v, c) for c in cs) == want
        assert [status(v, x)[0] for x in hs] == ["effective" if s == AM else "unauthorised" for s in want]
        out.append(v)

    # The karma/1 default gates (authorModPersonal 50, authorModFrontpage 2000) with real karma:
    # five seeds with K1 = KP = 0 upvote each of alice's 10 items (K = 5 × 10 = 50; no pair cap
    # binds) and 49 of bob's 50 opinion slots (K = 49).
    f = F(gates=None)
    voters = ("owner", "adam", "mona", "mike", "sam")
    pa = f.post("alice", T0 + HOUR)
    pf = f.post("alice", T0 + 2 * HOUR)
    tf = f.modaction("mona", "tier", pf, tier="frontpage", t=T0 + 3 * HOUR)
    a_items = [pa, pf] + [f.comment("alice", pa, T0 + 4 * HOUR + i * MIN) for i in range(8)]
    pb = f.post("bob", T0 + 5 * HOUR)
    b_items = [pb] + [f.comment("bob", pb, T0 + 6 * HOUR + i * MIN) for i in range(9)]
    ops = [f.opinion(w, x, t=T0 + 7 * HOUR, karma="up") for w in voters for x in a_items]
    ops += [f.opinion(w, x, t=T0 + 7 * HOUR, karma="up") for w in voters for x in b_items
            if not (w == "sam" and x is pb)]
    ca = f.comment("carol", pa, T0 + 8 * HOUR)
    cf = f.comment("carol", pf, T0 + 8 * HOUR + MIN)
    cb = f.comment("carol", pb, T0 + 8 * HOUR + 2 * MIN)
    ha = am(f, "alice", "hide", ca, t=T0 + 9 * HOUR)
    hf = am(f, "alice", "hide", cf, t=T0 + 9 * HOUR)
    hb = am(f, "bob", "hide", cb, t=T0 + 9 * HOUR)
    recs = [*mem(f, "alice", "bob", "carol", "sam"), *grant(f, "alice", "bob", "sam"), tf, *a_items,
            *b_items, *ops, ca, cf, cb, ha, hf, hb]
    v = V("author-gate-49-50-personal-and-frontpage", f, recs,
          "the karma/1 default gates: five seeds (four authorities and granted sam, all with K1 = KP = 0, "
          "so every vote has power 1 and no pair cap binds) upvote all 10 of alice's items (K = 50) and "
          "49 of bob's 50 voter-item pairs (K = 49). alice's hide works under her personal post (K = 50 ≥ "
          "authorModPersonal 50) but not under her frontpage post (50 < authorModFrontpage 2000); bob's "
          "fails under his personal post (49 < 50)", extra=("tiers",))
    view = _view(v)
    assert view.l3.K.get(did("alice")) == 50 and view.l3.K.get(did("bob")) == 49
    assert [st(v, c) for c in (ca, cf, cb)] == [AM, None, None]
    assert [status(v, x)[0] for x in (ha, hf, hb)] == ["effective", "unauthorised", "unauthorised"]
    out.append(v)

    # ------------------------------------------------------------------ immunity (§4.9 "Other rules")
    f = F()
    p = f.post("alice", T0 + HOUR)
    cm = f.comment("mike", p, T0 + 2 * HOUR)
    ca = f.comment("adam", p, T0 + 3 * HOUR)
    co = f.comment("owner", p, T0 + 4 * HOUR)
    cc = f.comment("carol", p, T0 + 5 * HOUR)
    cd = f.comment("dave", p, T0 + 6 * HOUR)
    cl = f.comment("alice", p, T0 + 7 * HOUR)
    hm = am(f, "alice", "hide", cm, t=T0 + 8 * HOUR)
    ba = am(f, "alice", "access", did("adam"), level="banned", t=T0 + 8 * HOUR)
    bc = am(f, "alice", "access", did("carol"), level="banned", t=T0 + 8 * HOUR)
    lk = am(f, "alice", "lock", p, allowed=[], t=T0 + 8 * HOUR)
    v = V("author-tier-immunity-of-authorities", f,
          [*mem(f, "alice", "carol", "dave"), *grant(f, "alice"), p, cm, ca, co, cc, cd, cl, hm, ba, bc, lk],
          "authorities are immune to author-tier hides, bans and locks: alice's hide of mike's comment and "
          "her ban of adam are unauthorised; her lock (allowed = []) leaves the comments of mike, adam and "
          "the owner, and her own comment, visible, and locks dave out; carol is author-banned")
    assert [st(v, c) for c in (cm, ca, co, cc, cd, cl)] == [None, None, None, AM, AM, None]
    assert status(v, hm) == ("unauthorised", None) and status(v, ba) == ("unauthorised", None)
    assert status(v, bc) == ("effective", None) and status(v, lk) == ("effective", None)
    out.append(v)

    # ------------------------------------------------------------------ precedence (§4.9)
    f = F()
    p = f.post("alice", T0 + HOUR)
    c1, c2, c3 = (f.comment("carol", p, T0 + i * HOUR) for i in (2, 3, 4))
    c4, c5 = (f.comment("dave", p, T0 + i * HOUR) for i in (5, 6))
    h1, h2, h3 = (am(f, "alice", "hide", c, t=T0 + 7 * HOUR) for c in (c1, c2, c3))
    bd = am(f, "alice", "access", did("dave"), level="banned", t=T0 + 7 * HOUR)
    k1 = f.modaction("mona", "hide", c1, hidden=False, t=T0 + 8 * HOUR)
    x2 = f.modaction("mike", "hide", c2, reason="rule 2", t=T0 + 8 * HOUR)
    k4 = f.modaction("adam", "hide", c4, hidden=False, t=T0 + 8 * HOUR)
    v = V("forum-keep-beats-author-hide", f,
          [*mem(f, "alice", "carol", "dave"), *grant(f, "alice"), p, c1, c2, c3, c4, c5, h1, h2, h3, bd, k1,
           x2, k4],
          "AuthorModerated(c) needs resolve(c, hidden) = Default: mona's keep of c1 (rank 1) beats alice's "
          "hide, which shows overridden (rank 1); mike's forum hide of c2 governs (Removed, rank 1); c3 is "
          "author-hidden. alice's ban of dave applies to c5 but not to c4, which adam kept (keepRank > 0)")
    assert [st(v, c) for c in (c1, c2, c3, c4, c5)] == [None, removed(1, "mike", reason="rule 2"), AM, None, AM]
    assert status(v, h1) == ("overridden", 1) and status(v, h2) == ("overridden", 1)
    assert status(v, h3) == ("effective", None) and status(v, bd) == ("effective", None)
    out.append(v)

    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    c1 = f.comment("carol", p1, T0 + 3 * HOUR)
    c2 = f.comment("carol", p2, T0 + 4 * HOUR)
    l1 = am(f, "alice", "lock", p1, allowed=[], t=T0 + 5 * HOUR)
    l2 = am(f, "alice", "lock", p2, allowed=[], t=T0 + 5 * HOUR)
    un = f.modaction("mona", "lock", p1, locked=False, t=T0 + 6 * HOUR)
    v = V("forum-unlock-beats-author-lock", f,
          [*mem(f, "alice", "carol"), *grant(f, "alice"), p1, p2, c1, c2, l1, l2, un],
          "an author lock applies only if resolve(p, locked) = Default: mona's forum #lock{locked: false} on "
          "p1 (rank 1) governs, so carol's comment there stays visible and alice's lock shows overridden; "
          "on p2 alice's lock (allowed = []) locks carol out")
    assert st(v, c1) is None and st(v, c2) == AM
    assert v["expected"]["resolved"][u(p1)]["locked"]["value"] is False
    assert status(v, l1) == ("overridden", 1) and status(v, l2) == ("effective", None)
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    cc = f.comment("carol", p, T0 + 2 * HOUR)
    cl = f.comment("alice", p, T0 + 3 * HOUR)
    la = am(f, "alice", "lock", p, allowed=[cc["uri"]], t=T0 + 4 * HOUR)
    lf = f.modaction("mona", "lock", p, allowed=[], t=T0 + 5 * HOUR)
    v = V("forum-lock-governs-author-lock", f, [*mem(f, "alice", "carol"), *grant(f, "alice"), p, cc, cl, la, lf],
          "a forum-tier lock resolves locked = true at rank 1, so alice's author lock (whose allowed list "
          "names carol's comment) is ignored and shows overridden: V7 hides carol's comment and also "
          "alice's own, because the post's author is exempt only from her own author lock")
    assert st(v, cc) == hidden("Locked") and st(v, cl) == hidden("Locked")
    assert status(v, la) == ("overridden", 1)
    out.append(v)

    # ------------------------------------------------------------------ author who is also a moderator
    f = F(gates=None)
    p = f.post("mona", T0 + HOUR)
    c1 = f.comment("alice", p, T0 + 2 * HOUR)
    c2 = f.comment("bob", p, T0 + 3 * HOUR)
    c3 = f.comment("mike", p, T0 + 4 * HOUR)
    fh1 = f.modaction("mona", "hide", c1, reason="forum rule 1", t=T0 + 5 * HOUR)
    ah1 = am(f, "mona", "hide", c1, t=T0 + 5 * HOUR)
    ah2 = am(f, "mona", "hide", c2, t=T0 + 5 * HOUR)
    ah3 = am(f, "mona", "hide", c3, t=T0 + 5 * HOUR)
    fh3 = f.modaction("mona", "hide", c3, t=T0 + 5 * HOUR)
    v = V("author-who-is-also-a-moderator", f,
          [*mem(f, "alice", "bob"), p, c1, c2, c3, fh1, ah1, ah2, ah3, fh3],
          "mona (moderator) wrote the post and issues both scopes; scope is part of the key, so her forum "
          "and author hides of c1 are two records. The forum hide governs c1 (Removed, rank 1; the author "
          "hide is overridden). Her author hide of c2 works under the default gates (50, 2000) because "
          "authorities bypass every gate. On mike's comment both fail: the forum hide by the rank rule "
          "(equal rank, not the subject's author) and the author hide by immunity")
    assert split_uri(u(fh1))[2] != split_uri(u(ah1))[2]
    assert [st(v, c) for c in (c1, c2, c3)] == [removed(1, "mona", reason="forum rule 1"), AM, None]
    assert status(v, fh1) == ("effective", None) and status(v, ah1) == ("overridden", 1)
    assert status(v, ah2) == ("effective", None)
    assert status(v, ah3) == ("unauthorised", None) and status(v, fh3) == ("unauthorised", None)
    out.append(v)

    # ------------------------------------------------------------------ bans
    f = F(gates={"authorModPersonal": 0, "authorModFrontpage": 1})
    pa = f.post("alice", T0 + HOUR)
    pf = f.post("alice", T0 + 2 * HOUR)
    pb = f.post("bob", T0 + 3 * HOUR)
    tf = f.modaction("mona", "tier", pf, tier="frontpage", t=T0 + 4 * HOUR)
    ca = f.comment("carol", pa, T0 + 5 * HOUR)
    cf = f.comment("carol", pf, T0 + 6 * HOUR)
    cb = f.comment("carol", pb, T0 + 7 * HOUR)
    pc = f.post("carol", T0 + 8 * HOUR)
    ban = am(f, "alice", "access", did("carol"), level="banned", t=T0 + 9 * HOUR)
    v = V("author-ban-only-in-own-threads-gated-per-thread", f,
          [*mem(f, "alice", "bob", "carol"), *grant(f, "alice", "bob"), pa, pf, pb, tf, ca, cf, cb, pc, ban],
          "alice's author ban of carol hides carol's comments only in threads of posts p with author(p) = "
          "alice ∧ authorGate(alice, p): her personal post passes (gate 0), her frontpage post does not "
          "(gate 1 > K = 0), bob's thread is not hers; carol's own post is untouched (author bans act on "
          "comments)", extra=("B",))
    assert [st(v, x) for x in (ca, cf, cb, pc)] == [AM, None, None, None]
    assert v["expected"]["B"] == [] and status(v, ban) == ("effective", None)
    out.append(v)

    f = F(membership="approval")
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("carol", p, T0 + 2 * HOUR)
    c2 = f.comment("dave", p, T0 + 3 * HOUR)
    pc = f.post("carol", T0 + 4 * HOUR)
    bc = am(f, "alice", "access", did("carol"), level="banned", t=T0 + 5 * HOUR)
    bd = am(f, "alice", "access", did("dave"), level="banned", t=T0 + 5 * HOUR)
    k2 = f.modaction("mona", "hide", c2, hidden=False, t=T0 + 6 * HOUR)
    gs = grant(f, "alice", "carol", "dave")
    v = V("author-ban-of-a-granted-user-in-approval-mode", f,
          [*mem(f, "alice", "carol", "dave"), *gs, p, c1, c2, pc, bc, bd, k2],
          "approval mode: carol and dave are granted (the owner's #access{member}, resolved at rank 3), as "
          "every non-authority participant there must be. alice's author bans still apply: a forum-tier "
          "#access value on the banned DID does not lift an author ban, which is judged per comment and "
          "yields only to a forum-tier keep (keepRank(c) = 0) or any forum-tier hidden value. carol's "
          "comment is author-banned; dave's is kept by mona (rank 1); carol's own post is untouched. Both "
          "bans are effective, never overridden (NOTES.md 46)", extra=("G", "S"))
    assert [st(v, x) for x in (c1, c2, pc)] == [AM, None, None]
    assert v["expected"]["resolved"][did("carol")]["access"]["value"] == "member"
    assert v["expected"]["resolved"][did("carol")]["access"]["rank"] == 3
    assert did("carol") in v["expected"]["G"] and did("carol") in v["expected"]["S"]
    assert status(v, bc) == ("effective", None) and status(v, bd) == ("effective", None)
    out.append(v)

    # ------------------------------------------------------------------ pins
    f = F()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("carol", p, T0 + 2 * HOUR)
    c2 = f.comment("dave", p, T0 + 3 * HOUR)
    c3_v1 = f.comment("erin", p, T0 + 4 * HOUR)
    p1 = am(f, "alice", "pin", c1, pin_scope="thread", t=T0 + 5 * HOUR)
    p2 = am(f, "alice", "pin", c2, pin_scope="thread", t=T0 + 5 * HOUR)
    p3 = am(f, "alice", "pin", c3_v1, pin_scope="thread", t=T0 + 5 * HOUR)
    c3 = edited(c3_v1, T0 + 6 * HOUR, body={**c3_v1["value"]["body"], "text": "Edited"})
    u2 = f.modaction("mona", "pin", c2, pin_scope="thread", pinned=False, t=T0 + 7 * HOUR)
    xp = am(f, "carol", "pin", c1, pin_scope="thread", t=T0 + 7 * HOUR)
    v = V("author-thread-pins", f,
          [*mem(f, "alice", "carol", "dave", "erin"), *grant(f, "alice", "carol"), p, c1, c2, c3, p1, p2, p3,
           u2, xp],
          "alice pins comments in her thread (author scope, CID-bound): c1's pin is effective; mona's "
          "forum unpin of c2 (rank 1) governs, so alice's pin of c2 is overridden; erin edited c3 after the "
          "pin, so it is stale; carol's pin is unauthorised (not her thread). Pins do not change states; "
          "rank/rank.json shows their effect on comment order")
    assert [status(v, x) for x in (p1, p2, p3, xp)] == [("effective", None), ("overridden", 1), ("stale", None),
                                                        ("unauthorised", None)]
    assert v["expected"]["resolved"][u(c2)]["pin.thread"]["value"] is False
    out.append(v)

    # ------------------------------------------------------------------ authorEffective conditions
    f = F(policy={"authorModeration": False})
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("carol", p, T0 + 2 * HOUR)
    c2 = f.comment("dave", p, T0 + 3 * HOUR)
    xs = [am(f, "alice", "hide", c1, t=T0 + 4 * HOUR),
          am(f, "alice", "access", did("dave"), level="banned", t=T0 + 4 * HOUR),
          am(f, "alice", "lock", p, t=T0 + 4 * HOUR),
          am(f, "alice", "pin", c2, pin_scope="thread", t=T0 + 4 * HOUR)]
    v = V("author-moderation-disabled", f, [*mem(f, "alice", "carol", "dave"), *grant(f, "alice"), p, c1, c2, *xs],
          "policy.authorModeration = false: no author-scope record has an effect; the mod log shows them "
          "unauthorised")
    assert st(v, c1) is None and st(v, c2) is None
    assert all(status(v, x) == ("unauthorised", None) for x in xs)
    out.append(v)

    f = F()
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    ca = f.comment("carol", pa, T0 + 3 * HOUR)
    cb = f.comment("carol", pb, T0 + 4 * HOUR)
    ha = am(f, "alice", "hide", ca, t=T0 + 5 * HOUR)
    hb = am(f, "bob", "hide", cb, t=T0 + 5 * HOUR)
    v = V("author-actor-must-be-in-E", f, [*mem(f, "alice", "bob", "carol"), *grant(f, "bob"), pa, pb, ca, cb, ha, hb],
          "authorEffective needs X ∈ E: alice is a participant (open mode) but neither a seed nor "
          "established, so her hide is unauthorised; bob is granted (a seed), so his works",
          extra=("S", "newcomers"))
    assert st(v, ca) is None and st(v, cb) == AM
    assert status(v, ha) == ("unauthorised", None) and status(v, hb) == ("effective", None)
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    c1, c2, c3, c4 = (f.comment(n, p, T0 + i * HOUR) for i, n in ((2, "carol"), (3, "dave"), (4, "erin"),
                                                                    (5, "fred")))
    h1 = am(f, "alice", "hide", c1, until=NOW + 1, t=T0 + 6 * HOUR)
    h2 = am(f, "alice", "hide", c2, until=NOW, t=T0 + 6 * HOUR)
    h3 = am(f, "carol", "hide", c3, until=NOW - DAY, t=T0 + 6 * HOUR)
    pin = am(f, "alice", "pin", c4, pin_scope="thread", subject_cid=c1["cid"], t=T0 + 6 * HOUR)
    v = V("author-until-and-cid-binding", f,
          [*mem(f, "alice", "carol", "dave", "erin", "fred"), *grant(f, "alice", "carol"), p, c1, c2, c3, c4, h1,
           h2, h3, pin],
          "until: a hide counts while µs(until) > now (now + 1 µs: effective; now: expired). carol's hide "
          "is both unauthorised (not her thread) and expired: authorisation is checked first. A positive "
          "author pin whose subjectCid is not the comment's current CID is stale")
    assert [st(v, c) for c in (c1, c2, c3, c4)] == [AM, None, None, None]
    assert [status(v, x)[0] for x in (h1, h2, h3, pin)] == ["effective", "expired", "unauthorised", "stale"]
    out.append(v)

    # ------------------------------------------------------------------ author lock and allowed
    f = F()
    p = f.post("alice", T0 + HOUR)
    q = f.post("alice", T0 + 2 * HOUR)
    c1 = f.comment("carol", p, T0 + 3 * HOUR)
    c2 = f.comment("dave", p, T0 + 4 * HOUR)
    c3 = f.comment("alice", p, T0 + 5 * HOUR)
    c4 = f.comment("mike", p, T0 + 6 * HOUR)
    c5 = f.comment("erin", p, T0 + 7 * HOUR)
    cq = f.comment("dave", q, T0 + 8 * HOUR)
    other = f.comment("carol", q, T0 + 9 * HOUR)
    lk = am(f, "alice", "lock", p, allowed=sorted([c1["uri"], other["uri"]], key=str.encode), t=T0 + 10 * HOUR)
    k5 = f.modaction("mona", "hide", c5, hidden=False, t=T0 + 11 * HOUR)
    un = am(f, "alice", "lock", q, locked=False, t=T0 + 10 * HOUR)
    v = V("author-lock-with-allowed", f,
          [*mem(f, "alice", "carol", "dave", "erin"), *grant(f, "alice"), p, q, c1, c2, c3, c4, c5, cq, other,
           lk, k5, un],
          "alice's author lock of p keeps the comments in its allowed list (carol's c1; the entry naming a "
          "comment of q is outside p's thread and ignored), her own comment (author(c) ≠ X), the "
          "authority's, and the one mona kept (resolve(c, hidden) ≠ Default); dave's comment is locked "
          "out. Her #lock{locked: false} on q is effective and locks nothing")
    assert [st(v, c) for c in (c1, c2, c3, c4, c5, cq, other)] == [None, AM, None, None, None, None, None]
    assert status(v, lk) == ("effective", None) and status(v, un) == ("effective", None)
    out.append(v)

    # ------------------------------------------------------------------ root(c) must exist
    f = F()
    ghost = f.post("alice", T0 + HOUR)
    bad = f.post("alice", T0 + 2 * HOUR, title=" ")
    c1 = f.comment("carol", ghost, T0 + 3 * HOUR)
    c2 = f.comment("carol", bad, T0 + 4 * HOUR)
    h1 = am(f, "alice", "hide", c1, t=T0 + 5 * HOUR)
    h2 = am(f, "alice", "hide", c2, t=T0 + 5 * HOUR)
    lk = am(f, "alice", "lock", ghost, t=T0 + 5 * HOUR)
    v = V("author-tier-needs-the-root-post", f,
          [*mem(f, "alice", "carol"), *grant(f, "alice"), bad, c1, c2, h1, h2, lk],
          "root(c) = records[c.root.uri] must exist for any author-tier effect on c: alice's first post is "
          "not in records and her second is invalid (PO3: its title is only U+2003 EM SPACE), which the "
          "model treats as missing (V1 excludes it; NOTES.md, Author tier). Her hides and her lock of the "
          "missing post are unauthorised and both comments stay visible",
          invalid={u(bad): "PO3"})
    assert st(v, c1) is None and st(v, c2) is None and st(v, bad) == {"state": "Excluded"}
    assert all(status(v, x) == ("unauthorised", None) for x in (h1, h2, lk))
    out.append(v)

    # ------------------------------------------------------------------ outside the author-scope table
    f = F()
    p = f.post("alice", T0 + HOUR)
    c = f.comment("carol", p, T0 + 2 * HOUR)
    bad = [am(f, "alice", "hide", c, hidden=False),
           am(f, "alice", "hide", p),
           am(f, "alice", "access", did("carol"), level="member"),
           am(f, "alice", "pin", p, pin_scope="forum"),
           am(f, "alice", "restrict", did("carol"), capabilities=["comment"]),
           am(f, "alice", "tier", p, tier="personal"),
           am(f, "alice", "tag", "tag:epistemics", removed=True)]
    fl = am(f, "alice", "lock", f.uri)
    v = V("author-scope-outside-the-table", f, [*mem(f, "alice", "carol"), *grant(f, "alice"), p, c, *bad, fl],
          "MA6: an author-scope keep, a hide of a post, #access{member}, a forum pin, #restrict, #tier and "
          "#tag are invalid (excluded from the fold and the mod log); an author-scope #lock of the forum "
          "uri is valid but reserved (MA9: reserved/unknown)",
          invalid={u(x): "MA6" for x in bad})
    assert st(v, c) is None and status(v, fl) == ("reserved/unknown", None)
    assert all(status(v, x) is None for x in bad)
    out.append(v)

    return out


FILES = [
    ("modfold/author.json", lambda: file_doc(
        "mod/1", "modfold",
        "social.md §4.9 author-tier moderation (L5): authorEffective (authorModeration, X ∈ E, until, "
        "CID binding, the table condition), authorGate per post tier, immunity of authorities, the "
        "precedence of the forum tier (keeps, hides, unlocks, locks, thread pins), author bans per "
        "thread, author locks with allowed, and the mod-log status of author-scope records. input: "
        "{fold: FoldInput}. expected: the modfold projection fields listed "
        "(testdata/vectors/README.md, social section); V6 appears in modStates as "
        "Hidden(AuthorModerated).",
        author_vectors())),
]
