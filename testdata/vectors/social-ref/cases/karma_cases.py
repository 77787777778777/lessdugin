# SPDX-License-Identifier: CC0-1.0
"""karma/ (behaviour karma/1): social.md §5–§8.

    karma/power.json    kind "power": §6.1 power(k, x) at every ladder boundary
    karma/staged.json   kind "karma": §5 opinion validity and the §6.2 stages (§19 karma cases)
    karma/scores.json   kind "karma": §7 score, agreement, unweighted, earlier versions
    karma/gates.json    kind "karma": §8 gate values and passes(u, g)

Every expectation below is derived by hand in the comments and asserted against the model.
Authorities are seeds (S = (A ∪ G) ∖ (B ∪ N)) whose K1, KP and K are 0 unless a vector says
otherwise, so each of their votes has power 1 in every stage.
"""

from __future__ import annotations

from lessdugin_ref.build import DAY, MIN, T0, Forum, at, did, dt, rec
from lessdugin_ref.common import h16, rkey_of
from lessdugin_ref.karma import LADDER, power, small, strong
from lessdugin_ref.scores import self_score
from lessdugin_ref.fold import fold_vector_input

from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))
A = ("owner", "adam", "mona", "mike")
STAGED = ("counting", "K1", "seedUp", "E", "established", "KP", "K")
KARMA_VALUES = ("up", "down", "strongUp", "strongDown")
AGREEMENT_VALUES = ("agree", "disagree", "strongAgree", "strongDisagree")


def F(**kw) -> Forum:
    kw.setdefault("moderators", MODS)
    kw.setdefault("membership", "open")
    return Forum(**kw)


def u(r):
    return r["uri"]


def ds(*names) -> list:
    return sorted({did(n) for n in names}, key=str.encode)


def km(**kv) -> dict:
    """{did(name): value} without zero entries (the projection lists non-zero entries only)."""
    return {did(n): v for n, v in sorted(kv.items(), key=lambda e: did(e[0]).encode()) if v != 0}


def us(*recs) -> list:
    return sorted((u(r) for r in recs), key=str.encode)


def mem(f, *names):
    return [f.membership(n) for n in names]


def grant(f, *names, by="owner"):
    return [f.modaction(by, "access", did(n), level="member") for n in names]


class Clock:
    """Distinct creation times for the items of one vector."""

    def __init__(self):
        self.t = T0

    def __call__(self) -> int:
        self.t += MIN
        return self.t


def posts(f, clk, who, n, **kw):
    return [f.post(who, clk(), **kw) for _ in range(n)]


def votes(f, voter, subjects, karma="up", **axes):
    return [f.opinion(voter, s, karma=karma, **axes) for s in subjects]


def V(name, f, records, note, *, fields=STAGED, extra=(), invalid=None, fi=None, expect=None):
    return vector("karma", name, f.fold_input(records, **(fi or {})), fields=fields, extra_fields=extra,
                  note=note, invalid=invalid, expect=expect)


# =========================================================================== power.json

def ks() -> list:
    out = [-5, 0]
    for t, _ in sorted(LADDER):
        out += [t - 1, t]
    out += [10 ** 9]
    return out


def power_vectors() -> list:
    out = []

    def vec(k, x, note=None):
        e = {"name": f"k{k}-{x}", "input": {"k": k, "x": x}, "expected": {"power": power(k, x)}}
        if note:
            e["note"] = note
        out.append(e)

    # hand-checked rows: small(k) = 2 iff k ≥ 1000; strong(k) from the ladder; k' = max(k, 0)
    hand = {-5: (1, 1), 0: (1, 1), 9: (1, 1), 10: (1, 2), 99: (1, 2), 100: (1, 3), 249: (1, 3),
            250: (1, 4), 499: (1, 4), 500: (1, 5), 999: (1, 5), 1000: (2, 6), 2499: (2, 6), 2500: (2, 7),
            4999: (2, 7), 5000: (2, 8), 9999: (2, 8), 10000: (2, 9), 24999: (2, 9), 25000: (2, 10),
            49999: (2, 10), 50000: (2, 11), 74999: (2, 11), 75000: (2, 12), 99999: (2, 12),
            100000: (2, 13), 174999: (2, 13), 175000: (2, 14), 249999: (2, 14), 250000: (2, 15),
            499999: (2, 15), 500000: (2, 16), 10 ** 9: (2, 16)}
    assert sorted(hand) == sorted(ks())
    for k in ks():
        sm, st = hand[k]
        assert (small(max(k, 0)), strong(max(k, 0))) == (sm, st), k
        assert [power(k, x) for x in KARMA_VALUES] == [sm, -sm, st, -st], k
        for x in KARMA_VALUES:
            vec(k, x, "k < 0 is treated as 0 (k' = max(k, 0))" if k < 0 and x == "up" else None)
    for k in (-5, 0, 999, 1000, 500000):
        sm, st = hand[k]
        assert [power(k, x) for x in AGREEMENT_VALUES] == [sm, -sm, st, -st]
        for x in AGREEMENT_VALUES:
            vec(k, x)
    big = 2 ** 53 - 1
    for k in (-big, big):
        for x in KARMA_VALUES:
            vec(k, x, "the largest JSON-safe magnitude" if x == "up" else None)
    assert [power(big, x) for x in KARMA_VALUES] == [2, -2, 16, -16]
    assert [power(-big, x) for x in KARMA_VALUES] == [1, -1, 1, -1]
    return out


# =========================================================================== staged.json

