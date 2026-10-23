# SPDX-License-Identifier: CC0-1.0
"""karma/tags.json (kind "tags"): tags and wiki, social.md §13 (behaviour karma/1).

The files live in karma/ because §13 belongs to karma/1 (§0.3; social.md §20.1 item 44, NOTES.md
53). karma/tags-large.json holds the cases whose inputs have about 1,000 records.

Wiki authors are participants (open mode, with a membership), because V2 hides a revision by a
non-participant and a hidden revision is on no page. Without opinions K = 0 for everyone, so the
vectors set the gates createTag and wikiEdit to 0 (passes) or 1 (fails for K = 0); authorities
pass every gate. Seed votes give exact K where a tie-break needs it: each seed with KP = 0
votes with power 1.
"""

from __future__ import annotations

from lessdugin_ref.build import MIN, T0, Forum, did, rec
from lessdugin_ref.cids import upper_spelling
from lessdugin_ref.fold import fold_vector_input

from . import kcast
from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))


def F(createTag=0, wikiEdit=0, policy=None, **kw) -> Forum:
    kw.setdefault("moderators", MODS)
    kw.setdefault("membership", "open")
    pol = dict(policy or {})
    pol["gates"] = {"createTag": createTag, "wikiEdit": wikiEdit}
    return Forum(policy=pol, **kw)


class Clock:
    def __init__(self):
        self.t = T0

    def __call__(self) -> int:
        self.t += MIN
        return self.t


def u(r):
    return r if isinstance(r, str) else r["uri"]


def us(*rs) -> list:
    return sorted((u(r) for r in rs), key=str.encode)


def mem(f, *names):
    return [f.membership(n) for n in names]


def V(name, f, records, note, *, fields=None, invalid=None):
    return vector("tags", name, f.fold_input(records), fields=fields, note=note, invalid=invalid)


def W(v, slug):
    return v["expected"]["wiki"][slug]


def cand(v, slug):
    return sorted(W(v, slug)["candidates"], key=str.encode)


