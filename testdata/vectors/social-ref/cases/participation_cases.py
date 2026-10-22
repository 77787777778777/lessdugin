# SPDX-License-Identifier: CC0-1.0
"""modfold/participation.json (kind "modfold"): participation and seeds, social.md §3 and §4.1.

These inputs contain no opinions, so E = S (no K1, no seed upvotes) and `newcomers` is exact.
"""

from __future__ import annotations

from lessdugin_ref.build import HOUR, T0, Forum, did, rec

from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))
SETS = ("mode", "members", "P", "G", "B", "N", "S", "applicants")


def V(name, f, records, note, *, fields=SETS, extra=(), invalid=None):
    return vector("modfold", name, f.fold_input(records), fields=fields, extra_fields=extra,
                  note=note, invalid=invalid)


def u(r):
    return r["uri"]


def ds(*names):
    return sorted((did(n) for n in names), key=str.encode)


def st(v, r):
    return v["expected"]["modStates"][u(r)]


def hidden(reason):
    return {"state": "Hidden", "reason": reason}


def population(f):
    """The same cast in either mode (see the vector notes)."""
    recs = [f.membership(n) for n in ("owner", "alice", "bob", "dave")]
    posts = {n: f.post(n, T0 + i * HOUR) for i, n in enumerate(("alice", "bob", "carol", "dave", "erin",
                                                                  "mona"), 1)}
    grants = [f.modaction("owner", "access", did(n), level="member") for n in ("alice", "carol")]
    bans = [f.modaction("mona", "access", did(n), level="banned") for n in ("dave", "erin")]
    return recs + list(posts.values()) + grants + bans, posts


