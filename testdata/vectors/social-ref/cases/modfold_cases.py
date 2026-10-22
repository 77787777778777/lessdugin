# SPDX-License-Identifier: CC0-1.0
"""modfold/forum.json (kind "modfold"): forum-tier moderation, social.md §4.1–§4.8, §4.10–§4.12.

Author-tier cases (§4.9) belong to the next stage (NOTES.md, TODO list).
"""

from __future__ import annotations

from lessdugin_ref.build import DAY, HOUR, MIN, NOW, T0, Forum, at, did, dt, rec, tid
from lessdugin_ref.common import NS

from .util import file_doc, vector

MODS = (("adam", "admin"), ("mona", "moderator"), ("mike", "moderator"))


def F(**kw) -> Forum:
    kw.setdefault("moderators", MODS)
    return Forum(**kw)


def V(name, f, records, note, *, extra=(), fields=None, invalid=None, fi=None, decl=None):
    fold_input = f.fold_input(records, decl=decl, **(fi or {}))
    return vector("modfold", name, fold_input, fields=fields, extra_fields=extra, note=note,
                  invalid=invalid)


def mem(f, *names):
    return [f.membership(n) for n in names]


def u(r):
    return r["uri"]


def removed(rank, actor, spam=False, reason=None):
    return {"state": "Hidden", "reason": "Removed",
            "removed": {"rank": rank, "actor": did(actor), "spam": spam, "reason": reason}}


def hidden(reason):
    return {"state": "Hidden", "reason": reason}


def res(value, rank, winner, **extra):
    return {"value": value, "rank": rank, "winner": u(winner), **extra}


def st(v, r):
    return v["expected"]["modStates"][u(r)]


def rs(v, subject, dim=None):
    s = subject if isinstance(subject, str) else u(subject)
    got = v["expected"]["resolved"].get(s, {})
    return got if dim is None else got.get(dim)


def log(v):
    return [(e["uri"], e["status"], e.get("rank")) for e in v["expected"]["modLog"]]


def status(v, r):
    for e in v["expected"]["modLog"]:
        if e["uri"] == u(r):
            return (e["status"], e.get("rank"))
    return None


def edited(f, r, t, **changes):
    """A new version of record r (same uri) with fields changed; returns (new, revision)."""
    value = dict(r["value"])
    value.update(changes)
    value["updatedAt"] = dt(t)
    new = rec(r["uri"], value)
    return new, f.revision(r, "update", t, record=r["value"])


