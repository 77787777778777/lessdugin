<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin social semantics (normative)

**Status:** draft for the `.temp` namespace (M0). Each behaviour ID defined here is frozen when it is first released (M7).
**Sources:** `docs/design/synthesis-v1.md` §4 (with §1, §3.2 and §10), with every fix from `docs/design/completeness-review.md` applied. Where they differ, the approved plan wins. Critique IDs (F*, H*, X*, C*) refer to `docs/design/archive/critiques.md`.

## 0. About this document

### 0.1 Key words

MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

**"Reserved (Pn)"** marks a name or behaviour that phase Pn will define. Until then, v1 clients MUST NOT write it, and MUST give it no effect when they read it.

### 0.2 Scope

This document defines how every lessdugin client, and the optional indexer, turns a forum's records into a forum view. That covers:
- authorities, participation and seeds;
- moderation;
- opinions, vote power, karma, scores and agreement;
- gates;
- reacts;
- visibility;
- threads;
- time and ranking;
- tags and wiki;
- sequences;
- notifications and local state.

| Companion document | Covers |
|---|---|
| `data-model.md` | Lexicons; record keys (`fk/1`, `sk/1`); record validity, with rule IDs G\*, PO\*, CO\*, RV\*, OP\*, MA\*, WR\*, SQ\*; fetch caps (§7.4); the write protocol, including `mod adopt` and lock writing (§8.6); reserved names (§9); unknown-value fallbacks (§13.3) |
| `lfm-1.md` | Rendering, the plain-text projection P, quote resolution and `qkey` (§11), derived `links` and `mentions` (§12) (`lfm/1`) |
| `sync.md` | Trust levels and admission; tombstones; first-seen times; repo status; discovery and the closure; `proofs/1` |
| `limits.md` | Byte, count and depth caps |
| `reacts-default-1.json` | The `default/1` react palette (§9.1) |

### 0.3 Behaviour IDs

This document defines four behaviour IDs:

| ID | Selected by | Covers |
|---|---|---|
| `mod/1` | `policy.moderation` | §3 participation and seeds; §4 moderation (authorities, forum tier, author tier, fail closed, mod log); §10 visibility, the thread pass and the lens; §11 threads |
| `karma/1` | `policy.karma.algorithm` | §5 opinion validity; §6 vote power and karma; §7 scores and agreement; §8 gates and their defaults; §9.2 react aggregation; §13 tags and wiki; §14 the sequence library |
| `magic/1` | `policy.ranking.algorithm` | §2 time (`t_rank`); §12 ranking, sorts, filters and Recent Discussion |
| `default/1` | `policy.reactPalette` | the set of react ids in `reacts-default-1.json` (§9.1) |

**Rules**
- A behaviour ID never changes meaning. Any change to a covered rule that could change any output for some input needs a new ID, for example `karma/2`. The old ID stays implementable from its vectors in `testdata/vectors/`.
- §1 (the fold contract) binds every ID.
- §15 to §18 define local presentation, or record deferrals. They are not behaviour IDs.
- An unknown ID in the declaration falls back to the default ID, plus the banner "this forum uses rules newer than your client" (`data-model.md` §13.3). The *effective* IDs are part of the fold input (§1.1).
- Other documents define the IDs referred to here: `fk/1` and `sk/1` (`data-model.md`), `lfm/1` (`lfm-1.md`) and `proofs/1` (`sync.md`).

### 0.4 Notation

```
F                      the forum's at-uri; fk = fk(F)
did(u), coll(u), rkey(u)   components of a canonical at-uri u
author(x)              did(x.uri) for a record x; the DID itself for a DID subject
kind(x)                post | comment | wikiRevision | sequence | membership | opinion | modAction | revision
records[u]             the fold-input record with uri u (absent if none)
cid(x)                 the CID of record x in the fold input
min_some(a, b)         the minimum of the values that are present; None if neither is
clamp(x, lo, hi)       max(lo, min(hi, x))
|X|, X ∖ Y             cardinality, set difference
µs                     integer microseconds since 1970-01-01T00:00:00Z
```

**Integers**
- Integers are signed 64-bit.
- Arithmetic saturates. Overflow cannot occur within the fetch caps, so saturation only keeps behaviour defined.
- An integer-valued map returns 0 for a missing key.

**Comparisons and identifiers**
- Strings (uris, DIDs, slugs) compare by their UTF-8 bytes. CIDs compare by the bytes of their canonical string form: CIDv1, base32 lowercase, multibase prefix `b`.
- "Smallest uri" means the smallest in that byte order.
- When a record x is used as a moderation subject, as in `resolve(x, d)` or `effective(x, d)`, it stands for `x.uri`.

### 0.5 Completeness fixes applied (informative)

| Fix (completeness review or plan) | Where |
|---|---|
| `#restrict` hides retroactively (rule 4b) | §4.7, V5 |
| The forum-wide lock is cut and reserved (P2); a lock is refused above 5,000 visible comments | §4.3, §4.8 |
| `tag:` subjects have rank 0 | §4.1 |
| modAction updates and removals go through revisions; the mod log shows them | §4.12 |
| `mods rm` runs `mod adopt` first | §4.10 |
| The membership lexicon default is `approval` | §3 |
| Stale third-party proofs are not fold inputs; fetch caps are normative | §1.1, §1.5 |
| `t_rank` uses the §4.2 clamp (D12 corrected) | §2.2 |
| The sybil property is restricted to K, scores and other users' visibility | §6.3 |
| The remark "`establishSeeds` = 1 in approval mode" is deleted (it has no effect there) | §3.2, §6.2 |
| The magic cutoff affects only the magic sort | §12 |
| Wiki: linear history in the MVP; a deleted parent gives a flagged root; forks, merges and `aliasOf` are reserved | §13 |
| Pingbacks are computed from rendered links; followed forums are seeded from the user's own memberships | §16 |
| Explicit ForumMagnum deferrals | §15 |

---

## 1. The fold

### 1.1 Contract

```
ForumView = fold(FoldInput, Query)

FoldInput {
  decl:         (uri = F, cid, value)        // the current declaration, admitted (pds | proven) and valid (FO1–FO4)
  records:      Map<uri, (cid, value)>       // one current version per uri; see "records" below
  absent:       Set<uri>                     // see "absent" below
  obs:          Map<(uri, cid), µs>          // first-seen time of each admitted version
  repoStatus:   Map<did, Fresh | Stale | Unavailable>
  now:          µs
  algorithmIds: (moderation, karma, ranking, reactPalette)   // effective IDs (§1.6)
}
Query = the requested list, sort, filter or thread (§12); it never changes any per-item value.
```

**`records`** holds every record admitted at level `pds` or `proven` (`sync.md`; `data-model.md` §7.1) that falls into one of these sets:
- `membership` records with rkey `fk`;
- records of `post`, `comment`, `revision`, `opinion`, `modAction`, `wikiRevision` or `sequence` in the `{fk}-` key range, after the normative fetch caps (`data-model.md` §7.4).

The following are never members:
- records of inactive repos;
- hints (Jetstream, Constellation, relay, Slingshot, indexer JSON);
- third-party proofs whose rev is below the author PDS's `getLatestCommit` head (`[stale]`).

Invalid records **are** members. V1 excludes them (§10.1) and the raw view shows them, so `inputDigest` covers exactly what was fetched.

**`absent`** is a set of uris. A uri u is in it if and only if all of these hold:
- u is the `subject.uri` of some opinion in `records`;
- u ∉ `records`;
- the sync layer has *confirmed* u's absence, either by a tombstone (`sync.md`, invariant I4) or by a complete listing of the `{fk}-` range of `coll(u)` in `did(u)`'s repo that did not contain u.

**`obs`** comes from the sync layer, which records a first-seen time for every version it admits (`sync.md`). In assisted mode it may instead come from the indexer's `firstSeen`. Parity tests inject it.

**`repoStatus`** is defined for every DID that owns a record in `records`, and for every authority:

| Value | Meaning | Records contributed |
|---|---|---|
| `Fresh` | The repo's in-scope ranges were completely listed at its latest known head | the listing |
| `Stale` | They were completely listed at an earlier rev | the cached records ("unknown is not absent") |
| `Unavailable` | They were never completely listed | whatever is cached, possibly nothing |

A DID that is missing from the map is `Unavailable`.

