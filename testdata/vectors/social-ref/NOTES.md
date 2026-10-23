<!-- SPDX-License-Identifier: CC0-1.0 -->

# social-ref: notes on the reference model

`lessdugin_ref/` is an independent reference model of the lessdugin social fold. It was written
only from `docs/spec/social.md`, `docs/spec/data-model.md` (§3 keys, §6 records, §7 validity),
`docs/spec/limits.md` and the lexicons. The Rust fold was neither read nor consulted (N-version
discipline). The model generates the normative vectors under `testdata/vectors/`, and the Rust and
TypeScript implementations must pass them. File formats and projection fields are documented in
`testdata/vectors/README.md`, section "Social fold vectors".

```sh
python3 -I -B testdata/vectors/social-ref/gen_social.py           # regenerate every file
python3 -I -B testdata/vectors/social-ref/gen_social.py --check   # exit 1 if a file differs
python3 -I -B testdata/vectors/social-ref/gen_social.py --list    # files and vector counts
python3 -I -B testdata/vectors/social-ref/gen_social.py --emit-shared DIR   # see "thread/" below
```

Always pass `-B` (or set `sys.dont_write_bytecode` before importing `lessdugin_ref`): Python
writes a package's own `__init__` bytecode before the package can switch caching off, so an
import without it leaves `lessdugin_ref/__pycache__/` behind.

Only the Python 3 standard library is used. Keys come from `testdata/vectors/keys/gen_keys.py`,
imported by path. Every record in a vector has its genuine DAG-CBOR CID. The DAG-CBOR encoder
is self-tested against `testdata/interop/data-model` on every run. Each case module asserts its
hand-derived expectations against the model before writing, so a model regression fails
generation.

Where the spec is ambiguous, the model takes the reading that the spec's own decisions
(`social.md` §20, `data-model.md` §14) support. Those readings are listed below and are open to
review, like §20. Where a reading departs from the literal text or fills a gap, the review of
this stage proposed normative wording for the spec owner to fold into `social.md` §20 and
`data-model.md` §14; a decision that a vector pins says which vector.

## Decisions

1. **Rule IDs are not asserted.** `data-model.md` §7 does not fix the order in which rules are
   checked, so "first failing rule" is not portable. The model checks, in this order: G1; G2 and
   G3 (lexicon); G4; G5, which includes G9 and the key equalities of FO1, ME1, PO1, CO1/CO2,
   RV1/RV3, OP2, MA2, WR1 and SQ1; G6, G7, G8 and G10; then the remaining per-collection rules
   in numeric order. A vector with a deliberately invalid record names the rule it breaks in
   `note`, and asserts only the consequence (for example `Excluded`). `data-model.md` §14 item 23
   says the first failing rule is exact, so §7 should state this order (proposed).
2. **Namespace.** Vectors use the final namespace `online.7778777.lessdugin`, recorded as the
   file-level field `ns`. Every `fk`, `h16` and CID depends on the namespace (`data-model.md`
   §1.3), so a runner cannot map prefixes by rewriting strings. It evaluates the fold with
   NS = `ns`, the same convention as `keys/rkeys.json` (`input.ns`). A `.temp` build cannot do
   that while NS is only a build-time constant: the Rust runner either runs these vectors in a
   build whose NS is the file's `ns` (the `final-ns` feature) or gives the fold and validity
   the namespace as a runtime parameter, as the key functions already take it.