def tag_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ linear chain head
    f, clk = F(), Clock()
    r1 = f.wiki("alice", clk(), "epistemics")
    r2 = f.wiki("bob", clk(), "epistemics", parents=[r1])
    r3 = f.wiki("carol", clk(), "epistemics", parents=[r2])
    v = V("linear-chain-head", f, [*mem(f, "alice", "bob", "carol"), r1, r2, r3],
          "a linear history: genesis r1, then r2 and r3, each naming the head it edited. Every author "
          "passes createTag and wikiEdit (gates 0), so all three are candidates; the canonical revision "
          "is the tip of the longest chain (depth 2), and the history follows the edges back to r1")
    w = W(v, "epistemics")
    assert w["canonical"] == u(r3) and w["history"] == [u(r3), u(r2), u(r1)] and w["unmerged"] == 0
    assert [w["candidates"][u(x)]["depth"] for x in (r1, r2, r3)] == [0, 1, 2]
    assert w["candidates"][u(r3)]["parents"] == [u(r2)] and w["exists"] and w["applicable"]
    out.append(v)

    # ------------------------------------------------------------------ fork: depth, then K, then uri
    f, clk = F(), Clock()
    d1 = f.wiki("owner", clk(), "depth")
    da = f.wiki("alice", clk(), "depth", parents=[d1])
    db = f.wiki("bob", clk(), "depth", parents=[d1])
    da2 = f.wiki("carol", clk(), "depth", parents=[da])
    k1 = f.wiki("owner", clk(), "karma")
    ka = f.wiki("alice", clk(), "karma", parents=[k1])
    kb = f.wiki("bob", clk(), "karma", parents=[k1])
    q1 = f.wiki("owner", clk(), "uri")
    qc = f.wiki("carol", clk(), "uri", parents=[q1])
    qd = f.wiki("dave", clk(), "uri", parents=[q1])
    pa = f.post("alice", clk())
    pb = f.post("bob", clk())
    ops = [f.opinion("mona", pa, karma="up"), f.opinion("mona", pb, karma="up"),
           f.opinion("mike", pb, karma="up")]
    v = V("fork-by-depth-then-K-then-uri", f,
          [*mem(f, "alice", "bob", "carol", "dave"), d1, da, db, da2, k1, ka, kb, q1, qc, qd, pa, pb, *ops],
          "three forked pages. depth: alice's and bob's edits both fork from the genesis; carol's edit of "
          "alice's makes the deepest chain (depth 2), which wins; bob's tip is an unmerged edit. karma: "
          "two edits at depth 1 by alice (K 1: one seed upvote) and bob (K 2: two seed upvotes); bob's "
          "wins on K although alice's uri is smaller. uri: carol and dave (K 0) tie on depth and K, so "
          "the smallest uri (carol's) wins")
    view = fold_vector_input(v["input"])
    assert view.l3.K.get(did("alice")) == 1 and view.l3.K.get(did("bob")) == 2
    assert u(ka) < u(kb) and u(qc) < u(qd)
    assert W(v, "depth")["canonical"] == u(da2) and W(v, "depth")["unmerged"] == 1
    assert W(v, "depth")["history"] == [u(da2), u(da), u(d1)]
    assert W(v, "karma")["canonical"] == u(kb) and W(v, "karma")["unmerged"] == 1
    assert W(v, "uri")["canonical"] == u(qc)
    out.append(v)

    # ------------------------------------------------------------------ cycle cut
    f, clk = F(), Clock()
    a0 = f.wiki("alice", clk(), "loop")
    b0 = f.wiki("bob", clk(), "loop")
    c = f.wiki("carol", clk(), "loop", parents=[b0])
    a = rec(u(a0), {**a0["value"], "parents": [{"uri": u(c), "cid": c["cid"]}]})
    b = rec(u(b0), {**b0["value"], "parents": [{"uri": u(a), "cid": a["cid"]}]})
    s0 = f.wiki("dave", clk(), "loop")
    s = rec(u(s0), {**s0["value"], "parents": [{"uri": u(s0), "cid": s0["cid"]}]})
    v = V("cycle-cut-at-the-greatest-uri", f, [*mem(f, "alice", "bob", "carol", "dave"), a, b, c, s],
          "parents name uris, so a history can contain a cycle: alice's revision names carol's, carol's "
          "names bob's, and bob's (rewritten later) names alice's. The cut removes, in each cyclic "
          "strongly connected component, every edge from its greatest-uri member into the component: "
          "carol's revision (did:plc:carol… sorts last) loses its edge, which leaves the chain bob → "
          "alice → carol, so bob's revision (depth 2) is canonical. dave's revision names itself (a "
          "self-loop, which the cut removes too) and counts as an unmerged edit")
    w = W(v, "loop")
    assert max([u(a), u(b), u(c)], key=str.encode) == u(c)
    assert w["candidates"][u(c)]["parents"] == [] and w["candidates"][u(a)]["parents"] == [u(c)]
    assert w["candidates"][u(b)]["parents"] == [u(a)] and w["candidates"][u(s)]["parents"] == []
    assert w["canonical"] == u(b) and w["history"] == [u(b), u(a), u(c)] and w["unmerged"] == 1
    assert not any(x["parentUnavailable"] or x["altered"] for x in w["candidates"].values())
    out.append(v)

    # ------------------------------------------------------------------ moderator #tag.head
    f, clk = F(createTag=1), Clock()
    r1 = f.wiki("mona", clk(), "chosen")
    r2 = f.wiki("alice", clk(), "chosen", parents=[r1])
    r3 = f.wiki("bob", clk(), "chosen", parents=[r2])
    h1 = f.modaction("mona", "tag", "tag:chosen", head={"uri": u(r2), "cid": r2["cid"]})
    s1 = f.wiki("owner", clk(), "other-page")
    s2 = f.wiki("alice", clk(), "other-page", parents=[s1])
    h2 = f.modaction("mona", "tag", "tag:other-page", head={"uri": u(r1), "cid": r1["cid"]})
    g1 = f.wiki("adam", clk(), "gated")
    g2 = f.wiki("carol", clk(), "gated")
    h3 = f.modaction("mona", "tag", "tag:gated", head={"uri": u(g2), "cid": g2["cid"]})
    v = V("moderator-tag-head", f,
          [*mem(f, "alice", "bob", "carol"), r1, r2, r3, s1, s2, g1, g2, h1, h2, h3],
          "createTag = 1, so only authorities create pages; edits need wikiEdit = 0. chosen: mona's "
          "#tag.head names r2, a candidate with its current CID, so r2 is canonical although r3 is "
          "deeper (r3 counts as an unmerged edit). other-page: the head names a revision of another "
          "slug, which is no candidate of this page, so the deepest candidate stands. gated: the head "
          "names carol's genesis, which fails createTag (K 0 < 1) and is no candidate; adam's genesis "
          "is canonical")
    assert W(v, "chosen")["canonical"] == u(r2) and W(v, "chosen")["history"] == [u(r2), u(r1)]
    assert W(v, "chosen")["unmerged"] == 1
    assert W(v, "other-page")["canonical"] == u(s2)
    assert W(v, "gated")["canonical"] == u(g1) and cand(v, "gated") == [u(g1)]
    out.append(v)

    # ------------------------------------------------------------------ removed tag
    f, clk = F(), Clock()
    g = f.wiki("alice", clk(), "gone")
    k = f.wiki("alice", clk(), "kept")
    rm1 = f.modaction("mona", "tag", "tag:gone", removed=True)
    rm2 = f.modaction("mona", "tag", "tag:kept", removed=True)
    un2 = f.modaction("adam", "tag", "tag:kept", removed=False)
    rm3 = f.modaction("mike", "tag", "tag:ghost", removed=False)
    p = f.post("bob", clk(), tags=["gone", "kept", "ghost"])
    v = V("removed-tag", f, [*mem(f, "alice", "bob"), g, k, rm1, rm2, un2, rm3, p],
          "mona's #tag{removed: true} makes gone non-existent although it has a canonical revision, so "
          "bob's own tag gives relevance 1 but does not apply; adam (rank 2) restores kept over mona's "
          "removal; ghost has a tag: subject but no revision, so it has no canonical revision and does "
          "not exist")
    assert W(v, "gone")["canonical"] == u(g) and not W(v, "gone")["exists"] and not W(v, "gone")["applicable"]
    assert W(v, "kept")["exists"] and W(v, "kept")["applicable"]
    assert W(v, "ghost") == {"canonical": None, "exists": False, "applicable": False, "partial": False,
                             "history": [], "unmerged": 0, "candidates": {}}
    assert v["expected"]["tagRel"] == {u(p): {"ghost": 1, "gone": 1, "kept": 1}}
    assert v["expected"]["applies"] == {u(p): ["kept"]}
    out.append(v)

    # ------------------------------------------------------------------ deleted, hidden, ineligible parents
    f, clk = F(createTag=1), Clock()
    gone = f.wiki("owner", clk(), "x")                       # never in records: deleted
    x2 = f.wiki("mona", clk(), "x", parents=[gone])
    x3 = f.wiki("alice", clk(), "x", parents=[gone])
    y1 = f.wiki("mike", clk(), "y")
    hy = f.modaction("adam", "hide", y1)
    y2 = f.wiki("alice", clk(), "y", parents=[y1])
    y3 = f.wiki("adam", clk(), "y", parents=[y1])
    z1 = f.wiki("alice", clk(), "z")
    z2 = f.wiki("bob", clk(), "z", parents=[z1])
    v = V("deleted-parent-gives-a-flagged-root-gated-as-genesis", f,
          [*mem(f, "alice", "bob"), x2, x3, y1, hy, y2, y3, z1, z2],
          "createTag = 1 (only authorities pass), wikiEdit = 0. x: both edits name a deleted revision, so "
          "par(r) = ∅; mona's enters Cand through genesisOK and is flagged parentUnavailable, alice's "
          "(wikiEdit only) is no candidate. y: mike's genesis is hidden by adam (V3), so it is on no page; "
          "adam's edit is a flagged root, alice's is no candidate. z: alice's genesis fails createTag, "
          "so bob's edit of it has no candidate parent and is no candidate either (a fake parent cannot "
          "bypass createTag); z has no canonical revision")
    assert cand(v, "x") == [u(x2)] and W(v, "x")["candidates"][u(x2)]["parentUnavailable"] is True
    assert cand(v, "y") == [u(y3)] and W(v, "y")["candidates"][u(y3)]["parentUnavailable"] is True
    assert W(v, "z")["canonical"] is None and cand(v, "z") == [] and not W(v, "z")["exists"]
    out.append(v)

    # ------------------------------------------------------------------ ancestor altered
    f, clk = F(), Clock()
    r1_old = f.wiki("alice", clk(), "altered", name="Altered")
    r1 = rec(u(r1_old), {**r1_old["value"], "name": "Altered (fixed)"})
    r2 = f.wiki("bob", clk(), "altered", parents=[r1_old])
    r3 = f.wiki("carol", clk(), "altered", parents=[r2])
    v = V("ancestor-altered", f, [*mem(f, "alice", "bob", "carol"), r1, r2, r3],
          "bob's revision names alice's genesis with the CID of its earlier version (the genesis was "
          "rewritten afterwards): the edge still exists (ancestry is by uri) and is marked altered; "
          "carol's edge to bob's revision carries the current CID")
    w = W(v, "altered")
    assert w["candidates"][u(r2)]["altered"] == [u(r1)] and w["candidates"][u(r3)]["altered"] == []
    assert w["canonical"] == u(r3)
    out.append(v)

    # ------------------------------------------------------------------ tagCreation = moderators
    f, clk = F(policy={"tagCreation": "moderators"}), Clock()
    a1 = f.wiki("alice", clk(), "mods-only")
    m1 = f.wiki("mona", clk(), "mods-only")
    a2 = f.wiki("alice", clk(), "mods-only", parents=[m1])
    b2 = f.wiki("bob", clk(), "mods-only", parents=[a1])
    v = V("tagCreation-moderators", f, [*mem(f, "alice", "bob"), a1, m1, a2, b2],
          "tagCreation = moderators: a genesis revision needs an authority author (createTag = 0 is "
          "ignored), so alice's genesis is no candidate and bob's edit of it neither; mona's genesis is, "
          "and alice's edit of it (wikiEdit 0) is the canonical head")
    assert cand(v, "mods-only") == us(m1, a2) and W(v, "mods-only")["canonical"] == u(a2)
    out.append(v)

    # ------------------------------------------------------------------ isTag = false
    f, clk = F(), Clock()
    w1 = f.wiki("alice", clk(), "wiki-only", isTag=False)
    t1 = f.wiki("alice", clk(), "now-a-tag", isTag=False)
    t2 = f.wiki("bob", clk(), "now-a-tag", parents=[t1], isTag=True)
    p = f.post("carol", clk(), tags=["wiki-only", "now-a-tag"])
    v = V("isTag-false", f, [*mem(f, "alice", "bob", "carol"), w1, t1, t2, p],
          "a page whose canonical revision has isTag = false exists but is not applicable, so carol's "
          "own tag gives relevance 1 and does not apply; the canonical revision decides, so now-a-tag "
          "(isTag false at genesis, true at the head) applies")
    assert W(v, "wiki-only")["exists"] and not W(v, "wiki-only")["applicable"]
    assert W(v, "now-a-tag")["applicable"]
    assert v["expected"]["applies"] == {u(p): ["now-a-tag"]}
    out.append(v)

    # ------------------------------------------------------------------ relevance (§13.3)
    f, clk = F(), Clock()
    page = f.wiki("owner", clk(), "t")
    p1 = f.post("alice", clk(), tags=["t"])
    p2 = f.post("alice", clk(), tags=["t", "t"])
    p3 = f.post("bob", clk())
    p4 = f.post("bob", clk())
    p5 = f.post("carol", clk(), tags=["t"])
    p6 = f.post("bob", clk())
    p7 = f.post("bob", clk())
    p8 = f.post("bob", clk())
    p9 = f.post("bob", clk())
    p10 = f.post("alice", clk(), tags=["t"])
    p11 = f.post("bob", clk(), tags=["unknown"])
    c = f.comment("bob", p1, clk())

    def tv(*pairs):
        return [{"tag": s, "value": x} for s, x in pairs]

    ops = [
        f.opinion("mona", p2, tags=tv(("t", "down"))),
        f.opinion("mona", p3, tags=tv(("t", "up"))),
        f.opinion("mona", p4, tags=tv(("t", "up"))),
        f.opinion("mike", p4, tags=tv(("t", "down"))),
        f.opinion("dave", p6, tags=tv(("t", "up"))),
        f.opinion("erin", p7, tags=tv(("t", "up"))),
        f.opinion("mona", p8, tags=tv(("t", "up"), ("t", "down"))),
        f.opinion("mike", p8, tags=tv(("t", "strongUp"), ("t", "strongUp"))),
        f.opinion("mona", p9, tags=tv(("t", "superUp"))),
        f.opinion("alice", p10, tags=tv(("t", "strongUp"))),
        f.opinion("mona", p10, tags=tv(("t", "strongDown"))),
        f.opinion("mike", p10, tags=tv(("t", "down"))),
        f.opinion("mona", c, karma="up", tags=tv(("t", "up"))),
    ]
    grants = [f.modaction("owner", "access", did("dave"), level="member", t=T0 + MIN)]
    rst = [f.modaction("mona", "restrict", did("carol"), capabilities=["tag"]),
           f.modaction("mona", "restrict", did("dave"), capabilities=["tag"])]
    v = V("relevance-boundaries", f,
          [*mem(f, "alice", "bob", "carol", "dave", "erin"), page, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10,
           p11, c, *ops, *grants, *rst],
          "rel(p, t) = own + votes, applies ⇔ applicable(t) ∧ rel > 0; voters are seeds with KP 0, so "
          "every vote has power ±1. p1: own tag, 1. p2: own tag (duplicates collapse, PO7) and one down "
          "vote, 0. p3: one up vote, 1. p4: up and down, 0. p5: carol is restricted from tag, so her own "
          "tag gives 0. p6: dave (a granted seed) is restricted from tag, so his vote is ignored. p7: "
          "erin is a newcomer outside E, so her vote has no weight. p8: mona's conflicting entries for t "
          "are all ignored (OP4) and mike's equal strongUp entries collapse to one, 1. p9: an unknown "
          "value discards the entry (OP3). p10: own tag 1, alice's vote on her own post is ignored (O3), "
          "strongDown and down, −1. p11: an own tag with no page, relevance 1 that never applies. Tag "
          "votes on a comment are ignored (OP4)")
    want_rel = {u(p1): {"t": 1}, u(p3): {"t": 1}, u(p8): {"t": 1}, u(p10): {"t": -1},
                u(p11): {"unknown": 1}}
    assert v["expected"]["tagRel"] == {k: want_rel[k] for k in sorted(want_rel, key=str.encode)}
    assert v["expected"]["applies"] == {k: ["t"] for k in us(p1, p3, p8)}
    out.append(v)

    out += m3_tag_vectors()
    return out


