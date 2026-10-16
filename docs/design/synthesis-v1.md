# lessdugin: final design and implementation plan (v1, 2026-10-16)

This document is the reconciled design. It replaces the four design drafts and three critiques it was built from, and it is meant to be split into `docs/spec/*` (normative) and `docs/plan.md`. Section 10 records every critique decision. Behaviour IDs use the form `name/N`; once released they never change meaning.

---

## 0. Decisions on one page

| # | Decision |
|---|---|
| D1 | **The PDS is the only database.** No MVP feature needs a lessdugin server. The indexer is an optional extension. |
| D2 | **Namespace.** Flat namespace `online.7778777.lessdugin.*` (`NS`), plus `NS.index.*` for indexer queries. Pre-release builds write to `online.7778777.lessdugin.temp.*`, and the prefix is a build-time constant. |
| D3 | **Ten record collections.** forum, membership, post, comment, revision, opinion, modAction, wikiRevision, sequence, profile. One `opinion` record per (voter, subject) holds the karma axis, the agreement axis, reacts and tag votes. |
| D4 | **Every forum-scoped record key starts with the forum key `{fk}-`.** Keys that track a "subject" are deterministic. A record whose key is not the canonical one is invalid. |
| D5 | **Every content record has a required `forum` field.** There are no forum-less posts. |
| D6 | **Trust levels.** Content counts only if it was fetched from the author's PDS over TLS with the CID recomputed (`pds`), or arrived with a signed commit proof (`proven`). Everything else is a *hint*: hints can schedule work and order placeholders, but never supply displayed text and never feed moderation or karma. |
| D7 | **MVP sync.** Per-forum `listRecords` key ranges (a cursor trick, checked at runtime), with Jetstream as the "this repo changed" signal and a daily reconcile. A verified sync that walks a range of the repo's Merkle Search Tree (MST) comes in P7. |
| D8 | **All derived state is a pure, deterministic fold** in the Rust core. Native code, wasm and the indexer produce byte-identical output for the same inputs. The fold runs under versioned behaviour IDs: `mod/1`, `karma/1`, `magic/1`, `lfm/1`, `default/1`, `sk/1`, `fk/1`. |
| D9 | **`karma/1` is seeded and staged:** K1 → E → KP → K, with a per-pair cap. In open mode, becoming an established voter takes upvotes from at least 2 distinct seeds. |
| D10 | **Moderation records state, not events.** There is one record per (actor, subject, action). Higher rank wins; at equal rank the safer value wins. Only current authorities count. Positive assertions are bound to a CID. The fold fails closed. |
| D11 | **Bans and locks are backdating-proof.** A ban hides all of the user's in-forum content. A lock carries an explicit allow-list of comments. |
| D12 | **Time.** `t_rank = max(causal lower bound, min(createdAt, TID, firstSeen + 5 min))`. Only ranking and display depend on time. |
| D13 | **Edits.** An edit is `applyWrites[create revision(verbatim prior value), update]` with `swapCommit` and a CID-aware automatic retry. Authors can redact revisions. |
| D14 | **LFM/1 markdown.** CommonMark, GFM tables, strikethrough, task lists, footnotes and alerts, math, spoiler/details blocks and figure captions. No raw HTML. Media is referenced with `cid:` URLs. External images are never fetched automatically. |
| D15 | **One renderer.** Rust produces HTML+MathML for the SPA and for export, and sanitised `TermText` styled lines for the TUI and CLI. |
| D16 | **Media.** Metadata is stripped by allowlist rebuild, images are physically rotated and downscaled, every fetch is checked against its CID, and media loads automatically only under a privacy policy. |
| D17 | **Auth.** OAuth everywhere: loopback or hosted-native for the CLI, web client for the SPA, and the CLI holds the session for `lessdugin web`. App passwords are accepted; main passwords are refused. `transition:generic` is never requested. |
| D18 | **Rust layout.** Wasm-safe crates `atp`, `lexicon`, `markdown`, `core` sit over a `Host` trait. jacquard is used only behind the native OAuth facade. |
| D19 | **SPA.** React, Vite and TypeScript. The wasm engine runs in a Web Worker. Hash routing. Strict CSP plus Trusted Types. |
| D20 | **Indexer.** The same engine plus `NS.index.*` XRPC. It returns uri+cid with hint values, and verifiable proof CARs. |
| D21 | **Licences.** Applications are AGPL-3.0-or-later. Lexicons, spec and test vectors are CC0-1.0. The `atp` and `lexicon` crates are MIT OR Apache-2.0 (owner to confirm). No dependency we need is incompatible with AGPL. |

---

## 1. Principles and trust model

### 1.1 Principles

1. **P1. The PDS is the database.** Author PDSes plus PLC/did:web plus DNS are enough for every MVP feature.
2. **P2. Authoritative bytes come from exactly two places.** Either the author's PDS (the one in the DID doc, over TLS, CID recomputed) or a signed commit proof checked against the DID's current `#atproto` key.
3. **P3. Hints never become content.** Hints may discover, schedule, order placeholders and supply approximate counts. They never supply text attributed to a DID, and never feed moderation, karma or visibility.
4. **P4. Determinism.** `ForumView = fold(decl, records, obs, repoStatus, now, algorithmIds)`. Native, wasm and indexer produce byte-identical output.
5. **P5. Validity by construction.** Deterministic keys, required fields and frozen limits. Invalid records are ignored and appear only in the raw view.
6. **P6. Time is a claim.** Honest content behaves the same on every client. Dishonest timestamps can only hurt their author.
7. **P7. Fail closed.** Unknown is not the same as absent. Moderation never vanishes because a PDS is down.
8. **P8. Public means public.** Votes, reacts, memberships and mod actions are public, and the UI says so at first use. Private state stays local.
9. **P9. One implementation of domain logic.** The Rust core serves the CLI, TUI, SPA (wasm) and indexer.
10. **P10. Decide the permanent things now.** NSIDs, rkey grammars, required sets, every limit and the behaviour IDs are fixed now. Anything that can later be added as an optional field, a new knownValue or a new collection is deferred.

### 1.2 Trust levels and badges

| Kind | Levels |
|---|---|
| Content | `hint`: any non-authoritative source. `pds`: from the PDS in the DID doc over TLS, with JSON → DAG-CBOR → CID equal to the returned cid. `proven@rev`: commit signature, MST path and CID all verified. |
| Identity | `hint`: Slingshot, AppView or indexer. `doc`: PLC or did:web over TLS. `audited`: PLC audit log verified (P7). |
| Handle | `verified`: both directions agree, using DNS-over-HTTPS (DoH) or `.well-known`. `unverified`: shown as a short DID with the claimed handle struck through. `invalid`. |
| Badges | Plain `pds` content has no badge. `[verified]` means proven with a `doc` identity. The others are `[stale]`, `[partial k/n]`, `[future-dated]`, `[identity unverified]`, `[edited]` and `[hint]` (placeholder). The badge column is fixed, and badge glyphs are stripped from user text. |

### 1.3 Source rules

| Source | Text shown? | Discovery and scheduling? | Fold input (moderation, karma)? |
|---|---|---|---|
| Author PDS (`pds`) | yes | yes | yes |
| Proof CAR from any source (`proven`, identity doc ≤ 1 h old) | yes | yes | yes |
| Jetstream | no (counts and placeholders only) | yes (marks repos dirty) | no |
| Constellation, relay, Slingshot | no | yes | no |
| Indexer JSON or aggregates | no. Scores may show as `~n` until recomputed locally | yes | no |

### 1.4 Dependencies by tier

- **CORE**: PDS, PLC/did:web, DNS (DoH provider is configurable).
- **CORE+**: relay, Jetstream, Constellation, Slingshot.
- **EXT**: lessdugin indexer.

| Feature | CORE | CORE+ | EXT |
|---|---|---|---|
| Read and write all content types | ✓ | | accelerates |
| Approval-mode participant set (complete) | ✓ | | |
| Open mode: granted and established users (closure, §3.2) | ✓ | | |
| Open mode: brand-new newcomers; approval-mode applications | | ✓ | ✓ |
| Forum directory | | ✓ relay | ✓ |
| Live updates | polling | ✓ Jetstream | ✓ `getChanges` |
| Notifications in followed forums | ✓ at sync time | ✓ live | ✓ offline and cross-forum |
| Search | ✓ local | | ✓ global |
| Karma, scores, moderation | ✓ | | hint only |
| Verification | ✓ `verify` | | ✓ proof CARs |
| Browser cold start above ~300 participants | slow | | ✓ |

**Settings:**
- `sources = direct | assisted` (default `assisted`).
- `verify = pds | proof` (default `pds`; `proof` checks every displayed item in the background).
- Indexers come from the forum's declaration or the user's config. A forum-declared endpoint needs one-time user consent.

---

## 2. Namespace and lexicons

### 2.1 Namespace, publishing, governance

| Group | DNS TXT record | Contents |
|---|---|---|
| `online.7778777.lessdugin` | `_lexicon.lessdugin.7778777.online "did=<publisher>"` | 10 records, `defs`, `markdown`, `authFull`, `authInteract` |
| `online.7778777.lessdugin.index` | `_lexicon.index.lessdugin.7778777.online "did=<publisher>"` | indexer queries and `index.defs` |
| `online.7778777.lessdugin.temp` | none; never published | pre-release and dogfood builds (`NS` constant); dropped at 0.1 |

**Publisher account**
- A did:plc account with handle `lessdugin.7778777.online` and an offline rotation key.
- It holds one `com.atproto.lexicon.schema` record per NSID (rkey = NSID).
- CI publishes after `goat lex lint`, `goat lex breaking` against the last tag, and `goat lex check-dns`.
- Domain hardening: registrar lock, multi-year auto-renew, DNSSEC, CAA.
- Lexicons are compiled into every binary and never fetched at runtime.

**Naming rule.** A child group may never share a name with a record, so there is never an `NS.forum.*` group.

### 2.2 Conventions

**Notation** used in the tables below:

| Notation | Meaning |
|---|---|
| `!` | required |
| `s(G/B)` | string with maxGraphemes G and maxLength B bytes |
| `kv{…}` | knownValues string; maxLength 64 bytes unless noted |
| `[T]≤N` | array of at most N items |
| `blob(accept; max)` | blob with accept list and maxSize |
| `md` | open `union[NS.markdown]` |
| `slug` | `s(64b)` matching `^[a-z0-9]+(-[a-z0-9]+)*$` |
| `sref` | `com.atproto.repo.strongRef` |
| `atu` | `at-uri` with maxLength 3000 |
| `url` | `uri` with maxLength 3000 |
| `int[a..b]` | integer between a and b |

**Limits.** Every string, array, integer and blob field gets its final limit at first publication. Limits can never be changed afterwards. Format-typed strings (did, cid, datetime, language, tid) rely on the limits built into their syntax.

**References**
- Stored at-uris are canonical: `at://<did>/<nsid>/<rkey>`, with no handle, query, fragment or trailing slash, and no non-canonical forms. On read, a reference that is not canonical makes the record invalid.
- Use `sref` where the version matters: comment root and parent, `opinion.subject`, `revision.subject`, wiki parents.
- Use `atu` where identity matters: `forum` fields, sequence entries, mod-action subjects, profile pins.

**Values**
- Datetimes are RFC 3339 UTC with a `Z` suffix and millisecond precision.
- There are no floats. Ranks are decimal strings in indexer responses.
- All unions are open. All enumerations use knownValues, each with a documented fallback (§2.11).

**Writes**
- Writers always include `$type`, never pass `validate: true` (they leave it unset), and preserve unknown fields on read-modify-write.

### 2.3 Record keys

```
b32s(x)        = base32, MSB-first 5-bit groups, alphabet "234567abcdefghijklmnopqrstuvwxyz", no padding (sort order = bit order)
H(tag, p…)     = sha256( UTF8(tag) ‖ (0x1F ‖ UTF8(NFC(p)))* )          // any part containing U+001F ⇒ reject
fk(forumUri)   = b32s(H("lessdugin-fk/1", forumUri))[0..12]           // 60 bits
h16(name, p…)  = b32s(H("lessdugin-sk/1", name, p…))[0..16]          // 80 bits; name = short collection name ("opinion")
threadTid(uri) = for a post rkey {fk}-{T}: T ; for a comment rkey {fk}-{T}-{t}: T
```

| Collection | Rkey grammar (type `any`; the core validates) | The record is valid only if |
|---|---|---|
| forum | `slug` | – |
| membership | `{fk}` | `fk == fk(forum)` |
| post | `{fk}-{tid}` | `fk == fk(forum)` |
| comment | `{fk}-{rootTid}-{tid}` | `fk == fk(forum)`, the root's rkey has the same fk, `rootTid == threadTid(root.uri)` |
| revision | `{P}-{tid}`, where `P = rkey(subject)`, or `fk(subject)` when the subject is a forum | subject is in the same repo |
| opinion | `{fk}-{stid}-{h16("opinion", subject.uri)}`, where `stid = threadTid(subject.uri)` for post or comment subjects, and `~` otherwise | `fk ==` the subject rkey's fk `== fk(forum)` |
| modAction | `{fk}-{h16("modAction", scope, subject, actionKey)}` | `fk == fk(forum)`. Keys of the form `{fk}-{tid}` are reserved for P2 list actions and ignored in v1 |
| wikiRevision, sequence | `{fk}-{tid}` | `fk == fk(forum)` |
| profile | `self` (literal) | – |

**Why this layout**
- A forum is one key range per collection in every repo.
- A thread is one key range (`{fk}-{rootTid}-`).
- Interactions can be windowed by the time of their thread (P7).
- Comments cannot be hidden from future windowed sync by backdating, because their middle key segment is tied to the root post.
- A one-per-(actor, subject) record is a single `getRecord` call.

**Collisions**
- A collision in fk (60 bits) costs only efficiency, except for membership, where it would block membership of both colliding forums. A targeted collision is about 2^60 work.
- A collision in h16 (80 bits) is negligible.

**Test vectors** are regenerated in M0 as `testdata/vectors/keys/*.json`. The draft sk/1 vectors are superseded: the domain now uses the short collection name, and opinion replaces vote.

### 2.4 Shared definitions

`NS.defs`:

```
#media        { blob!: blob(*/*; no max), alt: s(2000g/20000b), aspectRatio: #aspectRatio,
                durationMs: int[0..86400000], name: s(255b) /* opt-in, basename only, display-only */ }
#aspectRatio  { width!: int[1..1000000], height!: int[1..1000000] }
#textQuote    { exact!: s(1000g/10000b), prefix: s(64g/640b), suffix: s(64g/640b), start: int[0..100000000] }
#externalLink { uri!: url, title: s(300g/3000b), description: s(1000g/10000b), thumb: blob(image/*; 1000000) }
#link         { uri!: url, label: s(64g/640b) }
```

`NS.markdown` is the object def `main` used in every `md` union. Other apps may embed it, for example in `site.standard.document.content`.

```
profile:  kv{"lfm/1"} default "lfm/1"
text:     s(100000b)                    inline LFM source
textBlob: blob(text/*; 1000000)         spill-over, UTF-8 LFM
media:    [defs#media]≤128              the only scope for cid: URLs in this body
mentions: [did]≤128                     derived by the writer; hint only
links:    [atu]≤256                     derived by the writer (pingbacks); hint only
```

**Body rules**
- Exactly one of `text` or `textBlob` must be present.
- Writers spill to `textBlob` when the text exceeds 64 KiB, or when the serialized write body would exceed 900,000 bytes.
- `textBlob` is allowed only in `post.body`, `wikiRevision.body`, `forum.about` and `sequence.body`. Everything else is inline only.

### 2.5 Records (v1)