3. **An Excluded root is a Missing root.** §11 step 1 makes the root `Missing(p)` "when
   p ∉ records". A root post that is in `records` but Excluded by V1 (invalid, or another
   forum) is treated the same way. The thread gets a `Missing(p)` root (`rootMissing: true`),
   and its comments take `t_decl` as their time lower bound (§2.2, "if the root post is not in
   records"). This follows §11 step 2, which treats invalid and excluded parents as missing, and
   V1, under which Excluded items appear nowhere. It departs from the literal "p ∉ records" of
   §11 step 1 and §2.2. Pinned by `time/time.json` `root-excluded-bounded-by-decl` (T0 + 1 h,
   not T0 + 5 h), `thread/thread.json` `root-excluded` (`rootMissing`) and, through decision 44,
   `modfold/author.json` `author-tier-needs-the-root-post`.
4. **`lapsed` covers every non-authority actor.** §4.10 says that assertions by actors not in A
   have no forum-tier effect, and §4.12 defines `lapsed` as "the actor is no longer an
   authority". The fold cannot tell a former moderator from a user who never was one, so every
   forum-scope assertion by an actor ∉ A is `lapsed`.
5. **Mod-log status precedence for a forum-scope record** follows the numbered conditions of
   §4.3: `reserved/unknown` (MA2 reserved key, MA7, MA9), then `lapsed` (condition 2), then
   `unauthorised` (rank and role, conditions 3–4), then `expired` (`until`, condition 5), then
   `stale` (CID, condition 6), then `effective` or `overridden`. The first one that applies is
   the status. The README spells it out, and `modfold/forum.json` `mod-log-status-precedence`
   pins it (each record there fails two or three conditions).
6. **`effective` versus `overridden`.** A counting forum-scope record is `effective` if at least
   one dimension it asserts (other than the derived `spam`) resolves at the record's own rank to
   the record's value. For `restrict`, any value counts, because equal-rank sets unite.
   Otherwise the record is `overridden`, and `rank` is the highest rank among its dimensions'
   resolutions. Two consequences:
   - A record at the winning rank whose value equals the winning value is `effective` even when
     another record (with a smaller uri) is the winner.
   - In an unordered `tag.head` conflict, both records are `effective` through `tag.removed`.
     The conflict itself surfaces as a mod-queue item.

   Pinned by `modfold/forum.json` `equal-rank-winner-is-the-smallest-uri`,
   `restrict-equal-rank-sets-unite`, `empty-restrict-at-higher-rank-lifts`,
   `tag-head-conflict-at-equal-rank`, `tag-head-equal-values-agree` and
   `lock-allow-lists-intersect-at-equal-rank`.
7. **A stale `#tag.head` excludes only the head.** §4.5 says the head "counts only if…". A
   `#tag` record whose head fails that check still asserts `tag.removed`. Its mod-log status is
   `stale`. It becomes a mod-queue item when `head.uri` is in `records` with another CID, and
   not when the revision is absent. Pinned by `tag-head-bound-by-its-cid`.
8. **`Removed.spam` is `effective(x.uri, spam)`**, not the winner record's own `spam`. `spam`
   is a resolved dimension (it voids opinions, O5). `reason` and `showReason` are attributes of
   the winner of `hidden` (§4.4). The readings differ only when equal-rank hides disagree on
   `spam`; `modfold/forum.json` `removed-spam-is-the-resolved-spam` pins this one.
9. **Applicants exclude authorities.** Read literally, §3.3's "member(d) ∧ access(d) = none"
   includes the owner, whose membership `forum create` writes (`data-model.md` §14 item 12), and
   any authority without a grant. Authorities are already participants, so
   applicants = { d ∈ members ∖ A : access(d) = none }, in approval mode only. Pinned by
   `modfold/participation.json` `approval-mode-sets` (`applicants`, `modQueue`).
10. **Mod-log time `None`.** µs(createdAt) is `None` for a valid datetime before 1970. Such an
    entry sorts after every entry that has a time, then by uri. Pinned by `modfold/forum.json`
    `mod-log-untimed-entries-last`.
11. **The mod-queue projection** holds three kinds of item:
    - `stale`: a positive assertion whose subject is in `records` with another CID, or a
      `#tag.head` that names a revision in `records` with another CID;
    - `conflict`: an unordered `tag.head` conflict;
    - `applicant`.

    Items are sorted by (type, key bytes). Open-mode newcomer items are not projected, because
    they follow from the visibility states (§4.12; the queue is local presentation). A stale
    assertion whose subject (or `head.uri`) is not in `records` is not a queue item: nothing was
    edited (decision 23).
12. **MA3 exempts forum-uri subjects.** A `#lock` on the forum uri, which MA9 makes valid but
    without effect, has a slug key, so `fkOf(rkey(subject)) = fk(forum)` cannot hold for it.
    MA3 therefore applies only to subjects in forum-scoped collections. Otherwise MA9's "valid
    but without effect" could never be reached. This corrects `data-model.md` MA3 (proposed);
    pinned by `modfold/forum.json` `forum-uri-lock-has-no-effect`.
13. **Reserved keys** (MA2, `^FK-TID$`) are checked against G1–G10 and MA10–MA12 only: MA1 and
    MA3–MA7 are skipped, because the P2 actions that will use the key form may take subjects and
    values v1 does not know (`data-model.md` MA2, `social.md` §20.1 item 43). Such a record is
    valid, has no effect and is `reserved/unknown` in the mod log. (Until M3 the model checked
    MA3–MA6 for them; §20.1 item 43 did not adopt that reading.) Pinned by `modfold/forum.json`
    `reserved-key-skips-ma3` (one subject in another forum's key range, which MA3 would reject, and
    one that is no DID, at-uri or tag, which MA1 would reject).
14. **MA10 applies under MA7.** MA7 says MA10–MA12 still apply, so a `#tag` with an unknown
    `scope` whose `head` lies outside the forum's fk is invalid. (The earlier partial model
    skipped this; fixed in this stage.)
15. **A revision has no `forum` field.** The G4 `forum` check and G9 do not apply to it; its key
    range comes from `P(subject)` (RV3). (The earlier partial model failed on revisions; fixed.)
    `data-model.md` G9 lists `revision` among the forum-scoped collections, which is an error
    (proposed: exclude it and point to RV3).
