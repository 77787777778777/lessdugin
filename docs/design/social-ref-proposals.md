<!-- SPDX-License-Identifier: CC0-1.0 -->
# Proposed spec wording from the social-ref vector review

For the owner of `docs/spec/social.md` and `docs/spec/data-model.md`. Each item gives the
section, the problem, the proposed normative text, and the reading the reference model and the
vectors already implement (the `NOTES.md` decision number refers to
`testdata/vectors/social-ref/NOTES.md`). Every proposal matches the current vectors, so adopting
it changes no vector. Items marked **(pinned)** are fixed by vectors; changing the reading
instead means changing those vectors.

Suggested home: one new entry per item in `social.md` §20.1 (or `data-model.md` §14), plus the
in-place edits shown.

---

## A. Review findings

### A1. §4.9 Precedence: author bans and the forum tier (NOTES 46) (pinned)

**Problem.** The general sentence ("For any subject and dimension that has a forum-tier
`Resolved` value, the forum tier governs") applies to the subject DID and dimension `access`
of an author ban, so a forum grant (`#access{member}`) would lift it. In approval mode every
non-authority participant who is not banned is granted, so author bans would never act there.
The bullet list and the `AuthorModerated` formula only require `keepRank(c) = 0` (and
`resolve(c, hidden) = Default`).

**Replace the Precedence paragraph's first sentence with:**

> **Precedence.** Author-tier values act at rank 0. For hides, locks and thread pins, a
> forum-tier `Resolved` value for the same subject and dimension (`hidden` of the comment,
> `locked` of the post, `pin[thread]` of the comment) governs, and the author-tier value is
> ignored. An author ban is judged per comment: it applies to c if and only if
> `keepRank(c) = 0` (and, like every author-tier effect, `resolve(c, hidden) = Default`); a
> forum-tier `#access` value on the banned DID, a grant included, does not lift it.

**§20.1 entry:** "Author bans yield only to item-level forum decisions. A forum grant does not
lift an author ban; otherwise author bans could never act in approval mode."

Vector: `modfold/author.json` `author-ban-of-a-granted-user-in-approval-mode`.

### A2. §2.2 and §11 step 1: an Excluded root post is a Missing root (NOTES 3) (pinned)

**§11 step 1, replace** "(or a `Missing(p)` root when `p ∉ records`)" **with:**

> (or a `Missing(p)` root when p is not a post in `records` that V1 does not exclude)

**§2.2, replace** "If the root post is not in `records`, the lower bound is `t_decl`." **with:**

> If the root post is not in `records`, or V1 excludes it, the lower bound is `t_decl`.

Vectors: `time/time.json` `root-excluded-bounded-by-decl`, `thread/thread.json` `root-excluded`.

### A3. §4.9: root(c) must be a live post (NOTES 44) (pinned)

**Replace** `root(c) = records[c.root.uri]       // must exist for any author-tier effect on c`
**with:**

```
root(c) = records[c.root.uri]   // for any author-tier effect on c, c must be a comment and
                                // root(c) a post, neither excluded by V1 (an Excluded root is
                                // a Missing root, §11)
```

and add to the table's preamble: "The subject of an author `#lock` must be a post that V1 does
not exclude. A record whose subject fails this fails the table condition (mod log:
`unauthorised`)."

Vector: `modfold/author.json` `author-tier-needs-the-root-post`.

### A4. §3.3 Applications: applicants exclude authorities (NOTES 9) (pinned)

**Replace** "A DID with `member(d) ∧ access(d) = none` is an applicant." **with:**

> A DID d ∉ A with `member(d) ∧ access(d) = none` is an applicant. (Authorities are
> participants already; the owner's own membership, which `forum create` writes, makes no
> application.)

Vector: `modfold/participation.json` `approval-mode-sets` (`applicants`, `modQueue`).

### A5. §4.12 Mod log status and mod queue (NOTES 4, 5, 6, 7, 11, 23, 45) (pinned)

**Add after the status list:**

> **Status of a forum-scope modAction** (the first that applies):
> 1. `reserved/unknown`: a reserved `^FK-TID$` key (MA2), an unknown scope, action or value
>    (MA7), or a `#lock` of a forum uri (MA9);
> 2. `lapsed`: the actor is not in A (§4.3 condition 2; the fold cannot tell a former
>    moderator from a DID that never was one);
> 3. `unauthorised`: §4.3 condition 3 (rank) or 4 (role) fails;
> 4. `expired`: condition 5 fails;
> 5. `stale`: condition 6 fails (the subject is not in `records`, or has another CID), or the
>    record's `#tag.head` fails the §4.5 head check (the record still asserts `tag.removed`);
> 6. `effective`: some dimension the record asserts, other than the derived `spam`, resolves at
>    the actor's rank to the record's value (for `restrict`, to any value);
> 7. otherwise `overridden`, with the highest rank among the resolutions of the dimensions it
>    asserts.
>
> **Status of an author-scope modAction:** `unauthorised` (authorModeration, X ∈ E, MA6 or the
> table condition), else `expired`, else `stale`, else `overridden` (with the governing rank)
> when the forum tier has a `Resolved` value for the same subject and dimension (`hidden`,
> `locked`, `pin[thread]`), else `effective`. An author ban is never `overridden` (§4.9).
>
> Entries without a time (a datetime before 1970) sort after all others, then by uri.

**Replace the first mod-queue bullet with:**

> stale positive assertions whose subject (or, for `#tag.head`, `head.uri`) is in `records`
> with a different CID ("edited since curated, pinned or kept"); an assertion on a record that
> is not in `records` is not a queue item;

Vectors pinning step 6/7: `modfold/forum.json` `equal-rank-winner-is-the-smallest-uri`,
`restrict-equal-rank-sets-unite`, `empty-restrict-at-higher-rank-lifts`,
`tag-head-conflict-at-equal-rank`, `tag-head-equal-values-agree`,
`lock-allow-lists-intersect-at-equal-rank`; step 5 and the queue: `tag-head-bound-by-its-cid`,
`positive-assertion-on-a-subject-not-in-records-never-counts`; the order of steps 1–5:
`mod-log-status-precedence` (added after the review).

### A6. §5 deleted-subject rule (NOTES 30) (pinned)

**Replace the paragraph** "An opinion whose subject is in `absent` counts **only** toward karma
(§6.2), and only if: …" **with:**

> An opinion whose subject is in `absent` counts (O1–O5). The deleted-subject rule limits what
> it counts toward: only karma (§6.2), only when the subject's kind, read from `coll(uri)`, is
> post or comment, and only in a stage where at least **2 distinct voters** of the stage's
> eligible set have counting karma opinions on that uri (`included`). It never counts toward
> scores, reacts or tags.

Vector: `karma/staged.json` `deleted-subject-one-vs-two-voters` (`counting`).

### A7. CO5 and invalid roots (NOTES 20, 31) (pinned)

**data-model.md CO5, replace with:**

> CO5 *(x)* If a record with the root uri is in the fold input, valid or not, its
> `value.forum` equals `comment.forum` (an absent or non-string `forum` fails).

**social.md §5 O2, add:** "s is not excluded by V1, which includes the cross-record rule CO5."

Rationale: invalid records are fold inputs (§1.1), and without a 60-bit fk collision CO5 can only
fail through an invalid root (a valid root in F's key range names F, by G9). Vectors:
`visibility/visibility.json` `v1-before-v2-and-v3`, `karma/staged.json`
`opinions-that-never-count`.

### A8. §1.6 and data-model.md §13.3: which fallbacks raise the banner (NOTES 25) (pinned)

**social.md §1.6, replace the second bullet with:**

> An unknown value in one of the declaration's knownValue fields takes the fallback in
> `data-model.md` §13.3 and raises the banner "this forum uses rules newer than your client".
> Those fields are exactly `membership`, `defaultTier`, `moderators[].role` (in any entry,
> including one that FO2 ignores for another reason), `policy.moderation`, `policy.voting`,
> `policy.reactPalette`, `policy.tagCreation`, `policy.karma.algorithm`,
> `policy.ranking.algorithm` and `policy.display.newUsers`. Unknown values in other records
> never raise it. The fallback for `membership` is `approval`.

**data-model.md §13.3:** add "plus the banner" to the rows `#moderator.role`,
`forum.membership` and `forum.defaultTier`.

Vectors: `modfold/participation.json` `unknown-membership-falls-back-to-approval`,
`moderators-entry-rules`; `fold/digest.json` `unknown-known-values-fall-back`.

### A9. §2.1 datetime limits (NOTES 17) (pinned)

**Replace the µs(datetime) definition with:**

```
µs(datetime) = parse with the atproto datetime syntax: YYYY-MM-DDTHH:MM:SS[.F](Z|±HH:MM),
               uppercase T and Z, F of 1 to 20 digits, at most 64 characters, a real calendar
               date, hour ≤ 23, minute and second ≤ 59, offset not "-00:00"; convert to UTC and
               truncate fraction digits beyond the sixth; None if it does not parse or is < 0
```

These are the limits of the atproto reference implementations (Go indigo, TypeScript
`@atproto/syntax`) and of `crates/atp`. A datetime outside them fails G2 (format `datetime`).
Vectors: `time/conversions.json` `twenty-fraction-digits`, `twenty-one-fraction-digits`.

### A10. data-model.md MA3: forum-uri subjects (NOTES 12) (pinned)

**Replace MA3 with:**

> MA3 An at-uri subject in a forum-scoped collection (everything except `forum` and
> `profile`) has `fkOf(rkey(subject)) = fk(forum)`. A `forum` subject is checked by MA4 and
> MA9 only.

Without this, the `#lock` of a forum uri that MA9 calls "valid but without effect" is invalid.
Vector: `modfold/forum.json` `forum-uri-lock-has-no-effect`.

### A11. data-model.md G9: revisions have no forum field (NOTES 15)

**Replace G9 with:**

> G9 Collections with a `forum` field (every collection except `forum`, `profile` and
> `revision`): `fk(value.forum)` equals the key's fk segment. A revision's key range comes from
> its subject (RV3). This is part of G5, listed for clarity.

The G4 `forum` reference check likewise applies only to collections with a `forum` field.

### A12. data-model.md §7: the order of the validity rules (NOTES 1)

§14 item 23 says the first failing rule is exact, but §7 fixes no order. **Add to §7:**

> Rules are checked in this order, and the first failing one is reported: G1; G2 and G3 (one
> lexicon walk); G4; G5 (with G9 and the key equalities of FO1, ME1, PO1, CO1, CO2, RV1, RV3,
> OP2, MA2, WR1, SQ1); G6; G7; G8; G10; then the collection's remaining rules in numeric order.

(The vectors never assert rule IDs, so this order is not pinned.)

### A13. §9.2 qkey: the order of the steps (NOTES 35) (pinned)

**Replace** `(lfm-1.md §11: NFC, White_Space runs → U+0020, trimmed)` **with**
`(lfm-1.md §11: norm(trim_ws(exact)), in that order)`, and drop the list of steps, which
`lfm-1.md` §11 defines. The order matters: a control next to the trimmed edge leaves an edge
space. Vector: `reacts/reacts.json` `qkey-normalisation`.

### A14. §10.1 V3: Removed.spam (NOTES 8) (pinned)

**Replace the V3 state with:**

> `Hidden(Removed{rank, actor, spam, reason if showReason})`: rank, actor, `reason` and
> `showReason` from the winner of `hidden`; `spam = effective(x.uri, spam)`, the resolved value
> that also voids opinions (O5)

Vector: `modfold/forum.json` `removed-spam-is-the-resolved-spam`.

### A15. §10.1 V1 and §5 O1: forum equality (NOTES 60) (informative)

Add to §20.1 item 23: "Without an fk collision these checks never fire: a record in F's key
range whose `forum` names another forum is invalid by G9. Vectors therefore exercise G5/G9."

### A16. §3.1 newcomers (NOTES 26) (informative)

Add after `newcomer(d)`: "Banned and nullified participants are newcomers in both modes (V4
hides a banned user's content first); in approval mode they are the only newcomers." Replace
"meaningful in open mode only" accordingly.

### A17. §19 Test vectors

- **Directories:** `testdata/vectors/{time, modfold, karma, reacts, rank, thread, visibility,
  fold}/*.json`; the tags vectors are `karma/tags.json` (kind `tags`, behaviour `karma/1`)
  unless a `tags/` directory is wanted (then move the file).
- **Format:** "`input` is `{fold: FoldInput, query?: Query}`, except the kinds `conversion`
  (µs(datetime), tidTime) and `power` (power(k, x)), whose input names the function's
  arguments."
- **Namespace:** "Vector files carry `ns`, the NS under which keys, CIDs and `inputDigest` were
  derived. Runners evaluate the fold with NS = `ns` (the fold takes the namespace as a
  parameter, or the runner is built with that NS)." This resolves the conflict with
  `data-model.md` §1.3 for `.temp` CI builds.
- **time cases:** replace "missing or unparseable `createdAt`" (G2 already excludes such items)
  with "`createdAt` before 1970 (no claimed time)".
- **Floats:** "The vectors' `hotBits` are exact for any pow with an error below 0.9 ulp."

---

## B. Tags and wiki readings (new vectors `karma/tags.json`)

### B1. §13.2 flags and display values (NOTES 56)

**Add to §13.2 Rules:**

> - `Root(ParentUnavailable)` is defined for candidates: r ∈ Cand with a non-empty `parents`
>   and `par(r) ∩ Cand = ∅`.
> - "Ancestor altered" applies to the edges that remain after the cycle cut. If a uri appears
>   in `parents` more than once, any entry whose CID differs from `cid(q)` marks the edge.
> - "N unmerged edits" counts the candidates other than `canonical` that no edge points to.

### B2. §13.3 relevance (NOTES 58)

**Add:** "`own` does not require author(p) ∈ E. Tag-vote entries are filtered by OP3 (unknown
value) before OP4 (one slug: equal entries collapse, conflicting entries are all ignored)."

### B3. §13.2 which pages a client shows (NOTES 57) (informative)

A slug is known when a wikiRevision of that slug survives V1, or its `tag:` subject has a
forum-tier assertion; `exists` stays the only normative predicate.

---

## C. Standing readings to confirm (no change requested by the review)

- **§4.10 / §4.12 `lapsed`** covers every forum-scope assertion by an actor ∉ A (NOTES 4).
- **§4.4 `allowed(p)`** keeps entries whose rkey starts with `{fk}-{threadTid(p)}-` (MA9); an
  entry naming a comment that is not in `records` is kept (NOTES 24).
- **§4.4 `until`** before 1970 has no µs and has therefore expired (NOTES 22).
- **data-model.md §3.1 TIDs** are unsigned 64-bit values; tidTime ranges up to 2^54 − 1
  (NOTES 16).
- **§11** A Missing placeholder is a child of the root at depth 1, one per distinct parent uri
  that is not a node (NOTES 21).
- **§4.12 mod log** lists valid revisions of modActions whether or not the modAction is still
  in `records` (NOTES 27).
- **§10.2 / §12** the thread-pass and ordering readings of NOTES 47, 49 and 50.