```
forum (rkey slug; owner repo)
  name!: s(100g/1000b)   description: s(500g/5000b)   about: md   icon: blob(image/*; 2000000)
  moderators: [#moderator]≤64   membership: kv{open, approval} default open
  defaultTier: kv{personal, frontpage} default personal   featuredTags: [slug]≤64
  policy: #policy   reportContact: url   discovery: #discovery   createdAt!, updatedAt
  #moderator { did!: did, role!: kv{admin, moderator}, title: s(64g/640b) }
  #policy { moderation: kv{"mod/1"}, karma: #karmaP, ranking: #rankP,
            voting: kv{karmaOnly, twoAxis, reacts} default reacts, reactPalette: kv{"default/1"},
            gates: #gates, display: #display, authorModeration: bool default true,
            tagCreation: kv{open, moderators} default open }
  #karmaP  { algorithm: kv{"karma/1"}, pairCap: int[1..1000000] =25, establish: int[0..1000000] =1,
             establishSeeds: int[1..1000] =2 }
  #rankP   { algorithm: kv{"magic/1"}, offsetHours: int[1..10000] =2, exponentMilli: int[0..10000] =1150,
             curatedBonus: int[-1000000..1000000] =10, frontpageBonus: int[-1000000..1000000] =0, cutoffDays: int[1..36500] =90 }
  #gates   { reactSecond, reactCreate, reactAnti, authorModPersonal, authorModFrontpage, createTag, wikiEdit, sequence
             : int[-1000000000..1000000000] }      // an absent gate uses the karma/1 default (§4.8)
  #display { collapseBelow: int[-1000000..1000000] =-5, newUsers: kv{show, collapse, hide} =collapse }
  #discovery { indexers: [#endpoint]≤8 }   #endpoint { url!: url, did: did, note: s(300g/3000b) }

membership (rkey {fk}; member repo)
  forum!: atu   note: s(2000g/20000b) (publicly visible application text)   createdAt!

post (rkey {fk}-{tid})
  forum!: atu   kind: kv{article, link, question, quickTake} default article   title: s(300g/3000b)
  body: md   excerpt: s(500g/5000b) (MUST when body uses textBlob)   link: defs#externalLink
  tags: [slug]≤32   labels: union[com.atproto.label.defs#selfLabels]   createdAt!, updatedAt

comment (rkey {fk}-{rootTid}-{tid})
  root!: sref (post)   parent: sref (comment, same root)   forum!: atu (== root's forum)
  body!: md (inline)   quote: defs#textQuote (anchored in parent if present, else root)
  retracted: bool   moderatorHat: bool (shown only if the author is a current authority)   createdAt!, updatedAt

revision (rkey {P}-{tid})
  subject!: sref (the superseded version)   op!: kv{update, delete}
  record: union[NS.post, NS.comment, NS.forum, NS.sequence, NS.modAction] (open; verbatim prior value)
  summary: s(300g/3000b)   createdAt!

opinion (rkey {fk}-{stid}-{h16})
  subject!: sref (post | comment | wikiRevision | sequence)   forum!: atu
  karma: kv{up, down, strongUp, strongDown}   agreement: kv{agree, disagree, strongAgree, strongDisagree}
  reacts: [#react]≤32   tags: [#tagVote]≤32 (subject must be a post)   createdAt!, updatedAt
  #react   { react!: slug, stance: kv{for, against} default for, quote: defs#textQuote }
  #tagVote { tag!: slug, value!: kv{up, down, strongUp, strongDown} }
  (an empty opinion is deleted; an unknown value disables only that axis)

modAction (rkey {fk}-{h16(modAction, scope, subject, actionKey)}; actor repo)
  forum!: atu   scope: kv{forum, author} default forum
  subject!: s(3000b) (grammar: did | canonical at-uri | "tag:"slug)   subjectCid: cid (core-required for positive assertions)
  action!: union[#hide, #access, #restrict, #tier, #pin, #lock, #tag] (open)
  reason: s(2000g/20000b)   until: datetime   adopts: sref   createdAt!, updatedAt
  #hide     { hidden: bool =true, spam: bool, showReason: bool =true }            key "hide"
  #access   { level!: kv{banned, member} }                                       key "access"     (subject = did)
  #restrict { capabilities: [kv{post, comment, vote, react, tag, wiki, sequence}]≤16, nullifyVotes: bool } key "restrict"
  #tier     { tier!: kv{personal, frontpage, curated}, notice: md (inline) }      key "tier"       (post)
  #pin      { scope!: kv{forum, thread}, pinned: bool =true }                     key "pin:"+scope (post|comment|sequence)
  #lock     { locked: bool =true, allowed: [atu]≤5000 }                           key "lock"       (post; forum uri = read-only forum)
  #tag      { removed: bool, head: sref (wikiRevision), aliasOf: slug }           key "tag"        (subject "tag:"slug)
  (an unknown action type: key = its $type; no effect; shown as "unknown action")

wikiRevision (rkey {fk}-{tid}; immutable by convention)
  forum!: atu   slug!: slug   parents: [sref]≤8 (empty = genesis)   name!: s(100g/1000b)
  description: s(300g/3000b)   body: md (may spill)   isTag: bool =true   parentTags: [slug]≤16
  summary: s(300g/3000b)   createdAt!

sequence (rkey {fk}-{tid})
  forum!: atu   title!: s(300g/3000b)   body: md   cover: defs#media   chapters!: [#chapter]≤50   createdAt!, updatedAt
  #chapter { title: s(300g/3000b), body: md (inline), posts!: [atu]≤100 }

profile (rkey self)
  displayName: s(64g/640b)   avatar: blob(image/*; 2000000)   bio: md (inline)   links: [defs#link]≤16
  pinned: [atu]≤16   createdAt, updatedAt
  (falls back to app.bsky.actor.profile/self read from the PDS, never from the AppView)
```

### 2.6 Reserved names (not written or published until final)

| Reserved name | Purpose |
|---|---|
| `report` | sealed report |
| `turn` | dialogue message |
| `coauthor` | co-author acceptance |
| `pollVote` | poll vote |
| `collection` | books |
| `karmaSnapshot` | karma checkpoint |
| `observation` | authority first-seen batches |
| `subscription` | public subscriptions |
| `modAction` keys `{fk}-{tid}` | bulk actions: `#banList`, `#raid`, `#banHost` |
| optional fields | `post.coauthors`, `post.event`, `post.poll`, `comment.answer`, `profile.moderationGuidelines`, `#ban.keep` |

### 2.7 LFM/1 (normative; `docs/spec/lfm-1.md`; vectors in `testdata/vectors/lfm1`)

**Parser**
- pulldown-cmark `=0.13.4` (pinned; an upgrade must pass the vectors unchanged).
- Options on: `TABLES | FOOTNOTES (GFM) | STRIKETHROUGH | TASKLISTS | MATH | GFM`.
- Options off: smart punctuation, heading attributes, YAML or `+++` metadata, definition lists, super/subscript, wikilinks.
- Raw HTML blocks and inline HTML become literal escaped text.

**Post-passes**, in order:
1. GFM extended autolinks (`www.`, `http(s)://`) in text outside links and code.
2. A blockquote whose first line is `[!spoiler]` or `[!details] <summary>` (case-insensitive) becomes a spoiler or a `<details>` block.
3. A paragraph that contains only an image with a title becomes `figure` plus `figcaption`.
4. Mentions: `[@x](at://<did>)` displays as `@` plus the verified current handle, otherwise the short DID.
5. URL policy (below).
6. ID assignment.

**URL policy**
- Links: `https`, `http`, `mailto` and `#fragment` are allowed. `at://` becomes an internal route with a hover preview (our collections only, depth 1). Anything else, including relative URLs, `javascript:`, `data:` and `file:`, drops the link and keeps its text.
- Images: `cid:<CIDv1>` resolves only against this body's `media[].blob.ref`; a miss shows a placeholder. Unreferenced media renders as a gallery after the body. `http(s)` images are never fetched by default; they render as the link text "image: alt" and the user can opt in per forum. Other schemes render as alt text.

**Math**
- Content is TeX. It renders to MathML through math-core. If S10 fails, the fallback is `<span data-tex>` plus Temml loaded lazily.
- Disabled macros: `\href \url \includegraphics \class \style \html* \def \gdef \newcommand \let`.
- Per expression: at most 4 KB and brace depth 64. On error, show `<code class="math-error">` with the source.

**HTML contract**

Allowed elements:
- Text blocks: p, h1–h6, blockquote, ul, ol[start], li, hr, br, details, summary.
- Code: pre, code[class=language-[a-z0-9_+-]{1,32}].
- Inline: em, strong, del, sup.
- Task lists: input[type=checkbox][disabled].
- Links: a[href|data-at-uri|data-did|data-anchor|class|rel].
- Media: figure, figcaption, img[data-cid|alt|width|height]. `src` is set only by the SPA after verification.
- Tables: table, thead, tbody, tr, th and td with class `ta-l`, `ta-c` or `ta-r`.
- Other: section.footnotes, div.alert-{note,tip,important,warning,caution}, and allowlisted MathML.

Attributes and IDs:
- No style attributes and no other attributes from content.
- IDs are `md-h-<slug>` (GitHub-style slug over `[\p{L}\p{N}_-]`, deduplicated as `-1`, `-2`, …), `md-fn-<ordinal>` and `md-fnref-<ordinal>-<k>`.
- In-content `#frag` links become `data-anchor`. Content never assigns `location.hash`.

Other rules:
- External links get `rel="nofollow ugc noopener noreferrer"`.
- Every rendered root gets `dir="auto"`.
- Limits: nesting depth 64 (deeper content flattens to paragraphs) and at most 2^20 events (truncated with a notice). All walkers are iterative.

**Terminal contract.** `StyledDoc` holds blocks of `TermText` spans (§5). Math shows as TeX source. Images use ratatui-image or a placeholder.

**Plain-text projection P(source).** Walk the event stream:

| Events | Contribute |
|---|---|
| Text, inline code, math TeX, code-block text, literal HTML | their content |
| Link | inner text |
| Image | alt text, plus `"\n"` + caption for figures |
| SoftBreak, table cell boundary | `" "` |
| HardBreak and every block start or end | `"\n"` |
| Footnote references, task markers | nothing |

Then apply NFC, collapse every run of White_Space to U+0020, and trim. Offsets are Unicode code points.
- Excerpt: the first 500 graphemes, cut at the last space, plus `…`.
- Word count: the number of maximal non-space runs.

**Quote resolution** (comment and react `quote`) is deterministic, with no fuzzy matching in v1:
1. Find every occurrence of `exact` in P of the target's *current* version.
2. One occurrence: it matches.
3. Several: pick the one with the longest prefix-plus-suffix agreement, then nearest to `start`, then the lowest offset.
4. None: the quote is orphaned and shown as "quoted text no longer present".

### 2.8 Permission sets, scopes, client metadata

```json
{"lexicon":1,"id":"online.7778777.lessdugin.authFull","defs":{"main":{"type":"permission-set",
 "title":"lessdugin (full access)",
 "detail":"Create, edit and delete your lessdugin forums, memberships, posts, comments, revisions, votes and reactions, wiki edits, sequences, profile and moderation actions.",
 "permissions":[{"type":"permission","resource":"repo","collection":[
  "online.7778777.lessdugin.forum","online.7778777.lessdugin.membership","online.7778777.lessdugin.post",
  "online.7778777.lessdugin.comment","online.7778777.lessdugin.revision","online.7778777.lessdugin.opinion",
  "online.7778777.lessdugin.modAction","online.7778777.lessdugin.wikiRevision","online.7778777.lessdugin.sequence",
  "online.7778777.lessdugin.profile"]}]}}}
```

`authInteract` covers membership, comment, revision and opinion.

| Use | Scope |
|---|---|
| Default | `atproto include:NS.authFull blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf` |
| Fallback (chosen at PAR on `invalid_scope`, before the browser opens), devnet, `.temp` builds | `atproto repo?collection=NS.forum&…(all 10) blob:…(same)` |
| Never | `transition:generic` |
| P2 interop (requested incrementally) | `repo:community.lexicon.calendar.event?action=create …`, `repo:site.standard.document …` |

**Client metadata.** Both files carry `grant_types: [authorization_code, refresh_token]`, `response_types: [code]`, `token_endpoint_auth_method: none` and `dpop_bound_access_tokens: true`, and their `scope` is the superset of every variant above. `client_uri` uses the same host.
- **Web:** `https://lessdugin.7778777.online/oauth/client-metadata.json`, `application_type: web`, `redirect_uris: ["https://lessdugin.7778777.online/"]`.
- **Native:** `…/oauth/cli-client-metadata.json`, `application_type: native`, `redirect_uris: ["http://127.0.0.1/callback"]` (the port is ignored when matching).

**Callback handling.** The callback handler accepts both `?query` and `#fragment` responses. `xtask site --origin <url>` generates metadata for self-hosted origins.

### 2.9 Indexer query lexicons (`NS.index.*`)

All methods are unauthenticated GET queries with CORS `*`. Array parameters are limited to ≤ 25 at-uris or ≤ 50 DIDs. Every response carries `asOf{seq, timeUs, lagMs}` and `indexer{did, coreVersion}`.

```
index.defs
  #recordRef  { uri!, cid!, firstSeen: datetime, value: unknown (HINT) }
  #tally      { subject!, policy!: s(64b), score!: int, agreement: int, unweighted: int, reacts: [#reactTally]≤64 }
  #reactTally { react!: slug, for!: int, against: int, quote: defs#textQuote }
  #postView   { ref!, tally, state: kv{visible, hidden, pending, collapsed}, tier: kv{personal, frontpage, curated},
                tags: [slug]≤32, comments: int, lastCommentAt, effectiveAt!, rank: s(64b) (decimal) }
  #commentView{ ref!, parent: atu, depth: int, tally, state }
  #participant{ did!, status!: kv{owner, admin, moderator, member, established, newcomer}, rev: tid, lastSeen }
  #change     { seq!: int, uri!, cid, did!, rev!, op!: kv{create, update, delete} }
  #activity   { reason!: kv{reply, mention, quote, react, vote, modAction, newPost, newComment, pingback}, ref!, subject: atu }
describeIndexer()                                   → {did, version, coreVersion, policies[], features[], forums[{uri, lagMs, backfilled}]}
listForums(q?, limit≤100, cursor?)                  → {forums[{ref, participants, posts}], cursor}
listParticipants(forum!, status?, limit≤1000, cursor?) → {participants[#participant], cursor}
getRepoHeads(dids[≤50])                             → {heads[{did, rev, commit, pds, checkedAt, status}]}
getChanges(forum?, since!: int, limit≤1000)         → {changes[#change], cursor}  | error CursorTooOld
listPosts(forum!, sort kv{magic, top, new, old, recentComments}, tag?, author?, kind?, tier?, hydrate?, limit≤100, cursor?)
                                                    → {posts[#postView], cursor, policy{karma, ranking}, inputDigest}
listRecentDiscussion(forum!, limit≤50, cursor?)     → {items[{post: #postView, comments[#commentView]≤3}], cursor}
getThread(post!, sort?, hydrate?)                   → {post, comments[#commentView], truncated, inputDigest}
getTallies(subjects[≤25])  getRecords(uris[≤25])    (JSON, hints)
getRecordProofs(uris[≤25])                          → application/vnd.ipld.car (proofs/1, §3.6)
listBacklinks(subject!, collection!, path?, limit≤100, cursor?)
searchPosts(q!, forum?, author?, limit≤50, cursor?) → {hits[{ref, snippet (plain text), highlights[[start,end]]≤16}], cursor}
getActivity(actor!, since?, forums[≤25]?, limit≤100, cursor?) → {activity[#activity], cursor}   (public data; rate-limited per actor)
getKarma(forum!, actors[≤50])                       → {karma[{did, k1, kp, k}], policy, inputDigest}
```

**Indexer discovery.** The indexer is a `did:web` whose DID doc has service `#lessdugin_index` of type `LessduginIndex`. Clients also learn of indexers through `forum.discovery.indexers` or user config.

### 2.10 Interop decisions

