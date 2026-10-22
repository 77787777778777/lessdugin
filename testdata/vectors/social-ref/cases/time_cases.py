# SPDX-License-Identifier: CC0-1.0
"""time/time.json (kind "time") and time/conversions.json (kind "conversion"): social.md §2."""

from __future__ import annotations

from lessdugin_ref.build import DAY, HOUR, MIN, SEC, T0, Forum, dt, tid
from lessdugin_ref.syntax import datetime_us
from lessdugin_ref.time import tid_time

from .util import file_doc, vector

SKEW = 300_000_000          # 5 min
INCONS = 3_600_000_000      # 1 h


def V(name, f, records, note=None, **kw):
    return vector("time", name, f.fold_input(records, **kw.pop("fi", {})), note=note, **kw)


def tr(vec, r):
    return vec["expected"]["times"][r["uri"]]["tRank"]


def flags(vec, r):
    e = vec["expected"]["times"][r["uri"]]
    return e["futureDated"], e["inconsistentTimestamps"]


def at_us(f, r, us):
    f.obs_override[(r["uri"], r["cid"])] = us


def time_vectors() -> list:
    out = []

    # --- honest content -------------------------------------------------------------
    f = Forum()
    p = f.post("alice", T0 + HOUR)
    c = f.comment("bob", p, T0 + 2 * HOUR)
    v = V("honest-post-and-comment", f, [p, c],
          note="claimed time = TID time = first-seen time: t_rank is the claimed time")
    assert tr(v, p) == T0 + HOUR and tr(v, c) == T0 + 2 * HOUR and v["expected"]["tDecl"] == T0
    out.append(v)

    # --- future-dated boundaries (§2.2) ------------------------------------------------
    for label, delta, fut in (("just-under", SKEW - 1, False), ("exactly", SKEW, False),
                              ("just-over", SKEW + 1, True)):
        f = Forum()
        seen = T0 + HOUR
        p = f.post("alice", seen + delta)
        at_us(f, p, seen)
        v = V(f"future-dated-{label}-5-min", f, [p],
              note=f"t_eff = t_obs + {delta} µs; future(x) ⇔ t_eff > t_obs + 300000000")
        assert tr(v, p) == (seen if fut else seen + delta) and flags(v, p)[0] is fut
        out.append(v)

    f = Forum()
    seen = T0 + HOUR
    p = f.post("alice", seen + DAY)
    at_us(f, p, seen)
    c = f.comment("bob", p, seen + 2 * DAY)
    at_us(f, c, seen + 10 * MIN)
    v = V("future-dated-post-and-reply-clamped", f, [p, c],
          note="both claim a time a day or more after first-seen: each ranks at its first-seen time, "
               "and the reply is still bounded below by its parent")
    assert tr(v, p) == seen and tr(v, c) == seen + 10 * MIN and flags(v, c)[0] is True
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + 400 * DAY)
    f.obs_override[(p["uri"], p["cid"])] = None
    v = V("future-claim-without-obs-stands", f, [p],
          note="no obs entry for the item (vectors only): the claimed time stands and the item is not "
               "flagged future-dated (§2.3)")
    assert tr(v, p) == T0 + 400 * DAY and flags(v, p) == (False, False)
    out.append(v)

    # --- t_eff = min_some(t_claim, t_tid) ------------------------------------------------
    f = Forum()
    p = f.post("alice", T0 + 2 * HOUR, tid_us=T0 + 6 * HOUR)
    at_us(f, p, T0 + 6 * HOUR)
    v = V("claim-earlier-than-tid", f, [p],
          note="t_claim = T0+2h < t_tid = T0+6h: t_eff = t_claim; |t_claim − t_tid| = 4 h > 1 h flags "
               "inconsistent timestamps")
    assert tr(v, p) == T0 + 2 * HOUR and flags(v, p) == (False, True)
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + 20 * DAY, tid_us=T0 + 2 * HOUR)
    at_us(f, p, T0 + 2 * HOUR)
    v = V("tid-earlier-than-claim", f, [p],
          note="a far-future createdAt is harmless when the TID is honest: t_eff = t_tid; not future-dated "
               "(t_eff ≤ t_obs + 5 min), but inconsistent")
    assert tr(v, p) == T0 + 2 * HOUR and flags(v, p) == (False, True)
    out.append(v)

    for label, delta, inc in (("exactly", INCONS, False), ("just-over", INCONS + 1, True)):
        f = Forum()
        p = f.post("alice", T0 + 2 * HOUR + delta, tid_us=T0 + 2 * HOUR)
        at_us(f, p, T0 + 2 * HOUR)
        v = V(f"inconsistent-timestamps-{label}-1-h", f, [p],
              note=f"|t_claim − t_tid| = {delta} µs; the flag needs > 3600000000")
        assert flags(v, p) == (False, inc) and tr(v, p) == T0 + 2 * HOUR
        out.append(v)

    f = Forum()
    p = f.post("alice", T0 + HOUR, clock=1023)
    v = V("tid-clock-id-ignored", f, [p], note="tidTime drops the 10-bit clock id (clock id 1023 here)")
    assert tr(v, p) == T0 + HOUR
    out.append(v)

    # --- claimed time formats ----------------------------------------------------------
    f = Forum()
    p1 = f.post("alice", T0 + HOUR + 123456, created="2026-01-01T01:00:00.123456789Z")
    p2 = f.post("bob", T0 + HOUR, created="2026-01-01T02:00:00.000+01:00")
    p3 = f.post("carol", T0 + 3 * HOUR, created="2025-12-31T22:00:00-05:00")
    for r in (p1, p2, p3):
        at_us(f, r, T0 + 5 * HOUR)
    v = V("claimed-time-offsets-and-truncation", f, [p1, p2, p3],
          note="µs(createdAt): fraction digits beyond the sixth are truncated; offsets convert to UTC")
    assert tr(v, p1) == T0 + HOUR + 123456 and tr(v, p2) == T0 + HOUR and tr(v, p3) == T0 + 3 * HOUR
    out.append(v)

    # --- missing or unparseable createdAt ------------------------------------------------
    f = Forum()
    p1 = f.post("alice", T0 + 3 * HOUR, created="1969-12-31T23:59:59.000Z")
    p2 = f.post("bob", T0 + 4 * HOUR, created="0000-01-01T00:00:00.000Z")
    v = V("pre-epoch-createdAt-has-no-claim", f, [p1, p2],
          note="valid atproto datetimes before 1970 give µs = None: t_eff = t_tid, and no "
               "inconsistent-timestamps flag (t_claim absent)")
    assert tr(v, p1) == T0 + 3 * HOUR and tr(v, p2) == T0 + 4 * HOUR and flags(v, p1) == (False, False)
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + HOUR)
    bad = f.comment("bob", p, T0 + 2 * HOUR, created="2026-01-01")
    child = f.comment("carol", p, T0 + 30 * MIN, parent=bad)
    at_us(f, bad, T0 + 2 * HOUR)
    v = V("unparseable-createdAt-excludes-the-item", f, [p, bad, child],
          note="createdAt '2026-01-01' is not a datetime, so the comment fails G2 and is Excluded (no "
               "t_rank); its reply hangs under a Missing placeholder and is bounded by the root post",
          invalid={bad["uri"]: "G2"})
    assert bad["uri"] not in v["expected"]["times"] and tr(v, child) == T0 + HOUR
    out.append(v)

    # --- causal lower bound ---------------------------------------------------------------
    f = Forum()
    p = f.post("alice", T0 + 2 * HOUR)
    c = f.comment("bob", p, T0 + HOUR)
    at_us(f, c, T0 + 3 * HOUR)
    r = f.comment("carol", p, T0 + 90 * MIN, parent=c)
    at_us(f, r, T0 + 4 * HOUR)
    v = V("comment-claimed-before-its-parent", f, [p, c, r],
          note="claims (and TIDs) before the post: the comment ranks at the post's t_rank, and its "
               "reply at the comment's")
    assert tr(v, c) == T0 + 2 * HOUR and tr(v, r) == T0 + 2 * HOUR
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + HOUR)
    c1 = f.comment("bob", p, T0 + 5 * HOUR)
    c2 = f.comment("carol", p, T0 + 4 * HOUR, parent=c1)
    at_us(f, c2, T0 + 6 * HOUR)
    c3 = f.comment("dave", p, T0 + 7 * HOUR, parent=c2)
    v = V("chain-is-monotone", f, [p, c1, c2, c3],
          note="t_rank never decreases down a chain: 5h, max(5h, 4h) = 5h, 7h")
    assert [tr(v, x) for x in (c1, c2, c3)] == [T0 + 5 * HOUR, T0 + 5 * HOUR, T0 + 7 * HOUR]
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + 3 * HOUR)
    ghost = f.comment_uri("zed", p["uri"], T0 + 9 * HOUR, 7)
    c = f.comment("bob", p, T0 + HOUR, parent=(ghost, "bafyreibr6trtvr3b5w2vm7kmbvhdpqoxuaypcuq5euxsrwntvmq4yyjtuy"))
    at_us(f, c, T0 + 4 * HOUR)
    v = V("parent-missing-bounded-by-root", f, [p, c],
          note="the parent is not in records: the comment sits under a Missing placeholder, so its "
               "lower bound is the root post's t_rank (§2.2 treeParent)")
    assert tr(v, c) == T0 + 3 * HOUR
    out.append(v)

    f = Forum()
    ghost_post = f.post("zed", T0 + 5 * HOUR)
    c = f.comment("bob", ghost_post, T0 - 2 * HOUR)
    at_us(f, c, T0 + 6 * HOUR)
    v = V("root-missing-bounded-by-decl", f, [c],
          note="the root post is not in records: the lower bound is t_decl")
    assert tr(v, c) == T0
    out.append(v)

    f = Forum()
    badroot = f.post("zed", T0 + 5 * HOUR, title=" 　 ")
    c = f.comment("bob", badroot, T0 + HOUR)
    v = V("root-excluded-bounded-by-decl", f, [badroot, c],
          note="the root post fails PO3 (its title is only White_Space) and is Excluded; NOTES.md 3: an "
               "Excluded root counts as not in records, so the comment's lower bound is t_decl",
          invalid={badroot["uri"]: "PO3"})
    assert tr(v, c) == T0 + HOUR
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + 4 * HOUR)
    c1 = f.comment("bob", p, T0 + HOUR, clock=1)
    c2 = f.comment("carol", p, T0 + 6 * HOUR, clock=2)
    # make a 2-cycle: c1 -> c2 -> c1 (c2 is rebuilt with parent c1; c1 rebuilt with parent c2)
    c1 = f.comment("bob", p, T0 + HOUR, clock=1, parent=(c2["uri"], c2["cid"]))
    c2 = f.comment("carol", p, T0 + 6 * HOUR, clock=2, parent=(c1["uri"], c1["cid"]))
    at_us(f, c1, T0 + 7 * HOUR)
    v = V("orphaned-comment-bounded-by-root", f, [p, c1, c2],
          note="c1 and c2 name each other as parent; the greater uri (carol's) is re-attached to the root "
               "(Orphaned), so it is bounded by the post and the other by it")
    big, small = (c2, c1) if c2["uri"] > c1["uri"] else (c1, c2)
    assert tr(v, big) == max(T0 + 4 * HOUR, T0 + 6 * HOUR if big is c2 else T0 + HOUR)
    out.append(v)

    # --- declaration lower bound ------------------------------------------------------------
    f = Forum(created=T0 + 10 * DAY)
    p = f.post("alice", T0 + 2 * DAY)
    w = f.wiki("bob", T0 + DAY, "decision-theory")
    s = f.sequence("carol", T0 + 3 * DAY)
    for r in (p, w, s):
        at_us(f, r, T0 + 11 * DAY)
    v = V("items-claimed-before-the-declaration", f, [p, w, s],
          note="posts, wikiRevisions and sequences are bounded below by t_decl")
    assert all(tr(v, x) == T0 + 10 * DAY for x in (p, w, s))
    out.append(v)

    for label, delta, clamped in (("just-under", SKEW - 1, False), ("exactly", SKEW, False),
                                  ("just-over", SKEW + 1, True)):
        f = Forum(created=T0 + delta)
        f.obs_override[(f.decl["uri"], f.decl["cid"])] = T0
        p = f.post("alice", T0 + 2 * MIN)
        v = V(f"declaration-future-dated-{label}", f, [p],
              note=f"decl.createdAt = t_obs(decl) + {delta} µs; t_decl falls back to t_obs(decl) only "
                   f"beyond 5 minutes (c > o + 300000000)")
        assert v["expected"]["tDecl"] == (T0 if clamped else T0 + delta)
        assert tr(v, p) == max(v["expected"]["tDecl"], T0 + 2 * MIN)
        out.append(v)

    f = Forum(created=T0 + 30 * DAY)
    f.obs_override[(f.decl["uri"], f.decl["cid"])] = None
    p = f.post("alice", T0)
    v = V("declaration-without-obs", f, [p], note="no obs for the declaration: t_decl = µs(createdAt)")
    assert v["expected"]["tDecl"] == T0 + 30 * DAY and tr(v, p) == T0 + 30 * DAY
    out.append(v)

    f = Forum(created=0)
    f.decl["value"]["createdAt"] = "1969-07-20T20:17:40.000Z"
    from lessdugin_ref.build import rec
    f.decl = rec(f.decl["uri"], f.decl["value"])
    f.uri = f.decl["uri"]
    p = f.post("alice", T0)
    v = V("declaration-pre-epoch", f, [p],
          note="µs(decl.createdAt) = None (before 1970) gives c = 0, and the declaration has no obs "
               "entry, so t_decl = 0")
    assert v["expected"]["tDecl"] == 0 and tr(v, p) == T0
    out.append(v)

    # --- missing createdAt; future-dated through the TID alone; moderation is irrelevant ---
    f = Forum()
    p = f.post("alice", T0 + HOUR)
    bad = f.post("bob", T0 + 2 * HOUR)
    bad["value"].pop("createdAt")
    from lessdugin_ref.build import rec as _rec
    bad = _rec(bad["uri"], bad["value"])
    at_us(f, bad, T0 + 2 * HOUR)
    v = V("missing-createdAt-excludes-the-item", f, [p, bad],
          note="createdAt is required: the post without it fails G2, is Excluded (V1) and has no t_rank",
          invalid={bad["uri"]: "G2"})
    assert bad["uri"] not in v["expected"]["times"] and tr(v, p) == T0 + HOUR
    out.append(v)

    f = Forum()
    p = f.post("alice", T0 + 2 * DAY, created="1969-12-31T23:59:59.000Z")
    at_us(f, p, T0 + HOUR)
    v = V("future-dated-through-the-tid-alone", f, [p],
          note="createdAt is before 1970 (t_claim = None), so t_eff = t_tid = T0+2d, which is more than "
               "5 minutes after t_obs = T0+1h: future-dated, ranked at first-seen")
    assert tr(v, p) == T0 + HOUR and flags(v, p) == (True, False)
    out.append(v)

    f = Forum(moderators=[("mona", "moderator")])
    p = f.post("alice", T0 + 2 * HOUR)
    c = f.comment("bob", p, T0 + HOUR)
    at_us(f, c, T0 + 3 * HOUR)
    hide = f.modaction("owner", "hide", p, t=T0 + 4 * HOUR)
    v = V("hidden-items-keep-their-time", f, [p, c, hide],
          note="t_rank reads only validity, obs, decl and placement: the owner's hide of the post changes "
               "nothing, and the reply is still bounded below by the hidden post")
    assert tr(v, p) == T0 + 2 * HOUR and tr(v, c) == T0 + 2 * HOUR
    out.append(v)

    # --- t_obs is the first-seen time of the record (any version) --------------------------
    f = Forum()
    p_v1 = f.post("alice", T0 + HOUR, clock=5)
    p = f.post("alice", T0 + HOUR, clock=5, updatedAt=dt(T0 + 10 * HOUR), title="Edited")
    f.extra_obs[(p_v1["uri"], p_v1["cid"])] = T0 + HOUR
    v = V("edit-keeps-first-seen", f, [p],
          note="obs has both versions (T0+1h, T0+10h); t_obs is their minimum and the edit changes nothing")
    assert tr(v, p) == T0 + HOUR
    out.append(v)

    f = Forum()
    p_v1 = f.post("alice", T0 + HOUR, clock=5)
    p = f.post("alice", T0 + HOUR, clock=5, updatedAt=dt(T0 + 10 * HOUR), title="Edited")
    f.extra_obs[(p_v1["uri"], p_v1["cid"])] = T0 + HOUR - 6 * MIN
    v = V("future-check-uses-the-earliest-version", f, [p],
          note="the earliest version was seen 6 min before the claimed time: future-dated, clamped to "
               "that first-seen time although the current version was seen later")
    assert tr(v, p) == T0 + HOUR - 6 * MIN and flags(v, p)[0] is True
    out.append(v)

    return out