def staged_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ O3 self-opinions
    f, clk = F(), Clock()
    pa, pm = f.post("alice", clk()), f.post("mona", clk())
    self_a = f.opinion("alice", pa, karma="up", agreement="agree", reacts=[{"react": "insightful"}])
    self_m = f.opinion("mona", pm, karma="strongUp")
    ma = f.opinion("mona", pa, karma="up")
    out.append(V("self-vote-ignored", f, [*mem(f, "alice"), pa, pm, self_a, self_m, ma],
                 "O3: alice's opinion on her own post and mona's strongUp on her own post are ignored "
                 "entirely (not even counted as unweighted); only mona's upvote of alice counts: K1 = KP = K "
                 "= 1 for alice, whose single seed upvote does not establish her (seedUp 1 < 2)",
                 extra=("scores",),
                 expect={"counting": us(ma), "K1": km(alice=1), "seedUp": km(alice=1), "E": ds(*A),
                         "established": [], "KP": km(alice=1), "K": km(alice=1),
                         "scores": {u(pa): {"score": 1, "agreement": 0, "unweighted": 0, "earlier": 0,
                                            "weighted": 1},
                                    u(pm): {"score": 1, "agreement": 0, "unweighted": 0, "earlier": 0,
                                            "weighted": 0}}}))
    # pm: mona ∈ E with KP 0, so self = strong(0) = 1; her own strongUp is ignored

    # ------------------------------------------------------------------ strongUp is a sign in K1
    f, clk = F(), Clock()
    gus = posts(f, clk, "gus", 10)
    noah = posts(f, clk, "noah", 5)
    zed = f.post("zed", clk())
    recs = [*mem(f, "gus", "noah", "zed"), *grant(f, "gus", "noah"), *gus, *noah, zed,
            *votes(f, "mona", gus), *votes(f, "gus", noah, "strongUp"), f.opinion("noah", zed, karma="strongUp")]
    # K1 (sign, cap 25): gus 10, noah 5 (gus is a granted seed), zed 1 (noah is a seed)
    # KP (basis K1): gus 10 × power(0, up) = 10; noah 5 × power(K1[gus]=10, strongUp) = 5 × 2 = 10
    #     (cap 25 × strong(10) = 50); zed power(K1[noah]=5, strongUp) = 1
    # K (basis KP): gus 10; noah 5 × power(KP[gus]=10, strongUp) = 10; zed power(KP[noah]=10, strongUp) = 2
    out.append(V("strongUp-counts-1-in-K1", f, recs,
                 "a strongUp counts as +1 (its sign) in K1 and with power strong(basis) in KP and K: gus "
                 "(granted, K1 10) strongUps 5 of noah's posts (K1 5, KP 10); noah (granted, K1 5, KP 10) "
                 "strongUps zed's post: K1[zed] = 1, KP[zed] = strong(5) = 1, K[zed] = strong(10) = 2",
                 fields=STAGED[1:],
                 expect={"K1": km(gus=10, noah=5, zed=1), "seedUp": km(gus=1, noah=1, zed=1),
                         "E": ds(*A, "gus", "noah"), "established": [], "KP": km(gus=10, noah=10, zed=1),
                         "K": km(gus=10, noah=10, zed=2)}))

    # ------------------------------------------------------------------ establishment
    f, clk = F(), Clock()
    pn, po, pz = f.post("nina", clk()), f.post("noah", clk()), f.post("zoe", clk())
    o = [f.opinion("mona", pn, karma="up"), f.opinion("mona", po, karma="up"), f.opinion("mike", po, karma="up"),
         f.opinion("nina", pz, karma="up"), f.opinion("noah", pz, karma="up")]
    out.append(V("establishment-one-vs-two-seeds", f, [*mem(f, "nina", "noah", "zoe"), pn, po, pz, *o],
                 "open mode, establish 1, establishSeeds 2: nina has one seed upvote (K1 1, seedUp 1) and is "
                 "not established; noah has two (K1 2, seedUp 2) and is. Both upvote zoe: only noah's vote "
                 "has weight (KP[zoe] = power(K1[noah]=2, up) = 1); nina's still counts (O1–O5) and shows "
                 "as not yet counted",
                 extra=("scores",),
                 expect={"counting": us(*o), "K1": km(nina=1, noah=2), "seedUp": km(nina=1, noah=2),
                         "E": ds(*A, "noah"), "established": ds("noah"), "KP": km(nina=1, noah=2, zoe=1),
                         "K": km(nina=1, noah=2, zoe=1),
                         "scores": {u(pn): {"score": 1, "agreement": 0, "unweighted": 0, "earlier": 0,
                                            "weighted": 1},
                                    u(po): {"score": 3, "agreement": 0, "unweighted": 0, "earlier": 0,
                                            "weighted": 2},
                                    u(pz): {"score": 1, "agreement": 0, "unweighted": 1, "earlier": 0,
                                            "weighted": 1}}}))
    # po: noah ∈ E, KP 2 → self strong(2) = 1, plus two votes of power 1

    f, clk = F(), Clock()
    una, val, wes = posts(f, clk, "una", 2), posts(f, clk, "val", 3), f.post("wes", clk())
    o = [*votes(f, "mona", una), f.opinion("mona", val[0], karma="up"), f.opinion("mona", val[1], karma="down"),
         f.opinion("mike", val[2], karma="up"), f.opinion("mona", wes, karma="strongUp"),
         f.opinion("mike", wes, karma="up")]
    out.append(V("seedUp-counts-distinct-seeds-with-positive-net", f, [*mem(f, "una", "val", "wes"), *una, *val,
                                                                         wes, *o],
                 "seedUp counts distinct seeds whose net sign sum on the user is positive: una gets two "
                 "upvotes from mona (K1 2, seedUp 1); val gets +1 and −1 from mona (net 0) and +1 from mike "
                 "(K1 1, seedUp 1); wes gets a strongUp from mona and an up from mike (K1 2, seedUp 2) and is "
                 "the only one established",
                 fields=STAGED[1:],
                 expect={"K1": km(una=2, val=1, wes=2), "seedUp": km(una=1, val=1, wes=2), "E": ds(*A, "wes"),
                         "established": ds("wes"), "KP": km(una=2, val=1, wes=2), "K": km(una=2, val=1, wes=2)}))

    # ------------------------------------------------------------------ sybil rings
    for edge in (False, True):
        f, clk = F(), Clock()
        sy = {n: f.post(n, clk()) for n in ("syba", "sybb", "sybc")}
        pr = f.post("rhea", clk())
        o = [f.opinion(a, sy[b], karma="strongUp") for a in sy for b in sy if a != b]
        o += [f.opinion(a, pr, karma="strongUp") for a in sy]
        o += [f.opinion("mona", pr, karma="up"), f.opinion("mike", pr, karma="up")]
        if edge:
            o.append(f.opinion("mona", sy["syba"], karma="up"))
        recs = [*mem(f, "syba", "sybb", "sybc", "rhea"), *sy.values(), pr, *o]
        if not edge:
            out.append(V("sybil-ring-without-seed-edge", f, recs,
                         "three members strongUp each other's posts and rhea's; no seed votes for any of them, "
                         "so K1, seedUp, KP and K are 0 for the ring and its votes have no weight anywhere; "
                         "rhea is established by two seed upvotes and her K is 2 from those alone. The ring's "
                         "opinions still count (O1–O5)",
                         expect={"counting": us(*o), "K1": km(rhea=2), "seedUp": km(rhea=2), "E": ds(*A, "rhea"),
                                 "established": ds("rhea"), "KP": km(rhea=2), "K": km(rhea=2)}))
        else:
            out.append(V("sybil-ring-with-one-seed-edge", f, recs,
                         "as sybil-ring-without-seed-edge, plus one upvote from mona on syba's post: K1[syba] = "
                         "1 and seedUp[syba] = 1 < establishSeeds, so syba is still not established and the "
                         "ring's votes still carry no weight",
                         fields=STAGED[1:],
                         expect={"K1": km(rhea=2, syba=1), "seedUp": km(rhea=2, syba=1), "E": ds(*A, "rhea"),
                                 "established": ds("rhea"), "KP": km(rhea=2, syba=1), "K": km(rhea=2, syba=1)}))

    # ------------------------------------------------------------------ pairCap
    f, clk = F(), Clock()
    pat, quin = posts(f, clk, "pat", 25), posts(f, clk, "quin", 26)
    out.append(V("pairCap-boundary-25-26", f, [*mem(f, "pat", "quin"), *pat, *quin, *votes(f, "mona", pat),
                                               *votes(f, "mona", quin)],
                 "pairCap 25 (default): mona upvotes 25 posts of pat and 26 of quin; each (voter, recipient) "
                 "net sum is clamped to ±25 in K1 and to ±25 × strong(max(basis, 0)) = ±25 in KP and K "
                 "(mona's basis is 0), so both get 25",
                 fields=STAGED[1:],
                 expect={"K1": km(pat=25, quin=25), "seedUp": km(pat=1, quin=1), "E": ds(*A), "established": [],
                         "KP": km(pat=25, quin=25), "K": km(pat=25, quin=25)}))

    f, clk = F(moderators=MODS + (("milo", "moderator"),), policy={"karma": {"pairCap": 2}}), Clock()
    gus = posts(f, clk, "gus", 10)
    rob, sal = posts(f, clk, "rob", 5), posts(f, clk, "sal", 3)
    o = [f.opinion(a, gus[2 * i + j], karma="up") for i, a in enumerate(("owner", "adam", "mona", "mike", "milo"))
         for j in (0, 1)]
    recs = [*mem(f, "gus", "rob", "sal"), *grant(f, "gus"), *gus, *rob, *sal, *o, *votes(f, "gus", rob),
            *votes(f, "gus", sal, "strongDown")]
    # K1: gus = 5 authorities × clamp(2, ±2) = 10; rob clamp(5, ±2) = 2; sal clamp(−3, ±2) = −2
    # KP: gus 10; rob clamp(5 × small(10) = 5, ±2 × strong(10) = ±4) = 4; sal clamp(3 × −strong(10) = −6, ±4) = −4
    # K:  same with basis KP[gus] = 10
    out.append(V("pairCap-scaled-by-strong-in-KP-and-K", f, recs,
                 "pairCap 2: gus (granted) gets two upvotes from each of five authorities (K1 10). He upvotes "
                 "5 posts of rob and strongDowns 3 of sal: K1 clamps each pair to ±2; KP and K clamp to "
                 "±pairCap × strong(max(basis, 0)) = ±2 × strong(10) = ±4, so rob gets 4 and sal −4 although "
                 "the unclamped sums are 5 and −6",
                 fields=STAGED[1:],
                 expect={"K1": km(gus=10, rob=2, sal=-2), "seedUp": km(gus=5, rob=1), "E": ds(*A, "milo", "gus"),
                         "established": [], "KP": km(gus=10, rob=4, sal=-4), "K": km(gus=10, rob=4, sal=-4)}))

    # ------------------------------------------------------------------ deleted subjects (F24)
    f, clk = F(), Clock()
    host = f.post("owner", clk())
    gone_dora, gone_dan = f.post("dora", clk()), f.post("dan", clk())
    gone_cam = f.comment("cam", host, clk())
    gone_wes = f.wiki("wes", clk(), "decision-theory")
    o = [f.opinion("mona", gone_dora, karma="up"),
         *[f.opinion(a, s, karma="up") for s in (gone_dan, gone_cam, gone_wes) for a in ("mona", "mike")]]
    out.append(V("deleted-subject-one-vs-two-voters", f, [*mem(f, "dora", "dan", "cam", "wes"), host, *o],
                 "subjects in `absent` (confirmed deleted): dora's post has one seed voter and earns nothing; "
                 "dan's post and cam's comment have two distinct seed voters and earn 2 each (and establish "
                 "both); wes's wikiRevision earns nothing (votes on wikiRevisions never earn karma). All seven "
                 "opinions count (O2 holds through `absent`); the 2-voter rule only limits their karma",
                 fi={"absent": [u(gone_dora), u(gone_dan), u(gone_cam), u(gone_wes)]},
                 expect={"counting": us(*o), "K1": km(dan=2, cam=2), "seedUp": km(dan=2, cam=2),
                         "E": ds(*A, "dan", "cam"), "established": ds("dan", "cam"), "KP": km(dan=2, cam=2),
                         "K": km(dan=2, cam=2)}))

    f, clk = F(), Clock()
    pn = f.post("noah", clk())
    gone = f.post("eve", clk())
    o = [f.opinion("mona", pn, karma="up"), f.opinion("mike", pn, karma="up"),
         f.opinion("mona", gone, karma="up"), f.opinion("noah", gone, karma="up")]
    # K1 (Elig = S): eve's post has one S voter (mona) → excluded; KP, K (Elig = E): mona and noah → included
    out.append(V("deleted-subject-threshold-per-stage", f, [*mem(f, "noah", "eve"), pn, *o],
                 "the 2-voter threshold uses each stage's eligible set (social.md §20.1 item 14): eve's "
                 "deleted post has votes from mona (a seed) and noah (established). In K1 only one voter is "
                 "in S, so neither vote is included; in KP and K both are in E, so KP[eve] = K[eve] = 2",
                 fi={"absent": [u(gone)]}, fields=STAGED[1:],
                 expect={"K1": km(noah=2), "seedUp": km(noah=2), "E": ds(*A, "noah"), "established": ds("noah"),
                         "KP": km(noah=2, eve=2), "K": km(noah=2, eve=2)}))

    f, clk = F(), Clock()
    gone_olly, gone_sam = f.post("olly", clk()), f.post("sam", clk())
    spam = f.modaction("mona", "hide", u(gone_sam), spam=True)
    self_o = f.opinion("olly", gone_olly, karma="up")
    mo = f.opinion("mona", gone_olly, karma="up")
    so = [f.opinion(a, gone_sam, karma="up") for a in ("mona", "mike")]
    out.append(V("deleted-subject-self-and-spam", f, [*mem(f, "olly", "sam"), self_o, mo, *so, spam],
                 "O3 and O5 apply to the uri of a deleted subject: olly's own vote on his deleted post is "
                 "ignored, which leaves one voter (no karma); sam's deleted post is resolved spam by a "
                 "forum-scope #hide{spam} (a negative assertion, effective without the subject), so its "
                 "two seed votes do not count",
                 fi={"absent": [u(gone_olly), u(gone_sam)]},
                 expect={"counting": us(mo), "K1": {}, "seedUp": {}, "E": ds(*A), "established": [], "KP": {},
                         "K": {}}))

    # ------------------------------------------------------------------ nullify, restrict, ban
    f, clk = F(), Clock()
    pa, pg = f.post("alice", clk()), f.post("gil", clk())
    nul = f.modaction("owner", "restrict", did("gil"), nullifyVotes=True)
    o_gil = f.opinion("gil", pa, karma="up")
    o = [f.opinion("mona", pa, karma="up"), f.opinion("mona", pg, karma="up"), f.opinion("mike", pg, karma="up")]
    out.append(V("nullify", f, [*mem(f, "alice", "gil"), *grant(f, "gil"), nul, pa, pg, o_gil, *o],
                 "the owner nullifies gil (a granted seed): gil ∈ N, so gil leaves S, his opinions do not "
                 "count (O4) and he cannot be established although two seeds upvote him (K1 2, seedUp 2). "
                 "alice keeps only mona's vote (K1 1, seedUp 1, not established)",
                 expect={"counting": us(*o), "K1": km(alice=1, gil=2), "seedUp": km(alice=1, gil=2), "E": ds(*A),
                         "established": [], "KP": km(alice=1, gil=2), "K": km(alice=1, gil=2)}))

    f, clk = F(), Clock()
    pa = f.post("alice", clk())
    nul = f.modaction("owner", "restrict", did("mike"), nullifyVotes=True)
    o = [f.opinion("mona", pa, karma="up"), f.opinion("mike", pa, karma="up")]
    out.append(V("nullified-moderator-is-not-a-seed", f, [*mem(f, "alice"), nul, pa, *o],
                 "the owner (rank 3) nullifies mike (a moderator, rank 1): mike stays an authority but leaves "
                 "S and E, and his opinions do not count (O4), so alice has one seed upvote and is not "
                 "established",
                 expect={"counting": us(o[0]), "K1": km(alice=1), "seedUp": km(alice=1),
                         "E": ds("owner", "adam", "mona"), "established": [], "KP": km(alice=1), "K": km(alice=1)}))

    f, clk = F(), Clock()
    pa = f.post("alice", clk())
    rst = f.modaction("owner", "restrict", did("gil"), capabilities=["vote"])
    o = [f.opinion("gil", pa, karma="up", agreement="agree"), f.opinion("mona", pa, karma="up")]
    out.append(V("restrict-vote", f, [*mem(f, "alice", "gil"), *grant(f, "gil"), rst, pa, *o],
                 "gil (a granted seed) is restricted from `vote`: his opinion still counts (O1–O5) but its "
                 "karma and agreement axes are ignored, so alice has one effective seed upvote (K1 1, seedUp "
                 "1, not established) and agreement 0. A restriction is not a nullification: gil stays in S "
                 "and E",
                 extra=("scores",),
                 expect={"counting": us(*o), "K1": km(alice=1), "seedUp": km(alice=1), "E": ds(*A, "gil"),
                         "established": [], "KP": km(alice=1), "K": km(alice=1),
                         "scores": {u(pa): {"score": 1, "agreement": 0, "unweighted": 0, "earlier": 0,
                                            "weighted": 1}}}))

    f, clk = F(), Clock()
    pg = posts(f, clk, "gil", 2)
    pa = f.post("alice", clk())
    o = [f.opinion("mona", pg[0], karma="down"), f.opinion("mike", pg[1], karma="down"),
         f.opinion("gil", pa, karma="up")]
    # K1[gil] = −2; KP[alice] = power(max(K1[gil], 0) = 0, up) = 1; K[alice] = power(max(KP[gil] = −2, 0), up) = 1
    out.append(V("seed-with-negative-K1-stays-in-E", f, [*mem(f, "gil", "alice"), *grant(f, "gil"), *pg, pa, *o],
                 "P6: gil is a granted seed downvoted by two seeds (K1 −2, KP −2, K −2) and stays in E; his "
                 "upvote of alice has power(max(−2, 0), up) = 1 in every stage",
                 expect={"counting": us(*o), "K1": km(gil=-2, alice=1), "seedUp": km(alice=1), "E": ds(*A, "gil"),
                         "established": [], "KP": km(gil=-2, alice=1), "K": km(gil=-2, alice=1)}))

    f, clk = F(), Clock()
    pa, pb = f.post("alice", clk()), f.post("bea", clk())
    ban = f.modaction("mona", "access", did("bea"), level="banned")
    o_bea = f.opinion("bea", pa, karma="strongUp")
    o = [f.opinion("mona", pa, karma="up"), f.opinion("mona", pb, karma="up"), f.opinion("mike", pb, karma="up")]
    out.append(V("banned-voter", f, [*mem(f, "alice", "bea"), ban, pa, pb, o_bea, *o],
                 "bea is banned: her opinion does not count (O4), and although two seeds upvote her (K1 2, "
                 "seedUp 2) she is excluded from E (B ∪ N)",
                 expect={"counting": us(*o), "K1": km(alice=1, bea=2), "seedUp": km(alice=1, bea=2), "E": ds(*A),
                         "established": [], "KP": km(alice=1, bea=2), "K": km(alice=1, bea=2)}))

    # ------------------------------------------------------------------ participation
    f, clk = F(membership="approval"), Clock()
    pa, pb, pd = f.post("alice", clk()), f.post("bob", clk()), f.post("dora", clk())
    ban = f.modaction("mona", "access", did("dora"), level="banned")
    o = [f.opinion("mona", pb, karma="up"), f.opinion("mike", pb, karma="up"), f.opinion("bob", pa, karma="up"),
         f.opinion("carl", pa, karma="up"), f.opinion("mona", pd, karma="up"), f.opinion("mike", pd, karma="up")]
    out.append(V("approval-mode-E-is-S", f, [*mem(f, "alice", "bob", "dora"), *grant(f, "alice", "carl"), ban, pa,
                                             pb, pd, *o],
                 "approval mode: E = S. bob is an applicant (membership, no grant), so bob ∉ P and two seed "
                 "upvotes (K1 2, seedUp 2) do not establish him; his opinion counts (O4 excludes only B ∪ N) "
                 "but has no weight. dora is a banned member: a participant (approval mode counts banned "
                 "members) but in B, so not established either. carl is granted without a membership: a "
                 "seed, so his upvote of alice counts",
                 expect={"counting": us(*o), "K1": km(alice=1, bob=2, dora=2), "seedUp": km(alice=1, bob=2, dora=2),
                         "E": ds(*A, "alice", "carl"), "established": [], "KP": km(alice=1, bob=2, dora=2),
                         "K": km(alice=1, bob=2, dora=2)}))

    f, clk = F(), Clock()
    pn, pz = f.post("noah", clk()), f.post("zoe", clk())
    o = [f.opinion("mona", pn, karma="up"), f.opinion("mike", pn, karma="up"), f.opinion("gil", pz, karma="up"),
         f.opinion("noah", pz, karma="up")]
    out.append(V("leaving-the-forum", f, [*mem(f, "zoe"), *grant(f, "gil"), pn, pz, *o],
                 "open mode, after gil and noah deleted their membership records: gil was granted, so he "
                 "stays a seed and his vote keeps counting; noah would be established (K1 2, seedUp 2) but is "
                 "no longer a participant, so he leaves E and his vote stops carrying weight",
                 expect={"counting": us(*o), "K1": km(noah=2, zoe=1), "seedUp": km(noah=2, zoe=1),
                         "E": ds(*A, "gil"), "established": [], "KP": km(noah=2, zoe=1), "K": km(noah=2, zoe=1)}))

    # ------------------------------------------------------------------ establish parameters
    f, clk = F(policy={"karma": {"establish": 3, "establishSeeds": 1}}), Clock()
    ann, pb = posts(f, clk, "ann", 3), f.post("ben", clk())
    o = [*votes(f, "mona", ann), f.opinion("mona", pb, karma="up"), f.opinion("mike", pb, karma="up")]
    out.append(V("establish-3-seeds-1", f, [*mem(f, "ann", "ben"), *ann, pb, *o],
                 "establish 3, establishSeeds 1: ann has three upvotes from one seed (K1 3, seedUp 1) and is "
                 "established; ben has one from each of two seeds (K1 2, seedUp 2) and is not",
                 fields=STAGED[1:],
                 expect={"K1": km(ann=3, ben=2), "seedUp": km(ann=1, ben=2), "E": ds(*A, "ann"),
                         "established": ds("ann"), "KP": km(ann=3, ben=2), "K": km(ann=3, ben=2)}))

    f, clk = F(policy={"karma": {"establish": 0, "establishSeeds": 1}}), Clock()
    cora = posts(f, clk, "cora", 2)
    pd = f.post("dee", clk())
    o = [f.opinion("mona", cora[0], karma="up"), f.opinion("mike", cora[1], karma="down")]
    out.append(V("establish-0-seeds-1", f, [*mem(f, "cora", "dee"), *cora, pd, *o],
                 "establish 0, establishSeeds 1: cora gets +1 from mona and −1 from mike (K1 0, seedUp 1: "
                 "mona's net is positive) and is established; dee has no seed upvote (seedUp 0) and is not",
                 fields=STAGED[1:],
                 expect={"K1": {}, "seedUp": km(cora=1), "E": ds(*A, "cora"), "established": ds("cora"),
                         "KP": {}, "K": {}}))

    # ------------------------------------------------------------------ what earns karma, what counts
    f, clk = F(), Clock()
    w = f.wiki("wes", clk(), "decision-theory")
    s = f.sequence("sid", clk())
    o = [f.opinion(a, x, karma="up") for x in (w, s) for a in ("mona", "mike")]
    out.append(V("wiki-and-sequence-votes-earn-no-karma", f, [*mem(f, "wes", "sid"), w, s, *o],
                 "votes on a wikiRevision and a sequence count and give those items a score, but V (§6.2) "
                 "holds only opinions on posts and comments, so they earn no karma and establish nobody",
                 extra=("scores",),
                 expect={"counting": us(*o), "K1": {}, "seedUp": {}, "E": ds(*A), "established": [], "KP": {},
                         "K": {},
                         "scores": {u(x): {"score": 2, "agreement": 0, "unweighted": 0, "earlier": 0, "weighted": 2}
                                    for x in (w, s)}}))

    f, clk = F(), Clock()
    p_ok, p_hidden, p_spam, p_untitled, p_gone, p_unknown = (f.post("alice", clk()) for _ in range(6))
    p_untitled = rec(p_untitled["uri"], {k: v for k, v in p_untitled["value"].items() if k != "title"})
    other = at(did("owner"), "forum", "other")
    root_bad = f.post("alice", clk())
    root_bad = rec(root_bad["uri"], {**root_bad["value"], "forum": other})
    c_co5 = f.comment("alice", root_bad, clk())
    hide = f.modaction("mona", "hide", p_hidden)
    spam = f.modaction("mona", "hide", p_spam, spam=True)
    o_ok = f.opinion("mona", p_ok, karma="up")
    good = f.opinion("mona", p_hidden, karma="up")
    weird = f.opinion("mike", p_unknown, karma="superUp", agreement="agree")
    bad_key = f.opinion("mike", p_ok, karma="up")
    bad_key = rec(at(did("mike"), "opinion", rkey_of(u(bad_key)).rsplit("-", 1)[0] + "-" +
                     h16("opinion", [u(p_hidden)])), bad_key["value"])
    never = [f.opinion("mona", x, karma="up") for x in (p_spam, p_untitled, p_gone, c_co5)]
    recs = [*mem(f, "alice"), p_ok, p_hidden, p_spam, p_untitled, p_unknown, root_bad, c_co5, hide, spam, o_ok,
            good, weird, bad_key, *never]
    out.append(V("opinions-that-never-count", f, recs,
                 "mike's second opinion has a wrong h16 in its rkey (invalid: G5) and fails O1. mona's "
                 "opinions on the spam post (O5), on an untitled post (invalid: PO3), on a post that is "
                 "neither in records nor in absent, and on a comment that CO5 excludes (its root post is "
                 "invalid: G5, because its forum field names another forum) fail O5 or O2. The opinion on a "
                 "hidden (not spam) post counts. mike's opinion with an unknown karma value counts with no "
                 "karma axis (OP3); its agreement axis is kept. alice: K1 2, seedUp 1",
                 invalid={u(bad_key): "G5", u(p_untitled): "PO3", u(root_bad): "G5"}, extra=("scores",),
                 expect={"counting": us(o_ok, good, weird), "K1": km(alice=2), "seedUp": km(alice=1),
                         "E": ds(*A), "established": [], "KP": km(alice=2), "K": km(alice=2),
                         "scores": {u(p_ok): {"score": 1, "agreement": 0, "unweighted": 0, "earlier": 0,
                                              "weighted": 1},
                                    u(p_hidden): {"score": 1, "agreement": 0, "unweighted": 0, "earlier": 0,
                                                  "weighted": 1},
                                    u(p_spam): {"score": 0, "agreement": 0, "unweighted": 0, "earlier": 0,
                                                "weighted": 0},
                                    u(p_unknown): {"score": 0, "agreement": 1, "unweighted": 0, "earlier": 0,
                                                   "weighted": 0}}}))
    return out