def participation_vectors() -> list:
    out = []

    f = Forum(membership="approval", moderators=MODS)
    records, posts = population(f)
    v = V("approval-mode-sets", f, records,
          "approval mode. owner: authority with a membership; mona: authority without one (authority "
          "does not depend on it); alice: membership + grant; bob: membership only (an applicant); carol: "
          "grant only; dave: membership + ban; erin: ban only. P = A ∪ {member ∧ access ≠ none}; "
          "S = (A ∪ G) ∖ (B ∪ N); authorities are never listed as applicants (NOTES.md)",
          extra=("modStates", "modQueue"))
    e = v["expected"]
    assert e["members"] == ds("owner", "alice", "bob", "dave")
    assert e["P"] == ds("owner", "adam", "mona", "mike", "alice", "dave")
    assert e["G"] == ds("alice", "carol") and e["B"] == ds("dave", "erin") and e["N"] == []
    assert e["S"] == ds("owner", "adam", "mona", "mike", "alice", "carol")
    assert e["applicants"] == ds("bob")
    assert e["modQueue"] == [{"type": "applicant", "did": did("bob")}]
    assert [st(v, posts[n]) for n in ("alice", "bob", "carol", "dave", "erin", "mona")] == [
        None, hidden("NotParticipant"), hidden("NotParticipant"), hidden("Banned"),
        hidden("NotParticipant"), None]
    out.append(v)

    f = Forum(membership="open", moderators=MODS)
    records, posts = population(f)
    v = V("open-mode-sets", f, records,
          "the same records in open mode: P = A ∪ members; there are no applicants; newcomers = "
          "P ∖ (S ∪ E) with E = S here (no opinions). dave is banned but still a participant: V4 hides his "
          "content", extra=("newcomers", "modStates", "modQueue"))
    e = v["expected"]
    assert e["P"] == ds("owner", "adam", "mona", "mike", "alice", "bob", "dave") and e["applicants"] == []
    assert e["newcomers"] == ds("bob", "dave") and e["modQueue"] == []
    assert [st(v, posts[n]) for n in ("alice", "bob", "carol", "dave", "erin")] == [
        None, None, hidden("NotParticipant"), hidden("Banned"), hidden("NotParticipant")]
    out.append(v)

    f = Forum(membership="approval", moderators=MODS)
    pd = f.post("dave", T0 + HOUR)
    pd2 = f.post("dave", T0 + 2 * HOUR)
    recs = [f.membership("dave"), pd, pd2, f.modaction("mona", "access", did("dave"), level="banned"),
            f.modaction("owner", "hide", pd, hidden=False)]
    v = V("keep-overrides-a-ban-in-approval-mode", f, recs,
          "a banned member is still a participant in approval mode (access ≠ none), so an item-level keep "
          "at rank ≥ banRank keeps his post visible; his other post is Hidden(Banned)",
          fields=("P", "B", "S", "modStates"))
    assert v["expected"]["P"] == ds("owner", "adam", "mona", "mike", "dave")
    assert st(v, pd) is None and st(v, pd2) == hidden("Banned")
    out.append(v)

    f = Forum(membership="approval", moderators=MODS)
    pa = f.post("alice", T0 + HOUR)
    recs = [f.membership("alice"), pa,
            f.modaction("owner", "access", did("alice"), level="member"),
            f.modaction("mona", "restrict", did("alice"), nullifyVotes=True)]
    v = V("nullify-removes-from-seeds", f, recs,
          "#restrict{nullifyVotes: true} puts alice in N: she leaves S (and E) but stays granted and a "
          "participant; nullify hides nothing", extra=("modStates",))
    e = v["expected"]
    assert e["N"] == ds("alice") and did("alice") not in e["S"] and e["G"] == ds("alice")
    assert st(v, pa) is None
    out.append(v)

    f = Forum(membership="approval", moderators=MODS)
    recs = [f.membership("alice"), f.membership("bob"),
            f.modaction("mona", "access", did("alice"), level="banned"),
            f.modaction("adam", "access", did("alice"), level="member"),
            f.modaction("mona", "access", did("bob"), level="banned"),
            f.modaction("mike", "access", did("bob"), level="member")]
    v = V("grant-against-ban-by-rank", f, recs,
          "a higher rank's #access{member} overrides a ban and also grants (alice becomes a seed); at equal "
          "rank the ban wins (banned < member)")
    e = v["expected"]
    assert e["G"] == ds("alice") and e["B"] == ds("bob") and did("alice") in e["S"]
    assert e["P"] == ds("owner", "adam", "mona", "mike", "alice", "bob")
    out.append(v)

    f = Forum(membership="approval", moderators=(("adam", "admin"),))
    pa = f.post("alice", T0 + HOUR)
    g = f.modaction("mona", "access", did("alice"), level="member")
    v = V("grant-by-a-former-moderator-lapses", f, [f.membership("alice"), pa, g],
          "mona is no longer listed, so her grant lapses at once: alice is an applicant again and her "
          "content is Hidden(NotParticipant)", extra=("modStates", "modLog"))
    assert v["expected"]["applicants"] == ds("alice") and st(v, pa) == hidden("NotParticipant")
    assert v["expected"]["modLog"] == [{"uri": u(g), "status": "lapsed"}]
    out.append(v)

    f = Forum(membership="approval", moderators=MODS)
    pa = f.post("alice", T0 + HOUR)
    v = V("leaving-keeps-the-grant", f, [pa, f.modaction("owner", "access", did("alice"), level="member")],
          "alice deleted her membership record: V2 hides her content (not a participant) but the grant "
          "still counts, so she remains a seed", extra=("modStates",))
    assert v["expected"]["S"] == ds("owner", "adam", "mona", "mike", "alice")
    assert st(v, pa) == hidden("NotParticipant")
    out.append(v)

    f = Forum(membership="open", moderators=(
        ("owner", "admin"), ("adam", "admin"), ("mona", "superuser"), ("mona", "moderator"),
        ("mike", "moderator"), ("adam", "moderator")))
    v = V("moderators-entry-rules", f, [f.membership("mona")],
          "FO2: the owner's own entry and an unknown role are ignored, and the first entry for a DID wins "
          "even when it is itself ignored (mona's 'superuser' entry), so mona is no authority; adam's "
          "later 'moderator' entry is ignored. The unknown role is an unknown knownValue of the "
          "declaration, so it raises the 'rules newer than your client' banner (§1.6; NOTES.md 25)",
          fields=("authorities", "P", "S", "newerRulesBanner"))
    assert v["expected"]["authorities"] == {did("owner"): 3, did("adam"): 2, did("mike"): 1}
    assert v["expected"]["newerRulesBanner"] is True
    assert v["expected"]["S"] == ds("owner", "adam", "mike")
    out.append(v)

    f = Forum(membership=None, moderators=MODS)
    v = V("membership-absent-defaults-to-approval", f, [f.membership("alice")],
          "an absent membership field takes the lexicon default, approval (alice is an applicant)",
          fields=("mode", "P", "applicants", "newerRulesBanner"))
    assert v["expected"]["mode"] == "approval" and v["expected"]["newerRulesBanner"] is False
    out.append(v)

    f = Forum(membership="invite", moderators=MODS)
    v = V("unknown-membership-falls-back-to-approval", f, [f.membership("alice")],
          "an unknown membership value falls back to approval (data-model.md §13.3) with the 'rules newer "
          "than your client' banner (social.md §1.6; NOTES.md 25)", fields=("mode", "P", "applicants", "newerRulesBanner"))
    assert v["expected"]["mode"] == "approval" and v["expected"]["newerRulesBanner"] is True
    out.append(v)

    f = Forum(membership="open", moderators=MODS)
    bad = f.membership("alice")
    bad = rec(bad["uri"], {k: x for k, x in bad["value"].items() if k != "createdAt"})
    note_m = f.membership("bob", note="Hello, I would like to join.", interests=["forecasting"])
    pa = f.post("alice", T0 + HOUR)
    v = V("invalid-membership-is-no-membership", f, [bad, note_m, pa],
          "alice's membership lacks the required createdAt (G2), so member(alice) is false and her post is "
          "Hidden(NotParticipant); bob's membership with a note and an unknown field is valid",
          extra=("modStates",), invalid={u(bad): "G2"})
    assert v["expected"]["members"] == ds("bob") and st(v, pa) == hidden("NotParticipant")
    out.append(v)

    f = Forum(membership="approval", moderators=MODS)
    v = V("banned-authority-keeps-its-authority", f,
          [f.modaction("adam", "access", did("mona"), level="banned"),
           f.modaction("owner", "restrict", did("adam"), nullifyVotes=True)],
          "an admin bans a moderator and the owner nullifies the admin: both leave S, and both stay "
          "authorities and participants (only the declaration confers authority)",
          fields=("authorities", "P", "B", "N", "S"))
    assert v["expected"]["S"] == ds("owner", "mike") and v["expected"]["P"] == ds("owner", "adam", "mona", "mike")
    out.append(v)

    return out


FILES = [
    ("modfold/participation.json", lambda: file_doc(
        "mod/1", "modfold",
        "social.md §3 (with §4.1 for A): membership modes, member(d), access, the sets P, G, B, N, S, "
        "applicants and newcomers, consent and leaving, and the moderators entry rules (FO2). These "
        "inputs contain no opinions, so E = S. input: {fold: FoldInput}. expected: the modfold "
        "projection fields listed (testdata/vectors/README.md, social section).",
        participation_vectors())),
]