| Lexicon | Decision |
|---|---|
| `com.atproto.repo.strongRef`, `label.defs#selfLabels` | reuse |
| `app.bsky.actor.profile` | read-only fallback for name and avatar, fetched from the PDS |
| `site.standard.publication/document` | no default dual-publishing. P2: opt-in syndication of one's own posts, same rkey, with `content` set to our `markdown` object |
| `at.markpub.markdown` | not adopted; ours is deliberately similar. Readers may add it to the `md` union later (non-breaking) |
| `community.lexicon.calendar.event/rsvp`, `location.*` | P2 events, dual-published with the same rkey |
| `community.lexicon.bookmarks.bookmark` | P2 opt-in public bookmarks; local by default |
| `community.lexicon.interaction.like` | not used (would double-count) |
| Tangled, Frontpage, atbbs, atBB | shape inspiration only; possible importer later |
| Labelers | P2: `policy.labelers` |
| Private messages | out of scope; we link to Bluesky chat |

### 2.11 Evolution rules

**Allowed changes:** new optional fields, new knownValues, new refs in open unions, new defs and new collections.

**Frozen:** types, names, required sets, all limits and accept lists, rkey grammars.

**Breaking changes** get a new NSID, `<name>V2`. Clients read both, writers switch after a transition period, and nobody ever dual-writes.

**Fallbacks for unknown values:**

| Unknown | Fallback |
|---|---|
| kind | article |
| role | no capabilities |
| action | no effect |
| react id | ignored |
| vote value | that axis is ignored |
| markdown profile | rendered as lfm/1 with a badge |
| policy or algorithm ID | the default, plus a banner ("this forum uses rules newer than your client") |

**Behaviour IDs** never change meaning, and their normative vectors live in `testdata/vectors/`.

---

## 3. Discovery, sync, verification, caching

### 3.1 Identity

**Handle → DID**
- DNS TXT `_atproto.<handle>` via DoH JSON on every platform. The default provider is `cloudflare-dns.com`, falling back to `dns.google`. Using the same method everywhere means clients reach the same verdicts.
- Native can opt into system DNS with `network.dns=system`; those answers are flagged `dns-unauthenticated`.
- If DNS has no answer, try `https://<handle>/.well-known/atproto-did`. In the browser, a CORS failure counts as "no answer". DNS is authoritative.
- Last resort: Slingshot, as a `hint`.

**DID → document**
- PLC: configurable base, default `https://plc.directory`.
- did:web: `/.well-known/did.json`. Forms with a port or path are rejected.
- If the browser hits a CORS failure on a did:web doc, use the Slingshot mini-doc. The identity is then `hint`, and that DID's content is shown collapsed with `[identity unverified]`.

**Display and storage**
- A handle is displayed only if it is verified in both directions.
- Stored references are always DIDs.
- `xn--` handles are never decoded.
- A UTS #39 confusable check compares names against the owner, moderators and frequent posters.

**Cache durations and invalidation**
- Handle 6 h; DID doc 24 h.
- Both are invalidated by a Jetstream `identity` event, a signature failure (retry once with a fresh doc), or `RepoNotFound`/`RepoDeactivated`/`RepoTakendown` (refresh; if the PDS changed, follow the migration).
- Proofs from third parties require a doc fetched within the last hour.

**Account status** comes from `getRepoStatus` at the PDS. Content from inactive accounts is hidden and kept for 7 days.

**PLC pacing:** ≤ 20 requests/s and 8 in flight, adapting on 429.

**Login** checks that the session `sub`, the DID doc's PDS and the token issuer agree. The authorization server host is shown at first login and pinned (trust on first use).

### 3.2 Forum resolution and participant discovery

```
open_forum(ref):
  uri = parse(ref)                      // at-uri | SPA URL | handle/slug; handle → DID first
  decl = getRecord(owner PDS)           // pds level; first open also fetches a sync.getRecord proof
  A = {owner} ∪ decl.moderators         // authorities
  sync(A, priority P0, unwindowed)      // until every A repo is synced once (or cached): view = degraded (fail closed)
  G = granted members = resolve(#access=member)  ;  S = (A ∪ G) − banned − nullified      // seeds
  candidates =
    approval: A ∪ G
    open:     A ∪ G ∪ Closure ∪ Newcomers
       Closure   = DIDs owning subjects referenced by S's opinions/comments in this forum   (CORE)
       Newcomers = assisted: Constellation getBacklinks(subject=forum, source=NS.membership:forum),
                   Jetstream NS.membership events with rkey == fk, indexer listParticipants,
                   relay listReposByCollection(NS.membership) only while the ecosystem is ≤ 5,000 repos
  each non-authority candidate is confirmed by getRecord(NS.membership, rkey=fk) at its PDS
  sync order: A → authors on screen → recently active (Constellation NS.post:forum newest 100, Jetstream 36 h replay)
              → S → Closure → Newcomers (budget 500, most recent first)
  the fold re-runs as repos complete; aggregates show [partial k/n] until S ∪ Closure are complete
```

**Closure property.** E (eligible voters, §4.6) depends only on seeds' votes, and any newcomer who earns seed upvotes is referenced by seed records. So karma, scores and every outcome that depends on E are the same with or without CORE+ discovery. Only collapsed newcomer content differs.

**Applications.** In approval mode, mods find applications only through CORE+ or out of band, and the UI says so.

**Forum directory:**
1. Relay `listReposByCollection(NS.forum)` across configured relays.
2. `listRecords(NS.forum)` on each owner.
3. Indexer `listForums`.

Results are grouped by owner handle (F9).

### 3.3 Per-repo sync (L1)

```
sync_repo(did, scopes):                                   // scope = fk of a followed forum, or GLOBAL (profile, forum decls)
  head = getLatestCommit(did)                             // {cid, rev}
  if head.rev == repo.rev && every scope listed at repo.rev → Clean
  if head.rev < repo.rev → Rollback: keep cache, back off, flag after 24 h
  colls = (repo.colls if checked ≤ 24 h ago else describeRepo(did).collections) ∩ OURS
  for (coll, fk) in colls × scopes:                       // parallel, per-host limits
     rows = list_range(did, coll, fk)
     for r in rows: require cid(json→dagcbor(r.value)) == r.cid; validate(coll, rkey, value) else drop+log
     diff vs stored(fk, coll, did) → upserts; keys missing from a COMPLETE listing → tombstones
  membership: getRecord(NS.membership, fk) when not covered
  head2 = getLatestCommit(did); if head2 != head → requeue (P1)
  one atomic batch: records, tombstones, repo{rev: head.rev, listed[fk]: head.rev, checkedAt}
list_range(did, coll, fk):                                // cursors are opaque by spec → probe and remember per host
  if host.rangeCursor != No:
     page = listRecords(coll, limit 100, cursor "{fk}.")  // reference PDS: descending, rkeys < "{fk}."
     if any key ≥ "{fk}." → host.rangeCursor = No; goto full
     continue paging while every key starts with "{fk}-"
  full: page through the whole collection and filter by prefix
```

**Caps:** per repo and forum, at most 20k records and 50 MB. Cursors must strictly advance. Every response is byte-counted after decompression (§5.3).

**Deletes** are applied only after a complete listing, or after `getRecord` returns `RecordNotFound`.

**Own writes** are applied optimistically from the `applyWrites` results (`getLatestCommit` if `commit` is absent), and the repo is marked dirty.

**Single records:**
- `getRecord` for dangling strongRefs.
- `sync.getRecord` for `verify`, `--verify proof`, and the first open of a forum declaration.

### 3.4 Live and incremental updates

**One dirty queue.** Every change source feeds `(did, minRev?, priority, reason)` items, coalesced per repo:
- Jetstream;
- indexer `getChanges`;
- polling;
- own writes;
- discovery;
- view open.

| Priority | Content | Debounce and jitter |
|---|---|---|
| P0 | content on screen | 0–5 s |
| P1 | posts and comments | 2–30 s |
| P2 | opinions | 30–600 s |
| P3 | reconcile | scheduled |

**Jetstream (assisted mode)**
- Connect to `wss://jetstream.{us-east,us-west}.bsky.network/subscribe?wantedCollections=NS.*&requireHello=true&cursor=<time_us − 5 s>`.
- Default: no DID filter. Events are filtered client-side by the fk prefix of the rkey. This is cheap, and it also hides which forum you read.
- When lessdugin-wide traffic exceeds about 50 events/s, switch to `options_update{wantedDids ≤ 10k}` plus a second connection that carries only membership events.
- Cursor handling: `time_us` is persisted every 5 s or 1,000 events; on reconnect, rewind 5 s and deduplicate through an LRU.
- Reconnect backoff: 1 s doubling to 60 s, with jitter. Fail over to the other endpoint after 3 failures.
- A gap longer than 36 h triggers a full reconcile.

**Event handling**

| Event | Action |
|---|---|
| create or update | point fetch: with an indexer, batch `getRecordProofs` (≤ 25 per 1–3 s); otherwise PDS `getRecord` after jitter |
| delete | `getRecord` → 404 → tombstone |
| identity | invalidate identity caches |
| account | confirm with `getRepoStatus` |

**Polling** (direct mode, or Jetstream down): `getLatestCommit` with ±20% jitter.

| Repo class | Interval |
|---|---|
| authorities, while the forum is open | 2 min |
| hot (changed ≤ 1 day ago) | 10 min |
| warm (≤ 30 days) | 1 h |
| cold | 12 h |

Opening a view triggers a P0 head check for authors on screen whose last check is older than 5 min.

**Reconcile** always runs: a full re-list of every participant spread over 24 h, and of authorities hourly.

### 3.5 Write path (outbox)

**Outbox.** `PendingWrite{account, ops, intent, state}` is persisted in the state store before any network call. The pump:
- backs off on 429 and honours `RateLimit-*` and `Retry-After`;
- tracks the write budget (5,000 points/h and 35,000 points/day; create 3, update 2, delete 1);
- refuses to send a write stamped with a different account DID;
- holds opinion writes 4 s so they can be undone or coalesced.

| Operation | Protocol |
|---|---|
| First content in a forum | `applyWrites[create membership (if absent), create post or comment]`. If the batch fails because the membership already exists, refresh and retry without it. |
| Edit | `applyWrites[create revision{subject: old sref, op: update, record: old value}, update target]` with `swapCommit = head`. On `InvalidSwap`, re-read the target: same CID → rebuild and retry (≤ 3, 200–800 ms jitter); changed CID → real conflict, shown side by side. |
| Edit with `--no-history` | `putRecord` with `swapRecord = oldCid` |
| Delete | `applyWrites[delete target, delete its revisions]` (≤ 200 ops, chunked) |
| Opinion | A pure function of (current value, intent). No record → `createRecord(rkey)` (fails if it exists, which gives create-if-absent). Record exists → `putRecord(swapRecord = cid)`. Result empty → `deleteRecord(swapRecord)`. On a swap failure, re-read, re-apply the intent per axis, retry. |
| Mod action | as for an opinion. `mod adopt` writes `applyWrites` batches of ≤ 200, choosing create or update from a freshly listed own range, with `swapCommit` retry. |
| Blob | Uploaded at publish time only. The record embeds the blob object returned by the PDS verbatim. A local CID is for preview only and must equal the returned `ref.$link`. On 413, downscale or refuse. |

### 3.6 Verification (`proofs/1`)

```
verify_proof(car, did, coll, rkey, doc) -> Proven{rev, cid} | ProvenAbsent{rev} | Err
  own CARv1 parser (caps: 1 MB, ≤ 2,000 blocks, no preallocation from declared lengths)
  commit = dagcbor(root): version 3, commit.did == did, rev ≤ now + 5 min
  ECDSA low-S verify (k256 | p256 from doc #atproto multikey) over dagcbor(commit without sig)
  walk MST from commit.data toward "coll/rkey": every node sha256 == cid, keys sorted,
       layer == leading-zero-pairs(sha256(key)), depth ≤ 32
  leaf → record cid; record block hashes to it; or prove absence
```

**Indexer CARs.** `getRecordProofs` returns a CAR whose root is a manifest `{v:1, items:[{uri, cid|null, commit}]}`, followed by deduplicated blocks. Items are verified independently; a failed item is dropped and counted against the source.

**Rules**
- Never advance a stored rev past the head observed at the PDS on the strength of a third-party proof.
- A proof that fails against the current key but whose rev is older than the PDS head is `stale-proof`: fetch a fresh proof from the PDS and do not count it against the source (key rotation).
- `lessdugin verify <ref> --deep` also verifies the referenced strongRefs, revision hashes (`cid(revision.record) == revision.subject.cid`) and blob CIDs.

### 3.7 Scheduler and budgets

**Concurrency**

| Limit | Native | Browser |
|---|---|---|
| Requests in flight | 32 | 16 |
| Per host | 6 | 6 |
| PLC | 8 | 8 |
| DoH | 8 | 8 |

**Retries.** GETs retry 3 times with full-jitter exponential backoff (250 ms base, 8 s cap) on network errors, 5xx and 429.

**Rate-limit budget.** The Bluesky PDS budget is 3,000 requests per 5 min per IP. S9 measures whether that budget is shared across hosts. Background work may use at most 50% of the remaining budget; P0 work may use up to 80%.

**Circuit breaker.** A host is tripped after 10 consecutive failures and probed again after 60 s, doubling up to 1 h.

**Timeouts.** Connect 5 s; request 20 s; CAR 60 s.

**Request headers.** Cross-origin browser GETs carry no custom headers, so there are no CORS preflights. Native requests send `User-Agent: lessdugin/<ver> (+https://lessdugin.7778777.online)`.

**User-visible budgets**
- Interactive commands use `--time-budget 10s` by default, then print `[partial …]`.
- Other flags: `--max-requests`, `--offline`.
- The SPA estimates the cold-start cost and, above about 1,500 requests, offers the forum's indexer.

### 3.8 Cache and store

**Stores**
- **`cache` (disposable):** records, repo state, identities, tombstones, blobs, cursors.
- **`state` (durable):** outbox, drafts, read marks, subscriptions, settings, notification watermarks, muted users.
- **Sessions:** the OS keyring natively; IndexedDB (non-extractable keys) in the browser.

| # | Invariant |
|---|---|
| I1 | Deleting `cache` loses only time. |
| I2 | Every stored record and blob hashes to its CID; bytes are immutable. |
| I3 | Per uri, the current version only moves forward in rev. The same rev with a different CID is a conflict: keep the higher level and log the source. |
| I4 | A tombstone at rev R suppresses observations below R, and is kept ≥ 30 days. |
| I5 | The record store holds only `pds` or `proven` records. Hints live in memory and are never persisted as records. Level only increases for the same (uri, cid). |
| I6 | Claims of completeness require every relevant repo listed at rev ≥ the last known dirty rev; otherwise `[partial]`. |
| I7 | Moderation inputs are read only from the record store. |
| I8 | Records of DIDs that are not active are excluded. |
| I9 | One background-sync leader per cache: a native `fd-lock` on `cache.lock`, or the browser Web Lock `lessdugin-sync-leader`. Other processes and tabs do P0 fetches under per-repo locks. Writes are idempotent and rev-monotonic. Changes are broadcast over BroadcastChannel. |

**Eviction.** Whole forums are evicted, least recently viewed first, above 2 GB native or 500 MB browser. Never evicted: identities, the user's own repo, and authority repos of followed forums.

### 3.9 Blobs and media display

**Fetch and verify.** Media is fetched from the author's PDS with `sync.getBlob` and its sha256 compared with the raw CID.
- Browser: `new Blob([bytes], {type: ALLOWLIST_CONST})`, then `URL.createObjectURL`, revoked on unmount.
- Native: stored as `blobs/<cid[0..2]>/<cid>`.

**Rendering**
- The type comes from the declared mime in the verified record, which must agree with the sniffed type and be on the allowlist.
- Shown inline: png, jpeg, gif, webp, avif (the SPA only).
- Video and audio: click to load, full download, then verify.
- SVG: never inline in v1.
- Everything else: a download card that saves as `<cid>.<ext-from-sniffed-type>`.