# =========================================================================== scores.json

def S(score, agreement=0, unweighted=0, earlier=0, weighted=0) -> dict:
    return {"score": score, "agreement": agreement, "unweighted": unweighted, "earlier": earlier,
            "weighted": weighted}


def score_vectors() -> list:
    out = []

    f, clk = F(), Clock()
    gp = posts(f, clk, "gus", 10)
    g_post = f.post("gus", clk())
    g_comment = f.comment("gus", g_post, clk())
    a = posts(f, clk, "alice", 4)
    o = [*votes(f, "mona", gp),
         f.opinion("gus", a[0], karma="up", agreement="strongAgree"),
         f.opinion("gus", a[1], karma="strongUp", agreement="disagree"),
         f.opinion("gus", a[2], karma="down"), f.opinion("gus", a[3], karma="strongDown")]
    # KP[gus] = 10 → strong 2, small 1. gus's 10 voted posts: self strong(10) = 2 + mona's up 1 = 3.
    exp = {u(p): S(3, weighted=1) for p in gp}
    exp.update({u(g_post): S(2), u(g_comment): S(1), u(a[0]): S(1, 2, weighted=1), u(a[1]): S(2, -1, weighted=1),
                u(a[2]): S(-1, weighted=1), u(a[3]): S(-2, weighted=1)})
    out.append(V("self-score-and-vote-power", f, [*mem(f, "gus", "alice"), *grant(f, "gus"), *gp, g_post,
                                                  g_comment, *a, *o],
                 "gus is a granted seed with KP 10 (mona upvoted 10 of his posts). self(i) = strong(KP) = 2 "
                 "for his posts and small(KP) = 1 for his comment. His votes have power(KP[gus] = 10, x): up 1, "
                 "strongUp 2, down −1, strongDown −2; strongAgree 2, disagree −1. alice is not in E (gus's "
                 "net sign sum on her is 0), so her self is 0",
                 fields=("scores", "KP"), expect={"scores": exp, "KP": km(gus=10)}))
    # KP[alice] = Σ power(K1[gus] = 10, x) = 1 + 2 − 1 − 2 = 0, and her K1 is 0 too (signs +1 +1 −1 −1)

    for voting, agreement in (("karmaOnly", None), ("twoAxis", 2), ("reacts", 2)):
        f, clk = F(policy={"voting": voting}), Clock()
        pa = f.post("alice", clk())
        o = [f.opinion("mona", pa, karma="up", agreement="agree"),
             f.opinion("mike", pa, karma="down", agreement="strongAgree")]
        out.append(V(f"agreement-under-{voting}", f, [*mem(f, "alice"), pa, *o],
                     f"policy.voting = {voting}: the karma axis always counts (mona +1, mike −1: score 0); the "
                     "agreement axis (mona agree +1, mike strongAgree +1) is " +
                     ("ignored, so agreement is null" if agreement is None else "summed: 2"),
                     fields=("scores",), expect={"scores": {u(pa): S(0, agreement, weighted=2)}}))

    f, clk = F(), Clock()
    pa = f.post("alice", clk())
    ban = f.modaction("mona", "access", did("bea"), level="banned")
    rst = f.modaction("owner", "restrict", did("gil"), capabilities=["vote"])
    o = [f.opinion("mona", pa, karma="up"), f.opinion("nina", pa, karma="strongUp"),
         f.opinion("noah", pa, agreement="disagree"), f.opinion("bea", pa, karma="up"),
         f.opinion("gil", pa, karma="up", agreement="agree")]
    out.append(V("unweighted-counts", f, [*mem(f, "alice", "nina", "noah", "bea", "gil"), *grant(f, "gil"), ban,
                                          rst, pa, *o],
                 "on alice's post: mona (E) up is weighted; nina (a member outside E) up is 'not yet counted' "
                 "(unweighted 1); noah (outside E) has only an agreement value, which is neither weighted nor "
                 "unweighted; bea is banned (O4: not counted at all); gil (granted, restricted from vote) has "
                 "no karma or agreement axis left",
                 fields=("scores",), expect={"scores": {u(pa): S(1, 0, unweighted=1, weighted=1)}}))

    f, clk = F(), Clock()
    p1 = f.post("alice", clk())
    p2 = rec(p1["uri"], {**p1["value"], "title": "A post (edited)", "updatedAt": dt(T0 + DAY)})
    o = [f.opinion("mona", p1, karma="up"), f.opinion("mike", p1, karma="up"), f.opinion("adam", p2, karma="up"),
         f.opinion("nina", p1, karma="up"), f.opinion("owner", p1, agreement="agree")]
    # the owner's opinion has no karma value, so K1[alice] = 3 (mona, mike, adam) and seedUp 3: established,
    # KP 3, self strong(3) = 1; score 1 + 3 = 4; earlier 2 of weighted 3; agreement 1 (owner)
    out.append(V("earlier-version-votes", f, [*mem(f, "alice", "nina"), p2, *o],
                 "alice edited her post after mona, mike and nina voted (their subject.cid is the earlier "
                 "CID); adam voted on the current version. N of M = earlier 2 of weighted 3; nina (outside E) "
                 "is unweighted and not part of N or M; the owner's agreement-only opinion on the old version "
                 "counts for agreement but not for N or M. The CID never affects counting",
                 fields=("scores",), extra=("K",),
                 expect={"scores": {u(p2): S(4, 1, unweighted=1, earlier=2, weighted=3)}, "K": km(alice=3)}))

    f, clk = F(), Clock()
    w = f.wiki("wes", clk(), "decision-theory")
    s = f.sequence("sid", clk())
    o = [f.opinion("mona", w, karma="strongUp"), f.opinion("mike", s, karma="up"), f.opinion("adam", s, karma="down")]
    out.append(V("wiki-and-sequence-scores", f, [*mem(f, "wes", "sid"), *grant(f, "wes"), w, s, *o],
                 "wes (a granted seed, KP 0) gets self = small(0) = 1 on his wikiRevision (not a post), plus "
                 "mona's strongUp (1): score 2. sid is outside E (self 0): +1 −1 = 0. No karma is earned",
                 fields=("scores", "K"), expect={"scores": {u(w): S(2, weighted=1), u(s): S(0, weighted=2)}, "K": {}}))

    f, clk = F(), Clock()
    ph, ps, px = f.post("alice", clk()), f.post("alice", clk()), f.post("alice", clk())
    px = rec(px["uri"], {k: v for k, v in px["value"].items() if k != "title"})
    mods = [f.modaction("mona", "hide", ph), f.modaction("mona", "hide", ps, spam=True)]
    o = [f.opinion(a, x, karma="up") for x in (ph, ps, px) for a in ("mona", "mike")]
    out.append(V("hidden-spam-and-excluded-items", f, [*mem(f, "alice"), ph, ps, px, *mods, *o],
                 "a hidden post keeps its score (votes count: K1 2, seedUp 2, so alice is established with "
                 "KP 2 and self 1; score 1 + 2 = 3); votes on a spam post are void (O5) and leave self only; an "
                 "invalid post (PO3: no title) is Excluded and has no score entry",
                 invalid={u(px): "PO3"}, fields=("scores", "K"),
                 expect={"scores": {u(ph): S(3, weighted=2), u(ps): S(1)}, "K": km(alice=2)}))

    f, clk = F(), Clock()
    pn, pa = f.post("noah", clk()), f.post("alice", clk())
    ca = f.comment("alice", pn, clk())
    cn = f.comment("noah", pa, clk(), parent=None)
    o = [f.opinion("mona", pn, karma="up"), f.opinion("mike", pn, karma="up"), f.opinion("noah", pa, karma="strongUp"),
         f.opinion("noah", ca, karma="up"), f.opinion("mona", ca, karma="down"), f.opinion("adam", cn, karma="strongDown")]
    recs = [*mem(f, "noah", "alice"), pn, pa, ca, cn, *o]
    v = V("consistency-K-equals-scores-minus-self", f, recs,
          "§7 consistency: with no pair cap binding and no deleted subjects, K[a] = Σ over a's posts and "
          "comments of (score − self): noah (3 − 1) + (0 − 1) = 1, alice (1 − 0) + (0 − 0) = 1",
          fields=("scores", "K"),
          expect={"K": km(noah=1, alice=1),
                  "scores": {u(pn): S(3, weighted=2), u(pa): S(1, weighted=1), u(ca): S(0, weighted=2),
                             u(cn): S(0, weighted=1)}})
    # noah: K1 1 + 1 − 1 = 1, seedUp 2 (mona, mike; adam's net is negative) → established; KP = K = 1, so
    #       self strong(1) = 1 on pn and small(1) = 1 on cn
    # alice: K1 −1 (mona's downvote; noah is not a seed), not in E: self 0. KP = K = (noah 1 + 1) − 1 = 1
    view = fold_vector_input(v["input"])
    for who in ("noah", "alice"):
        mine = [x for x in (pn, pa, ca, cn) if x["uri"].startswith(f"at://{did(who)}/")]
        total = sum(view.l35[u(x)].score - self_score(view, u(x)) for x in mine)
        assert total == view.l3.K.get(did(who), 0), (who, total)
    out.append(v)

    f, clk = F(policy={"karma": {"pairCap": 2}}), Clock()
    pp = posts(f, clk, "pat", 3)
    out.append(V("scores-have-no-pair-cap", f, [*mem(f, "pat"), *pp, *votes(f, "mona", pp)],
                 "pairCap 2: mona upvotes three of pat's posts. Each post's score counts mona's vote (no pair "
                 "cap in scores) while K[pat] is clamped to 2, so here K ≠ Σ (score − self) = 3",
                 fields=("scores", "K"), expect={"scores": {u(p): S(1, weighted=1) for p in pp}, "K": km(pat=2)}))
    return out


