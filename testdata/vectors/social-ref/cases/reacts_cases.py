# SPDX-License-Identifier: CC0-1.0
"""reacts/reacts.json (kind "reacts", behaviour karma/1 with palette default/1): social.md §9.2.

Karma for the reactors comes from BOOST, a pool of 20 moderators: a user whose post is upvoted
by the first k of them has K1 = KP = K = k (each booster's own karma is 0, so every vote has power
1) and, with k ≥ 2 and a membership in an open forum, is established (in E). Authorities pass
every gate whatever their K, so they serve as creators where the K of the creator does not
matter. The reacts projection cannot list karma fields; the K of every reactor is in the note.
"""

from __future__ import annotations

from lessdugin_ref.build import MIN, T0, Forum, did
from lessdugin_ref.fold import fold_vector_input

from . import kcast
from .util import file_doc, vector

BOOST = tuple(f"bst{c}" for c in "abcdefghijklmnopqrst")
assert len(BOOST) == 20


def u(r):
    return r["uri"]


def ds(*names) -> list:
    return sorted({did(n) for n in names}, key=str.encode)


class Cast:
    """One forum plus the records that give each named user an exact K."""

    def __init__(self, membership="open", policy=None, extra_mods=()):
        mods = tuple((b, "moderator") for b in BOOST) + (("adam", "admin"), ("mona", "moderator")) + extra_mods
        self.f = Forum(membership=membership, moderators=mods, policy=policy)
        self.t = T0
        self.records = []
        self.members = set()

    def clk(self) -> int:
        self.t += MIN
        return self.t

    def add(self, *recs):
        self.records.extend(recs)

    def member(self, *names):
        for n in names:
            if n not in self.members:
                self.members.add(n)
                self.add(self.f.membership(n))

    def karma(self, who, k, member=True):
        """K[who] = k (k ≤ 20) from the first k boosters' upvotes on one post."""
        p = self.f.post(who, self.clk(), title=f"{who}'s karma source")
        self.add(p, *(self.f.opinion(b, p, karma="up") for b in BOOST[:k]))
        if member:
            self.member(who)
        return p

    def karma_one_seed(self, who, k):
        """K[who] = k from one booster's upvotes on k posts: seedUp 1, so who is not established."""
        ps = [self.f.post(who, self.clk(), title=f"{who}'s karma source") for _ in range(k)]
        self.add(*ps, *(self.f.opinion(BOOST[0], p, karma="up") for p in ps))
        self.member(who)

    def subject(self, who="zed"):
        p = self.f.post(who, self.clk(), title="The subject")
        self.add(p)
        if who not in BOOST and who not in ("owner", "adam", "mona"):
            self.member(who)
        return p

    def react(self, who, subject, *entries, **axes):
        o = self.f.opinion(who, subject, reacts=[e if isinstance(e, dict) else {"react": e} for e in entries], **axes)
        self.add(o)
        return o


def tally(react, creator, supporters, opposers=(), qkey=""):
    return {"react": react, "qkey": qkey, "creator": did(creator), "supporters": ds(*supporters),
            "opposers": ds(*opposers)}


def V(name, c: Cast, note, expect, *, invalid=None, fi=None):
    fold_input = c.f.fold_input(c.records, **(fi or {}))
    return vector("reacts", name, fold_input, fields=("reacts",), note=note, invalid=invalid,
                  expect={"reacts": expect})