def tv(*pairs):
    return [{"tag": s, "value": x} for s, x in pairs]


def m3_tag_vectors() -> list:
    """Cases proposed by the M3 fold reviews (social.md §13, §20.1 items 41, 45)."""
    out = []

    def m(k):
        return T0 + k * MIN

    # A head whose CID is spelled differently still names the current version (item 45)
    f = F(membership="approval", moderators=MODS + (("abby", "admin"),), policy={"tagCreation": "open"})
    r1 = f.wiki("owner", m(30), "w")
    r2 = f.wiki("owner", m(31), "w", parents=[(u(r1), upper_spelling(r1["cid"]))])
    ha = f.modaction("adam", "tag", "tag:w", head={"uri": u(r1), "cid": upper_spelling(r1["cid"])}, t=m(40))
    hb = f.modaction("abby", "tag", "tag:w", head={"uri": u(r1), "cid": r1["cid"]}, t=m(41))
    v = V("tag-head-spellings-agree", f, [r1, r2, ha, hb],
          "the owner writes genesis r1 of slug w and r2 on top of it, citing r1's CID in base32 upper case "
          "(multibase B); two admins set the head of tag:w to r1, one with that spelling and one in "
          "canonical form. CIDs written in records compare by canonical form (social.md §0.4, §20.1 item "
          "45): the heads agree, so r1 is canonical although r2 is deeper, and the edge r2 → r1 is not "
          "altered")
    w = W(v, "w")
    assert w["canonical"] == u(r1) and w["candidates"][u(r2)]["altered"] == [] and w["history"] == [u(r1)]
    out.append(v)

    # The canonical tie-break reads K (cases/kcast.py: K wes 2 > K abe 1; KP 1 = 1)
    f = kcast.forum(policy={"tagCreation": "open", "gates": {"createTag": 0, "wikiEdit": 0}})
    kc = kcast.cast(f)
    rz = f.wiki("abe", m(30), "t")
    rw = f.wiki("wes", m(30), "t")
    v = V("canonical-tie-by-k", f, [*kc["records"], rz, rw],
          "the K ≠ KP cast (approval mode, every user granted): abe and wes each write a genesis revision of "
          "slug t at the same time (depth 0 both). The tie-break after depth is the author's K (§13.2): "
          "wes has K 2 and abe K 1, so wes's revision is canonical; by KP (1 and 1) the smaller uri, abe's, "
          "would be")
    kcast.check(fold_vector_input(v["input"]))
    assert u(rz).encode() < u(rw).encode() and W(v, "t")["canonical"] == u(rw)
    out.append(v)

    # Tag votes weigh by KP (§13.3)
    f = kcast.forum(policy={"tagCreation": "open", "gates": {"createTag": 0, "wikiEdit": 0}})
    kc = kcast.cast(f)
    bp = kc["posts"]["boosterl"]
    page = f.wiki("owner", m(30), "t")
    vote = f.opinion("vera", bp, t=m(31), tags=tv(("t", "strongUp")))
    v = V("tag-vote-power-kp", f, [*kc["records"], page, vote],
          "the K ≠ KP cast: vera (K1 9, KP 10, K 9) votes strongUp for tag t on boosterl's post, with an "
          "opinion that carries only that tag vote. A tag vote weighs power(KP[voter], value) (§13.3): "
          "strong(10) = 2; reading K would give strong(9) = 1")
    kcast.check(fold_vector_input(v["input"]))
    assert v["expected"]["tagRel"] == {u(bp): {"t": 2}} and v["expected"]["applies"] == {u(bp): ["t"]}
    out.append(v)

    # History steps to the deepest parent, ties by the smallest uri (§13.2)
    f = F()
    r1 = f.wiki("owner", m(10), "h")
    r2 = f.wiki("owner", m(11), "h")
    r3 = f.wiki("owner", m(12), "h", parents=[r2, r1])
    v = V("history-tie-smallest-uri", f, [r1, r2, r3],
          "two genesis revisions r1 and r2, and r3 naming both (the greater uri first). r3 is canonical "
          "(depth 1); its parents have equal depth 0, so the history steps to the smaller uri, r1. Both "
          "parents have an incoming edge, so nothing is unmerged")
    w = W(v, "h")
    assert u(r1).encode() < u(r2).encode()
    assert w["canonical"] == u(r3) and w["history"] == [u(r3), u(r1)] and w["unmerged"] == 0
    assert w["candidates"][u(r3)]["depth"] == 1
    out.append(v)

    # Duplicate parent entries: any entry with another CID marks the edge altered (NOTES 56)
    f = F()
    r1 = f.wiki("owner", m(10), "d")
    foreign = f.wiki("owner", m(9), "elsewhere")["cid"]
    r2 = f.wiki("owner", m(11), "d", parents=[(u(r1), foreign), (u(r1), r1["cid"])])
    r3 = f.wiki("owner", m(12), "d", parents=[(u(r1), r1["cid"]), (u(r1), r1["cid"])])
    v = V("duplicate-parent-entries", f, [r1, r2, r3],
          "r2 lists r1 twice, once with the CID of another record and once with r1's own; r3 lists r1 twice "
          "with r1's CID. Duplicate parents collapse to one edge (WR2), and the edge is altered when any of "
          "its entries carries another CID: r2 → r1 is altered, r3 → r1 is not")
    w = W(v, "d")
    assert w["candidates"][u(r2)]["parents"] == [u(r1)] and w["candidates"][u(r2)]["altered"] == [u(r1)]
    assert w["candidates"][u(r3)]["altered"] == []
    out.append(v)

    # OP3 before OP4 (§13.3, item 41)
    f = F(membership="approval")
    page = f.wiki("owner", m(5), "t")
    p = f.post("adam", m(10))
    o = f.opinion("owner", p, t=m(20), tags=tv(("t", "up"), ("t", "superUp")))
    v = V("tag-votes-op3-before-op4", f, [page, p, o],
          "the owner's opinion on adam's post carries two entries for tag t, up and the unknown superUp. OP3 "
          "discards the unknown value first, then OP4 sees a single entry, so the vote counts: rel 1 "
          "(applying OP4 first would drop both conflicting entries)")
    assert v["expected"]["tagRel"] == {u(p): {"t": 1}}
    out.append(v)
    return out