# =========================================================================== gates.json

DEFAULT_GATES = {"reactCreate": 10, "reactSecond": 5, "reactAnti": 20, "authorModPersonal": 50,
                 "authorModFrontpage": 2000, "createTag": 10, "wikiEdit": 5, "sequence": 1000}
OPEN_PRESET = dict(DEFAULT_GATES)
APPROVAL_PRESET = {"reactCreate": 0, "reactSecond": 0, "reactAnti": 5, "authorModPersonal": 10,
                   "authorModFrontpage": 100, "createTag": 0, "wikiEdit": 0, "sequence": 0}


def all_gates(value: bool) -> dict:
    return {g: value for g in DEFAULT_GATES}


def gate_vectors() -> list:
    out = []
    GF = ("gateValues", "passes")

    f, clk = F(), Clock()
    pa = f.post("alice", clk())
    out.append(V("defaults-when-gates-absent", f, [*mem(f, "alice"), pa],
                 "no policy.gates: every gate takes its karma/1 default; authorities pass every gate, alice "
                 "(K 0) passes none",
                 fields=GF, expect={"gateValues": DEFAULT_GATES,
                                    "passes": {**{did(n): all_gates(True) for n in A}, did("alice"): all_gates(False)}}))

    f, clk = F(policy={"gates": OPEN_PRESET}), Clock()
    pa = f.post("alice", clk())
    out.append(V("open-preset", f, [*mem(f, "alice"), pa],
                 "the forum create open preset writes the karma/1 defaults explicitly",
                 fields=GF, expect={"gateValues": OPEN_PRESET,
                                    "passes": {**{did(n): all_gates(True) for n in A}, did("alice"): all_gates(False)}}))

    f, clk = F(membership="approval", policy={"gates": APPROVAL_PRESET}), Clock()
    pa = f.post("alice", clk())
    zero = {g: v <= 0 for g, v in APPROVAL_PRESET.items()}
    out.append(V("approval-preset", f, [*mem(f, "alice"), *grant(f, "alice"), pa],
                 "the approval preset: a granted member with K 0 passes the 0 gates (reactCreate, "
                 "reactSecond, createTag, wikiEdit, sequence) and fails reactAnti 5, authorModPersonal 10 "
                 "and authorModFrontpage 100",
                 fields=GF, expect={"gateValues": APPROVAL_PRESET,
                                    "passes": {**{did(n): all_gates(True) for n in A}, did("alice"): zero}}))

    f, clk = F(policy={"gates": {"reactCreate": 3}}), Clock()
    pa = f.post("alice", clk())
    out.append(V("partial-gates", f, [*mem(f, "alice"), pa],
                 "only reactCreate is present: it takes its value, every other gate its karma/1 default",
                 fields=("gateValues",), expect={"gateValues": {**DEFAULT_GATES, "reactCreate": 3}}))

    g = {"reactCreate": 3, "reactSecond": 2, "reactAnti": -1, "authorModPersonal": 0, "authorModFrontpage": -2,
         "createTag": 1000000000, "wikiEdit": -1000000000, "sequence": 1}
    f, clk = F(policy={"gates": g}), Clock()
    p = {n: f.post(n, clk()) for n in ("kim", "kai", "kat", "kev", "kit")}
    o = [*[f.opinion(a, p["kim"], karma="up") for a in ("owner", "adam", "mona")],
         *[f.opinion(a, p["kai"], karma="up") for a in ("adam", "mona")],
         f.opinion("mona", p["kev"], karma="down"), *[f.opinion(a, p["kit"], karma="down") for a in ("mona", "mike")]]
    K = {"kim": 3, "kai": 2, "kat": 0, "kev": -1, "kit": -2}
    exp = {**{did(n): all_gates(True) for n in A}, **{did(n): {gg: k >= v for gg, v in g.items()} for n, k in K.items()}}
    assert exp[did("kim")]["reactCreate"] and not exp[did("kai")]["reactCreate"]
    assert exp[did("kev")]["reactAnti"] and not exp[did("kit")]["reactAnti"]
    out.append(V("boundaries-and-extremes", f, [*mem(f, *K), *p.values(), *o],
                 "passes(u, g) ⇔ u ∈ A ∨ K[u] ≥ gateValue(g), at the boundary of each gate: K kim 3, kai 2, "
                 "kat 0, kev −1, kit −2 against gates 3, 2, −1, 0, −2, 10⁹, −10⁹ and 1 (negative and extreme "
                 "values are allowed: int[−10⁹..10⁹])",
                 fields=GF + ("K",), expect={"gateValues": g, "passes": exp, "K": km(**K)}))

    g = {gg: 1000000000 for gg in DEFAULT_GATES}
    f, clk = F(policy={"gates": g}), Clock()
    pm, pz = f.post("mike", clk()), f.post("zara", clk())
    o = [f.opinion("owner", pm, karma="down"), f.opinion("adam", pm, karma="down"),
         *[f.opinion(a, pz, karma="up") for a in A]]
    ban = f.modaction("owner", "access", did("mona"), level="banned")
    out.append(V("authorities-bypass-every-gate", f, [*mem(f, "zara"), pm, pz, *o, ban],
                 "every gate is 10⁹: mike (a moderator with K −2) and mona (a moderator banned by the owner, "
                 "so outside S, and whose vote no longer counts: O4) still pass every gate, because "
                 "passes(u, g) only asks u ∈ A; zara (K 3) passes none",
                 fields=GF + ("K",),
                 expect={"gateValues": g,
                         "passes": {**{did(n): all_gates(True) for n in A}, did("zara"): all_gates(False)},
                         "K": km(mike=-2, zara=3)}))

    f, clk = F(policy={"gates": {"reactCreate": 2}}), Clock()
    pn, pu, pd = f.post("noah", clk()), f.post("uma", clk()), f.post("dov", clk())
    o = [f.opinion("mona", pn, karma="up"), f.opinion("mike", pn, karma="up"),
         f.opinion("mona", pu, karma="up"), f.opinion("noah", pu, karma="up"),
         f.opinion("mona", pd, karma="up"), f.opinion("mike", pd, karma="up"), f.opinion("noah", pd, karma="down")]
    out.append(V("gates-use-K", f, [*mem(f, "noah", "uma", "dov"), pn, pu, pd, *o],
                 "gates read K, not K1: noah is established by two seeds. uma has K1 1 (one seed upvote) but K "
                 "2 (noah's upvote counts in KP and K) and passes reactCreate 2; dov has K1 2 but K 1 (noah's "
                 "downvote) and fails it",
                 fields=("passes", "K1", "K"),
                 expect={"K1": km(noah=2, uma=1, dov=2), "K": km(noah=2, uma=2, dov=1),
                         "passes": {**{did(n): all_gates(True) for n in A},
                                    did("noah"): {**all_gates(False), "reactCreate": True},
                                    did("uma"): {**all_gates(False), "reactCreate": True},
                                    did("dov"): all_gates(False)}}))
    return out