def react_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ §19: supporters and opposers
    c = Cast()
    c.karma("alice", 9)
    s = c.subject()
    c.react("alice", s, "insightful")
    c.react("mona", s, "seen")
    out.append(V("supporters-9-invalid", c,
                 "alice (K 9, in E) is the only one reacting `insightful`: 9 < reactCreate 10 (karma/1 "
                 "default), so C is empty and the react is not valid. mona's `seen` is a control",
                 {u(s): [tally("seen", "mona", ["mona"])]}))

    c = Cast()
    c.karma("alice", 10)
    c.karma("bob", 4)
    s = c.subject()
    c.react("alice", s, "insightful")
    c.react("bob", s, "insightful")
    out.append(V("supporters-10-4-one-supporter", c,
                 "alice (K 10) passes reactCreate and creates the react; bob (K 4) fails reactSecond 5, so "
                 "the supporters are alice alone", {u(s): [tally("insightful", "alice", ["alice"])]}))

    c = Cast()
    c.karma("alice", 10)
    c.karma("bob", 5)
    s = c.subject()
    c.react("alice", s, "insightful")
    c.react("bob", s, "insightful")
    out.append(V("supporters-10-5-two-supporters", c,
                 "alice (K 10) creates the react; bob (K 5) passes reactSecond 5 and seconds it",
                 {u(s): [tally("insightful", "alice", ["alice", "bob"])]}))

    c = Cast()
    c.karma("carl", 19)
    c.karma("dave", 20)
    s = c.subject()
    c.react("mona", s, "wrong")
    c.react("carl", s, {"react": "wrong", "stance": "against"})
    c.react("dave", s, {"react": "wrong", "stance": "against"})
    out.append(V("anti-19-ignored-20-counted", c,
                 "mona (an authority) creates `wrong`; carl (K 19) and dave (K 20) are against it: only dave "
                 "passes reactAnti 20", {u(s): [tally("wrong", "mona", ["mona"], ["dave"])]}))

    c = Cast()
    c.karma("alice", 10)
    s = c.subject()
    c.react("mona", s, "crux")
    c.react("alice", s, "crux", {"react": "crux", "stance": "against"}, "insightful",
            {"react": "typo", "stance": "against"}, {"react": "typo", "quote": {"exact": "teh"}})
    out.append(V("contradiction-dropped", c,
                 "alice (K 10) is both for and against `crux` (same qkey \"\"): that group is dropped, so mona "
                 "(an authority) alone supports crux and alice is neither supporter nor opposer. Her other "
                 "entries stand: `insightful`; `typo` against without a quote and `typo` for on \"teh\" are "
                 "different (react, qkey) groups, so both yield. typo/\"\" has no supporter and is not valid",
                 {u(s): [tally("crux", "mona", ["mona"]), tally("insightful", "alice", ["alice"]),
                         tally("typo", "alice", ["alice"], qkey="teh")]}))

    c = Cast()
    c.karma("alice", 10)
    c.karma("bob", 5)
    s = c.subject()
    c.react("alice", s, {"react": "insightful", "quote": {"exact": "  the  cat\n"}},
            {"react": "insightful", "quote": {"exact": "the cat", "start": 4}})
    c.react("bob", s, {"react": "insightful", "quote": {"exact": "the cat", "prefix": "on ", "suffix": "!"}},
            {"react": "crux", "quote": {"exact": "Caf\u00e9"}})
    c.react("adam", s, {"react": "insightful", "quote": {"exact": "the dog"}})
    c.react("mona", s, {"react": "crux", "quote": {"exact": "Cafe\u0301"}})
    out.append(V("same-qkey-two-quotes", c,
                 "qkey (lfm-1.md §11) = norm(trim_ws(exact)): alice's \"  the  cat\\n\" and bob's \"the cat\" "
                 "(other prefix, suffix) share qkey \"the cat\" and form one react (alice K 10 creates it, bob "
                 "K 5 seconds it); alice's second entry with the same qkey collapses with her first; adam's "
                 "\"the dog\" is a separate react. mona's \"Cafe\\u0301\" is NFC-normalised to bob's "
                 "\"Caf\\u00e9\": one `crux`, created by mona (bob, K 5, is not in C) and seconded by bob",
                 {u(s): [tally("crux", "mona", ["bob", "mona"], qkey="Caf\u00e9"),
                         tally("insightful", "alice", ["alice", "bob"], qkey="the cat"),
                         tally("insightful", "adam", ["adam"], qkey="the dog")]}))

    c = Cast()
    c.karma("alice", 10)
    c.karma("bob", 10)
    c.karma("carl", 11)
    s = c.subject()
    c.react("bob", s, "insightful", "important")
    c.react("alice", s, "insightful", "important")
    c.react("carl", s, "important")
    out.append(V("creator-tie-by-DID", c,
                 "insightful: alice and bob both have K 10, so the creator is the smaller DID (alice). "
                 "important: carl (K 11) has the greatest K and is the creator although his DID sorts after "
                 "alice's",
                 {u(s): [tally("important", "carl", ["alice", "bob", "carl"]),
                         tally("insightful", "alice", ["alice", "bob"])]}))

    c = Cast()
    c.karma("alice", 10)
    c.karma("bob", 4)
    s = c.subject()
    c.react("mona", s, "insightful", "crux")
    c.react("alice", s, "insightful")
    c.react("bob", s, "crux")
    out.append(V("authority-creator-and-greatest-K", c,
                 "authorities pass every gate whatever their K: mona (K 0) creates `crux` alone (bob, K 4, fails "
                 "reactSecond). For `insightful` the creator is the u ∈ C with the greatest K: alice (K 10) "
                 "rather than mona (K 0); mona is a supporter",
                 {u(s): [tally("crux", "mona", ["mona"]), tally("insightful", "alice", ["alice", "mona"])]}))

    # ------------------------------------------------------------------ who counts
    c = Cast()
    c.karma("nemo", 10, member=False)
    c.karma_one_seed("nell", 10)
    s = c.subject()
    c.add(c.f.modaction("owner", "access", did("mona"), level="banned"))
    c.react("adam", s, "insightful")
    c.react("nemo", s, "insightful")
    c.react("nell", s, "insightful")
    c.react("mona", s, {"react": "insightful", "stance": "against"})
    out.append(V("voters-outside-E-or-banned", c,
                 "nemo has K 10 but no membership (he left), so he is not a participant and not in E; nell is "
                 "a member with K 10 from a single seed (seedUp 1 < establishSeeds 2), a newcomer outside E. "
                 "Both pass reactCreate and reactSecond, but reacts by voters outside E are never counted. "
                 "mona is a moderator banned by the owner: she would pass reactAnti (an authority), but her "
                 "opinion does not count at all (O4)",
                 {u(s): [tally("insightful", "adam", ["adam"])]}))

    for voting in ("karmaOnly", "twoAxis", "reacts"):
        c = Cast(policy={"voting": voting})
        s = c.subject()
        c.react("adam", s, "insightful", karma="up")
        out.append(V(f"voting-{voting}", c,
                     f"policy.voting = {voting}: " + ("reacts count only under `reacts`, so adam's react is "
                                                      "stored but ignored" if voting != "reacts" else
                                                      "the default; adam's react counts"),
                     {} if voting != "reacts" else {u(s): [tally("insightful", "adam", ["adam"])]}))

    c = Cast()
    s = c.subject()
    c.add(c.f.modaction("owner", "restrict", did("mona"), capabilities=["react"]),
          c.f.modaction("owner", "restrict", did("adam"), capabilities=["vote"]))
    c.react("mona", s, "crux", karma="up")
    c.react("adam", s, "insightful", karma="up")
    out.append(V("restrict-react-and-vote", c,
                 "mona is restricted from `react`: her reacts are ignored (her opinion still counts). adam is "
                 "restricted from `vote` only: his karma axis is ignored, his react counts",
                 {u(s): [tally("insightful", "adam", ["adam"])]}))

    c = Cast()
    s = c.subject("mona")
    c.react("mona", s, "insightful")
    c.react("adam", s, "crux")
    out.append(V("self-react-ignored", c,
                 "O3: mona's react on her own post is ignored; adam's counts",
                 {u(s): [tally("crux", "adam", ["adam"])]}))

    c = Cast()
    s = c.subject()
    c.react("mona", s, "not-in-palette", {"react": "crux", "stance": "neutral"}, {"react": "seen"},
            {"react": "thanks", "stance": "for"})
    c.react("adam", s, {"react": "wrong", "stance": "against"})
    c.react("owner", s, {"react": "wrong", "stance": "against"})
    out.append(V("entry-rules", c,
                 "a react id outside the default/1 palette is ignored and an unknown stance discards its entry "
                 "(OP3); a missing stance means `for`. `wrong` has only opposers (adam, owner), so C is empty "
                 "and it is not valid",
                 {u(s): [tally("seen", "mona", ["mona"]), tally("thanks", "mona", ["mona"])]}))

    c = Cast(policy={"gates": {"reactCreate": 0, "reactSecond": 100, "reactAnti": 0}})
    s = c.subject()
    c.member("alice", "bob", "carl")
    c.add(*(c.f.modaction("owner", "access", did(n), level="member") for n in ("alice", "bob", "carl")))
    c.react("bob", s, "insightful")
    c.react("alice", s, "insightful")
    c.react("carl", s, {"react": "insightful", "stance": "against"})
    out.append(V("reactSecond-above-reactCreate", c,
                 "gates reactCreate 0, reactSecond 100, reactAnti 0; alice, bob and carl are granted seeds with "
                 "K 0. alice and bob are both in C (tie on K: the smaller DID, alice, creates); nobody passes "
                 "reactSecond, so supporters = {creator}; carl's against counts (K 0 ≥ 0)",
                 {u(s): [tally("insightful", "alice", ["alice"], ["carl"])]}))

    c = Cast()
    s = c.subject()
    c.react("mona", s, {"react": "typo", "quote": {"exact": "teh \u200e"}}, {"react": "seen", "quote": {"exact": " \u00a0\n"}})
    c.react("adam", s, {"react": "typo", "quote": {"exact": "teh"}}, "seen")
    c.react("owner", s, {"react": "typo", "quote": {"exact": "c\u0007a\u202et s\u001bat"}})
    out.append(V("qkey-normalisation", c,
                 "qkey = norm(trim_ws(exact)) exactly as lfm-1.md §11 orders it (NOTES.md 35): trim_ws runs "
                 "first, so mona's \"teh \\u200e\" keeps its space once the control is removed (qkey \"teh \", "
                 "not \"teh\"); controls are removed (\"c\\u0007a\\u202et s\\u001bat\" -> \"cat sat\"); a quote "
                 "of White_Space only has qkey \"\" and groups with adam's unquoted `seen` (the smaller DID, "
                 "adam, creates it)",
                 {u(s): [tally("seen", "adam", ["adam", "mona"]), tally("typo", "owner", ["owner"], qkey="cat sat"),
                         tally("typo", "adam", ["adam"], qkey="teh"), tally("typo", "mona", ["mona"], qkey="teh ")]}))

    c = Cast()
    ph, ps = c.subject(), c.subject()
    gone = c.f.post("zed", c.clk(), title="Deleted")
    c.add(c.f.modaction("mona", "hide", ph), c.f.modaction("mona", "hide", ps, spam=True))
    for x in (ph, ps, gone):
        c.react("adam", x, "insightful")
    out.append(V("hidden-spam-and-deleted-subjects", c,
                 "a hidden subject still collects reacts; reacts on a spam subject are void (O5); an opinion "
                 "whose subject is in `absent` counts only toward karma, never toward reacts",
                 {u(ph): [tally("insightful", "adam", ["adam"])]}, fi={"absent": [u(gone)]}))

    c = Cast(membership="approval", policy={"gates": {"reactSecond": 0, "reactCreate": 0, "reactAnti": 5,
                                                      "authorModPersonal": 10, "authorModFrontpage": 100,
                                                      "createTag": 0, "wikiEdit": 0, "sequence": 0}})
    s = c.subject()
    c.karma("dave", 5)
    c.member("alice", "bob", "carl", "erin")
    c.add(*(c.f.modaction("owner", "access", did(n), level="member") for n in ("alice", "bob", "carl", "dave", "zed")))
    c.react("alice", s, "insightful")
    c.react("bob", s, "insightful")
    c.react("erin", s, "insightful")
    c.react("carl", s, {"react": "insightful", "stance": "against"})
    c.react("dave", s, {"react": "insightful", "stance": "against"})
    out.append(V("approval-preset", c,
                 "approval mode with the approval preset (reactCreate 0, reactSecond 0, reactAnti 5): granted "
                 "members with K 0 create (alice, the smaller DID) and second (bob); erin is an applicant "
                 "(not granted, so not in P or E) and is ignored; carl (K 0) fails reactAnti; dave (K 5) is an "
                 "opposer",
                 {u(s): [tally("insightful", "alice", ["alice", "bob"], ["dave"])]}))

    c = Cast()
    p = c.subject()
    cm = c.f.comment("zed", p, c.clk())
    w = c.f.wiki("zed", c.clk(), "decision-theory")
    sq = c.f.sequence("zed", c.clk())
    c.add(cm, w, sq)
    c.react("mona", p, "not-a-crux", "crux", "changed-mind", "agree")
    for x in (cm, w, sq):
        c.react("adam", x, "thanks")
    out.append(V("subject-kinds-and-order", c,
                 "reacts apply to posts, comments, wikiRevisions and sequences; each subject's reacts are sorted "
                 "by (react, qkey) bytes: agree < changed-mind < crux < not-a-crux",
                 {u(p): [tally(r, "mona", ["mona"]) for r in ("agree", "changed-mind", "crux", "not-a-crux")],
                  **{u(x): [tally("thanks", "adam", ["adam"])] for x in (cm, w, sq)}}))

    c = Cast()
    c.karma("alice", 10)
    s = c.subject()
    c.add(c.f.modaction("owner", "restrict", did(BOOST[0]), nullifyVotes=True))
    c.react("alice", s, "insightful")
    c.react("mona", s, "seen")
    out.append(V("gates-are-retroactive", c,
                 "alice's karma came from 10 boosters, but the owner nullified one of them: that vote no longer "
                 "counts (O4), so K[alice] = 9 < reactCreate and her react disappears (§8: gates use the "
                 "current K). mona's `seen` is a control", {u(s): [tally("seen", "mona", ["mona"])]}))

    # The creator is chosen by K, not KP (cases/kcast.py: K wes 2 > K abe 1; KP 1 = 1)
    f = kcast.forum(policy={"gates": {"reactCreate": 0, "reactSecond": 0}})
    kc = kcast.cast(f)
    tp = kc["posts"]["tess"]
    recs = [*kc["records"], f.opinion("wes", tp, t=T0 + 20 * MIN, reacts=[{"react": "insightful"}]),
            f.opinion("abe", tp, t=T0 + 20 * MIN, reacts=[{"react": "insightful"}])]
    v = vector("reacts", "creator-by-k", f.fold_input(recs), fields=("reacts",),
               note="the K ≠ KP cast (approval mode, every user granted): wes has K 2 and KP 1, abe K 1 and "
                    "KP 1, and did(abe) < did(wes). Both react insightful on tess's post with the gates "
                    "reactCreate and reactSecond at 0. The creator is the supporter with the greatest K "
                    "(§9.2), so wes; by KP they would tie and abe, the smaller DID, would be the creator",
               expect={"reacts": {u(tp): [tally("insightful", "wes", ["wes", "abe"])]}})
    kcast.check(fold_vector_input(v["input"]))
    out.append(v)
    return out


FILES = [
    ("reacts/reacts.json", lambda: file_doc(
        "karma/1", "reacts",
        "social.md §9.2 react aggregation with the default/1 palette (§9.1): entries, (react, qkey) "
        "grouping (qkey per lfm-1.md §11), contradictions, Fv/Av restricted to E, the reactCreate, "
        "reactSecond and reactAnti gates, the creator (greatest K, ties by the smallest DID). input: "
        "{fold: FoldInput}. expected: reacts = {subject uri: [{react, qkey, creator, supporters, "
        "opposers}]} for valid reacts only, each list sorted by (react, qkey) bytes; supporters include "
        "the creator.",
        react_vectors())),
]
