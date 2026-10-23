# SPDX-License-Identifier: CC0-1.0
"""rank/rank.json (kind "rank"): ranking, sorts, filters and Recent Discussion, social.md §12.

hot() is the only float in the fold (§1.3 rule 3). The model computes it with Python's
math.pow; the Rust fold uses the `libm` crate (pinned to 0.2.16). Every hotBits value of the
file was checked at M3 to equal hot() computed with libm 0.2.16's pow (19 values, a scratch
program outside the repository), so the file carries "hotBitsSource": "rust-libm-0.2.16" and
runners compare bits exactly (testdata/vectors/README.md, "Floats"). To keep new values portable,
every case here is also checked before it is written:

- every hot value listed in `hotBits` comes from a pow(base, e) whose exact value lies within
  0.1 ulp of the double math.pow returned (exact rational arithmetic with `decimal`), so any
  pow with an error below 0.9 ulp returns the same double, and the final division is exact
  IEEE arithmetic;
- in every magic order, two hot values from different (b, t_rank) differ by more than 64 ulps,
  so no ordering depends on the last bits.

Scores come from seed votes: seeds with KP = 0 vote with power ±1, and a post's self score is
strong(KP[author]) = 1 while KP[author] < 10.
"""

from __future__ import annotations

import json
import math
from decimal import Decimal, localcontext

from lessdugin_ref.build import DAY, HOUR, MIN, NOW, T0, Forum, did, md
from lessdugin_ref.fold import fold_vector_input
from lessdugin_ref import rank as R

from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))
VOTERS = ("owner", "adam", "mona", "mike", "sam", "tom")      # seeds (sam and tom are granted)
GATES_0 = {"authorModPersonal": 0, "authorModFrontpage": 0}


def F(*, ranking=None, display=None, gates=None, **kw) -> Forum:
    kw.setdefault("moderators", MODS)
    kw.setdefault("membership", "open")
    kw.setdefault("defaultTier", "frontpage")
    pol = {}
    if ranking is not None:
        pol["ranking"] = dict(ranking)
    if display is not None:
        pol["display"] = dict(display)
    if gates is not None:
        pol["gates"] = dict(gates)
    return Forum(policy=pol or None, **kw)


def u(r):
    return r if isinstance(r, str) else r["uri"]


def us(rs):
    return [u(r) for r in rs]


def base_records(f, *authors):
    """Memberships of the authors, and grants making them seeds (V adds the voters sam and tom
    the same way when a vector has their opinions)."""
    names = sorted(set(authors) - {"owner", "adam", "mona", "mike"})
    return [f.membership(n) for n in names] + \
        [f.modaction("owner", "access", did(n), level="member", t=T0 + MIN) for n in names]


def with_voters(f, records):
    have = {r["uri"] for r in records}
    voters = sorted({n for n in ("sam", "tom") for r in records
                     if r["uri"].startswith(f"at://{did(n)}/") and "opinion" in r["uri"]})
    extra = [r for r in base_records(f, *voters) if r["uri"] not in have]
    return [*records, *extra]


def votes(f, item, ups=0, downs=0, t=T0 + HOUR):
    assert ups + downs <= len(VOTERS)
    return [f.opinion(w, item, t=t, karma="up" if i < ups else "down")
            for i, w in enumerate(VOTERS[:ups + downs])]


def ago(k_hours, now=NOW):
    return now - k_hours * HOUR


# --------------------------------------------------------------------------- float checks

def pow_margin(base: float, e: float) -> float:
    """(exact base^e − math.pow(base, e)) in ulps of the result."""
    with localcontext() as ctx:
        ctx.prec = 90
        exact = (Decimal(base).ln() * Decimal(e)).exp() if base != 1.0 else Decimal(1)
        d = math.pow(base, e)
        return float((exact - Decimal(d)) / Decimal(math.ulp(d)))


def hot_inputs(view, b, t):
    pol = view.policy
    age = float(max(view.input.now - t, 0))
    return age / 3_600_000_000.0 + float(pol.offsetHours), float(pol.exponentMilli) / 1000.0


def check_floats(v) -> None:
    view = fold_vector_input(json.loads(json.dumps(v["input"])))
    q = v["input"].get("query", {})
    groups = []                       # lists of (b, t) whose hot values are compared
    if "posts" in q and q["posts"]["sort"] == "magic":
        order = R.post_order(view, q["posts"])
        groups.append([(R.bonus_score(view, p), R.t_rank(view, p)) for p in order])
    if "comments" in q and q["comments"]["sort"] == "magic":
        th = view.threads[q["comments"]["thread"]]
        for n in [th.root, *th.depth]:
            kids = [c for c in th.children.get(n, []) if c not in th.missing]
            groups.append([(R.score(view, c), R.t_rank(view, c)) for c in kids])
    exact = view.policy.exponentMilli in (0, 1000)     # pow(x, 0) = 1 and pow(x, 1) = x are exact
    for g in groups if not exact else ():
        pts = sorted({(b, t) for b, t in g if b > 0})
        hv = {bt: R.hot(bt[0], bt[1], view.input.now, view.policy) for bt in pts}
        for i, x in enumerate(pts):
            for y in pts[i + 1:]:
                hx, hy = hv[x], hv[y]
                assert abs(hx - hy) > 64 * math.ulp(max(hx, hy)), (v["name"], x, y, hx, hy)
    if "hotBits" in v["expected"]:
        for p in v["expected"]["hotBits"]:
            base, e = hot_inputs(view, None, R.t_rank(view, p))
            m = pow_margin(base, e)
            assert abs(m) < 0.1, (v["name"], p, base, e, m)