FILES = [
    ("karma/power.json", lambda: file_doc(
        "karma/1", "power",
        "social.md §6.1 power(k, x), the port of ForumMagnum's calculateVotePower, at every ladder "
        "boundary (t − 1 and t), for k ∈ {−5, 0, 10⁹, ±(2^53 − 1)}, the four karma values and, at a few "
        "k, the four agreement values. input: {k: int, x: up | down | strongUp | strongDown | agree | "
        "disagree | strongAgree | strongDisagree}. expected: {power: int}. No fold input.",
        power_vectors())),
    ("karma/staged.json", lambda: file_doc(
        "karma/1", "karma",
        "social.md §5 opinion validity (O1–O5, axes, the deleted-subject rule) and §6.2 staged karma: "
        "K1 = pairsum(None, S), seedUp, E, KP = pairsum(Some(K1), E), K = pairsum(Some(KP), E) with the "
        "per-pair cap. input: {fold: FoldInput}. expected: the karma projection fields listed "
        "(testdata/vectors/README.md, social section).",
        staged_vectors())),
    ("karma/scores.json", lambda: file_doc(
        "karma/1", "karma",
        "social.md §7 scores: self(i), score, agreement (null under karmaOnly), unweighted ('n votes not "
        "yet counted'), earlier and weighted ('N of M votes were cast on an earlier version'). input: "
        "{fold: FoldInput}. expected: scores (every non-Excluded item) and karma maps.",
        score_vectors())),
    ("karma/gates.json", lambda: file_doc(
        "karma/1", "karma",
        "social.md §8 gates: gateValue(g) (declared value or karma/1 default) and passes(u, g) ⇔ u ∈ A ∨ "
        "K[u] ≥ gateValue(g). input: {fold: FoldInput}. expected: gateValues ({gate: int}) and passes "
        "({did: {gate: bool}} for every DID of A and every record owner), with K maps for context.",
        gate_vectors())),
]