def conversion_vectors() -> list:
    out = []

    def us(name, s, note=None):
        want = datetime_us(s)
        e = {"name": name, "input": {"fn": "us", "datetime": s}, "expected": {"us": want}}
        if note:
            e["note"] = note
        out.append(e)
        return want

    assert us("utc-millis", "2026-01-01T00:00:00.000Z") == T0
    assert us("epoch", "1970-01-01T00:00:00.000Z") == 0
    assert us("epoch-offset", "1970-01-01T01:00:00+01:00") == 0
    assert us("before-epoch-by-1-us", "1969-12-31T23:59:59.999999Z", "negative times give None") is None
    assert us("before-epoch-offset", "1970-01-01T00:59:59.999999+01:00") is None
    assert us("no-fraction", "2026-01-01T00:00:01Z") == T0 + SEC
    assert us("one-fraction-digit", "2026-01-01T00:00:00.5Z") == T0 + 500_000
    assert us("six-fraction-digits", "2026-01-01T00:00:00.123456Z") == T0 + 123456
    assert us("seven-fraction-digits-truncated", "2026-01-01T00:00:00.1234567Z") == T0 + 123456
    assert us("nine-nines-truncated", "2026-01-01T00:00:00.999999999Z", "truncation, not rounding") \
        == T0 + 999_999
    assert us("thirteen-fraction-digits", "1985-04-12T23:20:50.1235678912345Z") is not None
    assert us("twenty-fraction-digits", "2026-01-01T00:00:00.12345678901234567890Z",
              "20 fraction digits, the most the atproto datetime syntax allows (46 characters): valid, "
              "truncated after the sixth") == T0 + 123456
    assert us("twenty-one-fraction-digits", "2026-01-01T00:00:00.123456789012345678901Z",
              "21 fraction digits: not an atproto datetime (the reference implementations and "
              "crates/atp allow 1 to 20 fraction digits and 64 characters; NOTES.md 17): None") is None
    assert us("positive-offset", "2026-01-01T05:30:00.000+05:30") == T0
    assert us("negative-offset", "2025-12-31T19:00:00.000-05:00") == T0
    assert us("offset-minutes", "2026-01-01T01:45:00.000+01:45") == T0
    assert us("leap-day", "2024-02-29T12:00:00.000Z") is not None
    assert us("year-0000", "0000-01-01T00:00:00.000Z", "valid syntax, before 1970") is None
    assert us("largest-json-safe", "2255-06-05T23:47:34.740991Z",
              "2^53 − 1 µs, the largest value a JSON number carries exactly (README); later instants "
              "are valid but not in vectors") == 2 ** 53 - 1
    for name, s in (
        ("lowercase-z", "2026-01-01T00:00:00.000z"),
        ("lowercase-t", "2026-01-01t00:00:00.000Z"),
        ("minus-zero-offset", "2026-01-01T00:00:00.000-00:00"),
        ("no-timezone", "2026-01-01T00:00:00.000"),
        ("date-only", "2026-01-01"),
        ("no-seconds", "2026-01-01T00:00Z"),
        ("space-separator", "2026-01-01 00:00:00.000Z"),
        ("empty-fraction", "2026-01-01T00:00:00.Z"),
        ("compact-offset", "2026-01-01T00:00:00.000+0000"),
        ("month-13", "2026-13-01T00:00:00.000Z"),
        ("feb-29-non-leap", "2025-02-29T00:00:00.000Z"),
        ("hour-24", "2026-01-01T24:00:00.000Z"),
        ("second-61", "2026-01-01T00:00:61.000Z"),
        ("five-digit-year", "02026-01-01T00:00:00.000Z"),
        ("empty", ""),
    ):
        assert us(name, s, "not an atproto datetime: None") is None

    def tt(name, t, note=None):
        want = tid_time(t)
        e = {"name": name, "input": {"fn": "tidTime", "tid": t}, "expected": {"us": want}}
        if note:
            e["note"] = note
        out.append(e)
        return want

    assert tt("tid-zero", "2222222222222") == 0
    assert tt("tid-of-T0", tid(T0)) == T0
    assert tt("tid-of-T0-clock-1023", tid(T0, 1023), "the low 10 bits are the clock id") == T0
    assert tt("tid-keys-example", "3l2s5xxv2ze2c") > 0
    assert tt("tid-interop-example", "3jzfcijpj2z2a") > 0
    assert tt("tid-2-pow-63-minus-1", "bzzzzzzzzzzzz",
              "value 2^63 − 1, tidTime 2^53 − 1. TIDs from 'c' up to 'j' in the first character have "
              "bit 63 set (unsigned 64-bit values, NOTES.md) and times beyond 2^53 − 1, which JSON "
              "vectors do not carry") == 2 ** 53 - 1
    return out


FILES = [
    ("time/time.json", lambda: file_doc(
        "magic/1", "time",
        "social.md §2: t_obs, t_eff, the 5-minute future clamp, the declaration lower bound t_decl, the "
        "causal lower bound through the placed thread, and the inconsistent-timestamps flag. input: "
        "{fold: FoldInput}. expected: tDecl (µs), times: {uri: {tRank, futureDated, "
        "inconsistentTimestamps}} for every candidate item that V1 does not exclude.",
        time_vectors())),
    ("time/conversions.json", lambda: file_doc(
        "magic/1", "conversion",
        "social.md §2.1 conversions. input: {fn: \"us\", datetime} -> expected {us: µs or null} "
        "(µs(datetime): any UTC offset, 1 to 20 fraction digits truncated after the sixth, at most 64 "
        "characters, null if it does not parse as an atproto datetime or is before 1970); {fn: \"tidTime\", tid} -> expected {us} "
        "(the TID's 64-bit value >> 10).",
        conversion_vectors())),
]