def forum_vectors() -> list:
    out = []

    # ------------------------------------------------------------------ rank (§4.3, §4.4)
    f = F()
    p = f.post("alice", T0 + HOUR)
    keep = f.modaction("mona", "hide", p, hidden=False, t=T0 + 2 * HOUR)
    hide = f.modaction("owner", "hide", p, reason="off-topic", t=T0 + 3 * HOUR)
    v = V("owner-hide-beats-moderator-keep", f, [*mem(f, "alice"), p, keep, hide],
          "rank 3 beats rank 1: the owner's hide wins over the moderator's keep (§4.4: higher rank wins)")
    assert rs(v, p, "hidden") == res(True, 3, hide) and st(v, p) == removed(3, "owner", reason="off-topic")
    assert log(v) == [(u(hide), "effective", None), (u(keep), "overridden", 3)]
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    hide = f.modaction("mona", "hide", p, reason="spam?", t=T0 + 2 * HOUR)
    keep = f.modaction("owner", "hide", p, hidden=False, t=T0 + 3 * HOUR)
    v = V("owner-keep-beats-moderator-hide", f, [*mem(f, "alice"), p, hide, keep],
          "the owner's keep (hidden = false, CID-bound to the current version) resolves hidden = false at "
          "rank 3")
    assert rs(v, p, "hidden") == res(False, 3, keep) and st(v, p) is None
    assert log(v) == [(u(keep), "effective", None), (u(hide), "overridden", 3)]
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    hide = f.modaction("mona", "hide", p, t=T0 + 2 * HOUR)
    keep = f.modaction("mike", "hide", p, hidden=False, t=T0 + 3 * HOUR)
    v = V("same-rank-hide-and-keep-gives-hide", f, [*mem(f, "alice"), p, hide, keep],
          "two moderators (rank 1) disagree: the safest value (hidden = true) wins; a restore cannot beat "
          "an equal-rank hide (two-person rule)")
    assert rs(v, p, "hidden") == res(True, 1, hide) and st(v, p) == removed(1, "mona")
    assert status(v, keep) == ("overridden", 1) and status(v, hide) == ("effective", None)
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    h1 = f.modaction("mona", "hide", p, reason="spam link", t=T0 + 2 * HOUR)
    h2 = f.modaction("mike", "hide", p, reason="duplicate", t=T0 + 2 * HOUR)
    v = V("equal-rank-winner-is-the-smallest-uri", f, [*mem(f, "alice"), p, h1, h2],
          "both rank-1 records hide; the winner is the smallest uri (mike's DID sorts before mona's) and "
          "the shown reason comes from the winner")
    assert u(h2) < u(h1) and st(v, p) == removed(1, "mike", reason="duplicate")
    assert status(v, h1) == ("effective", None) and status(v, h2) == ("effective", None)
    assert log(v) == sorted(log(v), key=lambda e: e[0].encode())
    out.append(v)

    f = F()
    p1, p2, p3 = (f.post("alice", T0 + i * HOUR) for i in (1, 2, 3))
    h1 = f.modaction("mona", "hide", p1, reason="r1", showReason=False, t=T0 + 4 * HOUR)
    h2 = f.modaction("mona", "hide", p2, reason="r2", t=T0 + 5 * HOUR)
    h3 = f.modaction("mona", "hide", p3, showReason=True, t=T0 + 6 * HOUR)
    v = V("reason-shown-only-with-showReason", f, [*mem(f, "alice"), p1, p2, p3, h1, h2, h3],
          "Removed carries the winner's reason only if its showReason (default true) holds; a hide "
          "without reason shows null")
    assert st(v, p1) == removed(1, "mona") and st(v, p2) == removed(1, "mona", reason="r2")
    assert st(v, p3) == removed(1, "mona")
    out.append(v)

    f = F()
    p1, p2, p3 = (f.post("alice", T0 + i * HOUR) for i in (1, 2, 3))
    s1 = f.modaction("mona", "hide", p1, spam=True)
    k2 = f.modaction("mike", "hide", p2, hidden=False, spam=True)
    s3 = f.modaction("mona", "hide", p3, spam=True)
    h3 = f.modaction("adam", "hide", p3)
    v = V("spam-is-hidden-and-spam", f, [*mem(f, "alice"), p1, p2, p3, s1, k2, s3, h3],
          "#hide asserts spam = hidden ∧ spam: a hide with spam asserts spam = true; a keep with spam = "
          "true asserts spam = false; at a higher rank a plain hide resolves spam = false. Removed.spam is "
          "effective(x, spam) (NOTES.md)")
    assert rs(v, p1, "spam") == res(True, 1, s1) and st(v, p1) == removed(1, "mona", spam=True)
    assert rs(v, p2, "spam") == res(False, 1, k2) and st(v, p2) is None
    assert rs(v, p3, "spam") == res(False, 2, h3) and st(v, p3) == removed(2, "adam")
    assert status(v, s3) == ("overridden", 2)
    out.append(v)

    f = F()
    pa = f.post("adam", T0 + HOUR)
    pm = f.post("mike", T0 + 2 * HOUR)
    pmo = f.post("mona", T0 + 3 * HOUR)
    po = f.post("owner", T0 + 4 * HOUR)
    m_a = f.modaction("mona", "hide", pa, t=T0 + 5 * HOUR)
    m_m = f.modaction("mona", "hide", pm, t=T0 + 5 * HOUR)
    m_mo = f.modaction("mona", "hide", pmo, t=T0 + 5 * HOUR)
    m_o = f.modaction("mona", "hide", po, t=T0 + 5 * HOUR)
    a_m = f.modaction("adam", "hide", pm, t=T0 + 6 * HOUR)
    o_a = f.modaction("owner", "hide", pa, t=T0 + 7 * HOUR)
    v = V("rank-rule", f, [pa, pm, pmo, po, m_a, m_m, m_mo, m_o, a_m, o_a],
          "X = owner ∨ X = subjectAuthor(T) ∨ rank(X) > subjectRank(T): a moderator cannot act on an "
          "admin's, another moderator's or the owner's post, but can on her own; an admin can on a "
          "moderator's; the owner on anyone")
    assert [status(v, r) for r in (m_a, m_m, m_mo, m_o)] == [("unauthorised", None)] * 2 + \
        [("effective", None), ("unauthorised", None)]
    assert st(v, pa) == removed(3, "owner") and st(v, pm) == removed(2, "adam")
    assert st(v, pmo) == removed(1, "mona") and st(v, po) is None
    out.append(v)

    # ------------------------------------------------------------------ roles (§4.3 table, §4.10)
    f = F()
    p1, p2, p3 = (f.post("alice", T0 + i * HOUR) for i in (1, 2, 3))
    t1 = f.modaction("mona", "tier", p1, tier="curated")
    t2 = f.modaction("mona", "tier", p2, tier="frontpage")
    t3 = f.modaction("adam", "tier", p3, tier="curated")
    v = V("moderator-cannot-curate", f, [*mem(f, "alice"), p1, p2, p3, t1, t2, t3],
          "#tier curated needs an owner or admin; a moderator may set personal or frontpage",
          extra=("tiers",))
    assert status(v, t1) == ("unauthorised", None)
    assert v["expected"]["tiers"] == {u(p1): "personal", u(p2): "frontpage", u(p3): "curated"}
    out.append(v)

    f = F(moderators=(("adam", "moderator"), ("mona", "moderator")))
    p1, p2 = (f.post("alice", T0 + i * HOUR) for i in (1, 2))
    t1 = f.modaction("adam", "tier", p1, tier="curated")
    t2 = f.modaction("adam", "tier", p2, tier="frontpage")
    v = V("demoted-admin-curation-stops", f, [*mem(f, "alice"), p1, p2, t1, t2],
          "adam is now listed as a moderator: the role is re-evaluated at once, so his curation is "
          "unauthorised (the tier falls back to defaultTier) while his frontpage tier still counts",
          extra=("tiers", "authorities"))
    assert status(v, t1) == ("unauthorised", None) and status(v, t2) == ("effective", None)
    assert v["expected"]["tiers"] == {u(p1): "personal", u(p2): "frontpage"}
    out.append(v)

    f = F(moderators=(("adam", "admin"), ("mike", "moderator")))
    p = f.post("alice", T0 + HOUR)
    q = f.post("bob", T0 + 2 * HOUR)
    h = f.modaction("mona", "hide", p, t=T0 + 3 * HOUR)
    pin = f.modaction("mona", "pin", q, pin_scope="forum", t=T0 + 4 * HOUR)
    v = V("lapse-on-removal-from-the-declaration", f, [*mem(f, "alice", "bob"), p, q, h, pin],
          "mona is no longer in decl.moderators: her assertions have no forum-tier effect at once and the "
          "mod log shows them as lapsed",
          extra=("authorities",))
    assert status(v, h) == ("lapsed", None) and status(v, pin) == ("lapsed", None)
    assert st(v, p) is None and v["expected"]["resolved"] == {}
    out.append(v)

    f = F(moderators=(("adam", "admin"), ("mike", "moderator")))
    p = f.post("alice", T0 + HOUR)
    q = f.post("bob", T0 + 2 * HOUR)
    h = f.modaction("mona", "hide", p, reason="rule 3", t=T0 + 3 * HOUR)
    pin = f.modaction("mona", "pin", q, pin_scope="forum", t=T0 + 4 * HOUR)
    ah = f.modaction("adam", "hide", p, reason="rule 3", adopts=h, t=T0 + 5 * HOUR)
    apin = f.modaction("adam", "pin", q, pin_scope="forum", adopts=pin, t=T0 + 5 * HOUR)
    v = V("adopt-restores-lapsed-assertions", f, [*mem(f, "alice", "bob"), p, q, h, pin, ah, apin],
          "mod adopt re-issued mona's assertions under adam (same rkey in adam's repo, adopts pointing at "
          "the originals): the copies count at adam's rank; the originals stay lapsed")
    assert status(v, ah) == ("effective", None) and status(v, h) == ("lapsed", None)
    assert st(v, p) == removed(2, "adam", reason="rule 3") and rs(v, q, "pin.forum") == res(True, 2, apin)
    out.append(v)

    f = F(moderators=(("mona", "moderator"), ("mike", "moderator")))
    p1 = f.post("alice", T0 + HOUR)
    p2_v1 = f.post("alice", T0 + 2 * HOUR, clock=7)
    p3 = f.post("alice", T0 + 3 * HOUR)
    cur = f.modaction("adam", "tier", p1, tier="curated", t=T0 + 4 * HOUR)
    kp = f.modaction("adam", "hide", p2_v1, hidden=False, t=T0 + 4 * HOUR)
    ex = f.modaction("adam", "hide", p3, until=NOW + DAY, t=T0 + 4 * HOUR)
    p2, rev = edited(f, p2_v1, T0 + 5 * HOUR, title="Edited")
    m_cur = f.modaction("mona", "tier", p1, tier="curated", adopts=cur, t=T0 + 6 * HOUR)
    m_kp = f.modaction("mike", "hide", p2_v1, hidden=False, adopts=kp, t=T0 + 6 * HOUR)
    m_ex = f.modaction("mona", "hide", p3, until=NOW - HOUR, adopts=ex, t=T0 + 6 * HOUR)
    v = V("adopted-copy-counts-on-its-own-merits", f,
          [*mem(f, "alice"), p1, p2, p3, rev, cur, kp, ex, m_cur, m_kp, m_ex],
          "adam (former admin) is gone; copies of his assertions count only on their own merits: mona "
          "(moderator) cannot curate (unauthorised); mike's copy of a keep carries the pre-edit "
          "subjectCid (stale); mona's copy has its own until in the past (expired). adopts is "
          "informational", extra=("tiers", "modQueue"))
    assert [status(v, r) for r in (m_cur, m_kp, m_ex)] == [("unauthorised", None), ("stale", None),
                                                           ("expired", None)]
    assert all(status(v, r) == ("lapsed", None) for r in (cur, kp, ex))
    assert v["expected"]["modQueue"] == [{"type": "stale", "uri": u(m_kp)}]
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    q = f.post("bob", T0 + 2 * HOUR)
    h = f.modaction("alice", "hide", q, t=T0 + 3 * HOUR)
    v = V("non-authority-assertion-is-lapsed", f, [*mem(f, "alice", "bob"), p, q, h],
          "a forum-scope assertion by an actor outside A has no effect; the fold cannot tell a former "
          "moderator from a user who never was one, so the mod log shows lapsed (NOTES.md)")
    assert status(v, h) == ("lapsed", None) and st(v, q) is None
    out.append(v)

    # ------------------------------------------------------------------ until (§4.3 rule 5)
    f = F()
    p1, p2, p3 = (f.post("alice", T0 + i * HOUR) for i in (1, 2, 3))
    h1 = f.modaction("mona", "hide", p1, until=NOW + 1)
    h2 = f.modaction("mona", "hide", p2, until=NOW)
    h3 = f.modaction("mona", "hide", p3, until=NOW - HOUR)
    v = V("until-expiry", f, [*mem(f, "alice"), p1, p2, p3, h1, h2, h3],
          "an assertion counts while µs(until) > now: until = now + 1 µs counts; until = now and earlier "
          "have expired")
    assert st(v, p1) == removed(1, "mona") and st(v, p2) is None and st(v, p3) is None
    assert [status(v, r)[0] for r in (h1, h2, h3)] == ["effective", "expired", "expired"]
    out.append(v)

    f = F()
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    ban = f.modaction("mona", "access", did("alice"), level="banned", until=NOW - MIN)
    rst = f.modaction("mike", "restrict", did("bob"), capabilities=["post"], until=NOW + MIN)
    v = V("ban-and-restrict-until", f, [*mem(f, "alice", "bob"), pa, pb, ban, rst],
          "an expired ban has no effect (alice is not in B); a restriction with a future until still hides",
          extra=("B",))
    assert v["expected"]["B"] == [] and st(v, pa) is None and st(v, pb) == hidden("Restricted")
    out.append(v)

    # ------------------------------------------------------------------ CID binding (§4.5)
    f = F()
    p_v1 = f.post("alice", T0 + HOUR)
    f.extra_obs[(p_v1["uri"], p_v1["cid"])] = T0 + HOUR
    pin = f.modaction("mona", "pin", p_v1, pin_scope="forum", t=T0 + 2 * HOUR)
    tier = f.modaction("adam", "tier", p_v1, tier="curated", t=T0 + 2 * HOUR)
    keep = f.modaction("adam", "hide", p_v1, hidden=False, t=T0 + 2 * HOUR)
    hide = f.modaction("mona", "hide", p_v1, reason="later edit is spam", t=T0 + 4 * HOUR)
    p, rev = edited(f, p_v1, T0 + 3 * HOUR, title="Edited")
    v = V("pin-tier-and-keep-stop-after-an-edit", f, [*mem(f, "alice"), p, rev, pin, tier, keep, hide],
          "alice edited the post after the pin, the curation and the keep: their subjectCid names the "
          "previous version, so all three are stale (excluded from resolve; mod-queue items); the pin "
          "drops, the tier falls back to defaultTier and the lower-rank hide now governs. The hide is "
          "negative, so its (old) subjectCid does not matter",
          extra=("tiers", "modQueue"))
    assert rs(v, p) == {"hidden": res(True, 1, hide), "spam": res(False, 1, hide)}
    assert st(v, p) == removed(1, "mona", reason="later edit is spam")
    assert v["expected"]["tiers"] == {u(p): "personal"}
    assert v["expected"]["modQueue"] == [{"type": "stale", "uri": x} for x in sorted(
        (u(pin), u(tier), u(keep)), key=str.encode)]
    out.append(v)

    f = F()
    p_v1 = f.post("alice", T0 + HOUR)
    keep = f.modaction("mike", "hide", p_v1, hidden=False, t=T0 + 2 * HOUR)
    p, rev = edited(f, p_v1, T0 + 3 * HOUR, title="Edited")
    tier = f.modaction("adam", "tier", p, tier="curated", t=T0 + 4 * HOUR)
    pin = f.modaction("mona", "pin", p, pin_scope="forum", t=T0 + 4 * HOUR)
    v = V("reasserted-on-the-new-version", f, [*mem(f, "alice"), p, rev, keep, tier, pin],
          "assertions whose subjectCid is the current CID count; the keep still names the old version "
          "and stays stale", extra=("tiers", "modQueue"))
    assert rs(v, p, "tier") == res("curated", 2, tier) and rs(v, p, "pin.forum") == res(True, 1, pin)
    assert v["expected"]["modQueue"] == [{"type": "stale", "uri": u(keep)}]
    out.append(v)

    f = F()
    p_v1 = f.post("alice", T0 + HOUR)
    q = f.post("bob", T0 + 2 * HOUR)
    p, rev = edited(f, p_v1, T0 + 3 * HOUR, title="Edited")
    tier = f.modaction("mona", "tier", p_v1, tier="personal")
    unpin = f.modaction("mona", "pin", p_v1, pin_scope="forum", pinned=False)
    hide = f.modaction("mike", "hide", q, subject_cid=False)
    ban = f.modaction("mona", "access", did("bob"), level="banned", subject_cid=p["cid"])
    v = V("negative-assertions-ignore-subjectCid", f, [*mem(f, "alice", "bob"), p, q, rev, tier, unpin,
                                                       hide, ban],
          "negative assertions (tier personal, pinned = false, hide) count whatever their subjectCid, "
          "even without one; on a DID subject subjectCid is ignored", extra=("tiers", "B"))
    assert rs(v, p, "tier") == res("personal", 1, tier) and rs(v, p, "pin.forum") == res(False, 1, unpin)
    assert st(v, q) == removed(1, "mike") and v["expected"]["B"] == [did("bob")]
    out.append(v)

    f = F()
    ghost = f.post("alice", T0 + HOUR)
    p = f.post("alice", T0 + 2 * HOUR)
    pin = f.modaction("mona", "pin", ghost, pin_scope="forum")
    keep = f.modaction("adam", "hide", ghost, hidden=False)
    tier = f.modaction("adam", "tier", ghost, tier="frontpage")
    v = V("positive-assertion-on-a-subject-not-in-records-never-counts", f,
          [*mem(f, "alice"), p, pin, keep, tier],
          "the subject post is not in records (not yet synced, or deleted): positive assertions on it "
          "never count; they are shown as stale but are not mod-queue items (nothing was edited)",
          extra=("modQueue",))
    assert v["expected"]["resolved"] == {} and v["expected"]["modQueue"] == []
    assert all(status(v, r) == ("stale", None) for r in (pin, keep, tier))
    out.append(v)

    # ------------------------------------------------------------------ ban (§4.6)
    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    ban = f.modaction("mona", "access", did("alice"), level="banned", reason="harassment")
    keep = f.modaction("mike", "hide", p1, hidden=False)
    v = V("keep-at-equal-rank-beats-ban", f, [*mem(f, "alice"), p1, p2, ban, keep],
          "keepRank(p1) = 1 ≥ banRank(alice) = 1: the kept post stays visible; her other post is "
          "Hidden(Banned)", extra=("B", "S"))
    assert st(v, p1) is None and st(v, p2) == hidden("Banned") and v["expected"]["B"] == [did("alice")]
    out.append(v)

    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    p3 = f.post("alice", T0 + 3 * HOUR)
    ban = f.modaction("adam", "access", did("alice"), level="banned")
    k1 = f.modaction("mona", "hide", p1, hidden=False)
    k2 = f.modaction("owner", "hide", p2, hidden=False)
    v = V("keep-at-lower-rank-loses-to-ban", f, [*mem(f, "alice"), p1, p2, p3, ban, k1, k2],
          "banRank(alice) = 2: a moderator's keep (1) does not exempt p1; the owner's keep (3) exempts p2")
    assert st(v, p1) == hidden("Banned") and st(v, p2) is None and st(v, p3) == hidden("Banned")
    out.append(v)

    f = F()
    q = f.post("bob", T0 + HOUR)
    p = f.post("alice", T0 + 2 * HOUR)
    c = f.comment("alice", q, T0 + 3 * HOUR)
    s = f.sequence("alice", T0 + 4 * HOUR, chapters=([p["uri"]],))
    w = f.wiki("alice", T0 + 5 * HOUR, "decision-theory")
    ban = f.modaction("mona", "access", did("alice"), level="banned")
    v = V("ban-hides-every-item-kind", f, [*mem(f, "alice", "bob"), q, p, c, s, w, ban],
          "every item the banned user authored in F (post, comment, sequence, wikiRevision) is "
          "Hidden(Banned), whatever its claimed time")
    assert all(st(v, r) == hidden("Banned") for r in (p, c, s, w)) and st(v, q) is None
    out.append(v)

    f = F()
    pm = f.post("mona", T0 + HOUR)
    pa = f.post("alice", T0 + 2 * HOUR)
    ban = f.modaction("adam", "access", did("mona"), level="banned")
    h = f.modaction("mona", "hide", pa)
    v = V("banning-a-lower-ranked-authority", f, [*mem(f, "alice"), pm, pa, ban, h],
          "an admin may ban a moderator: her content is Hidden(Banned) and she leaves S, but she keeps "
          "her authority (only the declaration confers it), so her hide still counts",
          extra=("authorities", "B", "S"))
    assert st(v, pm) == hidden("Banned") and st(v, pa) == removed(1, "mona")
    assert did("mona") not in v["expected"]["S"] and v["expected"]["authorities"][did("mona")] == 1
    out.append(v)

    # ------------------------------------------------------------------ restrict (§4.7)
    f = F()
    p_old = f.post("alice", T0 + DAY)
    p_new = f.post("alice", T0 + 20 * DAY)
    c = f.comment("alice", p_old, T0 + 2 * DAY)
    rst = f.modaction("mona", "restrict", did("alice"), capabilities=["post"], t=T0 + 10 * DAY)
    v = V("restrict-hides-retroactively", f, [*mem(f, "alice"), p_old, p_new, c, rst],
          "a #restrict{post} written on day 10 hides alice's posts claimed before and after it (rule 4b: "
          "time is self-asserted, so it is retroactive); her comment is not restricted")
    assert st(v, p_old) == hidden("Restricted") and st(v, p_new) == hidden("Restricted") and st(v, c) is None
    out.append(v)

    f = F()
    a1, a2, a3 = (f.post("alice", T0 + i * HOUR) for i in (1, 2, 3))
    q = f.post("carol", T0 + 4 * HOUR)
    bc = f.comment("bob", q, T0 + 5 * HOUR)
    r_a = f.modaction("mona", "restrict", did("alice"), capabilities=["post"])
    k1 = f.modaction("adam", "hide", a1, hidden=False)
    k2 = f.modaction("mike", "hide", a2, hidden=False)
    r_b = f.modaction("adam", "restrict", did("bob"), capabilities=["comment"])
    k3 = f.modaction("mona", "hide", bc, hidden=False)
    v = V("restrict-overridden-by-a-keep-at-higher-or-equal-rank", f,
          [*mem(f, "alice", "bob", "carol"), a1, a2, a3, q, bc, r_a, k1, k2, r_b, k3],
          "keepRank(item) ≥ restrictRank(author) exempts the item: adam's keep (2) and mike's keep (1) "
          "exempt alice's posts from mona's restriction (1); mona's keep (1) does not exempt bob's comment "
          "from adam's restriction (2)")
    assert st(v, a1) is None and st(v, a2) is None and st(v, a3) == hidden("Restricted")
    assert st(v, bc) == hidden("Restricted")
    out.append(v)

    f = F()
    q = f.post("bob", T0 + HOUR)
    p = f.post("alice", T0 + 2 * HOUR)
    c = f.comment("alice", q, T0 + 3 * HOUR)
    s = f.sequence("alice", T0 + 4 * HOUR, chapters=([q["uri"]],))
    w = f.wiki("alice", T0 + 5 * HOUR, "epistemics")
    r_a = f.modaction("mona", "restrict", did("alice"),
                      capabilities=["comment", "sequence", "wiki", "comment", "dance"])
    r_b = f.modaction("mona", "restrict", did("bob"), capabilities=["vote", "react", "tag"])
    v = V("restrict-capabilities-map-to-collections", f, [*mem(f, "alice", "bob"), q, p, c, s, w, r_a, r_b],
          "comment, sequence and wiki hide those collections (cap(x)); post is not restricted here; the "
          "unknown capability 'dance' is ignored and the duplicate collapses (MA8); vote, react and tag "
          "hide nothing (they act on opinions, L3/L5)")
    assert st(v, p) is None and st(v, q) is None
    assert all(st(v, r) == hidden("Restricted") for r in (c, s, w))
    assert rs(v, did("alice"), "restrict") == res(["comment", "sequence", "wiki"], 1, r_a)
    assert rs(v, did("bob"), "restrict") == res(["react", "tag", "vote"], 1, r_b)
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    q = f.post("carol", T0 + 2 * HOUR)
    c = f.comment("alice", q, T0 + 3 * HOUR)
    r1 = f.modaction("mona", "restrict", did("alice"), capabilities=["post"])
    r2 = f.modaction("mike", "restrict", did("alice"), capabilities=["comment"])
    v = V("restrict-equal-rank-sets-unite", f, [*mem(f, "alice", "carol"), p, q, c, r1, r2],
          "two rank-1 restrictions unite their sets; the winner of the set dimension is the smallest uri "
          "among all rank-1 records (mike's)")
    assert u(r2) < u(r1) and rs(v, did("alice"), "restrict") == res(["comment", "post"], 1, r2)
    assert st(v, p) == hidden("Restricted") and st(v, c) == hidden("Restricted")
    assert status(v, r1) == ("effective", None) and status(v, r2) == ("effective", None)
    out.append(v)

    f = F()
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    ra = f.modaction("mona", "restrict", did("alice"), capabilities=["post", "comment"], nullifyVotes=True)
    lift = f.modaction("adam", "restrict", did("alice"))
    rb = f.modaction("mona", "restrict", did("bob"), capabilities=["post"])
    keep_b = f.modaction("mike", "restrict", did("bob"), capabilities=[])
    v = V("empty-restrict-at-higher-rank-lifts", f, [*mem(f, "alice", "bob"), pa, pb, ra, lift, rb, keep_b],
          "a #restrict states the complete set: adam's empty record (rank 2) resolves restrict = ∅ and "
          "nullify = false, lifting mona's restriction and nullification of alice; at equal rank an empty "
          "set only unites, so mike's empty record does not lift mona's restriction of bob",
          extra=("N",))
    assert rs(v, did("alice")) == {"restrict": res([], 2, lift), "nullify": res(False, 2, lift)}
    assert st(v, pa) is None and st(v, pb) == hidden("Restricted") and v["expected"]["N"] == []
    assert status(v, ra) == ("overridden", 2) and status(v, keep_b) == ("effective", None)
    out.append(v)

    # ------------------------------------------------------------------ tags (§4.1 tag: subjects, §4.4 tag.head)
    f = F()
    r1 = f.wiki("alice", T0 + HOUR, "decision-theory")
    t_rm = f.modaction("mona", "tag", "tag:epistemics", removed=True)
    t_head = f.modaction("mike", "tag", "tag:decision-theory", head={"uri": r1["uri"], "cid": r1["cid"]})
    t_user = f.modaction("alice", "tag", "tag:decision-theory", removed=True)
    v = V("tag-subject-by-a-moderator", f, [*mem(f, "alice"), r1, t_rm, t_head, t_user],
          "tag: subjects have rank 0, so every authority (here moderators) can assert tag.removed and "
          "tag.head; a user's #tag is lapsed")
    assert rs(v, "tag:epistemics", "tag.removed") == res(True, 1, t_rm)
    assert rs(v, "tag:decision-theory") == {"tag.removed": res(False, 1, t_head),
                                            "tag.head": res({"uri": u(r1), "cid": r1["cid"]}, 1, t_head)}
    assert status(v, t_user) == ("lapsed", None)
    out.append(v)

    f = F()
    r1 = f.wiki("alice", T0 + HOUR, "decision-theory")
    r2 = f.wiki("bob", T0 + 2 * HOUR, "decision-theory", parents=[r1])
    h1 = f.modaction("mona", "tag", "tag:decision-theory", head={"uri": r1["uri"], "cid": r1["cid"]})
    h2 = f.modaction("mike", "tag", "tag:decision-theory", head={"uri": r2["uri"], "cid": r2["cid"]})
    v = V("tag-head-conflict-at-equal-rank", f, [*mem(f, "alice", "bob"), r1, r2, h1, h2],
          "tag.head is unordered: two different rank-1 heads give Default (no override) and a mod-queue "
          "conflict item; tag.removed = false still resolves. Both records show effective in the mod log "
          "through tag.removed (NOTES.md)", extra=("modQueue",))
    assert rs(v, "tag:decision-theory") == {"tag.removed": res(False, 1, h2)}
    assert v["expected"]["modQueue"] == [{"type": "conflict", "subject": "tag:decision-theory"}]
    out.append(v)

    f = F()
    r1 = f.wiki("alice", T0 + HOUR, "decision-theory")
    r2 = f.wiki("bob", T0 + 2 * HOUR, "decision-theory", parents=[r1])
    h1 = f.modaction("mona", "tag", "tag:decision-theory", head={"uri": r1["uri"], "cid": r1["cid"]})
    h2 = f.modaction("mike", "tag", "tag:decision-theory", head={"uri": r2["uri"], "cid": r2["cid"]})
    h3 = f.modaction("adam", "tag", "tag:decision-theory", head={"uri": r2["uri"], "cid": r2["cid"]})
    v = V("tag-head-higher-rank-settles-a-conflict", f, [*mem(f, "alice", "bob"), r1, r2, h1, h2, h3],
          "the admin's head (rank 2) resolves tag.head; the moderators' records are overridden",
          extra=("modQueue",))
    assert rs(v, "tag:decision-theory", "tag.head") == res({"uri": u(r2), "cid": r2["cid"]}, 2, h3)
    assert v["expected"]["modQueue"] == [] and status(v, h1) == ("overridden", 2)
    out.append(v)

    f = F()
    r1 = f.wiki("alice", T0 + HOUR, "decision-theory")
    h1 = f.modaction("mona", "tag", "tag:decision-theory", head={"uri": r1["uri"], "cid": r1["cid"]})
    h2 = f.modaction("mike", "tag", "tag:decision-theory", head={"uri": r1["uri"], "cid": r1["cid"]},
                     removed=True)
    v = V("tag-head-equal-values-agree", f, [*mem(f, "alice"), r1, h1, h2],
          "equal heads at equal rank resolve (winner: smallest uri); tag.removed resolves to the safest "
          "value, true, from mike's record")
    assert rs(v, "tag:decision-theory") == {
        "tag.removed": res(True, 1, h2), "tag.head": res({"uri": u(r1), "cid": r1["cid"]}, 1, h2)}
    out.append(v)

    f = F()
    r1 = f.wiki("alice", T0 + HOUR, "decision-theory")
    r2 = f.wiki("bob", T0 + 2 * HOUR, "decision-theory", parents=[r1])
    gone = f.wiki("carol", T0 + 3 * HOUR, "epistemics")
    h1 = f.modaction("mona", "tag", "tag:decision-theory", head={"uri": r1["uri"], "cid": r2["cid"]})
    h2 = f.modaction("mike", "tag", "tag:epistemics", head={"uri": gone["uri"], "cid": gone["cid"]})
    v = V("tag-head-bound-by-its-cid", f, [*mem(f, "alice", "bob"), r1, r2, h1, h2],
          "#tag.head counts only if records[head.uri] exists with CID head.cid: mona's head names r1 with "
          "r2's CID (stale, a mod-queue item), mike's names a revision that is not in records (stale, no "
          "queue item); their tag.removed = false still counts", extra=("modQueue",))
    assert rs(v, "tag:decision-theory") == {"tag.removed": res(False, 1, h1)}
    assert rs(v, "tag:epistemics") == {"tag.removed": res(False, 1, h2)}
    assert v["expected"]["modQueue"] == [{"type": "stale", "uri": u(h1)}]
    out.append(v)

    # ------------------------------------------------------------------ lock (§4.8)
    f = F()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("bob", p, T0 + 2 * HOUR)
    c2 = f.comment("carol", p, T0 + 3 * HOUR)
    c3 = f.comment("mike", p, T0 + 4 * HOUR)
    c4 = f.comment("dave", p, T0 + 5 * HOUR)
    c5 = f.comment("erin", p, T0 + 6 * HOUR)
    c6 = f.comment("alice", p, T0 + 7 * HOUR)
    lock = f.modaction("mona", "lock", p, allowed=[c1["uri"]], t=T0 + 2 * HOUR + MIN)
    k4 = f.modaction("adam", "hide", c4, hidden=False)
    k5 = f.modaction("mike", "hide", c5, hidden=False)
    v = V("lock-hides-comments-outside-allowed", f,
          [*mem(f, "alice", "bob", "carol", "dave", "erin"), p, c1, c2, c3, c4, c5, c6, lock, k4, k5],
          "a forum-tier lock (rank 1) hides every comment of the thread not in allowed, except those by an "
          "authority (mike) and those kept at rank ≥ lockRank (adam's keep, rank 2; mike's keep, rank 1); "
          "the post's author is not exempt from a forum-tier lock")
    assert rs(v, p, "locked") == res(True, 1, lock, allowed=[u(c1)])
    assert [st(v, c) for c in (c1, c2, c3, c4, c5, c6)] == [None, hidden("Locked"), None, None, None,
                                                            hidden("Locked")]
    assert st(v, p) is None
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    c1, c2, c3 = (f.comment(n, p, T0 + i * HOUR) for i, n in ((2, "bob"), (3, "carol"), (4, "dave")))
    l1 = f.modaction("mona", "lock", p, allowed=sorted([c1["uri"], c2["uri"]], key=str.encode))
    l2 = f.modaction("mike", "lock", p, allowed=sorted([c2["uri"], c3["uri"]], key=str.encode))
    v = V("lock-allow-lists-intersect-at-equal-rank", f, [*mem(f, "bob", "carol", "dave"), p, c1, c2, c3,
                                                          l1, l2],
          "allowed(p) is the intersection of the allow-lists of the rank-r records that lock")
    assert rs(v, p, "locked")["allowed"] == [u(c2)]
    assert [st(v, c) for c in (c1, c2, c3)] == [hidden("Locked"), None, hidden("Locked")]
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    q = f.post("bob", T0 + 2 * HOUR)
    c1 = f.comment("carol", p, T0 + 3 * HOUR)
    cq = f.comment("carol", q, T0 + 4 * HOUR)
    gone = f.comment_uri("dave", p["uri"], T0 + 5 * HOUR, 9)
    lock = f.modaction("mona", "lock", p, allowed=sorted([c1["uri"], cq["uri"], gone], key=str.encode))
    v = V("lock-allowed-entries-outside-the-thread-ignored", f,
          [*mem(f, "bob", "carol"), p, q, c1, cq, lock],
          "MA9: an allowed entry whose rkey is not {fk}-{threadTid(p)}-… (a comment of another thread) is "
          "ignored; an entry in p's key range is kept even if that comment is not in records")
    assert rs(v, p, "locked")["allowed"] == sorted([u(c1), gone], key=str.encode)
    assert st(v, cq) is None and st(v, c1) is None
    out.append(v)

    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    c1 = f.comment("bob", p1, T0 + 3 * HOUR)
    c2 = f.comment("bob", p2, T0 + 4 * HOUR)
    l1 = f.modaction("mona", "lock", p1)
    u1 = f.modaction("adam", "lock", p1, locked=False)
    l2 = f.modaction("mona", "lock", p2)
    u2 = f.modaction("mike", "lock", p2, locked=False)
    v = V("unlock-needs-a-higher-rank", f, [*mem(f, "alice", "bob"), p1, p2, c1, c2, l1, u1, l2, u2],
          "a higher rank's #lock{locked: false} unlocks p1; an equal-rank unlock loses to the lock (the "
          "safe value), and allowed comes only from the rank-r records that lock")
    assert rs(v, p1, "locked") == res(False, 2, u1) and st(v, c1) is None
    assert rs(v, p2, "locked") == res(True, 1, l2, allowed=[]) and st(v, c2) == hidden("Locked")
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    c = f.comment("bob", p, T0 + 2 * HOUR)
    flock = f.modaction("owner", "lock", f.uri)
    alock = f.modaction("adam", "lock", f.uri, locked=True, allowed=[])
    v = V("forum-uri-lock-has-no-effect", f, [*mem(f, "alice", "bob"), p, c, flock, alock],
          "a #lock whose subject is the forum uri is reserved (P2, MA9): valid, without effect for any "
          "actor, shown as reserved/unknown")
    assert v["expected"]["resolved"] == {} and st(v, c) is None
    assert status(v, flock) == ("reserved/unknown", None)
    out.append(v)

    # ------------------------------------------------------------------ pins, tiers
    f = F()
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    c = f.comment("bob", p1, T0 + 3 * HOUR)
    s = f.sequence("alice", T0 + 4 * HOUR, chapters=([p1["uri"]],))
    pin1 = f.modaction("mona", "pin", p1, pin_scope="forum")
    unpin1 = f.modaction("mike", "pin", p1, pin_scope="forum", pinned=False)
    pin2 = f.modaction("mona", "pin", p2, pin_scope="forum")
    unpin2 = f.modaction("adam", "pin", p2, pin_scope="forum", pinned=False)
    pinc = f.modaction("mona", "pin", c, pin_scope="thread")
    pins = f.modaction("mike", "pin", s, pin_scope="forum")
    v = V("pins", f, [*mem(f, "alice", "bob"), p1, p2, c, s, pin1, unpin1, pin2, unpin2, pinc, pins],
          "pin[s] is ordered false < true, so an equal-rank unpin beats a pin (p1) and so does a higher one "
          "(p2); thread pins apply to comments and forum pins to posts and sequences")
    assert rs(v, p1, "pin.forum") == res(False, 1, unpin1) and rs(v, p2, "pin.forum") == res(False, 2, unpin2)
    assert rs(v, c, "pin.thread") == res(True, 1, pinc) and rs(v, s, "pin.forum") == res(True, 1, pins)
    out.append(v)

    f = F(defaultTier="frontpage")
    p1 = f.post("alice", T0 + HOUR)
    p2 = f.post("alice", T0 + 2 * HOUR)
    t1 = f.modaction("mona", "tier", p1, tier="personal")
    v = V("default-tier-frontpage", f, [*mem(f, "alice"), p1, p2, t1],
          "decl.defaultTier = frontpage is the tier of a post with no resolved #tier", extra=("tiers",))
    assert v["expected"]["tiers"] == {u(p1): "personal", u(p2): "frontpage"}
    out.append(v)

    # ------------------------------------------------------------------ no effect, invalid (MA2, MA7, MA*)
    f = F()
    p = f.post("alice", T0 + HOUR)
    x1 = f.modaction("mona", f"{NS}.modAction#slowmode", p, seconds=60)
    x2 = f.modaction("mona", "com.example.moderation#flag", p)
    x3 = f.modaction("mona", "hide", p, scope="thread")
    x4 = f.modaction("mona", "access", did("alice"), level="suspended")
    x5 = f.modaction("mona", "tier", p, tier="featured")
    x6 = f.modaction("mona", "pin", p, pin_scope="sidebar")
    v = V("unknown-action-scope-or-value-has-no-effect", f, [*mem(f, "alice"), p, x1, x2, x3, x4, x5, x6],
          "MA7: an unknown action $type (in or outside NS), an unknown scope, or an unknown #access.level, "
          "#tier.tier or #pin.scope makes the record valid but without effect (reserved/unknown)",
          extra=("B", "tiers"))
    assert v["expected"]["resolved"] == {} and st(v, p) is None
    assert all(status(v, r) == ("reserved/unknown", None) for r in (x1, x2, x3, x4, x5, x6))
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    x = f.modaction("mona", "hide", p, rkey=f"{f.fk}-{tid(T0 + 2 * HOUR, 3)}")
    v = V("reserved-tid-key-is-ignored", f, [*mem(f, "alice"), p, x],
          "MA2: a modAction key of the form ^FK-TID$ is reserved for P2 list actions; such a record is not "
          "invalid but has no effect in v1")
    assert v["expected"]["resolved"] == {} and status(v, x) == ("reserved/unknown", None)
    out.append(v)

    f = F()
    p = f.post("alice", T0 + HOUR)
    c = f.comment("bob", p, T0 + 2 * HOUR)
    bad5 = f.modaction("mona", "pin", p, pin_scope="forum", subject_cid=False)
    bad4 = f.modaction("mona", "hide", did("alice"))
    bad6 = f.modaction("mona", "tier", p, scope="author", tier="personal")
    ok = f.modaction("mike", "hide", c)
    v = V("invalid-modactions-are-excluded", f, [*mem(f, "alice", "bob"), p, c, bad5, bad4, bad6, ok],
          "invalid records have no effect and are not in the mod log (raw view only): a positive pin "
          "without subjectCid (MA5), a #hide whose subject is a DID (MA4), and an author-scope #tier (MA6)",
          invalid={u(bad5): "MA5", u(bad4): "MA4", u(bad6): "MA6"})
    assert log(v) == [(u(ok), "effective", None)] and st(v, p) is None
    out.append(v)

    # ------------------------------------------------------------------ mod log (§4.12)
    f = F()
    p1, p2, p3, p4 = (f.post("alice", T0 + i * HOUR) for i in (1, 2, 3, 4))
    m1_v1 = f.modaction("mona", "hide", p1, reason="v1", t=T0 + 5 * HOUR)
    m1 = rec(m1_v1["uri"], {**m1_v1["value"], "reason": "v2", "updatedAt": dt(T0 + 9 * HOUR)})
    r1 = f.revision(m1_v1, "update", T0 + 9 * HOUR, record=m1_v1["value"])
    m2 = f.modaction("mike", "hide", p2, t=T0 + 7 * HOUR)
    m3 = f.modaction("adam", "hide", p3, t=T0 + 5 * HOUR)
    r3 = f.revision(m3, "delete", T0 + 8 * HOUR, record=m3["value"])
    m4 = f.modaction("owner", "hide", p4, hidden=False, t=T0 + 7 * HOUR)
    p4_old = rec(p4["uri"], {**p4["value"], "title": "Old title"})
    p4_new, r4 = p4, f.revision(p4_old, "update", T0 + 6 * HOUR, record=p4_old["value"])
    v = V("mod-log-order-edits-and-removals", f, [*mem(f, "alice"), p1, p2, p3, p4_new, m1, r1, m2, r3, m4, r4],
          "order (time desc, uri asc) with time = µs(updatedAt) ?? µs(createdAt) for a modAction and "
          "µs(createdAt) for a revision; revisions of modActions appear as edited (op update) or removed "
          "(op delete, the modAction itself is gone); a post's revision is not a mod-log entry",
          extra=("modQueue",))
    assert log(v) == [(u(m1), "effective", None), (u(r1), "edited", None), (u(r3), "removed", None),
                      (u(m2), "effective", None), (u(m4), "effective", None)]
    assert st(v, p1) == removed(1, "mona", reason="v2") and st(v, p3) is None
    out.append(v)

    # ------------------------------------------------------------------ fail closed (§4.11)
    f = F(membership="open")
    pa = f.post("alice", T0 + HOUR)
    pb = f.post("bob", T0 + 2 * HOUR)
    pb2 = f.post("bob", T0 + 3 * HOUR)
    pm = f.post("mona", T0 + 4 * HOUR)
    grant = f.modaction("owner", "access", did("bob"), level="member")
    h = f.modaction("mona", "hide", pb2, reason="cached")
    v = V("degraded-view", f, [*mem(f, "alice", "bob"), pa, pb, pb2, pm, grant, h],
          "mona's repo is Unavailable, so the view is degraded: items by authors outside S are "
          "Hidden(ModerationUnavailable); bob (granted) and mona (an authority) are in S; mona's cached "
          "hide still applies", fi={"repo_status": {did("mona"): "unavailable"}}, extra=("degraded", "S"))
    assert v["expected"]["degraded"] is True
    assert st(v, pa) == hidden("ModerationUnavailable") and st(v, pb) is None and st(v, pm) is None
    assert st(v, pb2) == removed(1, "mona", reason="cached")
    out.append(v)

    f = F()
    pa = f.post("alice", T0 + HOUR)
    h = f.modaction("mona", "hide", pa)
    v = V("stale-authority-does-not-degrade", f, [*mem(f, "alice"), pa, h],
          "a Stale authority repo contributes its cached assertions and does not degrade the view; an "
          "Unavailable non-authority repo does not either",
          fi={"repo_status": {did("mona"): "stale", did("alice"): "unavailable"}}, extra=("degraded",))
    assert v["expected"]["degraded"] is False and st(v, pa) == removed(1, "mona")
    out.append(v)

    f = F()
    pa = f.post("alice", T0 + HOUR)
    v = V("authority-missing-from-repoStatus-degrades", f, [*mem(f, "alice"), pa],
          "a DID missing from repoStatus is Unavailable: mike has no entry, so the view is degraded",
          fi={"repo_status": {did("mike"): None}}, extra=("degraded",))
    assert v["expected"]["degraded"] is True and st(v, pa) == hidden("ModerationUnavailable")
    out.append(v)

    return out


FILES = [
    ("modfold/forum.json", lambda: file_doc(
        "mod/1", "modfold",
        "social.md §4.1–§4.8 and §4.10–§4.12: authorities and ranks, forum-scope assertions and their "
        "authorisation (rank, role, until, CID binding), resolution of every dimension, keeps against "
        "bans, restrictions and locks, lapse and adoption, fail closed, the mod log and mod queue. Author "
        "tier (§4.9) vectors are a separate file. input: {fold: FoldInput}. expected: the modfold "
        "projection fields listed (testdata/vectors/README.md, social section).",
        forum_vectors())),
]
