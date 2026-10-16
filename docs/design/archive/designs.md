##### DESIGN DOC: PROTOCOL #####
# lessdugin protocol and data model (v1)

## 0. Decisions in brief

1. **Namespace.** All records, shared defs and permission sets go in one flat authority group, `online.7778777.lessdugin.*`, which needs one DNS TXT record. The optional indexer's queries go in a second group, `online.7778777.lessdugin.index.*`, with its own TXT record. Experiments, if they ever need publishing, go in `online.7778777.lessdugin.temp.*`. I chose a flat layout over feature groups (`feed.*`, `mod.*`, ...) for these reasons:
   - A permission set may only cover its own group and child groups. Flat means one set covers everything.
   - Jetstream clients want `online.7778777.lessdugin.*` as a single wildcard anyway.
   - Feature grouping only adds DNS records and lexicon repos to govern.
2. **Trust.** A record is authoritative only as fetched from its author's PDS. Optionally it can be proof-verified with `sync.getRecord`. Everything else is a hint: relays, Jetstream, Constellation and indexers.
3. **One record per (author, subject).** Collections with this rule use deterministic subject-key rkeys (`sk/1`, section 2.3). Readers must drop any record whose rkey is not the expected key. Without that read-side rule, "one vote per user" is only cosmetic.
4. **Moderation is state, not events.** Each mod action is a record asserting a current state, keyed deterministically. Deleting the record undoes the action. Only the forum owner and moderators listed in the current forum declaration have authority.
5. **Edits are snapshotted.** An edit is one atomic `applyWrites`: create a `revision` that holds the superseded record verbatim (as a union of record types; the spec discourages `unknown` in records) plus a strongRef to it, then update the record. The hash check `cid(revision.record) == revision.subject.cid` lets anyone verify the history.
6. **Timestamps are untrusted.** Effective time = min(`createdAt`, rkey TID time, first-seen time if known). Records dated in the future are hidden until that time.
7. **Lexicon limits are permanent.** The spec forbids changing any size or length limit. Every limit below was chosen once, and media blobs have no `maxSize`.
8. **No floats.** Lexicon has no float type. Scores are integers, and ranks are decimal strings in index responses.

---

## 1. Namespace, publishing, DNS

| Group (authority) | DNS TXT | Contents |
|---|---|---|
| `online.7778777.lessdugin` | `_lexicon.lessdugin.7778777.online  "did=<publisher DID>"` | 13 record types, `markdown`, `defs`, `authFull`, `authInteract` |
| `online.7778777.lessdugin.index` | `_lexicon.index.lessdugin.7778777.online  "did=<publisher DID>"` | indexer XRPC queries and `index.defs` |
| `online.7778777.lessdugin.temp` (only if used) | `_lexicon.temp.lessdugin.7778777.online` | experiments; stable clients never write these |

- **Publisher.** A dedicated did:plc account with handle `lessdugin.7778777.online` (`_atproto` TXT). It holds one `com.atproto.lexicon.schema` record per NSID, with rkey = NSID. The JSON lives in `lexicons/` in the repo, is linted with `goat lex lint`, checked against the previous release with `goat lex breaking`, and published by CI.
- **Naming rule.** No child group may share a name with a record (for example, never create `online.7778777.lessdugin.forum.*`).

**Record NSIDs and rkeys**

| NSID | key | Phase |
|---|---|---|
| `.forum` | any (= slug) | v1 |
| `.membership` | any (= sk) | v1 |
| `.post` | tid | v1 |
| `.comment` | tid | v1 |
| `.revision` | tid | v1 |
| `.vote` | any (= sk) | v1 |
| `.react` | any (= sk) | v1 |
| `.tagVote` | any (= sk) | v1 |
| `.wikiRevision` | tid | v1 |
| `.sequence` | tid | v1 |
| `.coauthor` | any (= sk) | v1 |
| `.profile` | literal:self | v1 |
| `.modAction` | any (= sk) | v1 |
| `.turn` | tid | P2 (reserved) |
| `.report` | tid | P2 (reserved) |
| `.pollVote` | any (= sk) | P2 (reserved) |
| `.collection` | tid | P2 (reserved) |

Non-record lexicons: `.markdown` (object), `.defs`, `.authFull`, `.authInteract`, and `.index.*`.

Reserved P2 names may not be written or published until they are final.

---

## 2. Global conventions

### 2.1 Notation

- `!` means required.
- `str≤Ng/Mb` means maxGraphemes N and maxLength M bytes (UTF-8).
- `[T]≤N` means an array of at most N items.
- `kv{…}` means knownValues: an open set, never `enum`.
- `int[a..b]` means an integer with minimum a and maximum b.
- `strongRef` means `com.atproto.repo.strongRef`.
- `md` means `union[online.7778777.lessdugin.markdown]`. It is an **open** union.
- `slug` means `str≤64b`, matching `^[a-z0-9]+(-[a-z0-9]+)*$` (ASCII only).
- Every datetime is RFC 3339 UTC with `Z` and millisecond precision.
- No closed unions anywhere.

### 2.2 References

- **Stored at-uris always use a DID authority**, never a handle. Writers resolve handles first. Readers drop records with handle authorities in reference fields. The canonical form is `at://<did>/<collection>/<rkey>`, with no query, fragment or trailing slash. A user subject is written `at://<did>`.
- **Use `strongRef` where the version matters:** comment→post and comment→parent, votes, reacts and tagVotes (pin the version judged, and make quotes anchorable), `revision.subject`, `wikiRevision.prev`, `coauthor.subject`.
- **Use a plain `at-uri` where the identity matters:** `forum` fields, mod-action subjects (they apply to all versions), sequence entries, profile pins.
- A `forum` field is fixed after creation. If a post's forum changes, its comments and votes no longer match it and are dropped. Clients refuse to change it.

### 2.3 Subject keys (sk/1)

```
sk(collectionNSID, parts[]) =
  b32s( sha256( UTF8( "lessdugin-sk/1" ⧺ 0x1F ⧺ collectionNSID ⧺ (0x1F ⧺ NFC(part_i))* ) ) [0..10 bytes] )
b32s: read bits MSB-first, 5-bit groups, alphabet "234567abcdefghijklmnopqrstuvwxyz",
      80 bits → exactly 16 chars, no padding.
```

- Parts must not contain U+001F; reject the input if they do. An absent optional part is the empty string.
- Keys are 16 characters, so they can never be confused with 13-character TIDs, and they fit the rkey charset.
- At 80 bits, an accidental collision is negligible. A targeted second-preimage costs about 2^80, and the only harm would be blocking one vote.

| Collection | parts |
|---|---|
| membership | `[forumUri]` |
| vote | `[subject.uri]` |
| react | `[subject.uri, react, quote.prefix?, quote.exact?, quote.suffix?]` (as stored) |
| tagVote | `[subject.uri, tag]` |
| coauthor | `[subject.uri]` |
| modAction | `[forumUri or "", subjectUri, actionKey(action)]` (see 5.13) |
| pollVote (P2) | `[subject.uri]` |

**Vectors.** I computed these with a reference script; the repo should regenerate them as `testdata/vectors/sk.json`.

| Collection | Inputs | Key |
|---|---|---|
| vote | `at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.post/3l2s5xxv2ze2c` | `ncepyaoj3bijw6li` |
| membership | `at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.forum/rationality` | `q2swgvbea4gb2m6c` |
| react | same post, `"insightful"`, `""`, `""`, `""` | `mbibtewaiedhx3gd` |
| modAction | forum above, `at://did:plc:abcdefghijklmnopqrstuvwx`, `"ban"` | `bawazc54xxhjkhdy` |

### 2.4 Write rules

- Every record carries `$type`.
- Writers never pass `validate:true`, because the PDS cannot resolve our lexicons and would reject the write. They leave it unset.
- Writers must preserve unknown fields on read-modify-write. This is the forward-compatibility rule.
- Updates to posts, comments, the forum declaration and mod actions use `applyWrites` with `swapCommit`. Writes to sk-keyed records are idempotent `putRecord` calls with no swap.
- Media is uploaded only at publish time. An unreferenced blob is garbage-collected after about an hour.
- **Membership beacon.** Before a client first writes content into a forum, it must make sure a `membership` record exists. This is what keeps serverless discovery working.
- Deleting a post or comment also deletes its `revision` records (≤200 ops per commit). Deleting or updating a mod action creates a `revision` (`op` = delete or update) if `forum.policy.modLog` is true. Content deletions are never snapshotted.

### 2.5 Read rules

These are core validation rules, beyond the lexicon itself.

- Validate against the lexicon, ignore unknown fields, and drop invalid records.
- Drop sk-collection records whose rkey is not the expected key.
- Hide records with `createdAt > now + 5 min` until that time.
- Detect cycles in comment `parent` chains. Records are mutable, so cycles are possible; treat cyclic comments as orphans.
- Do not show cached content after its source record is deleted or the account is unavailable.
- Normalisation and size limits for rendering are in section 4.

---

## 3. Shared defs

### `online.7778777.lessdugin.markdown`

The `main` def is an object. It is the `$type` used in every `md` union and can be embedded by other apps (for example in `site.standard.document.content`).

```
profile:   str≤64b kv{"lfm/1"} default "lfm/1"
text:      str≤100000b                         inline LFM source
textBlob:  blob accept["text/*"] maxSize 10000000   spillover, UTF-8 LFM
media:     [defs#media]≤200                    scope for cid: URLs in this body
mentions:  [did]≤128                           DIDs mentioned (for notifications and backlinks)
links:     [at-uri]≤256                        at:// targets linked (pingbacks)
```

- Exactly one of `text` and `textBlob` must be present.
- Writers spill to `textBlob` above 64 KiB.
- `textBlob` is allowed only in `post.body`, `wikiRevision.body`, `forum.about` and `sequence.body`. Comments, bios, chapters, guidelines and curation notices must be inline; that keeps thread assembly to a single fetch.
- 100,000 bytes is safe for an edit: post plus revision snapshot is at most about 2 × 200 KB of escaped JSON, under the 1 MB `applyWrites` limit.
- `mentions` and `links` are derived by the writer. Readers treat them as hints for indexing and notification; rendering always comes from the text.

### `online.7778777.lessdugin.defs`

```
#media        { blob!: blob (accept */*, no maxSize), alt: str≤2000g/20000b, name: str≤255b,
                aspectRatio: #aspectRatio, thumb: blob accept[image/jpeg,image/png,image/webp,image/avif] maxSize 1000000,
                durationMs: int≥0 }
#aspectRatio  { width!: int≥1, height!: int≥1 }
#textQuote    { exact!: str≤2000g/20000b, prefix: str≤128g/1280b, suffix: str≤128g/1280b, start: int≥0 }
#externalLink { uri!: uri, title: str≤300g/3000b, description: str≤1000g/10000b, thumb: blob accept[image/*] maxSize 1000000 }
#link         { uri!: uri, label: str≤64g/640b }
#moderationPrefs { style: kv{easy-going, norm-enforcing, reign-of-terror}, guidelines: md }
#poll (P2)    { options!: [str≤300g/3000b]≤20, multiple: bool, closesAt: datetime }
```

**`cid:` URLs.** `![alt](cid:<CIDv1 base32>)` resolves only against the same body's `media[].blob.ref`. If nothing matches, show a placeholder. Never fetch an arbitrary CID. `media` items that are not referenced in the text render as an attachment gallery after the body. Render by mime type:

- `image/*`, `video/*`, `audio/*` from an allow-list render inline. The SPA does this with fetch, an object URL with the declared type, then the element.
- Everything else renders as a download card.

---

## 4. LFM/1 (lessdugin flavoured markdown, profile `lfm/1`)

The profile is fixed once released. Normative test vectors go in `testdata/vectors/lfm1/*.json` (input, events, HTML, plain text).

### 4.1 Syntax

- **Base:** CommonMark 0.31.2.
- **Enabled extensions** (these are pulldown-cmark 0.13 options, so the Rust core is the reference implementation):
  - GFM tables.
  - Strikethrough, `~~x~~` and `~x~`. Sub- and superscript are off, so single `~` means strikethrough.
  - Task lists, rendered read-only.
  - GFM-style footnotes (`ENABLE_FOOTNOTES`, not the old footnote syntax).
  - Math (`ENABLE_MATH`): `$…$` inline and `$$…$$` display. The content is TeX in the KaTeX/Temml-compatible subset; on a render error, show the source as code.
  - GFM alerts (`ENABLE_GFM`): `> [!NOTE|TIP|IMPORTANT|WARNING|CAUTION]`.
- **Extended autolinks.** GFM §6.9 literals (`www.`, `http://`, `https://`, but not email or xmpp) are recognised by a core post-pass over Text events outside links and code. pulldown-cmark has no such option, so this pass is part of the spec.
- **Disabled**, so the input renders as literal text:
  - raw HTML, both blocks and inline (escaped and shown);
  - smart punctuation;
  - heading attributes;
  - YAML and `+++` metadata blocks;
  - definition lists;
  - super/subscript;
  - wikilinks.

### 4.2 URL policy

**Link destinations**

- Allowed: `https:`, `http:`, `mailto:`, `#fragment`.
- `at://` becomes an internal route with a hover preview. Writers rewrite handle authorities to DIDs on publish.
- `at://<did>` with no path is a mention.
- Anything else, including relative URLs, `javascript:`, `data:` and `file:`, is dropped: the link text stays and the link does not.

**Images**

- `cid:` images render as media (see 3).
- `http(s):` images are **not fetched by default**. They render as a link reading "image: alt", so there are no tracking pixels and no IP leak. Users can opt in per user or per forum.
- Other schemes render as the alt text only.

**Mentions.** If the link text starts with `@` and the destination is `at://<did>`, the display text is replaced with `@` plus the bidirectionally verified current handle, or the DID if the handle is unverified. This prevents spoofing like `[@alice](at://did:bob)`.

### 4.3 HTML output contract

- Allowed elements: p, h1–h6, blockquote, ul, ol[start], li, input[type=checkbox][disabled], pre, code[class=language-*], em, strong, del, a[href][rel="nofollow ugc noopener"], img/video/audio (object URLs only), table, thead, tbody, tr, th, td[style=text-align], hr, br, sup, the footnote section, MathML or KaTeX spans, div.alert-*.
- No other attributes come from content.
- Heading ids: `h-` + a GitHub-style slug (Unicode-lowercase; keep letters, digits, `-` and `_`; spaces become `-`; deduplicate with `-1`, `-2`, ...). Footnote ids: `fn-<label>`, references `fnref-<label>-<n>`. The prefixes prevent DOM clobbering.
- Renderer limits: nesting depth 64 (deeper content flattens to paragraphs); at most 2^20 events, after which output is truncated with a notice.
- The TUI renders the same event stream: TeX source for math, and images through ratatui-image or as a placeholder.

### 4.4 Canonical plain-text projection P(source)

P is used for quotes, excerpts, search and word counts. Walk the LFM/1 event stream:

| Event | Contributes |
|---|---|
| Text, Code (inline), InlineMath, DisplayMath (TeX source), code-block text, Html/InlineHtml (shown literally, so projected literally) | its content |
| Link, Image | their inner text events only (link text, alt text). URLs contribute nothing |
| SoftBreak, table cell boundary | `" "` |
| HardBreak, start or end of any block (paragraph, heading, blockquote, list item, code block, table row, footnote definition, alert, rule) | `"\n"` |
| FootnoteReference, TaskListMarker | nothing |

Then:

1. Unicode NFC.
2. Replace every maximal run of `White_Space` characters with a single U+0020.
3. Trim.

Offsets are counted in Unicode scalar values (code points), not UTF-16 units or bytes.

Derived values:

- Excerpt: the first 500 graphemes of P, cut at the last space, plus `…`.
- Word count: the number of maximal non-space runs (informational only).

### 4.5 Text quote selectors

A quote (in `comment.quote` or `react.quote`) is resolved against P of the target's body at the referenced cid version. The target is the parent comment if one is given, otherwise the subject.

- `exact` is trimmed, so it starts and ends on non-space.
- `prefix` and `suffix` are verbatim substrings of P immediately before and after `exact`, so they may include a boundary space. Writers use 32 graphemes.
- `start` is a code-point offset hint.

**Matching:**

1. Find all occurrences of `exact`.
2. If there is one, it matches.
3. If there are several, choose the one with the longest prefix-suffix agreement, then the one nearest `start`.
4. If there are none in the current version, try fuzzy matching (implementation-defined, threshold ≥ 0.8). If that fails, the quote is **orphaned** and shown under the item with "quoted an earlier version".

---

## 5. Records (v1)

### 5.1 `online.7778777.lessdugin.forum` (key any = slug; in the owner's repo)

```
name!:          str≤100g/1000b
description:    str≤500g/5000b            plain tagline
about:          md                         rules/intro; may spill
icon:           blob accept[image/*] maxSize 2000000
banner:         blob accept[image/*] maxSize 5000000
langs:          [language]≤8
links:          [defs#link]≤16
moderators:     [#moderator]≤128
membership:     str kv{open, approval} default "open"
sections:       [#section]≤64
defaultSection: slug
defaultTier:    str kv{personal, frontpage} default "frontpage"
featuredTags:   [slug]≤64                  "core tags" for filters
policy:         #policy
reacts:         #reactPalette
discovery:      #discovery
labels:         union[com.atproto.label.defs#selfLabels]   forum-wide content warning
movedTo:        at-uri                     forum migration
archived:       bool                       read-only
createdAt!, updatedAt: datetime

#moderator   { did!: did, role!: str kv{admin, moderator, curator, reviewer}, title: str≤64g/640b }
#section     { id!: slug, name!: str≤100g/1000b, description: str≤500g/5000b,
               postPolicy: kv{anyone, members, moderators} default anyone,
               commentPolicy: kv{anyone, members, moderators} default anyone,
               defaultTier: kv{personal, frontpage}, hidden: bool }
#policy      { scoring: str≤64b kv{"karma/1"} default "karma/1",
               ranking: str≤64b kv{"magic/1"} default "magic/1",
               voting:  kv{default, twoAxis, reactions} default twoAxis,
               gates: #gates, rateLimits: [#autoRateLimit]≤32,
               authorModeration: { enabled: bool default true, minKarmaPersonal: int, minKarmaFrontpage: int },
               tagCreation: kv{open, moderators} default open,
               modLog: bool default true,
               karmaSources (P2): [at-uri]≤16, labelers (P2): [did]≤16 }
#gates       { vote, strongVote, agree, comment, post, reactNew, reactSecond, antiReact, sideComment,
               tagVote, createTag, wikiEdit, sequence, coauthor : int }   (karma thresholds; absent = policy default)
#autoRateLimit { action!: kv{post, comment}, belowKarma!: int, count!: int≥0, periodHours!: int≥1 }
#reactPalette  { base: str≤64b kv{"default/1","none"} default "default/1", add: [#reactDef]≤256, remove: [slug]≤256 }
#reactDef    { id!: slug, label!: str≤64g/640b, emoji: str≤8g/64b, icon: blob accept[image/*] maxSize 262144,
               description: str≤300g/3000b, group: str≤64b }
#discovery   { indexers: [#endpoint]≤8, jetstream: [uri]≤8, relays: [uri]≤8, backlinks: [uri]≤8, web: uri }
#endpoint    { url!: uri, did: did, note: str≤300g/3000b }
```

- The forum's identity is its at-uri. The owner DID is the root of authority; an organisation account is recommended.
- The owner is an implicit admin.
- **Only the current `moderators` list confers authority.** Removing a moderator also removes the effect of their actions. To keep a retiring moderator's actions, the CLI command `lessdugin mod adopt` re-issues them under another moderator, with `adopts` pointing at the original.
- Unknown `role` values grant nothing. An unknown policy id falls back to the default policy and the client shows a warning badge.

### 5.2 `.membership` (key = sk(membership, [forum]))

```
forum!: at-uri
note: str≤2000g/20000b      application text (approval mode)
createdAt!
```

- In **open** mode, a membership record is a discovery beacon and subscription. It is required for posting in sections with `postPolicy: members`.
- In **approval** mode, it is a join request. Eligibility comes from a `modAction#member`.
- Leaving a forum means deleting the record.

### 5.3 `.post` (key tid)

```
forum:        at-uri        absent ⇒ personal blog (author-moderated)
section:      slug
kind:         str≤64b kv{article, question, link, quickTake, event, dialogue} default article
title:        str≤300g/3000b       app-required unless kind=quickTake
body:         md
excerpt:      str≤500g/5000b       app-required iff body uses textBlob
link:         defs#externalLink    kind=link
tags:         [slug]≤32            author's tag applications
coauthors:    [did]≤32             displayed only when confirmed by .coauthor
votingSystem: kv{default, twoAxis, reactions}   overrides forum default
moderation:   defs#moderationPrefs              overrides profile
labels:       union[com.atproto.label.defs#selfLabels]
langs:        [language]≤3
canonicalUrl: uri                  imports/crossposts (RSS import)
event (P2):   strongRef            community.lexicon.calendar.event with the SAME rkey
dialogue (P2): { order: [at-uri]≤2000, open: bool }
poll (P2):    defs#poll
createdAt!, updatedAt
```

Rules:

- Unknown `kind` renders as `article`.
- `quickTake` replaces FM's per-user container post: it is just a post without a title.
- Questions use `kind=question`. Answers are comments with `answer:true`.
- Validation: `section` must exist in the forum, and its `postPolicy` must permit the author; otherwise the post goes to `defaultSection` and becomes `pending` if mod-only.
- `coauthors` must not include the author.
- `createdAt` never changes on edit.

### 5.4 `.comment` (key tid)

```
subject!:     strongRef      root: a post (P2: a sequence, or a wiki page's creation revision for tag discussion)
parent:       strongRef      comment replied to; absent ⇒ top-level
forum:        at-uri         MUST equal root's forum (both absent for personal posts) else dropped
body!:        md             inline only
quote:        defs#textQuote anchored in parent if present, else subject (side comment / inline reply)
answer:       bool           valid only top-level under kind=question
retracted:    bool           rendered struck through
moderatorHat: bool           valid only if author is a moderator/admin of forum
langs, createdAt!, updatedAt
```

- `parent.subject` must equal `subject`.
- Side comments require karma ≥ `gates.sideComment`; below the gate, the comment renders as a normal comment without its anchor.

### 5.5 `.revision` (key tid)

```
subject!: strongRef     the superseded or deleted version
op!:      str kv{update, delete}
record:   union[.post, .comment, .forum, .sequence, .profile, .modAction] (open)  verbatim prior value
summary:  str≤300g/3000b    edit summary
createdAt!
```

- **Edit** = `applyWrites`([create revision{subject: post@oldCid, op: update, record: oldValue}, update post], `swapCommit`).
- **Verification:** the DAG-CBOR CID of `record` (which includes `$type`) must equal `subject.cid`. Old blobs stay referenced, so history keeps its media.
- Comments pinned to an older cid display "on revision n".
- Rate cost of an edit: 5 points.

### 5.6 `.vote` (key = sk(vote, [subject.uri]))

```
subject!:  strongRef     post | comment | wikiRevision
root:      at-uri        thread root when subject is a comment (backlink hint only)
forum:     at-uri        MUST equal subject's forum
overall:   int[-2..2]    ±1 small, ±2 strong ("karma" axis)
agreement: int[-2..2]
createdAt!, updatedAt
```

- At least one axis must be non-zero, and 0 means the same as absent. Removing a vote means deleting the record.
- One record covers both axes: about 3 rate points instead of 6, and the change is atomic.
- New axes may be added later as optional fields.

### 5.7 `.react` (key = sk(react, [subject.uri, react, quote.prefix, quote.exact, quote.suffix]))

```
subject!: strongRef
react!:   slug          palette id (forum palette = base ∪ add − remove; unknown ids shown by id)
against:  bool          anti-react ("this react doesn't apply")
quote:    defs#textQuote inline react anchor
root, forum: at-uri     as in vote
createdAt!
```

- A react's first occurrence (lowest effective time) requires `gates.reactNew`. Later ones require `reactSecond`, and `against` requires `antiReact`.

### 5.8 `.tagVote` (key = sk(tagVote, [subject.uri, tag]))

```
subject!: strongRef (post)
tag!:     slug
value!:   int[-2..2]    non-zero; negative = "does not apply"
forum:    at-uri
createdAt!, updatedAt
```

- A tag's relevance to a post (FM's TagRel) is computed from tagVotes plus the author's `post.tags` (section 7). There is no shared TagRel object.

### 5.9 `.wikiRevision` (key tid; immutable; tag and wiki pages)