**Autoload policy (F29).** Media loads automatically only if all of these hold:
- the author is in S ∪ E;
- the item is visible;
- it has no `sexual`, `nudity`, `porn` or `graphic-media` self-label;
- the PDS host serves ≥ 5 participants of the forum, or matches `*.host.bsky.network`, or is on the user's allowlist.

Otherwise the user clicks through a notice: "loads from <host>, which will see your IP".

**Caching and limits**
- Hidden or pending content is never prefetched.
- Cached blobs are purged when their referencing records are hidden or deleted (reference-counted).
- Images up to 5 MB load automatically; larger ones are click-to-load.

### 3.10 Cost model and indexer thresholds

**Assumptions**, per participant per year: about 80 records and 40 KB (1.5 posts, 15 comments, 60 opinions). A cold sync costs about 6 requests per participant (PLC, `describeRepo`, about 3 range listings, membership). Handle lookups over DoH happen only for authors on screen.

| Participants | Cold requests | Wire | Native cold | Browser cold | Warm, assisted | Direct polling |
|---|---|---|---|---|---|---|
| 50 | ~300 | ~2.5 MB | ~2 s | ~4 s | one point fetch per write, plus 50 reconciles/day | ~80 req/h |
| 500 | ~3,000 | ~25 MB | ~15 s (†) | ~40 s (†) | writes + 500/day | ~1.4k req/h |
| 5,000 | ~30,000 | ~250 MB | 3–50 min (†) | indexer needed | writes + 5k/day | not recommended |
| 50,000 | ~300,000 | ~2.5 GB | indexer required | indexer required | n/a | n/a |

(†) If Bluesky hosts share one 3,000-requests-per-5-min budget per IP, about 3,000 requests use a whole window. S9 decides which case holds.

**Fan-out at the author's PDS.** Without an indexer, every write causes about one request per online reader at the author's PDS. With an indexer, readers fetch proofs from the indexer instead, at no loss of trust.

**Thresholds**

| Size | Guidance |
|---|---|
| ≤ 300 participants | serverless everywhere |
| 300–3,000 | native is fine; the forum should declare an indexer for SPA first visits, global search and offline notifications |
| > 3,000 participants, > 10k DIDs, > 300 concurrent readers or > 1M records | the forum should declare ≥ 1 indexer (preferably 2); the SPA defaults to the indexer for cold starts; direct mode stays available for audit |

**Cold start with an indexer:**
1. Forum resolution: 3 requests.
2. `listPosts`: titles render as placeholders.
3. `getRecordProofs` for the visible posts (~30), in 1–2 requests.
4. Text appears verified in about 1 s.

---

## 4. Social semantics (`docs/spec/social.md`)

### 4.1 Fold contract and layers

```
FoldInput { decl, records: Map<uri,(cid,value)>, obs: Map<(uri,cid),micros>, repoStatus: Map<did, Fresh|Stale|Unavailable>, now }
inputDigest = sha256(sorted (uri ‖ 0 ‖ cid ‖ "\n")* ‖ declCid ‖ algorithmIds ‖ params)
```

**Determinism rules**
- Collections are `BTreeMap` or sorted vectors.
- Time is integer microseconds. Every tunable is an integer.
- Floats appear only in `hot()`, via the `libm` crate (pure Rust, bit-identical on native and wasm).
- Ties break by uri bytes, then cid bytes.
- `now` is always an explicit argument.

**Layers** (each reads only lower layers):
- **L0** authorities
- **L1** forum-tier moderation state
- **L2** participation and seeds
- **L3** karma (K1, E, KP, K)
- **L3.5** scores and agreement
- **L4** gates
- **L5** author-tier moderation, reacts, tags, wiki
- **L6** visibility, threads, ranking
- **L7** the viewer's lens

**Unknown is not absent.** An `Unavailable` repo contributes its last cached records. A record leaves the input only after a complete listing or a confirmed delete.

### 4.2 Time

```
t_claim = createdAt (unparseable → None);  t_tid = TID time of the record's own TID segment
t_eff   = min_some(t_claim, t_tid);  if t_obs known and t_eff > t_obs + 5 min → t_eff = t_obs, flag future-dated
lower(r): comment → t_rank(parent or root); post, wikiRevision, sequence → decl.createdAt
t_rank  = max(lower(r), t_eff)
```

- Honest content (whose claimed time is at most 5 min after it was observed) gets the same `t_rank` on every client.
- Backdating only hurts the author.
- Future-dated items are not hidden; they are shown at their first-seen time with a `[future-dated]` badge.
- `t_obs` is the local first-seen time, or the indexer's `firstSeen` when an indexer is used. Parity tests inject it.
- Account age (P2 locks and review rules) comes from the PLC genesis op; did:web uses first-seen.
- Display-only flag: `|t_claim − t_tid| > 1 h` shows "inconsistent timestamps".

### 4.3 Participation, membership modes, seeds

| | approval | open |
|---|---|---|
| Participant (content visible) | authority, or granted (`#access member`) **and** has own membership record | authority, or has own membership record and is not banned |
| Seeds S | authorities ∪ granted, minus banned/nullified | authorities ∪ granted, minus banned/nullified |
| Newcomer (not in S ∪ E) | n/a (not a participant) | shown according to `display.newUsers` (default `collapse`, labelled "new user") |
| Completeness | provable (CORE) | S ∪ E complete under CORE (closure); newcomers best-effort |

- **Consent.** Granting requires a human decision. There is no automatic sweep. The batch-grant UI shows account age and activity.
- **Leaving.** A user leaves by deleting their membership record, which hides their content in the default view. Their grant still counts for voting eligibility, so other people's karma survives a departure unless the user deletes their opinions.
- **Moderator badge.** A listed moderator is *shown* as a moderator only if they have a membership record (F9). Their authority does not depend on it.

### 4.4 Moderation fold (`mod/1`)

**Ranks:** owner 3, admin 2, moderator 1, post author 0 (author scope only). Only the *current* declaration confers authority. Assertions by former moderators lapse at once. `lessdugin mod adopt <did>` re-issues them under the adopting moderator, with `adopts` pointing at each original.

**Authorization**
- An assertion `a` by actor X on target T is authorized if any of these holds:
  - X is the owner;
  - X is T's author;
  - `rank(X) > rank(author(T))`, where the rank of a DID subject is the rank of that DID.
- The action must also be allowed for X's role:

| Action | owner / admin | moderator | post author (scope `author`, gated by §4.8) |
|---|---|---|---|
| hide (content) | ✓ | ✓ | `hidden=true` on comments in own thread |
| hide with `spam` | ✓ | ✓ | – |
| access (banned/member) | ✓ | ✓ | `banned` only; applies only to own threads |
| restrict, nullify | ✓ | ✓ | – |
| tier personal/frontpage | ✓ | ✓ | – |
| tier curated | ✓ | – | – |
| pin (forum scope) | ✓ | ✓ | – |
| pin (thread scope) | ✓ | ✓ | own thread |
| lock post | ✓ | ✓ | own post |
| lock forum (read-only) | ✓ | – | – |
| tag | ✓ | ✓ | – |

**Dimensions** (the safe value is listed first):

| Dimension | Values | Positive, so CID-bound (F20) |
|---|---|---|
| hidden (+reason, showReason) | true < false | `false` (keep despite a ban or a lower-rank hide) |
| spam | true < false | – |
| access | banned < member | – |
| restrict | set union | – |
| nullify | true < false | – |
| tier | personal < frontpage < curated | ≥ frontpage |
| pin[scope] | false < true | `true` |
| locked (+allowed) | true < false; allow-lists of equal-rank locks are intersected | – |
| tag.removed | true < false | – |
| tag.head, tag.aliasOf | unordered | head is CID-bound by its strongRef |

**Resolution**

```
resolve(subject, dim):
  xs = assertions where authorized ∧ actor ∈ current authorities (or author tier valid)
       ∧ rkey canonical ∧ !(until ≤ now) ∧ (negative ∨ subjectCid == currentCid(subject))
  stale positive (cid mismatch) → mod-queue item "edited since <action>"; author warned before editing
  r = max rank in xs;  vs = values at rank r
  ordered → safety-min(vs);  unordered → (all equal ? v : none);  none → default (tier = decl.defaultTier)
```

**Content-level decisions over user-level ones.** An item-level `hidden=false` at rank ≥ the ban's rank keeps that item visible despite a ban.

**Ban (F21).** A resolved forum-tier `access=banned` hides *all* of that user's content in the forum. Item-level keeps override. P2 adds `#access.keep` snapshots.

**Lock (F21).** Comments on a locked post are hidden unless they are in `allowed` or written by an authority. The mod's client writes the currently visible comment uris into `allowed`. A forum-uri lock makes the forum read-only for non-authorities.

**Fail closed.** An authority repo that has never synced, with nothing cached, puts the view in `degraded`: only content by S (derived from the authorities that did sync) is shown, under a banner.

**Mod log and raw view**
- The mod log shows every modAction from current and former authorities, plus `revision`s of modActions, sorted by `updatedAt`.
- The raw view shows everything, annotated with every assertion, including lapsed ones and invalid keys.

**Author moderation outside threads.** An author-scope `access=banned` hides that user's comments in all of the author's threads in this forum. A moderator who is also an author can issue both forum-scope and author-scope assertions, because scope is part of the key.

### 4.5 Opinion validity (shared by every axis)

An opinion v counts if all of the following hold:
1. Its rkey is canonical.
2. `subject.uri` is canonical, and the subject is a post, comment, wikiRevision or sequence.
3. The subject exists in the input, with forum equal to `v.forum`. The exception is the deleted-subject rule below.
4. The voter is not the subject's author.
5. The voter is not banned or nullified, and is not restricted from `vote` or `react`.
6. The subject is not resolved as spam.

`subject.cid` does not affect counting. It drives the "N of M votes were cast on an earlier version" display.

**Deleted-subject rule (F24).** The subject is *confirmed absent*: the author's range was completely listed, or a tombstone exists. Its votes still count toward karma only if they come from ≥ 2 distinct voters in E. They never count toward a score, since there is no item.

### 4.6 Vote power and `karma/1`

```
small(k)  = k ≥ 1000 ? 2 : 1
strong(k) = first t in [(500000,16),(250000,15),(175000,14),(100000,13),(75000,12),(50000,11),(25000,10),
            (10000,9),(5000,8),(2500,7),(1000,6),(500,5),(250,4),(100,3),(10,2)] with k ≥ t, else 1
power(k, v): k = max(k,0); up|agree → small(k); down|disagree → −small(k); strongUp|strongAgree → strong(k); strongDown|strongDisagree → −strong(k)
sign(v)  = ±1

pairsum(basis, eligible):       // karma axis only; subjects of kind post|comment (+ deleted-subject rule)
  acc[(voter, author(subject))] += basis ? power(basis[voter], v.karma) : sign(v.karma)   for v ∈ V, voter ∈ eligible
  out[r] = Σ_voter clamp(acc[(voter,r)], ±pairCap · (basis ? strong(basis[voter]) : 1))

K1 = pairsum(None, S)
seedUp[u] = |{ s ∈ S : Σ_{v by s on u's items} sign(v.karma) > 0 }|
E  = S ∪ { u ∈ Participants∖S : K1[u] ≥ establish ∧ seedUp[u] ≥ establishSeeds }   // open default 2; approval 1
KP = pairsum(Some(K1), E)          // power basis
K  = pairsum(Some(KP), E)          // displayed karma; gates use K
```

**Properties (each property-tested)**
- Order-independent.
- Acyclic: each stage reads only earlier ones.
- O(|V|) per stage.
- Sybil theorem: records from DIDs outside E change no output for anyone else.
- Removing one voter changes any recipient's K by at most pairCap times that voter's strong power.
- A seed with negative K1 is still in E.

**Other rules**
- `forum create` writes the policy explicitly.
- A local alternative algorithm is a lens only ("my view") and is never used for gates.
- Co-author credit comes later as `karma/2`.

### 4.7 Scores and agreement

```
self(i)      = author ∈ E ? (post ? strong(KP[a]) : small(KP[a])) : 0          // FM: authors' own strong/normal upvote
score(i)     = self(i) + Σ_{valid v on i, voter ∈ E} power(KP[voter], v.karma)  // no pairCap on items
agreement(i) = Σ_{valid v on i, voter ∈ E} power(KP[voter], v.agreement)        // never feeds karma
unweighted(i)= |{valid v on i : voter ∉ E}|        // shown as "n votes not yet counted"
```

- Voting systems: `karmaOnly` ignores agreement and reacts; `twoAxis` ignores reacts; `reacts` uses everything. Axes that are not enabled are still stored.
- A voter outside E sees "recorded; will count once you're established (applied retroactively)".

### 4.8 Gates (current K; authorities bypass every gate)

| Gate | karma/1 default (absent field) | `forum create` (open) | `forum create` (approval) |
|---|---|---|---|
| Weighted vote | ∈ E | ∈ E | ∈ E |
| reactSecond / reactCreate / reactAnti | 5 / 10 / 20 | 5 / 10 / 20 | 0 / 0 / 5 |
| authorModPersonal / authorModFrontpage | 50 / 2000 | 50 / 2000 | 10 / 100 |
| createTag / wikiEdit | 10 / 5 | 10 / 5 | 0 / 0 |
| sequence (library listing, or a mod pin) | 1000 | 1000 | 0 |

`tagCreation=moderators` restricts genesis revisions to authorities.

### 4.9 Reacts (`default/1` palette in `docs/spec/reacts-default-1.json`)

**Palette IDs:** agree, disagree, important, insightful, changed-mind, crux, not-a-crux, locally-valid, locally-invalid, typo, wrong, misleading, unclear, dont-understand, examples, citation-needed, concrete, overconfident, underconfident, hits-the-mark, missed-the-point, strawman, steelman, scout-mindset, soldier-mindset, too-harsh, good-point, thanks, seen, will-reply-later, laugh, heart, surprise, empathy. Unknown IDs are ignored.

```
anchor(r) = resolve(quote) in P(current subject) → range | "orphan:" + qkey | none     // qkey = NFC(collapse_ws(trim(exact)))
within one opinion: dedupe by (react, anchor); for+against contradiction drops both
per (subject, react, anchor):  F = {u∈E∖authors : stance=for},  A = {… against}
  valid ⇔ ∃u∈F: K[u] ≥ reactCreate
  supporters = {u∈F : K[u] ≥ reactSecond};  opposers = {u∈A : K[u] ≥ reactAnti}
  creator = argmax_{supporters}(K, −did); everyone else "seconded"
```

Reacts never affect karma. The margin UI for inline reacts comes in P2; anchors are stored and shown in a list from the MVP.

### 4.10 Visibility (L6) and lens (L7)

The first matching rule wins.
1. Invalid or out of scope → `Excluded`.
2. Author not a participant → `Hidden(NotParticipant)`.
3. Resolved `hidden` → `Hidden(Removed{rank, actor?, reason if showReason})`.
4. Author banned at forum tier, with no keep at ≥ that rank → `Hidden(Banned)`.
5. Author tier effective: author-hidden, banned from the author's threads, or locked and not allowed → `Hidden(AuthorModerated)`.
6. Forum-tier lock (post or forum), not allowed, author not an authority → `Hidden(Locked)`.
7. Open mode, author ∉ S ∪ E, `newUsers = collapse | hide` → `Collapsed(NewUser)` or `Hidden(NewUser)`.
8. `score ≤ collapseBelow` → `Collapsed(LowScore)`.
9. Otherwise `Visible`.

**Thread pass.** A hidden node that has a non-hidden descendant becomes `Tombstone(reason)`, so threads keep their shape.

**Lens**, applied after the fold: local mutes (`Collapsed(Muted)`), optionally honouring one's own `app.bsky.graph.block` records, and the raw view.

### 4.11 Ranking (`magic/1`) and sorts

```
hot(b, t) = b / libm::pow(max(now−t,0)/3.6e9 + offsetHours, exponentMilli/1000.0)
post b    = score + (curated ? curatedBonus : 0) + (tier ≥ frontpage ? frontpageBonus : 0)
magic key = (pinned desc, b>0 desc, b>0 ? hot desc (total_cmp) : (b desc, t_rank desc), uri asc); only t_rank within cutoffDays
```