def V(name, f, records, query, note, *, fields=None, invalid=None, fi=None):
    if fields is None:
        fields = ("postOrder",) if "posts" in query else ("commentOrder",)
    v = vector("rank", name, f.fold_input(with_voters(f, records), **(fi or {})), fields=fields, query=query,
               note=note, invalid=invalid)
    check_floats(v)
    return v


def PQ(sort, filt="all", **extra):
    return {"posts": {"sort": sort, "filter": filt, **extra}}


def CQ(p, sort):
    return {"comments": {"thread": u(p), "sort": sort}}


def po(v):
    return v["expected"]["postOrder"]


def co(v):
    return v["expected"]["commentOrder"]


def rank_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ hot (§12.1)
    f = F()
    pa = f.post("alice", ago(5))
    pb = f.post("bob", ago(8))
    pc = f.post("carol", ago(12))
    pd = f.post("dave", ago(27))
    cur = f.modaction("adam", "tier", pc, tier="curated", t=ago(11))
    recs = [*base_records(f, "alice", "bob", "carol", "dave"), pa, pb, pc, pd, cur,
            *votes(f, pb, ups=1), *votes(f, pd, ups=2)]
    v = V("hot-bit-patterns-default-parameters", f, recs, PQ("magic"),
          "offsetHours 2, exponentMilli 1150, curatedBonus 10, frontpageBonus 0; ages are whole hours, so "
          "h is exact: b(alice) = 1 at 5 h, b(bob) = 1 + 1 = 2 at 8 h, b(carol) = 1 + 10 + 0 = 11 "
          "(curated) at 12 h, b(dave) = 1 + 2 = 3 at 27 h; hot = b / (h + 2)^1.15",
          fields=("postOrder", "hotBits"))
    view = fold_vector_input(v["input"])
    assert [R.bonus_score(view, u(p)) for p in (pa, pb, pc, pd)] == [1, 2, 11, 3]
    assert po(v) == us([pc, pb, pa, pd])
    out.append(v)

    f = F(ranking={"offsetHours": 1})
    pa = f.post("alice", NOW + HOUR)
    pb = f.post("bob", ago(6))
    pc = f.post("carol", ago(9))
    recs = [*base_records(f, "alice", "bob", "carol"), pa, pb, pc, *votes(f, pb, ups=3), *votes(f, pc, ups=5)]
    v = V("hot-age-is-clamped-at-0", f, recs, PQ("magic"),
          "offsetHours = 1. alice's post claims a time 1 h after now (honest: its first-seen time is the "
          "same, so it is not future-dated and t_rank > now): age = max(now − t_rank, 0) = 0, base = 1 and "
          "hot = b / 1^1.15 = 1 exactly (bit pattern 3ff0000000000000)", fields=("postOrder", "hotBits"))
    assert v["expected"]["hotBits"][u(pa)] == "3ff0000000000000"
    out.append(v)

    f = F(ranking={"offsetHours": 24, "exponentMilli": 1800})
    pa = f.post("alice", ago(7))
    pb = f.post("bob", ago(25))
    pc = f.post("carol", ago(56))
    recs = [*base_records(f, "alice", "bob", "carol"), pa, pb, pc, *votes(f, pb, ups=2), *votes(f, pc, ups=5)]
    v = V("hot-offsetHours-24-exponentMilli-1800", f, recs, PQ("magic"),
          "hot = b / (h + 24)^1.8 with b = 1, 3, 6 at 7 h, 25 h, 56 h", fields=("postOrder", "hotBits"))
    assert po(v) == us([pb, pc, pa])
    out.append(v)

    f = F(ranking={"exponentMilli": 1000})
    pa = f.post("alice", ago(1) - 30 * MIN)
    pb = f.post("bob", ago(4))
    pc = f.post("carol", ago(10) + 17 * MIN + 3)
    recs = [*base_records(f, "alice", "bob", "carol"), pa, pb, pc, *votes(f, pb, ups=2), *votes(f, pc, ups=4)]
    v = V("hot-exponentMilli-1000", f, recs, PQ("magic"),
          "exponentMilli 1000: pow(base, 1.0) = base, so hot = b / (h + 2) uses only correctly rounded IEEE "
          "operations (ages here are not whole hours): b = 1 at 1.5 h, 3 at 4 h, 5 at 9 h 43 min",
          fields=("postOrder", "hotBits"))
    assert po(v) == us([pb, pc, pa])
    out.append(v)

    f = F(ranking={"exponentMilli": 0})
    pa = f.post("alice", ago(1))
    pb = f.post("bob", ago(50))
    pc = f.post("carol", ago(70))
    pd = f.post("dave", ago(3))
    recs = [*base_records(f, "alice", "bob", "carol", "dave"), pa, pb, pc, pd, *votes(f, pb, ups=1),
            *votes(f, pc, ups=1)]
    v = V("hot-exponentMilli-0-ignores-age", f, recs, PQ("magic"),
          "exponentMilli 0: pow(base, 0) = 1 and hot = b, so age plays no part among b > 0: bob and carol "
          "(b = 2) come first, in uri order (carol's 70-hour-old post is not behind bob's newer one), then "
          "alice and dave (b = 1), also in uri order", fields=("postOrder", "hotBits"))
    assert po(v) == us([pb, pc, pa, pd])
    assert set(v["expected"]["hotBits"].values()) == {"4000000000000000", "3ff0000000000000"}
    out.append(v)

    f = F(ranking={"curatedBonus": 30, "frontpageBonus": -2})
    pp = f.post("alice", ago(5))
    pf = f.post("bob", ago(8))
    pc = f.post("carol", ago(9))
    tp = f.modaction("mona", "tier", pp, tier="personal", t=ago(4))
    tc = f.modaction("adam", "tier", pc, tier="curated", t=ago(4))
    recs = [*base_records(f, "alice", "bob", "carol"), pp, pf, pc, tp, tc]
    v = V("hot-curatedBonus-and-frontpageBonus", f, recs, PQ("magic"),
          "b(p) = score + (curated ? curatedBonus : 0) + (tier ≥ frontpage ? frontpageBonus : 0): personal "
          "alice 1; frontpage bob 1 − 2 = −1 (so b ≤ 0); curated carol 1 + 30 − 2 = 29 (curated is ≥ "
          "frontpage, so both bonuses apply)", fields=("postOrder", "hotBits"))
    view = fold_vector_input(v["input"])
    assert [R.bonus_score(view, u(p)) for p in (pp, pf, pc)] == [1, -1, 29]
    assert po(v) == us([pc, pp, pf]) and set(v["expected"]["hotBits"]) == {u(pc), u(pp)}
    out.append(v)

    # ------------------------------------------------------------------ magic partition, pins, ties (§12.2)
    f = F()
    p1 = f.post("alice", ago(60))
    p2 = f.post("bob", ago(1))
    p3 = f.post("carol", ago(2))
    p4 = f.post("dave", ago(3))
    p5 = f.post("erin", ago(10))
    p6 = f.post("fred", ago(1))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin", "fred"), p1, p2, p3, p4, p5, p6,
            *votes(f, p1, ups=1), *votes(f, p3, downs=1), *votes(f, p4, downs=2), *votes(f, p5, downs=2),
            *votes(f, p6, downs=4)]
    v = V("b-le-0-partition", f, recs, PQ("magic"),
          "posts with b > 0 come first, by hot desc (bob's 1 h post with b = 1 before alice's 60 h post with "
          "b = 2); then b ≤ 0 by (b desc, t_rank desc): carol 0, dave −1 (3 h) before erin −1 (10 h), fred "
          "−3 last although newest")
    assert po(v) == us([p2, p1, p3, p4, p5, p6])
    out.append(v)

    f = F()
    p1 = f.post("alice", ago(80 * 24))
    p2 = f.post("bob", ago(70 * 24))
    p3 = f.post("carol", ago(1))
    p4 = f.post("dave", ago(2))
    pin1 = f.modaction("mona", "pin", p1, pin_scope="forum", t=ago(1))
    pin2 = f.modaction("adam", "pin", p2, pin_scope="forum", t=ago(1))
    recs = [*base_records(f, "alice", "bob", "carol", "dave"), p1, p2, p3, p4, pin1, pin2,
            *votes(f, p1, downs=3)]
    v = V("pinned-first", f, recs, PQ("magic"),
          "forum-pinned posts come first under magic, ordered among themselves by the same keys: bob's "
          "pinned post (b = 1, 70 days) before alice's (b = −2, 80 days, in the b ≤ 0 partition); then the "
          "unpinned posts by hot")
    assert po(v) == us([p2, p1, p3, p4])
    out.append(v)

    f = F()
    pa = f.post("alice", ago(3))
    pb = f.post("bob", ago(3))
    pc = f.post("carol", ago(1))
    pd = f.post("dave", ago(5))
    pe = f.post("erin", ago(5))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), pa, pb, pc, pd, pe,
            *votes(f, pd, downs=2), *votes(f, pe, downs=2)]
    for sort, why in (("magic", "alice's and bob's posts have equal b = 1 and equal t_rank (3 h), so equal hot: "
                                "uri asc decides (alice's DID sorts first); dave's and erin's tie in the b ≤ 0 "
                                "partition (b = −1 at 5 h) and are also ordered by uri"),
                      ("new", "the same posts under new: equal t_rank is ordered by uri asc"),
                      ("top", "the same posts under top: equal (score, t_rank) is ordered by uri asc")):
        v = V(f"uri-tie-break-{sort}", f, recs, PQ(sort), why)
        assert po(v) == us([pc, pa, pb, pd, pe]), sort
        out.append(v)

    # ------------------------------------------------------------------ cutoff (§12.2, magic only)
    now = T0 + 200 * DAY
    f = F()
    q1 = f.post("alice", now - HOUR)
    q2 = f.post("bob", now - 89 * DAY)
    q3 = f.post("carol", now - 90 * DAY)
    q4 = f.post("dave", now - 90 * DAY - 1)
    q5 = f.post("erin", now - 150 * DAY)
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), q1, q2, q3, q4, q5,
            *votes(f, q2, ups=1), *votes(f, q4, ups=3), *votes(f, q5, ups=5)]
    want = {"magic": [q1, q2, q3], "top": [q5, q4, q2, q1, q3], "new": [q1, q2, q3, q4, q5],
            "old": [q5, q4, q3, q2, q1]}
    for sort in ("magic", "top", "new", "old"):
        v = V(f"cutoff-90-days-{sort}", f, recs, PQ(sort),
              "now = T0 + 200 days, cutoffDays = 90: magic includes posts with now − t_rank ≤ 90 days "
              "(exactly 90 days is in, 90 days + 1 µs is out); top, new and old include every listed post",
              fi={"now": now})
        assert po(v) == us(want[sort]), sort
        out.append(v)

    pin = f.modaction("mona", "pin", q5, pin_scope="forum", t=now - DAY)
    v = V("pinned-post-beyond-the-cutoff", f, [*recs, pin], PQ("magic"),
          "erin's post is 150 days old, beyond the 90-day cutoff, but forum-pinned: pinned posts are exempt "
          "from the cutoff (social.md §20.1 item 18) and come first", fi={"now": now})
    assert po(v) == us([q5, q1, q2, q3])
    out.append(v)

    f = F(ranking={"cutoffDays": 1})
    r1 = f.post("alice", ago(23))
    r2 = f.post("bob", ago(24))
    r3 = f.post("carol", NOW - DAY - 1)
    recs = [*base_records(f, "alice", "bob", "carol"), r1, r2, r3]
    v = V("cutoffDays-1", f, recs, PQ("magic"),
          "cutoffDays = 1: the magic cutoff is now − t_rank ≤ 1 × 86_400_000_000 µs, so the 24-hour-old "
          "post is in and the one 1 µs older is out")
    assert po(v) == us([r1, r2])
    out.append(v)

    # ------------------------------------------------------------------ top, new, old, recentComments
    f = F()
    pa = f.post("alice", ago(1))
    pb = f.post("bob", ago(2))
    pc = f.post("carol", ago(3))
    pd = f.post("dave", ago(4))
    pe = f.post("erin", ago(5))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), pa, pb, pc, pd, pe,
            *votes(f, pb, ups=2), *votes(f, pc, ups=2), *votes(f, pd, downs=1), *votes(f, pe, ups=4)]
    for sort, want, why in (
            ("top", [pe, pb, pc, pa, pd], "score desc (5, 3, 3, 1, 0), then t_rank desc: bob (2 h) before "
                                          "carol (3 h)"),
            ("new", [pa, pb, pc, pd, pe], "t_rank desc"),
            ("old", [pe, pd, pc, pb, pa], "t_rank asc")):
        v = V(f"sort-{sort}", f, recs, PQ(sort), why)
        assert po(v) == us(want), sort
        out.append(v)

    f = F(gates=GATES_0)
    pa = f.post("alice", ago(10))
    pb = f.post("bob", ago(9))
    pc = f.post("carol", ago(8))
    pd = f.post("dave", ago(7))
    pe = f.post("erin", ago(6))
    ca = f.comment("fred", pa, ago(1))
    cb = f.comment("gina", pb, ago(2))
    cc = f.comment("fred", pc, ago(3))
    cd = f.comment("fred", pd, ago(1))
    hc = f.modaction("mona", "hide", cc, t=ago(2))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin", "fred"), f.membership("gina"), pa, pb, pc,
            pd, pe, ca, cb, cc, cd, hc]
    v = V("sort-recentComments", f, recs, PQ("recentComments"),
          "activity(p) = max(t_rank(p), t_rank of p's Visible comments): fred's comments bump alice's and "
          "dave's posts to 1 h (tie: uri asc); gina is a newcomer (Collapsed, not Visible) and carol's "
          "comment is hidden, so neither bumps; posts without a Visible comment keep their own t_rank")
    assert po(v) == us([pa, pd, pe, pc, pb])
    out.append(v)

    # ------------------------------------------------------------------ filters (§12.3)
    f = F()
    pp = f.post("alice", ago(1))
    pf = f.post("bob", ago(2), kind="question")
    pc = f.post("carol", ago(3), kind="link", link={"uri": "https://example.com/"})
    qf = f.post("dave", ago(4), kind="quickTake", title=None, body=md("A quick take"))
    qc = f.post("erin", ago(5), kind="quickTake", title=None, body=md("Another quick take"))
    pu = f.post("fred", ago(6), kind="poll")
    tp = f.modaction("mona", "tier", pp, tier="personal", t=ago(1))
    tc = f.modaction("adam", "tier", pc, tier="curated", t=ago(1))
    tq = f.modaction("adam", "tier", qc, tier="curated", t=ago(1))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin", "fred"), pp, pf, pc, qf, qc, pu, tp, tc, tq]
    for name, q, want, why in (
            ("filter-frontpage", PQ("new", "frontpage"), [pf, pc, pu],
             "frontpage (the default filter): tier ≥ frontpage and kind ≠ quickTake; the personal post and "
             "both quick takes are left out; fred's unknown kind `poll` is effectively an article (PO2)"),
            ("filter-all", PQ("new", "all"), [pp, pf, pc, qf, qc, pu], "all: any tier and any kind"),
            ("filter-curated", PQ("new", "curated"), [pc], "curated: tier = curated and kind ≠ quickTake"),
            ("filter-kind-quickTake", PQ("new", "all", kind="quickTake"), [qf, qc],
             "the kind filter combines with all"),
            ("filter-kind-article-is-effective", PQ("new", "all", kind="article"), [pp, pu],
             "the kind filter uses the effective kind: `poll` is unknown, so fred's post is an article"),
            ("filter-author", PQ("new", "frontpage", author=did("bob")), [pf],
             "the author filter combines with frontpage")):
        v = V(name, f, recs, q, why)
        assert po(v) == us(want), name
        out.append(v)

    f = F(display={"newUsers": "collapse"})
    pa = f.post("alice", ago(1))
    pb = f.post("bob", ago(2))
    pc = f.post("carol", ago(3))
    pd = f.post("dave", ago(4))
    pe = f.post("erin", ago(5))
    cc = f.comment("alice", pc, ago(2))
    hb = f.modaction("mona", "hide", pb, t=ago(1))
    hc = f.modaction("mona", "hide", pc, t=ago(1))
    ban = f.modaction("mike", "access", did("erin"), level="banned", t=ago(1))
    recs = [*base_records(f, "alice", "bob", "carol"), f.membership("dave"), f.membership("erin"), pa, pb, pc, pd,
            pe, cc, hb, hc, ban]
    v = V("listed-posts-only", f, recs, PQ("new"),
          "lists hold listed posts only (Visible or Collapsed): bob's hidden post is out, and so is carol's "
          "although its comment makes it a Tombstone; dave is a newcomer (Collapsed(NewUser), listed); "
          "erin is banned")
    assert po(v) == us([pa, pd])
    out.append(v)

    f = F()
    page = f.wiki("owner", ago(30), "t")
    off = f.wiki("owner", ago(30), "off", isTag=False)
    pa = f.post("alice", ago(6), tags=["t"])
    pb = f.post("bob", ago(5), tags=["u", "off"])
    pc = f.post("carol", ago(4))
    pd = f.post("dave", ago(3), tags=["t"])
    pe = f.post("erin", ago(2), tags=["t"])
    tv = f.opinion("mona", pc, t=ago(1), tags=[{"tag": "t", "value": "up"}])
    dn = f.opinion("mike", pe, t=ago(1), tags=[{"tag": "t", "value": "down"}])
    hd = f.modaction("mona", "hide", pd, t=ago(1))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), page, off, pa, pb, pc, pd, pe, tv, dn, hd]
    v = V("tag-filter", f, recs, PQ("new", tag="t"),
          "the tag filter keeps listed posts with applies(p, t) (§13.3): alice's own tag (relevance 1) "
          "and mona's tag vote on carol's untagged post (1) apply; erin's own tag is cancelled by "
          "mike's down vote (0); dave's tagged post is hidden; bob's tags name a page that does not "
          "exist (u) and one that is not a tag (off, isTag = false)")
    assert po(v) == us([pc, pa])
    out.append(v)

    # ------------------------------------------------------------------ comments (§12.4)
    f = F(gates=GATES_0)
    p = f.post("alice", ago(20))
    c1 = f.comment("bob", p, ago(10))
    c2 = f.comment("carol", p, ago(9))
    c3 = f.comment("dave", p, ago(8))
    c4 = f.comment("erin", p, ago(5))
    r11 = f.comment("carol", p, ago(4), parent=c1)
    r12 = f.comment("dave", p, ago(3), parent=c1)
    r21 = f.comment("bob", p, ago(1), parent=c2)
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), p, c1, c2, c3, c4, r11, r12, r21,
            *votes(f, c1, ups=3), *votes(f, c2, ups=1), *votes(f, c3, downs=2), *votes(f, r12, ups=2)]
    want = {
        "magic": ([c1, r12, r11, c2, r21, c4, c3],
                  "siblings at every level: b > 0 by hot (c1: b = 4 at 10 h; c2: 2 at 9 h; c4: 1 at 5 h), then "
                  "b ≤ 0 (c3: −1); replies of c1: r12 (b = 3) before r11 (b = 1)"),
        "top": ([c1, r12, r11, c2, r21, c4, c3], "score desc, then t_rank desc, at every level"),
        "new": ([c4, c3, c2, r21, c1, r12, r11], "t_rank desc at every level"),
        "old": ([c1, r11, r12, c2, r21, c3, c4], "t_rank asc at every level"),
        "latestReply": ([c2, r21, c1, r12, r11, c4, c3],
                        "latest(c) = max(t_rank(c), t_rank of c's Visible descendants): c2 (its reply at 1 h), "
                        "c1 (3 h), c4 (5 h), c3 (8 h)"),
    }
    for sort, (order, why) in want.items():
        v = V(f"comments-{sort}", f, recs, CQ(p, sort), why)
        assert co(v) == us(order), sort
        out.append(v)

    f = F(gates=GATES_0)
    p = f.post("alice", ago(20))
    c1, c2, c3, c4, c5 = (f.comment(n, p, ago(k)) for n, k in (("bob", 10), ("carol", 9), ("dave", 8),
                                                                ("erin", 7), ("fred", 6)))
    fp = f.modaction("mona", "pin", c1, pin_scope="thread", t=ago(5))
    ap = f.modaction("alice", "pin", c3, pin_scope="thread", scope="author", t=ago(5))
    ap4 = f.modaction("alice", "pin", c4, pin_scope="thread", scope="author", t=ago(5))
    un4 = f.modaction("mike", "pin", c4, pin_scope="thread", pinned=False, t=ago(4))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin", "fred"), p, c1, c2, c3, c4, c5, fp, ap, ap4,
            un4]
    for sort, order in (("new", [c3, c1, c5, c4, c2]), ("old", [c1, c3, c2, c4, c5])):
        v = V(f"comment-pins-first-{sort}", f, recs, CQ(p, sort),
              "thread pins come first in every comment sort: mona's forum pin of c1 and alice's author pin of "
              "c3 (the forum tier is Default for c3); mike's forum unpin of c4 governs alice's author pin")
        assert co(v) == us(order), sort
        out.append(v)

    f = F(gates=GATES_0)
    p = f.post("alice", ago(20))
    gone1 = f.comment_uri("zoe", p["uri"], ago(19), 7)
    gone2 = f.comment_uri("yan", p["uri"], ago(19), 8)
    c1 = f.comment("bob", p, ago(10))
    c2 = f.comment("carol", p, ago(9))
    c3 = f.comment("dave", p, ago(8), parent=(gone1, p["cid"]))
    c4 = f.comment("erin", p, ago(7), parent=(gone2, p["cid"]))
    r1 = f.comment("dave", p, ago(6), parent=c1)
    r2 = f.comment("erin", p, ago(5), parent=c2)
    h1 = f.modaction("mona", "hide", c1, t=ago(4))
    h2 = f.modaction("mona", "hide", c2, t=ago(4))
    h4 = f.modaction("mona", "hide", c4, t=ago(4))
    hr2 = f.modaction("mona", "hide", r2, t=ago(4))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), p, c1, c2, c3, c4, r1, r2, h1, h2, h4, hr2]
    v = V("comment-tree-display", f, recs, CQ(p, "new"),
          "displayed nodes (§10.2): c1 is hidden but has a listed reply, so it is shown as a Tombstone; c2 "
          "and its reply are both hidden and not shown; the Missing placeholder of c3's parent is shown "
          "(listed descendant) and sorts after every comment sibling; c4's placeholder is not (c4 is "
          "hidden)")
    assert co(v) == us([c1, r1, gone1, c3])
    out.append(v)

    # ------------------------------------------------------------------ Recent Discussion (§12.5)
    f = F(gates=GATES_0)
    pa = f.post("alice", ago(30))
    pb = f.post("bob", ago(29), kind="quickTake", title=None, body=md("A quick take"))
    pc = f.post("carol", ago(28))
    pd = f.post("dave", ago(27))
    pe = f.post("erin", ago(26))
    tp = f.modaction("mona", "tier", pc, tier="personal", t=ago(25))
    ca = [f.comment(n, pa, ago(k)) for n, k in (("bob", 9), ("carol", 8), ("dave", 7), ("erin", 6))]
    cb = [f.comment("alice", pb, ago(5))]
    cc = [f.comment("bob", pc, ago(4)), f.comment("gina", pc, ago(1))]
    cd = [f.comment("gina", pd, ago(2))]
    ce = [f.comment("alice", pe, ago(3))]
    he = f.modaction("mona", "hide", pe, t=ago(2))
    recs = [*base_records(f, "alice", "bob", "carol", "dave", "erin"), f.membership("gina"), pa, pb, pc, pd, pe,
            tp, *ca, *cb, *cc, *cd, *ce, he]
    v = V("recent-discussion", f, recs, {}, fields=("recentDiscussion",),
          note="listed posts of any tier and kind with at least one Visible comment, by the t_rank of the "
               "newest Visible comment desc; each with its 3 newest Visible comments. gina is a newcomer "
               "(Collapsed comments neither count nor show); erin's post is hidden (a Tombstone, not "
               "listed); carol's personal post and bob's quick take are included")
    assert v["expected"]["recentDiscussion"] == [
        {"post": u(pc), "comments": [u(cc[0])]},
        {"post": u(pb), "comments": [u(cb[0])]},
        {"post": u(pa), "comments": us([ca[3], ca[2], ca[1]])},
    ]
    out.append(v)

    out += m3_rank_vectors()
    return out


