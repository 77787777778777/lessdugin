# SPDX-License-Identifier: CC0-1.0
"""fold/digest.json (kind "digest"): inputDigest, social.md §1.2."""

from __future__ import annotations

import hashlib

from lessdugin_ref.build import HOUR, NOW, T0, Forum, did, rec

from .util import file_doc, vector

DEFAULT_PARAMS = (
    "membership=approval,defaultTier=personal,voting=reacts,authorModeration=true,tagCreation=open,"
    "pairCap=25,establish=1,establishSeeds=2,offsetHours=2,exponentMilli=1150,curatedBonus=10,"
    "frontpageBonus=0,cutoffDays=90,reactSecond=5,reactCreate=10,reactAnti=20,authorModPersonal=50,"
    "authorModFrontpage=2000,createTag=10,wikiEdit=5,sequence=1000,collapseBelow=-5,newUsers=collapse")
DEFAULT_ALGS = "mod/1,karma/1,magic/1,default/1"


def hand_preimage(fold: dict, algs: str, params: str) -> str:
    """A second, deliberately naive rendering of §1.2 D (all inputs here are ASCII)."""
    lines = [f"{r['uri']}\x00{r['cid']}\n" for r in fold["records"]]
    lines += [f"{a}\x00-\n" for a in fold["absent"]]
    lines.sort(key=str.encode)
    return ("".join(lines) + f"decl\x00{fold['decl']['cid']}\n" + f"algs\x00{algs}\n"
            + f"params\x00{params}\n")


def V(name, f, records, note, *, params=DEFAULT_PARAMS, algs=DEFAULT_ALGS, banner=False, fi=None,
      invalid=None):
    fold = f.fold_input(records, **(fi or {}))
    v = vector("digest", name, fold, extra_fields=("newerRulesBanner",), note=note, invalid=invalid)
    e = v["expected"]
    want = hand_preimage(v["input"]["fold"], algs, params)
    assert e["params"] == params and e["algs"] == algs and e["newerRulesBanner"] is banner, (name, e)
    assert e["digestPreimage"] == want, (name, e["digestPreimage"], want)
    assert e["inputDigest"] == hashlib.sha256(want.encode("utf-8")).hexdigest()
    return v