def large_vectors() -> list:
    """Inputs of about 1,000 records: the page cap (§13.2, limits.md WIKI_WALK_MAX) and own-tag
    weight at KP 1,000."""
    out = []

    def m(k):
        return T0 + k * MIN

    # The page cap keeps the 1,000 revisions with the greatest TIDs, ties by the smallest uri
    for n_low in (1, 2):
        f = F()
        recs = [f.wiki("owner", m(10 + i), "cap", clock=1) for i in range(999)]
        low = [f.wiki(w, m(1), "cap", clock=1) for w in ("adam", "mona")[:n_low]]
        total = len(recs) + len(low)
        v = vector("tags", f"page-cap-{total}", f.fold_input([*recs, *low], obs={}),
                   note=(f"{total} genesis revisions of slug cap: 999 by the owner at T0 + 10 … 1008 min, and "
                         + ("adam's at T0 + 1 min. The page holds exactly 1,000 revisions, so it is not partial"
                            if n_low == 1 else
                            "adam's and mona's with the same TID at T0 + 1 min. The page keeps the 1,000 "
                            "revisions with the greatest (TID desc, uri asc): adam's is kept, mona's (the "
                            "greater uri at the same TID) is dropped, and the page is partial")
                         + ". Every revision is a candidate of depth 0; the canonical one is the smallest uri "
                           "(K 0 for everyone), adam's. obs is empty: the page does not read it"),
                   fields=("wiki",))
        w = v["expected"]["wiki"]["cap"]
        assert len(w["candidates"]) == 1000 and w["partial"] is (total == 1001)
        assert u(low[0]) in w["candidates"] and w["canonical"] == u(low[0])
        if n_low == 2:
            assert u(low[1]) not in w["candidates"] and u(low[0]).split("/")[-1] == u(low[1]).split("/")[-1]
        out.append(v)

    # own relevance reads KP: small() changes at 1,000 (§13.3)
    boosters = [f"bst{a}{b}" for a in "abcd" for b in "abcdefghij"]
    assert len(boosters) == 40
    f = kcast.forum(policy={"gates": {"createTag": 0, "wikiEdit": 0}})
    names = (*boosters, "tess", "sid", "ari")
    recs = []
    for n in names:
        recs += [f.membership(n), f.modaction("owner", "access", did(n), level="member", t=m(1))]
    tp = f.post("tess", m(2))
    sp = f.post("sid", m(3))
    ap = [f.post("ari", m(10 + i), **({"tags": ["t"]} if i == 0 else {})) for i in range(25)]
    recs += [tp, sp, *ap]
    recs += [f.opinion(b, tp, t=m(50), karma="up") for b in boosters[:10]]
    recs += [f.opinion(b, sp, t=m(50), karma="up") for b in boosters[:11]]
    recs.append(f.opinion("tess", sp, t=m(50), karma="strongDown"))
    ups = 0
    for b in boosters:
        for p in ap:
            if ups == 998:
                break
            recs.append(f.opinion(b, p, t=m(60), karma="up"))
            ups += 1
    recs.append(f.opinion("sid", ap[1], t=m(60), karma="strongUp"))
    v = vector("tags", "own-tag-weight-kp", f.fold_input(recs, obs={}),
               note="approval mode, every user granted, pairCap 25. 40 boosters (K 0) cast 998 ups on ari's 25 "
                    "posts (at most one per post, so 25 per booster); tess gets 10 ups (K1 = KP = 10); sid "
                    "gets 11 ups and tess's strongDown (K1 10, KP 9, K 9) and votes strongUp on ari's second "
                    "post, which weighs strong(K1 sid 10) = 2 in KP and strong(KP sid 9) = 1 in K. So ari has "
                    "KP 1000 and K 999. ari's first post has the own tag t: own = max(1, small(KP)) = "
                    "small(1000) = 2; reading K would give small(999) = 1. obs is empty: no time is read")
    view = fold_vector_input(v["input"])
    assert view.l3.KP[did("ari")] == 1000 and view.l3.K[did("ari")] == 999 and view.l3.K1[did("ari")] == 999
    assert v["expected"]["tagRel"] == {u(ap[0]): {"t": 2}}
    out.append(v)
    return out