```
forum!:      at-uri
slug!:       slug              page identity = (forum, slug); no page "owner"
prev:        strongRef         revision this edit is based on; absent ⇒ page creation
name!:       str≤100g/1000b
description: str≤300g/3000b    plain, for hovers and cards
body:        md                may spill
isTag:       bool default true usable as post tag
parents:     [slug]≤16         parent tags
summary:     str≤300g/3000b    edit summary
createdAt!
```

**Head selection**

- Candidates are revisions that are eligible (author meets `gates.wikiEdit`, is not restricted, and is not hidden) and whose `prev` chain resolves, with matching cids, through candidates of the same (forum, slug).
- Heads are candidates with no candidate child.
- If a `modAction#wikiPage{state: locked, revision}` exists, that revision is the page.
- Otherwise the page is the head with the maximum (effective time, uri). Additional heads are shown as edit conflicts.

**Existence**

- A page exists if it has at least one candidate, and either `tagCreation=open` or its creation revision was written by a moderator or approved by `#wikiPage{state: approved}`.

**Other rules**

- Votes may target revisions.
- Tag discussion uses comments whose subject is the page's creation revision.

### 5.10 `.sequence` (key tid)

```
forum:     at-uri
title!:    str≤300g/3000b
body:      md
cover:     defs#media
chapters!: [#chapter]≤256
createdAt!, updatedAt
#chapter { title: str≤300g/3000b, body: md (inline), posts!: [at-uri]≤512 }
```

- Entries are at-uris, so they always show the latest version, and they may include other authors' posts.
- Listing a sequence in a forum's library requires `gates.sequence`, or a moderator `#pin`.

### 5.11 `.coauthor` (key = sk(coauthor, [subject.uri]))

```
subject!: strongRef (post that lists me in coauthors)
createdAt!
```

- A co-author is shown if and only if the post lists them **and** this record exists. Withdrawing means deleting it.
- Dialogue participants work the same way.

### 5.12 `.profile` (key literal:self)

```
displayName: str≤64g/640b      override; fallback app.bsky.actor.profile (read from PDS, not AppView)
avatar:      blob accept[image/*] maxSize 2000000   override; same fallback
bio:         md (inline)
links:       [defs#link]≤32
pinned:      [at-uri]≤16
moderation:  defs#moderationPrefs   FM moderationStyle + guidelines
createdAt, updatedAt
```

### 5.13 `.modAction` (key = sk(modAction, [forum ?? "", subject, actionKey]))

```
forum:      at-uri       absent ⇒ author-scope (personal posts / all my posts)
subject!:   at-uri       record uri, or at://<did> for user-scoped actions, or forum uri for wiki actions
subjectCid: cid          version observed (audit)
action!:    union[#hide, #ban, #restrict, #member, #section, #tier, #pin, #lock, #rateLimit, #wikiPage, #tagAlias] (open)
reason:     str≤2000g/20000b
until:      datetime     expiry
adopts:     strongRef    re-issue of a former moderator's action
createdAt!, updatedAt

#hide      { public: bool default true (placeholder shows reason), spam: bool }          key "hide"
#ban       { post: at-uri (author scope: only this post) }                               key "ban" | "ban:"+post
#restrict  { capabilities!: [kv{post,comment,vote,react,tag,wiki,sequence}]≤16, nullifyVotes: bool }  key "restrict"
#member    { }                                                                            key "member"
#section   { section!: slug }                                                             key "section"
#tier      { tier!: kv{personal, frontpage, curated}, notice: md (curation notice) }      key "tier"
#pin       { scope!: kv{forum, section, thread}, order: int }                             key "pin:"+scope
#lock      { allowAccountsCreatedBefore: datetime (PLC audit-log time), exempt: [did]≤128 } key "lock"
#rateLimit { kind!: kv{post, comment, vote, react}, count!: int≥0, periodHours!: int≥1 }  key "rateLimit:"+kind
#wikiPage  { slug!: slug, state!: kv{approved, locked, hidden}, revision: strongRef }      key "wiki:"+slug
#tagAlias  { slug!: slug, target!: slug }                                                 key "alias:"+slug
```

Unknown action types use their `$type` as the action key and have **no effect**; they are displayed as "unknown action".

**Authority.** Actions are evaluated against the current forum record.

| Action | owner/admin | moderator | curator | reviewer | Post author (if `authorModeration.enabled` and karma ≥ minKarmaPersonal or minKarmaFrontpage by tier) |
|---|---|---|---|---|---|
| hide | yes | yes | – | spam only | comments on own post |
| ban | yes | yes | – | – | from own post or all own posts |
| restrict, rateLimit, section | yes | yes | – | – | – |
| member | yes | yes | – | yes | – |
| tier | yes | personal and frontpage | all, including curated | – | – |
| pin | yes | yes | forum scope | – | thread scope on own post |
| lock | yes | yes | – | – | own post |
| wikiPage, tagAlias | yes | yes | – | – | – |

**Immunity**

- Actions targeting the owner are ignored.
- Actions targeting an admin are effective only if issued by the owner.
- Actions targeting a moderator are effective only if issued by an admin or the owner.

**Combining assertions from several moderators**

- hide, ban, lock and member are combined with OR.
- For section and tier, the authorised assertion with the latest effective time wins (ties broken by uri).
- pins are unioned.
- rateLimit takes the strictest value.
- An expired `until` means no effect.

**Author-scope assertions.** A `modAction` with no `forum` field, a subject of `at://did` and `#ban` bans that user from all of the author's posts, wherever author moderation is permitted.

**Fold outputs** per item: visible, hidden(reason, public), pending (approval mode, not yet a member), author-banned, future-dated, invalid, orphaned. Plus the effective section, tier, pins and lock.

---

## 6. Phase 2: reserved shapes

These are not written or published until they are final.

- **`.turn`** (dialogues): `{subject!: at-uri (dialogue post), body!: md, createdAt!, updatedAt}`.
  - A turn is included only if its author is the initiator or a confirmed co-author.
  - Order comes from `post.dialogue.order` if present, otherwise (effective time, did).
  - Live updates via Jetstream `wantedDids`.
  - Note that dialogue drafts are public.
- **`.report`**: `{forum!: at-uri, ciphertext!: bytes≤16384, recipients!: [str≤64b]≤16, createdAt!}`.
  - The ciphertext is age v1 with X25519 recipients taken from a future optional field `forum.reportKeys: [{id!, did!, x25519!: bytes(32)}]`.
  - The plaintext is `report#payload {subject, cid, reason kv{spam, harassment, rule, illegal, other}, comment≤2000g}`.
  - This leaks metadata (reporter, forum, time) but not the target. The alternative is a forum-designated atproto labeler (server-side, optional).
- **`.pollVote`**: key sk; `{subject!: strongRef, choices!: [int]≤20, createdAt!}`. The poll itself is `defs#poll` on a post or comment.
- **`.collection`** (FM books): `{forum, title!, body: md, books!: [{title, body, sequences!: [at-uri]≤128}]≤64, createdAt!}`.
- **Events:** dual-publish `community.lexicon.calendar.event` with the same rkey as the `kind=event` post, which points to it via `post.event`. RSVPs use `community.lexicon.calendar.rsvp`. Event details live only in the community record, so it is the single source of truth.

---

## 7. Policies (versioned and immutable; frozen once released)

### karma/1 (scope: one forum; personal posts use power 1 for every vote)

**Eligible vote:**

- the rkey is valid;
- the subject resolves and is in the forum;
- the subject is not hidden as spam;
- the voter is not one of the subject's authors (author plus confirmed co-authors);
- the voter is not banned, restricted from voting, or nullified;
- in approval mode, the voter is a member or moderator;
- the vote's effective time ≤ now.

**Computation:**