def digest_vectors() -> list:
    out = []

    f = Forum(membership=None)
    out.append(V("declaration-only-all-defaults", f, [],
                 "no records, no absent uris, a declaration without membership or policy: every PARAMS "
                 "value is its lexicon or karma/1 default (membership=approval, gates 5/10/20/50/2000/10/5/"
                 "1000, collapseBelow=-5 written with '-')"))

    preset_open = {
        "moderation": "mod/1", "reactPalette": "default/1", "voting": "reacts", "authorModeration": True,
        "tagCreation": "open",
        "karma": {"algorithm": "karma/1", "pairCap": 25, "establish": 1, "establishSeeds": 2},
        "ranking": {"algorithm": "magic/1", "offsetHours": 2, "exponentMilli": 1150, "curatedBonus": 10,
                    "frontpageBonus": 0, "cutoffDays": 90},
        "gates": {"reactSecond": 5, "reactCreate": 10, "reactAnti": 20, "authorModPersonal": 50,
                  "authorModFrontpage": 2000, "createTag": 10, "wikiEdit": 5, "sequence": 1000},
        "display": {"collapseBelow": -5, "newUsers": "collapse"},
    }
    f = Forum(membership="open", defaultTier="personal", policy=preset_open)
    out.append(V("forum-create-open-preset", f, [],
                 "the declaration `forum create` writes for the open preset (every field explicit): the "
                 "params line differs from the defaults only in membership; decl's CID differs",
                 params=DEFAULT_PARAMS.replace("membership=approval", "membership=open")))

    preset_approval = {**preset_open, "gates": {
        "reactSecond": 0, "reactCreate": 0, "reactAnti": 5, "authorModPersonal": 10,
        "authorModFrontpage": 100, "createTag": 0, "wikiEdit": 0, "sequence": 0}}
    f = Forum(membership="approval", defaultTier="personal", policy=preset_approval)
    out.append(V("forum-create-approval-preset", f, [],
                 "the approval preset's gates 0/0/5/10/100/0/0/0 (absent gates would mean the karma/1 "
                 "defaults instead)",
                 params=DEFAULT_PARAMS.replace("reactSecond=5,reactCreate=10,reactAnti=20,authorModPersonal=50,"
                                               "authorModFrontpage=2000,createTag=10,wikiEdit=5,sequence=1000",
                                               "reactSecond=0,reactCreate=0,reactAnti=5,authorModPersonal=10,"
                                               "authorModFrontpage=100,createTag=0,wikiEdit=0,sequence=0")))

    every = {
        "voting": "karmaOnly", "authorModeration": False, "tagCreation": "moderators",
        "karma": {"pairCap": 1000000, "establish": 0, "establishSeeds": 1000},
        "ranking": {"offsetHours": 1, "exponentMilli": 0, "curatedBonus": -1000000, "frontpageBonus": 7,
                    "cutoffDays": 36500},
        "gates": {"reactSecond": -1000000000, "reactCreate": 1000000000, "reactAnti": 0,
                  "authorModPersonal": -1, "authorModFrontpage": 123456789, "createTag": 3, "wikiEdit": 4,
                  "sequence": 0},
        "display": {"collapseBelow": 1000000, "newUsers": "hide"},
    }
    f = Forum(membership="open", defaultTier="frontpage", policy=every)
    out.append(V("every-param-non-default", f, [],
                 "every PARAMS value differs from its default, including range extremes, negative integers "
                 "and false",
                 params="membership=open,defaultTier=frontpage,voting=karmaOnly,authorModeration=false,"
                        "tagCreation=moderators,pairCap=1000000,establish=0,establishSeeds=1000,offsetHours=1,"
                        "exponentMilli=0,curatedBonus=-1000000,frontpageBonus=7,cutoffDays=36500,"
                        "reactSecond=-1000000000,reactCreate=1000000000,reactAnti=0,authorModPersonal=-1,"
                        "authorModFrontpage=123456789,createTag=3,wikiEdit=4,sequence=0,"
                        "collapseBelow=1000000,newUsers=hide"))

    unknown = {
        "moderation": "mod/2", "reactPalette": "custom/1", "voting": "quadratic", "tagCreation": "anyone",
        "karma": {"algorithm": "karma/9"}, "ranking": {"algorithm": "magic/2"},
        "display": {"newUsers": "shadow"},
    }
    f = Forum(membership="invite", defaultTier="home", policy=unknown)
    out.append(V("unknown-known-values-fall-back", f, [],
                 "unknown behaviour IDs and knownValues take their fallbacks (data-model.md §13.3) with the "
                 "'rules newer than your client' banner; the algs and params lines carry the effective "
                 "values", banner=True))

    # A forum with records, an absent uri and an invalid record.
    open_params = DEFAULT_PARAMS.replace("membership=approval", "membership=open")

    def populated(f):
        p = f.post("alice", T0 + HOUR)
        c = f.comment("bob", p, T0 + 2 * HOUR)
        gone = f.post("carol", T0 + 3 * HOUR)
        o1 = f.opinion("bob", p, karma="up")
        o2 = f.opinion("alice", gone, karma="strongUp")
        bad = f.post("dave", T0 + 4 * HOUR, title=" ")
        return [f.membership("alice"), f.membership("bob"), p, c, o1, o2, bad], [gone["uri"]], bad

    f = Forum(membership="open")
    recs, absent, bad = populated(f)
    v1 = V("records-absent-and-invalid", f, recs,
           "records of several collections, one absent uri (the deleted subject of an opinion, written "
           "with '-' for its CID) and an invalid record (a post whose title is only U+00A0 fails PO3): "
           "invalid records are fold inputs and are covered",
           fi={"absent": absent}, invalid={bad["uri"]: "PO3"}, params=open_params)
    out.append(v1)

    f = Forum(membership="open")
    recs, absent, bad = populated(f)
    v2 = V("obs-repoStatus-and-now-are-not-covered", f, recs,
           "the same records, absent set and declaration as records-absent-and-invalid, with different "
           "obs (none), repoStatus and now: the digest is the same (dave's post still fails PO3)",
           fi={"absent": absent, "now": NOW + 1000 * HOUR, "obs": {},
               "repo_status": {did("alice"): "stale", did("bob"): "unavailable", did("owner"): None}},
           invalid={bad["uri"]: "PO3"}, params=open_params)
    assert v2["expected"]["inputDigest"] == v1["expected"]["inputDigest"]
    assert v2["input"]["fold"]["obs"] != v1["input"]["fold"]["obs"]
    out.append(v2)

    f = Forum(membership="open")
    recs, absent, bad = populated(f)
    v3 = V("algorithmIds-given", f, recs,
           "the optional algorithmIds input equals the effective IDs derived from the declaration; it "
           "changes nothing (same records as records-absent-and-invalid, with the PO3-invalid post)",
           fi={"absent": absent, "algorithm_ids": {"moderation": "mod/1", "karma": "karma/1",
                                                    "ranking": "magic/1", "reactPalette": "default/1"}},
           invalid={bad["uri"]: "PO3"}, params=open_params)
    assert v3["expected"]["inputDigest"] == v1["expected"]["inputDigest"]
    out.append(v3)

    # Revision keys nest: the revision of a post is a prefix of the revision of a comment.
    f = Forum(membership="open")
    p = f.post("alice", T0 + HOUR, clock=1)
    c = f.comment("alice", p, T0 + 2 * HOUR, clock=2)
    p_old = rec(p["uri"], {**p["value"], "title": "First"})
    c_old = rec(c["uri"], {**c["value"], "retracted": False})
    r_post = f.revision(p_old, "update", T0 + 2 * HOUR, record=p_old["value"], clock=2)
    r_comment = f.revision(c_old, "update", T0 + 3 * HOUR, record=c_old["value"], clock=3)
    assert r_comment["uri"].startswith(r_post["uri"] + "-"), (r_post["uri"], r_comment["uri"])
    out.append(V("line-order-with-nested-revision-keys", f, [f.membership("alice"), p, c, r_post, r_comment],
                 "the revision of the post (rkey {fk}-{T}-{t}) is a prefix of the revision of the comment "
                 "{fk}-{T}-{t} (rkey {fk}-{T}-{t}-{tid}): the 0x00 separator sorts the shorter uri first, "
                 "as uri order does", params=open_params))
    return out


FILES = [
    ("fold/digest.json", lambda: file_doc(
        "mod/1,karma/1,magic/1,default/1", "digest",
        "social.md §1.2 inputDigest (§1 binds every behaviour ID, hence the composite behaviour). input: "
        "{fold: FoldInput}. expected: inputDigest (64 lowercase hex), digestPreimage (the byte string D as "
        "a JSON string; every byte here is ASCII), algs and params (the bodies of the algs and params "
        "lines), newerRulesBanner. obs, repoStatus and now are not covered by the digest.",
        digest_vectors())),
]