16. **TID values are unsigned.** A TID has 13 characters, and its first character
    (`[2-7a-j]`) carries 4 bits, so its value ranges up to 2^64 − 1 and `tidTime = value >> 10`
    up to 2^54 − 1. Implementations MUST NOT parse TIDs as i64. Vectors carry tidTimes only up
    to 2^53 − 1 (decision 18).
17. **µs(datetime)** accepts exactly the atproto datetime syntax of `testdata/interop/syntax`:
    - at most 64 characters, and 1 to 20 fraction digits when there is a `.`: the limits of the
      atproto reference implementations (Go and TypeScript) and of `crates/atp`. With 20
      fraction digits a datetime has 46 characters, so the length limit never binds. A longer
      fraction makes the datetime invalid, and so the record (G2);
    - uppercase `T` and `Z`;
    - real calendar dates;
    - no `-00:00` offset;
    - no leap second (second ≤ 59). The fixtures reject `:61` and do not test `:60`; the
      reference implementations reject `:60`, and no vector uses it.

    Fraction digits beyond the sixth are truncated, and an instant before 1970 gives `None`.
    `time/conversions.json` pins all of this (`twenty-fraction-digits`,
    `twenty-one-fraction-digits`). `social.md` §2.1 should state the limits (proposed). CIDv0
    strings (`Qmb…`) are not CIDs.
18. **JSON-safe integers.** Every integer in a vector file lies within ±(2^53 − 1), so the
    TypeScript runner reads it exactly. The generator enforces this. Floats are never written:
    `hot` values appear as bit patterns.
19. **Input strictness.** The parser rejects malformed inputs (`InputError`); no vector contains
    one. The rules:
    - `records` is strictly sorted by uri bytes; `obs` by (uri, cid); `absent` by bytes;
    - every `absent` uri is the subject of an opinion in `records` and is not itself in
      `records`;
    - `records` holds only memberships with rkey `fk(F)`, plus the ranged collections within
      `{fk}-`;
    - `algorithmIds`, when given, equals the effective IDs derived from `decl`;
    - an invalid `decl` is an input error, because the forum cannot be opened (§1.6, FO4).
20. **CO5 compares only a valid root** (`social.md` §20.1 item 51, `data-model.md` CO5). If a
    valid root post is in `records`, its `value.forum` must equal the comment's `forum`; an invalid
    root is not compared. It is a Missing root (decision 3), so its comments stay in the thread
    under the placeholder and the opinions on them count. CO5 then fails only on a 60-bit fk
    collision: a valid root in F's key range names a forum with F's fk (G9). Until M3 the model
    compared an invalid root's `forum` too (the reading first adopted as §20.1 item 36), which let a
    post author exclude a whole thread by editing the root's `forum`. Pinned by
    `visibility/visibility.json` `v1-before-v2-and-v3` and `co5-invalid-root-is-a-missing-root`,
    and `karma/staged.json` `opinions-that-never-count` and `co5-invalid-root-opinions-count` (a
    root post whose `forum` names another forum is invalid by G9; its comments are Visible and
    the opinions on them count).
21. **A Missing placeholder** is a child of the root, at depth 1. The `parent` map points a
    comment at the placeholder's uri, and `missing` lists placeholder uris. One placeholder
    exists per distinct parent uri that is not a node of the thread.
22. **`until` before 1970.** A valid datetime before 1970 has µs = `None`, so `µs(until) > now`
    is false and the assertion has expired.
23. **A positive assertion on a subject that is not in `records`** has status `stale`, because
    §4.5 says it "never counts". It is not a mod-queue item, since nothing was edited. Pinned by
    `positive-assertion-on-a-subject-not-in-records-never-counts`.