**Post sorts**

| Sort | Order |
|---|---|
| magic | key above |
| top | (score desc, t_rank desc, uri) |
| new / old | t_rank |
| recentComments | maximum t_rank among visible comments |

**List filters**
- Frontpage: tier ≥ frontpage. This is the default list.
- All: everything visible.
- Curated: tier = curated.
- `quickTake` posts appear only in All, with a badge.

**Comment sorts:** magic (same formula on score), top, new, old, latestReply (maximum t_rank in the subtree). Thread pins come first.

**Recent Discussion** (MVP): posts ordered by the t_rank of their latest visible comment, each with its newest 3 visible comments.

**Deferred:** `topAdjusted`, personalised magic (`magic/2`), tag-filter weights.

### 4.12 Threads

1. Keep comments whose `root.uri` is the post.
2. A missing parent, or a parent under another root, gives a `Missing(uri)` placeholder under the root.
3. **Cycles** (possible because `parent` can be edited): walk parent chains in uri order, cut the edge leaving the greatest-uri member of each cycle, re-attach it to the root, and flag it `Orphaned`.
4. The depth cap is 1000. Display indentation is capped at about 6 levels (TUI and mobile), then "continue thread →".
5. `t_rank` is computed top-down after cycles are cut.
6. A comment pinned to an older CID shows "on revision n" or "on an unavailable version".

### 4.13 Tags and wiki

**Identity.** A tag is `(forum, slug)`. A page is the set of `wikiRevision` records sharing it.