FILES = [
    ("karma/tags.json", lambda: file_doc(
        "karma/1", "tags",
        "social.md §13 tags and wiki: page membership (V1–V5), candidates (genesisOK, editOK, "
        "tagCreation), the cycle cut, depth, the canonical head (moderator #tag.head, then depth desc, "
        "K desc, uri asc), removed tags, isTag, the flags parentUnavailable and ancestor altered "
        "(§13.2), and relevance and application (§13.3: own tags, tag votes, OP3/OP4, restrictions). "
        "input: {fold: FoldInput}. expected: wiki ({slug: {canonical, exists, applicable, partial, "
        "history, unmerged, candidates: {uri: {depth, parents, altered, parentUnavailable}}}}), tagRel "
        "({post uri: {slug: rel}}, non-zero only) and applies ({post uri: [slug]}) "
        "(testdata/vectors/README.md, social section).",
        tag_vectors())),
    ("karma/tags-large.json", lambda: file_doc(
        "karma/1", "tags",
        "social.md §13 vectors whose inputs hold about 1,000 records, in a file of their own: the wiki page "
        "cap of 1,000 revisions (§13.2, limits.md WIKI_WALK_MAX) at 1,000 and 1,001 revisions, and the own-tag "
        "weight small(KP) at KP 1,000 (§13.3). Same kind and shapes as karma/tags.json.",
        large_vectors())),
]