**`ForumView`** (informative; the serialisation is the engine's versioned DTO schema) contains:
- **Per user:** authorities and ranks; the sets P, G, B, N, S and E; newcomers; K1, KP and K.
- **Per item:** score, agreement, unweighted count, votes on earlier versions; react tallies; resolved tier, pins and lock; applied tags; visibility state; `t_rank`; flags.
- **Per forum:** threads; wiki heads; the sequence library; the requested orderings; the mod log, mod-queue items and raw annotations; `degraded`; banners.

### 1.2 `inputDigest`

`inputDigest` identifies a fold input. Indexers return it from `listPosts`, `getThread` and `getKarma` (`data-model.md` §11), and `--audit` compares it with the client's own value.

```
lines = { UTF8(u) ‖ 0x00 ‖ ASCII(cidString) ‖ 0x0A  :  (u, (cid, _)) ∈ records }
      ∪ { UTF8(u) ‖ 0x00 ‖ "-"               ‖ 0x0A  :  u ∈ absent }
D     = concat(lines sorted ascending by bytes)
      ‖ "decl"   ‖ 0x00 ‖ ASCII(declCid) ‖ 0x0A
      ‖ "algs"   ‖ 0x00 ‖ moderation ‖ "," ‖ karma ‖ "," ‖ ranking ‖ "," ‖ reactPalette ‖ 0x0A
      ‖ "params" ‖ 0x00 ‖ join(",", [name ‖ "=" ‖ value  for name in PARAMS]) ‖ 0x0A
inputDigest = lowercase hex of sha256(D)                    // 64 characters
PARAMS = membership, defaultTier, voting, authorModeration, tagCreation, pairCap, establish,
         establishSeeds, offsetHours, exponentMilli, curatedBonus, frontpageBonus, cutoffDays,
         reactSecond, reactCreate, reactAnti, authorModPersonal, authorModFrontpage, createTag,
         wikiEdit, sequence, collapseBelow, newUsers
```

**Encoding of values**
- Values are the effective policy values (§1.6).
- Integers are written as shortest decimal, with `-` for negatives. Booleans are `true` or `false`. knownValues are written as their string.
- A uri never contains 0x00, so sorting lines by bytes sorts by (uri, cid). A uri cannot be in both `records` and `absent`.

**What the digest does not cover:** `obs`, `repoStatus` and `now`. Two inputs with equal digests therefore produce equal outputs, except for:
- the `t_rank` of future-dated items (§2.2);
- `degraded` (§4.11);
- rules that depend on `now`: `until`, `hot` and the cutoff.

### 1.3 Determinism rules

1. **Purity.** The fold is a pure function of `(FoldInput, Query)`. It does no I/O and uses no clock, randomness or locale. Hash-map iteration order MUST NOT affect output: implementations use `BTreeMap` or sorted vectors.
2. **Integers.** Time is integer µs, and every tunable is an integer.
3. **Floats.** Floats appear only in `hot()` (§12.1), computed with the `libm` crate pinned exactly; an upgrade must pass `testdata/vectors/rank` unchanged.
   - Platform `pow`, fused multiply-add and fast-math are not used.
   - Values are compared with `f64::total_cmp`.
4. **Ties** break by uri bytes, then CID bytes, unless a rule says otherwise.
5. **`now`** is always an explicit argument.
6. **Identical output.** Native, wasm and indexer builds produce byte-identical serialised output for the same `(FoldInput, Query)`.
7. **Batch is the reference.** Any incremental implementation MUST be property-tested equal to the batch fold.
8. **Layering.** No rule reads a layer above its own (§1.4).

### 1.4 Layers

| Layer | Computes | Reads |
|---|---|---|
| L0 | authorities A and ranks (§4.1) | `decl` |
| L1 | forum-tier moderation state: `resolve` (§4.4) | L0, modActions, the existence and CIDs of records |
| L2 | participation and seeds: P, G, B, N, S (§3) | L0–L1, memberships |
| L3 | opinion validity (§5); K1, `seedUp`, E, KP, K (§6) | L0–L2, opinions, items, `absent` |
| L3.5 | score, agreement, unweighted (§7) | L0–L3 |
| L4 | gates (§8) | L0–L3 |
| L5 | author-tier moderation (§4.9), reacts (§9), tag relevance and wiki heads (§13), the sequence library (§14) | L0–L4, plus states from rules V1–V5 |
| L6 | visibility V1–V10 and the thread pass (§10); threads (§11); ranking (§12) | L0–L5, time |
| L7 | lens (§10.3) | L6, local state |

**Rules that sit outside the layer stack**
- Rules V1–V5 read only validity and layers L0–L2, so L5 may use their results.
- Time (§2) and thread placement (§11, steps 1–4) read only validity, `obs` and `decl`. Any layer may use them, but only L6 and display do.

### 1.5 Unknown is not absent; partial input

- **No silent drops.** `Stale` and `Unavailable` repos contribute their cached records. A record leaves `records` only after a complete listing without it, or after `RecordNotFound` (`sync.md`).
- **Fail closed.** Unavailable authorities put the view into `degraded` (§4.11).
- **Fetch caps** are normative (`data-model.md` §7.4): the first N keys in **descending** rkey order of each `{fk}-` range per collection, with N = 100,000 for `opinion` and 20,000 otherwise. Every client therefore excludes the same excess.
- **Completeness badges** (`[partial k/n]`, `[partial]`) are computed by the engine from sync state (`sync.md`). They are not fold outputs.

### 1.6 Effective policy

- An absent field takes its lexicon default (`data-model.md` §6.1). An absent `#gates` field takes the `karma/1` default (§8).
- An unknown knownValue takes the fallback in `data-model.md` §13.3, plus the banner. The fallback for `membership` is `approval`.
- An integer out of range invalidates the declaration (FO4). The forum then cannot be opened, which fails closed.
- The fold reads only the **current** declaration. Every policy change, including a change of mode or gates, therefore applies retroactively. This is inherent, and the forum settings UI says so.

---

## 2. Time (`magic/1`)

### 2.1 Conversions

```
µs(datetime) = parse with the atproto datetime syntax (any UTC offset), convert to UTC, and
               truncate fractional digits beyond the sixth; None if it does not parse or is < 0
tidTime(tid) = (the 64-bit integer value of the base32-sortable TID) >> 10     // data-model.md §3.1
```

### 2.2 Effective time `t_rank`

`t_rank` is defined for **items**: posts, comments, wikiRevisions and sequences.

```
t_obs(x)   = min{ obs[(x.uri, c)] : (x.uri, c) ∈ keys(obs) }, or None    // first-seen of the record (any version)
t_claim(x) = µs(x.createdAt)
t_tid(x)   = tidTime(x's own TID)        // the rkey TID; for a comment, the last TID segment
t_eff(x)   = min_some(t_claim(x), t_tid(x))                               // always present
future(x)  ⇔ t_obs(x) ≠ None ∧ t_eff(x) > t_obs(x) + 300_000_000           // flag: [future-dated]
t_eff'(x)  = future(x) ? t_obs(x) : t_eff(x)

t_decl     = let c = µs(decl.createdAt) or 0, o = t_obs(decl)
             in (o ≠ None ∧ c > o + 300_000_000) ? o : c

lower(x)   = x is a comment ? t_rank(treeParent(x)) : t_decl
t_rank(x)  = max(lower(x), t_eff'(x))
```

**`treeParent(c)`** is c's parent node after thread placement (§11):
- a comment;
- or the root post, for top-level comments, for children of a `Missing` placeholder, and for re-attached comments.

If the root post is not in `records`, the lower bound is `t_decl`. Posts are computed first, then comments, top-down through the placed tree.

### 2.3 Rules

- **Honest content gets the same `t_rank` everywhere.** This holds for any item whose claimed time is at most 5 minutes after its first-seen time.
- **Backdating only hurts its author.** The causal lower bound keeps a reply from ranking before its parent, and only ranking and display depend on time.
- **Moderation, karma, gates and wiki heads never depend on `t_rank`.** The only dependence on `now` outside ranking is `until` (§4.4).
- **Future-dated items are not hidden.** They rank at their first-seen time and carry the `[future-dated]` badge. If `t_obs` is unknown, which happens only in vectors, the claimed time stands.
- **Inconsistent timestamps (display-only flag).** When both times are present and `|t_claim − t_tid| > 3_600_000_000`, the item is flagged "inconsistent timestamps".
- **Displayed time.** An item's displayed time is its `t_rank`.
- **Account age** (used by P2 locks and review rules) comes from the PLC genesis operation. For did:web it is the first-seen time. **Reserved (P2).**

---

## 3. Participation, membership modes, seeds (L2, `mod/1`)

### 3.1 Definitions

```
mode       = effective decl.membership ∈ {approval, open}        // lexicon default: approval
A          = authorities (§4.1)
member(d)  ⇔ records holds a valid membership record in d's repo with rkey fk and forum = F
access(d)  = effective(d, access) (§4.4) ∈ {member, banned, none}
G          = { d : access(d) = member }                          // granted
B          = { d : access(d) = banned }                          // banned
N          = { d : effective(d, nullify) = true }                // nullified
P          = participants:
               approval:  { d : d ∈ A ∨ (member(d) ∧ access(d) ≠ none) }
               open:      { d : d ∈ A ∨ member(d) }
S          = (A ∪ G) ∖ (B ∪ N)                                   // seeds
E          = §6.2; S ⊆ E
newcomer(d) ⇔ d ∈ P ∖ (S ∪ E)                                    // meaningful in open mode only
```

**How bans interact with participation.** A ban never removes participation by itself. Rule V4 hides a banned user's content, so that an item-level keep (§4.6) can still override the ban in both modes. In approval mode, `access(d) ≠ none` means "granted, or banned".

### 3.2 Modes

| | `approval` (lexicon default) | `open` |
|---|---|---|
| Participant | an authority, or own membership record **and** granted (`#access member`) or banned | an authority, or own membership record |
| Content of a banned participant | `Hidden(Banned)` (V4) unless kept | same |
| Seeds S | `(A ∪ G) ∖ (B ∪ N)` | same |
| Eligible voters E | E = S, because P ∖ S ⊆ B ∪ N. `establish` and `establishSeeds` have no effect. | S plus established users (§6.2) |
| Newcomer content | none | collapsed by default, labelled "new user", per `display.newUsers` (V8) |
| Completeness | provable at CORE | S ∪ E is complete at CORE thanks to the closure (`sync.md`); newcomers are best effort (CORE+) |
| UI label | – | "relies on public relay infrastructure (Bluesky relay, Jetstream, Constellation)" |

`forum create` always writes the mode explicitly. With no flag it writes `approval`.

### 3.3 Rules

- **Consent.** Participation needs the user's own membership record.
  - A grant is a forum-scope `#access{level: member}` modAction, one per DID. It needs an explicit human decision.
  - There is no automatic sweep.
  - The batch-grant UI shows account age and activity.
- **Leaving.** A user leaves by deleting their membership record. V2 then hides their content in the default view, and the raw view still shows it.
  - If they were granted, the grant still counts. They remain a seed, and their opinions keep counting until they delete them, so other people's karma survives the departure.
  - An established user who is not a seed leaves E, because §6.2 requires participation, so their votes stop carrying weight.
- **Moderator badge.** A listed moderator is *shown* as a moderator only if `member(d)`. Their authority does not depend on it.
- **Applications (approval mode).** A DID with `member(d) ∧ access(d) = none` is an applicant.
  - Moderators find applications only through CORE+ (Constellation, Jetstream, the indexer) or out of band, and the UI says so.
  - Applicants appear in the mod queue (§4.12).
- **Changing the mode is retroactive.** Switching from `open` to `approval` hides the content of every non-authority participant who is not granted. An owner who archives a forum this way SHOULD first grant the participants whose content should stay visible. A forum-wide lock is **Reserved (P2)**.
- **Closure property** (completeness of S ∪ E). E depends only on seeds' votes (§6.2), and any newcomer who earns seed upvotes is referenced by seed records. So karma, scores and every outcome that depends on E are the same with or without CORE+ discovery. Only newcomer content differs.

---

## 4. Moderation (`mod/1`)

### 4.1 Authorities and ranks (L0)

```
owner   = did(F)                                                        rank 3
admins  = { m.did : m ∈ decl.moderators, m.role = admin }               rank 2
mods    = { m.did : m ∈ decl.moderators, m.role = moderator }           rank 1
A       = {owner} ∪ admins ∪ mods
rank(d) = 3, 2 or 1 for d ∈ A;  0 otherwise

subjectAuthor(T) = T        if T is a DID
                 = did(T)   if T is an at-uri
                 = none     if T is "tag:" slug
subjectRank(T)   = subjectAuthor(T) = none ? 0 : rank(subjectAuthor(T))      // tag: subjects rank 0
```

- `decl.moderators` entries are read per FO2: the owner's own entry and unknown roles are ignored, and the first entry for a DID wins.
- **Only the current declaration confers authority.** A DID removed from `moderators` loses its authority immediately (§4.10).
- **Post authors** act only in author scope, at rank 0 (§4.9).
- **Moderator hat.** `comment.moderatorHat` is shown only if the comment's author is in A (CO6).
- **Succession.** A forum is bound to its owner DID forever. The guide recommends a dedicated did:plc forum account whose rotation keys are held by at least two admins (`data-model.md` §6.1). `forum.successor` and `forum.aliases` are **Reserved (P2)**.

### 4.2 Assertions and dimensions

An **assertion** is a modAction m that meets all of these:
- it is valid (`data-model.md` §7, MA1–MA12);
- `m.forum = F`;
- its rkey has the form `^FK-H16$`;
- its scope and action are known (MA7).

Its **actor** is `did(m.uri)` and its subject is `m.subject`.

A record states the **complete desired state** of its action key. Each dimension of its action is asserted with the field's value, or with the field's default when the field is absent. The one exception is `tag.head`, which is asserted only when `head` is present. To override another actor's assertion, write your own record with the opposite value (`data-model.md` §6.7).

| Action | Dimension asserted | Value asserted | Order (safe value first) | Positive, so CID-bound |
|---|---|---|---|---|
| `#hide` | `hidden` | `hidden` (default `true`) | `true` < `false` | `hidden = false` (a "keep") |
| | `spam` | `hidden ∧ spam` | `true` < `false` | – |
| | attributes `reason` (record field), `showReason` (default `true`) | | | |
| `#access` | `access` | `level` | `banned` < `member` | – |
| `#restrict` | `restrict` | the known items of `capabilities` (a set; default ∅) | larger set is safer; equal rank → union | – |
| | `nullify` | `nullifyVotes` (default `false`) | `true` < `false` | – |
| `#tier` | `tier` | `tier` | `personal` < `frontpage` < `curated` | `tier ∈ {frontpage, curated}` |
| | attribute `notice` | | | |
| `#pin` | `pin[scope]` | `pinned` (default `true`) | `false` < `true` | `pinned = true` |
| `#lock` | `locked` | `locked` (default `true`) | `true` < `false` | – |
| | attribute `allowed` | | equal rank → intersection | |
| `#tag` | `tag.removed` | `removed` (default `false`) | `true` < `false` | – |
| | `tag.head` | `head`, only if present | unordered | bound by `head.cid` (§4.5) |

**Positivity** belongs to the whole record. A record is positive if any of its values is in the "Positive" column. A positive record without `subjectCid` is invalid (MA5).

### 4.3 Authorisation (forum scope)

An assertion m by actor X on subject T **counts at forum tier** only if every condition holds:
1. `scope_eff(m) = forum`.
2. X ∈ A.
3. **Rank:** `X = owner ∨ X = subjectAuthor(T) ∨ rank(X) > subjectRank(T)`.
4. **Role:** the table below allows the action, with its values, for X's role.
5. **Not lapsed:** `until` is absent, or `µs(until) > now`.
6. **CID binding:** if m is positive, the CID check of §4.5 passes.

| Action | owner | admin | moderator | post author (author scope, §4.9) |
|---|---|---|---|---|
| `#hide`: hide, keep, `spam` | ✓ | ✓ | ✓ | hide (`hidden = true`) of comments in their own thread; `spam` ignored; no keep |
| `#access`: `banned`, `member` | ✓ | ✓ | ✓ | `banned` only; applies only to their own threads |
| `#restrict`: capabilities, `nullifyVotes` | ✓ | ✓ | ✓ | – |
| `#tier`: `personal`, `frontpage` | ✓ | ✓ | ✓ | – |
| `#tier`: `curated` | ✓ | ✓ | – | – |
| `#pin` scope `forum` (post, sequence) | ✓ | ✓ | ✓ | – |
| `#pin` scope `thread` (comment) | ✓ | ✓ | ✓ | comments in their own thread |
| `#lock` on a post | ✓ | ✓ | ✓ | their own post |
| `#lock` on the forum uri (forum-wide lock) | **Reserved (P2)**: no effect for any actor (MA9) | | | |
| `#tag`: `removed`, `head` | ✓ | ✓ | ✓ | – |
| Edit `decl.moderators` | ✓ (the declaration is in the owner's repo) | – | – | – |

**What the rank rule implies**
- The owner can act on anyone.
- An admin can act on moderators and users; a moderator on users.
- Equal ranks act only on their own content or DID, through `X = subjectAuthor(T)`.
- Every authority can act on `tag:` subjects, which have rank 0.
- Banning or restricting a lower-ranked authority is authorised. It hides that authority's content (V4, V5) and removes them from S, but it never removes authority: only the declaration does that.

### 4.4 Resolution (L1)

```
assertions(T, d) = [ (rank(X), value_d(m), m) : m counts at forum tier (§4.3), m.subject = T, m asserts d ]

resolve(T, d) -> Default | Resolved{ value, rank, winner }:
  xs = assertions(T, d)
  if xs = []: return Default
  r  = max rank in xs
  vs = [ v : (r, v, _) ∈ xs ]
  v* = ordered d (hidden, spam, access, nullify, tier, pin[s], locked, tag.removed) → the safest value in vs
       set d     (restrict)                                                      → the union of vs
       unordered (tag.head) → if all vs are equal: vs[0]
                              else: return Default            // conflict: no override, plus a mod-queue item
  winner = the m with the smallest uri among the rank-r entries whose value equals v*
           (for the set dimension restrict: among all rank-r entries)
  return Resolved{ v*, r, winner }

effective(T, d) = resolve(T, d) is Resolved ? its value : default(d)
default: hidden false, spam false, access none, restrict ∅, nullify false, tier decl.defaultTier,
         pin[s] false, locked false, tag.removed false, tag.head none

allowed(p)  = ⋂ { m.allowed (entries per MA9) : m at rank r of resolve(p, locked) with m.locked = true }
              // defined when effective(p, locked) = true
keepRank(x) = r  if resolve(x.uri, hidden) = Resolved{false, r, _}
            = 0  otherwise                    // forum ranks are ≥ 1, so 0 means "no keep"
```

**Rules**
- **No time ordering.** Higher rank wins. At equal rank the safest value wins, so a restore cannot beat an equal-rank hide. A higher rank must step in, or the remover must retract: this is a two-person rule.
- **Attributes come from the winner.** `reason` and `showReason` come from `winner` of `hidden`; `notice` from `winner` of `tier`. For `restrict`, `winner` is the smallest-uri record at rank r.
- **Content-level decisions beat user-level ones.** An item-level keep at rank ≥ the rank of a ban, restriction or lock keeps the item visible (V4, V5, V7).
- **`hidden` alone never touches karma.** Only `spam`, bans, `nullify` and `restrict{vote, react, tag}` change which opinions count (§5).

### 4.5 CID-bound positive assertions (F20)

**When a positive assertion counts.** A positive record counts only if `T ∈ records` and `m.subjectCid = cid(records[T])`. A positive assertion on a subject that is not in `records` never counts.

**`#tag.head`** counts only if `records[head.uri]` exists and its CID equals `head.cid`.

**A stale positive assertion** (CID mismatch) is excluded from `resolve`. It then:
- produces a mod-queue item "edited since <action>" (§4.12);
- makes the tier fall back, the pin drop and the keep stop, until an authority re-asserts it on the new version.

Before an author edits an item that carries positive assertions, their client warns that the edit will suspend them.

**Negative assertions** take effect whatever their `subjectCid`. On DID and `tag:` subjects, `subjectCid` is ignored.

**Votes are not CID-bound.** Items show "N of M votes were cast on an earlier version" (§7).

### 4.6 Ban (F21)

A DID in B (forum tier, `access = banned`, resolved at rank `banRank(d)`):
- **Content:** every item they authored in F is `Hidden(Banned)` (V4), unless `keepRank(item) ≥ banRank(d)`.
  - The claimed time plays no part, so backdating cannot escape a ban.
- **Seeds and voting:** they are excluded from S and from E, and their opinions do not count (O4).

**Unbanning**
- The actor deletes their own record, using the modAction removal protocol, which leaves a `delete` revision (`data-model.md` §8.4).
- A higher rank can override by writing `#access{member}`. That also grants the user, so they become a seed.

`#access.keep` snapshots are **Reserved (P2)**.

### 4.7 Restrict (retroactive, rule 4b) and nullify

`effective(d, restrict)` is a set of capabilities, resolved at rank `restrictRank(d)`:

| Capability | Effect |
|---|---|
| `post` | the user's posts are `Hidden(Restricted)` (V5) |
| `comment` | the user's comments are `Hidden(Restricted)` (V5) |
| `sequence` | the user's sequences are `Hidden(Restricted)` (V5) |
| `wiki` | the user's wikiRevisions are `Hidden(Restricted)` (V5), so they are never wiki candidates (§13.2) |
| `vote` | the karma and agreement axes of the user's opinions are ignored (§5) |
| `react` | the reacts in the user's opinions are ignored (§5) |
| `tag` | the user's tag votes are ignored (§5), and their own `post.tags` contribute 0 relevance (§13.3) |

**Restriction rules**
- **Retroactive.** A restriction hides past items as well, exactly like a ban: time is self-asserted, so "from now on" cannot be defined. The mod UI MUST say that the restriction is retroactive.
- **Keeps override it.** An item with `keepRank(item) ≥ restrictRank(author)` is exempt from V5.
- **Lapse.** `until` lapses the restriction.
- **A record states the full set.** At a higher rank it replaces lower ranks' sets, so an empty set lifts them. At equal rank the sets are united.

**Nullify.** `effective(d, nullify) = true` puts d in N. None of d's opinions count (O4), and d is excluded from S and from E.

### 4.8 Lock

**Forum-tier lock.** A post p is locked when `effective(p, locked) = true`, resolved at rank `lockRank(p)`. A comment c in p's thread is `Hidden(Locked)` (V7) if and only if all of these hold:
- `c.uri ∉ allowed(p)`;
- `author(c) ∉ A`;
- `keepRank(c) < lockRank(p)`.

**Writing a lock** (`data-model.md` §8.6)
1. The moderator's client sets `allowed` to the uris of the thread's comments in state `Visible` or `Collapsed` in its own current view, sorted.
2. With **more than 5,000**, the lock is **refused**, and the client tells the moderator to use hide instead.

**Further rules**
- **Backdating-proof.** A comment written after the lock is not in `allowed`, whatever time it claims.
- **Allow-lists.** Equal-rank locks intersect their allow-lists. Entries outside p's thread are ignored (MA9).
- **Unlocking.** The actor deletes their own record, or a higher rank writes `#lock{locked: false}`.
- **Forum-wide locks** (a forum uri as subject) are **Reserved (P2)** and have no effect. An owner can archive a forum by switching it to approval mode (§3.3).
- **Composer.** Clients refuse to compose a comment on a locked post, unless the writer is in A or the comment would be exempt. This is UX, not security.

### 4.9 Author moderation (L5)

An author-scope modAction m with actor X **is effective** only if every condition holds:

```
authorEffective(m) ⇔ m is valid ∧ scope_eff(m) = author ∧ the action is allowed in author scope (MA6)
                   ∧ decl.policy.authorModeration = true
                   ∧ X ∈ E
                   ∧ (until absent ∨ µs(until) > now)
                   ∧ (m positive ⇒ the CID check of §4.5)
                   ∧ the condition in the table below

authorGate(X, p) ⇔ X ∈ A ∨ K[X] ≥ (effective(p.uri, tier) = personal ? gates.authorModPersonal
                                                                       : gates.authorModFrontpage)
root(c)          = records[c.root.uri]       // must exist for any author-tier effect on c
```

| Author-scope action | Subject | Condition | Effect |
|---|---|---|---|
| `#hide{hidden: true}` | comment c | `author(root(c)) = X`, `authorGate(X, root(c))`, `author(c) ∉ A` | c is author-hidden |
| `#access{level: banned}` | DID d with d ∉ A | – | every comment by d in the thread of any post p with `author(p) = X ∧ authorGate(X, p)` is author-banned |
| `#pin{scope: thread, pinned}` | comment c | `author(root(c)) = X`, `authorGate(X, root(c))` | author thread-pin value of c (positive when `pinned`) |
| `#lock{locked, allowed}` | post p | `author(p) = X`, `authorGate(X, p)` | author lock of p, with `allowed` |
| any other action, value or subject | | | no effect |

**Precedence.** Author-tier values act at rank 0. For any subject and dimension that has a forum-tier `Resolved` value, the forum tier governs and the author-tier value is ignored:
- **author-hidden** applies only if `resolve(c, hidden) = Default`;
- **an author lock** applies only if `resolve(p, locked) = Default`;
- **an author thread pin** applies only if `resolve(c, pin[thread]) = Default`;
- **an author ban** applies to c only if `keepRank(c) = 0`.

```
// p = root(c); X = author(p); "eff" = authorEffective (above, including the table's condition)
authorHidden(c)    ⇔ ∃ eff m by X: m = #hide{hidden: true}, m.subject = c.uri
authorBanned(c)    ⇔ ∃ eff m by X: m = #access{level: banned}, m.subject = author(c), authorGate(X, p)
authorLock(p)      ⇔ ∃ eff m by X: m = #lock{locked: true}, m.subject = p.uri ;  aAllowed(p) = m.allowed (MA9 entries)
authorLockedOut(c) ⇔ authorLock(p) ∧ resolve(p, locked) = Default ∧ c.uri ∉ aAllowed(p)
                     ∧ author(c) ∉ A ∧ author(c) ≠ X
AuthorModerated(c) ⇔ resolve(c, hidden) = Default                  // so any forum-tier hide or keep governs
                     ∧ (authorHidden(c) ∨ authorBanned(c) ∨ authorLockedOut(c))
```

Each (actor, scope, subject, action key) has exactly one record (`data-model.md` §6.7), so there is at most one author-tier record per subject and action, and author-tier values never conflict.

**Other rules**
- **Immunity.** Authorities are immune to author-tier hides, bans and locks.
- **No effect on L3.** Author bans and hides never affect opinions, karma or participation, because L3 cannot read L5.
- **Both scopes.** A moderator who also wrote the post can issue both forum-scope and author-scope assertions, because scope is part of the key.
- **Mod log.** Author-tier assertions appear in the mod log.
- **Reserved (P2):** moderation styles and guidelines (`profile.moderationGuidelines`), and reason-dependent rules.

### 4.10 Lapse, adoption, removing a moderator

**Lapse**
- Assertions whose actor is not in A have no forum-tier effect, starting immediately. This includes grants (`#access member`) made by former moderators.
- The records remain. The mod log and the raw view show them as "lapsed" ("by former mod").

**Adoption**
- `lessdugin mod adopt <did>` re-issues a former moderator's non-lapsed forum-scope assertions under the adopting authority, with `adopts` pointing at each original (`data-model.md` §8.6).
- `adopts` is informational. A copy counts only on its own merits: the adopter's rank and role, its own `until`, and its own `subjectCid`.

**Removing a moderator (`mods rm <did>`)**
1. The owner MUST first run `mod adopt <did>`.
2. Only if adoption completes may the owner edit the declaration to remove the entry. Reported skips are allowed.
- The CLI enforces this order.
- An opt-out (for example, for a rogue moderator) is allowed only after a confirmation that lists every assertion that will lapse.

**Role change.** A role change, such as admin to moderator, is re-evaluated at once. For example, a curation by a demoted admin stops counting.

### 4.11 Fail closed

```
degraded ⇔ ∃ a ∈ A : repoStatus[a] = Unavailable
```

**In a degraded view:**
- S is computed from the assertions that are available.
- Rule V2 also hides every item whose author is not in S, as `Hidden(ModerationUnavailable)`.
- A banner says "moderation data unavailable".

**What does not cause degradation.** A `Stale` authority repo does not degrade the view: its cached assertions apply. Moderation never vanishes because a PDS is down, so hidden spam is never resurrected.

### 4.12 Mod log, mod queue, raw view

**Mod log** (CORE, MVP). It lists:
- every valid modAction in `records` with `forum = F`, of any scope and by any actor;
- every valid revision whose subject is an `NS.modAction`, with `op = update` (edited) or `op = delete` (removed).

Revisions exist for modActions because their updates and removals always go through the revision protocol (`data-model.md` §8.3, §8.4).
- **Order:** `(time desc, uri asc)`, where time is `µs(updatedAt) ?? µs(createdAt)` for a modAction and `µs(createdAt)` for a revision.
- **Status annotation** on each entry, one of:
  - `effective`;
  - `overridden` (with the winning rank);
  - `lapsed` (the actor is no longer an authority);
  - `expired` (`until`);
  - `stale` (CID);
  - `unauthorised` (rank, role, gate or author condition);
  - `reserved/unknown` (no effect);
  - `edited`;
  - `removed`.

**Mod queue** (MVP; local presentation). Each item takes one key to act on, with a reason. It holds:
- stale positive assertions ("edited since curated, pinned or kept");
- unordered conflicts (`tag.head`);
- applicants (approval mode, §3.3);
- in open mode, newcomers' recent items (`Collapsed(NewUser)` or `Hidden(NewUser)`).

**Raw view.** It shows every record in `records`. Invalid records are annotated with their first failing rule ID. Each item is annotated with every assertion on it, including lapsed, expired, stale, unauthorised and reserved-key records.

---

## 5. Opinion validity (L3, `karma/1`)

An opinion v **counts** if and only if all of these hold:

```
O1  v is valid (data-model.md §7: G1–G10, OP1, OP2) and v.forum = F
                                                      // canonical rkey and subject; subject kind ∈ {post, comment, wikiRevision, sequence};
                                                      // v.forum = F also guards against fk collisions (data-model.md §3.3)
O2  s = records[v.subject.uri] exists, s is valid, and s.forum = v.forum
    — or v.subject.uri ∈ absent (deleted-subject rule, below)
O3  did(v.uri) ≠ did(v.subject.uri)                   // self-opinions are ignored entirely
O4  did(v.uri) ∉ B ∪ N
O5  effective(v.subject.uri, spam) = false
```

**Axes.** These rules act per axis or per entry. They never invalidate the record (OP3, OP4).

| Axis | Also requires |
|---|---|
| karma | a known value; the voter is not restricted from `vote` |
| agreement | a known value; not restricted from `vote`; `policy.voting ∈ {twoAxis, reacts}` |
| reacts | palette id and known stance (OP3); not restricted from `react`; `policy.voting = reacts` |
| tags | the subject is a post; entries per OP4; not restricted from `tag` |

**Deleted-subject rule (F24).** An opinion whose subject is in `absent` counts **only** toward karma (§6.2), and only if:
- the subject's kind is post or comment, read from `coll(uri)`;
- in that stage, at least **2 distinct voters** of the stage's eligible set have counting karma opinions on that uri.

It never counts toward scores, reacts or tags. O3 and O5 apply to the uri.

**Other rules**
- **Weight.** An opinion has weight only if its voter is in E (§6, §7). Counting opinions of other voters are shown as "not yet counted".
- **Hidden subjects** still collect counting opinions. Only `spam` voids them.
- **Author-tier bans** do not affect opinions.
- **`subject.cid`** never affects counting. It drives the "earlier version" display (§7).
- **Never counted:** opinions on subjects that are neither in `records` (valid and in F) nor in `absent`. That covers subjects not yet synced, invalid ones, and ones in another forum.

---

## 6. Vote power and `karma/1`

### 6.1 Power (a port of ForumMagnum's `calculateVotePower`)

```
small(k)   = k ≥ 1000 ? 2 : 1
strong(k)  = the power p of the first (t, p) in LADDER with k ≥ t; else 1
LADDER     = [(500000,16), (250000,15), (175000,14), (100000,13), (75000,12), (50000,11), (25000,10),
              (10000,9), (5000,8), (2500,7), (1000,6), (500,5), (250,4), (100,3), (10,2)]
power(k, x) = let k' = max(k, 0) in
              x ∈ {up, agree}                   →  small(k')
              x ∈ {down, disagree}              → −small(k')
              x ∈ {strongUp, strongAgree}       →  strong(k')
              x ∈ {strongDown, strongDisagree}  → −strong(k')
sign(x)     = +1 for up, strongUp;   −1 for down, strongDown
```

Displayed vote power of user u: normal `small(max(KP[u], 0))`, strong `strong(max(KP[u], 0))`.

### 6.2 Stages

```
V            = { v : v counts (§5) ∧ the karma axis is present and enabled
                     ∧ coll(v.subject.uri) ∈ {post, comment} }       // includes absent-subject opinions
voter(v)     = did(v.uri);   recipient(v) = did(v.subject.uri)

included(v, Elig) ⇔ voter(v) ∈ Elig
                    ∧ ( v.subject.uri ∉ absent
                      ∨ |{ voter(w) : w ∈ V, w.subject.uri = v.subject.uri, voter(w) ∈ Elig }| ≥ 2 )

pairsum(basis: Option<Map<did, i64>>, Elig: Set<did>) -> Map<did, i64>:
  acc = BTreeMap<(did, did), i64>                          // (voter, recipient)
  for v in V with included(v, Elig):
     w = basis = None ? sign(v.karma) : power(basis[voter(v)], v.karma)
     acc[(voter(v), recipient(v))] += w
  out = BTreeMap<did, i64>
  for ((u, r), x) in acc:
     cap = basis = None ? pairCap : pairCap × strong(max(basis[u], 0))
     out[r] += clamp(x, −cap, cap)
  return out                                                // a missing recipient has 0

K1        = pairsum(None, S)                                // stage 1: vote counts from seeds
seedUp[u] = |{ s ∈ S : Σ_{v ∈ V, included(v, S), voter(v) = s, recipient(v) = u} sign(v.karma) > 0 }|
E         = S ∪ { u ∈ P ∖ S : u ∉ B ∪ N ∧ K1[u] ≥ establish ∧ seedUp[u] ≥ establishSeeds }
KP        = pairsum(Some(K1), E)                            // power basis
K         = pairsum(Some(KP), E)                            // displayed karma; gates use K
```

**Parameters.** The defaults are `pairCap = 25`, `establish = 1` and `establishSeeds = 2`. In approval mode E = S (§3.2).

**What K covers**
- Karma is per forum. There is no cross-forum karma.
- Votes on wikiRevisions and sequences earn no karma. Karma from votes on tag (wiki) revisions is deferred to P2 (`karma/2`, §15).

### 6.3 Properties (each property-tested)

| # | Property |
|---|---|
| P1 | **Permutation invariance.** Every output is independent of the order of records. |
| P2 | **Idempotence.** Supplying the same `(uri, cid)` twice changes nothing. |
| P3 | **Acyclic and linear.** Each stage reads only earlier stages and makes one pass over V. |
| P4 | **Layering.** Adding or removing records that are only L5–L7 inputs never changes K1, `seedUp`, E, KP or K. Those inputs are: author-scope modActions; the `reacts` and `tags` entries of opinions; wikiRevisions; sequences; and opinions whose subject is a wikiRevision or sequence. |
| P5 | **Stage pair bound.** In each stage, voter u's net contribution to recipient r is at most `pairCap` (K1), or `pairCap × strong(max(basis[u], 0))` (KP, K), in absolute value. Corollary: if u ∉ S and none of u's opinions has a subject in `absent`, removing u's opinions changes no K1, `seedUp` or E, and changes `KP[r]` by at most `pairCap × strong(max(K1[u], 0))` for every r. |
| P6 | **Seed persistence.** s ∈ S ⇒ s ∈ E, whatever K1[s] is (even negative). |
| P7 | **Sybil bound** (restricted to K, scores and other users' visibility; see below). |
| P8 | **Incremental equals batch.** |
| P9 | **Native equals wasm**, on random inputs. |

**P7, sybil bound.** Let I be a fold input, and R a set of records with uris not in I, each authored by a DID ∉ E(I) ∪ A, such that:
- (a) no record in R is the subject (by uri) of an opinion in I whose voter is in E(I);
- (b) R contains no membership record of a DID u with `K1(I)[u] ≥ establish ∧ seedUp(I)[u] ≥ establishSeeds`.

Then for I' = I ∪ R, all of these are unchanged:
- K1, `seedUp`, E, KP and K;
- the score and agreement of every item of I;
- the V1–V10 visibility state (before the thread pass) of every item of I whose author did not author a record in R.

**Outputs outside P7**, which outsiders can change:
- `unweighted` counts;
- the visibility of their own items (newcomer collapse);
- thread placement (cycles), tombstones, and the `recentComments` and Recent Discussion orders;
- notifications;
- wiki candidates.

**Consequences.** Seeded trust has a two-hop radius. E depends only on seeds' votes, so sybils created by an established user never become eligible voters unless at least `establishSeeds` distinct seeds upvote them.

### 6.4 Other rules

- **Explicit policy.** `forum create` writes the whole policy explicitly (`data-model.md` §6.1).
- **Lenses.** A locally selected alternative algorithm is a lens ("my view"). It is never used for gates, visibility or anything shared, and never shown as the forum's karma.
- **Retroactive.** Changes to S, grants, bans, `nullify` or `restrict` recompute every stage.
- **Votes not yet counted.** A voter outside E sees "recorded; will count once you're established (applied retroactively)".
- **Co-author credit** is `karma/2`, **Reserved (P2)**.

---

## 7. Scores and agreement (L3.5, `karma/1`)

For every valid item i in F (post, comment, wikiRevision, sequence), with a = author(i):

```
self(i)       = a ∈ E ? (kind(i) = post ? strong(max(KP[a], 0)) : small(max(KP[a], 0))) : 0
W(i)          = { v : v counts (§5), v.subject.uri = i.uri, voter(v) ∈ E }
score(i)      = self(i) + Σ_{v ∈ W(i), karma axis enabled} power(KP[voter(v)], v.karma)            // no pair cap
agreement(i)  = policy.voting = karmaOnly ? none
              : Σ_{v ∈ W(i), agreement axis enabled} power(KP[voter(v)], v.agreement)
unweighted(i) = |{ v : v counts, v.subject.uri = i.uri, karma axis enabled, voter(v) ∉ E }|
earlier(i)    = |{ v ∈ W(i) : karma axis enabled, v.subject.cid ≠ cid(i) }|
```

**Display**
- "n votes not yet counted" shows `unweighted(i)`.
- "N of M votes were cast on an earlier version" shows `N = earlier(i)` and `M = |{ v ∈ W(i) : karma axis enabled }|`.

| `policy.voting` | karma axis | agreement axis | reacts |
|---|---|---|---|
| `karmaOnly` | ✓ | ignored | ignored |
| `twoAxis` | ✓ | ✓ | ignored |
| `reacts` (default) | ✓ | ✓ | ✓ |

**Rules**
- **Stored but ignored.** Axes that are not enabled are still stored and preserved by writers.
- **Karma only.** Agreement never feeds karma, and neither do reacts.
- **Implicit self-vote.** Authors never vote on their own items (O3). `self(i)` stands in for ForumMagnum's automatic self-vote.
- **Hidden items have scores too.** They are shown in the raw view and to moderators.
- **Consistency (informative).** When no pair cap binds and no absent-subject opinions exist, `K[a] = Σ over a's posts and comments of (score(i) − self(i))`.

---

## 8. Gates (L4, `karma/1`)

```
passes(u, g) ⇔ u ∈ A ∨ K[u] ≥ gateValue(g)
gateValue(g) = decl.policy.gates.g if present, else the karma/1 default below
```

| Gate | Used by | `karma/1` default (field absent) | `forum create` open preset | `forum create` approval preset |
|---|---|---|---|---|
| weighted vote | §6, §7 | voter ∈ E (not configurable) | same | same |
| `reactCreate` | a react is valid (§9.2) | 10 | 10 | 0 |
| `reactSecond` | supporter (§9.2) | 5 | 5 | 0 |
| `reactAnti` | opposer (§9.2) | 20 | 20 | 5 |
| `authorModPersonal` | author moderation on a `personal` post (§4.9) | 50 | 50 | 10 |
| `authorModFrontpage` | author moderation on a `frontpage` or `curated` post | 2000 | 2000 | 100 |
| `createTag` | genesis wiki revision (§13.2) | 10 | 10 | 0 |
| `wikiEdit` | non-genesis wiki revision | 5 | 5 | 0 |
| `sequence` | sequence library listing (§14), unless forum-pinned | 1000 | 1000 | 0 |

**Rules**
- **Tag creation.** `tagCreation = moderators` restricts genesis revisions to authorities, and `createTag` is then ignored.
- **Authorities bypass every gate.**
- **Gates use the current K, so they are retroactive.** A react can disappear when its supporters lose karma, which is an intended sybil property.
- **Who enforces gates.** Readers enforce them in the fold. Writers check them at compose time as UX only.
- **Not gates in v1 (§15):** removing `nofollow` at 10 karma (Never, `docs/plan.md` §6.5), and side-comment margin display (P2).

---

## 9. Reacts (`default/1` palette; aggregation under `karma/1`)

### 9.1 Palette

The `default/1` palette is the 34 ids below, in this order:

`agree`, `disagree`, `important`, `insightful`, `changed-mind`, `crux`, `not-a-crux`, `locally-valid`, `locally-invalid`, `typo`, `wrong`, `misleading`, `unclear`, `dont-understand`, `examples`, `citation-needed`, `concrete`, `overconfident`, `underconfident`, `hits-the-mark`, `missed-the-point`, `strawman`, `steelman`, `scout-mindset`, `soldier-mindset`, `too-harsh`, `good-point`, `thanks`, `seen`, `will-reply-later`, `laugh`, `heart`, `surprise`, `empathy`.

**`docs/spec/reacts-default-1.json`** is the machine-readable form: `{spdx, id, description, groups[{id, label}], reacts[{id, label, description, glyph, group}]}`.
- **Normative for the fold:** only the set of `id` values.
- **Presentation defaults:** `label`, `description`, `glyph` (a single emoji code point with default emoji presentation, or short ASCII), `group`, and the order of `groups` and `reacts`. Clients SHOULD use them and MAY localise them.

**Unknown values**
- A react id outside the palette is ignored (OP3).
- An unknown `policy.reactPalette` falls back to `default/1`, with the banner.

**Reserved (P2):** custom palettes and custom react icons.

### 9.2 Aggregation (L5)

```
// Within one counting opinion v on subject s, with the reacts axis enabled (§5):
entries(v) = [ e ∈ v.reacts : e.react ∈ palette ∧ e.stance ∈ {for, against} ]    // stance default: for
qkey(e)    = e.quote ? qkey(e.quote) (lfm-1.md §11: NFC, White_Space runs → U+0020, trimmed) : ""
group the entries by (e.react, qkey(e)):
   a group with both stances is dropped (contradiction); otherwise it yields one entry (react, q, stance)

// Per subject s and key (react, q):
Fv  = { voter(v) : v yields (react, q, for),     voter(v) ∈ E }
Av  = { voter(v) : v yields (react, q, against), voter(v) ∈ E }
C   = { u ∈ Fv : passes(u, reactCreate) }
valid ⇔ C ≠ ∅
if valid:
   creator    = the u ∈ C with the greatest K[u]; ties → the smallest DID
   supporters = { u ∈ Fv : passes(u, reactSecond) } ∪ {creator}
   opposers   = { u ∈ Av : passes(u, reactAnti) }
```

**Display.** A valid react shows its creator, with everyone else in `supporters` shown as having "seconded" it, and `|opposers|` against.

**Anchors are display-only.** Each valid `(react, q ≠ "")` is anchored by resolving the **creator's** `quote` against P of the subject's current version (`lfm-1.md` §11).
- `Anchored{start, end}` highlights the range.
- `Orphaned` shows "quoted text no longer present".
- `Pending` (a spilled body not yet fetched) shows as loading.
- `q = ""` applies to the whole item.

**Rules**
- **Grouping is deterministic.** Aggregation groups by `qkey`, never by resolved range, so the fold never depends on whether a body blob has been fetched.
- **Self-reacts.** Reacts on one's own items are ignored (O3), and reacts by voters outside E are never counted.
- **No side effects.** Reacts never affect karma, scores or visibility.
- **Display in the MVP.** Anchored reacts are listed under the item. The inline margin UI is **Reserved (P2)**.
- **Indexer hint.** The indexer's `#reactTally{react, for, against, quote}` is a hint carrying `(react, |supporters|, |opposers|, the creator's quote)`.

---

## 10. Visibility (L6) and lens (L7) (`mod/1`)

### 10.1 Rules

Every candidate item x (a record in `records` whose collection is post, comment, sequence or wikiRevision) gets exactly one state. The first matching rule wins.

| # | Condition | State |
|---|---|---|
| V1 | x is invalid (`data-model.md` §7), `x.forum ≠ F`, or x fails a cross-record rule (CO5) | `Excluded` |
| V2 | `author(x) ∉ P` | `Hidden(NotParticipant)` |
| | `degraded ∧ author(x) ∉ S` (§4.11) | `Hidden(ModerationUnavailable)` |
| V3 | `effective(x.uri, hidden) = true` | `Hidden(Removed{rank, actor, spam, reason if showReason})`, from the winner |
| V4 | `author(x) ∈ B ∧ keepRank(x) < banRank(author(x))` | `Hidden(Banned)` |
| V5 (rule 4b) | `cap(x) ∈ effective(author(x), restrict) ∧ keepRank(x) < restrictRank(author(x))` | `Hidden(Restricted)` |
| V6 | x is a comment ∧ `AuthorModerated(x)` (§4.9) | `Hidden(AuthorModerated)` |
| V7 | x is a comment, with `p = x.root.uri`, ∧ `effective(p, locked) = true ∧ x.uri ∉ allowed(p) ∧ author(x) ∉ A ∧ keepRank(x) < lockRank(p)` | `Hidden(Locked)` |
| V8 | `mode = open ∧ author(x) ∉ S ∪ E` and `display.newUsers = collapse` | `Collapsed(NewUser)` |
| | the same with `display.newUsers = hide` (with `show`, V8 does not apply) | `Hidden(NewUser)` |
| V9 | `score(x) ≤ display.collapseBelow` (default −5) | `Collapsed(LowScore)` |
| V10 | otherwise | `Visible` |

`cap(x)` maps the item's collection to a capability: `post → post`, `comment → comment`, `sequence → sequence`, `wikiRevision → wiki`.

**Listed and visible**
- An item is **listed** if its state is `Visible` or `Collapsed(*)`. Collapsed items render collapsed, with their reason.
- Lists, feeds, search results and notifications never include `Excluded` or `Hidden(*)` items.

### 10.2 Thread pass

A `Hidden(reason)` post or comment that has at least one listed descendant in the placed tree (§11) becomes `Tombstone(reason)`, so the thread keeps its shape. A tombstone shows its reason; for `Removed`, the reason text appears only if `showReason`. A hidden node with no listed descendant is not displayed. A `Missing` placeholder is displayed only if it has a listed descendant.

### 10.3 Lens (L7)

The lens is applied after the fold, from local state only. It never changes any fold output: scores, karma, gates and the states of other viewers.
- **Mutes.** A listed item by a muted author becomes `Collapsed(Muted)`.
- **Blocks.** Optionally, the viewer's own `app.bsky.graph.block` records, read from their PDS, are treated as mutes. They are never written.
- **Raw view.** Shows everything, with annotations (§4.12).
- **"My view".** A local alternative algorithm (§6.4).
- **Display limit.** Clients show at most 50 comments per author per thread expanded, in `(t_rank, uri)` order. The rest collapse behind "show N more by @x" (`limits.md`). This is presentation only.

---

## 11. Threads (`mod/1`)

Thread placement for a post uri p:

1. **Nodes.** The thread's nodes are the root p (or a `Missing(p)` root when `p ∉ records`), plus every comment c that is not `Excluded` (V1) with `c.root.uri = p`.
2. **Parent pointers**, for each comment c:
   - `parent` absent → the root;
   - `parent.uri` is a node of this thread → that comment;
   - otherwise (missing, invalid, excluded, or under another root) → a `Missing(parent.uri)` placeholder. There is one placeholder per distinct uri, each a child of the root.
3. **Cycles.** These are possible because `parent` can be edited.
   - Every comment has exactly one parent pointer, so each connected component contains at most one cycle. A comment that names itself as its parent is a cycle of length 1.
   - For each cycle, let m be its member with the greatest uri. Cut m's parent edge, re-attach m to the root, and flag it `Orphaned`.
   - The result does not depend on the order in which chains are walked.
4. **Depth.** The root's children have depth 1. A comment whose depth would exceed **1,000** is re-attached to the root and flagged `Orphaned`. Nodes are processed top-down, so its subtree continues from depth 1.
5. **Time.** `t_rank` is computed top-down after steps 3–4 (§2.2).
6. **Versions.** A comment whose `root.cid` or `parent.cid` differs from the target's current CID shows "on revision n" or "on an unavailable version".
   - Let the target's valid `op: update` revisions, in TID order, be R1 … Rk. Version i (1 ≤ i ≤ k) is the snapshot with CID `Ri.subject.cid`, and version k + 1 is the current one.
   - n is the version whose CID equals the referenced CID. If there is none, the version is unavailable.
   - Placement never depends on CIDs.

**Display**
- **Indentation** is capped at about 6 levels in the TUI and on mobile, then "continue thread →".
- **Missing roots.** A thread opened for a post that is not in `records` shows its comments under the `Missing(p)` root. Those comments do not appear in lists or in Recent Discussion.
- **Edited.** An item with valid `op: update` revisions, or with `updatedAt` later than its `createdAt`, gets the `[edited]` badge.
- **Retracted comments** (`retracted: true`) render struck through and still count.

---

## 12. Ranking (`magic/1`), sorts, filters

### 12.1 `hot`

```
hot(b: i64, t: µs) -> f64:
  age  = max(now − t, 0) as f64                       // exact: < 2^53
  h    = age / 3_600_000_000.0                        // IEEE-754 binary64, round to nearest even
  base = h + (offsetHours as f64)                     // offsetHours ≥ 1, so base ≥ 1
  e    = (exponentMilli as f64) / 1000.0
  return (b as f64) / libm::pow(base, e)
```

The operations are performed exactly in this order. The vectors assert the result's bit pattern (`testdata/vectors/rank`).

### 12.2 Post sorts

**Bonus-adjusted score.** For a listed post p, `b(p) = score(p) + (tier = curated ? curatedBonus : 0) + (tier ≥ frontpage ? frontpageBonus : 0)`, where `tier = effective(p.uri, tier)`.

**Pins.** `pinned(p) ⇔ effective(p.uri, pin[forum]) = true`.

| Sort | Order (every key ends with `uri asc`) | Includes |
|---|---|---|
| `magic` (default) | `pinned desc`, then `b > 0 desc`, then `b > 0 ? hot(b, t_rank) desc (total_cmp) : (b desc, t_rank desc)` | posts with `now − t_rank ≤ cutoffDays × 86_400_000_000`, plus every forum-pinned post regardless of age |
| `top` | `score desc`, `t_rank desc` | all |
| `new` | `t_rank desc` | all |
| `old` | `t_rank asc` | all |
| `recentComments` | `activity desc`, where `activity(p) = max({t_rank(p)} ∪ {t_rank(c) : c in p's thread, state Visible})` | all |

`cutoffDays` (default 90) affects **only** the `magic` sort.

### 12.3 List filters

| Filter | Shows listed posts with |
|---|---|
| `frontpage` (the default) | `tier ≥ frontpage` and kind ≠ `quickTake` |
| `all` | any tier and any kind; `quickTake` posts carry a badge |
| `curated` | `tier = curated` and kind ≠ `quickTake` |

**Additional filters**, which combine with the above:
- `tag t`: `applies(p, t)`, from §13.3;
- `kind k`: the effective kind;
- `author d`.

`featuredTags` from the declaration are offered as tag filters.

### 12.4 Comment sorts

Sorting applies among the siblings at every level of the placed tree.

**Pins.** Thread pins come first in every comment sort. The pin is `effective(c, pin[thread])`, or the author-tier pin (§4.9) when the forum tier is `Default`.

| Sort | Order after the pins (every key ends with `uri asc`) |
|---|---|
| `magic` (default) | `b > 0 desc`, then `b > 0 ? hot(b, t_rank) desc : (b desc, t_rank desc)`, with `b = score(c)`; no bonuses and no cutoff |
| `top` | `score desc`, `t_rank desc` |
| `new` | `t_rank desc` |
| `old` | `t_rank asc` |
| `latestReply` | `latest desc`, where `latest(c) = max({t_rank(c)} ∪ {t_rank(d) : d a descendant of c in state Visible})` |

`Missing` placeholders sort after all comment siblings, by uri ascending.

### 12.5 Recent Discussion (MVP)

**Which posts.** Listed posts in any tier and of any kind that have at least one `Visible` comment.

**Order.** `(t_rank of the newest Visible comment desc, uri asc)`.

**Content.** Each post shows its 3 newest `Visible` comments, ordered by `(t_rank desc, uri asc)`.

### 12.6 Deferred

The following are **Reserved (P2)**: `topAdjusted`, personalised magic (`magic/2`), and tag-filter weights.

---

## 13. Tags and wiki (`karma/1`)

### 13.1 Identity

- A tag is `(F, slug)`.
- Its page is the set of `wikiRevision` records with that `forum` and `slug`.
- The `tag:` + slug modAction subject carries the forum-tier tag state: `tag.removed` and `tag.head`.

### 13.2 Head selection (MVP: linear history)

**Linear history.** MVP writers write `parents` as either empty or exactly the head they edited (`data-model.md` §6.8). Readers accept up to 8 parents. Ancestry is by uri, independent of the ancestors' integrity (H3).

```
page   = { r ∈ records : r is a valid wikiRevision, r.forum = F, r.slug = slug, r not Hidden by V1–V5 }
         if |page| > 1000: keep the 1000 with the greatest (TID of rkey(r), then smallest uri); flag the page [partial]
par(r) = { q.uri : q ∈ r.parents } ∩ uris(page)       // a parent of another (forum, slug), or one that is
                                                       // missing, invalid or hidden, gives no edge
genesisOK(r) ⇔ tagCreation = moderators ? author(r) ∈ A : passes(author(r), createTag)
editOK(r)    ⇔ passes(author(r), wikiEdit)

Cand  = the least set such that:  r ∈ page ∧ ( genesisOK(r) ∨ (editOK(r) ∧ par(r) ∩ Cand ≠ ∅) )  ⇒  r ∈ Cand
Edges = { (r, q) : r ∈ Cand, q ∈ par(r) ∩ Cand }
cycle cut: while (Cand, Edges) has a cycle:
             for each strongly connected component C with |C| > 1, or with a self-loop:
               remove every edge (m, q) ∈ Edges with q ∈ C, where m is the greatest-uri member of C
depth(r)  = 0 if r has no outgoing edge; else 1 + max{ depth(q) : (r, q) ∈ Edges }

headRef   = effective("tag:" + slug, tag.head)
canonical = headRef ≠ none ∧ headRef.uri ∈ Cand ∧ headRef.cid = cid(records[headRef.uri])
              ? records[headRef.uri]
              : argmax over Cand of (depth desc, K[author] desc, uri asc)       // none if Cand = ∅
exists(slug)     ⇔ canonical ≠ none ∧ effective("tag:" + slug, tag.removed) = false
applicable(slug) ⇔ exists(slug) ∧ canonical.isTag (default true)
```

**Rules**
- **The head is the latest revision.** In a linear history, the canonical revision is the tip of the longest chain of eligible revisions, unless a moderator's `#tag.head` names a candidate.
- **A deleted parent gives a flagged root.** A revision r with a non-empty `parents` and `par(r) ∩ Cand = ∅` is flagged `Root(ParentUnavailable)`. Its parents are missing, invalid, hidden, of another page or not candidates. It enters `Cand` only through `genesisOK`, so a fake or deleted parent cannot bypass `createTag` or `tagCreation = moderators`.
- **Ancestor altered.** An edge (r, q) whose stored `parents` entry has a CID different from `cid(q)` is shown with the note "ancestor altered".
- **History view.** The history shows the chain from `canonical`, following at each step the edge to the parent with the greatest depth, ties by the smallest uri. Clients SHOULD show the number of other candidates with no incoming edge as "N unmerged edits".
- **Reserved (P2):** fork and merge UI, merge revisions, `#tag.aliasOf`, and tag discussion threads anchored to `(forum, slug)`.
- **No karma.** Opinions on wikiRevisions give them a score (§7), but earn no karma (§15).

### 13.3 Relevance and application

```
rel(p, slug) = own + votes
own   = slug ∈ p.tags (set, PO7) ∧ author(p) not restricted from tag
          ? max(1, small(max(KP[author(p)], 0)))
          : 0
votes = Σ over v counting on p, voter(v) ∈ E, tags axis enabled, with an entry for slug:
          power(KP[voter(v)], entry.value)
applies(p, slug) ⇔ applicable(slug) ∧ rel(p, slug) > 0
```

**Rules**
- There are no separate tag-relation records. Applying a tag means writing a `post.tags` entry (the author) or a `#tagVote` in one's opinion (anyone else).
- Voters cannot vote on tags of their own posts (O3).
- Unknown tags (`exists = false`) never apply.
- **Reserved (P2):** authorOnly tags.

---

## 14. Sequences (`karma/1`)

A sequence is an ordered list of chapters, each listing post uris. The posts may be by other authors.

**Library**

```
eligible(s) ⇔ s is a valid sequence in F ∧ s not Hidden by V1–V5
listed(s)   ⇔ eligible(s) ∧ ( passes(author(s), sequence) ∨ effective(s.uri, pin[forum]) = true )
order       = (forum-pinned desc, score(s) desc, t_rank(s) desc, uri asc)
```

Sequence scores come from opinions on the sequence. They earn no karma.

**Entries**
- `entries(s)` is the concatenation of `chapters[i].posts` in order.
- An entry u is **displayable** if `fkOf(rkey(u)) = fk`, `records[u]` exists, and u is listed (§10.1).
- An entry with the forum's fk that is not displayable shows as "unavailable post". An entry with another fk is not shown (SQ2).

**Next and previous** for post p:
1. The **context** is chosen in this order:
   - the sequence the reader came from, if it contains p;
   - else the eligible sequence by `author(p)` that contains p, with the greatest `(score, then smallest uri)`;
   - else the listed sequence that contains p, with the greatest `(score, then smallest uri)`;
   - else none.
2. Next and previous are the adjacent displayable entries around the **first** occurrence of p in `entries(context)`.

"Continue reading" is local state (§17).

---

## 15. Other content types

| Feature | v1 status |
|---|---|
| Article (`kind=article`, the default) | MVP. `title` is required (PO3). |
| Questions (`kind=question`) | MVP: `title` required, plus a question badge. Answers and accepted answers (`comment.answer`) are **Reserved (P2)**. |
| Quick takes (`kind=quickTake`) | MVP: untitled; `body` required, and any `title` is ignored (PO4). Listed only under the All filter, with a badge (§12.3). The dedicated feed is **Reserved (P2)**. |
| Link posts (`kind=link`) | MVP: `title` and `link` required (PO3, PO5). Cards are entered by hand or fetched by the CLI. The SPA never fetches cards, because it has no CORS proxy. |
| Side comments | `comment.quote` is MVP data, with quotes listed in the MVP. The margin UI is **Reserved (P2)**. |
| Co-authors | **Reserved (P2)**: `post.coauthors`, a CID-bound `coauthor` acceptance, and `karma/2` credit. |
| Dialogues | **Reserved (P8+)**: `turn` records. The UI must warn that drafts are public. |
| Events, polls | **Reserved (P8+)**: community calendar records (same rkey) and `pollVote`. |
| Recommendations | Deferred to P8+ (`docs/plan.md` §6.5); no reserved name. |
| Shareable drafts and comments on drafts | Deferred to P8+; drafts are local. |
| authorOnly tags | Deferred to P2 (with `karma/2`). |
| Removing `nofollow` at 10 karma | Never (`docs/plan.md` §6.5). External links always carry `nofollow` (`lfm-1.md` §8.2). |
| `lockedToAccountsCreatedAfter`, `repliesBlockedUntil` | Deferred to P2 (with `ratelimit/1`). They need account age (§2.3). |
| Karma from votes on tag (wiki) revisions | Deferred to P2. It needs `karma/2`. |
| Books (`collection`) | **Reserved (P8+)**. |

---

## 16. Notifications, subscriptions, read state (local)

Everything in this section is computed locally from the fold and local state. That happens at sync time (CORE), live through Jetstream (CORE+), and offline or across forums through the indexer's `getActivity` (EXT, later), whose results are hints followed by point fetches. Push and email are **Reserved (P8+)**.

**Newness.** An item x is new for the viewer if and only if `t_obs(x) > watermark(viewer)`. `createdAt` is never used, so backdating cannot hide an item.

**Eligible items.** Only items that are:
- listed (§10.1);
- not authored by the viewer;
- authored by someone in S ∪ E (the default filter; configurable to "all participants");
- in a followed forum.

**Reasons.** When an item matches several reasons, it gives one notification with the first matching reason in this table:

| Reason | Trigger |
|---|---|
| Reply | x is a comment whose parent comment is by the viewer, or a top-level comment on the viewer's post |
| Mention | the viewer's DID ∈ `Rendered.mentions` of x's rendered body (`lfm-1.md` §12) |
| Quote | x is a comment with `quote` whose target (the parent if present, else the root) is by the viewer |
| Pingback | x is a post whose `Rendered.links` contains the uri of a post, comment or sequence by the viewer |
| NewComment | x is a comment whose root is in the viewer's subscribed posts |
| NewPost | x is a post by a subscribed user, or in a subscribed forum, or with `applies(x, t)` for a subscribed tag t; or a post newly added to the entries of a subscribed sequence (diff against the last snapshot) |
| Reacted | aggregated per item of the viewer's: the change in `(react, \|supporters\|)` since the last snapshot |
| ModAction | a change since the last snapshot in resolved state affecting the viewer: forum-tier `hidden`, `spam`, `tier`, `pin` or `lock` on their items; author-tier effects on their comments; their `access`, `restrict` or `nullify` |
| KarmaChange | the change in `K[viewer]` since the last snapshot, delivered per the setting: realtime, daily (the default) or weekly |

**Pingbacks**
- Pingbacks are computed from the **rendered** bodies of synced records, never from `markdown.links`, which is a hint for external backlink indexes.
- An item's page lists the listed posts that link to it ("linked from").
- Indexer `listBacklinks` results are hints.

**Read state**
- `lastVisited[post]` is local.
- A comment c is unread if `t_obs(c) > lastVisited[root]`.
- Viewing a thread sets `lastVisited` to the greatest `t_obs` among the comments displayed.

**Subscriptions, follows, bookmarks**
- Subscriptions to a forum, post, user, tag or sequence are local.
- Followed forums are a local list. Clients SHOULD seed it from the user's own membership records: list `NS.membership` in their repo and read each `forum` field (`data-model.md` §6.2). This is how follows carry across devices.
- Bookmarks are local.

---

## 17. Public versus local

| Data | Where |
|---|---|
| Posts, comments, revisions, wiki revisions, sequences, profile | public repo |
| **Opinions (votes, agreement, reacts, tag votes), memberships with their notes, mod actions with their reasons** | **public repo**. The UI shows an interstitial on first use of each action type: vote or agree, react, tag vote, join or apply, mod action. A "what's public about me" view is **Reserved (P2)**. |
| Drafts, read marks, subscriptions, followed forums, bookmarks, mutes, notification state and snapshots, tag filters, karma history, "continue reading" | local `state` store, with JSON export and import |
| Public bookmarks and subscriptions, cross-device sync of local state, private ballots (a tally extension) | **Reserved (P2+)** |

---

## 18. Rate limits and reports

**Rate limits.** Automatic and manual rate limits are deferred.
- They are **Reserved (P2)** as `ratelimit/1`.
- They will be best-effort and documented as advisory, because time is self-asserted.
- No v1 client enforces rate limits on others' content.

**Reports in the MVP.** The only mechanism is `forum.reportContact` (`mailto:`, `https:`, or an `at://` handle for DMs), shown by the "Report" action through `safe_href`. Nothing is written to public repos.

**Reports, Reserved (P2)**
- A sealed `report{ciphertext ≤ 16 KiB}` record: age v1 with X25519 anonymous recipients and padding, **no** forum, subject or recipient fields, and a random publication delay of 0–6 h.
- Or a forum report service via `com.atproto.moderation.createReport`, with scope `rpc:…createReport?aud=*`.

---

## 19. Test vectors and property tests

**Vectors.** Normative vectors live in `testdata/vectors/{time, modfold, karma, reacts, rank, thread, tags, fold}/*.json`.
- **Format:** `{name, input, expected}`, where `input` is a `FoldInput` (§1.1) plus a `Query`, with `now` and `obs` injected.
- **Runners:** the Rust testkit, and TypeScript through the wasm `vectors` build. Their results must be equal.
- **Floats:** `hot` values are written as 16 lowercase hex digits of the binary64 bit pattern.

**Required cases**

| Kind | Cases |
|---|---|
| time | future-dated by just under and just over 5 min; comment claimed before its parent; missing or unparseable `createdAt`; declaration lower bound |
| modfold | owner beats moderator; same-rank hide/keep gives hide; unordered `tag.head` conflict; lapse on removal from the declaration, then adopt; `until` expiry; CID-bound pin, tier and keep after an edit; keep vs ban at equal and lower rank; restrict hides retroactively, overridden by a keep at higher rank; empty restrict at higher rank; `tag:` subject by a moderator; author tier at gate 49/50 and personal/frontpage; author-tier immunity of authorities; forum keep beats author hide; forum unlock beats author lock; author who is also a moderator (both scopes); forum-uri lock has no effect; degraded view |
| karma | power boundaries for k ∈ {−5, 0, 9, 10, 99, 100, 249, 250, …, 499999, 500000, 10⁹} × 4 values; self-vote ignored; strongUp counts 1 in K1; one vs two seeds for establishment; sybil ring with no seed edge; `pairCap` boundary 25/26; deleted subject with 1 vs 2 E voters; nullify; restrict vote; seed with negative K1 stays in E |
| reacts | supporters {9} invalid; {10, 4} one supporter; {10, 5} two; anti from K 19 ignored and from K 20 counted; for/against contradiction dropped; two quotes with the same `qkey`; creator tie by DID |
| rank | exact `hot` bit patterns; b ≤ 0 partition; pinned first; 90-day cutoff only in magic (top, new, old unaffected); pinned post beyond the cutoff; uri tie-break |
| thread | 2-cycle and 3-cycle; missing parent placeholder; cross-root parent; depth 1,001 re-attached |
| tags | linear chain head; fork by depth, then K, then uri; moderator `#tag.head`; removed tag; deleted parent gives a flagged root gated as genesis; ancestor altered; `tagCreation = moderators`; `isTag = false`; relevance boundaries |
| fold | `inputDigest` of a fixed input; native == wasm |

**Property tests (proptest):** P1–P9 (§6.3). Also:
- visibility and ranking outputs are permutation-invariant;
- the thread cycle cut is independent of walk order;
- the lens never changes fold outputs.

**Client tests** (CLI and engine, not fold vectors):
- the lock is refused above 5,000 allowed comments;
- `mods rm` runs `mod adopt` first.

---

## 20. Decisions made while writing

These choices fill gaps in the sources or resolve ambiguities. Each takes the simplest safe option. All are open to review before M7.

### 20.1 Decisions

1. **Behaviour-ID coverage (§0.3).** The sources name the IDs but do not assign every rule to one.
   - Time goes under `magic/1`.
   - Participation, visibility and threads go under `mod/1`.
   - Opinion validity, scores, gates, react aggregation, tags and wiki, and the sequence library go under `karma/1`.
   - `default/1` covers only the palette's ids.
   - Notifications are local presentation and are not versioned.
2. **Fold input (§1.1).**
   - Added `absent` (confirmed-absent opinion subjects), which the deleted-subject rule needs.
   - `repoStatus` is defined as: `Stale` = listed before and cached; `Unavailable` = never completely listed.
   - Invalid records are fold inputs.
   - `inputDigest` is made exact: line encoding, `absent` lines, an `algs` line and a fixed `params` list.
3. **Time (§2).**
   - `t_obs` is the first-seen time of the *record* (the minimum over its versions), so an edit never makes an item "new".
   - The declaration's `createdAt` lower bound gets the same future clamp.
   - `t_rank` is defined only for items, and it is the displayed time.
4. **Participation ignores bans (§3.1).** Bans act only through V4, so that "an item-level keep overrides a ban" works in both modes. E explicitly excludes B ∪ N.
5. **Authorities can be banned or restricted by a higher rank (§4.3).** It hides their content but never removes authority.
6. **Dimension assignment (§4.2).**
   - `#hide` asserts `spam = hidden ∧ spam`.
   - Positivity belongs to the whole record.
   - Absent fields assert their defaults, except `#tag.head`.
   - A `#restrict` record states the complete set: a higher rank replaces lower sets, so an empty set lifts them, and equal ranks unite. `data-model.md` MA8 has been amended to say the same (formerly README §1.4 K2).
7. **Overriding a ban (§4.6).** A higher rank can only write `#access{member}`, which also grants seed status. This is stated rather than adding a new access level.
8. **Keeps exempt from locks (V7).** A forum-tier keep at rank ≥ the lock's rank exempts a comment from the lock, by analogy with "content-level over user-level". Any forum-tier keep or explicit forum-tier value overrides the author tier.
9. **Author tier (§4.9).**
   - The actor must be in E, which makes the restricted sybil property (P7) hold for other users' visibility.
   - Authorities are immune to author-tier hides, bans and locks (the "immunity" vector).
   - Post authors' own comments are exempt from their own lock.
10. **Restrict capability effects (§4.7).**
    - `vote` disables the karma and agreement axes.
    - `react` disables reacts.
    - `tag` disables tag votes and the user's own `post.tags` relevance.
    - `wiki` hides wikiRevisions.
    - `post`, `comment` and `sequence` hide items.
    - Restrictions apply per axis rather than voiding the whole opinion.
11. **`unweighted` counts** only counting opinions that carry a karma value (§7).
12. **Pair bound (P5)** is stated per stage, with a corollary. Read literally for K, the synthesis claim is false, because removing a voter changes others' KP and therefore their vote power.
13. **Sybil bound (P7)** gets precise hypotheses. Without (a) and (b), adding an outsider's voted-on content, or a returning user's membership record, legitimately changes E.
14. **The deleted-subject threshold** ("≥ 2 distinct voters") uses each stage's eligible set (S for K1, E for KP and K), which keeps the stages acyclic.
15. **React aggregation (§9.2).**
    - Groups by `(react, qkey)`, not by resolved range, so the fold never depends on whether a spilled body blob has been fetched (`lfm-1.md` "Pending"). Anchors are display-only.
    - The creator is chosen among creator-qualified supporters, and `supporters ∪ {creator}` is displayed. This stays well-defined if `reactSecond > reactCreate`.
16. **Degraded view (§4.11).** Uses the explicit state `Hidden(ModerationUnavailable)`.
17. **Thread depth cap (§11).** A node beyond depth 1,000 is re-attached to the root as `Orphaned`. `Missing` placeholders sort after comment siblings.
18. **Ranking (§12).**
    - The cutoff applies only to *post lists* under magic. Forum-pinned posts **are** exempt, as in ForumMagnum (sticky posts stay at the top regardless of age): a pin is an explicit, CID-bound moderator decision, so the cutoff exists to age out unpinned content only. (Changed after the consistency review to match ForumMagnum; the rank vectors include a pinned post beyond the cutoff.)
    - Pins come first only in post magic, and in every comment sort.
    - `recentComments` and Recent Discussion use only `Visible` comments, so collapsed newcomers cannot bump posts.
    - `recentComments` falls back to the post's own `t_rank`.
19. **Wiki (§13.2).**
    - Candidates are a least fixpoint, which prevents bypassing `createTag` or `tagCreation = moderators` with fake or ineligible parents.
    - A general SCC cycle cut is used.
    - The 1,000-revision cap keeps the greatest TIDs.
    - "Latest" means the deepest (tip of the longest chain), with K and then uri as tie-breaks.
    - `isTag = false` pages are wiki-only.
20. **Sequences (§14).**
    - The navigation fallback considers the author's own eligible sequences, then only library-listed ones, so unlisted spam sequences cannot hijack navigation.
    - Forum pins come first in the library.
21. **Notifications (§16).**
    - One notification per item, with fixed reason precedence.
    - Pingbacks come from posts only; mentions and pingbacks use `Rendered.mentions` and `Rendered.links`.
    - KarmaChange defaults to daily.
    - Reading sets `lastVisited` to the greatest displayed `t_obs`, not to `now`.
22. **Mod log (§4.12)** lists every modAction in the forum's input (both scopes, current and former actors) plus modAction revisions, ordered by `(updatedAt ?? createdAt) desc`.
23. **Forum equality (V1, O1, §3.1, §4.2).** Every fold rule checks `forum = F` explicitly, not only through the key's fk. Records of another forum whose fk collides (60 bits) therefore never leak in.
24. **P7 also excludes authorities.** A banned or nullified authority is outside E but its assertions still count, so its records are excluded from the hypothesis.

### 20.2 Open questions

1. *(Resolved in the consistency review: `data-model.md` MA8 now says that an empty `#restrict` lifts lower-rank restrictions, as §4.2 and §4.7 require.)*
2. **The `absent` input (§1.1)** must be produced by `sync.md`, from tombstones and complete listings, and included in the indexer's `inputDigest`.
3. **Author moderation requires E (§4.9).** This tightens the gate beyond the synthesis. The alternative is to drop it and weaken P7 to exclude visibility inside outsiders' own threads.
4. *(Resolved: forum-pinned posts are exempt from the magic cutoff, matching ForumMagnum's sticky posts; see 20.1 item 18.)*