**Head selection** (H3: ancestry by uri, independent of ancestors' integrity):

```
candidates = revisions of (forum, slug) whose author meets wikiEdit (genesis: createTag / authority if tagCreation=moderators),
             is not restricted(wiki), and that are not hidden
edges      = parents resolved by uri among candidates (cid mismatch → "ancestor altered" note); cycles cut as in threads
heads      = candidates with no candidate child;  depth = longest path through edges
canonical  = resolved #tag.head if it is a candidate, else argmax_heads (depth, K[author], uri asc)
exists     ⇔ canonical exists ∧ ¬resolved tag.removed;   other heads shown as "N unmerged edits"; merges = multi-parent revisions
```

**Relevance**

```
rel(post, slug) = (slug ∈ post.tags ? max(1, small(KP[author])) : 0) + Σ_{v∈valid opinions on post, voter∈E∖authors} power(KP[voter], v.tags[slug])
```

A tag applies if the tag exists and `rel > 0`. `#tag.aliasOf` merges a slug into its target.

The walk is capped at 1,000 revisions per page.

### 4.14 Sequences

- A sequence is an ordered list of chapters of post uris. Posts may be by other authors.
- **Library listing:** the author's K ≥ the `sequence` gate, or a forum-scope `#pin`; and the sequence is not hidden. Library order is by sequence score (opinions on the sequence, which earn no karma).
- **Next/previous:** use the sequence the reader came from, else the post author's own sequence, else the highest-scoring one.
- "Continue reading" is local state.

### 4.15 Other content types

| Feature | v1 status |
|---|---|
| Q&A | `kind=question` gets a badge in the MVP. Answers (`comment.answer`) and accepted answers come in P2. |
| Quick takes | `kind=quickTake`: untitled post. The MVP renders it; the dedicated feed comes in P2. |
| Link posts | MVP (`kind=link` + `link`). Cards are entered manually or fetched by the CLI. The SPA does not fetch cards: it has no CORS proxy. |
| Side comments | `comment.quote` is MVP data. The margin UI comes in P2. |
| Co-authors | P2: `post.coauthors` plus a CID-bound `coauthor` acceptance, with `karma/2` credit. |
| Dialogues | P2: `turn` records. The UI warns that drafts are public. |
| Events and polls | P2/P3: community calendar records and `pollVote`. |

### 4.16 Notifications, subscriptions, read state

All of this is computed locally at sync time and live via Jetstream. An item is new when `t_obs > watermark`; `createdAt` is never used, so backdating cannot hide an item.
- **MVP reasons:** Reply, Mention, Quote (`comment.quote` on my content), NewComment (subscribed post), NewPost (subscribed forum, user, tag or sequence), Reacted (aggregated), ModAction (diff of resolved state affecting me), KarmaChange (batched realtime, daily or weekly).
- **Default filter:** authors in S ∪ E.
- **Unread comments:** `t_obs > lastVisited[post]`.
- **Indexer `getActivity`:** offline and cross-forum mentions (hint, then point fetch).

### 4.17 Public versus local

| Data | Where |
|---|---|
| Posts, comments, revisions, wiki, sequences, profile | public repo |
| **Opinions (votes, agreement, reacts, tag votes), memberships with notes, mod actions with reasons** | **public repo.** The UI shows an interstitial on first use per action type, and a "what's public about me" view (P2). |
| Drafts, read marks, subscriptions, bookmarks, mutes, notification state, tag filters, karma history | local `state` store; JSON export/import |
| Public bookmarks and subscriptions, cross-device sync, private ballots (tally extension) | P2+ |

### 4.18 Rate limits and reports

- **Automatic and manual rate limits:** deferred to P2 as `ratelimit/1` (best-effort; documented as advisory, because time is self-asserted).
- **Reports:** MVP offers only `forum.reportContact` (mailto, https, or an at:// handle for DMs). P2:
  - sealed `report{ciphertext ≤16 KiB}` using age v1 with X25519 anonymous recipients and padding, with **no** forum, subject or recipient fields, and a random publication delay of 0–6 h;
  - or a forum report service via `createReport`, scope `rpc:…createReport?aud=*`.

---

## 5. Security and privacy

### 5.1 Threat model (`docs/spec/threat-model.md`)

**Adversaries**
- hostile participants, including self-hosted PDSes that equivocate or cloak;
- sybil farms;
- lying third parties (relay, Jetstream, Constellation, indexer);
- network attackers;
- malicious markdown and media;
- a compromised hosted SPA or domain;
- XSS through dependencies;
- another local user (`lessdugin web`).

**Inherent limits, stated in the UI and documentation**
- Votes and mod actions are public and permanent in archives.
- Reading reveals your IP to self-hosting participants' PDSes when media loads, and to every PDS you sync.
- Signed content is non-repudiable.
- Time-based rules are best-effort.
- The hosted SPA is trusted while you use it; `lessdugin web` is the zero-trust path.

### 5.2 Measures

| Area | Measures |
|---|---|
| Terminal (F1, F2) | `TermText` newtype, built only by `term_sanitize`: C0 controls (except `\n`, `\t`), DEL and C1 become visible `^[` or `\u{..}`; U+2028 and U+2029 become a space; bidi controls are stripped (shown as visible `⟨RLO⟩` markers in code); names lose zero-width characters and are grapheme-capped. OSC 8 links only for `safe_href` URLs, with the host shown when the text differs. Content never sets the terminal title. A sanitising `tracing` writer. On a TTY, `--json` and `--raw` escape C1 and bidi characters; piped output stays byte-exact. Editor templates: ≥ 6 fixed trailing lines, quoted lines prefixed `# ` with modelines neutralised, files 0600 inside 0700, serde-saphyr alias and budget limits. |
| Local files (F3, F4) | Draft image paths must canonicalise inside the draft's own directory; absolute, `~`, `..` and special files are rejected; the sniffed type must be on the allowlist; publish confirmation lists every local file, and `--yes` is not honoured for foreign drafts. Downloads are saved as `<cid>.<ext>`; `open` works only for image, video and audio; Windows gets Mark-of-the-Web; no shell is ever used. |
| SPA (F5–F8) | CSP below plus Trusted Types (`lessdugin-md`, `dompurify`). DOMPurify is configured with exactly the renderer's allowlist, and vectors assert `DOMPurify(html) === html`. `<SafeLink>` with `safe_href` from the core, plus a lint rule banning raw `href`. `<bdi>` for names. MathML allowlist and disabled TeX macros. Referrer `no-referrer`; JS frame guard. |
| Poison pills (F8) | Iterative walkers; depth limits checked before parsing (CBOR/JSON ≤ 32, markdown ≤ 64, TeX); no preallocation from declared lengths; native `catch_unwind` per record. The worker writes in-flight `(uri, cid, RENDERER_VERSION)` to IndexedDB, so after a crash they are quarantined as "failed to render" and the worker restarts with backoff. Fuzzing (cargo-fuzz) on the CAR, CBOR, MST, lexicon validator and renderer. |
| Identity (F9–F11) | DoH by default; bidirectional handles; confusable warnings; mention styling only for verified `at://did`; when link text looks like a URL, the real host is shown; TOFU auth-server pin; third-party proofs need a doc ≤ 1 h old; `rev ≤ now + 5 min`; owners or moderators on did:web are warned (domain expiry means takeover). |
| Infrastructure (F13–F17) | Hints never become text. Byte caps are counted while streaming, after decompression. Every Jetstream message is capped at 1.2 MB. Equivocation: record `(did, rev, cid)` and compare against Jetstream commits; a fork shows a warning (P2: evidence attached to a hide). Fan-out: identities resolved lazily only for participants; forum endpoints need consent, https, a hostname and the default port; per-domain budgets. Indexer: FTS tokens quoted and capped at 16 terms; snippets as plain text; takedown blocklist; SSRF guard (re-checked on every redirect, resolved IP pinned, private and v4-mapped ranges blocked); no `immutable` caching of record data; no blob storage. |
| Social (F18–F24) | Seeded `karma/1` with `establishSeeds = 2`; grants are explicit; positive assertions are CID-bound; a ban hides everything; locks carry allow-lists; fail closed; ≤ 32 reacts per opinion; canonical uris; the deleted-subject rule; wiki walks capped. |
| Auth (F25–F28) | No `transition:generic`. No main passwords (the `xxxx-xxxx-xxxx-xxxx` shape is enforced; app passwords are taken by prompt, stdin or environment variable, never argv, and never persisted). Enumerated blob scopes. Native sessions in the OS keyring (`keyring` crate); the file fallback is 0600 with `O_NOFOLLOW` and refuses lax permissions. Tokens and DPoP proofs redacted in logs. `UserPds` requests never follow redirects and only go to the PDS bound at login. Browser: non-extractable WebCrypto keys in IndexedDB, and Web Locks around refresh. AuthBridge allowlist: `com.atproto.repo.{createRecord, putRecord, deleteRecord, applyWrites, uploadBlob}` on NS collections only, plus `server.getSession`. `web` mode keeps no secrets in the browser (§6.13). `logout --purge`. |
| Privacy (F29–F34) | Media autoload policy (§3.9). Media cache purged on hide or delete. The editor warns that history stays public; "replace without history" and "redact revisions" (delete revision records, so their blobs are garbage-collected); a missing snapshot shows "history redacted". `export` defaults to `--mine`. Metadata stripping by allowlist rebuild (JPEG keeps only image segments and ICC APP2; PNG drops tEXt, iTXt, zTXt and eXIf; WebP drops EXIF and XMP; GIF drops comment and application extensions except NETSCAPE loop). MP4 `udta` and `meta` boxes are overwritten as `free` boxes, so no offsets shift. `media.name` is opt-in. HEIC is not accepted natively (the SPA converts via canvas). Public-action interstitials. |
| Supply chain (F35, F36) | Domain hardening; reproducible SPA and wasm builds with published hashes; `lessdugin verify-site <url>`; build hash shown in Settings. pnpm `minimumReleaseAge = 7d`, `onlyBuiltDependencies: []`, lockfile-lint, osv-scanner. cargo-vet plus exact `=` pins for the parsing and crypto crates; `cargo auditable`. GitHub Actions pinned by SHA, with `permissions: contents: read`, protected release and Pages environments, and no `pull_request_target`. TUI images use pure-Rust decoders only, limited to 64 MB, 8192² pixels and 200 GIF frames, decoded off the UI thread with a timeout. |

**SPA CSP** (sent as a header by `lessdugin web` and Cloudflare/Netlify `_headers`; a `<meta>` tag on GitHub Pages):

```
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self'; font-src 'self';
img-src 'self' blob:; media-src blob:; connect-src 'self' https: wss:; object-src 'none'; base-uri 'none';
form-action 'none'; frame-ancestors 'none'; require-trusted-types-for 'script'; trusted-types lessdugin-md dompurify
```

A dev variant adds `http:` for the devnet.

### 5.3 Limits (`crates/core/src/limits.rs`, vectors in `testdata/vectors/limits`)

| What | Limit |
|---|---|
| Record JSON | 1 MB |
| `listRecords` page | 4 MB |
| Proof CAR | 1 MB |
| Indexer bundle | 8 MB |
| Blob, automatic load | 5 MB (images) |
| Blob, hard limit | 300 MB |
| Per repo per forum | 20k records, 50 MB |
| Per thread | 50 items per author shown; the rest collapsed |
| Mentions resolved per body | 16, lazily |
| Hover previews | depth 1 |
| Aspect ratio | clamped to between 1:20 and 20:1 |
| Custom react icons | P2, lazy |
| Wiki walk | 1,000 revisions |
| Newcomer budget | 500 |
| Quote matching | target text ≤ 200 KB, quote ≤ 2 KB |

---

## 6. Software architecture

### 6.1 Repository layout

```
lessdugin/
├─ Cargo.toml  rust-toolchain.toml(1.97, +wasm32-unknown-unknown)  .cargo/config.toml(xtask alias)
├─ clippy.toml (disallowed: SystemTime::now, Instant::now, thread::sleep, Utc::now in wasm-safe crates)  deny.toml
├─ package.json  pnpm-workspace.yaml  biome.json  tsconfig.base.json
├─ lexicons/online/7778777/lessdugin/{forum,membership,post,comment,revision,opinion,modAction,wikiRevision,
│        sequence,profile,defs,markdown,authFull,authInteract}.json  index/{defs,…}.json        (CC0)
├─ crates/{atp,lexicon,markdown,core,native,tui,cli,wasm,indexer,testkit}   xtask/
├─ packages/{engine,e2e}   web/
├─ testdata/vectors/{syntax,datamodel,keys,lexicon,lfm1,terminal,xss,mathml,limits,media,thread,modfold,karma,
│        rank,reacts,tags,time,fold,proofs}/*.json   testdata/interop/ (vendored atproto-interop-tests + LICENSE)
├─ docs/{spec/{data-model,lfm-1,social,sync,threat-model,limits,glossary,conformance-level0}.md, adr/, guide/, plan.md}
└─ deploy/{site/{_headers, oauth/*.json, lessdugin-config.json}, indexer/Dockerfile, compose.e2e.yml}  .github/workflows/
```

### 6.2 Crates and dependencies

Versions are pinned at M0 in `[workspace.dependencies]`. A `=` pin means an exact pin.

| Crate | wasm-safe | Responsibility | Key dependencies |
|---|---|---|---|
| `lessdugin-atp` (MIT/Apache) | yes | Syntax types (Did, Handle, Nsid, AtUri, Rkey, Tid, Datetime); JSON ⇄ DAG-CBOR; CID; CARv1 reader; MST proof walk; commit verify; did:key; DID doc; XRPC request/response structs (~16 endpoints) | `cid 0.11`, `serde_ipld_dagcbor 0.7`, `ipld-core 0.4`, `sha2 0.10`, `k256`/`p256` (`ecdsa` verify, latest stable, `=`), `multibase 0.9`, `data-encoding 2`, `serde`/`serde_json 1`, `thiserror 2` |
| `lessdugin-lexicon` (MIT/Apache) | yes | Hand-written owned serde structs with `#[serde(flatten)] extra` for unknown fields; a runtime data-driven validator that embeds the lexicon JSON (lengths, graphemes, formats, knownValues); key derivation and grammar; `NS` constant | atp, `unicode-segmentation 1`, `unicode-normalization 0.1`, `tsify 0.5` (optional) |
| `lessdugin-markdown` | yes | LFM/1 events → post-passes → HTML+MathML, `StyledDoc<TermText>`, P projection, ToC, excerpt, links, mentions, media refs, quote resolution; `term_sanitize`, `safe_href` | `pulldown-cmark =0.13.4` (no defaults), `math-core 0.9` (feature `math`), `unicode-width 0.2`, `unicode-security 0.1`, `ammonia 4.2.3` (feature `ammonia`, native only) |
| `lessdugin-core` | yes | `Host`/`KvStore`; store keys; identity (DoH over Host); discovery; sync; Jetstream parsing; outbox; verification orchestration; `social/` fold; `ForumModel`; search (in-memory inverted index); notifications; media (sniff, strip, CID); `limits`; view DTOs; `Engine<H>` | atp, lexicon, markdown, `futures 0.3`, `http 1`, `url 2`, `tracing 0.1`, `libm 0.2`, `img-parts 0.4`, `imagesize 0.15` |
| `lessdugin-native` | no | `NativeHost`: reqwest with an SSRF guard; `SqliteKv` (WAL, busy_timeout); WebSocket; system DNS (opt-in); keyring sessions; app-password auth; OAuth facade; config and paths; image decode, rotate and downscale | `tokio 1`, `reqwest 0.13` (rustls, http2, gzip, stream), `rusqlite 0.40` (bundled, fts5), `tokio-tungstenite 0.30` (rustls), `zstd 0.13`, `hickory-resolver 0.26`, `keyring 3`, `fd-lock 4`, `directories 6`, `image 0.25` (png, jpeg, gif, webp only), `jacquard-oauth =0.12.2` (feature `oauth`), `webbrowser 1` |
| `lessdugin-tui` | no | Elm-style app | `ratatui 0.30.2`, `crossterm 0.29`, `ratatui-image 11.1.0` (`default-features=false`, `image-defaults,crossterm`), a `tui-textarea` release compatible with ratatui 0.30 (checked in M0) |
| `lessdugin` (cli, bin) | no | Command tree, output, drafts, `tui`, `web` (axum + embedded SPA + bridge) | `clap 4.6` (derive, env), `clap_complete 4.6`, `serde-saphyr 2`, `axum 0.8`, `tower-http 0.6`, `rust-embed 8`, `indicatif 0.18` |
| `lessdugin-wasm` | target | Bindings and `WasmHost` | `wasm-bindgen =0.2.129`, `wasm-bindgen-futures 0.4`, `js-sys`/`web-sys 0.3`, `tsify 0.5`, `serde-wasm-bindgen 0.6`, `getrandom 0.4` (`wasm_js`), `console_error_panic_hook` (feature `debug-panic`) |
| `lessdugin-indexer` (bin) | no | Ingest, XRPC, SQL side tables | core, native, `axum 0.8`, `tower-http 0.6`, `tower_governor`, `metrics-exporter-prometheus` |
| `lessdugin-testkit` | both | `MemHost`, `FakePds` (XRPC subset with fault injection, proofs, cursor modes), fake Constellation, mini-Jetstream (devnet `subscribeRepos` → Jetstream JSON), Devnet launcher, vector runner, synthetic forum generator | `proptest 1`, `insta 1`, `assert_cmd 2` |

**Feature flags**
- markdown: `math` (default), `ammonia`
- core: `verify` (default)
- wasm: `verify`, `math`, `vectors` (tests only), `debug-panic`
- native: `oauth` (default)
- cli: `tui` and `web` (default)

**Release profiles**
- `release`: thin LTO, strip.
- `wasm`: `opt-level="z"`, fat LTO, `codegen-units=1`, then `wasm-opt -Oz`.

**CI rules**
- Clippy for wasm32 on atp, lexicon, markdown, core and wasm.
- An `assert_send` test for every engine future on native.
- No mutex guard held across `.await`.

### 6.3 Host abstraction

```rust
#[cfg(not(target_arch="wasm32"))] pub trait MaybeSend: Send {}   #[cfg(target_arch="wasm32")] pub trait MaybeSend {}
pub enum Target { Anon(url::Url), UserPds { account: Did, path_and_query: String } }   // host adds auth; never follows redirects
pub struct HttpRequest  { method: http::Method, target: Target, headers: Vec<(String,String)>, body: Option<Vec<u8>>, max_bytes: u64, timeout_ms: u32 }
pub struct HttpResponse { status: u16, headers: Vec<(String,String)>, body: Vec<u8> }   // body counted while streaming, after decompression
pub trait Host: MaybeSendSync + 'static {
    type Kv: KvStore;
    fn cache(&self) -> &Self::Kv;   fn state(&self) -> &Self::Kv;
    fn http(&self, r: HttpRequest) -> impl Future<Output=Result<HttpResponse,HostError>> + MaybeSend;
    fn websocket(&self, url: &str, max_msg: u32) -> impl Future<Output=Result<WsStream,HostError>> + MaybeSend;
    fn resolve_txt_system(&self, name: &str) -> impl Future<Output=Result<Option<Vec<String>>,HostError>> + MaybeSend; // None on wasm
    fn sleep(&self, d: Duration) -> impl Future<Output=()> + MaybeSend;
    fn now_ms(&self) -> i64;   fn fill_random(&self, b: &mut [u8]);
}
pub trait KvStore: MaybeSendSync {          // IndexedDB-compatible: no interactive transactions
    fn get_many(&self, k: Vec<Vec<u8>>) -> impl Future<Output=Result<Vec<Option<Vec<u8>>>>> + MaybeSend;
    fn scan(&self, r: KeyRange, d: Dir, limit: u32) -> impl Future<Output=Result<Vec<(Vec<u8>,Vec<u8>)>>> + MaybeSend;
    fn write(&self, batch: Vec<KvOp /*Put|Delete|DeleteRange*/>) -> impl Future<Output=Result<()>> + MaybeSend; // atomic
}
```

The core never spawns tasks. Long-lived futures (`live_tail`, `outbox_pump`, `background_verify`) are handed to the host, which runs them with `tokio::spawn` or `spawn_local`.

### 6.4 Store schema

Keys are tuple-encoded so they sort correctly as bytes: strings are escaped with 0x00/0xFF, integers are big-endian with the sign bit flipped.

| Store | Prefix | Key → value |
|---|---|---|
| cache | `rec` | `scope(fk \| "_"), coll, did, rkey` → `{cid, dagcbor, level, source, firstSeen, fetchedAt, asOfRev}`. A forum is one range scan. Scope is parsed from the rkey, so there is no secondary index. |
| cache | `repo` | `did` → `{rev, commit, pds, colls, listed{fk→rev}, checkedAt, nextCheck, class, fails, backoff}` |
| cache | `tomb`, `id`, `blob`, `cur`, `host` | tombstones; identity (doc and handle); blob bytes or path plus refcount; cursors; host capabilities (`rangeCursor`, CORS) |
| state | `out`, `loc.*`, `notif`, `meta` | outbox; drafts, read marks, subscriptions, bookmarks, mutes, settings, aliases, `last_listing:<tty>`; notifications; schema version |

**ForumModel** is an in-memory per-forum model built from one `rec/{fk}/` scan plus global records. It keeps skeletons (references, opinion values, titles, times) and drops bodies, which are re-read with `get_many` when needed. It is rebuilt or updated incrementally on `Changed` events, and incremental results are property-tested equal to the batch fold. Render output goes to an in-memory LRU keyed by `(cid, RENDERER_VERSION)`.

### 6.5 Engine API (shared by CLI, TUI and wasm; DTOs are `serde` + `tsify`)

| Group | Calls |
|---|---|
| Identity and session | `resolve_actor`, `accounts`, `set_active_account`, `session_status` |
| Forums | `forum_get`, `forum_discover`, `forum_follow`, `forum_unfollow`, `forum_create`, `forum_update`, `join`, `leave`, `members`, `grant`, `mods_set` |
| Sync | `sync_forum(scope, SyncOpts{budget}) -> SyncReport`, `sync_thread`, `live_tail()`, `outbox_pump()`, `background_verify()` |
| Queries | `list_posts(ListPostsQuery{forum, sort, filter, tag, kind, cursor})`, `recent_discussion`, `get_post`, `get_thread(uri, ThreadOpts{sort, unread_only}) -> ThreadView` (pre-order, with depth), `history(uri)`, `tags`, `tag`, `sequences`, `sequence`, `user`, `search`, `notifications`, `mod_queue`, `mod_log`, `raw(uri)` |
| Writes | `publish(Draft)`, `edit(uri, Draft, EditOpts{history})`, `delete`, `retract`, `comment`, `opinion(subject, OpinionIntent)` (axis, react or tag delta), `mod_action`, `mod_adopt`, `wiki_edit`, `seq_*`, `redact_revision`, `prepare_media(bytes, mime) -> MediaRef`, `undo(write_id)` |
| Local | `mark_read`, `save_draft`, `bookmark`, `subscribe`, `mute`, `settings`, `export_local`, `import_local` |
| Pure | `render(md, media, RenderOpts)`, `parse_ref`, `verify(uri, deep) -> VerifyReport`, `fold_vectors` (tests) |

**Events:** `Changed{topics}`, `SyncProgress{scope, done, total}`, `Write{id, state}`, `Auth{account, state}`, `Warning{kind, source}`.

`--json` output from the CLI and the SPA's types share one versioned schema, so parity checks are byte comparisons.

### 6.6 Wasm boundary and `@lessdugin/engine`

**Wasm side**
- `WasmEngine` mirrors §6.5 one-to-one. `JsHost` is imported with the methods `http`, `kvGetMany`, `kvScan`, `kvWrite` (per store), `openWs`, `now`, `sleep` and `random`.
- `subscribe(cb)` forwards events. Free functions: `renderMarkdown`, `parseRef`, `validateRecord`, `deriveRkey`, `runVector`.
- Built with `wasm-bindgen --target web`. `xtask` fails if the CLI version differs from `Cargo.lock`.
- **Size budget:** target 900 KB gzipped; CI fails above 1.1 MB. `xtask wasm --report` (twiggy) attributes size. If verification pushes the build over budget, it moves into a lazily loaded `verify.wasm`.

**`packages/engine`**
- `worker.ts`: loads wasm and builds JsHost (`idb 8` stores `cache` and `state`, `fetch` with streaming byte caps, WebSocket, DoH). Exposed with `Comlink.expose`.
- `client.ts`: `startEngine({config, auth}) → EngineClient`, plus a supervisor that quarantines records and restarts the worker after a crash.
- `auth-bridge.ts`: on the main thread, enforces the allowlist and holds the OAuth client (hosted mode) or the local-bridge client (`web` mode).
- `leader.ts`: Web Locks leader election plus BroadcastChannel.
- **Login:** the core resolves the handle with DoH, then the main thread calls `signIn(did)`, so the OAuth library's own handle resolver is unused. The library is `@atproto/oauth-client-browser 0.5.9` (IndexedDB plus WebCrypto; S2 confirms non-extractable keys). The fallback is `@atcute/oauth-browser-client 5.1.0` with a custom store that keeps keys non-extractable.

### 6.7 Authentication

**Native**, behind the facade `AuthSession { async fn send(HttpRequest) }`:
1. **OAuth loopback (default):** client_id `http://localhost?redirect_uri=http://127.0.0.1:<port>/callback&scope=…` via `jacquard-oauth` `=0.12.2`.
2. **Hosted native metadata:** used automatically after `invalid_client`, or with `--oauth-client hosted`.
3. **`--no-browser`:** prints the URL; the user pastes back the failed `127.0.0.1` redirect URL (PKCE and state are checked). Works over SSH.
4. **`--app-password`:** for bots and headless use.

**Common rules**
- Scope fallback is decided at PAR time.
- Refresh runs under an exclusive `fd-lock`, because refresh tokens are single-use and the CLI, TUI and `web` may run at once.
- Expiry comes from `expires_in`.
- Sessions are capped at about 14 days. Users are warned 3 days ahead and can re-login inline in the TUI.
- **Fallback if S1 fails:** our own PAR, PKCE and DPoP client, about 1.5k lines over atp's XRPC.

### 6.8 Rendering pipeline

```
source → pulldown-cmark events → LFM post-passes (§2.7) → URL policy and sanitising → three outputs:
  HTML+MathML (+ammonia equality check natively),   StyledDoc<TermText>,   P projection
```

`Rendered{html, styled, text, toc, word_count, read_minutes, links, mentions, media_used, excerpt, anchors}`. `RENDERER_VERSION` is part of the cache key.

**Font.** The SPA ships `mathml.css` plus a STIX Two Math subset (OFL, about 200 KB), which browsers load only when a page contains math.

### 6.9 Media pipeline (in core, so the CLI and SPA behave identically)

1. **Sniff and check:** sniff the type, check it against the allowlist (jpeg, png, webp, gif, mp4, webm, mp3, ogg, m4a, pdf), and read dimensions.
2. **Strip:** rebuild from allowlisted segments only (§5.2).
3. **Orient:** if EXIF orientation ≠ 1, physically rotate. Natively that is `image` re-encode; in the SPA, `createImageBitmap({imageOrientation:'from-image'})` → canvas.
4. **Downscale** to the PDS limit: try 5 MiB for self-hosted PDSes first, and fit the image when the PDS returns 413. Native re-encodes JPEG or PNG; the SPA uses canvas WebP or JPEG.
5. **Compute** the raw CID for preview.
6. **Upload** at publish time; embed the returned blob reference verbatim.
7. **Alt text:** a warning when it is missing.

Video and audio get only the box neutralisation from §5.2. Thumbnails are deferred to P2.

### 6.10 CLI (`lessdugin`)

**Global flags:** `--account`, `--forum`, `--json`, `--offline`, `--sync`, `--sources direct|assisted`, `--verify pds|proof`, `--indexer <url|none|forum>`, `--time-budget`, `--max-requests`, `--data-dir`, `--color auto|always|never`, `--dev`, `-q`/`-v`.

**Bare invocation:** `lessdugin` on a TTY with an account opens the TUI; otherwise it prints help. Reading never needs a login.

```
login [handle|did] [--app-password] [--no-browser] [--oauth-client loopback|hosted]   logout [--purge]   whoami   accounts [use <a>]
resolve <handle|did>    doctor [--pds <url>]    verify <ref> [--deep]    verify-site <url>
forum discover [q] | ls | follow <ref> [--alias a] | unfollow | use <ref> | show | create <slug> [--mode open|approval] | edit
join <ref> [--note]   leave [forum]          # join = follow + membership + set current + alias
members [--pending] | approve|reject <actor>... | mods add|rm <actor> [--role admin|moderator]
ls [forum] [--sort magic|top|new|old|recent-comments] [--frontpage|--all|--curated] [--tag t] [--kind k] [--limit n]
activity [forum]                                  # Recent Discussion
read <ref> [--sort] [--unread] [--no-comments] [--raw|--html] [--no-pager] [--history]      # comments labelled c1…cN
post [file.md] [--forum] [--title] [--tag t]... [--link url] [--kind] [--draft] [--yes]
comment <ref> [file|-] [-m text] [--quote "…"]    edit <ref> [--summary s] [--no-history]    delete <ref>   retract <ref>
history <ref> [--redact <n>|--redact-all]
vote <ref> up|down|strong-up|strong-down|clear      agree <ref> agree|disagree|strong-agree|strong-disagree|clear
react <ref> <react> [--quote "…"] [--against] [--remove]    reacts                         # palette
tag <post> add|rm|up|down|strong-up|strong-down <tag>    tags [forum] | tags show <tag> | tags edit <tag>
seq ls | show <ref> | create | edit <ref> | add <seq> <post> [--chapter n] [--at i] | rm | mv
bookmark add|rm|ls     subscribe|unsubscribe <ref|forum|tag|user|seq>     subs
notifications [--all] [--mark-read]     search <q> [--forum] [--author] [--remote]     user <actor>   profile edit
mod hide|unhide|keep|ban|unban|approve|restrict|unrestrict|nullify|personal|frontpage|curate|pin|unpin|lock|unlock|
    tag-remove|tag-head|tag-alias <ref|actor|tag> [--reason] [--until] [--spam] [--author-scope] [--scope forum|thread]
mod queue | log | adopt <former-mod>
draft ls | edit <id> | rm <id> | publish <id>        media add <file> | get <ref> --index N [-o f] | open <ref> --index N
sync [forum] [--full] [--follow]        export [--mine|forum] [--format json|md] [--out dir]
tui [forum]    web [--port] [--no-open]    config get|set|edit    completions <shell>
```

**Refs:** `3` (from the last listing in this terminal), `c12` (comment label from the last `read`), an at-uri, any SPA URL, or `<handle>/<rkey>`. `#` is never used in refs, because shells treat it as a comment.

**Drafts**
- Stored as `$DATA/drafts/<id>.md` with YAML frontmatter (forum, title, kind, url, tags, sequence).
- The editor chain is `$VISUAL` → `$EDITOR` → `editor` → `nano` → `vi` (notepad on Windows).
- Validation errors appear as single-line YAML comments.
- Publishing shows a summary (forum, tags, local files, media, word count) and asks `[y/N/e]`.

**Output**
- Human output wraps to the terminal and pages through `less -R`. Everything is `TermText`.
- `--color never` gives plain, accessible output with no box drawing.
- Exit codes: 0 ok, 1 error, 2 usage, 3 auth required, 4 not found, 5 conflict, 6 partial.

**Paths:** config `~/.config/lessdugin/config.toml`; data `~/.local/share/lessdugin/{state.sqlite, drafts/}`; cache `~/.cache/lessdugin/{cache.sqlite, blobs/}`.

### 6.11 TUI (`lessdugin-tui`)

**Architecture**
- `State` plus a stack of screens. `update(State, Action) → Vec<Effect>`; `view` is pure.
- Engine events become Actions. A live update never moves the cursor: it shows a "3 new — R" banner.
- `$EDITOR` runs with the terminal suspended. An inline composer (tui-textarea) handles short replies; `Ctrl-e` escalates to `$EDITOR`.
- Images use the ratatui-image protocol picked by `Picker::from_query_stdio()`. tmux passthrough is detected; otherwise halfblocks are used.

**Screens:** Home (forums with unread counts, notification badge, sync status), Forum (sort and filter tabs; Posts and Activity), Post, Thread, Tags/Tag, Sequences/Sequence, User, Notifications, Search, Mod queue (one key per action, plus reason), Mod log, Drafts. Overlays: `?` help, `:` command line (accepts any CLI subcommand), react palette, confirmations, login.

**Keys** (remappable in `[tui.keys]`):

| Group | Keys |
|---|---|
| Navigation | `j/k` `↑/↓`, `g/G`, `Ctrl-d/u`, `Enter`/`l` open, `q`/`Esc`/`h` back, `Tab` |
| Threads | `Space` collapse, `p` parent, `]`/`[` next/prev unread, `}`/`{` next/prev sibling |
| Opinions | `u`/`U` up/strong, `d`/`D` down/strong, `a`/`A` agree/strong, `z`/`Z` disagree/strong, `0` clear, `x` react palette; `Backspace` undoes the pending write |
| Writing | `c` comment on post, `r` reply, `P` new post, `e` edit own, `Del` delete own, `H` history |
| Lists | `s` sort, `f` filter, `t` tags, `b` bookmark, `S` subscribe |
| Other | `m` mod menu, `o` open in `lessdugin web`, `y` yank uri, `R` sync, `/` search, `n`/`N` next/prev match, `?` |

### 6.12 SPA (`web/`)

**Stack**
- React 19, Vite 8, TypeScript 7 (tsgo, with a `tsc` fallback job in CI), Biome 2.5.
- `@tanstack/react-router` 1 (hash history, typed search params), `@tanstack/react-query` 5 (engine `Changed` topics trigger `invalidateQueries`), `@tanstack/react-virtual` 3.
- CodeMirror 6 with `@codemirror/lang-markdown`, a toolbar (bold, link, image, math, spoiler) and a live preview from the worker (150 ms debounce).
- `radix-ui`, CSS modules (light and dark), DOMPurify 3.4, Comlink 4.4.

**Routes**
- `#/` — home
- `#/f/:owner/:slug` — forum, with `?sort&filter&tag`
- `#/f/:owner/:slug/{activity, new, tags, t/:tag, seqs, mod, members, settings}`
- `#/p/:author/:rkey` — post
- `#/c/:author/:rkey` — comment permalink
- `#/s/:author/:rkey` — sequence
- `#/u/:actor` — user
- `#/at/*` — at-uri resolver
- `#/notifications`, `#/drafts`, `#/bookmarks`, `#/search?q=`, `#/settings`, `#/login`

**Behaviour**
- Rendered HTML is inserted through Trusted Types. One delegated handler covers `data-at-uri` hover previews, `data-cid` media (verify, then object URL), `data-anchor` scrolling, placeholders, and footnotes as sidenotes on wide screens.
- Mention autocomplete draws on the participant set.
- Paste or drop of images goes through the media pipeline.
- Mobile layout: touch voting, compact thread indentation, bottom-sheet react palette.
- Verification and pending-write badges.
- Settings: accounts, sources, verification, indexer consent, external images, media autoload hosts, export/import of local state, storage persistence, build hash.
- On startup the SPA calls `navigator.storage.persist()`, compiles the wasm with a streaming compile, and paints the placeholder UI first.

### 6.13 Hosting modes

| Mode | Behaviour |
|---|---|
| **Static site** `https://lessdugin.7778777.online/` (GitHub Pages; `_headers` for Cloudflare/Netlify) | Web OAuth client with tokens in IndexedDB. Ships `lessdugin-config.json {oauth: "hosted"}`, the source and licence link (AGPL §13), and source maps. Trusted while in use; reproducible hash published per release. Self-hosting: `xtask site --origin <url>`. |
| **`lessdugin web`** (the zero-trust path) | axum on `127.0.0.1:<port>` only (default 47778, persisted; fails if busy). Rejects any Host header other than `127.0.0.1:<port>` or `[::1]:<port>`. Serves the embedded SPA with CSP headers and `lessdugin-config.json {oauth: "bridge"}`. A 256-bit per-launch capability travels in the URL fragment, is exchanged once (`POST /bridge/session`), and lives in sessionStorage. |

**Bridge endpoints** (POST only, bearer token, Origin checked):
- `/bridge/xrpc`: the allowlisted write methods, sent through the CLI's keyring session.
- `/bridge/login`: native loopback OAuth, using the same server as the redirect target.
- `/bridge/kv`: the `state` store kept in native `state.sqlite`, so drafts are not exposed to port-squatting.
- `/bridge/fetch`: an anonymous read proxy for PDSes without CORS, limited to allowlisted `com.atproto.{repo,sync}` GETs and behind the SSRF guard.

The browser holds no long-lived secrets. `build.rs` embeds `web/dist`, or a placeholder page that explains how to build it. Release binaries always embed the real SPA.

### 6.14 Indexer (`lessdugin-indexer`)

**Ingest**
- Jetstream wildcard `NS.*` from two endpoints, `time_us` cursor with a 10 s rewind.
- Backfill: every 6 h, `listReposByCollection` for each NSID across relays; then each repo's range listing (`pds` level).
- Live events: `sync.getRecord` proof per event (one request), stored so proofs can be served. Proofs for backfilled records are fetched lazily on the first `getRecordProofs` request and cached per rev.
- Daily `getLatestCommit` reconcile. Identity and account events handled as in §3.1.
- Polite to PDSes: ≤ 5 requests/s and 4 in flight per host, 200 in flight globally.
- `--poll <pds>...` mode for the devnet and air-gapped setups.
- Indexes only forum declarations and forum participants (quotas per DID and per host).

**Storage**
- `SqliteKv` with the client schema, plus SQL tables: `change_log` (30 days), `post_stats`, `karma`, `backlink`, `proof_cache`, `fts5`.
- Comfortable up to about 50M records. A Postgres adapter can come later.

**Serving**
- The full method list of §2.9. Responses carry `asOf` and ETags; lists are `max-age=5`; nothing is `immutable`.
- 20 requests/s per IP; per-actor limits on `getActivity`.
- `/metrics`, `/_health`, and `/` (status page with the source link).

**Operations:** the takedown blocklist is applied at query time. Ships as a distroless image on GHCR.

**Clients**
- The indexer serves as a discovery, ordering and proof source.
- Displayed values are recomputed locally.
- `--audit` (and a 1%-per-hour automatic check) compares indexer heads against PDS `getLatestCommit`, compares Jetstream events against `getChanges` (an item missing after 2 minutes counts against the indexer), and compares `inputDigest` and aggregates against local recomputation.
- An indexer above the thresholds (> 2% contradictions, or > 3 omissions per day) is demoted with a warning.

### 6.15 Licensing

- **Applications** (core, native, cli, tui, wasm, indexer, web, packages): AGPL-3.0-or-later, with SPDX headers.
- **Lexicons, `docs/spec` and vectors:** CC0-1.0.
- **`atp` and `lexicon` crates:** MIT OR Apache-2.0 (owner to confirm). Contributions under DCO.
- **`deny.toml` allowlist:** MIT, Apache-2.0, BSD-2/3, ISC, Zlib, 0BSD, Unicode-3.0, MPL-2.0 (jacquard, DOMPurify), CC0-1.0, CDLA-Permissive-2.0, OFL-1.1 (font).

---

## 7. Testing and CI

| Layer | How |
|---|---|
| Protocol | Vendored atproto-interop-tests: syntax (including `online.7778777.lessdugin.*` cases), data model JSON ⇄ CBOR ⇄ CID, MST, signatures with high-S rejected, did:key. Tampered-proof fixtures. `cargo-fuzz` on CAR, CBOR, MST, validator and renderer (nightly). jacquard-repo and `@atproto/repo` used as test oracles. |
| Shared vectors | `testdata/vectors/<kind>/*.json` as `{name, input, expected}`. A Rust runner (testkit) and a TS runner (vitest) call `runVector` in a `vectors`-feature wasm build, so native and wasm results are proven equal. `xtask vectors --bless` regenerates expected output, and the diff is reviewed. |
| Social | Vectors: power boundaries; keys; karma (self-vote, seed establishment with 1 vs 2 seeds, sybil ring, pairCap boundary, deleted subject with 1 vs 2 voters, nullify); modfold (rank, same-rank safety-min, lapse and adopt, expiry, CID-bound pin after an edit, keep vs ban, immunity, author tier at gate 49/50); reacts (gate boundaries, contradiction, anchors with 0, 1 or 2 occurrences); time (future-dated, comment before parent); rank (exact f64 bit patterns); threads (2- and 3-cycles, missing and cross-root parents); wiki (fork, merge, pin, removed head, altered ancestor). **Property tests:** permutation invariance, duplicate idempotence, layering (L5–L7 records never change E, K1, KP or K), sybil theorem, pair bound, incremental = batch, native == wasm on random inputs. |
| Markdown and terminal | ≥ 150 LFM/1 vectors; XSS and mXSS corpora (HTML and MathML); escape corpus asserting that no ESC appears except our own SGR and OSC 8; proptest that ammonia and DOMPurify leave renderer output unchanged; StyledDoc snapshots. |
| Sync and engine | `MemHost` + `FakePds` scenarios: idempotent sync, cursor-range fallback, rollback, deletes only after confirmation, outbox survives `kill -9`, `InvalidSwap` retry vs real conflict, opinion merge across two devices, Jetstream hint → fetch, quarantine, fail closed with a mod PDS down. A 5,000-repo load simulation asserts the request budgets in §3.10. A synthetic 1k-post / 50k-opinion forum must `ls` in under 300 ms warm. |
| CLI and TUI | `assert_cmd` + `insta` against FakePds. TUI key sequences produce `TestBackend` snapshots at 80×24 and 160×48, plus a resize fuzz test. |
| End-to-end | `packages/e2e/devnet.mjs` starts `@atproto/dev-env 0.6.20` `TestNetworkNoAppView`, plus the testkit mini-Jetstream and fake Constellation (or the indexer in poll mode). Rust e2e tests run when `LESSDUGIN_E2E=1`. Playwright 1.64 (Chromium from `/opt/pw-browsers`) does real OAuth against the devnet; if http issuers are rejected, an e2e-only app-password build is used. Any `securitypolicyviolation` fails the test; axe runs. |
| Cross-client parity | Scenario: alice creates a forum (CLI); bob joins and posts with an image (SPA); carol replies, votes, reacts and tags (TUI); a mod hides and pins (CLI); edits and revisions. With `now` and `obs` injected, CLI `--json`, SPA `getThread`, indexer-hydrated output and the TUI snapshot must match byte for byte. Variants: approval mode, indexer on/off, direct/assisted. |
| Production smoke | Manual or nightly runs on throwaway bsky.social accounts using `.temp` NSIDs, and on one self-hosted PDS. |

**Workflows**

| Workflow | Contents |
|---|---|
| `ci.yml` (PRs and pushes) | **rust:** fmt; clippy native plus wasm32; nextest; doctests; lexicon conformance test (every lexicon property mapped to a struct field); `cargo deny`; `cargo vet`. **wasm:** wasm-bindgen-cli `=0.2.129` and binaryen (SHA-pinned); `xtask wasm --release --budget`. **web:** `pnpm i --frozen-lockfile`, biome, tsgo and tsc, vitest, `vite build`, lockfile-lint, osv-scanner. **lexicons:** pinned goat `lex lint` and `lex breaking` against the merge base; `@atproto/lexicon` parse. |
| `e2e.yml` (PRs and nightly) | devnet, Rust e2e, Playwright; traces uploaded on failure |
| `release.yml` (tags) | Linux x86_64/aarch64 musl, macOS arm64/x86_64 and Windows binaries with the embedded SPA; `SHA256SUMS`; build provenance attestation; `cargo auditable`; indexer image on GHCR; SPA dist tarball and hash |
| `pages.yml` | tagged dist plus site files, deployed through a protected environment |

**xtask commands:** `wasm [--report|--budget]`, `web`, `dist`, `site --origin`, `devnet`, `e2e`, `vectors [--bless]`, `lex lint|breaking|publish|check-dns`, `ci`.

---

## 8. Phased plan

Milestones are vertical slices. M3 freezes engine API v0. After that, M4, M5 and M6 run in parallel.

| Milestone | Scope | Acceptance criteria |
|---|---|---|
| **M0: Spec and spikes** | This document split into `docs/spec` (data model, LFM/1, social, sync, threat model, limits, glossary) plus a decision log. Draft lexicons under `.temp` pass `goat lex lint`. Key vectors generated. Workspace, toolchain, CI skeleton, devnet script. **Spikes (each written up as an ADR):** **S1** jacquard-oauth loopback, hosted-native and `--no-browser` against bsky.social, and signed XRPC through the facade. **S2** the browser OAuth library: non-extractable IndexedDB keys, `signIn(did)`, query and fragment callbacks, OAuth against an http devnet PDS. **S3/S10** wasm size with and without math-core and k256/p256, and Temml fallback cost. **S4** the digit NSID through goat, `@atproto/lexicon`, the Jetstream wildcard, relay `listReposByCollection` and Constellation `source=`; `validate`-unset writes containing a union of record types plus nested blob refs, and blobs referenced only by a revision surviving GC. **S5** devnet in GitHub Actions. **S6** Jetstream wildcard plus rkey-prefix filtering. **S7** `listRecords` cursor-range behaviour on bsky.social and on non-reference PDSes. **S8** detecting `include:` failure at PAR. **S9** PDS read limits per host vs shared (cold sync of 200 real repos), and PLC throughput at 20 requests/s. **S11** Safari/iOS IndexedDB eviction and session persistence. **S12** `createRecord`-with-rkey create-if-absent, and `putRecord swapRecord` semantics. | Every spike has an ADR. `cargo xtask ci` is green. The devnet boots 3 accounts in under 30 s. Spec reviewed and frozen for `.temp`. |
| **M1: Protocol foundations** | atp (complete); lexicon types and validator; key derivation; identity (DoH, PLC, did:web, Slingshot hint); XRPC subset; NativeHost (reqwest + SSRF guard, SqliteKv, WebSocket, keyring); auth (app password, OAuth loopback, hosted, no-browser, refresh lock); `TermText`; CLI `login`, `logout`, `whoami`, `accounts`, `resolve`, `verify`, `doctor`. | Interop vectors pass. `lessdugin verify at://<real bsky post>` reports proven with the key source, and tampered fixtures fail. Login works on bsky.social and the devnet with **one** consent screen. Parallel CLI processes never break a refresh. |
| **M2: Walking skeleton** (text, all three clients) | KvStore for SQLite, IndexedDB and memory; L1 forum-range sync with fallback; outbox (`applyWrites` + `swapCommit` retry, membership beacon); forum create, join, post, comment, edit with revision, delete, retract; threads; basic LFM (no math or media); CLI `read` (anonymous), `ls`, `post`, `comment`, `edit`, `delete`, `sync`, `join`, `forum *`; TUI read and reply via `$EDITOR`; wasm worker, JsHost, AuthBridge, leader election; SPA anonymous read, login and comment; `lessdugin web` with bridge auth. | Devnet parity scenario (text subset) is byte-identical across CLI, SPA and TUI. A production smoke test on `.temp` succeeds. A 50-member FakePds forum cold-syncs natively in under 5 s. Wasm size reported in CI. No CSP or Trusted Types violations. |
| **M3: Social core** | Opinion records with undo window and merge; `karma/1`; scores and agreement; `magic/1` sorts; Recent Discussion; `mod/1` fold including author tier, CID-bound positives, ban, lock, fail closed; approval and open modes, closure, newcomer collapse; mod queue, log and adopt; grants UI; notifications and unread; votes-public interstitial; "not counted yet" UI; reacts with gates, against-reacts and anchors listed. Engine API v0 frozen. | All social vectors and property tests pass. With the mod's PDS offline, no hidden spam reappears. Every MVP action is reachable in all three clients (`:` command line in the TUI). Parity diffs are clean. |
| **M4: Rich content and media** | Full LFM/1 (math, footnotes and sidenotes, ToC, alerts, spoilers, details, captions, hover previews, verified mentions); media pipeline (strip, orient, downscale, CID, verbatim refs); display and autoload policy; attachments; history view and redaction; SPA editor (toolbar, paste/drop, preview, mention autocomplete); TUI images; `export`. | ≥ 150 LFM vectors, plus the XSS, mXSS and escape corpora. DOMPurify and ammonia leave output unchanged. A 10-minute fuzz run without panics. EXIF, GPS and MP4 `udta` removed with orientation preserved. Auto-downscale stays under 5 MiB. |
| **M5: Organisation, discovery, production** | Tags (relevance, wiki DAG, mod tag actions); sequences (library, next/prev, continue reading); local search; forum directory; live Jetstream; open-mode discovery (Constellation, Jetstream, relay); `include:` scope with PAR-time fallback; hosted SPA (client metadata, CSP, Trusted Types, Pages); mobile layout; `--verify proof` background checks. | Open-mode devnet scenario passes with mini-Jetstream and fake Constellation. Login on bsky.social works through CLI loopback, CLI hosted-native, the hosted SPA and `lessdugin web`, each with one consent screen. A cached forum renders in under 1.5 s. Axe and Lighthouse accessibility ≥ 90. |
| **M6: Optional indexer** | Ingest, backfill and poll mode; every §2.9 method including `getRecordProofs`; client integration (assisted mode, cold start, proof fetches on Jetstream events, audit, demotion); Docker image; operator docs. | Devnet: consistent within 5 s of a write; restart resumes with no gaps. Clients produce identical views with and without the indexer. The SPA cold-paints a 2k-participant synthetic forum in under 2 s with the indexer. `--audit` flags an injected omission and a forged proof. `listPosts` p95 under 50 ms at 200 requests/s on 1M records. |
| **M7: Release 0.1** | Security review pass (threat model, fuzzing, corpora); switch `NS` from `.temp` to the release prefix; publish lexicons and permission sets; DNS TXT records; publisher account hardening; release pipeline; user guide, spec, indexer self-hosting guide, Level-0 conformance doc (read posts, comments and mod hides). | `goat lex check-dns` passes. `include:` works on bsky.social. On a fresh machine: install from a release, log in, post, and the post appears on the hosted SPA and in `lessdugin web` with matching hashes. |

**Deferred (P2 and later, in rough order)**
- **P2:** Q&A answers and accepted answers; quick-take feed; co-authors (+`karma/2`); side-comment and inline-react margin UI; author moderation styles and guidelines; rate limits (`ratelimit/1`); reports (sealed or service); bulk mod actions (`#banList`, `#raid`, `#banHost`) and `#access.keep`; public bookmarks; standard.site syndication; thumbnails; labelers; "what's public about me" view; `sensitive` forum flag; fork-evidence actions; `topAdjusted`; personalised magic and tag-filter weights.
- **P7:** verified MST range sync (`sync.getRecord` + `getBlocks`) as the default; sync windows; indexer range proofs (`getRepoRanges`) and set digests; `karmaSnapshot` checkpoints; PLC audit verification (`strict` mode); media mirrors; Service Worker media streaming; full native engine bridge.
- **P8+:** dialogues, events and RSVPs, polls, books, cross-device encrypted state, push notifications, private-ballot tally extension, an RSS importer and exporter, importers from other forum lexicons.
- **Never:** LLM features, real-time collaborative editing, the jargon glossary, the annual review, and private messages (we link to Bluesky chat).

---

## 9. Open risks

| Risk | Mitigation |
|---|---|
| PDS read limits are shared across Bluesky hosts, so cold sync takes minutes (S9) | Progressive paint, time budget, persistent cache, few collections, fk ranges, indexer thresholds, honest cost table |
| Cursor range listing unsupported on non-reference PDSes (S7) | Per-host probe and fallback to a full collection listing; P7 range sync |
| `include:` resolution fails, blocking login (S8, M14) | Fallback chosen at PAR; enumerated scopes on the devnet and `.temp` |
| jacquard is alpha (0.13 is breaking) | Exact pin, facade, own OAuth client as fallback (S1) |
| Browser OAuth key storage (S2) | Library choice gated on non-extractable keys; strict CSP, Trusted Types, two sanitiser layers |
| Wasm size and math-core coverage (S3, S10) | Feature gating; lazy `verify.wasm`; Temml fallback |
| Constellation or Jetstream disappear | Approval mode and closure need neither; indexer; relay fallback |
| Lexicon lock-in before learning | `.temp` dogfooding; final limits reviewed in M0 and again at M7 |
| Seeded karma slows bootstrap in new open forums | Configurable gates and `establishSeeds`; authorities bypass gates; grants |
| Public votes change the social dynamics compared with ForumMagnum | Clear interstitials; private-ballot tally extension on the roadmap |
| CID-bound curation and pins create friction on edits | Warning before editing; one-key re-assert in the mod queue |
| A ban hides past content (differs from ForumMagnum) | Item-level keep now; `keep` snapshots in P2 |
| Abuse and legal exposure for indexer operators (CSAM, takedowns) | No blob storage, blocklist, documented contact, honour takedowns from upstream |
| Safari storage eviction (S11) | PWA install, `persist()`, draft download, re-login prompt |
| PLC throughput for cold starts | Pacing, caching, Slingshot hints (badged), indexer |
| Single-maintainer domain or publisher compromise | Hardening, offline rotation key, reproducible builds, `lessdugin web` |

---

## 10. Resolved critiques

**Security critique**

| ID | Decision |
|---|---|
| F1 | **Accept.** `TermText` everywhere, OSC 8 policy, JSON-on-TTY escaping, escape corpus. |
| F2 | **Accept.** |
| F3, F4 | **Accept.** |
| F5 | **Accept:** `img-src 'self' blob:`, alignment classes, no inline SVG, `data-anchor`, ordinal footnote ids, `dir=auto`. |
| F6 | **Accept.** |
| F7 | **Accept** (`safe_href` + `<SafeLink>`). |
| F8 | **Accept** (iterative walkers, pre-parse limits, quarantine). |
| F9 | **Accept**, with one change: the moderator role is *displayed* only if that moderator has a membership record (no separate acceptance record). `movedTo` is dropped from v1. |
| F10 | **Accept:** DoH by default on native too, TOFU auth server, DID login. |
| F11 | **Accept.** |
| F13 | **Accept** with no exception in the MVP. Indexer content becomes displayable only through proofs. |
| F14 | **Accept** (`limits.rs`). |
| F15 | **Partial.** Record and compare commits; a warning in the MVP; evidence actions in P2. |
| F16, F17 | **Accept.** |
| F18 | **Accept:** seeded `karma/1`, `establishSeeds=2`. Seeds include *granted* members, which are explicit human grants. |
| F19 | **Accept:** no automatic sweep; explicit grants only. The per-moderator grant cap is **rejected** for v1: removing a moderator lapses every grant they made. |
| F20 | **Accept** (CID-bound positive assertions). |
| F21 | **Accept:** a ban hides everything; locks carry allow-lists; rate limits are advisory and deferred. |
| F22 | **Partial.** Fail closed accepted. Bulk actions deferred, with the rkey form reserved. `modMirrors` deferred. |
| F23 | **Accept** (one opinion, ≤ 32 reacts, dedupe). |
| F24 | **Accept:** canonical uris; deleted-subject rule with ≥ 2 E voters; wiki walk cap; JS `Map`. Window disagreement is moot because the MVP has no windows. |
| F25–F28 | **Accept** (blob scopes enumerated, so no `*/*`). |
| F29–F31 | **Accept.** |
| F32 | **Accept** for the P2 design; `showReason` rename accepted now. |
| F33 | **Accept** (box neutralisation instead of rewriting). |
| F34 | **Accept** (interstitials in the MVP; the rest in P2). |
| F35, F36 | **Accept.** |

**Protocol critique**

| ID | Decision |
|---|---|
| H1 | **Accept:** `swapCommit` with CID-aware retry; no upsert, so create-if-absent uses `createRecord`. |
| H2 | **Accept the intent:** blob scopes are enumerated to cover every type we upload, and every variant begins with `atproto`. |
| H3 | **Accept:** ancestry by uri; tag discussion anchors to (forum, slug) in P2; revision history is "verifiable, not complete". |
| H4 | **Accept:** every field has a final limit (§2.5). |
| H5 | **Accept:** Jetstream as the dirty signal; `getRepo?since` dropped except for export. |
| H6 | **Accept:** runtime probe plus fallback. |
| H7 | **Accept:** comment key `{fk}-{rootTid}-{tid}`. |
| M1–M3 | **Accept:** separate CAR method; array limits 25 and 50; no `forum.checkpoint`. |
| M4 | **Accept:** mangle the module name `_7778777`; tooling covered by S4. |
| M5–M14 | **Accept.** |
| Low items | **Accept all** (indexer on did:web, PLC pacing, optional commit output, `getBlocks` batches ≤ 50, dev CSP, checking granted scopes before interop writes). |
| X1–X18 | **Accept the stated resolutions**, with these refinements: X2, the hash domain uses the short collection name and fk is 60 bits. X4, per-member `#access` grants plus mutual consent. X6, the MVP uses `pds` level plus proofs for indexer content and on demand, not mandatory proofs for moderation inputs (TLS from the author's PDS gives the same trust when the PDS holds the key). X8, future-dated items are badged, not hidden. X18, author moderation uses the same `modAction` collection with `scope=author`. |

**Product critique**

| ID | Decision |
|---|---|
| C1 | **Accept** (`opinion`). |
| C2 | **Partial.** One domain-separated hash function plus the fk prefix. The `stid` segment is **kept**, contrary to the product critique, because rkeys are permanent and it enables P7 windowing at no MVP cost. |
| C3–C5 | **Accept.** |
| C6 | **Accept** (per-action records with rank and safety-min). |
| C7–C10 | **Accept.** |
| C11 | **Accept:** `lfm/1`, `md-` id prefix, math engine decided by S10. |
| C12 | **Accept.** |
| C13 | **Accept**, adding `getRecordProofs` to the MVP. |
| C14 | **Accept** (enumerated blob scopes). |
| C15 | **Accept** (§8). |
| C16–C18 | **Accept.** |
| §2 cuts | **Accept**, except that these stay: minimal author moderation (core ForumMagnum behaviour, cheap with a unified collection), `discovery.indexers`, `post.labels` (safety), and `excerpt` (only when the body is spilled). In-memory indices and hand-written types with a runtime validator: **accept**. |
| §3 must-haves | **Accept all.** |
| §4 CLI | **Accept all.** |
| §5 | **Accept all.** |
| §6 | **Accept** `.temp` dogfooding. Multi-tab: **choose** Web Locks leader election plus per-repo locks rather than independent per-tab workers. |
| §7, §8 | **Accept**, merged into §8. |

**Earlier design drafts (the protocol, sync, social and architecture designs)**

| Draft position | Decision |
|---|---|
| Sync design: proofs and rangesync by default in the MVP | **Defer to P7.** Keep its key layout, verification code, scheduler concepts and cost model. |
| Sync design: three profiles × three verification levels | **Reduce** to `sources` × `verify`. |
| Sync design: roster pages and mod sweep | **Reject for v1** (could later be added as compaction). |
| Architecture design: KV-only indices, own lexicon codegen, `transition:generic`, `blob:image/* blob:video/*` only, 16 KB revision blobs, comments limited to `editedAt` | **Reject.** |
| Protocol design: forum-less posts, sections, author-scope mod actions without `forum`, integer vote values, unseeded karma, latest-time-wins for tier and section | **Reject.** |
| Social design: the late-arrival rule | **Reject** (needs a live observer; replaced by allow-lists and ban-hides-all). |
| Social design: the remaining dimensions (`readonly`, `votesVoid`, `official`, `acceptedAnswer`) | **Defer.** |

---

## Glossary

| Term | Meaning |
|---|---|
| Authority | The owner, or a moderator or admin in the current declaration. |
| Seed (S) | An authority, or a granted member, who is not banned or nullified. |
| E | Eligible (weighted) voters: S plus established users. |
| Newcomer | A participant who is not in S ∪ E. |
| fk | The 12-character forum key. |
| h16 | The 16-character subject hash. |
| t_rank | Effective time used for ranking (§4.2). |
| pds / proven / hint | Content trust levels (§1.2). |
| Closure | Repos referenced by seeds' records (§3.2). |
| Positive assertion | A moderation value that grants prominence or keeps content (CID-bound). |
| Fold | The pure derivation of a forum view from its inputs. |
| NS | The namespace prefix constant. |

---

### Critical Files for Implementation
- /home/user/lessdugin/docs/spec/data-model.md: the normative lexicons, rkey grammar, write and read rules, and decision log from §§2, 3.5 and 10.
- /home/user/lessdugin/lexicons/online/7778777/lessdugin/: one JSON file per NSID with final limits, plus `authFull.json`, `authInteract.json` and `index/`. The highest-stakes files are `opinion.json`, `modAction.json`, `forum.json`, `comment.json` and `post.json`.
- /home/user/lessdugin/crates/core/src/ids.rs (with /home/user/lessdugin/crates/lexicon/src/keys.rs): `fk/1` and `sk/1` derivation, key grammars, canonical at-uris and the time helpers, together with `testdata/vectors/keys`.
- /home/user/lessdugin/crates/core/src/social/fold.rs: the layered fold (`mod/1`, `karma/1`, scores, visibility, `magic/1`), `inputDigest`, and the property-tested invariants.
- /home/user/lessdugin/crates/core/src/host.rs (with /home/user/lessdugin/packages/engine/src/worker.ts): the `Host`/`KvStore` contract and the wasm JsHost, AuthBridge and leader bridge, which the M2 walking skeleton has to prove.