24. **The `allowed(p)` entry filter** keeps entries whose rkey starts with
    `{fk}-{threadTid(p)}-` (MA9's wording). An entry that names a comment not in `records` is
    kept. A comment of another post with the same TID passes the filter but is never consulted,
    because V7 only looks at comments of p.
25. **The banner.** Every knownValue of the declaration that falls back sets
    `newerRulesBanner` (§1.6: "an unknown knownValue takes the fallback … plus the banner").
    The declaration has exactly ten knownValue fields: `membership`, `defaultTier`,
    `moderators[].role`, `policy.moderation`, `policy.voting`, `policy.reactPalette`,
    `policy.tagCreation`, `policy.karma.algorithm`, `policy.ranking.algorithm` and
    `policy.display.newUsers`. This includes `membership`, `defaultTier` and the moderator
    role, for which the `data-model.md` §13.3 table does not mention the banner. An unknown
    role raises it in any `moderators` entry, also one that FO2 ignores for another reason (the
    owner's own entry, a later duplicate). Unknown values in other records (opinions,
    modActions, posts) never raise it. Pinned by `modfold/participation.json`
    `unknown-membership-falls-back-to-approval` and `moderators-entry-rules`, and by
    `fold/digest.json` `unknown-known-values-fall-back`.
26. **Newcomers follow §3.1 literally**: P ∖ (S ∪ E). A banned or nullified participant is
    therefore a newcomer, in open mode and in approval mode alike (V4 hides a banned user's
    content first). In approval mode those are the only newcomers, since P ∖ S ⊆ B ∪ N there.
27. **Mod-log revisions.** A revision is listed when it is valid and its subject's collection
    is `modAction`. The subject need not be in `records`, and a `delete` revision usually names
    a removed record. A revision of a post is not a mod-log entry.
28. **Participation vectors** use kind `modfold` (file `modfold/participation.json`), because
    §3 belongs to `mod/1` and the modfold projection already carries P, G, B, N and S.
29. **Builder conventions** (informative):
    - DIDs are `did:plc:` plus a readable name padded with `2` to 24 characters;
    - `T0` = 2026-01-01T00:00:00Z and `now` = T0 + 30 days, unless a vector says otherwise;
    - `obs` defaults to each version's claimed time (`updatedAt ?? createdAt`);
    - `repoStatus` defaults to `fresh` for every record owner and authority.

### Karma, scores, gates and reacts (`karma/1`, §5–§9)

30. **`counting` is O1–O5, literally.** An opinion whose subject is in `absent` satisfies O2
    and is listed in `counting` when O1 and O3–O5 hold, even when the deleted-subject rule
    then gives it no effect: a wikiRevision or sequence subject, or fewer than 2 eligible
    voters in a stage. The rule limits what such an opinion counts toward (karma only, per
    stage); it does not stop it from counting. V (§6.2) is the subset with an enabled karma
    axis and a post or comment subject. This departs from §5's "counts **only** toward karma,
    and only if" read as a condition on counting; pinned by `karma/staged.json`
    `deleted-subject-one-vs-two-voters` (`counting` lists mike's and mona's opinions on a
    deleted wikiRevision, and mona's opinion on dora's deleted post, its only voter).
31. **O2 "s is valid … and s.forum = v.forum"** means that s is a candidate item not Excluded
    by V1. That includes the cross-record rule CO5, which `data-model.md` §7.3 lists among the
    validity rules: an opinion on a comment whose valid root post names another forum does not
    count (decision 20: only an fk collision can make that happen). O3 and O5 read the subject uri, so they apply to absent subjects too; a
    forum-scope `#hide{spam}` on a deleted uri is a negative assertion and counts without its
    subject (§4.5), voiding the opinions on it.
32. **Axes are resolved per counting opinion** (§5 table):
    - karma: the value if known, unless the voter is restricted from `vote`;
    - agreement: the value if known, unless restricted from `vote` or `voting = karmaOnly`;
    - reacts: enabled iff `voting = reacts`, the voter is not restricted from `react` and the
      subject is not absent; palette and stance are entry rules (OP3);
    - tags: enabled iff the subject is a post, not absent, and the voter is not restricted from
      `tag`; the OP3/OP4 entry rules belong to `tags.py`.

    An opinion with no enabled axis still counts and is listed in `counting` (for example a
    vote-restricted voter's). Restrictions have no keep exemption: keeps are item-level, while
    axis restrictions act on the voter's opinions.
33. **seedUp** sums sign(v.karma) per (seed, recipient) over included(v, S) without a clamp
    (the clamp keeps the sign anyway) and counts the seeds whose sum is > 0. A seed with net 0
    (one up, one down) does not count.
34. **Saturation.** Every sum and the cap pairCap × strong(·) use saturating i64 arithmetic
    (§0.4). Within the fetch caps nothing saturates, and no vector reaches it.
35. **qkey follows `lfm-1.md` §11 in its own order**: qkey = norm(trim_ws(exact)), where norm
    removes the §10.2 controls, applies NFC, and replaces White_Space runs with one U+0020.
    `social.md` §9.2 lists the steps ("NFC, White_Space runs → U+0020, trimmed") without an
    order, and `lfm-1.md` is the definition. Consequences, pinned by the reacts vector
    `qkey-normalisation`:
    - a control next to the trimmed edge leaves an edge space: `"teh ‎"` → `"teh "`, a
      different react from `"teh"`;
    - a quote of White_Space only has qkey `""` and groups with entries without a quote.

    NFC comes from Python's `unicodedata` (Unicode 15.1). The vectors use only compositions
    that are stable across Unicode versions (`e` + U+0301).
36. **React entries within one opinion.** An unknown stance or an id outside the palette is
    discarded before grouping, so it never creates a contradiction. Entries with the same
    (react, qkey) and the same stance collapse to one (a writer-rule violation that readers
    tolerate). The creator's `quote` (display only, never projected) is the first entry of the
    creator's group in array order.
37. **Reacts need `voting = reacts`.** Under `karmaOnly` and `twoAxis` there are no tallies at
    all (§7 table). Voters outside E are dropped before C, supporters and opposers are formed.
    A contradiction is decided within one opinion and does not depend on E.
38. **Gate projection fields.** `gateValues` and `passes` are registered by `gates.py` into the
    one field namespace of `projections.py`, because this stage owned `gates.py` and not
    `projections.py`. `passes` lists every DID of A and every DID that owns a record in
    `records`. Gate vectors live in `karma/gates.json` with kind `karma`, because gates are part
    of `karma/1` (§0.3) and need no kind of their own.
39. **`karma/power.json`** has kind `power`: input `{k, x}`, expected `{power}`, with no fold.
    It covers every ladder threshold t (t − 1 and t), k ∈ {−5, 0, 10⁹, ±(2^53 − 1)}, the four
    karma values everywhere and the four agreement values at five k.
40. **Scores.** `scores` lists every candidate item not Excluded by V1, including Hidden items
    (§7). Opinions on absent subjects contribute nothing. W(i) needs voter ∈ E for both axes,
    so the agreement of a voter outside E is neither weighted nor unweighted. `weighted` is the
    M of "N of M": |{v ∈ W(i) : karma axis enabled}|.
41. **E uses P from L2 directly.** In approval mode E = S follows from P ∖ S ⊆ B ∪ N (§3.2)
    and is not special-cased.
42. **Authorities pass every gate even when banned or nullified.** passes(u, g) only asks
    u ∈ A, and a ban or nullification never removes authority (§4.3). Their opinions do not
    count anyway (O4).
43. **The reacts vectors' cast** (informative): a pool of 20 moderators ("boosters") upvotes
    one post of a user to give that user an exact K (K1 = KP = K = k, established when k ≥ 2).
    The reacts projection cannot list karma fields, so each vector's note states the K of its
    reactors.

### Property self-check of §6.3 (informative)

The stage ran a randomized check of P1–P7 against the model: 1,000 random open and approval
forums (random pairCap 1–3 or 25, establish 0–2, establishSeeds 1–3, voting, gates; grants,
bans, nullifications, vote and react restrictions, spam hides; posts, comments, wikiRevisions,
sequences; 26,878 opinions with unknown values and reacts; deleted subjects in `absent`).
Every generated record was valid. Results: no failure.
- **P1:** two random permutations of every input map per case give identical karma, scores,
  gates, reacts and visibility outputs.
- **P2** holds by construction: the input is a map keyed by uri, and the vector format rejects
  duplicates. **P3** holds by inspection: `karma.py` makes one pass over V per stage, and each
  stage reads only the stage before it.
- **P4:** adding reacts and tags entries to opinions, wikiRevisions, sequences, opinions on them
  and author-scope `#hide` records never changed K1, seedUp, E, KP or K.
- **P5:** every pair stays within its stage's cap. The corollary held in 969 cases: removing
  all opinions of a voter u ∉ S without deleted subjects left K1, seedUp and E unchanged, and
  moved each KP[r] by at most pairCap × strong(max(K1[u], 0)).
- **P6:** S ⊆ E in every case.
- **P7:** adding up to 12 records by outsiders (new DIDs and existing DIDs ∉ E ∪ A: posts,
  comments, opinions with reacts, memberships subject to hypothesis (b), lapsed `#hide{spam}`
  records), subject to hypotheses (a) and (b), left K1, seedUp, E, KP, K, every score and
  agreement of I, and the V1–V10 state of every item whose author wrote nothing in R
  unchanged.
- **P8** has no counterpart, because the model has no incremental fold. **P9** is approximated:
  `python3 -I` randomises string hashing per process, and three runs gave byte-identical files.

Three injected faults were caught: seedUp counted from all voters (P7), K1 from all voters
(P5 corollary), and no pair cap (P5).

### Author tier, visibility and ranking (`mod/1` §4.9, §10; `magic/1` §12)

44. **Author tier: root(c) must be a live post.** §4.9 requires root(c) = records[c.root.uri] to
    exist for any author-tier effect on c. The model also requires it to be a post that V1 does
    not exclude (decision 3: an Excluded root is a Missing root), and c to be a comment that V1
    does not exclude. An author-scope `#hide` or `#pin` whose subject has no such root, and a
    `#lock` whose subject is not such a post, fail the table condition and are `unauthorised`
    (vector `author-tier-needs-the-root-post`; under the literal "must exist" the comment on the
    invalid root would be `Hidden(AuthorModerated)` and the hide `effective`). Wording for §4.9
    is proposed.
45. **Author tier: mod-log status.** authorEffective is checked in the order of §4.3's numbered
    conditions, as decision 5 does for forum scope:
    - `unauthorised`: authorModeration = false, the actor ∉ E, or the table condition fails
      (thread, author gate, immunity);
    - then `expired` (`until`);
    - then `stale` (the CID check of a positive pin);
    - then `overridden` or `effective`.

    A record that passes is `overridden`, with the rank of the forum-tier value, when the forum
    tier resolves its subject and dimension: `hidden` for `#hide`, `locked` for `#lock`,
    `pin.thread` for `#pin`. Otherwise it is `effective`, which includes `#lock{locked: false}`
    and `#pin{pinned: false}` (they lock or pin nothing). A stale author pin is not a mod-queue
    item, because the queue projection lists forum-scope items only (decision 11).
46. **Author tier: bans.** The table gives `#access{banned}` no condition beyond d ∉ A, so its
    status ignores the author gate. authorGate(X, p) is applied per comment, with p = root(c),
    as authorBanned(c) says: a ban works in the threads of X's posts that pass the gate, and
    nowhere else. The forum tier governs a ban only through the comment. The §4.9 precedence
    list says "an author ban applies to c only if keepRank(c) = 0", and AuthorModerated also
    needs resolve(c, hidden) = Default. A forum-tier `#access` on d does not lift an author ban.
    If a grant did, author bans could never act in approval mode, where every non-authority
    participant who is not banned is granted. An author ban is therefore never `overridden`.
    §4.9's general sentence ("for any subject and dimension that has a forum-tier Resolved
    value, the forum tier governs") contradicts this for the `access` dimension; the bullet list
    and the formula support the reading taken here, and wording is proposed. Pinned by
    `modfold/author.json` `author-ban-of-a-granted-user-in-approval-mode` (a granted user, whose
    `access` resolves to `member` at rank 3, is author-banned; a forum keep still exempts a
    comment).
47. **Thread pass.**
    - Listed descendants are judged on the states before the pass (Visible or Collapsed).
    - Excluded comments are not nodes of the placed tree, so they never keep a Tombstone.
    - The pass walks through Missing placeholders.
    - A Tombstone keeps the Hidden state's `reason` and `removed`; `removed.reason` stays null
      unless showReason, as for Hidden.
    - Only posts and comments become Tombstones. A Tombstone post is not listed: it is in no list
      and not in Recent Discussion, although its comments keep their own states.
48. **The lens (L7)** is modelled only as `visibility.lens()`: a listed item by a muted author
    becomes Collapsed(Muted). No vector asserts it. The visibility case module applies it to
    every vector and checks that no projected field changes.
49. **Comment order.** `commentOrder` is the pre-order of the displayed nodes (§10.2: listed or
    Tombstone comments, and Missing placeholders with a listed descendant).
    - Thread pins sort first among the siblings of every level, in every sort, `latestReply`
      included.
    - A Tombstone sorts by its own score and t_rank.
    - A hidden comment that is not displayed has no listed descendant, so leaving it out drops no
      displayed node.
50. **Activity and Recent Discussion.** `recentComments`, `latestReply` and Recent Discussion
    use the `Visible` state of comments, which the thread pass never changes. They consider
    every comment of the placed thread, including comments under Missing placeholders and
    re-attached comments. Recent Discussion lists every qualifying post, since §12.5 gives no
    limit. Posts whose newest Visible comments have equal t_rank are ordered by post uri.
51. **Rank queries.** The vectors always give `sort` and `filter`, so the default filter
    (frontpage) is never relied on. The `tag` filter keeps the posts with applies(p, t)
    (§13.3, `tags.py`); `rank/rank.json` `tag-filter` exercises it.
52. **Author vectors set the gates** (informative). Without opinions K = 0, so most author
    vectors set `gates.authorModPersonal` and `authorModFrontpage` to 0 (passes) or 1 (fails).
    `author-gate-49-50-personal-and-frontpage` uses the karma/1 defaults (50, 2000) with real
    karma from seed votes.

### Tags and wiki (`karma/1` §13)

53. **Where the tags vectors live.** The vectors are `karma/tags.json` and
    `karma/tags-large.json` (inputs of about 1,000 records), kind `tags`: §13 belongs to `karma/1`
    (§0.3), the precedent of `karma/gates.json` (decision 38). `social.md` §20.1 item 44 adopted
    this, so §19 no longer names a `tags/` directory.
54. **The page** of a slug holds the wikiRevisions with that slug that V1 does not exclude and
    V2–V5 do not hide (`visibility.mod_state` is `None`; V6 and V7 concern comments only). A
    revision by a non-participant is therefore on no page (V2). The 1,000-revision cap keeps the
    greatest (TID of the rkey desc, uri asc) and sets `partial`. Pinned by `karma/tags-large.json`
    `page-cap-1000` (not partial) and `page-cap-1001` (partial; of two revisions with the lowest
    TID, the smaller uri is kept).
55. **Candidates and the cycle cut** follow §13.2 literally. `genesisOK(r)` does not look at
    `parents`, so a revision whose author passes `createTag` (or is an authority under
    `tagCreation = moderators`) is a candidate whatever its parents. The cut is recomputed until
    no cycle remains; in each round every cyclic SCC (size > 1, or a self-loop) loses the edges
    from its greatest-uri member into the SCC.
56. **Flags and display values** (projected by `wiki`):
    - `parentUnavailable(r)` ⇔ r ∈ Cand, r has a non-empty `parents` field, and
      par(r) ∩ Cand = ∅ (§13.2 "a deleted parent gives a flagged root"); only candidates are
      projected, because a non-candidate is never displayed;
    - `altered` lists, per candidate, the parents along its edges *after* the cycle cut whose
      stored `parents` entry has a CID ≠ cid(q); when one uri is listed twice with different
      CIDs, any differing entry marks the edge (`karma/tags.json` `duplicate-parent-entries`);
    - `history` is the chain from `canonical`, each step to the parent with the greatest depth,
      ties by the smallest uri; `unmerged` = |{ r ∈ Cand ∖ {canonical} : no edge into r }|.
57. **Which slugs `wiki` lists**: every slug of a wikiRevision that V1 does not exclude (even if
    every revision is hidden, so the page is empty), and every slug whose `tag:` subject has a
    forum-tier assertion of `tag.removed` or `tag.head` (resolved or in conflict). A slug that
    only appears in `post.tags` or tag votes is not listed; its relevance is in `tagRel`.
58. **Relevance.** `own` does not require author(p) ∈ E: §13.3 asks only that the author is not
    restricted from `tag` (own = max(1, small(max(KP, 0))), which is 1 unless KP ≥ 1000). Tag
    votes need a counting opinion on a post, voter ∈ E and the tags axis (§5: not restricted
    from `tag`); OP3 is applied before OP4 (the order in which `data-model.md` §6.6 lists
    them: an unknown value is discarded first, then equal entries for one slug collapse and
    conflicting ones are all ignored). The sum uses saturating i64 arithmetic. `tagRel` lists
    the non-zero rel of every post that V1 does not exclude, hidden posts included; `applies`
    lists the slugs with applicable(slug) ∧ rel > 0.
59. **The canonical tie-break uses K** (the displayed karma, §6.2) of the revision's author, as
    §13.2 writes `K[author]`.

### Other review findings

60. **V1's `x.forum ≠ F` and O1's `v.forum = F`** can only fail through a 60-bit fk collision:
    every record in `{fk}-` ranges names a forum with that fk (G9) or is invalid (G5). They are
    defence in depth (§20.1 item 23). The vectors that put another forum in `value.forum`
    therefore test G5/G9 and say so in their notes.
61. **`hotBits` are portable.** For every `hotBits` value the rank case module checks, with
    exact rational arithmetic, that the exact pow(base, e) lies within 0.1 ulp of the double
    `math.pow` returned, so any pow whose error is below 0.9 ulp returns the same double; the
    remaining operations are exact or correctly rounded IEEE arithmetic. At M3 every `hotBits`
    value of `rank/rank.json` (19 values) was also recomputed with the `libm` crate 0.2.16 (the
    version the workspace pins), by a scratch Rust program that calls `libm::pow` directly and
    reads only the inputs of hot() (b, age, offsetHours, exponentMilli) dumped by the model: all
    19 bit patterns are equal, and libm's pow equals the platform pow on all of them. The file
    therefore carries `"hotBitsSource": "rust-libm-0.2.16"`, and runners compare bits exactly.

### M3 integration (`social.md` §20.1 items 43–51)

62. **CIDs written in records compare by canonical form** (`social.md` §0.4, §20.1 item 45;
    `lessdugin_ref/cids.py`). canon(s) decodes the multibase (base16, base32 and base32pad in both
    cases, base32hex, base32z, base36, base58btc, base58flickr, base64 and its url and padded
    forms; strictly: one case per alphabet, no non-zero trailing bits), requires the bytes of a
    CIDv1 (version 1, codec, multihash code and length as minimal varints, nothing after the
    digest) and re-encodes it as multibase `b`. A string that is not a CIDv1 names no version.
    A prefix of the other multibases (identity, base2, base8, base10, proquint, base45,
    base256emoji) raises NotModelled. It is used by the §4.5 CID check (forum and author tier,
    positive assertions and `#tag.head`, whose resolved value is the canonical form, so two
    spellings of one head agree), `earlier` (§7), "ancestor altered" (§13.2), versions (§11 step
    6), and RV5, which compares CIDs (`data-model.md`). No earlier vector changed. Pinned by
    `modfold/forum.json` `positive-assertion-cid-spelling` (multibase `B`, `z`, and a non-CID),
    `tag-head-spellings-agree` (also in `karma/tags.json`), `karma/scores.json`
    `earlier-ignores-cid-spelling` and `thread/thread.json` `versions-on-revision-n`.
63. **Versions and the edited badge** (`social.md` §11 step 6 and "Edited", §20.1 item 46;
    `lessdugin_ref/versions.py`, fields `versions` of kind `thread` and `edited` of kind `time`).
    `versions` lists every non-Excluded comment of the input, whatever the query's thread. A
    parent comment that is in `records` and not Excluded but under another root is a Missing
    placeholder in the comment's thread (§11 step 2), and §11 step 6 does not say whether a
    reference to it shows a version: the model raises NotModelled there, and no vector has one.
    The `edited` badge reads valid `op: update` revisions and `updatedAt` against `createdAt`
    (a `createdAt` before 1970 counts as earlier; an `updatedAt` before 1970 never counts).
64. **The K ≠ KP cast** (informative, `cases/kcast.py`). Every other cast has K1 = KP = K for the
    users that matter. This one (approval mode, everyone granted) gives vera KP 10 and K 9 and wes
    KP 1 and K 2, so the vectors `karma/staged.json` `k-and-kp-differ`, `reacts/reacts.json`
    `creator-by-k`, `karma/tags.json` `canonical-tie-by-k` and `tag-vote-power-kp`, and
    `modfold/author.json` `author-gate-reads-k` each fail a fold that reads the other value.
    `karma/tags-large.json` `own-tag-weight-kp` needs KP 1,000 (small() changes there) and builds
    its own cast of 40 boosters.
65. **M3 review cases.** The other vectors added at M3 come from the reviews of the Rust fold
    (`social.md` §20.1 items 18, 31, 32, 34, 40, 41, 47), each computed by the model and asserted
    against a hand derivation in its case module: `rank/rank.json` (Recent Discussion ties and
    collapsed posts, recentComments and latestReply activity, thread pins in every comment sort,
    forum pins only in magic, tombstones through placeholders and re-attached comments),
    `visibility/visibility.json` (the same tombstones), `time/time.json`
    (`orphaned-comment-bounded-by-root` reworked so that the bound binds, the
    `decl-obs-moves-a-non-future-item-*` pair, `edited-badge`), `fold/digest.json` (the pair
    `decl-obs-is-outside-the-digest-*`: equal digests), `modfold/forum.json` (`until` before
    1970, lock allow lists, untimed mod-log entries), `modfold/author.json` (author unpin,
    author lock on an invalid post), `karma/tags.json` (history ties, duplicate parents, OP3
    before OP4).

## `thread/`

An earlier stage wrote `thread/thread.json` and `thread/depth.json` (the 1,002-comment depth-cap
vector in its own file) when `testdata/vectors/thread/` did not exist. Until M3 the directory was
shared with the Rust engine agent, so the generator treated it as read-only:
- `cases/thread_cases.py` exposes its cases as `SHARED`; they are built and their hand
  expectations asserted on every run, but written only with `--emit-shared DIR` (for example
  into the scratch directory, for the directory's owner to adopt);
- every run cross-checks **every** vector of every `thread/*.json` file, whoever wrote it: it
  folds the vector's input with the model and compares each listed `expected` field with the
  model's projection. A disagreement fails the run (including `--check`); a field or input the
  model does not model is reported and skipped;
- the run also says whether each shared file is identical to the model's own rendering.

The last cross-check (before M3) found both files identical to the model's own cases. At M3 the
generator took the directory over (it is in `OWNED_DIRS`, and `cases/thread_cases.py` exposes
`FILES`); the cross-check machinery stays for a directory another producer may share
(`SHARED_DIRS`, empty at present). `thread/thread.json` gained `versions-on-revision-n`
(decision 63).

## TODO (next stages)

**Other kinds.** `karma` and `reacts` are done (decisions 30–43); the author tier,
`visibility` and `rank` (44–52); `tags` with the `tag` post filter (53–59). One stub remains:
- `sequences` (§14), the sequence library; it has no vector kind yet (§19 requires none).

The interfaces are in each stub module's docstring and in `fold.py`. The projection field names
and shapes are already fixed in `projections.py` and the README.

**Properties** (§19 property tests) are not vectors and are out of scope for the model. The
karma stage ran a one-off randomized check of §6.3 P1–P7 (see "Property self-check" above);
the Rust proptests remain the normative ones.

**Ownership.** `thread/` is owned by the generator since M3 (section `thread/` above), and the
tags vectors live under `karma/` (decision 53, `social.md` §20.1 item 44).