def m3_rank_vectors() -> list:
    """Cases proposed by the M3 fold reviews (social.md §12.2, §12.4, §12.5, §20.1 items 18, 40)."""
    out = []

    def m(k):
        return T0 + k * MIN

    def cyc(f, root, a, ta, b, tb):
        """A 2-cycle: a's parent is b and b's parent is a (b is built first with a placeholder parent;
        placement never reads CIDs)."""
        xa = f.comment_uri(a, u(root), ta, 1)
        cb = f.comment(b, root, tb, parent=(xa, root["cid"]), clock=1)
        ca = f.comment(a, root, ta, parent=cb, clock=1)
        assert u(ca) == xa
        return ca, cb

    # Recent Discussion: equal newest-comment times order posts by uri, comments by uri (§12.5)
    f = F(membership="approval")
    pa = f.post("adam", m(10))
    pb = f.post("mona", m(10))
    a1 = f.comment("mona", pa, m(50))
    a2 = f.comment("adam", pa, m(50))
    a3 = f.comment("owner", pa, m(40))
    a4 = f.comment("mona", pa, m(30))
    b1 = f.comment("owner", pb, m(50))
    v = V("recent-discussion-ties-by-post-uri", f, [pa, pb, a1, a2, a3, a4, b1], {}, fields=("recentDiscussion",),
          note="approval mode, authorities only. Both posts' newest Visible comments have t_rank T0 + 50 min, so "
               "the posts are ordered by uri; within a post, comments with equal t_rank are ordered by uri, "
               "and only the 3 newest are listed")
    first, second = sorted([a1, a2], key=lambda r: u(r).encode())
    assert v["expected"]["recentDiscussion"] == [
        {"post": p_, "comments": cs} for p_, cs in sorted(
            [(u(pa), us([first, second, a3])), (u(pb), us([b1]))], key=lambda e: e[0].encode())]
    out.append(v)

    # Recent Discussion lists Collapsed posts (§12.5 "listed posts")
    f = F(membership="approval", display={"collapseBelow": 0})
    p = f.post("adam", m(10))
    c = f.comment("mona", p, m(20))
    recs = [p, c, f.opinion("owner", p, t=m(30), karma="down"), f.opinion("owner", c, t=m(30), karma="up")]
    v = V("recent-discussion-lists-collapsed-posts", f, recs, {}, fields=("recentDiscussion",),
          note="collapseBelow = 0: the owner's downvote takes adam's post to score 1 − 1 = 0, Collapsed(LowScore), "
               "which is still listed, so it appears in Recent Discussion with mona's Visible comment (score 2)")
    view = fold_vector_input(v["input"])
    assert view.states[u(p)].to_json() == {"state": "Collapsed", "reason": "LowScore"}
    assert view.l35[u(p)].score == 0 and view.states[u(c)].state == "Visible"
    assert v["expected"]["recentDiscussion"] == [{"post": u(p), "comments": [u(c)]}]
    out.append(v)

    # recentComments activity reads comments under placeholders and re-attached comments (§12.2, item 40)
    f = F(membership="approval")
    pa = f.post("adam", m(10))
    pb = f.post("adam", m(20))
    pc = f.post("adam", m(30))
    gone = f.comment_uri("zoe", u(pa), m(5), 9)
    ca = f.comment("owner", pa, m(100), parent=(gone, pa["cid"]))
    ux, uy = cyc(f, pb, "adam", m(89), "mona", m(90))
    hide = f.modaction("owner", "hide", ux, t=m(95))
    v = V("recent-comments-through-placeholders-and-orphans", f, [pa, pb, pc, ca, ux, uy, hide],
          PQ("recentComments"),
          "activity = max(t_rank(p), t_rank of p's Visible comments) over the whole placed thread: pa's only "
          "comment (T0 + 100 min) sits under a Missing placeholder; in pb, adam's and mona's comments name "
          "each other as parent, mona's (the greater uri) is re-attached to the root and is Visible at "
          "T0 + 90 min, while the owner hid adam's; pc has no comment (T0 + 30 min)")
    assert u(uy) > u(ux) and po(v) == us([pa, pb, pc])
    out.append(v)

    # latestReply: the newest Visible descendant; a collapsed newcomer's reply does not count (§12.4)
    f = F(display={"newUsers": "collapse"})
    p = f.post("owner", m(5))
    c1 = f.comment("adam", p, m(10))
    c2 = f.comment("adam", p, m(20))
    c3 = f.comment("adam", p, m(30))
    c1a = f.comment("mona", p, m(40), parent=c1)
    c1b = f.comment("mona", p, m(100), parent=c1a)
    c3a = f.comment("mona", p, m(41), parent=c3)
    c3b = f.comment("nina", p, m(200), parent=c3a)
    v = V("latest-reply-visible-descendants-only", f, [f.membership("nina"), p, c1, c2, c3, c1a, c1b, c3a, c3b],
          CQ(p, "latestReply"),
          "open mode, newUsers = collapse: nina has a membership only, so her reply c3b is Collapsed(NewUser) "
          "and does not count toward latestReply. Keys: c1 → 100 (c1b, a grandchild), c3 → 41 (c3a), "
          "c2 → 20 (its own t_rank)")
    view = fold_vector_input(v["input"])
    assert view.states[u(c3b)].to_json() == {"state": "Collapsed", "reason": "NewUser"}
    assert co(v) == us([c1, c1a, c1b, c3, c3a, c3b, c2])
    out.append(v)

    # Thread pins come first in every comment sort (§12.4, item 18)
    f = F(membership="approval")
    p = f.post("owner", m(5))
    cp = f.comment("adam", p, m(10))
    cq = f.comment("mona", p, m(20))
    cr = f.comment("adam", p, m(30))
    recs = [p, cp, cq, cr, f.opinion("owner", cp, t=m(40), karma="down"),
            f.modaction("owner", "pin", cp, pin_scope="thread", t=m(45))]
    view = None
    for sort, order in (("magic", [cp, cr, cq]), ("top", [cp, cr, cq]), ("latestReply", [cp, cr, cq]),
                        ("new", [cp, cr, cq]), ("old", [cp, cq, cr])):
        v = V(f"thread-pins-in-every-comment-sort-{sort}", f, recs, CQ(p, sort),
              "the owner pins cp (#pin{scope: thread}) and downvotes it (score 0; cq and cr score 1): the pin "
              "puts cp first in every comment sort, latestReply included; without it every sort but old "
              "would put cp last")
        view = view or fold_vector_input(v["input"])
        assert co(v) == us(order), sort
        out.append(v)
    assert [view.l35[u(x)].score for x in (cp, cq, cr)] == [0, 1, 1]

    # Forum pins come first only in post magic (§12.2, item 18)
    f = F(membership="approval")
    p2 = f.post("adam", m(10))
    p1 = f.post("adam", m(20))
    p3 = f.post("adam", m(30))
    recs = [p1, p2, p3, f.opinion("owner", p1, t=m(40), karma="down"),
            f.modaction("owner", "pin", p1, pin_scope="forum", t=m(45))]
    view = None
    for sort, order in (("magic", [p1, p3, p2]), ("top", [p3, p2, p1]), ("new", [p3, p1, p2]),
                        ("old", [p2, p1, p3]), ("recentComments", [p3, p1, p2])):
        v = V(f"post-pins-only-in-magic-{sort}", f, recs, PQ(sort),
              "the owner pins p1 (#pin{scope: forum}) and downvotes it (score 0; p2 and p3 score 1): the pin "
              "puts p1 first only under magic; the other sorts ignore it")
        view = view or fold_vector_input(v["input"])
        assert po(v) == us(order), sort
        out.append(v)
    assert [view.l35[u(x)].score for x in (p1, p2, p3)] == [0, 1, 1]

    # A hidden root post with a listed comment under a placeholder (§10.2, §12.4, §12.5)
    f = F(membership="approval")
    p = f.post("adam", m(10))
    gone = f.comment_uri("zoe", u(p), m(15), 9)
    c = f.comment("mona", p, m(20), parent=(gone, p["cid"]))
    hide = f.modaction("owner", "hide", p, t=m(30))
    v = V("hidden-root-tombstone-through-placeholder", f, [p, c, hide], CQ(p, "magic"),
          "the owner hid adam's post; mona's comment names a parent comment that is no record, so it sits "
          "under a Missing placeholder. The thread pass walks through the placeholder: the post becomes a "
          "Tombstone, which is not listed, so Recent Discussion is empty; the placeholder (with a listed "
          "descendant) and the comment are displayed",
          fields=("commentOrder", "recentDiscussion"))
    assert co(v) == [gone, u(c)] and v["expected"]["recentDiscussion"] == []
    out.append(v)

    # A re-attached hidden comment with a listed child is a Tombstone (§10.2)
    f = F(membership="approval")
    p = f.post("owner", m(5))
    ux, uy = cyc(f, p, "adam", m(10), "mona", m(11))
    hide = f.modaction("owner", "hide", uy, t=m(20))
    v = V("reattached-hidden-comment-tombstone", f, [p, ux, uy, hide], CQ(p, "old"),
          "adam's ux and mona's uy name each other as parent; uy (the greater uri) is re-attached to the root "
          "and ux becomes its child. The owner hid uy, which has a listed child, so uy is displayed as a "
          "Tombstone before ux")
    assert u(uy) > u(ux) and co(v) == us([uy, ux])
    out.append(v)
    return out


FILES = [
    ("rank/rank.json", lambda: file_doc(
        "magic/1", "rank",
        "social.md §12: hot (§12.1), post sorts magic/top/new/old/recentComments with the b ≤ 0 "
        "partition, forum pins and the magic-only cutoff (§12.2), list filters (§12.3), comment sorts "
        "with thread pins and Missing placeholders (§12.4), and Recent Discussion (§12.5). input: "
        "{fold: FoldInput, query: {posts: {sort, filter, tag?, kind?, author?}} | {comments: {thread, sort}} "
        "| {}}. expected: postOrder (listed post uris), hotBits ({post uri: 16 lowercase hex digits of "
        "the binary64 hot value} for each post in postOrder with b > 0, magic only), commentOrder "
        "(displayed node uris in pre-order), recentDiscussion ([{post, comments}]). hotBits equal hot() "
        "computed with the pow of the libm crate 0.2.16 (hotBitsSource) and with Python's math.pow: "
        "compare them exactly, like the orderings (testdata/vectors/README.md, social section).",
        rank_vectors(), hotBitsSource="rust-libm-0.2.16")),
]