1. **Pass 1:** `k0(u)` = Σ sign(overall) over eligible votes on u's posts, comments and wikiRevisions.
2. **Power:** `power(v)` = sign × (1 if |v| = 1 and k0 < 1000; 2 if |v| = 1 and k0 ≥ 1000; strong(k0) if |v| = 2), using FM's strong table: <10:1, 10:2, 100:3, 250:4, 500:5, 1000:6, 2500:7, 5000:8, 10000:9, 25000:10, 50000:11, 75000:12, 100000:13, 175000:14, 250000:15, 500000:16.
3. **Pass 2:** `karma(u)` = Σ power over eligible overall-axis votes on u's content. Every confirmed co-author receives the full amount.
4. `score(x)` = Σ power + an implicit small upvote by the primary author (FM parity).
5. `agreement(x)` = the same calculation on the agreement axis. Agreement never affects karma, and reacts never do.
6. **Tag relevance** of (P, T) = Σ power(tagVote.value) + (T ∈ P.tags ? power(+1 by P's author) : 0). The tag applies if relevance > 0.

Why two passes: FM computes vote power from the voter's karma at the time of the vote. That cannot be reproduced trustlessly with self-asserted timestamps. Two passes are deterministic, O(votes), and immune to timestamp games.

**Gate defaults** (FM): vote 0, reactSecond 5, reactNew 10, antiReact 20, sideComment 10, coauthor 1, sequence 1000, authorModPersonal 50, authorModFrontpage 2000, createTag 10, wikiEdit 5.

- `lessdugin forum create` writes gates of 0 by default so that new forums can bootstrap.
- Moderators bypass all gates.
- Gates are evaluated with current karma.
- Default auto rate limits (FM): comments below 5 karma, 3 per day; below −2, 1 per day; posts below 5 karma, 2 per week.
- Rate limits can only be measured against observed times (client or indexer first-seen), so they are **best-effort**. That is a protocol limit and should be documented as such.

### magic/1 (ranking)

- `effectiveTime` = min(createdAt, TID time of the rkey, firstSeen).
- Post rank = (score + 10 if curated) / (ageHours + 2)^1.15.
- Comment magic = score / (ageHours + 2)^1.15.
- Sorts: magic, top, new, old, recentComments.
- The frontpage activity-factor variant is deferred to `magic/2`.

### default/1 react palette

The normative file is `docs/spec/reacts-default-1.json`. Its ids are slugs: agree, disagree, important, insightful, changed-mind, crux, not-a-crux, locally-valid, locally-invalid, typo, wrong, misleading, unclear, dont-understand, examples, citation-needed, concrete, overconfident, underconfident, hits-the-mark, missed-the-point, strawman, steelman, scout-mindset, soldier-mindset, too-harsh, good-point, thanks, seen, will-reply-later, laugh, heart, surprise.

---

## 8. Permission sets and OAuth scopes

`online.7778777.lessdugin.authFull`:

```json
{"lexicon":1,"id":"online.7778777.lessdugin.authFull","defs":{"main":{"type":"permission-set",
 "title":"lessdugin (full access)",
 "detail":"Create, edit and delete your lessdugin forums, posts, comments, votes, reactions, wiki edits, sequences, co-authorships, profile and moderation actions.",
 "permissions":[{"type":"permission","resource":"repo","collection":[
  "online.7778777.lessdugin.forum","online.7778777.lessdugin.membership","online.7778777.lessdugin.post",
  "online.7778777.lessdugin.comment","online.7778777.lessdugin.revision","online.7778777.lessdugin.vote",
  "online.7778777.lessdugin.react","online.7778777.lessdugin.tagVote","online.7778777.lessdugin.wikiRevision",
  "online.7778777.lessdugin.sequence","online.7778777.lessdugin.coauthor","online.7778777.lessdugin.profile",
  "online.7778777.lessdugin.modAction"]}]}}}
```

- `authInteract` covers: membership, comment, revision, vote, react, tagVote, coauthor. It is meant for lightweight third-party readers.
- Reads are public, and indexer calls are unauthenticated, so no `rpc` permissions are needed.

| Use | Scope string |
|---|---|
| Default (CLI, TUI, SPA) | `atproto include:online.7778777.lessdugin.authFull blob:*/*` (blob scope cannot go in a set) |
| Opt-in interop (requested when enabled) | add `repo:community.lexicon.bookmarks.bookmark repo:community.lexicon.calendar.event repo:community.lexicon.calendar.rsvp repo:site.standard.document repo:site.standard.publication` |
| Fallback if the PDS fails to resolve `include:` | `atproto repo?collection=<each of the 13> blob:*/*` |

- The `scope` in client metadata is the superset of all of these.
- The app-password path needs no scopes.

---

## 9. Optional indexer (`online.7778777.lessdugin.index.*`, unauthenticated queries)

- Discovered through the DID document service `#lessdugin_index`, type `LessduginIndex`, or through `forum.discovery.indexers`.
- Every hit carries uri and cid.
- Aggregates carry the `policy` they were computed with.
- All results are hints that clients can recompute.

```
index.defs
 #recordRef     { uri!: at-uri, cid!: cid, firstSeen: datetime, value: unknown, proof: bytes≤65536 (sync.getRecord CAR) }
 #tally         { subject!: at-uri, policy!: str, overall!: int, agreement: int, voters: int, reacts: [#reactTally] }
 #reactTally    { react!: str, for!: int, against: int, quote: defs#textQuote }
 #postView      { ref!: #recordRef, tally: #tally, state: kv{visible,hidden,pending}, section: str, tier: kv{personal,frontpage,curated},
                  tags: [str], comments: int, lastCommentAt: datetime, effectiveAt!: datetime, rank: str (decimal) }
 #commentView   { ref!: #recordRef, parent: at-uri, depth: int, tally: #tally, state: kv{visible,hidden,pending,deleted} }
 #repoView      { did!: did, roles: [kv{owner,admin,moderator,member,participant}], rev: tid, lastSeen: datetime }
 #notification  { reason!: kv{reply,mention,quote,react,vote,coauthorRequest,modAction,newPost,newComment,pingback}, ref!: #recordRef, subject: at-uri }

describeIndexer   () → { did, forums:[{uri, since, lagMs}], policies:[str], features:[str], version }
listForums        (q?, limit≤100, cursor?) → { forums:[{ref, members:int, posts:int}], cursor }
listForumRepos    (forum!, role?, limit≤1000, cursor?) → { repos:[#repoView], cursor }        ← the key low-trust primitive
listPosts         (forum!, sort kv{magic,top,new,old,recentComments}, section?, tag?, author?, kind?, minTier?, since?, until?,
                   hydrate?:bool, limit≤100, cursor?) → { posts:[#postView], cursor, policy:{scoring, ranking} }
getThread         (post!, sort?, depth?, hydrate?) → { post:#postView, comments:[#commentView], truncated:bool }
getTallies        (subjects![≤100]) → { tallies:[#tally] }
listBacklinks     (subject!, collection!:nsid, path?, limit≤1000, cursor?) → { records:[#recordRef], cursor }
getKarma          (forum!, actors![≤100]) → { karma:[{did, karma:int, policy}] }
getRecords        (uris![≤100], proof?:bool) → { records:[#recordRef] }   ← verifiable hydration via cached proof CARs
search            (q!, forum?, author?, kind?, collection?, limit≤100, cursor?) → { hits:[{ref, snippet}], cursor }
listNotifications (actor!, since?, limit≤100, cursor?) → { notifications:[#notification], cursor }   (public data only)
getWikiPage       (forum!, slug!) → { head:#recordRef, heads:[#recordRef], revisions:int, posts:int }
listModQueue      (forum!, limit, cursor?) → { items:[{ref, reason kv{pending, reported, spamSuspect}}], cursor }
```

- Proofs in `getRecords` are signed commits as of revision `rev`. They prove the record existed at that revision, not that it still exists. Clients show a "verified at rev" badge.
- Indexers must apply deletions.

---

## 10. Discovery and sync (what the data model enables)

**Forum directory.** Relay `listReposByCollection(.forum)` gives the owners; `listRecords(.forum)` on each gives their forums. No lessdugin server is involved.

**Approval mode.** This path needs no third-party service beyond the PDSes.

1. Fetch the forum record to get the owner and moderators.
2. List `.modAction` in each moderator repo, filtered by forum, to get the roster from `#member` and all moderation state.
3. Sync each member's content collections with `listRecords`.
4. Stay incremental with `getLatestCommit` per repo, or Jetstream `wantedDids` (≤10k) plus the `online.7778777.lessdugin.*` collection prefix.

**Open mode.**

1. Relay `listReposByCollection(.membership)` returns every lessdugin user across all forums.
2. For each, call `getRecord(did, .membership, sk(forum))`. That is one lookup per user, possible only because of deterministic keys.
3. Participants found only through Constellation (`online.7778777.lessdugin.comment:subject.uri`, `…post:forum`, `…membership:forum`, `…vote:root`), Jetstream or an indexer are admitted after they are re-fetched from their PDS.
4. This scales to roughly 10k ecosystem users. Beyond that, use an indexer or Constellation.

**Local-only by default:** drafts, read status, subscriptions, notification state, frontpage tag filters, bookmarks, karma overrides. Optional public bookmark sync uses `community.lexicon.bookmarks.bookmark`.

**Public, and the UI must say so:** votes, reacts, memberships, bans, mod actions, revisions. There is no protocol-level private data; atproto Spaces is alpha and only offers access control, not encryption.

---

## 11. Interop decisions

| Lexicon | Decision | Why |
|---|---|---|
| `com.atproto.repo.strongRef`, `com.atproto.label.defs#selfLabels` | Reuse | Standard |
| `app.bsky.actor.profile` | Read-only fallback for name and avatar, fetched from the PDS | Avoids duplicating profiles; no AppView dependency |
| `site.standard.publication` / `.document` | No default dual-publishing. P2 opt-in syndication for personal-blog posts, using the same rkey and `content` = our `markdown` object | standard.site assumes single-author publications; multi-author forums don't fit, and dual-publishing doubles rate cost and risks drift |
| `at.markpub.markdown` | Not adopted. Our `markdown` def deliberately looks similar (inline text plus text blob) | Still "taking feedback"; no media-scope (`cid:`) or profile semantics. Can be added later as a union ref (non-breaking) |
| `community.lexicon.calendar.event` / `.rsvp` / `location.*` | Reuse in P2, dual-published for events | Existing calendar apps then see our events |
| `community.lexicon.bookmarks.bookmark` | Optional public sync | Bookmarks are private by default |
| `community.lexicon.interaction.like` | Not used | Would double-count alongside votes |
| `sh.tangled.feed.comment`, `fyi.frontpage.*`, `xyz.atbbs.*`, `space.atbb.*` | Shape inspiration only (subject plus parent strongRefs); a possible importer later | Different semantics |
| Atproto labelers | P2 `policy.labelers` | Signed, verifiable spam and safety labels for each forum |
| Private messages | Out of scope; link to Bluesky chat (optional `rpc:chat.bsky.*`) | No E2EE in the protocol |

---

## 12. ForumMagnum feature mapping

| FM feature | lessdugin |
|---|---|
| Posts, link posts, word count | `post` (`kind`, `link`); derived from P |
| Q&A | `kind=question` plus `comment.answer` |
| Threaded comments, retract, deletedPublic, moderator hat, pinned | `comment` (`parent`, `retracted`, `moderatorHat`), `#hide{public}`, `#pin{thread}` |
| Quick takes | `kind=quickTake` |
| Revisions | `revision` (verifiable) |
| Co-authors with accept | `post.coauthors` plus `.coauthor` |
| Two-axis voting, strong votes, karma | `.vote{overall, agreement}`, karma/1 |
| Reacts, anti-reacts, inline reacts | `.react{against, quote}`, palette, gates |
| Side comments | `comment.quote` |
| Tags as wiki pages, relevance voting | `.wikiRevision` DAG plus `.tagVote` |
| Sequences (books: P2) | `.sequence` (`.collection`) |
| Frontpage, personal, curated, meta | `#tier`, sections |
| Review queue | approval mode plus `#member` |
| Bans, restrictions, nullify votes | `#ban`, `#restrict` |
| Author moderation, styles, guidelines | author-scope `.modAction` plus `#moderationPrefs` |
| Comment locks | `#lock` |
| Auto and manual rate limits | `policy.rateLimits`, `#rateLimit` |
| Reports | P2 `.report` (sealed) |
| Dialogues, polls, events | P2 `.turn`, `#poll` plus `.pollVote`, community calendar |
| Footnotes, LaTeX, table of contents, hover previews, pingbacks | LFM/1, heading ids, `markdown.links` |
| Subscriptions, notifications, read status, bookmarks, tag filters, recommendations | Local, or `index.listNotifications` |
| Search | Local FTS, or `index.search` |
| Crossposting, RSS import | `canonicalUrl` (P3: `crosspostOf`) |
| Out of scope | LLM features, collaborative real-time editing, jargon glossary, annual review |

---

## 13. Lexicon evolution rules

1. **Allowed changes:** new optional fields, new knownValues, new refs in open unions, new defs, new collections.
2. **Frozen:** types, field names, required sets, every length, grapheme, array and blob limit or accept list, rkey type. We have no closed unions to freeze.
3. **Breaking changes** need a new NSID, `<name>V2` (for example `online.7778777.lessdugin.postV2`). Clients read both; writers switch after a transition period; never dual-write.
4. **Open values only.** Every enumerated string uses knownValues. Each field has a documented fallback for unknown values:
   - kind: article
   - role: no capabilities
   - action: no effect
   - react: show the id
   - markdown profile: render as lfm/1 with a badge
   - policy: default policy plus a warning
5. **Behaviour ids are immutable:** `lfm/N`, `karma/N`, `magic/N`, `default/N`, `sk/1`. Any change gets a new id, and normative vectors live in `testdata/vectors/`.
6. **Unknown fields.** Readers ignore them; writers preserve them.
7. **Experiments** go only in `.temp.*`.

---

## 14. Changes to the lead's preliminary ideas

- **Revisions:** snapshot the **superseded** record verbatim, as a typed union, with a strongRef. That makes history verifiable by hash and covers mod-action deletions (mod log). It replaces a loosely typed snapshot of the new version.
- **Subject keys:** add collection domain separation and a fixed 80-bit, 16-character format, and make rkey verification on read **mandatory**.
- **Namespace:** flat group plus `index`, rather than many feature groups.
- **Roster:** use `modAction#member` instead of a separate roster collection, so one listing per moderator repo yields all moderation state.
- **Moderation:** state assertions with deterministic keys. Only current moderators count, with an explicit `adopts` mechanism for keeping a former moderator's actions.
- **Karma:** FM's power-at-vote-time cannot be reproduced trustlessly, so karma/1 is two-pass.
- **Time:** untrusted. Use the effective-time rule, hide future-dated records, and document rate limits as best-effort.
- **Tags:** identity is (forum, slug), not a record. This avoids squatting and ownership problems, and the wiki is a revision DAG.
- **Bodies:** comments never spill to a blob. External images are not loaded by default. Mentions display the verified handle.
- **Quick takes:** a post kind, not a container post.
- **Forum references** are at-uris, not strongRefs.
- **No float fields;** index ranks are strings.
- **standard.site:** no dual-publish by default.

---

## 15. Open risks

- **jacquard codegen** may not handle unions of record types (`revision.record`). The fallback is to generate the union by hand. Verify early.
- **Permission-set resolution:** the PDS must resolve `include:` from our TXT and publisher repo, and a failure blocks login. Ship the enumerated-scope fallback and test it against bsky.social before launch.
- **LFM/1 is tied to pulldown-cmark 0.13 behaviour.** Pin the version, and any upgrade must pass `testdata/vectors/lfm1` unchanged.
- **Open-mode serverless discovery** is O(ecosystem users). The relay's `listReposByCollection` is best-effort and may include stale DIDs.
- **Constellation paths for union fields** (for example `.body[...]`) are unverified. Only top-level `forum`, `subject.uri` and `root` are relied on.
- **Account age** for `#lock.allowAccountsCreatedBefore` is only available for did:plc, via the PLC audit log. did:web is treated as new.

### Critical Files for Implementation

These are new files to create; the repo currently holds only LICENSE.

- /home/user/lessdugin/lexicons/online/7778777/lessdugin/ (one JSON file per NSID above, including `markdown.json`, `defs.json`, `authFull.json`, `modAction.json`, and the `index/` subdirectory)
- /home/user/lessdugin/docs/spec/data-model.md (this document as the normative spec, including the fold and authority tables)
- /home/user/lessdugin/docs/spec/lfm-1.md (LFM/1 profile, projection P, quote anchoring)
- /home/user/lessdugin/crates/lessdugin-core/src/ids.rs (sk/1 derivation, canonical at-uri, slug and rkey validation, effective-time rule)
- /home/user/lessdugin/testdata/vectors/ (`sk.json`, `lfm1/*.json`, `karma1/*.json`, shared by Rust and TypeScript)

##### DESIGN DOC: SYNC #####
# lessdugin: discovery, sync, verification and caching design

## 0. Decisions

- **D1. Per-repo sync is a verified, complete MST range walk ("rangesync").** It uses `com.atproto.sync.getRecord` as an anchor proof plus `com.atproto.sync.getBlocks`, both public and CORS-enabled (see section 1). Every record we hold is checked against the author's signed commit. Every key range we sync is proven complete, so edits and deletes are detected exactly. Proof verification is the default sync path, not an add-on. `listRecords` is only a fallback.
- **D2. Record keys are laid out so related records sit next to each other in the MST.** Range queries can then select one forum, and optionally a time window, inside any participant's repo. Formats are in section 4. This is a hard requirement on the lexicon design.
- **D3. Anything not obtained by rangesync or by a proof is a *hint*.** All hints feed one dirty-repo queue, and verification settles them. Moderation inputs are never taken from hints: forum declaration, roster, bans and mod actions, and checkpoints.
- **D4. Discovery happens once, done by mods, and is published as roster pages in mod repos.** Each roster record holds up to 1,000 members. Open mode means self-declared membership, found through aggregators, then swept into the roster by mod clients. Readers who use no third parties still see every rostered member.
- **D5. Jetstream is used only to learn which repos changed (a "dirty" signal), never as authority.** It uses `time_us` cursors, the wildcard `online.7778777.lessdugin.*`, and DID filters of at most 10k sent via `options_update`. Without it, clients poll repo heads. Either way there is a daily reconciliation pass.
- **D6. Three source profiles: `strict`, `standard` (default) and `accelerated`.** The verification level is a separate setting: `proof` (default), `tls`, or `none`.
- **D7. Aggregates that need all-time data (karma, vote power) come from scorekeeper checkpoints (phase 2).** These are deterministic and anyone can audit them by recomputing. Without checkpoints, clients compute from what they have and badge the result `partial`.
- **D8. The cache is disposable.** Natively it is SQLite; in the browser it is IndexedDB; both use the same logical schema. The verified store is separate from a hint overlay, and local user state is separate from the cache.
- **D9. Blobs are fetched, checked against their CID, then shown via an object URL (browser) or a file (native).** Because of the CID check, any mirror or CDN can serve them without being trusted for integrity.
- **D10. The indexer is the same core engine running continuously, plus an XRPC read API.** It serves proof bundles, including range proofs, so content from it is as trustworthy as content from the PDS. What an indexer can still do (omit, delay, mis-rank) is caught by cross-checks and audits.
- **D11. In the browser, the wasm engine runs in a dedicated Web Worker.** Web Locks elect one tab as leader for sync; the other tabs read. The main thread is plain TS/React.
- **D12. Identity resolution lives in the Rust core on every platform.**
  - Native uses DNS plus HTTPS.
  - The browser uses DNS-over-HTTPS (DoH) plus `.well-known` where CORS allows, with flagged fallbacks.
  - Handle and DID are always checked in both directions.
  - PLC is trusted over TLS. `strict` mode verifies the PLC audit log (phase 2).

### Where I disagree with the preliminary ideas

1. **"Optional cryptographic verification via sync.getRecord."** I suggest making proofs the default, by syncing ranges instead of single records. `getBlocks` plus the anchor proof costs about the same as `listRecords`. It also gives exact detection of edits and deletes, completeness proofs, and lets any mirror serve content without being trusted. Per-record `sync.getRecord` remains the primitive for fetching one record or spot checks.
2. **Pure-hash deterministic rkeys (`base32(sha256(subject))`).** These destroy locality. Use `{fk}-{subjectTid}-{h16}` instead (section 4). It is still deterministic and idempotent, and it can be windowed by forum and by the subject's time.
3. **"Open mode: discovery via relay listReposByCollection."** That call returns every repo that has the collection, across *all* forums, so finding one forum's members costs O(whole lessdugin network). Use Constellation backlinks to the forum URI (exact), the indexer, and mod sweeps into the roster. The relay is only for the forum directory and as a fallback while the network is small.
4. **Per-member grant records in mod repos.** At 3 rate-limit points per create and 35k points/day, a mod can grant at most about 11k members per day. Reading costs 500 requests at 50k members. Roster pages (1,000 members per record, about 2 points per update) fix both problems.
5. **"Jetstream for live/incremental."** Jetstream is unsigned and lossy. It is a dirty signal and a source of provisional display, never authority.
6. **`getRepo?since=<rev>`.** Rejected for clients and indexer. It returns every block since `rev`, including all of the user's `app.bsky.*` activity. Interpreting deletes from it also requires the full previous MST. Use it only for `lessdugin export`.
7. **"Karma computed client-side."** All-time karma needs all-time votes, which is infeasible client-side beyond about 2,000 participants or for windowed clients. Clients with different windows would disagree on vote power and therefore on scores. Add checkpoints (D7).
8. **An indexer that returns only uri+cid.** It should also return proof bundles and range proofs. That makes its content trustless and removes the load amplification on author PDSes (section 12). It should also offer `getRepoHeads`, which cheaply tells a client which repos are dirty.
9. **Inline post bodies up to about 100KB.** Rangesync downloads every post body in the window for every participant. Cap inline bodies at 64KB; above that, store a `text/markdown` blob and keep an inline excerpt. The post record must always carry title, summary and word count, so lists never need bodies.
10. **Revision snapshots.** Make them self-verifying: `{post: at-uri, cid: <old cid>, record: <old record value verbatim>}`. The client re-encodes `record` and checks that it hashes to `cid`. Use the rkey `{postRkey}-{tid}` so all revisions of a post form one range. Fetch them lazily.

## 1. Facts I verified by live probes (2026-10-16)

- **PDS limits and CORS (`puffball.us-east.host.bsky.network`).**
  - Rate limit `ratelimit-policy: 3000;w=300` per IP, i.e. 10 requests/s. Responses carry `access-control-allow-origin: *` and expose the `RateLimit-*` headers.
  - It is unknown whether this limit is shared across Bluesky PDS hosts, so the scheduler adapts (section 9).
- **PDS sync endpoints.** `sync.getLatestCommit` returns about 100B (`{cid, rev}`). `sync.getRecord` proof CARs are about 3.5KB, *including proofs of non-existence* for our NSIDs in a repo that has none. `sync.getBlocks?cids=<commit>` returns a 244B CAR. All three are public, CORS `*`, and gzip-encoded. `describeRepo` works.
- **getBlob headers.** `content-security-policy: default-src 'none'; sandbox`, `content-disposition: attachment`, `cache-control: private`, CORS `*`. So `fetch()` → `Blob` → `createObjectURL` works.
- **Jetstream.** `wss://jetstream.us-east.bsky.network/subscribe?wantedCollections=<prefix>.*&cursor=<time_us>` works with a wildcard.
  - Events carry `time_us`, a seq `cursor`, `commit.{rev,operation,collection,rkey,cid,record}`.
  - `identity` and `account` events arrive even when filtering by collection (network-wide if there is no DID filter).
- **Relay.** `listReposByCollection` and `getLatestCommit` work, and send CORS `*` when an Origin header is present.
- **Identity endpoints.** Cloudflare and Google DoH JSON APIs are CORS `*`. `plc.directory` is CORS `*` with no rate-limit headers; an audit log is about 4KB. Slingshot `resolveMiniDoc` returns `{did, handle, pds, signing_key}`.
- **Constellation.** `getBacklinks?subject=&source=<collection>:<path>` works, CORS `*`, newest first.

## 2. Trust model

**Content verification level**, stored per record:
- `L2 proof`: commit signature verified with the DID's `#atproto` key, MST path checked, CID matches.
- `L1 tls`: fetched from the author's PDS (as resolved from the DID doc) without a proof.
- `L0 hint`: anything else.

**Identity level**, stored per DID:
- `plc-audit`: op chain verified.
- `plc` / `web`: doc obtained over TLS from the authority.
- `hint`: from Slingshot or an AppView.

A **verified** badge requires `L2` and an identity level of at least `plc`/`web`. A proof checked against a key that came from a hint is only `L0`, because Slingshot plus a fake PDS could forge it.

**Profiles** (defaults are the same in CLI, TUI and SPA):

| Source | strict | standard (default) | accelerated |
|---|---|---|---|
| Author PDSes, PLC, did:web | yes | yes | yes |
| DNS (native) / DoH (browser) | yes (browser DoH required for handle checks; otherwise handles show "unverified") | yes | yes |
| Bluesky Jetstream (live dirty signal) | no (polling only) | yes | yes |
| Relay `listReposByCollection` (forum directory) | no | yes | yes |
| Constellation (open-mode member and post discovery, backlinks to me) | no | yes | yes |
| Slingshot (fallback for identity and for PDSes without CORS) | no | fallback, badged L0 | yes |
| lessdugin indexers (forum-recommended or user-configured) | no | asks on first visit to forums above about 1,000 participants | yes |
| Media mirrors/CDN (CID-verified) | no | no | yes |

**Rules:**
- Hints may be *displayed* only when `show_unverified=true` (the default), marked `[hint]`, and are upgraded in place once verified.
- Never applied from hints: forum declaration, roster pages, bans and mod actions, checkpoints, and account deactivation or takedown (confirmed with `getRepoStatus` at the PDS first).
- A delete is applied only after the PDS proves absence.

**Badges** (CLI `--json` puts these in `verification`; TUI uses coloured glyphs; SPA uses icons with tooltips):
- `[verified]`: signed by `did`, at rev `X`, with the key source named.
- `[pds]`: L1.
- `[hint:<source>]`: not yet verified.
- `[stale]`: verified, but the repo is known to have a newer rev that has not been synced, or the last check is older than the freshness bound.
- `[partial k/n]`: on aggregates and lists, when not every relevant participant range has been proven complete.
- Contradicted or invalid-signature data is never shown (except `--debug`) and counts as a strike against its source.

## 3. Identity resolution

**Handle → DID**
- **Native:** query DNS TXT `_atproto.<handle>` (hickory-resolver, system configuration) and HTTPS `https://<handle>/.well-known/atproto-did` in parallel. If the two answers are both non-empty and disagree, the handle is invalid. Otherwise take whichever answered.
- **Browser:** query DoH (default `cloudflare-dns.com/dns-query` with `accept: application/dns-json`, alternate `dns.google/resolve`), plus `.well-known`, where a CORS failure counts as "no answer".
- If both fail, fall back to Slingshot or `public.api.bsky.app` `resolveHandle`, with identity level `hint`.

**DID → document**
- `did:plc`: from the configured PLC base (default `https://plc.directory`; a mirror list is supported).
- `did:web`: from `https://<host>/.well-known/did.json`. In the browser this needs CORS; otherwise use the Slingshot mini-doc as a hint.

**Bidirectional check.** A handle is shown only if `doc.alsoKnownAs` contains `at://<handle>` *and* the handle resolves back to the same DID. Otherwise the client shows a short DID with the claimed handle struck through. Stored URIs and links always use DIDs.

**PLC trust**
- Default: trust `plc.directory` over TLS.
- `strict` (phase 2): fetch `/log/audit`, check that `did = base32(sha256(genesis signed op))[:24]`, check each op's signature against the previous op's rotation keys (handling nullification), and cache the head op CID. Refresh by fetching `/log/last` and checking that it chains onto the cached head. Optionally compare the head against a second PLC source to detect equivocation.

**Cache durations and invalidation**
- Handle → DID: 1 hour.
- DID doc: 7 days, but refreshed immediately on any of:
  - a Jetstream `identity` event;
  - a signature failure (retry once with the fresh doc, then mark `bad-signature`);
  - PDS errors `RepoNotFound` / `RepoDeactivated` / `RepoTakendown` (refresh; if the PDS changed this is a migration and sync continues at the new PDS).

**Account status** comes from `getRepoStatus` and confirmed `account` events. Content from inactive accounts is hidden and kept for 7 days; a tombstoned DID is purged.

**PLC throughput** is the cold-start bottleneck (about 1 lookup per participant). Pace PLC at ≤20 requests/s with 8 in flight, adapting on 429. `accelerated` may take DID docs from the indexer as hints, but the `verified` badge waits for PLC confirmation in the background.

## 4. Key layout requirements (for the lexicon/data-model architect)

`fk` is the forum key: `base32sortable(sha256(forum at-uri))[:10]` (50 bits). It is a locality hint only; the forum field in the record body is authoritative. A mismatch is legal but inefficient, and the client warns. All forum-scoped collections use rkey type `any`, and the core validates the formats below.

| Record kind | rkey | Why |
|---|---|---|
| Forum declaration | `slug` | human-readable URI; `fk` is derived from it |
| Post, comment, quick take, mod action, report-ref | `{fk}-{tid}` | one range per forum; can be windowed by time |
| Per-(actor, subject) interaction (vote axes and reacts merged into *one* record per actor+subject) | `{fk}-{stid}-{h16}`, where `stid` = TID part of the subject rkey if it matches `{fk}-{tid}`, else `~` (sorts after every TID, so always inside the window); `h16` = `base32(sha256(subjectUri#discriminator))[:16]` | deterministic and idempotent; windowed by the *subject's* time, which is what scores of recent content need |
| Membership (self-declared, lives in the member's repo) | `{fk}` | one per forum; checking membership is a single proof request |
| Roster page (lives in a mod's repo) | `{fk}-{nnnn}` (zero-padded page number) | at most 1,000 members per page (about 60–70KB); one range per forum |
| Revision snapshot | `{postRkey}-{tid}` | all revisions of a post form one range |
| Checkpoint (lives in the scorekeeper's repo) | `{fk}-{epochTid}` | |
| Actor profile | `self` | |

**Other requirements:**
- Forum containment fields should be plain `at-uri` strings, not strongRefs, so that Constellation paths stay simple (e.g. `online.7778777.lessdugin.membership:forum`).
- Content subjects use strongRefs.
- The forum declaration must carry the fields sync needs:
  - `mods[{did, role}]`;
  - `membership: approval|open` and the open-mode admission policy (e.g. minimum account age);
  - `scorekeepers[did]`;
  - optional `indexers[{did, url}]` and `mediaMirrors[urlTemplate]`;
  - `algorithms{karma, rank}` (versioned identifiers);
  - `syncWindowDays` (a suggestion).
- Future-dated content (claimed time in the future by more than 5 minutes) is treated as *scheduled* and hidden until then. This removes any freshness-ranking advantage from future timestamps. Effective time = `min(createdAt, tid_time(rkey))`.

## 5. Forum resolution and participant discovery

**Resolution** for `lessdugin read alice.example/slug`, or an at-uri, or an SPA link:

1. Resolve the handle to a DID (both directions), then the DID doc, to get the PDS and key.
2. Fetch the forum record with `sync.getRecord` and verify it. One request of about 3.5KB is enough for a verified first paint.
3. In parallel, resolve the mods and run a full rangesync of the `fk` ranges in each mod's repo (roster pages, bans, mod actions, checkpoints). Mod repos are always synced without a window, at high priority.
4. Participant set = owner ∪ mods ∪ (all rostered members − banned), plus in `standard`/`accelerated` and open mode: self-declared members found via
   - Constellation `getBacklinks(subject=forumUri, source=online.7778777.lessdugin.membership:forum)`,
   - the Jetstream membership stream and 36-hour replay,
   - the indexer `listParticipants`.
5. For a fast first paint in `standard`:
   - Constellation backlinks from posts to the forum (newest 100) plus Jetstream 36-hour replay name the recently active authors and posts.
   - Point-fetch those posts with proofs (about 100 requests).
   - Render with `[partial]` scores while the full sync runs.
6. Sync order:
   1. mods;
   2. authors visible on screen;
   3. recently active authors (from hints);
   4. rostered members, newest grant first;
   5. pending (not yet rostered) members, within budget.

**Membership modes**
- **Approval:** only rostered DIDs count. A member's `membership` record is just an application, which mods see through discovery.
- **Open:** rostered DIDs, plus discovered self-declared members (badged "new member") who satisfy the declared admission policy and are not banned.

**Mod sweep** (`lessdugin mod sweep <forum>`; TUI/SPA run it automatically for mods when the last sweep is over 1 hour old):
- Discover self-declared members, verify each membership record by proof, apply the policy, and append them to the mod's last non-full roster page with `putRecord` + `swapRecord`.
- This is how readers using no third parties learn about everyone, with a delay equal to the sweep interval.
- If a mod is removed from the declaration, the owner's sweep copies their roster entries into the owner's pages.

**Spam and Sybil budgets.** Discovery results are attacker-controlled, so:
- Pending members are synced lazily: posts and comments only, at most 500 pending repos per forum, highest priority to those a rostered member has interacted with.
- Interaction ranges are never synced for zero-karma or pending members, because their votes carry no power.
- Per-repo caps: 20k records per forum range, 50MB, MST depth ≤ 32, at most 100k nodes walked.

**Forum directory:** relay `listReposByCollection(online.7778777.lessdugin.forum)` gives the owners, then their forum ranges are point-fetched. The indexer offers `listForums`.

## 6. Per-repo sync: rangesync

Ranges are `KeyRange{lo, hi}` over the string `collection/rkey`.
- One whole forum in one collection: `[c/{fk}-, c/{fk}.)` (`.` is the next byte after `-`).
- A time window: `lo = c/{fk}-{tid(now − window)}`.
- Singleton records are exact single keys.
- Own repo: the entire namespace `[online.7778777.lessdugin., online.7778777.lessdugin/)`.
- Default window: 365 days native, 90 days browser, `--window all` for everything. Mods, roster, checkpoints and own repo are never windowed.

```
rangesync(did, ranges) -> Unchanged | Applied{rev, upserts, tombstones} | Fallback
  head = getLatestCommit(did)                      // ~100B; skipped for "hot" repos (head changed in ≥3 of last 5 checks)
  if head.cid == repo.commit_cid && ranges ⊆ repo.complete_ranges -> Unchanged
  car = sync.getRecord(did, anchor = lowest key of ranges)   // commit + root→leaf path, ~3.5KB, one round trip
  commit = verify_commit(car.root, key(did))       // DAG-CBOR, version 3, did match, low-S ECDSA (k256|p256) over unsigned commit
  require commit.rev >= repo.rev                   // otherwise: Rollback, keep cache, retry with backoff, flag after 24h
  frontier = {commit.data}; avail = car.blocks ∪ node_cache(did)
  loop by level: need = frontier − avail; getBlocks(did, need) in batches of 50 CIDs, in parallel
       for each node: sha256 == cid; decode; rebuild keys; check sort order + layer = lz2(sha256(key))
       collect leaves in ranges; push children whose key interval intersects ranges
  diff leaves against stored records in ranges -> new/changed CIDs, removed keys
  fetch missing record blocks (skip CIDs already present from hints or old versions) via getBlocks
  verify cid, decode, lexicon-validate -> one atomic store txn:
       upserts (level=L2, as_of_rev=commit.rev), tombstones, repo{commit_cid, rev, complete_ranges},
       node cache (keep only nodes reachable in this walk)
```

**Properties**
- Cost is a few sequential round trips, roughly the MST depth of the region holding our ranges (about 4–7). Many repos run in parallel.
- When a user's only activity was on Bluesky, a re-check costs 2 requests (head + anchor). The path to our ranges splits from `app.bsky.*` near the root, so the cached child CIDs show nothing changed.
- Edits and deletes are detected exactly.
- `BlockNotFound` during a walk means a concurrent lessdugin write. Restart from the new head, up to 3 times, then use the fallback.

**Single-record fetch** (dangling strongRefs, or posts older than the window): `sync.getRecord`, which proves either existence or absence.

**Fallback (L1).** Used when the PDS lacks `getBlocks`/`sync.getRecord` or CORS, or after 3 restarts:
- `getLatestCommit`, then `describeRepo` to learn the collections, then `listRecords` for each relevant collection.
- Pages of 100, newest first, stopping at the window's lower rkey bound.
- Recompute each CID from JSON→DAG-CBOR and compare with the returned `cid`; a mismatch means the record is dropped.
- Deletes are inferred by a full re-list of the range whenever the head changes.

**Own writes:** apply the `applyWrites` result (uri, cid per op, commit cid/rev) optimistically as L1, then mark the repo dirty so rangesync confirms it.

**Migration:** cached nodes are content-addressed and remain valid. Sync continues at the new PDS from the DID doc, and revs continue increasing.

## 7. Live and incremental updates

**Every source of change feeds one dirty-repo queue:**
- Jetstream events;
- the indexer change feed;
- the poll schedule;
- the user's own writes;
- discovery results;
- the user opening a view.

Each queue item carries `(did, min_rev?, priority, reason)` and is coalesced per repo. Debounce by priority:

| Priority | Content | Debounce / jitter |
|---|---|---|
| P0 | content currently on screen | 0–5 s |
| P1 | posts and comments | 2–30 s |
| P2 | interactions | 30–600 s |
| P3 | audit and reconciliation | as scheduled |

Jitter spreads the load when many readers verify the same write.

**Jetstream**
- Connect to `wss://jetstream.{us-east,us-west}.bsky.network/subscribe?requireHello=true&cursor=<last_time_us − 5s>`, using `compress=true` (zstd with dictionary) natively and no compression in the browser.
- Then send `{"type":"options_update","payload":{"wantedCollections":["online.7778777.lessdugin.*"],"wantedDids":[≤10k], "maxMessageSizeBytes":0}}`. A DID list does not fit in a URL.
- When the union of DIDs across followed forums is over 10k, drop the DID filter and filter client-side.
- When a DID filter is active and an open-mode forum is followed, a second connection with `wantedCollections=online.7778777.lessdugin.membership` (no DIDs) catches newcomers, who are then added via `options_update`.

**Events and cursors**
- `commit` create/update becomes a `RecordExists` hint carrying the JSON record. It is canonicalized to DAG-CBOR and must hash to `commit.cid`, otherwise it is dropped. Once rangesync sees the same CID it upgrades to L2 with no re-download.
- `delete` → `RecordDeleted` hint. `identity` → invalidate the identity cache. `account` → status hint, confirmed with `getRepoStatus`.
- Cursors are `time_us`, which are portable across instances; the instance-specific seq `cursor` is never used. Persist every 5 s or 1,000 events. On reconnect, rewind 5 s and dedupe with an LRU of `(did, rev, collection, rkey, op)`.
- Reconnect backoff: 1 s doubling to a 60 s cap, with jitter. Fail over to the other endpoint after 3 failures.
- If the gap exceeds the 36-hour replay window, mark every participant "needs head check".

**Polling** (`strict` mode, or Jetstream down):
- Interval by repo class, ±20% jitter:

| Repo class | Interval |
|---|---|
| mods and owner, while the forum is open | 2 min |
| hot (our range changed within 1 day) | 10 min |
| warm (within 30 days) | 1 h |
| cold | 12 h |

- Opening a view triggers a P0 head check for authors on screen when their last check is over 5 minutes old.

**Reconciliation** (always runs, even with Jetstream):
- A head check of every participant once per 24 hours, spread evenly; mods hourly.
- This bounds what lossy Jetstream, an omitting indexer, or out-of-band writers can hide to under 24 hours.

## 8. Accelerators and hint handling

| Hint type | Comes from | How it is settled |
|---|---|---|
| `RepoDirty(did, min_rev)` | Jetstream, indexer `getChanges`/`getRepoHeads` | rangesync. If the PDS rev is still below `min_rev` after 2 attempts, the hint is "unconfirmed" and the source gets a strike. |
| `RecordExists(uri, cid, rev, value?)` | Jetstream, indexer, Constellation (no CID), Slingshot | rangesync or proof. A different CID or absence at rev ≥ the hint's rev is a contradiction. |
| `RecordDeleted(uri, rev)` | Jetstream, indexer | absence proof before anything is hidden |
| `Participant(forum, did)` | Constellation, relay, Jetstream, indexer | proof of the `membership/{fk}` key, then admission policy |
| `Backlink(subject → uri)` | Constellation, indexer | point-fetch the linking record |
| `Aggregate(forum, key, value, algo, digest)` | indexer | cross-checked (section 13); never treated as verified |
| `Identity(did, doc)` | Slingshot, AppView, indexer | PLC or did:web fetch |

- `source_stats` counts hints, confirmations, contradictions and omissions for each source. A source above 2% contradictions or 3 omissions per day is demoted (its hints are ignored) and a warning is shown.
- Hints are held in the hint overlay and never written into the verified `record` store (invariant I5).

**Checkpoints (phase 2).**
- The scorekeeper publishes `forum.checkpoint` `{forum, algo, cutoff, setDigest, recordCount, karma: blob (application/cbor map did→int), prev}`.
- Vote power for votes cast during epoch *e* uses checkpoint *e−1*. Scores of windowed content are then computable from votes on that content plus one blob, and all clients agree.
- The checkpoint is published by a mod CLI/TUI/SPA when the latest one is older than the epoch (default weekly), or by an indexer the mods designate as scorekeeper.
- Anyone with a full sync or an indexer can recompute it. A mismatch produces a public warning.

## 9. Scheduler, budgets, back-off, limits

**Concurrency**

| Limit | Native | Browser |
|---|---|---|
| Requests in flight, global | 32 | 16 |
| Per host | 6 | 6 |
| PLC | 8 | 8 |
| DoH | 8 | 8 |

**Per-host rate**
- Token bucket of 8 requests/s with a burst of 20.
- Adapted from `RateLimit-Remaining/Reset`: background work uses at most 50% of the remaining budget, P0 up to 80%.
- *Rate-limit domains* are learned: if hosts' remaining counters fall in lockstep, they are grouped and treated as one. Initially all `*.host.bsky.network` hosts are treated as a single domain with a 10 requests/s cap, which loosens if observation shows separate limiters.

**Retries and back-off**
- GETs retry 3 times with full-jitter exponential back-off (250 ms base, 8 s cap) on network errors, 5xx and 429, honouring `Retry-After`/`RateLimit-Reset`.
- A repo that still fails gets `next_check = now + min(30s·2^n, 24h)` with jitter.
- Per-host circuit breaker: trips at 10 consecutive failures, or 50% of the last 20 requests. It stays open 60 s, then lets one half-open probe through, and doubles up to 1 h.

**Timeouts and response caps**
- Connect 5 s; request 20 s; CAR responses 60 s.
- Caps: proof 1MB, `getBlocks` 8MB, record 1MB, blobs per policy.
- Cross-origin GETs from the browser send no custom headers, so there are no CORS preflights. Native requests send `User-Agent: lessdugin/<ver> (+https://lessdugin.7778777.online)`.

**Budgets visible to the user**
- The SPA estimates cold-sync cost from roster size (section 12) and above about 1,500 requests offers `accelerated` or a smaller window.
- CLI flags: `--max-requests`, `--time-budget`, `--window`, `--offline`.
- With `navigator.connection.saveData` set: 30-day window, no media prefetch.

## 10. Client cache

**Separate storage areas:**
- **cache** (disposable): `$XDG_CACHE_HOME/lessdugin/cache.sqlite`, or IndexedDB `lessdugin-cache-v{N}`.
- **state** (never garbage-collected; holds drafts, read marks, subscriptions, settings, OAuth sessions): `$XDG_DATA_HOME/lessdugin/state.sqlite`, or IndexedDB `lessdugin-state`.
- **config**: `$XDG_CONFIG_HOME/lessdugin/config.toml`.
- **secrets**: OS keyring, otherwise a 0600 file.

```sql
-- cache.sqlite: WAL, PRAGMA user_version = schema; a mismatch drops the file
identity(did PK, handle, handle_state /*verified|unverified|invalid*/, handle_checked_at, pds, signing_key,
         doc BLOB, id_level /*plc-audit|plc|web|hint*/, plc_head, status, fetched_at, expires_at)
handle(handle PK, did, method /*dns|https|doh|hint*/, resolved_at, expires_at)
repo(did PK, commit_cid, rev, complete_ranges BLOB /*[{lo,hi,commit,rev}]*/, last_change_rev,
     last_checked_at, next_check_at, class, fail_count, backoff_until, last_error, rollback_flag)
mst_node(did, cid, bytes BLOB, PRIMARY KEY(did,cid))          -- only nodes reachable in the last walk
record(uri PK, did, collection, rkey, cid, bytes BLOB /*canonical DAG-CBOR*/, forum, subject, root, parent,
       eff_time, as_of_rev, level /*1|2*/, source, valid, first_seen_at, last_confirmed_at)
  INDEX(forum, collection, eff_time); INDEX(subject, collection); INDEX(did, collection)
tombstone(uri PK, rev, observed_at)
record_version(cid PK, uri, bytes, as_of_rev)                   -- old versions still referenced, verified revisions
hint(id PK, kind, did, uri, cid, rev, source, received_at, payload BLOB)   INDEX(uri), INDEX(did)
blob(cid PK, did, mime, size, path /*native*/, fetched_at, last_access, pinned)
cursor(source PK, value, updated_at)                            -- jetstream time_us, indexer seq per indexer DID
forum_member(forum, did, status /*rostered|pending|banned*/, via, since, PRIMARY KEY(forum,did))
derived(key PK, algo, input_digest, value BLOB, computed_at)    -- memoized threads/scores/karma
source_stats(source PK, hints, confirmed, contradicted, omissions, last_seen)
record_fts USING fts5(uri UNINDEXED, title, text)               -- native only
```

**IndexedDB layout:**
- One object store per table, with the same key paths and compound indexes.
- Blob bytes are kept in a separate `blob_data` store (or Cache Storage).
- Each repo apply is one `readwrite` transaction across all affected stores.
- Search uses an in-memory inverted index built by the core per forum.
- The SPA calls `navigator.storage.persist()` and watches `storage.estimate()`.

**Invariants**

| # | Invariant |
|---|---|
| I1 | Deleting the cache loses only time. |
| I2 | Every stored record, node and blob hashes to its CID; checked on write; bytes are immutable. |
| I3 | Per URI, the current version only moves forward in `as_of_rev` (TID order). The same rev with a different CID is a conflict: keep the higher level and flag the source. |
| I4 | A tombstone at rev R suppresses any observation with rev < R. It is kept for at least 30 days, or until a range is proven complete at a rev greater than R. |
| I5 | `record` holds only rows at L ≥ L1 (L ≥ L2 when `verify=proof`, except L1 fallback repos, which are marked). Hints live only in `hint`. Views overlay hints when `show_unverified` is on. Level only ever goes up for the same (uri, cid). |
| I6 | Claims of completeness ("all comments", scores) require every relevant participant range to be in `complete_ranges` at rev ≥ the last known dirty rev. Otherwise the view is `[partial k/n]`. |
| I7 | Moderation inputs are read only from `record` rows at the configured minimum level (default L2). |
| I8 | Records of non-active DIDs are excluded from views. |
| I9 | `derived` rows are pure functions of `(input_digest, algo)` and are never a source of truth. |
| I10 | One sync writer per cache: a native `cache.lock` file lock or the browser Web Lock `lessdugin-sync-leader`. Other processes or tabs read the cache and do only targeted P0 fetches. The leader broadcasts changed URIs (BroadcastChannel in the browser). |

**Garbage collection**
- Evict whole forums, least recently viewed first, when over budget (default 2GB native, 500MB browser).
- Never evict identities, the user's own repo, or mod repos of followed forums.
- Prune MST nodes on every repo sync. Blobs have their own LRU (1GB native, 200MB browser). Expire tombstones per I4.

## 11. Blobs and media

**Fetching and verification**
- Fetch `sync.getBlob?did&cid` from the author's PDS. The URL is never placed directly in an `<img>` tag (the PDS sends `sandbox`, `attachment` and `private` headers).
- Hash the bytes (sha256 via WebCrypto or Rust) and compare with the raw-codec CID. Only then:
  - browser: `URL.createObjectURL`, revoked when the element unmounts;
  - native: write `blobs/<cid[0:2]>/<cid>`.
- On a mismatch, discard the bytes and give the source a strike.

**What may be rendered**
- The type comes from the declared mime in the verified record, checked against an allow-list, never from the response header.
- Inline: png, jpeg, gif, webp, avif, plus svg via `<img>` only (scripts do not run in image context).
- mp4/webm/audio: click to load. MVP downloads the whole file and then verifies. A Service Worker that streams range requests and verifies on completion, badged until complete, is phase 2.

**Limits and decoding**
- Images up to 5MB load automatically; anything larger is click-to-load.
- Native decoding uses the `image` crate with limits (16k×16k, 256MB allocation), then ratatui-image.
- CLI: `lessdugin blob get <uri> --index N -o file`, or `open`.

**Thumbnails are author-generated.** The authoring client uploads a WebP no wider than 640px alongside the original, and the record references both. Proxy-generated thumbnails cannot be verified, so they are not used.

**Mirrors and CDN.** URL templates `https://cdn.example/{did}/{cid}`, declared by the forum or the user. Integrity needs no trust because of the CID check. On failure, fall back to the PDS and give the mirror a strike.

**Privacy.** Records are synced per range, so the PDS cannot tell *which* post you read. Media fetched when viewed does reveal that. Defaults:
- Native prefetches thumbnails for the whole window (reading stays private).
- The browser fetches lazily.
- The "private reading" setting prefetches all thumbnails and routes requests through the configured proxy.

## 12. Cost model and when an indexer is needed

**Assumptions**, per participant per year:
- 1.5 posts (8KB each), 15 comments (0.8KB), 60 interactions (0.25KB), 2 other records.
- That is about 80 records and about 40KB of record bytes.
- MST interior about 140B per record; boundary nodes about 15KB per repo.
- About 65KB raw, about 50KB on the wire, per participant.

**Cold requests per participant:** about 7. That is 1 PLC lookup + 1 DNS/DoH query + 1 anchor + about 3 level batches + about 1 record batch. There are about 7 sequential round trips per repo, run in parallel across repos.

Estimates are ±2×, `standard` profile, 365-day window:

| Participants | Cold requests | Cold wire / storage | Cold time, native (32 in flight, ~120 ms) | Cold time, browser (16, ~150 ms) | Warm reopen ≤36 h (≈10% repos dirty × 3 req) | Offline >36 h | Live, per client |
|---|---|---|---|---|---|---|---|
| 50 | ~370 | 2.5MB / 3.5MB | ~2 s | ~4 s | ~15 req | ~65 | negligible |
| 500 | ~3.5k | 25MB / 35MB | ~15 s | ~35 s | ~150 | ~650 | ~330 req/day |
| 5,000 | ~35k | 250MB / 350MB | ~2.5 min (PLC pacing up to ~4) | ~6 min (too slow) | ~1.5k | ~6.5k | ~3.3k req/day |
| 50,000 | ~350k | 2.5GB / 3.5GB | 25–40 min | not feasible | ~15k | ~65k | ~33k req/day (0.4/s) |

**Other effects**
- A 90-day window cuts record bytes by about 4×, but not the per-repo fixed cost (about 4–5 requests and 20KB).
- Pending-member and zero-karma rules cap spam overhead.

**Amplification at author PDSes.** Each write causes about `R_online × 3` requests to the author's PDS within the jitter window. With 500 concurrent readers, one comment means 1,500 requests at a possibly self-hosted PDS. This is the main operational reason for an indexer, which serves proofs and so absorbs that load with no loss of trust.

**Thresholds (decision):**
- **Up to 1,000 participants:** no indexer needed anywhere.
- **1,000–10,000:** native is fine (persistent cache plus Jetstream). Forums should recommend an indexer for SPA first visits, full-history search and offline notifications. Without one, use scorekeeper checkpoints for karma.
- **10,000 or more participants, or 300 or more concurrent readers, or over 1M records, or over 10k DIDs (beyond Jetstream's DID filter):**
  - The forum should declare at least two independent indexers.
  - Clients default to `accelerated` for cold start and use lazy per-view bundles.
  - `strict`/direct mode stays available for audit and backup; its warm path is fine.

**Accelerated cold start (any size):**
- Forum resolution: 3 requests.
- `listPosts`: 1 request; titles render with hint badges in about 300 ms.
- `getRecords?proof` for the 30 visible posts: 1 request, about 120KB.
- Then Jetstream, and the direct rangesync of mod repos.
- First meaningful paint in about 1 s. Opening a thread is one bundle request (200 comments from 80 repos ≈ 400KB raw / 300KB gzip with proofs).

## 13. Optional indexer (`crates/lessdugin-indexer`, binary `lessdugin-indexer`)

**Code reuse.** The indexer is the `lessdugin-core` engine with the native transport and the native SQLite store from `lessdugin-native`, set to index every forum and never sleep. It uses the same core modules as clients: rangesync, verify, moderation fold, thread, score, rank, karma, search tokenizer. Its numbers are therefore bit-identical to what any client computes from the same record set and algorithm version.

**Ingest**
1. **Discovery:**
   - every 6 hours, relay `listReposByCollection` for each NSID (several relays, union of results);
   - forum rosters;
   - Jetstream (wildcard, no DID filter, two endpoints, `time_us` cursors, 10 s rewind on restart);
   - optionally Constellation.
2. **Changes:** every Jetstream commit marks its repo dirty, then core rangesync runs (debounced 1–3 s). It is polite to PDSes: at most 5 requests/s and 4 in flight per host, 200 in flight globally. Stale DIDs are dropped by absence proofs.
3. **Reconciliation:** a PDS `getLatestCommit` for every repo daily. Identity and account events trigger DID doc refresh, native DNS handle checks, and `getRepoStatus`.
4. **Retained state:** for every repo, the commit block and every MST node intersecting our namespace. This lets the indexer **construct existence, absence and range proofs locally**, with no request to the PDS per query.

**Storage**
- SQLite (WAL, rusqlite bundled with FTS5) using the client-cache tables plus:
  - `change_log(seq PK, forum, uri, cid|null, did, rev, op, at)` (kept 30 days);
  - `post_stats(forum, uri, algo, score, base_score, comment_count, last_comment_at)`;
  - `karma(forum, did, algo, value)`;
  - `backlink(subject, collection, path, uri)`;
  - `set_bucket(forum, bucket, digest)`;
  - `fts`.
- Rationale: one binary, no operations burden, comfortable to around 50M records. A Postgres adapter behind the `Store` trait can come later.
- At 50k participants: about 4M records per year, about 5GB per year. Ingest is about 33k PDS requests/day. A 2-vCPU VPS is enough.

**XRPC surface**
- All endpoints are unauthenticated GET queries, CORS `*`.
- NSIDs `online.7778777.lessdugin.index.*`. This needs its own DNS TXT record, `_lexicon.index.lessdugin.7778777.online`.
- Every response carries `asOf{seq, time_us, lag_s}` and `indexer{did, coreVersion}`.
- `format=car` returns a bundle.

| Method | Parameters | Returns |
|---|---|---|
| `describe` | – | DID, versions, algorithms, forums, Jetstream lag, backfill status |
| `listForums` | cursor | forum refs |
| `listParticipants` | forum, status, cursor | DIDs with status (rostered/pending) |
| `getRepoHeads` | dids[≤200] | `{did, commit, rev, pds, syncedAt, status}`: batch dirty check |
| `getChanges` | forum?, since=seq, limit≤1000 | `{seq, uri, cid?, did, rev, op}` (`CursorTooOld` → client re-bootstraps) |
| `listPosts` | forum, sort (magic, top, topAdjusted, recentComments, new, old), tag?, period?, algo, cursor | refs + score fields + `setDigest` |
| `getThread` | uri, sort | post, comment and interaction refs; `format=car` gives the proof bundle |
| `getRecords` | uris[≤100], proof | values (hint) or proof bundle |
| `getRepoRanges` | dids[≤100], forum, window | range-proof bundle (completeness proven at the indexer's commit) |
| `getBacklinks` | subject, collection?, path?, cursor | refs |
| `searchPosts` | forum?, q, cursor | refs + snippet (snippet is a hint) |
| `getActivity` | did, since, forums[], subjects[] | replies, mentions, karma deltas (all public data; stateless) |
| `getKarma` | forum, dids[], algo | values + checkpoint ref + digest |
| `getSetDigest` / `listBucket` | forum, algo / bucket | 256 bucket digests, sha256 of sorted `uri\0cid\n` lines / bucket members |

**Bundle format `lessdugin.bundle/1`**
- A CAR v1 whose root is a DAG-CBOR manifest: `{v, generatedAt, commits:[{did, cid}], records:[{uri, cid, commit}], absent:[{uri, commit}], ranges:[{did, commit, lo, hi}], meta}`.
- It contains commit blocks, deduplicated MST nodes, and record blocks.
- Verification is one core function, `verify_bundle(car, resolver) → Vec<Verified>`. It accepts partial bundles: items that fail are dropped and the source gets a strike.
- The same format serves static snapshot segments (phase 2): monthly immutable segments plus a head segment, hostable anywhere or as blobs.

**Responses and limits:** ETag from `(params, seq)`; `Cache-Control: public, max-age=5` for lists; bundles keyed by commit are `immutable`. Per-IP limit of 20 requests/s.

**Identity and discovery of the indexer:** `did:web`, with a service `#lessdugin_index` in the DID doc, and an optional declaration record in the operator's repo. Clients reach indexers through the forum declaration's `indexers` field or user configuration.

**How clients cross-check indexers**
- **Content:** a valid proof is required for anything shown as verified, so an indexer cannot forge content.
- **Freshness:** a proof's `rev` is compared with a direct PDS `getLatestCommit` (cheap, and reveals only that this repo is being synced) for a random 1% of participants per hour, and through the daily reconciliation. Deleted content an indexer keeps serving therefore lasts at most about 24 hours.
- **Omission:**
  - Jetstream stays on in `accelerated` as an independent stream. A verified Jetstream event missing from `getChanges` after 2 minutes is a strike.
  - With two or more indexers: union their list results; compare top-N `(uri, cid)` sets for 20% of queries, allowing a 2-minute settle window; compare `getSetDigest` hourly. For mismatched buckets, call `listBucket` on each, diff, and rangesync the disputed repos at the PDS, which is the ground truth.
- **Aggregates:** equal set digests with unequal karma or scores (same `algo`) means the indexer is buggy or lying, and it is flagged. Unequal digests are reconciled first. Lists are re-ranked locally over the union when score sources disagree beyond a tolerance; then the scores are badged `[partial]`.
- **Demotion:** above the strike thresholds an indexer is demoted; the primary role passes to the healthiest indexer and the UI shows "indexer X omitted N verified items in the last 24h".

## 14. Privacy

| Party | What it learns | Mitigation |
|---|---|---|
| Author's PDS operator (may be the author) | Reader IP, which repos you sync and when; media you open | Range sync hides which posts you read; thumbnail prefetch; `network.proxy = "socks5h://…"` (Tor) natively; `accelerated` mode moves this exposure to the indexer |
| PLC directory | DIDs you resolve | 7-day cache; mirrors |
| DNS resolver or DoH provider (browser) | Handles you check | Native uses system DNS; DoH provider is configurable |
| Jetstream operator (Bluesky) | IP, DID filter (roughly which forums you read), online times | `strict` disables it; wildcard without a DID filter hides which forum, at more bandwidth cost |
| Relay, Constellation | Interest in lessdugin / the forum URI | Used only for forum-level queries, not per post |
| Indexer | Everything you browse; your DID if you call `getActivity` | Self-host; `standard` profile; cross-checks do not help privacy |
| Static SPA host | IP, and that you use lessdugin | `lessdugin web` serves the SPA on 127.0.0.1 |
| Everyone | Votes, reacts, memberships and public bookmarks are public repo data | UI states this when voting. Read marks, drafts and subscriptions stay local. |

## 15. Code layout and interfaces

- **`lessdugin-core`** (wasm-safe; a `MaybeSend` bound is cfg-gated: `Send` natively, `?Send` on wasm):
  - `identity/` (resolver, PLC, did:web, DoH-over-Transport, bidirectional check);
  - `verify/` (`car.rs` with our own minimal CAR v1 parser, `commit.rs` using k256/p256/sha2, `mst.rs` for range walk and proofs, `cid.rs`, `bundle.rs`);
  - `sync/` (`rangesync.rs`, `fallback.rs`, `scheduler.rs` with priority queue, host buckets, breakers and rate-limit domains, `dirty.rs`, `jetstream.rs` for event parsing and cursor logic, `discovery.rs`, `sources/{relay,constellation,slingshot,indexer}.rs`, `reconcile.rs`);
  - `store/` (`Store` trait, batch-oriented: `get_records`, `apply(WriteBatch)`, `scan_forum`, `scan_subject`, `get_repo`, `nodes`, `hints`, `cursors`; plus `invariants.rs` tests run against every implementation);
  - `blob.rs`;
  - `engine.rs`, exposing `open_forum`, `sync`, `subscribe_live`, `query_posts`, `get_thread`, `verify(uri) → VerificationReport`, `fetch_blob`, `stats`, and events `Changed{uris}`, `Progress`, `SourceWarning`.
- **Traits** the core needs from each platform: `Transport` (GET/POST with host key), `EventSource` (websocket), `Clock` (now, sleep), `DnsTxt`, `Store`.
- **I own the MST range walk and proof code** (about 1–1.5k lines, the most security-critical code). `jacquard-repo` and `@atproto/repo` are used as test oracles, not dependencies on this path. Jacquard is still used for lexicon types and native OAuth.
- **`lessdugin-native`:** reqwest (rustls, HTTP/2, gzip), hickory-resolver, tokio-tungstenite plus zstd (Jetstream dictionary), rusqlite store, `cache.lock`.
- **`lessdugin-wasm`:** web-sys fetch and WebSocket, `indexed_db_futures` store, running in a Worker. TypeScript does no identity resolution or verification. `@atcute/oauth-browser-client` is used only for login and authenticated writes.
- **`lessdugin web`** (phase 2 option): a token-protected "engine bridge" on 127.0.0.1, so the SPA can share the native cache, DNS and proxy.

## 16. Tests and phases

**Tests**
- Property tests of rangesync: random MSTs including our prefixes, random edits and deletes, windows, and adversarial servers (wrong CIDs, bad signatures, rolled-back revs, a missing block mid-walk, oversized depth). The local mirror must equal ground truth, and range completeness must hold.
- An in-process fake PDS (getLatestCommit, getRecord, getBlocks, listRecords) with fault injection: 429, 5xx, missing CORS.
- A load simulation of 5,000 repos that asserts request and byte budgets in CI, keeping the cost table honest.
- End-to-end on `@atproto/dev-env` TestNetworkNoAppView, with a test-only "mini-Jetstream" that converts the PDS's `subscribeRepos` firehose into Jetstream JSON.
- Shared bundle and proof vectors under `testdata/vectors`.
- Playwright: first-visit SPA sync, multi-tab leader failover.
- `lessdugin doctor --pds <url>`: probes CORS, getBlocks, sync.getRecord and rate-limit headers.

**MVP**
- Identity on native and browser (PLC over TLS).
- Rangesync, plus the `listRecords` fallback and single-record fetch.
- Roster pages and mod sweep.
- Open-mode discovery via Constellation and the Jetstream membership stream.
- Jetstream live updates and the dirty queue.
- Scheduler with back-off; both caches; blob verification.
- Badges and the three profiles.
- Indexer with `describe`, `listParticipants`, `getRepoHeads`, `getChanges`, `listPosts`, `getThread`, `getRecords(proof)`, `getRepoRanges`, `getBacklinks`, `searchPosts`, `getActivity`.
- Cross-checks: Jetstream against the change feed, PDS audit sampling, and multi-indexer union.

**Phase 2**
- Checkpoints and `getKarma`, set digests.
- Static snapshot bundles.
- PLC audit verification for `strict`.
- Service Worker media streaming.
- The engine bridge.
- Media mirrors.

### Critical Files for Implementation (all new; the repo currently contains only `LICENSE`)
- /home/user/lessdugin/crates/lessdugin-core/src/sync/rangesync.rs
- /home/user/lessdugin/crates/lessdugin-core/src/verify/mst.rs
- /home/user/lessdugin/crates/lessdugin-core/src/store/mod.rs
- /home/user/lessdugin/crates/lessdugin-core/src/identity/mod.rs
- /home/user/lessdugin/crates/lessdugin-indexer/src/main.rs

##### DESIGN DOC: SOCIAL #####
# lessdugin: social semantics design (voting, karma, ranking, moderation, community features)

## 0. Decisions at a glance

1. **The view is a pure fold.** `ForumView = fold(ForumDecl, RecordSet, Observations, now)`, followed by a local `Lens`. The fold is implemented once, in `lessdugin-core` (Rust, also compiled to wasm). Given the same input digest and the same `now`, it produces bit-identical output on every client and on the indexer. All derived social state (karma, scores, visibility, ranking, canonical wiki revision) is computed. None of it is stored as an authoritative record.
2. **The layers form a strict DAG, so there is no recursion anywhere.** L0 authorities → L1 forum-moderation state → L2 participation and seeds → L3 karma (staged) → L3.5 raw scores → L4 gates → L5 author moderation, reacts, tags, wiki, rate limits → L6 visibility and ranking → L7 viewer lens. No layer reads a higher one. In particular, karma ignores author moderation, reacts, tags and visibility.
3. **State records, not event logs.** Each "one opinion per (actor, subject)" thing lives at a **derived rkey**. A repo can hold only one record per rkey, so dedup comes from the structure and needs no "latest timestamp wins" rule. A record whose rkey is not the canonical derivation is **invalid**, not just a duplicate.
4. **One opinion record per (voter, subject).** It carries the karma axis, the agreement axis, reacts and tag-relevance votes together. This replaces the lead's idea of one record per `subject + '#' + axis`.
5. **Karma is `karma/v1`.** It is staged and seeded: K1 is a vote count from seeds, E is the set of eligible voters, KP is the power basis, and K is the displayed karma. There are exactly three linear passes and a per-pair cap. Voting power never extends more than two hops from the seeds.
6. **Moderation precedence uses rank, not time.** Ranks are owner > admin > moderator > post author (scoped). When two actors of the same rank disagree, the **safety-ordered minimum** wins. Removing a moderator from the declaration makes their actions lapse immediately. Keeping those actions requires another authority to explicitly copy them.
7. **Time has an explicit model.** Self-asserted timestamps never affect correctness (karma, moderation, dedup, wiki). They only affect ranking, rate-limit windows and locks, and there they are clamped: `t_rank = max(lower_causal, min(createdAt, t_obs, now))`. Honest content therefore ranks the same on every client.
8. **Votes and reacts are public by construction.** The UI must say so. Private ballots are only possible through an optional forum-run tally EXTENSION, at the cost of trusting it.
9. **No in-protocol reports in the MVP.** The MVP offers a contact link. A forum report service via `com.atproto.moderation.createReport` is a LATER EXTENSION. Sealed HPKE reports are LATER, with the metadata leak documented.
10. **Determinism hygiene.**
    - `BTreeMap`/sorted vectors only.
    - Times are integer microseconds.
    - Floats only in ranking, via the `libm` crate, never platform `powf`, so native and wasm agree bit for bit.
    - Ties break by URI bytes, then CID bytes.
    - `now` is always an explicit argument.
    - All tunables are **integers**, because the atproto data model has no floats; e.g. `exponentMilli: 1150` means 1.15.

---

## 1. Determinism contract

**Input.** `FoldInput` contains:
- `decl`: the current forum declaration.
- `records`: at most one current value per URI, typed by collection.
- `obs: Map<(uri,cid), Micros>`: earliest observation per record version.
- `repoStatus: Map<Did, Fresh|Stale|Unavailable>`.
- `now`.

`inputDigest = sha256(sorted (uri, cid) of every input record ‖ decl cid ‖ params)`. Indexer responses carry `inputDigest` so a client can check whether it and the indexer folded the same data.

Rules:
- **Unknown is not absent.** If an authority's or a participant's repo is `Unavailable`, the sync engine supplies the last cached records and marks the view `degraded`. A record leaves the input only after a *successful complete listing* of its repo. Otherwise a moderator's PDS outage would bring spam back: the system would fail open.
- **Duplicate URIs with different CIDs.** This should never happen, because the sync engine keeps the higher repo rev. If it does, the fold picks the larger CID bytes and emits a diagnostic.
- **Reference closure.** The sync engine fetches subjects referenced by in-forum opinions and comments that are not yet in the store, within a budget. A vote whose subject has not been fetched does not count yet.
- **Reference implementation is batch.** Incremental implementations (karma deltas, re-ranking) must be property-tested equal to the batch fold.

---

## 2. Time model

| Symbol | Source | Trust |
|---|---|---|
| `t_claim` | `createdAt` (RFC 3339; unparseable → None) | self-asserted |
| `t_tid` | TID rkey timestamp | self-asserted; used for consistency flags only |
| `t_obs` | min(local first-seen of `(uri,cid)`, Jetstream `time_us` / indexer first-seen if configured) | observer; a liar can only make things *older* |
| `t_trusted` | timestamps written by L0/L1 authorities (`until`, `since`, `lockedAt`, ban expiry) | trusted (they are already trusted to moderate) |
| `lower(r)` | comment → `t_rank(parent)`; post → `decl.createdAt`; revision → max `t_rank(parents)` | causal: strongRef CIDs cannot reference the future |

```
t_rank(r) = max(lower(r), min_some(t_claim(r), t_obs(r), now))
```

- **Future-dated content** is pinned to the moment it was first seen and then ages normally. It cannot stay "new" forever. Clamping only to `now` would allow exactly that, which is a bug in the naive approach.
- **Backdating** only hurts the author in hot/new ranking. Where backdating would help an author (lock and ban `since` cut-offs, rate-limit windows), we apply the **late-arrival rule**. If `t_obs` comes from a source that was live-watching the repo (Jetstream tail or indexer), and `t_obs > cutoff + 10 min` while `t_claim < cutoff`, the item is treated as created at `t_obs`. It is also flagged `backdated`.
  - CORE-only clients doing a cold sync cannot detect backdating. This is documented, and moderators can remove such items.
- **Flags (display only):** `|t_claim − t_tid| > 1h` → "inconsistent timestamps"; a comment whose `t_claim` is earlier than its parent's → clamped and flagged.
- **Account age** (for "locked to accounts created after X" and `minAccountAgeDays`) comes from the `createdAt` of the did:plc genesis op in the PLC audit log, which is a trust root anyway. For did:web, the first-seen time is used, which is the most restrictive choice.

---

## 3. Derived record keys

```
rkey(kind, parts…) = base32lower_nopad( sha256( "lessdugin:" ‖ kind ‖ ":v1" ‖ Σ("\n" ‖ part) ) )[0..26]   // 130 bits, chars a-z2-7
```

| Kind | Parts | Collection (semantic name) | Repo |
|---|---|---|---|
| `opinion` | canonical subject at-uri (DID authority, never a handle) | opinion | voter |
| `mod` | forumUri, subjectKey | modState (tier=forum) | owner/admin/mod |
| `authormod` | forumUri, subjectKey | modState (tier=author) | post author |
| `member` | forumUri | membership | participant |
| `coauthor` | postUri | coauthorAccept | coauthor |
| `author` | forumUri | authorSettings | author |

- `subjectKey` is one of: `did:…`, `at://…`, or `tag:<slug>` (slug `[a-z0-9-]{1,64}`).
- In lexicons, these fields are unions. Restricted at-uris cannot carry fragments, so tags cannot be at-uri fragments.
- Posts, comments, revisions, sequences and dialogue messages use TIDs.
- Validation: if `record.rkey ≠ rkey(kind, derived from record fields)`, the record is invalid and ignored. Raw view shows it with an "invalid key" marker.

---

## 4. Authorities, participation and seeds (L0–L2)

**Authorities (L0).** These come from the current `forum` declaration in the owner's repo.
- `owner` = repo DID, rank 3. Cannot be removed. Migration is via `successor` (LATER).
- `moderators[]` = `{did, role: admin|moderator}`, rank 2 or 1, at most 50.
- Only the owner edits the list. Admins cannot appoint, so there are no delegation chains.
- If a DID is listed more than once, its highest rank applies.

**Participation modes** (`decl.policy.membership`):

| | `roster` | `open` | `open` + `reviewNewUsers` |
|---|---|---|---|
| Participant | authority, OR (resolved `access ∈ {member, readonly}` AND own membership record) | own membership record AND `access ≠ banned` | same as open; content `Hidden(PendingReview)` until `access=member` |
| Discovery | **CORE**: list modState in owner and mod repos → candidate DIDs → their repos. Complete and enumerable. | **CORE+**: Constellation `getBacklinks(subject=forumUri, source=membership:forum)` as primary; relay `listReposByCollection(membership)` as fallback (it grows with the whole ecosystem, not the forum); Jetstream `online.7778777.lessdugin.*` for live updates; indexer optional | same; the review queue needs CORE+ |
| Seeds S | authorities ∪ participants | authorities ∪ `trusted` ∪ (all participants if `openVoting`) | authorities ∪ `trusted` ∪ `access=member` |
| Completeness | provable | best-effort | best-effort |

- **Mutual consent.** A membership record by the user plus, in roster mode, a grant by a moderator. Moderators cannot conscript people. Users leave by deleting their membership record, which hides their content in the default view; raw view still shows it.
- Moderator grants count for **vote eligibility even after a member leaves**, so a departure does not wipe other people's karma. The one exception is if the departing user deletes their opinion records.
- `readonly` (alumni): content with `t_rank ≤ since` stays visible; later content is hidden.

---

## 5. Moderation fold (L1, plus L5 for the author tier)

### Dimensions and safety order

For ordered dimensions, the safe end wins ties.

| Subject | Dimension | Values (safe value first) | Who may set it |
|---|---|---|---|
| content | `removed` (+reason) | true < false | forum tiers; author tier for comments in own posts |
| content | `votesVoid` | true < false | forum tiers only (affects karma) |
| post | `promotion` (+curationNote) | personal < frontpage < curated | forum tiers |
| post/comment | `pinned` | false < true | forum tiers; author tier for comments on own posts |
| post | `locked` (+`lockedAt` t_trusted) | true < false | forum tiers (authors lock through the post record) |
| question | `acceptedAnswer` | unordered | forum tiers (overrides the author's choice) |
| `tag:slug` | `removed`, `official` (false < true), `canonicalRevision` (unordered) | | forum tiers |
| DID | `access` (+since, +until) | banned < readonly < member | forum tiers |
| DID | `votesNullified` | true < false | forum tiers |
| DID | `trusted` | false < true | forum tiers |
| DID | `rateLimit {commentsPerDay, postsPerWeek}` | componentwise min | forum tiers |

### Resolution

```
opinions(subject, dim) = [(rank(actor), value) for rec in modState
     where actor ∈ CurrentAuthorities                       // former mods lapse instantly
       and rec.forum == F and rec.rkey == rkey(tier, F, subjectKey(rec))
       and dim ∈ rec and !(rec.until ≤ now)]
resolve(dim) = let r = max rank; vals = values at rank r;
               ordered dim   → min_safety(vals)
               unordered dim → if all equal { that } else { None }   // same-rank conflict → no override
```

- Owner opinions always win.
- At equal rank, a restore (for example `removed=false`) cannot beat another moderator's removal. A higher rank must step in, or the remover must retract. This is a deliberate two-person safety property.
- **Content vs user.** An explicit content-level decision at rank ≥ the rank of the user-level decision overrides it for that item ("keep this post despite the ban").
- **Lapse and adoption.** `lessdugin mod adopt <former-mod>` copies a former moderator's currently-relevant resolved opinions into the adopter's repo. It uses `applyWrites` batches of up to 200 ops and records `adoptedFrom` for provenance. No time reasoning is needed.
- **Author tier (rank 0).** Effective only when all of these hold:
  - `decl.policy.authorModeration` is true;
  - the post author's K ≥ `gates.authorMod.personal` (or `.frontpage` if the post's resolved promotion ≥ frontpage), which is an L4 gate;
  - the subject is inside the author's own post thread.

  Author tools:
  - `authormod` records: `removed` and `pinned` on comments;
  - post fields `bannedUsers[]` and `commentsLocked` (+`lockedAt`);
  - `authorSettings.bannedUsers[]` (applies to all of the author's posts in the forum), `moderationStyle` (easy-going | norm-enforcing | reign-of-terror) and `guidelines` (markdown).

  `moderationStyle` changes behaviour in one way only: removals without a `reason` count only under reign-of-terror. Forum tiers always override the author tier.
- **Moderator hat.** A comment's `asModerator: true` shows a hat only if its author is a current authority.
- **Raw view.** Everything discovered is shown, each item annotated with every opinion, including lapsed ones (shown as "by former mod") and invalid-key records.
- **Mod log.** All modState records from current and former authorities, sorted by the record's `updatedAt` (trusted for current authorities). Both are CORE MVP.

---

## 6. Opinion record and dedup rules

```
opinion (rkey = rkey("opinion", subject.uri)) {
  subject: strongRef, forum: at-uri,
  karma?: "up"|"down"|"bigUp"|"bigDown",
  agreement?: "agree"|"disagree"|"bigAgree"|"bigDisagree",
  reacts?: [{react: string, stance: "for"|"against", quote?: string ≤ 500}] ≤ 40,
  tags?: [{slug, dir: "up"|"down"|"bigUp"|"bigDown"}] ≤ 32,     // subject must be a post
  createdAt, updatedAt
}
```

All values are `knownValues`. An unknown value means that axis is ignored, while the rest of the record still counts.

A vote V is **valid** iff all of the following hold:
1. The rkey matches.
2. `subject.uri` is canonical (DID authority).
3. The subject's collection is one of post, comment, sequence, tagRevision, dialogueMessage.
4. The subject **currently exists** in the input, and its forum (for a comment, the forum of its root post) equals `V.forum`.
5. The voter is not in `listedAuthors(subject)` (primary author plus all *listed* coauthors). A self-opinion is ignored entirely.
6. The voter does not have resolved `votesNullified` and is not `access=banned`.
7. The subject is not `votesVoid`.

Notes:
- **Weight additionally requires the voter ∈ E** (eligible voters, §7). Other valid votes are shown as "n unweighted votes".
- `subject.cid` does not affect counting. It drives the "voted on an earlier version" display and quote anchoring.
- Deleting content removes its karma in both directions. A vote on a nonexistent URI must not count, because that would allow fabricated downvotes, and absence cannot be told apart from deletion without history. This is documented.
- Author bans from a post do **not** suppress votes. Taking author bans into account would make L3 depend on L5.

---

## 7. Vote power (port of `calculateVotePower`, verified against ForumMagnum master)

```
small(k) = if k >= 1000 {2} else {1}
big(k)   = first match in [(500000,16),(250000,15),(175000,14),(100000,13),(75000,12),(50000,11),
           (25000,10),(10000,9),(5000,8),(2500,7),(1000,6),(500,5),(250,4),(100,3),(10,2)] where k >= t, else 1
power(k, d) = let k = max(k,0); up→small(k), down→−small(k), bigUp→big(k), bigDown→−big(k)
```

The agreement axis uses the same table.

---

## 8. `karma/v1` (L3): staged, seeded, order-independent

```
credited(s, mode) = PrimaryOnly → [author(s)]
                    WithCoauthors → [author(s)] ++ accepted coauthors ∩ E, in post.coauthors order, first coauthorCreditMax (4)
        // dialogue posts: participants = coauthors. Votes on dialogueMessage, sequence, tagRevision earn no karma in v1.

accumulate(V, eligible, w, mode) :
  pair = BTreeMap<(voter, recipient), i64>
  for v in V where v.karma.is_some() and v.voter ∈ eligible and kind(v.subject) ∈ {post, comment}:
      for r in credited(v.subject, mode): pair[(v.voter, r)] += w(v)
  out[r] = Σ_voter clamp(pair[(voter,r)], −pairCap, +pairCap)        // pairCap default 25

karma_v1(in):
  V  = valid votes (§6)
  S  = seeds (§4)
  K1 = accumulate(V, S, v ↦ sign(v.karma), PrimaryOnly)             // stage 1: vote counts from seeds
  E  = S ∪ { u ∈ Participants : K1[u] ≥ establish (1) }              // eligible voters ("needs ≥1 karma to vote")
  KP = accumulate(V, E, v ↦ power(K1[v.voter], v.karma), PrimaryOnly) // power basis
  K  = accumulate(V, E, v ↦ power(KP[v.voter], v.karma), WithCoauthors) // displayed karma
```

Properties:
- **Order-independent.** Everything is integer sums over sets.
- **Acyclic.** Each stage reads only the previous stage.
- **Linear cost**, O(|V|), three passes.
- Coauthor credit uses E (from K1). It appears **only in K**, never in K1 or KP, which closes the coauthor karma-laundering route to voting rights.
- Displayed vote power for user u is `small(KP[u])` / `big(KP[u])`.
- **Consistency.** Without caps and coauthors, `K[a] = Σ over a's items of (baseScore − selfVote)`, because scores also use `power(KP)` (§9).
- **Seeded trust has a two-hop radius.** Sybils created by an established user never become eligible voters, because E depends only on K1, which only seeds feed. Any single voter's effect on any single recipient is bounded by ±pairCap. This also blunts revenge mass-downvoting.
- **Scale.** K1 is count-based, so a voter's power basis grows more slowly than ForumMagnum's power-accumulated karma. This is accepted; KP and K are close in practice.
- **Versioning.** `decl.policy.karma = {algorithm: "karma/v1", pairCap, establish, coauthorCreditMax}`. Any change that affects results (for example, crediting tag revisions) requires `karma/v2`. That is why coauthor and dialogue credit are specified in v1 even though their UI is LATER.
  - Clients must implement every version they claim to support. An unknown algorithm falls back to showing raw counts with a banner.
- **Local override** of the algorithm is a lens ("my view"). It is never shown as the forum's karma.
- **Scaling hook (LATER, CORE).** Authorities may publish a `karmaSnapshot` blob (`{algorithm, inputDigest, asOf, entries[(did, K1, KP, K)]}`). Clients may use the snapshot from the highest rank with the latest `asOf` for power and gates, and compute item scores live. Moderators are already trusted, so this introduces no new trust. `lessdugin karma verify` recomputes and diffs.

---

## 9. Scores and agreement (L3.5)

```
self(i)  = author ∈ E ? (i is post ? big(KP[author]) : small(KP[author])) : 0       // FM: authors self-strong-vote posts, self-small-vote comments
base(i)  = self(i) + Σ_{valid v on i, voter∈E} power(KP[voter], v.karma)            // no pairCap on item scores
agree(i) = Σ_{valid v on i, voter∈E} power(KP[voter], v.agreement)                  // never feeds karma
```

- Which axes count depends on the post's voting system: `decl.policy.votingSystems` allows a set; `post.votingSystem` picks one; the default is `twoAxis`. Axes that are not enabled are ignored but still stored.

---

## 10. Reacts (L5)

- The palette is `decl.policy.reactPalette` (default `reacts/v1`, a fixed list of about 24 kebab-case IDs: agree, disagree, important, insightful, changed-mind, crux, not-a-crux, locally-valid, locally-invalid, typo, confused, thanks, heart, laugh, wrong, examples, clear, missed-point, strawman, combative, scout-mindset, soldier-mindset, seen, empathy). Unknown IDs are ignored.
- **Quote keys.** `qkey = NFC(collapse_ws(trim(quote)))` (or the empty string).
  - Anchored if `qkey` occurs exactly once in the plaintext of the rendered subject version `subject.cid`. Verification uses the revision snapshot whose recomputed CID equals `subject.cid`, otherwise the current version.
  - Otherwise the react is shown un-anchored as "on an earlier version".
- **Within one record,** entries are deduped by `(react, qkey)`. A for/against contradiction drops both.
- **Per subject `(react, qkey)`:**

```
F = {u : u has stance=for, u ∈ E, u ∉ listedAuthors}, A = {… stance=against …}
valid ⇔ ∃u∈F: K[u] ≥ gates.react.create (10)
if valid: supporters = {u∈F : K[u] ≥ gates.react.second (5)}, opposers = {u∈A : K[u] ≥ gates.react.anti (20)}
display "creator" = argmax_{u∈supporters} (K[u], −did) ; others "seconded"
```

This is an order-free reformulation of ForumMagnum's order-dependent create/second rule. Gates use current K, so a react can disappear retroactively if its supporters lose karma, which is a useful sybil property.

---

## 11. Gates (L4)

Values come from `decl.policy.gates`. These are the defaults the `forum create` command writes.

| Gate | open (ForumMagnum) | roster |
|---|---|---|
| Weighted vote | ∈ E | ∈ E (= members) |
| React second / create / anti | 5 / 10 / 20 | 0 / 0 / 5 |
| Links followed (no `rel="nofollow ugc"`) | 10 | 0 |
| Author moderation, personal / frontpage post | 50 / 2000 | 10 / 100 |
| Side-comment margin display | 10 | 0 |
| Sequence listed in library | 1000 | 0 |
| Tag create (genesis revision) / wiki edit | 10 / ∈E | 0 / member |
| Coauthor, dialogue participation | ∈ E | member |

---

## 12. Client-enforced rate limits (L5)

- Applicable rules come from `decl.policy.rateLimits = "auto/v1" | "none"` plus the resolved per-user `rateLimit` override.
- `auto/v1` applies only in open mode by default.
- Every applicable rule is enforced independently; an item is excess if *any* rule flags it.

| Class | Condition (current K, recent scores) | Limit |
|---|---|---|
| Comments | K < 5 | 3 / 24h |
| Comments | K < −2 | 1 / 24h |
| Comments | net base of last 20 items < 0 AND ≥ 3 distinct E downvoters | 1 / 1h |
| Posts | K < 5 | 2 / 7d |

```
excess(i) ⇔ ∃ rule (L, W) applicable to author(i): |{ j ∈ items(author, class) : (t_rank(j), uri(j)) < (t_rank(i), uri(i)) ∧ t_rank(j) > t_rank(i) − W }| ≥ L
```

- This does not depend on `now`, and the late-arrival rule applies.
- Excess items are shown as `Collapsed(RateLimited)`.
- The author's own client refuses to publish an item that would be excess. This is UX, not security.
- Applying limits retroactively when K changes is intended.

---

## 13. Visibility pipeline (L6)

The first match wins, then the thread pass runs.

```
not in scope / invalid                          → Excluded
author not participant                           → Hidden(NotParticipant)
resolved removed == true                         → Hidden(ModRemoved{rank, actor, reason})
(no explicit keep ≥ rank) and author banned/readonly with t_rank ≥ since (or since absent) → Hidden(Banned)
review mode and author access ≠ member           → Hidden(PendingReview)
author-tier effective: authormod removed | commenter ∈ post.bannedUsers ∪ authorSettings.bannedUsers
   | (commentsLocked ∧ t_rank > lockedAt) | (forum locked ∧ t_rank > lockedAt) → Hidden(AuthorModerated{style, reason})
excess                                           → Collapsed(RateLimited)
open ∧ author ∉ E ∧ display.newUserVisibility ∈ {collapse, hide} → Collapsed/Hidden(NewUser)   // default collapse
base ≤ display.collapseBelow (−5)                → Collapsed(LowScore)
else Visible
thread pass: Hidden node with ≥1 non-Hidden descendant → Tombstone(reason)   // threads keep their shape
```

The **lens** runs after the fold. It applies local mutes (`Collapsed(Muted)`), optional "honor my `app.bsky.graph.block` records", excluded moderators (recompute L1 without them), and the raw view. Labeler subscriptions are LATER.

---

## 14. Ranking, comment sorting and threads

```
hot(b, t, now) = let age_h = max(now − t, 0) as f64 / 3.6e9;
                 b as f64 / libm::pow(age_h + offsetHours, exponentMilli / 1000.0)    // offsetHours ≥ 1 enforced
```

**Post sorts.**
- `magic`: computed over `b = base + (curated ? curatedBonus(10) : 0) + (frontpage ? frontpageBonus(0) : 0)`.
  - Sort key: `(pinned desc, b > 0 desc, b > 0 ? hot desc (total_cmp) : (b desc, t_rank desc), uri asc)`.
  - Excludes posts older than `magicCutoffDays` (90).
  - The frontpage list shows promotion ≥ frontpage; the "All" list shows everything; quick takes, drafts and dialogues still in draft are excluded.
- `top`: (base desc, t_rank desc, uri).
- `new` and `old`: by t_rank.
- `recentComments`: the maximum t_rank among visible comments.
- Defaults: offset 2h, exponent 1150. LessWrong's own setting is offset 6, which owners can choose.
- Personalized frontpage magic (LATER, local data only): `offset 6, exponent = min(0.5·(1+1.4·af), 1.08)`, with `af = Σ_{d=1..30} visited(d)·0.9^d / Σ 0.9^d`. Tag-filter lens: hidden → exclude; required → filter; numeric weight w → `b += w` (subscribed +25, reduced −25).
- `topAdjusted` is LATER.

**Comment sorts.**
- magic (same formula, comment parameters), top, new, old, latestReply (maximum visible t_rank in the subtree).
- Always: pinned first; on question posts, the accepted answer and then answers (by score) before ordinary comments.

**Thread assembly.**
1. Keep comments whose `root.uri` equals the post.
2. If a parent is missing or in another post, synthesize a `Missing(uri)` placeholder under the root so subtrees survive.
3. **Cycles** can arise from edits that rewrite `parent` at the URI level. Walk the parent chains in URI order, and in each cycle cut the edge leaving the member with the greatest URI; re-attach it to the root and flag it `Orphaned`.
4. Depth cap is 1000.
5. Compute `t_rank` top-down after cycles are cut.

---

## 15. Tags: relevance and collaborative wiki (L5)

- A tag is identified by `(forum, slug)`. The tag page is a DAG of `tagRevision` records `{forum, slug, parents: strongRef[] ≤ 8 (empty means genesis), name, body(md), summary}`, stored in editors' repos.
- An edge counts only if the parent's strongRef CID equals the parent's *current* CID; otherwise it dangles. CIDs make cycles impossible.
- **Valid revisions:** the author is in E with K ≥ the edit gate (genesis requires the create gate), and the revision is not `removed`. Removed revisions can be ancestors but never heads.

```
heads = valid revs with no valid child
canonical = resolved tag:<slug>.canonicalRevision if it resolves to an existing rev   // moderator pin
          else argmax_{h∈heads} ( revScore(h) = Σ_{E voters} power(KP, opinion.karma on h),
                                  K[author(h)], depth(h) = longest valid-parent path, reverse cid bytes )
```

- The tag exists iff a canonical revision exists and `tag:slug` is not `removed`.
- The UI shows "N unmerged branches", and the editor creates merge revisions with multiple parents.
- **Relevance** of a tag on a post:

```
rel(post, slug) = (slug ∈ post.tags ? max(1, small(KP[author])) : 0)
                + (slug ∈ authorOnlyTags ? 0 : Σ_{E voters ≠ listedAuthors} power(KP[voter], opinion.tags[slug]))
```

The tag is applied iff the tag exists and rel > 0. There are no TagRel objects: applying a tag means writing the `tags[]` entry in your opinion record.

---

## 16. Other content types

- **Sequences (MVP, CORE).** `sequence {forum, title, description, chapters[{title, posts: at-uri[] }] ≤ 1000 posts}` in the author's repo. Posts by other authors are allowed (collections). Library listing: author K ≥ gate, not removed, sorted by sequence score (opinions, no karma credit). Next/prev navigation uses the sequence the reader came from, else the post author's own sequence, else the highest-scored one. "Continue reading" is local. Books/collections are LATER.
- **Q&A (LATER, CORE).** `post.kind = question`. Answers are top-level comments with `answer: true`. `post.acceptedAnswer` (author) is overridable by the moderator dimension.
- **Quick takes (LATER, CORE).** `post.kind = quickTake` (no title, ≤ 10k chars) rather than ForumMagnum's per-user container post. Replies are normal comments. They have a separate feed and moderators can promote them to the frontpage.
- **Coauthors (credit in v1; UI LATER).** `post.coauthors[] ≤ 8`. A coauthor is accepted iff they have a `coauthorAccept` record at `rkey("coauthor", postUri)` and are in E. Pending coauthors are never displayed as authors.
- **Dialogues (LATER, CORE).** `post.kind = dialogue` with accepted coauthors as participants. Messages are `dialogueMessage {dialogue: at-uri, prev: strongRef[], body}` in participants' repos.
  - Order: topological order over `prev`, ties broken by (t_rank, uri).
  - Messages from non-participants are ignored.
  - **"Draft" dialogues are public.** This must be called out. Private drafting requires atproto Spaces (alpha), so LATER.
- **Events (LATER, CORE).** Reuse `community.lexicon.calendar.event` (name, createdAt, startsAt, endsAt, mode, status, locations, uris) and `rsvp` (subject strongRef, status interested/going/notgoing).
  - The event post has `kind: event` and dual-publishes the calendar event with the same rkey, for interop.
  - RSVPs are discovered by crawling participants (roster mode) or via Constellation backlinks.
- **Polls, local groups, side comments.** The side-comment `quote` field is MVP data with LATER margin UI. Polls and local groups are LATER.

---

## 17. Sybil resistance in open mode

**Threat.** DIDs are effectively free, especially with a self-hosted PDS.

**Guarantee, enforced by a property test.** Adding any records authored by DIDs outside E changes none of the following for anyone outside that set: karma, scores, agreement, reacts, tag relevance, wiki canonical selection, or rate-limit state.

**What remains possible:**
- One established user (in E) can add at most one vote per subject, and at most ±pairCap of karma to any one recipient.
- Sybil content is visible, but collapsed by default (`newUserVisibility`).
- Bandwidth exhaustion: the sync engine fetches at most `newcomerBudget` (500) newcomer repos per sync, the most recent first, according to Constellation ordering.

**Mitigations available to moderators and owners:**
- `trusted` (a vouch) and `access` decisions;
- `reviewNewUsers`;
- `minAccountAgeDays` (PLC genesis, LATER);
- `votesNullified` and `votesVoid`;
- `openVoting: false` (the default).

**Mitigation available to viewers:** mutes. Optional labelers (LATER).

**Hardening in the LATER phase:** "vouch" records from E members (rate-limited, e.g. 5 per 30 days), which extend seeds by one hop while keeping the two-hop voting-power radius.

---

## 18. Reports

- **MVP.** `decl.reportContact` (mailto or a Bluesky handle for DMs) is shown in the "Report" action. Nothing goes into public repos, because a public report record leaks the reporter's identity and the subject.
- **LATER, EXTENSION.** `decl.reportService` DID. The client calls `com.atproto.moderation.createReport`, proxied through the reporter's PDS with an `atproto-proxy` header. This needs the scope `rpc:com.atproto.moderation.createReport?aud=<did>%23atproto_labeler`. The report is private to the forum's service (Ozone-compatible).
- **LATER, CORE+.** Sealed reports.
  - `decl.reportKeys[]` holds X25519 public keys.
  - The reporter writes a `report` record containing only an `age`-style multi-recipient HPKE ciphertext: X25519, HKDF-SHA256, ChaCha20-Poly1305, anonymous recipient stanzas, padded to 1 KiB or 4 KiB buckets.
  - The record has **no `forum` or `subject` field**. Moderators trial-decrypt reports found on participant repos.
  - Remaining leak: "X filed a report somewhere around time T". This is documented.

---

## 19. Public vs local data

| Data | Where | Notes |
|---|---|---|
| Posts, comments, revisions, sequences, tag revisions, dialogue messages, events, RSVPs | public repo | |
| **Opinions (votes, agreement, reacts, tag votes)** | **public repo** | **Unavoidable.** The UI shows aggregates only, but warns "your votes are public" on first vote. Retaliation risk is a real social change compared with ForumMagnum's private votes. |
| Membership, coauthor acceptances, author settings, modState (with reasons) | public repo | moderation is transparent by design |
| Read status, notification watermark, drafts, subscriptions, bookmarks, mutes, tag filters, activity factor, karma history (for the notifier) | local (SQLite or IndexedDB) | default |
| Bookmarks (`community.lexicon.bookmarks.bookmark`), subscriptions | public, opt-in, LATER | subscriptions enable a push EXTENSION |
| Cross-device sync of local state | LATER | Spaces, or a passphrase-encrypted blob in your own repo |
| Private ballots | LATER EXTENSION | Users send votes to the forum's `tallyService` via service auth. It publishes signed per-item tallies and per-user karma. Clients trust it the way they trust moderators. |

---

## 20. Notifications, subscriptions, read status

```
notifications(viewer, view, watermark):        // a "new" item has t_obs > watermark (never createdAt → backdating cannot hide it)
  Reply (parent author = viewer) · Mention (at://did link or mention facet = viewer) · NewComment (root ∈ subs.posts)
  · NewPost (author ∈ subs.users | tag ∈ subs.tags | ∈ subs.sequences) · Reacted (aggregated per item)
  · ModAction (diff of resolved L1/L5 state affecting the viewer's content or account vs the last snapshot)
  · KarmaChange (diff of K vs the stored snapshot, batched realtime/daily/weekly)
  LATER: CoauthorInvite, DialogueMessage, Answer, AcceptedAnswer, Rsvp
  Default filter: only from authors ∈ E (configurable).
```

- CORE: computed at sync time.
- CORE+: live in the TUI and SPA via Jetstream with `wantedCollections=online.7778777.lessdugin.*` plus participant DIDs (at most 10k).
- EXTENSION: push and email (LATER).
- Unread-comment highlighting: comment `t_obs` > the local `lastVisited[post]`.

---

## 21. Feature classification

CORE = PDS plus PLC/DNS only. CORE+ = plus generic public infrastructure (relays, Jetstream, Constellation, Slingshot, labelers). EXTENSION = a lessdugin-specific optional service.

| Feature | Tier | Phase |
|---|---|---|
| Forum declaration, authorities, policy | CORE | MVP |
| Roster discovery, membership | CORE | MVP |
| Open-mode discovery | CORE+ (indexer optional) | MVP |
| Posts, link posts, edit revisions, threaded comments, tombstones | CORE | MVP |
| Opinion record: karma and agreement axes, scores | CORE | MVP |
| `karma/v1` (including coauthor credit) | CORE (roster), CORE+ (open) | MVP |
| Karma snapshots by moderators | CORE | LATER |
| Reacts (gates, anti-react, quote keys) | CORE | MVP (inline margin UI LATER) |
| Ranking: magic, top, new, old, recentComments; comment sorts | CORE | MVP |
| Personalized magic, tag-filter weights, topAdjusted | CORE (local) | LATER |
| Moderation fold, raw view, mod log, moderator hat, pin, curate/frontpage | CORE | MVP |
| Moderator adoption tool | CORE | MVP |
| Author moderation (bans, removals, locks, styles) | CORE | MVP |
| Auto and manual rate limits | CORE | MVP |
| Review queue (`reviewNewUsers`), `minAccountAgeDays` | CORE+ / CORE | LATER |
| Reports: contact link | CORE | MVP |
| Reports: service | EXTENSION | LATER |
| Reports: sealed | CORE+ | LATER |
| Tags, relevance, wiki DAG with canonical selection | CORE | MVP |
| Revision voting UI | CORE | LATER |
| Sequences | CORE | MVP |
| Books / collections | CORE | LATER |
| Q&A, quick takes, coauthor UI, dialogues | CORE | LATER (phase 2) |
| Events / RSVP (community.lexicon), polls, local groups | CORE | LATER |
| Backlinks within forum / cross-forum | CORE / CORE+ | MVP / LATER |
| Notifications at sync / live / push | CORE / CORE+ / EXTENSION | MVP / MVP / LATER |
| Local read status, drafts, bookmarks, subscriptions | local | MVP |
| Public bookmarks/subscriptions, cross-device sync | CORE | LATER |
| Local search / global search | CORE / EXTENSION (indexer) | MVP / MVP (indexer) |
| Recommendations, private ballots (tally), push notifier | EXTENSION | LATER |
| Labeler advisories | CORE+ | LATER |
| Private messages | not planned (link to Bluesky chat; Spaces LATER) | — |
| Annual review, jargon, LLM features, crossposting, RSS import | not planned / LATER | — |
| RSS export | CORE (CLI) / EXTENSION (indexer feeds) | LATER |

---

## 22. Challenges to the preliminary ideas

1. **Per-axis vote rkeys:** replace with one opinion record per (voter, subject), which is atomic, cheaper, and gives a single Constellation backlink path (`opinion:subject.uri`). Domain-separate the hash by kind and version.
2. **"Clients still dedupe":** make a non-canonical rkey *invalid*. Dedup then follows from repo structure, with no timestamp tie-breaks.
3. **Moderation "ordering":** no temporal ordering at all. Use per-(actor, subject) state, rank, and same-rank safety-min. Lapse is immediate, and continuity comes from explicit adoption.
4. **"Mods publish roster" alone:** also require the user's own membership record (consent and a clean way to leave). Vote eligibility follows the moderator grant, so departures do not wipe others' karma.
5. **Open discovery via `listReposByCollection`:** it grows with the whole lessdugin ecosystem, not the forum. Use Constellation per-forum backlinks as primary, the relay as fallback, and the indexer optionally.
6. **"Users can override karma locally":** only as a lens. Gates enforced by readers must use the forum's algorithm, or clients will disagree about what is visible.
7. **"Votes necessarily public":** true for CORE. Offer an explicit trust-the-forum tally EXTENSION rather than pretending otherwise.
8. **"Clamp future timestamps":** clamping to `now` is wrong, because the item would be perpetually fresh. Clamp to first observation, plus causal lower bounds.
9. **Revision snapshots:** they must embed the full prior record value so the client can recompute its CID and match `opinion.subject.cid`. That gives verifiable history for quote anchoring and "voted on version X".
10. **Encrypted reports "maybe later":** agreed to defer, but the design must avoid public `forum` and `subject` fields; otherwise encryption protects little.
11. **Unknown is not absent** must be part of the spec, not just the sync engine: moderation must not fail open on PDS outages.

---

## 23. Policy block (integers only, because atproto has no floats)

```
policy: { membership: "roster"|"open", reviewNewUsers, openVoting, newcomerBudget,
  karma: {algorithm: "karma/v1", pairCap: 25, establish: 1, coauthorCreditMax: 4},
  votingSystems: ["twoAxis","reacts"], defaultVotingSystem, reactPalette: "reacts/v1" | [ids], authorOnlyTags[],
  gates: {...§11}, rateLimits: "auto/v1"|"none",
  ranking: {offsetHours: 2, exponentMilli: 1150, curatedBonus: 10, frontpageBonus: 0, magicCutoffDays: 90},
  display: {collapseBelow: -5, newUserVisibility: "collapse", defaultPromotion: "personal"(open)|"frontpage"(roster)},
  authorModeration: true, reportContact, reportService?, reportKeys?, tallyService?, labelers? }
```

Policy changes apply retroactively; this is inherent and documented.

---

## 24. Core module layout (`lessdugin-core/src/social/`)

| File | Contents |
|---|---|
| `time.rs` | t_claim, t_tid, t_rank, late-arrival rule |
| `keys.rs` | rkey derivation, subjectKey |
| `authority.rs` | L0 |
| `modfold.rs` | L1 + author tier, raw annotations |
| `participation.rs` | L2: modes, seeds |
| `power.rs` | vote power port |
| `opinion.rs` | validity, dedup |
| `karma.rs` | L3 |
| `score.rs` | L3.5 |
| `gates.rs` | L4 |
| `reacts.rs` | |
| `ratelimit.rs` | |
| `tags.rs` | relevance + wiki DAG |
| `sequence.rs` | |
| `thread.rs` | |
| `visibility.rs` | |
| `rank.rs` | uses `libm` |
| `notify.rs` | |
| `lens.rs` | |
| `view.rs` | `fold(&FoldInput, now) -> ForumView`, `inputDigest`; wasm exports the same |

---

## 25. Test vectors and property tests

Vectors live in `testdata/vectors/social/*.json`:

```
{forum, records[{uri, cid, value}], obs, repoStatus, now,
 expect: {E, K1, KP, K, scores{uri: {base, agree, unweighted}}, visibility{uri: state},
          ranking{sort: [uri]}, reacts{…}, tags{…}, canonicalRevision{…}, hot_bits{uri: u64}}}
```

They run in Rust and, through wasm, in TypeScript.

**Vector cases:**
- **power:** k ∈ {−5, 0, 9, 10, 99, 100, 249, 250, …, 499999, 500000, 10⁹} × 4 directions.
- **keys:** fixed URIs mapped to fixed rkeys; the handle-form URI is rejected.
- **karma:**
  - self-vote ignored;
  - bigUp in K1 counts as 1;
  - seed → newcomer brings the newcomer into E;
  - a sybil ring of 100 with no seed edge has K1 = 0 and gains no influence;
  - the pairCap boundary at 25/26;
  - coauthor laundering: credit appears in K but not in K1 or E;
  - a vote on a deleted subject does not count;
  - `votesVoid`, `votesNullified`;
  - a seed with negative K1 is still in E.
- **modfold:**
  - owner beats moderator;
  - same-rank remove/restore resolves to removed;
  - unordered conflict resolves to None;
  - moderator removed from the declaration → lapse;
  - adoption restores the actions;
  - expired `until`;
  - content keep beats user ban at ≥ rank;
  - author-tier gating at K 49/50 and personal/frontpage;
  - a reason-less author removal under each moderation style;
  - the author also being a moderator (two record kinds).
- **reacts:** supporters {9} → invalid; {10, 4} → one supporter; {10, 5} → two; anti from K 19 ignored, from K 20 counted; contradiction dropped; quote matching 0/1/2 occurrences.
- **time:**
  - future `createdAt` → `t_obs`;
  - comment before parent → clamped;
  - missing `createdAt`;
  - late-arrival past a lock.
- **ranking:** exact f64 bit patterns for hot; negative partition; pinned first; 90-day cutoff; tie-break by URI.
- **thread:** an edit-induced 2-cycle and 3-cycle; missing parent placeholder; cross-post parent.
- **tags:** two-head fork decided by revScore, then K, then depth, then CID; moderator pin; removed head; dangling parent CID; merge revision; author-only tag.
- **rate limits:** window boundaries and independence from `now`.

**Property tests (proptest):**
- Permutation invariance of every output.
- Duplicate-record idempotence.
- **Layering:** arbitrary author-tier, react, tag and visibility-only records never change E, K1, KP or K.
- **Sybil theorem:** adding records from non-E DIDs leaves every other user's outputs unchanged.
- **Pair bound:** removing one voter changes any recipient's K by at most pairCap.
- Incremental fold equals batch fold.
- Native and wasm produce byte-identical `ForumView` serialization on random inputs.

---

### Critical Files for Implementation

The repo currently contains only a LICENSE; these are the planned files:
- /home/user/lessdugin/crates/lessdugin-core/src/social/karma.rs
- /home/user/lessdugin/crates/lessdugin-core/src/social/modfold.rs
- /home/user/lessdugin/crates/lessdugin-core/src/social/view.rs (fold entry point, layering, inputDigest)
- /home/user/lessdugin/lexicons/online/7778777/lessdugin/opinion.json (with forum.json policy and modState.json)
- /home/user/lessdugin/testdata/vectors/social/ (shared Rust/TS vectors)

##### DESIGN DOC: ARCH #####
# lessdugin: implementation architecture

## 0. Decisions at a glance

**Layout and crate rules**
- **Monorepo.** One Cargo workspace (`crates/*`, `xtask`) and one pnpm workspace (`web`, `packages/*`).
- **Lexicons are the source of truth.** The JSON files under `lexicons/` drive everything else:
  - A Rust generator in xtask turns them into Rust types with `validate()`.
  - tsify turns those Rust types into TypeScript types.
  - No TypeScript code handles raw records, so there is no second TS codegen.
- **Domain logic stays IO-free.** The atproto primitives, lexicon, markdown and core crates never use tokio, reqwest, `SystemTime::now`, threads or spawning.
- **Only the core touches the outside world, through one `Host` trait.** It covers HTTP (anonymous or as the user), DNS TXT, WebSocket, sleep, clock, randomness, and an ordered byte key-value store (`KvStore`).
- **One generic `Engine<H: Host>` everywhere.** The CLI, TUI and indexer use `NativeHost` (reqwest + SQLite). The SPA uses `JsHost` (worker fetch + IndexedDB + a main-thread OAuth bridge). Tests use `MemHost` with `FakePds`.

**Where jacquard is used**
- jacquard is pinned `=0.12.2` and used only in `lessdugin-native::auth::oauth`, behind a facade.
- The docs.rs build for 0.12.2 failed. It is alpha, has a single maintainer, and 0.13 brings breaking changes.
- Its generated types borrow data with lifetimes (`Post<'a>`), which do not suit tsify or wasm-bindgen.
- We write the protocol primitives ourselves: data model, CID, CAR, MST proofs, commit signatures, identity and the XRPC subset. They are about 2–3k lines, tested against the official atproto-interop-tests vectors, and smaller in wasm.

**Browser side**
- **Engine in a Web Worker from day one.**
  - OAuth stays on the main thread, because @atcute/oauth-browser-client keeps sessions in localStorage, which workers cannot use.
  - Anonymous reads go straight from the worker to the PDSes.
  - Signed-in calls are proxied to the main thread.
- **Rendering.** Rust renders markdown to sanitized HTML, including MathML through `math-core`, with no KaTeX. The SPA inserts it through a Trusted Types policy backed by DOMPurify, which is defence in depth only.
  - The TUI and CLI get styled lines from the same event stream.
  - SPA, TUI, CLI, `export --html` and `lessdugin web` all produce the same output.

**Content and trust**
- **Records are stored as canonical DAG-CBOR together with their CID.** Each record is checked on ingest (JSON → CBOR → CID must match). A record can later be upgraded to proof-verified without fetching it again.
- **Media.** Blobs are fetched, checked against their CID (sha256), and shown through `blob:` URLs. External images are click-to-load placeholders.

**Hosting**
- **`lessdugin web`** serves the embedded SPA on `127.0.0.1` at a fixed, persisted port. The SPA then does its own loopback OAuth.
- **Static site.** Hash routing, `base: './'`, client metadata at `https://lessdugin.7778777.online/oauth/client-metadata.json`, CSP with Trusted Types.

---

## 1. Where I disagree with the preliminary ideas

1. **jacquard as the foundation → confine it to native OAuth.** See section 0. The facade (`AuthSession`) lets us replace jacquard-oauth with our own PAR/PKCE/DPoP code (roughly 1k lines) if spike S1 fails.
2. **"SQLite store" / a high-level Store trait → an ordered byte KV with atomic batch writes.**
   - IndexedDB transactions auto-commit as soon as you await anything that is not IDB, so the trait cannot offer interactive transactions.
   - The trait therefore offers only `get_many`, `scan(range, dir, limit)` and an atomic `write(batch)`.
   - All secondary indices are encoded and maintained in core, so query behaviour is the same on SQLite, IndexedDB and memory.
   - Schema migrations are "drop the derived prefixes and re-index from the records table".
3. **Deterministic rkeys: agree, and make them a validity rule.**
   - The rkey is derived from the subject URI, not the CID, so it survives edits: `rkey = base32lower(sha256(subject_uri ‖ 0x00 ‖ axis))[..32]`.
   - Votes, memberships, bookmarks and reactions stored under any other rkey are ignored by every client.
   - Counting becomes trivial (at most one record per key per repo).
   - Writes become idempotent: vote = `putRecord`, unvote = `deleteRecord`, retries are safe, and the outbox can merge rapid toggles.
4. **Client-side karma: agree, but it needs a seed and a fixed point.**
   - Vote power depends on the voter's karma, which depends on votes, so the definition is recursive.
   - A voter cannot be trusted to declare their own vote power.
   - karma/v1 must therefore be:
     - scoped to one forum;
     - computed by a bounded fixed-point iteration from seed karma (owner, moderators, and approved roster members if the forum declares it);
     - with zero power for votes from accounts below 1 karma, which gives basic Sybil resistance;
     - deterministic over the set of records the client has synced.
   - The indexer can publish a karma snapshot plus a hash of its inputs, as a hint only.
   - The formula belongs to the protocol spec; the engine just provides a deterministic `ForumModel` recompute (section 4.4).
5. **Timestamps are claims, not facts.** `createdAt` and TID rkeys are set by the author.
   - Ranking clamps `createdAt` to at most the time the record was observed.
   - Unread state uses local first-seen time.
   - Automatic rate limits are approximate. The spec must say so.
6. **Sync cost needs an explicit envelope.**
   - `listRecords` cannot filter by forum, so a participant's lessdugin records are fetched for all forums at once.
   - Mitigations:
     - call `describeRepo` once per DID to learn which collections exist;
     - skip a repo when `getLatestCommit` shows the rev unchanged;
     - use Jetstream for freshness between syncs.
   - Target for serverless use: about 2k participants, 20k posts and 100k votes per forum.
   - Beyond that the indexer is recommended.
   - Verified MST range sync via `getRepo?since` comes after the MVP.
7. **Discovery order.** Open forums need at least one discovery source, and none of them has to be trusted.
   - Ordered by precision: Constellation backlinks to the forum URI (exact), then the indexer (exact), then relay `listReposByCollection` (coarse), then Jetstream (live only).
   - Approval-mode forums need none of these.
8. **Hotlinking `getBlob` → fetch it and check it against the CID.** This gives real integrity at no extra cost and stops tracking pixels.
   - CDNs that transcode images break the CID check, so they are allowed only as unverified fallbacks.
   - Video is the exception: it streams directly and is marked unverified.
9. **`lessdugin web` needs a fixed port.** IndexedDB and localStorage are tied to the origin, and the port is part of the origin.
   - The port defaults to 47778 and is saved in config. If it is busy, the command fails rather than silently picking another one.
   - The server checks the Host header to block DNS rebinding.
   - Browser and CLI tokens are never shared.
10. **NSIDs stay flat (`online.7778777.lessdugin.<name>`) with no sub-groups.**
    - Lexicon resolution is not hierarchical, so each sub-group would need its own `_lexicon` TXT record.
    - The permission set `online.7778777.lessdugin.authFull` covers the whole group.
11. **`include:` scope needs a fallback chain.** Permission-set resolution needs published lexicons and a recent PDS, and the dev-env cannot resolve our domain.
    - Order: `include:online.7778777.lessdugin.authFull` → an explicit `repo?collection=…` list → `transition:generic`.
    - Blob scopes are always added: `blob:image/* blob:video/*`.
12. **Short numeric refs are stored per TTY.** The last listing is keyed by `ttyname()` (falling back to the parent PID), so two terminals do not clobber each other.
13. **Revisions.** Each edit is one `applyWrites` call that writes a snapshot record and updates the post.
    - Snapshots larger than 16 KB keep the previous body as a `text/markdown` blob rather than inline, to limit repo growth.
    - In the MVP, comments get only an `editedAt` field.
14. **Reports stay out of the MVP.** Reports must never be public records. Sealed reports, encrypted with X25519 `crypto_box` to keys moderators publish, come in phase P9.

---

## 2. Monorepo layout

```
lessdugin/
├─ Cargo.toml                 # workspace, [workspace.dependencies], profiles, lints
├─ rust-toolchain.toml        # 1.97, targets wasm32-unknown-unknown, rustfmt+clippy
├─ .cargo/config.toml         # alias xtask = "run -p xtask --"
├─ clippy.toml                # disallowed-methods: SystemTime::now, Instant::now, thread::sleep, Utc::now
├─ deny.toml                  # license allowlist + advisories
├─ package.json  pnpm-workspace.yaml  biome.json  tsconfig.base.json
├─ lexicons/online/7778777/lessdugin/*.json        # source of truth (CC0)
├─ crates/
│  ├─ atp/        lessdugin-atp       # protocol primitives, wasm-safe
│  ├─ lexicon/    lessdugin-lexicon   # generated types + validate + rkey derivation
│  ├─ markdown/   lessdugin-markdown  # parse → sanitize → html/mathml | styled lines | text
│  ├─ core/       lessdugin-core      # domain + engine over Host
│  ├─ native/     lessdugin-native    # NativeHost: reqwest, SQLite KV, auth, DNS, WS, media decode
│  ├─ tui/        lessdugin-tui       # ratatui app (lib)
│  ├─ cli/        lessdugin           # bin `lessdugin` (cli + tui + web)
│  ├─ wasm/       lessdugin-wasm      # cdylib, wasm-bindgen + tsify
│  ├─ indexer/    lessdugin-indexer   # bin, axum
│  └─ testkit/    lessdugin-testkit   # publish=false: MemHost, FakePds, Devnet, vector runner, generators
├─ xtask/
├─ packages/
│  ├─ engine/     @lessdugin/engine   # wasm pkg + worker + JsHost (IDB/fetch/ws) + typed client
│  └─ e2e/        @lessdugin/e2e      # devnet.mjs (@atproto/dev-env), Playwright specs, fixtures
├─ web/           @lessdugin/web      # React SPA
├─ testdata/
│  ├─ vectors/{syntax,datamodel,rkey,lexicon,markdown,media,thread,moderation,karma,rank,search}/*.json
│  └─ interop/    # vendored subset of bluesky-social/atproto-interop-tests (+ LICENSE)
├─ docs/{spec/, adr/, guide/}
├─ deploy/{site/_headers, site/oauth/*.json, indexer/Dockerfile, compose.e2e.yml}
└─ .github/workflows/{ci.yml, e2e.yml, release.yml, pages.yml}
```

---

## 3. Crates

| Crate | wasm-safe | Responsibility | Key deps |
|---|---|---|---|
| `lessdugin-atp` | yes | Syntax types and validators (Did, Handle, Nsid, AtUri, RecordKey, Tid, Datetime); data model (JSON ⇄ DAG-CBOR with `$link`, `$bytes`, blob); CID (dag-cbor 0x71, raw 0x55); CARv1 reader; MST proof walk; commit decode and signature verify (k256/p256, low-S); did:key; DID doc parsing; PLC audit-log verify (post-MVP); typed XRPC request/response structs for the subset we use | `cid 0.11`, `serde_ipld_dagcbor 0.7`, `sha2`, `k256 0.14`, `p256 0.14` (ecdsa verify only, feature `verify`), `multibase`, `data-encoding`, `serde`, `serde_json`, `chrono` (no defaults), `thiserror 2` |
| `lessdugin-lexicon` | yes | Generated owned serde structs for every record and query; `validate()` with lengths, graphemes, formats and knownValues; rkey derivation; forum-scope checks; optional `tsify` derives | `lessdugin-atp`, `unicode-segmentation`, `tsify 0.5` (opt) |
| `lessdugin-markdown` | yes | pulldown-cmark 0.13 events → sanitizer → (a) HTML + MathML, (b) `StyledDoc` (blocks of styled spans and image placeholders) for terminals, (c) plain text, ToC, word count, links, mentions, media refs, excerpt, quote-anchor resolution | `pulldown-cmark` (default-features=false), `math-core 0.9` (feature `math`), `ammonia 4.2` (feature `ammonia`, native only), `unicode-width` |
| `lessdugin-core` | yes | `Host`/`KvStore` traits, key encoding, records store, indices, identity resolver, discovery, sync, outbox, ForumModel (moderation fold, karma, scoring), thread assembly, ranking, search, notifications, local state, render cache, verification orchestration, events, view DTOs | atp, lexicon, markdown, `futures`, `http 1`, `url`, `tracing`, `img-parts 0.4`, `imagesize 0.15` |
| `lessdugin-native` | no | `NativeHost`: reqwest 0.13 (rustls) with SSRF guard; `SqliteKv` (rusqlite 0.40 bundled, WAL, busy_timeout); hickory-resolver 0.26 for TXT; tokio-tungstenite 0.30; app-password auth; OAuth through jacquard-oauth `=0.12.2`; session files with `fd-lock`; config and paths (`directories 6`); image decode for orientation and downscale (`image`) | tokio |
| `lessdugin-tui` | no | Elm-style ratatui 0.30 app, crossterm 0.29, ratatui-image 11 (`default-features=false, features=["image-defaults","crossterm","tokio"]`, no libchafa) | core, native |
| `lessdugin` (cli) | no | clap 4.6 (derive, env) + clap_complete; human, ANSI and JSON output; `$EDITOR` drafts (serde-saphyr 2 YAML frontmatter); `tui`; `web` (axum 0.8 + rust-embed 8) | core, native, tui |
| `lessdugin-wasm` | target | wasm-bindgen 0.2.129, wasm-bindgen-futures, js-sys, tsify 0.5 + serde-wasm-bindgen 0.6, `getrandom 0.4 {wasm_js}` (enabled only here) | core |
| `lessdugin-indexer` | no | axum 0.8, tower-http (cors, compression, trace), Jetstream ingest and backfill on the same engine, SQLite KV plus FTS5 tables, Prometheus metrics | core, native |
| `lessdugin-testkit` | both | `MemHost` (in-memory KV, scripted clock), `FakePds` (in-memory repos speaking the XRPC subset, no proofs), `Devnet` launcher, vector runner, synthetic forum generator | — |

**Rules enforced by CI:**
- `cargo clippy --target wasm32-unknown-unknown -p lessdugin-{atp,lexicon,markdown,core,wasm}` must pass.
- `clippy.toml` `disallowed-methods` applies to those crates.
- A compile-time `assert_send` test on all public engine futures runs on native targets.

**Feature flags:**
- atp: `verify`
- markdown: `math` (default), `ammonia`
- core: `verify` (default), `search` (default)
- wasm: `verify`, `math`, `vectors` (test builds only), `debug-panic`
- native: `oauth` (default)
- cli: `tui` (default), `web` (default)

**Release profiles:**
- `[profile.release]`: thin LTO, strip.
- `[profile.wasm]`: inherits release, `opt-level="z"`, fat LTO, `codegen-units=1`, then `wasm-opt -Oz`.

---

## 4. The engine

### 4.1 Host abstraction (load-bearing)

```rust
// lessdugin-core::host
#[cfg(not(target_arch = "wasm32"))] pub trait MaybeSend: Send {}  // blanket impls
#[cfg(target_arch = "wasm32")]      pub trait MaybeSend {}
pub trait MaybeSendSync: MaybeSend /* + Sync on native */ {}

pub enum Target { Url(url::Url), UserPds { account: Did, path_and_query: String } }
pub struct HttpRequest  { pub method: http::Method, pub target: Target, pub headers: Vec<(String,String)>, pub body: Option<Vec<u8>> }
pub struct HttpResponse { pub status: u16, pub headers: Vec<(String,String)>, pub body: Vec<u8> }

pub trait Host: MaybeSendSync + 'static {
    type Kv: KvStore;
    fn kv(&self) -> &Self::Kv;
    fn http(&self, req: HttpRequest) -> impl Future<Output = Result<HttpResponse, HostError>> + MaybeSend;
    fn resolve_txt(&self, name: &str) -> impl Future<Output = Result<Vec<String>, HostError>> + MaybeSend; // wasm: DoH
    fn websocket(&self, url: &str) -> impl Future<Output = Result<WsStream, HostError>> + MaybeSend;      // Stream<Item=WsMsg>
    fn sleep(&self, d: Duration) -> impl Future<Output = ()> + MaybeSend;
    fn now_ms(&self) -> i64;
    fn fill_random(&self, buf: &mut [u8]);
}

pub trait KvStore: MaybeSendSync {
    fn get_many(&self, keys: Vec<Vec<u8>>) -> impl Future<Output = Result<Vec<Option<Vec<u8>>>>> + MaybeSend;
    fn scan(&self, range: KeyRange, dir: Dir, limit: u32) -> impl Future<Output = Result<Vec<(Vec<u8>, Vec<u8>)>>> + MaybeSend;
    fn write(&self, batch: Vec<KvOp /* Put|Delete|DeleteRange */>) -> impl Future<Output = Result<()>> + MaybeSend; // atomic
}
```

- **`Target::UserPds`** is how signed-in calls are expressed. The core encodes the XRPC itself, and the host only adds authentication:
  - natively: DPoP or Bearer plus refresh;
  - in the browser: `OAuthUserAgent.handle(path, init)` on the main thread.
- **The core never spawns tasks.** Long-running behaviour (`live_tail`, `outbox_pump`, `background_verify`) is returned as futures; the host runs them with `tokio::spawn` or `spawn_local`.
- **Interior state** uses `std::sync::Mutex`. A lint rule forbids holding a guard across an `.await`.
- **Native backends:**
  - `SqliteKv` is one table: `kv(k BLOB PRIMARY KEY, v BLOB) WITHOUT ROWID`.
  - Each batch is one transaction.
  - WAL mode lets the CLI and TUI run at the same time.
- **Browser backend:**
  - One IDB object store with ArrayBuffer keys, which IDB compares bytewise.
  - Each batch is one readwrite transaction.
  - Reverse scans use a `prev` cursor.
  - The SPA calls `navigator.storage.persist()`.

### 4.2 Key-value schema

Keys are tuple-encoded so they sort correctly as bytes: strings are escaped with 0x00/0xFF; integers are big-endian with the sign bit flipped.

| Prefix | Key → value | Notes |
|---|---|---|
| `rec` | did, coll, rkey → `{cid, cbor, source, verify_level, first_seen, fetched_at}` | Ground truth; everything else is derived |
| `ix.forum` | forum, kind, created, uri | Posts per forum |
| `ix.child` | parent, created, uri | Thread tree |
| `ix.root` | post, created, uri | All comments of a post |
| `ix.subj` | subject, coll, author → uri | Votes, reacts, tag relations, mod actions |
| `ix.author` | did, coll, created, rkey | Per-author records |
| `ix.tag` | forum, tag, post | Tags |
| `ix.seq` | seq, pos | Sequences |
| `ix.mod` | forum, created, uri | Moderation log |
| `ix.roster` | forum, did | Members |
| `ix.part` | forum, did → `{sources, last_seen}` | Participants |
| `fts` | forum, token, uri → tf | Core search (all clients) |
| `sync` | did → `{rev, collections, listed_at}`; `sync.js` | Jetstream cursor per scope |
| `id` | did → doc, handle → did | TTL 24h; re-resolved on failure |
| `out` | seq → `PendingWrite` | Outbox, overlaid on query results |
| `loc` | drafts, read marks, bookmarks, subscriptions, settings, aliases, `last_listing:<tty>`, notification read state | Local only |
| `notif` | account, ts, id | Notifications |
| `rcache` | cid, renderer_version → rendered HTML + StyledDoc | Render cache |
| `meta` | `schema_version` | A version bump re-indexes from `rec` |

### 4.3 Sync pipeline

**Discover → Fetch → Check → Store → Index → Recompute ForumModel → Notify → Emit.**

**Discovery sources.** All implement `Discovery` and return candidate DIDs or URIs as hints:
- forum declaration (owner repo) and moderator repos;
- the roster (approval mode);
- Constellation `getBacklinks(subject=forum, source=<coll>:forum)`;
- the indexer `listParticipants`;
- relay `listReposByCollection`;
- Jetstream (`wantedCollections=online.7778777.lessdugin.*`, optionally `wantedDids`).

**Fetching per DID:**
1. Resolve the DID document to find the PDS.
2. `getLatestCommit`: skip if the rev is unchanged and the last listing is fresh.
3. `describeRepo` to learn which collections exist.
4. `listRecords` for each present lessdugin collection (limit 100, paged).
5. Filter the results by the record's `forum` field.

On-demand single records use `getRecord`. Jetstream events become refetch jobs: they are coalesced, limited to 8 concurrent requests per PDS and 32 in total, and a delete only takes effect after a `RecordNotFound` confirms it.

**Checking on ingest:**
1. Run lexicon `validate()`.
2. Apply the deterministic-rkey rule.
3. Check that the forum scope is consistent.
4. Convert JSON to DAG-CBOR and recompute the CID; it must equal the CID the PDS reported. Mismatches are dropped and logged.

**Verification levels.** These are shown in the UI. The default is `pds`; stricter modes are `proof` (verify displayed items in the background) and `strict` (hide items until proven).

| Level | Meaning | Trust root |
|---|---|---|
| `hint` | Known only from relay, Jetstream, Constellation or the indexer; shown as "unconfirmed" | — |
| `pds` | Fetched from the author's PDS over TLS | PDS operator + DID resolution |
| `proven` | `sync.getRecord` CAR checked: commit signature against the DID doc `#atproto` key, MST path, CID. Verified commits are cached per (did, rev) | DID resolution |
| `plc` (post-MVP) | `proven`, plus the PLC audit log chain verified back to the genesis hash | Nothing beyond the DID itself |

**ForumModel.** This is an in-memory snapshot per forum, rebuilt with a debounce when the forum's generation counter changes. It holds:
- the moderation fold: removed, hidden, banned, locked, pinned, frontpage and curated state, using the current moderator list; actions by removed moderators are void;
- karma (fixed point) and the score and agreement of each item;
- tag relevance;
- membership.

Queries read from KV, overlay pending outbox writes, apply the ForumModel, rank, and paginate.

### 4.4 Writes and events

**Writes.** Each user action becomes a `PendingWrite` containing a list of `applyWrites` operations, stored in `out` before any network call (so it survives `kill -9`).
- The pump runs batches with backoff on 429. It honours the `ratelimit-*` headers and the 5000-points-per-hour budget.
- Edits use `swapRecord`; a conflict produces a `WriteState::Conflict` event.
- Vote toggles on the same derived rkey are merged before sending.
- Blob CIDs are computed locally (raw codec, sha256), so the editor previews `cid:` media before upload. After upload, the returned ref must equal the local CID.

**Events.** Delivered through `Engine::events()` (an unbounded mpsc per subscriber):
- `Changed{topics}`: topic strings such as `forum:<uri>`, `thread:<uri>`, `post:<uri>`, `user:<did>`, `notifications`, `drafts`, `outbox`;
- `SyncProgress{scope, done, total}`;
- `Write{id, state}`;
- `Auth{account, state}`;
- `Warning{…}`.

### 4.5 Public engine API (shared by CLI, TUI and wasm)

- **Accounts and identity:** `resolve_actor`, `accounts`, `set_active_account`.
- **Forums:**
  - Read: `forum_add`, `forum_get`, `forums`.
  - Owner actions: `forum_create`, `forum_update`, `join`, `leave`, `roster_*`.
- **Sync:** `sync_forum(scope, opts) -> SyncReport`, `sync_thread`, `live_tail(forum)`, `outbox_pump()`.
- **Queries, which return view DTOs:**
  - posts and threads: `list_posts(ListPostsQuery{forum, sort, filter: frontpage|all|curated|tag, since, cursor})`, `get_post`, `get_thread(uri, ThreadOpts{sort, unread_only})`, where the thread is returned flattened in pre-order with depth;
  - tags and sequences: `tags`, `tag`, `sequences`, `sequence`;
  - people and search: `user`, `search`, `notifications`;
  - moderation: `mod_queue`, `mod_log`;
  - local state: `drafts`, `bookmarks`.
- **Writes:**
  - Content: `publish(Draft)`, `edit`, `delete`, `comment`.
  - Reactions: `vote(subject, Axis::Karma|Agree, VoteValue)`, `react`.
  - Organising: `tag_*`, `seq_*`, `mod_action`, `bookmark`, `subscribe`.
  - Media: `prepare_media(bytes, mime) -> MediaRef`.
- **Local state:** `mark_read`, `save_draft`, `settings`.
- **Pure functions:** `render(md, media, RenderOpts)`, `parse_ref`, `verify(uri, depth) -> VerifyReport`.
- **View DTOs** (`ForumView`, `PostView`, `ThreadView`, `CommentNode`, `UserView`, `ScoreView`, `ModState`, `Verification`, `NotificationView`) live in core with `serde` and `#[cfg_attr(feature="tsify", derive(Tsify))]`.
  - The CLI's `--json` output and the SPA's types are the same schema, so cross-client comparisons in e2e tests are simple diffs.

---

## 5. Protocol layer and authentication

**XRPC subset in `lessdugin-atp::xrpc`.** About 15 endpoints:
- `repo`: getRecord, listRecords, describeRepo, createRecord, putRecord, deleteRecord, applyWrites, uploadBlob;
- `sync`: getRecord (CAR), getBlob, getLatestCommit, listReposByCollection;
- `identity`: resolveHandle (hint);
- `server`: getSession;
- Constellation getBacklinks;
- the indexer queries.

PDS XRPC calls are always made against the PDS listed in the DID document.

**Identity** lives in core:
- DID → document: `plc.directory` (configurable) or did:web.
- Handle → DID: DNS TXT `_atproto.` through the host (hickory natively, DoH JSON in the browser, defaulting to `https://cloudflare-dns.com/dns-query`), then `/.well-known/atproto-did`.
- The handle is shown only if it verifies in both directions; otherwise the DID is shown with a marker.
- Slingshot `resolveMiniDoc` is a speed hint only.
- Dev mode allows overriding the PLC URL and the handle resolver (`pds:<url>`) and allows http, but only behind `LESSDUGIN_DEV=1` or `--dev`.

**Native authentication** (`lessdugin-native::auth`, facade `AuthSession { async fn send(HttpRequest) }`):
1. **OAuth loopback (default).** jacquard `resume_or_login_with_local_server` with client_id `http://localhost?redirect_uri=http://127.0.0.1:<port>/callback&scope=…`.
2. **OAuth with hosted native metadata.** Used automatically after `invalid_client`, or with `--oauth-client hosted`. Metadata at `https://lessdugin.7778777.online/oauth/cli-client-metadata.json` declares `application_type: native` and `redirect_uris: ["http://127.0.0.1/callback"]`, where the port is ignored when matching.
3. **App password** (`--app-password`). For SSH or headless use, and for PDSes without OAuth. Supports `authFactorToken` 2FA prompts.

Session handling:
- Sessions are stored at `$DATA/sessions/<did>.json` with mode 0600.
- Refresh happens under an exclusive `fd-lock`, because refresh tokens are single-use and the CLI, TUI and `web` may run at once.
- Access-token expiry is read from `expires_in`.
- The OAuth session lasts at most about 2 weeks; after that, `Auth{Expired}` produces a re-login prompt.

**Browser authentication:** @atcute/oauth-browser-client 5.1 with @atcute/identity-resolver 2.0. The client_id is the hosted metadata or the loopback form in `web` mode. The callback is handled before the router starts (see 8.2).

**Scopes** follow the fallback chain from section 1, point 11. The hosted metadata `scope` lists every variant, because the metadata scope is the maximum a client may request.

---

## 6. Markdown and rendering pipeline (`lessdugin-markdown`)

**Parser options.** One named profile, `lessdugin-md/1`: pulldown-cmark with TABLES, FOOTNOTES (GFM style), STRIKETHROUGH, TASKLISTS, MATH, GFM alerts, HEADING_ATTRIBUTES off, smart punctuation off. The spec lives in `docs/spec/07-markdown.md`.

**Sanitizing at the event level:**
- `Html`/`InlineHtml` → escaped text, so content is visible rather than silently lost.
- Allowed link schemes: `https`, `http`, `mailto`, `at`, and `#` fragments. Anything else becomes plain text.
- `at://` links → `<a href="#/at/…" data-at-uri>`. Mentions `[@h](at://did)` → `class="mention" data-did`.
- Images:
  - `cid:<cid>` resolves against the record's media list, giving `<img data-cid alt width height>` (dimensions from the aspect ratio). A missing entry shows a placeholder.
  - External `https` images become click-to-load placeholders by default (`RenderOpts.external_images = Placeholder | Link | Inline`).
- Every generated `id` is prefixed `md-` to prevent DOM clobbering.
- External links get `rel="nofollow ugc noopener noreferrer"`.
- Code fences keep a sanitized `language-*` class; there is no syntax highlighting in the MVP.

**Math.** `math-core` turns TeX into MathML Core. On error the source is shown as `<code class="math-error">`. The SPA ships `mathmlfixes.css` and an OFL math font (a STIX Two Math subset, about 200 KB woff2) through `@font-face`, which browsers fetch only when a page actually contains math.

**Outputs:** `Rendered { html, styled: StyledDoc, text, toc, word_count, read_minutes, links, mentions, media_used, excerpt, anchors }`.
- `StyledDoc` (blocks and spans with semantic styles) is shared by the TUI (mapped to ratatui `Line`s) and the CLI (mapped to ANSI with OSC 8 hyperlinks).
- Quote anchors use W3C TextQuoteSelector (exact, prefix and suffix) over the `text` projection. They are rendered as `<mark data-anchor>` or styled spans; this is core-ready even though the margin UI comes after the MVP.

**Defence in depth:**
- Native HTML export also runs ammonia, with a test that `ammonia(html) == html` over vectors and proptest input.
- The SPA runs DOMPurify, configured with exactly the renderer's tag and attribute set, as a Trusted Types policy.
- A vector test asserts `DOMPurify(html) === html` for every case.

**Versioning.** `RENDERER_VERSION` is part of the render-cache key.

---

## 7. Media pipeline

The media code lives in core (`media` module, wasm-safe) so the CLI and SPA strip metadata identically.

1. Sniff the type and get dimensions with `imagesize`.
2. Remove metadata losslessly with `img-parts`: JPEG APP1 (EXIF/XMP), PNG eXIf/tEXt/iTXt/zTXt, WebP EXIF/XMP.
3. **If EXIF orientation is not 1, the image must be physically rotated**; otherwise stripping the tag rotates the picture.
   - Native: re-encode with the `image` crate.
   - SPA: `createImageBitmap(file, {imageOrientation:'from-image'})` → canvas → re-encode.
4. Compute the raw CID.
5. Upload at publish time, never at draft time, because unreferenced blobs are garbage-collected after about an hour. Retry on `BlobTooLarge`; native can downscale.

Other details:
- **Video** is uploaded as-is after a size check, with a warning that container metadata is not stripped.
- **Display:** the client fetches `getBlob` from the author's PDS, checks the CID, and caches the bytes in `$CACHE/blobs/<cid>` (native) or the IDB `blobs` store (browser) under an LRU limit.

---

## 8. wasm bindings and the `@lessdugin/engine` package

### 8.1 Rust bindings (`lessdugin-wasm`)

```rust
#[wasm_bindgen] extern "C" {
  pub type JsHost;   // implemented in TS (worker side)
  #[wasm_bindgen(method, catch)] async fn http(this:&JsHost, req: JsValue) -> Result<JsValue, JsValue>;
  #[wasm_bindgen(method, catch)] async fn kvGetMany(this:&JsHost, keys: js_sys::Array) -> Result<JsValue, JsValue>;
  #[wasm_bindgen(method, catch)] async fn kvScan(this:&JsHost, lo:&[u8], hi:&[u8], reverse:bool, limit:u32) -> Result<JsValue, JsValue>;
  #[wasm_bindgen(method, catch)] async fn kvWrite(this:&JsHost, ops: js_sys::Array) -> Result<JsValue, JsValue>;
  #[wasm_bindgen(method, catch)] async fn resolveTxt(this:&JsHost, name:&str) -> Result<JsValue, JsValue>;
  #[wasm_bindgen(method)] fn openWs(this:&JsHost, url:&str, on_msg:&Closure<dyn FnMut(String)>, on_close:&Closure<dyn FnMut()>) -> JsValue;
  #[wasm_bindgen(method)] fn now(this:&JsHost) -> f64;
  #[wasm_bindgen(method, catch)] async fn sleep(this:&JsHost, ms:u32) -> Result<JsValue, JsValue>;
}
#[wasm_bindgen] pub struct WasmEngine(Engine<WasmHost>);
// methods mirror §4.5 1:1; args/returns are #[derive(Tsify)] DTOs (into_wasm_abi/from_wasm_abi)
// + free fns: renderMarkdown, parseRef, validateRecord, deriveRkey, runVector (feature "vectors")
// + subscribe(cb: js_sys::Function) -> Subscription (spawn_local loop forwarding Event DTOs)
```

- One build target: `wasm-bindgen --target web`. Node tests load it with `initSync({module: readFileSync(...)})`.
- The `xtask wasm` command fails if the wasm-bindgen CLI version differs from the `wasm-bindgen` version in `Cargo.lock`.
- **Size budget:** a target of 900 KB gzipped, with CI failing above 1.1 MB. Each feature's contribution is recorded by `xtask wasm --report` (using twiggy).
- **Panics** trap the instance. A supervisor in `client.ts` restarts the worker; all state is in IDB, so restarting is safe. `console_error_panic_hook` is enabled only with `debug-panic`.

### 8.2 TypeScript package

- **`worker.ts`:** loads the wasm, builds the `JsHost` (IDB through `idb` 8, `fetch`, `WebSocket`, DoH), and calls `Comlink.expose(engine)`.
  - Signed-in requests (`Target::UserPds`) are forwarded to a main-thread `AuthBridge` callback (`Comlink.proxy`) that calls the atcute `OAuthUserAgent`.
  - Blob bytes are passed with `Comlink.transfer`.
- **`client.ts`:** `startEngine({config, auth}) → EngineClient` (typed via tsify `.d.ts`), plus `onEvent`.
- **Security note.** atcute stores DPoP keys exportably in localStorage, so cross-site scripting would expose the session. That makes strict CSP, Trusted Types and the sanitizer non-negotiable.

---

## 9. CLI (`lessdugin`)

**Global flags:** `--account <handle|did>`, `--forum <alias|ref>`, `--json`, `--offline` (no network), `--sync` (force refresh), `--verify <pds|proof|strict>`, `--indexer <url|none>`, `--data-dir`, `--color <auto|always|never>`, `--dev`, `-q/-v`.

**Behaviour:**
- A bare `lessdugin` in an interactive terminal with an account opens the TUI; otherwise it prints help.
- Read commands refresh their scope first if it is stale (5 minutes by default), showing a spinner on stderr.

```
login [--app-password] [--oauth-client loopback|hosted] [handle]   logout   whoami   accounts [use <a>]
forum  ls | add <ref> [--alias a] | rm | show | create <slug> | edit | join | leave
       members [approve|remove <actor>] | mods [add|rm <actor>]
ls     [forum] [--sort magic|top|top-adjusted|new|old|recent-comments] [--tag t] [--frontpage|--all|--curated] [--limit n]
read   <ref> [--sort ...] [--unread] [--no-comments] [--raw|--html] [--no-pager]
post   [file.md] [--forum] [--title] [--tag t]... [--link url] [--draft]      # $EDITOR when no file
edit   <ref>          delete <ref>        retract <ref>
comment <ref> [file|-|-m text]            # reply to post or comment; template has quoted parent below git-style scissors line
vote   <ref> up|down|strong-up|strong-down|clear
agree  <ref> agree|disagree|strong-agree|strong-disagree|clear
react  <ref> <react> [--quote "text"] [--anti] [--remove]
tag    <post-ref> add|rm|up|down <tag>    tags [forum] | tags show <tag> | tags edit <tag>
seq    ls | show <ref> | create | add <seq> <post> [--at n] | rm | mv
bookmark add|rm|ls     subscribe|unsubscribe <ref|forum|tag|user|seq>    subs
notifications [--all] [--mark-read]        search <query> [--forum] [--author] [--remote]
user   <actor> [--forum]
mod    hide|unhide|remove|restore|ban|unban|lock|unlock|pin|unpin|frontpage|unfrontpage|curate|uncurate|approve <ref|actor> [--reason]
mod    queue | log
draft  ls | edit <id> | rm <id> | publish <id>
media  add <file> | open <cid>
sync   [forum] [--full] [--follow]          # --follow: live tail; with --json emits JSONL events
verify <ref> [--deep]                       export [forum|--mine] [--format json|md|html] [--out dir]
tui [forum]    web [--port] [--no-open]    config get|set|edit    completions <shell>    resolve <handle|did>
```

**Refs** can be:
- `3` (item from the last listing in this terminal);
- an `at://` URI;
- any SPA URL (`https://…/#/p/<did>/<rkey>` or `http://127.0.0.1:47778/#/…`);
- `<handle>/<rkey>`.

The `#` character is never used for refs, because shells treat it as a comment.

**Draft files** live in `$DATA/drafts/<id>.md` with YAML frontmatter (parsed by serde-saphyr): `forum, title, kind (post|link|question), url, tags, sequence`.
- Local image paths such as `![](./a.png)` go through the media pipeline at publish time and are rewritten to `cid:`.
- Validation errors reopen the editor, with the errors written as YAML comments in the frontmatter.
- Before publishing, a summary (forum, tags, media, word count) asks to confirm with `[y/N/e]`.

**Output:**
- Human output wraps to terminal width and pipes `read` through `$PAGER` (`less -R`).
- `--json` prints the core view DTOs as a stable, versioned schema.
- Exit codes: 0 ok, 1 error, 2 usage, 3 auth required, 4 not found, 5 conflict, 6 partial sync.

**Paths:**
- Config: `~/.config/lessdugin/config.toml`, with `[forums.<alias>]`, `[network]` (plc, relays, jetstream, constellation, indexer, doh), `[verify]`, `[ui]`, `[tui.keys]`, `[web] port`.
- Data: `~/.local/share/lessdugin/{store.sqlite, sessions/, drafts/}`.
- Cache: `~/.cache/lessdugin/blobs/`.

---

## 10. TUI (`lessdugin-tui`)

**Architecture:**
- `State` plus a stack of `Screen`s; `update(state, Action) -> Vec<Effect>`.
- Effects run on tokio and come back as Actions; `view(&State, Frame)` is pure.
- Engine events map to Actions, for live updates from the live-tail and outbox-pump tasks.
- `$EDITOR` runs with the terminal suspended (leave the alternate screen and raw mode, then restore).
- Images use ratatui-image `Picker::from_query_stdio()` with `ThreadProtocol`, falling back to halfblocks over SSH.

**Screens:**

| Screen | What it shows |
|---|---|
| Home | Forums with unread counts, notifications badge, sync status |
| Forum | Post list with sort tabs, filter tabs (frontpage/all/curated), tag filter |
| Post | Rendered body, ToC jump, inline images, footnotes |
| Thread | Collapsible comment tree, unread highlighting, score and agreement, react chips |
| Tags / Tag | Tag list and tag page |
| Sequences / Sequence | Ordered chapters, next/previous |
| User | Profile |
| Notifications | Notification list |
| Search | Results |
| Mod queue / log | Moderation |
| Drafts | Local drafts |
| Overlays | `?` help, `:` command line that accepts any CLI subcommand, react palette, confirmation dialogs |

**Default keys** (remappable in `[tui.keys]`):

| Group | Keys |
|---|---|
| Navigation | `j/k`, `↑/↓` move; `g/G` top/bottom; `Ctrl-d/u` half page; `Enter`/`l` open; `q`/`Esc`/`h` back; `Tab` cycle focus |
| Threads | `Space` collapse/expand; `p` parent; `]`/`[` next/previous unread; `}`/`{` next/previous sibling |
| Voting | `u`/`U` up/strong up; `d`/`D` down/strong down; `a`/`A` agree/strong; `z`/`Z` disagree/strong; `0` clear |
| Writing | `x` react palette; `r` reply; `c` new comment on post; `n` new post; `e` edit own; `Del` delete own |
| Lists and organising | `s` cycle sort; `f` filter; `t` tags; `b` bookmark; `S` subscribe |
| Other | `m` moderation menu; `o` open in browser (`lessdugin web` URL); `y` yank URI; `R` sync; `/` search, `n`/`N` results; `?` help |

---

## 11. SPA (`web/`)

### 11.1 Stack
- React 19.3, Vite 8, TypeScript 7 (tsgo for type-checking), Biome 2.5 for lint and format (it does not depend on the TS compiler API).
- TanStack Router 1.x with `createHashHistory` and a code-based route tree; search parameters are typed (sort, filter, tag).
- TanStack Query 5 as the async cache over engine RPC. Engine `Changed{topics}` events map to `invalidateQueries`; query keys follow topic names.
- @tanstack/react-virtual 3 for post lists and the flattened comment tree.
- CodeMirror 6 (`codemirror`, `@codemirror/lang-markdown`) with a live preview from the worker, debounced 150 ms.
- `radix-ui` primitives (popover for hover previews, dropdown, dialog, tooltip), CSS modules with CSS custom properties (light and dark), DOMPurify 3.4, Comlink 4.4.

### 11.2 Routes (hash)
- `#/` — home: followed forums, discovered forums, recent activity.
- `#/f/:owner/:slug` — forum; `?sort=&filter=&tag=`.
- `#/f/:owner/:slug/{new,tags,t/:tag,seqs,mod,settings,members}`.
- `#/p/:author/:rkey` — post and thread (the forum comes from the record).
- `#/c/:author/:rkey` — comment permalink, shown inside its thread.
- `#/s/:author/:rkey` — sequence.
- `#/u/:actor` — user.
- `#/at/*` — at-uri resolver.
- `#/notifications`, `#/drafts`, `#/bookmarks`, `#/search?q=`, `#/settings`, `#/login`.

URLs accept DIDs or handles; internally everything is canonicalized to DIDs.

### 11.3 Behaviour
- **Rendered content:** a container whose `innerHTML` is set through Trusted Types, with one delegated listener for:
  - `data-at-uri` hover previews;
  - `data-cid` images (fetched and verified, then shown as an object URL);
  - placeholder clicks;
  - footnotes shown as sidenotes on wide screens.
- Keyboard shortcuts mirror the TUI.
- Badges show verification status and pending writes.
- Settings cover: accounts, verification level, discovery sources and indexer, external images, export/import of local state (JSON), and storage persistence.

### 11.4 OAuth and hosting
- **Callback:** `main.tsx` checks `location.hash` for `code` or `error` together with `state`/`iss` before mounting the router, calls `finalizeAuthorization`, then `history.replaceState` back to `#/`.
- **Config:** the SPA loads `./lessdugin-config.json`, which is static on the hosted site and generated by `lessdugin web`.
  - `{oauth:{mode:"hosted"|"loopback"}, defaults:{…}}`.
  - In loopback mode the client_id is `http://localhost?redirect_uri=http%3A%2F%2F127.0.0.1%3A<port>%2F&scope=…`.
- **Static deploy:**
  - GitHub Pages via `pages.yml`, triggered on release tags.
  - Site contents: `index.html`, `assets/*`, `oauth/client-metadata.json` (`application_type: web`, `token_endpoint_auth_method: none`, `dpop_bound_access_tokens: true`, `redirect_uris: ["https://lessdugin.7778777.online/"]`), `oauth/cli-client-metadata.json`, `lessdugin-config.json`, `_headers`, and the source/licence link required by AGPL.
  - Pages cannot set headers, so CSP goes in a `<meta>` tag plus a JS frame guard that refuses to render the UI inside an iframe. `_headers` is provided for Cloudflare Pages and Netlify.
- **CSP:**
  ```
  default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self'; font-src 'self';
  img-src 'self' blob: data: https:; media-src 'self' blob: https:; connect-src 'self' https: wss:;
  object-src 'none'; base-uri 'none'; form-action 'none'; require-trusted-types-for 'script'; trusted-types lessdugin-md dompurify
  ```
  (`frame-ancestors 'none'` is added only via headers.)
- **`lessdugin web`:**
  - axum on `127.0.0.1:<port>` only, serving rust-embed assets with the same CSP as headers.
  - Rejects any Host header other than `127.0.0.1:<port>` or `[::1]:<port>`.
  - Opens the browser.
  - `build.rs` embeds `web/dist` if present, otherwise a page explaining how to build it; `xtask dist` always builds it first.

---

## 12. Indexer (`lessdugin-indexer`)

**Purpose and model.**
- The indexer runs the same `Engine<NativeHost>` continuously.
- Every client treats it as both a discovery source and a query accelerator; nothing it returns is trusted.

**Ingest:**
- Jetstream v2, connecting to us-east and failing over to us-west, filtered to `online.7778777.lessdugin.*`.
- The cursor is persisted and rewound 5 s on reconnect.
- Events → refetch from the PDS at `pds` level by default; `--verify proof` upgrades every record.
- Backfill: `listReposByCollection` for each collection, then the engine syncs each repo.
- Poll mode (`listRepos` on the given PDSes) exists for dev-env and for air-gapped setups.

**Query lexicons** (flat names, unauthenticated, CORS `*`):
- `online.7778777.lessdugin.{describeIndexer, listForums, listParticipants, listPosts, getThread, searchPosts, getKarma, listBacklinks}`.
- Responses are skeletons `{uri, cid, indexedAt}` plus a cursor.
- `hydrate=true` adds `records{uri:{cid, value}}`. Clients recompute the CID; the value stays a hint until confirmed by the PDS or a proof.
- `getKarma` returns a snapshot plus `inputsHash` (the SHA-256 of the sorted vote CIDs).

**Storage:** `SqliteKv` plus FTS5 tables for search. Query results are cached for 30 s.

**Operations:**
- An SSRF guard (a resolver that blocks private, loopback and link-local ranges; https only) because DID documents choose the URLs.
- tower-governor rate limiting.
- `/metrics` (Prometheus) and `/_health`.
- `/` shows status and an AGPL source link.
- Distroless Docker image on GHCR; config in TOML.

**Clients** have an `--audit` option that checks a sample of indexer answers against their own sync.

---

## 13. Testing

| Layer | How |
|---|---|
| Protocol primitives | Vendored atproto-interop-tests: syntax (valid/invalid NSIDs, including an `online.7778777.lessdugin.*` case), data model JSON⇄CBOR⇄CID, MST, signatures (k256/p256, high-S rejected), did:key; tampered-proof fixtures; `cargo-fuzz` targets for the CAR/CBOR/MST decoders (nightly job) |
| Shared vectors | `testdata/vectors/<kind>/*.json` as `{name, input, expected}`. Rust runner in testkit; TS runner (vitest 5) calls `runVector` in a `vectors`-feature wasm build, guaranteeing the native and wasm builds agree. `xtask vectors --bless` regenerates expected output; the diff is reviewed |
| Markdown | At least 150 cases, an XSS corpus, proptest checks that ammonia and DOMPurify leave renderer output unchanged, and terminal `StyledDoc` snapshots |
| Engine | `MemHost` + `FakePds` scenario tests (sync idempotence, outbox crash recovery, swapRecord conflicts, Jetstream hint → refetch, deletion confirmation, cycle-safe thread assembly, approval mode); a synthetic 1k-post / 50k-vote forum for performance (`ls` under 300 ms warm) |
| CLI | `assert_cmd` + `insta` against FakePds via the `LESSDUGIN_TEST_PDS` dev hook |
| TUI | Key sequences → `TestBackend` buffer `insta` snapshots at 80×24 and 160×48; a resize fuzz test |
| E2E (devnet) | `packages/e2e/devnet.mjs` starts `TestNetworkNoAppView` and prints `{plc, pds, accounts}`. testkit's `Devnet` launches it. Rust e2e tests are `#[ignore]` unless `LESSDUGIN_E2E=1`. Optional `compose.e2e.yml` runs a Jetstream container pointed at the devnet PDS firehose |
| SPA | Playwright 1.64 (Chromium; `PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers` locally). One spec performs real OAuth against the devnet PDS (falling back to an e2e-only app-password build if http issuers are rejected — spike S2). Other specs reuse `storageState` including IndexedDB. A `securitypolicyviolation` listener fails any test that triggers a CSP violation |
| Cross-client | Scenario: alice creates a forum, bob joins and posts with an image, carol replies, votes and reacts, tags, a sequence, a moderator hides a comment. The CLI `--json` thread must be byte-identical to `engine.getThread` in the SPA, and the TUI snapshot must match. The approval-mode scenario and an indexer-on vs indexer-off comparison also run |

---

## 14. CI/CD

**`ci.yml`** runs on PRs and pushes:
- **rust:** fmt; clippy for native (all features) and for wasm32 (the wasm-safe crates); nextest; doctests; `xtask codegen --check`; `cargo deny check`.
- **wasm:** wasm-bindgen-cli 0.2.129 and binaryen via `taiki-e/install-action`; `xtask wasm --release --budget`; uploads an artifact.
- **web:** `pnpm i --frozen-lockfile`, biome check, `tsgo --noEmit`, vitest (vectors + host), `vite build`.
- **lexicons:** `go install github.com/bluesky-social/goat@<pin>`; `goat lex lint`; `goat lex breaking` against the merge base; the `@atproto/lexicon` parse check.

**Other workflows:**
- **`e2e.yml`** (PRs, plus nightly): devnet + Rust e2e + Playwright; uploads traces on failure.
- **`release.yml`** (tags):
  - Binaries for x86_64/aarch64 Linux (musl), macOS (arm64/x86_64) and Windows x86_64, each with the embedded SPA.
  - `SHA256SUMS` and `actions/attest-build-provenance`; GitHub release; indexer image pushed to GHCR.
  - The SPA `dist` tarball and its hash are published so the hosted site can be checked against the release.
- **`pages.yml`:** deploys the tagged `web/dist` and the site files.

**`cargo xtask` commands:** `codegen [--check]`, `wasm [--release] [--report] [--budget]`, `web`, `dist`, `devnet`, `e2e`, `vectors [--bless]`, `lex lint|breaking|publish|check-dns`, `ci` (runs everything CI runs, locally).

---

## 15. Licensing

**Recommended split:**
- Application code (core, native, cli, tui, wasm, indexer, web, engine package): **AGPL-3.0-or-later**, with SPDX headers.
  - The indexer and the hosted SPA link to their exact source commit (AGPL §13).
  - The SPA ships source maps.
- `lexicons/`, `testdata/vectors/` and `docs/spec/`: **CC0-1.0**, so other implementations can adopt the protocol freely.
- `lessdugin-atp` and `lessdugin-lexicon` crates: recommended **MIT OR Apache-2.0** so third-party clients can reuse them. This is the owner's decision; the owner holds the copyright, and contributions come in under DCO.

**Dependency compatibility:**
- jacquard (MPL-2.0) is compatible.
- DOMPurify (MPL-2.0 OR Apache-2.0) is fine.
- atcute is 0BSD; ring is Apache-2.0 AND ISC; webpki-roots is CDLA-Permissive-2.0.
- The math font is OFL-1.1.
- `deny.toml` allows: MIT, Apache-2.0, BSD-2/3, ISC, Zlib, 0BSD, Unicode-3.0, MPL-2.0, CC0-1.0, CDLA-Permissive-2.0.

---

## 16. Phased plan

M3 freezes engine API v0, after which M4, M5 and M6 run in parallel.

| Phase | Scope | Acceptance criteria |
|---|---|---|
| **M0 Bootstrap** | Workspace and toolchain, CI skeleton, pnpm/Biome, xtask stubs, devnet script, docs/adr. **Spikes** written up as ADRs: **S1** jacquard-oauth loopback and hosted-native login against bsky.social, plus how to make our own signed-in calls (raw DPoP or `XrpcRequest` impls for the ~6 write endpoints); **S2** atcute loopback mode on 127.0.0.1, and OAuth against an http devnet PDS; **S3** wasm size of pulldown-cmark + math-core + k256/p256 + serde_json + tsify; **S4** a test account accepts `online.7778777.lessdugin.*` records, and atcute, jacquard and @atproto/lexicon all accept the NSID; **S5** TestNetworkNoAppView boots in GH Actions without Postgres; **S6** `listReposByCollection` and the Jetstream wildcard work on our namespace | `cargo xtask ci` green locally and in Actions; devnet with 3 accounts in under 30 s; `lessdugin web` serves a placeholder SPA; ADRs merged |
| **M1 Protocol foundations** | atp crate (complete); identity; XRPC subset; draft lexicons + codegen + validate; NativeHost (reqwest, SqliteKv, DNS); auth (app password with 2FA, OAuth loopback, hosted fallback, session lock); CLI `login/logout/whoami/accounts/resolve/verify` | All interop vectors pass; `lessdugin verify at://<real bsky post>` reports `proven` with the key; tampered fixtures fail; both login methods work against bsky.social and devnet; parallel CLI processes never break a refresh |
| **M2 Rendering** | Markdown crate (HTML/MathML, StyledDoc, text, ToC, anchors), media module (strip, orientation, CID), vectors | At least 150 markdown vectors pass; XSS corpus neutralized; 10-minute fuzz run without panics; EXIF/GPS removed and orientation preserved in fixtures |
| **M3 Engine + CLI MVP** | KV schema and indices, discovery (roster, Constellation, relay, Jetstream), sync, ingest checks, outbox, ForumModel (moderation fold, karma/v1, scores), threads, ranking (magic/top/new/old/recentComments/topAdjusted), reacts, tags, sequences, search, notifications, local state, full CLI tree, export | Devnet scenarios (open and approval) pass; synthetic forum `ls` under 300 ms warm; a re-sync changes no bytes in views; outbox survives `kill -9`; karma/rank/moderation vectors pass; a manual production smoke test on bsky.social accounts succeeds |
| **M4 TUI** | All screens, keymap, images, editor, live updates | Snapshots for every screen at both sizes; every MVP action reachable by keyboard; works over SSH (halfblocks); resize fuzz without panics |
| **M5 wasm + SPA** | Bindings, `@lessdugin/engine` (worker, IDB host, auth bridge, supervisor), SPA screens, editor with media paste/drop, hosted and loopback OAuth, `lessdugin web` | Vector parity 100%; wasm at most 1.1 MB gz (target 900 KB); a cached forum renders in under 1.5 s; Playwright scenarios pass with no CSP or Trusted Types violations; axe/Lighthouse accessibility at least 90; cross-client byte-identical thread JSON |
| **M6 Indexer** | Ingest, backfill, poll mode, XRPC queries, hydration, karma snapshot, SSRF guard, Docker image; client integration as a discovery source, plus `--audit` | Devnet (local Jetstream or poll): consistent within 5 s of a write; restart resumes with no gaps; clients produce identical views with and without the indexer; synthetic 1M-record load: `listPosts` p95 under 50 ms at 200 rps |
| **M7 Release 0.1** | Final lexicons + permission set; publish `com.atproto.lexicon.schema` records + `_lexicon.lessdugin.7778777.online` TXT; hosted client metadata; Pages deploy; docs (user guide, spec, indexer self-hosting, threat model); release pipeline | `goat lex check-dns` passes; `include:` scope works on bsky.social; on a fresh machine, install from release → login → post → the post is visible on the hosted SPA and in `lessdugin web` |

**After the MVP:**
- **P8:** questions and answers, quick takes, revision history UI, co-authors, side comments and inline reacts in the margin, standard.site dual-publishing.
- **P9:** sealed reports and moderator keys.
- **P10:** sync v2 (verified MST range diffs via `getRepo?since`) and PLC audit-log verification.
- **P11:** events/RSVP, polls, dialogues.
- **P12:** labeler integration, encrypted cross-device sync of local state, indexer push notifications.

---

## 17. Main risks

| Risk | Mitigation |
|---|---|
| jacquard is unstable | Pinned version, facade, own-OAuth fallback (S1) |
| atcute or jacquard reject an http issuer on devnet | E2E-only app-password path (S2) |
| wasm size creeps up | Budget enforced in CI, feature gating, twiggy reports |
| Serverless sync is slow on large forums | Documented scale envelope; indexer; describeRepo/rev gating; P10 |
| Karma is Sybil-gameable | Seed and threshold rules belong in the spec, not the code |
| Third-party discovery services disappear | Every source is optional and pluggable; roster mode and the indexer need none of them |
| localStorage DPoP keys exposed by cross-site scripting | Strict CSP, Trusted Types, DOMPurify and the event-level sanitizer, plus no user-supplied HTML anywhere |

### Critical files for implementation
- /home/user/lessdugin/crates/core/src/host.rs (the `Host`/`KvStore`/`MaybeSend` contract every backend implements)
- /home/user/lessdugin/crates/core/src/engine/mod.rs (sync, outbox, ForumModel, the public API and view DTOs)
- /home/user/lessdugin/crates/atp/src/lib.rs (data model, CID, CAR, MST proof, commit signature verification, XRPC subset)
- /home/user/lessdugin/crates/markdown/src/lib.rs (sanitized event pipeline → HTML/MathML and StyledDoc)
- /home/user/lessdugin/crates/wasm/src/lib.rs together with /home/user/lessdugin/packages/engine/src/worker.ts (the JsHost bridge, IDB KV and the main-thread OAuth bridge)