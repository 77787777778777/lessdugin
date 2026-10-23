<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin data model (normative)

**Status:** draft for the `.temp` namespace (M0). It is frozen at publication (M7).
**Sources:** `docs/design/synthesis-v1.md` §2, §3.5 and §10, with every fix from `docs/design/completeness-review.md` and the approved plan applied.

## 0. About this document

**Key words.** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

**Contents.** This document defines:
- namespaces and governance;
- every lexicon: the records, the shared definitions, the permission sets and the indexer queries;
- record keys;
- the record validity rules;
- the write protocol.

**Companion specifications**

| Document | Covers |
|---|---|
| `lfm-1.md` | LFM/1 rendering, the HTML contract, the plain-text projection P, quote resolution, excerpts |
| `social.md` | the fold: `mod/1`, `karma/1`, scores, gates, `default/1` reacts, visibility, `magic/1`, threads, tags and wiki, time (`t_rank`) |
| `sync.md` | identity, discovery, per-repo sync, live updates, `proofs/1`, store, media display |
| `limits.md` | every byte, count and depth cap |
| `threat-model.md`, `glossary.md` | adversaries and measures; terms |

**Behaviour IDs.** This document defines two:
- `fk/1`: the forum key, §3.1.
- `sk/1`: `h16`, `threadTid`, `stid` and the per-collection rkey grammars, §3.1–§3.3.

It refers to six more: `mod/1`, `karma/1`, `magic/1`, `lfm/1`, `default/1` and `proofs/1`. A behaviour ID never changes meaning. Its normative vectors live in `testdata/vectors/`.

**Precedence.** Until M7, this document wins over the lexicon JSON files in `lexicons/`, which must be corrected to match. After publication, the published lexicons win and this document is corrected.

---

## 1. Namespace, publishing, governance

### 1.1 Groups

`NS` = `online.7778777.lessdugin`; the NSID authority is `lessdugin.7778777.online`.

| Group | DNS TXT record | Contents |
|---|---|---|
| `online.7778777.lessdugin` | `_lexicon.lessdugin.7778777.online  "did=<publisher DID>"` | 10 record collections, `defs`, `markdown`, `authFull`, `authInteract` |
| `online.7778777.lessdugin.index` | `_lexicon.index.lessdugin.7778777.online  "did=<publisher DID>"` | `index.defs` and the indexer queries (§11) |
| `online.7778777.lessdugin.temp` | none; never published | development and dogfood builds (§1.3) |

### 1.2 Lexicon inventory

| NSID (suffix after `NS.`) | Lexicon type | Record key | Section |
|---|---|---|---|
| `forum` | record | `any` (slug) | §6.1 |
| `membership` | record | `any` (`{fk}`) | §6.2 |
| `post` | record | `any` (`{fk}-{tid}`) | §6.3 |
| `comment` | record | `any` (`{fk}-{rootTid}-{tid}`) | §6.4 |
| `revision` | record | `any` (`{P}-{tid}`) | §6.5 |
| `opinion` | record | `any` (`{fk}-{stid}-{h16}`) | §6.6 |
| `modAction` | record | `any` (`{fk}-{h16}`) | §6.7 |
| `wikiRevision` | record | `any` (`{fk}-{tid}`) | §6.8 |
| `sequence` | record | `any` (`{fk}-{tid}`) | §6.9 |
| `profile` | record | `literal:self` | §6.10 |
| `defs` | shared object defs | – | §4 |
| `markdown` | object (`main`) | – | §5 |
| `authFull`, `authInteract` | permission-set | – | §10 |
| `index.defs`, `index.*` | defs, queries | – | §11 |

The lexicon type of every record key is `any`, except `profile`, which is `literal:self`. The grammar of each key is enforced by the core (§3), not by the lexicon.

### 1.3 Development namespace (`NS.temp`)

**The prefix constant**
- The prefix is a build-time constant, `NS`.
  - Release builds (M7 and later) use `online.7778777.lessdugin`.
  - Development, devnet, CI and dogfood builds use `online.7778777.lessdugin.temp`.
- Lexicon files and this document always use the **final** NSIDs.
- A `.temp` build MUST substitute its prefix in every NSID it reads or writes:
  - collection names;
  - `$type` values, including union members such as `NS.markdown` and `NS.modAction#hide`;
  - the collection segment of at-uris;
  - indexer method names (`NS.temp.index.*`).
- A build MUST NOT read or write records of the other prefix.

**What does and does not change**
- The hash domain tags (`lessdugin-fk/1`, `lessdugin-sk/1`) do not change.
- Every `fk` and every `h16` does change between prefixes, because they hash at-uris that contain the collection NSID.
- `.temp` builds request the enumerated fallback scope (§10.3), because no permission set is ever published under `.temp`.

**Lifetime and migration**
- `.temp` data carries no compatibility promise and is orphaned at 0.1.
- `xtask migrate-temp` copies a user's own `.temp` records to `NS` with a pure, deterministic rewrite `m`:
  - `$type` values and at-uri collection segments get the final prefix;
  - every rkey is recomputed from the rewritten values by §3; TID segments are kept;
  - every strongRef `cid` becomes `cid(m(v))`, where `v` is the value whose CID is the old `cid`. That value is the target's current value, or a revision snapshot with that CID. If neither is available, the `cid` is left unchanged, and it then reads as "an unavailable version";
  - `m` applies recursively to nested values, including revision snapshots, and a revision's `subject.cid` becomes `cid(m(record))`;
  - blob refs, `createdAt` and every other field are unchanged.
- `m` depends only on record values, so every user's migration produces the same references. The recursion terminates: a strongRef can only name a value that already existed, so the chain of CID references is acyclic.
- Records are written in dependency order: forum, then posts, sequences and wikiRevisions, then comments, then opinions and modActions, then revisions.

### 1.4 Publisher account and governance

**Publisher account**
- A dedicated did:plc account with handle `lessdugin.7778777.online` and an offline rotation key.
- It holds one `com.atproto.lexicon.schema` record per published NSID, with rkey = NSID.

**Publishing**
- CI publishes only at M7, which needs the owner, and only after these pass:
  - `goat lex lint -I large-string` (goat warns on strings above 20,480 bytes; the 100,000-byte `markdown.text` of §5.1 is deliberate, and every other lint MUST pass);
  - `goat lex breaking` against the last published tag;
  - `goat lex check-dns`.
- Lexicons are compiled into every binary and MUST NOT be fetched at runtime.

**Domain hardening:** registrar lock, multi-year auto-renew, DNSSEC and CAA.

### 1.5 Naming rules

- A child group MUST NOT share a name with a record. There is never an `NS.forum.*` group.
- `index` and `temp` are group names and MUST NEVER be used as record names.
- Reserved names (§9) MUST NOT be written or published until they are final.

---

## 2. Conventions

### 2.1 Notation

| Notation | Meaning | Lexicon JSON |
|---|---|---|
| `!` | required (in the lexicon `required` set) | – |
| `s(G/B)` | string, at most G graphemes and B UTF-8 bytes | `{"type":"string","maxGraphemes":G,"maxLength":B}` |
| `s(Bb)` | string, at most B bytes, no grapheme limit (for example `s(64b)`) | `{"type":"string","maxLength":B}` |
| `kv{a, b}` | knownValues string; at most 64 bytes unless noted | `{"type":"string","knownValues":["a","b"],"maxLength":64}` |
| `=v` | lexicon default | `"default": v` |
| `[T]≤N` | array of at most N items | `{"type":"array","items":T,"maxLength":N}` |
| `blob(accept; max)` | blob with accept list and maxSize in bytes | `{"type":"blob","accept":[…],"maxSize":max}` |
| `blob(*/*; no max)` | blob, any type, no size limit | `{"type":"blob","accept":["*/*"]}` |
| `md` | open union of `NS.markdown` | `{"type":"union","refs":["online.7778777.lessdugin.markdown"]}` |
| `slug` | `s(64b)` that the core checks against `^[a-z0-9]+(-[a-z0-9]+)*$` | `{"type":"string","maxLength":64}` |
| `sref` | `com.atproto.repo.strongRef` `{uri, cid}` | `{"type":"ref","ref":"com.atproto.repo.strongRef"}` |
| `atu` | at-uri, at most 3000 bytes | `{"type":"string","format":"at-uri","maxLength":3000}` |
| `url` | URI, at most 3000 bytes. The `uri` format is RFC 3986 and therefore ASCII-only: the scheme is case-insensitive, and writers percent-encode any non-ASCII character (an IRI fails G2) | `{"type":"string","format":"uri","maxLength":3000}` |
| `int[a..b]` | integer between a and b inclusive | `{"type":"integer","minimum":a,"maximum":b}` |
| `did`, `cid`, `tid`, `datetime`, `nsid` | format-typed strings | `{"type":"string","format":"did"}` and so on |
| `count`, `sint` | integer in `[0..2^53−1]`, or in `[−(2^53−1)..2^53−1]` (indexer responses only, §11) | `{"type":"integer","minimum":0,"maximum":9007199254740991}` and so on |
| `bool` | boolean; absent means `false` unless a default is given | `{"type":"boolean"}` |
| `#x` | a def in the same lexicon | `{"type":"ref","ref":"#x"}` |

Lexicon unions are always open (`"closed"` is never set). Patterns, such as the slug grammar and the rkey grammars, cannot be expressed in Lexicon. The core enforces them (§7).

Lexicon does not allow an inline object as an array item or as a property, so every object in this document is a named def: `#moderator`, `#chapter` and so on, and the method-local defs of §11.3. Every lexicon document, def and field carries a short `description`. Descriptions are informative only; this document governs.

### 2.2 Limits

- Every string, array, integer and blob field gets its final limit at first publication. Limits never change afterwards (§13).
- Format-typed strings (did, cid, datetime, tid, language) rely on the limits built into their syntax.
- `maxLength` counts UTF-8 bytes.
- `maxGraphemes` counts extended grapheme clusters (UAX #29) as computed by the pinned `unicode-segmentation` crate. Changing that crate's Unicode version is a behaviour change, and it must pass `testdata/vectors/limits` unchanged.
- Operational caps (record size, page size, fetch caps, render limits) are in `limits.md`. The two that affect validity are repeated in §7.

### 2.3 References

**Canonical at-uri.** Every stored at-uri MUST have this form:

```
canonical-at-uri = "at://" DID "/" NSID "/" RKEY
DID   = "did:plc:" [a-z2-7]{24}
      | "did:web:" HOSTNAME        ; lowercase, no port, no path, no percent-encoding
NSID  = a valid NSID
RKEY  = a valid atproto record key; for an NS collection, it MUST also match that collection's grammar (§3.2)
```

- There is no handle authority, query, fragment, trailing slash or other non-canonical form.
- Other DID methods are not supported.
- On read, a reference field that holds a non-canonical at-uri, or a uri in a collection the field does not allow, makes the record **invalid** (G4, §7.2).

**Choosing `sref` or `atu`**
- Use **`sref`** where the version matters:
  - `comment.root` and `comment.parent`;
  - `opinion.subject`;
  - `revision.subject`;
  - `wikiRevision.parents`;
  - `modAction.adopts`;
  - `#tag.head`.
- Use **`atu`** where identity matters:
  - every `forum` field;
  - `#chapter.posts`;
  - `modAction.subject` (as a string, §6.7);
  - `#lock.allowed`;
  - `profile.pinned`;
  - `markdown.links`.

**Allowed collections per reference field**

| Field | Allowed collection(s) |
|---|---|
| `*.forum` | `NS.forum` |
| `comment.root` | `NS.post` |
| `comment.parent` | `NS.comment` |
| `revision.subject` | `NS.post`, `NS.comment`, `NS.forum`, `NS.sequence`, `NS.modAction` |
| `opinion.subject` | `NS.post`, `NS.comment`, `NS.wikiRevision`, `NS.sequence` |
| `modAction.adopts` | `NS.modAction` |
| `modAction.subject` (at-uri form) | depends on the action (§6.7) |
| `#lock.allowed[]` | `NS.comment` |
| `#tag.head` | `NS.wikiRevision` |
| `wikiRevision.parents[]` | `NS.wikiRevision` |
| `#chapter.posts[]` | `NS.post` |
| `profile.pinned[]` | `NS.post`, `NS.comment`, `NS.sequence` |
| `markdown.links[]` | any collection (canonical syntax still required) |

### 2.4 Values

**Datetimes**
- Writers emit RFC 3339 UTC with millisecond precision and a `Z` suffix: `YYYY-MM-DDTHH:MM:SS.sssZ`.
- Readers MUST accept any valid atproto datetime. How a datetime converts to integer microseconds is defined in `social.md` (time).

**Numbers.** There are no floats in any record or query. Integers are signed and stay within their lexicon bounds. Indexer ranks are decimal strings (§11).

**Unions and enumerations**
- All unions are open.
- Every enumeration is a knownValues string with a documented fallback for unknown values (§13.3).

**Strings** are stored as written; there is no normalisation on write. The core applies NFC only where a spec says so: hash parts (§3.1) and the projection P.

### 2.5 Writes (summary)

The full write protocol is §8. In short, writers:
- always include `$type`;
- never pass `validate` (they leave it unset);
- preserve unknown fields on read-modify-write;
- write only canonical references, deterministic keys and fields within limits;
- serialise writes per account, with the swap head taken from a fresh `getLatestCommit`.

---

## 3. Record keys

### 3.1 Functions (`fk/1`, `sk/1`)

```
b32s(bytes)    = base32, read MSB-first in 5-bit groups, alphabet "234567abcdefghijklmnopqrstuvwxyz",
                 no padding. b32s(x)[0..n] is its first n characters; only n ≤ 51 is used, so the
                 trailing partial group never matters. Sort order of the characters = bit order.
H(tag, p1..pk) = sha256( UTF8(tag) ‖ (0x1F ‖ UTF8(NFC(p_i)))  for i = 1..k )
                 MUST reject (no key; the input is invalid) if any p_i contains U+001F.
fk(forumUri)   = b32s(H("lessdugin-fk/1", forumUri))[0..12]                      // 60 bits     (fk/1)
h16(name, p…)  = b32s(H("lessdugin-sk/1", name, p…))[0..16]                     // 80 bits     (sk/1)
                 name = the short collection name: "opinion" or "modAction"
threadTid(uri) = let c = collection(uri), k = rkey(uri):
                   c = NS.post    ∧ k matches ^FK-TID$       → the TID segment
                   c = NS.comment ∧ k matches ^FK-TID-TID$   → the FIRST TID segment
                   otherwise undefined
stid(subject)  = threadTid(subject.uri)   if collection(subject.uri) ∈ {NS.post, NS.comment}
               = "~"                      if collection(subject.uri) ∈ {NS.wikiRevision, NS.sequence}
P(subject)     = fk(subject.uri)          if collection(subject.uri) = NS.forum
               = rkey(subject.uri)        otherwise (post, comment, sequence, modAction)
fkOf(rkey)     = the first 12 characters of rkey (for rkeys of the forum-scoped collections)
```

**Notes**
- `name` is hashed as a part, so the input is `"lessdugin-sk/1" 0x1F "opinion" 0x1F <uri>`.
- `forumUri` and every at-uri part are canonical (§2.3). All v1 parts are ASCII, so NFC is a no-op; it is specified for robustness.

**Token grammars**

```
FK   = [2-7a-z]{12}
H16  = [2-7a-z]{16}
TID  = [2-7a-j][2-7a-z]{12}          ; atproto TID syntax (13 chars, top bit 0)
SLUG = [a-z0-9]+(-[a-z0-9]+)*        ; at most 64 bytes
```

TID time is `(integer value of the TID) >> 10`, in microseconds since the Unix epoch. The low 10 bits are the clock id.

### 3.2 Grammar per collection

A record is valid only if its rkey equals the canonical key computed from its own fields. The table lists the grammar and the equalities checked.

| Collection | Rkey grammar | Canonical value (all equalities MUST hold) | Own TID |
|---|---|---|---|
| `forum` | `^SLUG$` | – | – |
| `membership` | `^FK$` | `rkey = fk(forum)` | – |
| `post` | `^FK-TID$` | `fkOf(rkey) = fk(forum)` | the TID |
| `comment` | `^FK-TID-TID$` (`{fk}-{rootTid}-{tid}`) | `fkOf(rkey) = fk(forum)`; root rkey matches `^FK-TID$` with `fkOf(root rkey) = fkOf(rkey)`; middle TID `= threadTid(root.uri)` | the last TID |
| `revision` | `^P-TID$` | `rkey = P(subject) + "-" + TID`; `did(subject.uri) =` the repo DID | the last TID |
| `opinion` | `^FK-(TID\|~)-H16$` | `rkey = fk(forum) + "-" + stid(subject) + "-" + h16("opinion", subject.uri)`; `fkOf(rkey(subject.uri)) = fk(forum)` | – |
| `modAction` | `^FK-H16$` | `rkey = fk(forum) + "-" + h16("modAction", scope_eff, subject, actionKey(action))` (§6.7) | – |
| `modAction` (reserved) | `^FK-TID$` | reserved for P2 list actions; such records are **ignored** in v1 (no effect, raw view only) | – |
| `wikiRevision` | `^FK-TID$` | `fkOf(rkey) = fk(forum)` | the TID |
| `sequence` | `^FK-TID$` | `fkOf(rkey) = fk(forum)` | the TID |
| `profile` | `self` | literal | – |

**Notes**
- `P` is variable-length, so the revision grammar is checked by equality with `P(subject)`, not by a regex. A revision key is `P` followed by exactly `"-"` and one TID.
- In the table's grammar column, the `|` inside the opinion row is a regex alternation.
- `stid = "~"` sorts after every TID, so opinions on wiki revisions and sequences fall in their own sub-range.
- TID syntax is checked for every TID segment. TID *time* plays no part in validity; it is an input to the time rules in `social.md`.

**Key lengths:** membership 12; post, wikiRevision and sequence 26; modAction 29; opinion 43 (or 31 with `~`); comment 40; revision `|P| + 14`, which is 26 for a forum, 40 for a post or sequence, 43 for a modAction and 54 for a comment; forum ≤ 64; profile 4. All are within the 512-character rkey limit.

**Range listings and prefixes.** A prefix scan is not an exact filter, because revision and comment keys nest:
- A post's revisions are `{fk}-{T}-{tid}`.
- Revisions of comments in the same thread are `{fk}-{T}-{t}-{tid}`.
- Both start with `{fk}-{T}-`.

So any code that selects records by key prefix (thread ranges, revisions of a subject) MUST also check the full grammar and, for revisions, `subject.uri`.

### 3.3 Why this layout (informative)

**Benefits**
- A forum is one key range (`{fk}-`) per collection in every repo.
- A thread's comments are one range (`{fk}-{rootTid}-`).
- Interactions can later be windowed by thread time (P7).
- Comments cannot hide from a future windowed sync by backdating, because their middle segment is tied to the root post.
- A one-per-(actor, subject) record (membership, opinion, modAction) is a single `getRecord`.

**Collisions**
- An fk collision (60 bits) costs only efficiency, except for membership: there it would block membership of both colliding forums. A targeted collision takes about 2^60 work.
- An h16 collision (80 bits) is negligible.

### 3.4 Worked example (informative; the normative vectors are in `testdata/vectors/keys/*.json`)

| Input | Output |
|---|---|
| `fk("at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.forum/rationality")` | `uetjixxr3a33` |
| the same forum under `.temp` (`…lessdugin.temp.forum/rationality`) | `2xbiijsnjtau` |
| post with TID `3l2s5xxv2ze2c` in that forum: rkey | `uetjixxr3a33-3l2s5xxv2ze2c` |
| `h16("opinion", "at://did:plc:ewvi7nxzyoun6zhxrhs64oiz/online.7778777.lessdugin.post/uetjixxr3a33-3l2s5xxv2ze2c")` | `wcz54x7liy3jsh3g` |
| opinion rkey on that post | `uetjixxr3a33-3l2s5xxv2ze2c-wcz54x7liy3jsh3g` |
| comment by `did:plc:abcdefghijklmnopqrstuvwx` with TID `3l2s6aaaaaaa2` in that thread: rkey | `uetjixxr3a33-3l2s5xxv2ze2c-3l2s6aaaaaaa2` |
| opinion rkey on that comment | `uetjixxr3a33-3l2s5xxv2ze2c-3mjfititlivah7tw` |
| modAction `#access`, scope `forum`, subject `did:plc:abcdefghijklmnopqrstuvwx` | `uetjixxr3a33-ckjbz7a2pjeplbao` |
| modAction `#hide`, scope `forum`, subject = the post uri | `uetjixxr3a33-jmsm5bwwzljcu2zo` |
| modAction `#pin` (`pin:forum`), subject = the post uri | `uetjixxr3a33-4vdjzbg6ohwpa323` |
| modAction `#tag`, subject `tag:decision-theory` | `uetjixxr3a33-yext7l62fmg5ufrt` |

The same `b32s` implementation reproduces the superseded draft vectors (`ncepyaoj3bijw6li` and `q2swgvbea4gb2m6c`). Those used the full NSID as the name, a 10-byte truncation and no fk prefix, and they MUST NOT be used.

**Vector files.** `testdata/vectors/keys/gen_keys.py` generates them (`--check` verifies them); each file is `{"behaviour", "kind", "description", "vectors": [{"name", "input", "expected", "raw"?, "note"?}]}`.

| File | Behaviour | `input` → `expected` |
|---|---|---|
| `fk.json` | `fk/1` | `{forumUri}` → `{fk, digest}` |
| `sk.json` | `sk/1` | `{fn: "b32s", hex, n}` → `{b32s}`; `{fn: "H", tag, parts}` → `{preimage, digest}`; `{fn: "h16", name, parts}` → `{h16, digest}` (hashed parts are `[name, …parts]`) |
| `rkeys.json` | `sk/1` (uses `fk/1`) | `{fn, ns, …}` for `threadTid`, `stid`, `P`, `actionKey` and `rkey` (by short `collection` name) → the named result, plus intermediates (`fk`, `threadTid`, `stid`, `h16`, `P`, `scopeEff`, `actionKey`) |

- `digest` and `preimage` are lowercase hex. `ns` is the build prefix the runner assumes (§1.3).
- A failing derivation expects `{"error": code}`: `UnitSeparator` (a hash part contains U+001F), `Undefined` (`threadTid`, `stid` or `P` is undefined, or a comment root is not an `NS.post` uri with key `^FK-TID$`), `InvalidTid` (a `tid` input is not TID syntax). No vector has more than one applicable error.
- `raw: true` marks inputs that are not canonical v1 values; they test NFC and U+001F handling in `H`. Runners MUST evaluate them with the string-level functions, without at-uri validation.
- Runners MUST compare the named result or `error`, and SHOULD compare every intermediate they expose. Derivation never checks record validity (G4, G5, OP2, MA1–MA4).

---

## 4. Shared definitions (`NS.defs`)

```
#media        { blob!: blob(*/*; no max), alt: s(2000g/20000b), aspectRatio: #aspectRatio,
                durationMs: int[0..86400000], name: s(255b) }
#aspectRatio  { width!: int[1..1000000], height!: int[1..1000000] }
#textQuote    { exact!: s(1000g/10000b), prefix: s(64g/640b), suffix: s(64g/640b), start: int[0..100000000] }
#externalLink { uri!: url, title: s(300g/3000b), description: s(1000g/10000b), thumb: blob(image/*; 1000000) }
#link         { uri!: url, label: s(64g/640b) }
```

**`#media`**
- `blob` holds the object returned by `uploadBlob`, verbatim (§8.8).
- `alt` is the accessible description. Writers warn when it is missing.
- `aspectRatio` is a layout hint. Display clamps it to between 1:20 and 20:1.
- `durationMs` applies to audio and video.
- `name` is opt-in, display-only, and a basename only. Readers MUST ignore a `name` that contains `/`, `\`, a C0 or C1 control, or U+2028/U+2029.
- Downloads are always saved as `<cid>.<ext-from-sniffed-type>`.

**`#textQuote`** selects text in the plain-text projection P (`lfm-1.md`) of the target's body.
- `exact` is trimmed, so it starts and ends on a non-space.
- `prefix` and `suffix` are verbatim substrings of P immediately before and after `exact`. Writers SHOULD use 32 graphemes or fewer.
- `start` is the code-point offset of `exact` in P of the version the writer saw. It is a hint only.
- Resolution is deterministic and defined in `lfm-1.md`.

**`#externalLink`** is a link card.
- Writers fill `title`, `description` and `thumb` manually or by a native fetch. The SPA never fetches cards.
- `uri` is shown as a link only if `safe_href` accepts it; otherwise it is shown as text.

**`#link`** is a labelled URL, shown through `safe_href`.

---

## 5. `NS.markdown`

`online.7778777.lessdugin.markdown` has a `main` def of type `object`. It is the only member of every `md` union, written with `"$type": "online.7778777.lessdugin.markdown"`. Other apps may embed it, for example as `site.standard.document.content`.

### 5.1 Fields

| Field | Type | Req | Default | Meaning |
|---|---|---|---|---|
| `profile` | `kv{"lfm/1"}` | | `"lfm/1"` | the markdown dialect |
| `text` | `s(100000b)` | (§5.2) | | inline LFM source |
| `textBlob` | `blob(text/*; 1000000)` | (§5.2) | | spilled LFM source, UTF-8 |
| `media` | `[defs#media]≤128` | | | the only scope for `cid:` URLs in this body |
| `mentions` | `[did]≤128` | | | derived by the writer; hint only |
| `links` | `[atu]≤256` | | | derived by the writer; hint only |

### 5.2 Body rules

1. Exactly one of `text` or `textBlob` MUST be present. An empty `text: ""` counts as present. If both are present, or neither, the `md` value is invalid, and so is its record (G7).
2. `textBlob` is allowed only in `post.body`, `wikiRevision.body`, `forum.about` and `sequence.body`. Every other `md` field is **inline only**: `comment.body`, `#tier.notice`, `profile.bio`, `#chapter.body`. A `textBlob` there makes the record invalid (G7).
3. Writers set the `textBlob` `mimeType` to exactly `text/markdown`.
   - Readers fetch the blob from the author's PDS through the streaming, CID-checked `Host::fetch_blob` (`sync.md`), so no blob bytes pass through wasm memory unbounded.
   - Readers accept any `text/*`, decode the bytes as UTF-8 whatever the charset parameter says, and strip one leading U+FEFF.
   - Invalid UTF-8, or a fetched blob whose sha256 does not match its CID, renders an "unreadable body" placeholder. The record stays valid.
4. `cid:` image URLs resolve only against `media[].blob.ref` of the same object (`lfm-1.md`). Media that the text does not reference renders as a gallery after the body.
5. An unknown `profile` value is rendered as `lfm/1` with a badge (§13.3).
6. An `md` union member with an unknown `$type` is not rendered: the UI shows "unsupported body format", and the record stays valid.

### 5.3 Spill rules (writers)

1. A spillable field (rule 2 above) MUST be written as `textBlob` when its UTF-8 source exceeds 65,536 bytes (64 KiB).
2. Before sending, the writer serialises the exact XRPC request body (§8.9). If it exceeds 900,000 bytes, the writer spills the largest remaining inline spillable field of the **new** value and re-measures, repeating until the body fits or nothing is left to spill. If it still does not fit, the write is refused with `TooLarge`; for an edit, the client offers `--no-history`.
3. A spilled `post.body` requires `post.excerpt` (§6.3). The writer computes it as the LFM/1 excerpt (`lfm-1.md` §10.4, computed on the spoiler-free projection P_x).
4. Non-spillable fields stay inline up to their 100,000-byte limit. A longer source is refused at compose time.

### 5.4 Derived hint fields and pingbacks

- Writers derive `mentions` and `links` from the rendered LFM: mentions are `[@x](at://did)` links, and links are other `at://` links, rewritten to canonical at-uris.
- Readers MUST NOT use them for rendering, notifications or the fold. Rendering, mentions and **pingbacks are always computed from the rendered bodies of synced records**.
- The two fields exist for external backlink indexes such as Constellation and the indexer's `listBacklinks`, whose results are hints.

---

## 6. Records (v1)

Each table is the complete field list. `createdAt` is set once at creation and never changed. `updatedAt`, where a collection has it, is set on every update. Collections without `updatedAt` (membership, revision, wikiRevision) are never updated by lessdugin writers.

### 6.1 `forum` (rkey `slug`; in the owner's repo)

| Field | Type | Req | Default | Notes |
|---|---|---|---|---|
| `name` | `s(100g/1000b)` | ! | | display name |
| `description` | `s(500g/5000b)` | | | plain-text tagline |
| `about` | `md` | | | rules and introduction; MAY spill |
| `icon` | `blob(image/*; 2000000)` | | | |
| `moderators` | `[#moderator]≤64` | | `[]` | authorities other than the owner |
| `membership` | `kv{open, approval}` | | **`approval`** | writers MUST write it explicitly |
| `defaultTier` | `kv{personal, frontpage}` | | `personal` | tier of a post with no resolved `#tier` |
| `featuredTags` | `[slug]≤64` | | | core tags offered as filters |
| `policy` | `#policy` | | every sub-default | |
| `reportContact` | `url` | | | `mailto:`, `https:` or `at://` (DM handle); shown via `safe_href` |
| `discovery` | `#discovery` | | | |
| `createdAt` | `datetime` | ! | | |
| `updatedAt` | `datetime` | | | |

**Definitions inside `forum`**

```
#moderator { did!: did, role!: kv{admin, moderator}, title: s(64g/640b) }
#policy    { moderation: kv{"mod/1"} ="mod/1", karma: #karmaP, ranking: #rankP,
             voting: kv{karmaOnly, twoAxis, reacts} =reacts, reactPalette: kv{"default/1"} ="default/1",
             gates: #gates, display: #display, authorModeration: bool =true,
             tagCreation: kv{open, moderators} =open }
#karmaP    { algorithm: kv{"karma/1"} ="karma/1", pairCap: int[1..1000000] =25,
             establish: int[0..1000000] =1, establishSeeds: int[1..1000] =2 }
#rankP     { algorithm: kv{"magic/1"} ="magic/1", offsetHours: int[1..10000] =2,
             exponentMilli: int[0..10000] =1150, curatedBonus: int[-1000000..1000000] =10,
             frontpageBonus: int[-1000000..1000000] =0, cutoffDays: int[1..36500] =90 }
#gates     { reactSecond, reactCreate, reactAnti, authorModPersonal, authorModFrontpage,
             createTag, wikiEdit, sequence : int[-1000000000..1000000000] }   // absent ⇒ karma/1 default
#display   { collapseBelow: int[-1000000..1000000] =-5, newUsers: kv{show, collapse, hide} =collapse }
#discovery { indexers: [#endpoint]≤8 }
#endpoint  { url!: url, did: did, note: s(300g/3000b) }
```

**Authority and display**
- **Identity.** A forum is identified by its at-uri. The owner is the repo DID: rank 3 and an implicit admin. The owner is never listed in `moderators`.
- **Authority comes only from the current declaration** (`social.md`, `mod/1`). Removing a moderator lapses their assertions at once (§8.6).
- **Moderator badge.** A listed moderator is *shown* as a moderator only if they have a membership record in the forum. Their authority does not depend on it.
- **Gates.** `#gates` holds karma thresholds, and an absent field means the `karma/1` default. Semantics and defaults are in `social.md`.

**Ranking and display parameters**
- `#rankP.cutoffDays` limits **only** the `magic` sort; `top`, `new` and `old` include every item.
- `curatedBonus` and `frontpageBonus` feed `magic/1`.
- `#display.collapseBelow` and `newUsers` feed visibility.

**Membership mode**
- `approval` (the lexicon default) needs only CORE services to be complete.
- The UI labels `open` as "relies on public relay infrastructure (Bluesky relay, Jetstream, Constellation)".
- An owner can archive a forum by switching it to `approval`. A forum-wide lock is reserved (§9).

**Succession**
- The forum is bound to its owner DID forever, because `fk` hashes its at-uri.
- The guide MUST recommend a dedicated did:plc forum account whose rotation keys are held by at least two admins.
- Clients warn when the owner or a moderator uses did:web, because domain expiry means takeover.
- `successor` and `aliases` are reserved field names for P2 (§9).

**Discovery endpoints.** An endpoint in `discovery.indexers` is used only after one-time user consent, and only if its `url` is `https:` with a hostname and the default port. Other entries are ignored.

**Values written by `forum create`.** The writer MUST write `membership`, `defaultTier` and every `policy` field explicitly. With no flags it writes `membership: approval`.

| Field | open preset | approval preset |
|---|---|---|
| `defaultTier` | `personal` | `personal` |
| `policy.moderation`, `.reactPalette`, `.voting`, `.authorModeration`, `.tagCreation` | `mod/1`, `default/1`, `reacts`, `true`, `open` | same |
| `policy.karma` | `{karma/1, pairCap 25, establish 1, establishSeeds 2}` | same (`establishSeeds` has no effect in approval mode) |
| `policy.ranking` | `{magic/1, offsetHours 2, exponentMilli 1150, curatedBonus 10, frontpageBonus 0, cutoffDays 90}` | same |
| `policy.gates` `reactSecond/reactCreate/reactAnti` | 5 / 10 / 20 | 0 / 0 / 5 |
| `policy.gates` `authorModPersonal/authorModFrontpage` | 50 / 2000 | 10 / 100 |
| `policy.gates` `createTag/wikiEdit` | 10 / 5 | 0 / 0 |
| `policy.gates` `sequence` | 1000 | 0 |
| `policy.display` | `{collapseBelow -5, newUsers collapse}` | same |

### 6.2 `membership` (rkey `{fk}`; in the member's repo)

| Field | Type | Req | Notes |
|---|---|---|---|
| `forum` | `atu` (`NS.forum`) | ! | |
| `note` | `s(2000g/20000b)` | | application text; **public** |
| `createdAt` | `datetime` | ! | |

**Meaning**
- The key allows exactly one membership per (member, forum).
- The record is the join beacon (open mode) or the application (approval mode). Participation rules are in `social.md`.
- Leaving deletes the record.

**Followed forums.** Clients SHOULD seed the "followed forums" list (Home) from the user's own membership records: list `NS.membership` in the user's repo and read each `forum` field. The fk cannot be reversed. This is how the followed list carries across devices.

### 6.3 `post` (rkey `{fk}-{tid}`)

| Field | Type | Req | Default | Notes |
|---|---|---|---|---|
| `forum` | `atu` (`NS.forum`) | ! | | fixed after creation |
| `kind` | `kv{article, link, question, quickTake}` | | `article` | |
| `title` | `s(300g/3000b)` | by kind | | |
| `body` | `md` | by kind | | MAY spill |
| `excerpt` | `s(500g/5000b)` | if spilled | | plain text; read only when `body` uses `textBlob` |
| `link` | `defs#externalLink` | by kind | | |
| `tags` | `[slug]≤32` | | | the author's own tag applications (set semantics) |
| `labels` | `union[com.atproto.label.defs#selfLabels]` | | | self-labels, for example `sexual`, `nudity`, `porn`, `graphic-media` |
| `createdAt` | `datetime` | ! | | |
| `updatedAt` | `datetime` | | | |

**Fields required per kind.** These are core validity rules (PO3–PO6). The effective kind is `kind` if it is known, otherwise `article`.

| Effective kind | `title` | `body` | `link` | Display |
|---|---|---|---|---|
| `article` | **required** | optional | ignored | |
| `question` | **required** | optional | ignored | question badge; answers are reserved (P2) |
| `link` | **required** | optional | **required** | link card plus body |
| `quickTake` | **ignored** (not shown, indexed or searched) | **required** | ignored | untitled; listed only under the All filter, with a badge |
| any kind with a spilled `body` | | | | `excerpt` **required** |

A "required" `title` must contain at least one non-`White_Space` character.

### 6.4 `comment` (rkey `{fk}-{rootTid}-{tid}`)

| Field | Type | Req | Notes |
|---|---|---|---|
| `root` | `sref` (`NS.post`) | ! | the thread's post; the key's middle segment is bound to it |
| `parent` | `sref` (`NS.comment`) | | the comment replied to; absent means top-level |
| `forum` | `atu` (`NS.forum`) | ! | MUST equal the root post's `forum` |
| `body` | `md` | ! | inline only |
| `quote` | `defs#textQuote` | | anchored in `parent` if present, otherwise in `root` |
| `retracted` | `bool` | | rendered struck through |
| `moderatorHat` | `bool` | | shown only if the author is a current authority |
| `createdAt` | `datetime` | ! | |
| `updatedAt` | `datetime` | | |

**Placement**
- A parent that is missing, or that belongs to another root, puts the comment under a `Missing(uri)` placeholder.
- Cycles are cut deterministically.
- Both rules are in `social.md` (threads).

**Versions.** `root.cid` and `parent.cid` record the version replied to. A comment on an older version shows "on revision n" or "on an unavailable version".

### 6.5 `revision` (rkey `{P}-{tid}`)

| Field | Type | Req | Notes |
|---|---|---|---|
| `subject` | `sref` | ! | the superseded (or removed) version; same repo as the revision |
| `op` | `kv{update, delete}` | ! | `delete` only when the subject is a `modAction` |
| `record` | `unknown` | (writers MUST include) | the verbatim prior value; an object whose `$type` MUST be present and equal the subject's collection |
| `summary` | `s(300g/3000b)` | | edit summary |
| `createdAt` | `datetime` | ! | |

There is no `updatedAt`, because revisions are never updated.

**What gets revisions**
- An edit snapshots the value it replaces (§8.3).
- `op: update` is used for posts, comments, sequences, the forum declaration and modActions.
- `op: delete` exists **only** for modActions: removing a modAction writes a delete revision, which preserves the mod log.
- Deleting content (post, comment, sequence, forum) **purges** that content's revisions instead (§8.7).

**Integrity**
- `cid(DAG-CBOR(record)) = subject.cid` is a validity rule (RV5), so a snapshot is the exact prior version.
- History is verifiable but not complete: authors may delete (redact) revision records.
- A missing snapshot shows "history redacted".
- A revision without `record` is valid. Its history entry shows "snapshot not available".

**Display.** A snapshot is rendered in the history view only if it passes lexicon validation for its collection (G2). Its rkey and reference rules do not apply. Otherwise the entry shows "invalid snapshot", and the content appears only in the raw view.

### 6.6 `opinion` (rkey `{fk}-{stid}-{h16("opinion", subject.uri)}`)

There is one opinion per (voter, subject). It holds the karma axis, the agreement axis, reacts and tag votes.

| Field | Type | Req | Notes |
|---|---|---|---|
| `subject` | `sref` (post, comment, wikiRevision, sequence) | ! | `cid` = the version the voter judged |
| `forum` | `atu` (`NS.forum`) | ! | |
| `karma` | `kv{up, down, strongUp, strongDown}` | | karma axis |
| `agreement` | `kv{agree, disagree, strongAgree, strongDisagree}` | | agreement axis; never feeds karma |
| `reacts` | `[#react]≤32` | | |
| `tags` | `[#tagVote]≤32` | | only when the subject is a post |
| `createdAt` | `datetime` | ! | |
| `updatedAt` | `datetime` | | |

```
#react   { react!: slug, stance: kv{for, against} =for, quote: defs#textQuote }
#tagVote { tag!: slug, value!: kv{up, down, strongUp, strongDown} }
```

**Writer rules**
- An opinion whose known content is empty is deleted, not written (§8.5).
- A writer MUST NOT write two `#react` entries with the same `(react, qkey(quote))`, where `qkey` is defined in `lfm-1.md` §11 (the `exact` text trimmed, NFC-normalised, with White_Space runs replaced by one U+0020), or the empty string when there is no quote.
- A writer MUST NOT write two `#tagVote` entries with the same `tag`.

**Reader rules** (entry level; the record stays valid)
- An unknown value disables only its axis or entry.
- Tags on a non-post subject are ignored.
- Several `#tagVote` entries for one slug: if all have the same value they collapse to one; otherwise every entry for that slug is ignored.
- React deduplication and contradiction rules are in `social.md`.
- `subject.cid` never affects counting. It drives the "N of M votes were cast on an earlier version" display.

### 6.7 `modAction` (rkey `{fk}-{h16("modAction", scope_eff, subject, actionKey)}`; in the actor's repo)

There is one record per (actor, scope, subject, action key). It states moderation *state*: deleting it removes the assertion.

| Field | Type | Req | Default | Notes |
|---|---|---|---|---|
| `forum` | `atu` (`NS.forum`) | ! | | |
| `scope` | `kv{forum, author}` | | `forum` | `scope_eff` = `scope` if present, else `"forum"` |
| `subject` | `s(3000b)` | ! | | `did` \| canonical at-uri \| `"tag:" + slug` |
| `subjectCid` | `cid` | (core-required for positive assertions) | | the version the action was taken on |
| `action` | open union of the defs below | ! | | |
| `reason` | `s(2000g/20000b)` | | | public; shown if `#hide.showReason` |
| `until` | `datetime` | | | the assertion lapses when `until ≤ now` |
| `adopts` | `sref` (`NS.modAction`) | | | the former moderator's assertion re-issued by this record |
| `createdAt` | `datetime` | ! | | |
| `updatedAt` | `datetime` | | | |

**Action definitions.** The `$type` of each is `online.7778777.lessdugin.modAction#<name>`.

| Def | Fields | `actionKey` | Allowed subject | Positive (needs `subjectCid`) when | Allowed in `scope=author` |
|---|---|---|---|---|---|
| `#hide` | `hidden: bool =true`, `spam: bool`, `showReason: bool =true` | `"hide"` | at-uri of a post, comment, sequence or wikiRevision | `hidden = false` (keep) | yes, if `hidden = true` and the subject is a comment; `spam` is ignored |
| `#access` | `level!: kv{banned, member}` | `"access"` | `did` | – | yes, if `level = banned` |
| `#restrict` | `capabilities: [kv{post, comment, vote, react, tag, wiki, sequence}]≤16`, `nullifyVotes: bool` | `"restrict"` | `did` | – | no |
| `#tier` | `tier!: kv{personal, frontpage, curated}`, `notice: md` (inline) | `"tier"` | at-uri of a post | `tier ∈ {frontpage, curated}` | no |
| `#pin` | `scope!: kv{forum, thread}`, `pinned: bool =true` | `"pin:" + scope` | `forum`: post or sequence; `thread`: comment | `pinned = true` | yes, if `scope = thread` |
| `#lock` | `locked: bool =true`, `allowed: [atu]≤5000` | `"lock"` | at-uri of a post (a forum uri is **reserved**, §9) | – | yes (own post) |
| `#tag` | `removed: bool`, `head: sref` (`NS.wikiRevision`) | `"tag"` | `"tag:" + slug` | – (`head` is CID-bound by its strongRef) | no |
| unknown `$type` | – | the `$type` string | any | – | – (no effect; shown as "unknown action") |

**Semantics** (resolution, ranks and authorisation are in `social.md`, `mod/1`)
- **Positive assertions.** Keep, tier ≥ frontpage and pinned are bound to `subjectCid`. They take effect only while `subjectCid` equals the subject's current CID; a stale one becomes a mod-queue item.
- **Negative assertions** take effect whatever the CID. On did and tag subjects, `subjectCid` is ignored.
- **`#restrict` is retroactive.** It hides every in-forum item of each restricted kind by that user, as `Hidden(Restricted)`. Item-level keeps override it, and `until` lapses it. The mod UI says it is retroactive.
- **`tag:` subjects** have author rank 0.
- **`#lock.allowed`** lists the comments that stay visible on a locked post. Entries outside the post's thread are ignored. A lock that would need more than 5,000 entries is refused at write time (§8.6).
- **Undoing** an action deletes the actor's own record, through the modAction removal protocol (§8.4). Overriding someone else's assertion means writing the opposite value. For example, *keep* is `#hide{hidden: false}`.
- **Edits.** Updates and removals of modActions always go through the revision protocol. There is no `--no-history` for modActions.

### 6.8 `wikiRevision` (rkey `{fk}-{tid}`; immutable by convention)

| Field | Type | Req | Default | Notes |
|---|---|---|---|---|
| `forum` | `atu` (`NS.forum`) | ! | | |
| `slug` | `slug` | ! | | page identity is `(forum, slug)` |
| `parents` | `[sref]≤8` (`NS.wikiRevision`) | | | empty or absent means genesis |
| `name` | `s(100g/1000b)` | ! | | |
| `description` | `s(300g/3000b)` | | | plain text, for hovers and cards |
| `body` | `md` | | | MAY spill |
| `isTag` | `bool` | | `true` | the page can be applied as a post tag |
| `parentTags` | `[slug]≤16` | | | parent tags |
| `summary` | `s(300g/3000b)` | | | edit summary |
| `createdAt` | `datetime` | ! | | |

There is no `updatedAt`; writers never update a wikiRevision.

**Linear history in the MVP**
- MVP writers MUST write `parents` as either empty (genesis) or exactly one sref: the page head the author edited.
- Readers MUST accept up to 8 parents, because merges come in P2.
- Head selection and the "deleted parent means a flagged root" rule are in `social.md`.
- A parent with a different `(forum, slug)` gives no edge.

### 6.9 `sequence` (rkey `{fk}-{tid}`)

| Field | Type | Req | Notes |
|---|---|---|---|
| `forum` | `atu` (`NS.forum`) | ! | |
| `title` | `s(300g/3000b)` | ! | |
| `body` | `md` | | MAY spill |
| `cover` | `defs#media` | | displayed only if `blob.mimeType` is `image/*` |
| `chapters` | `[#chapter]≤50` | ! | |
| `createdAt` | `datetime` | ! | |
| `updatedAt` | `datetime` | | |

```
#chapter { title: s(300g/3000b), body: md (inline only), posts!: [atu]≤100 }   // posts: NS.post at-uris
```

**Entries**
- Entries are at-uris, so they always show the latest version. They may be by other authors.
- An entry whose rkey fk differs from the sequence's fk is not displayed.

### 6.10 `profile` (rkey `self`)

| Field | Type | Req | Notes |
|---|---|---|---|
| `displayName` | `s(64g/640b)` | | |
| `avatar` | `blob(image/*; 2000000)` | | |
| `bio` | `md` | | inline only |
| `links` | `[defs#link]≤16` | | |
| `pinned` | `[atu]≤16` | | post, comment or sequence uris |
| `createdAt` | `datetime` | | optional here |
| `updatedAt` | `datetime` | | |

**Fallback per field**
- When `NS.profile/self` is absent, or lacks `displayName` or `avatar`, clients use that field from `app.bsky.actor.profile/self`.
- The fallback is read from the user's own PDS and validated against its lexicon. It is never read from an AppView.

---

## 7. Record validity

Validity is decided per record by the core. It is a pure function of the record's uri and value, plus, for the rules marked *(x)*, of other records in the fold input.

An **invalid** record:
- is excluded from the fold (moderation, karma, scores, visibility, ranking, notifications, search);
- appears only in the raw view, annotated with the first failing rule ID.

Rules marked *entry* do not invalidate the record: they discard one field, axis or array entry.

**Rule order.** The first failing rule is the first in this order: G1, G2, G3, G4, G5, G6, G7, G8, G9, G10, then the collection's rules in table order (§7.3). G9 and the key equalities of FO1, ME1, PO1, CO1, CO2, RV1, RV3, OP2, MA2, WR1, SQ1 and PF1 are part of G5, so a record that fails one of them also fails G5, which is reported first. The order is that of the rule IDs, not the order in which an implementation checks them (§14 item 23).

### 7.1 Admission (preconditions, defined in `sync.md`)

A record is considered at all only if it was admitted at trust level `pds` or `proven`:
- **`pds`:** fetched from the author's PDS (the one in the DID doc) over TLS, with JSON → DAG-CBOR → CIDv1 (dag-cbor, sha256) equal to the returned CID.
- **`proven`:** proven by `proofs/1`.

A third-party proof counts as current only if its rev is at least the head returned by the author PDS's `getLatestCommit`. Otherwise it is `[stale]` and is not a fold input. Hints (Jetstream, Constellation, relay, Slingshot, indexer JSON) are never fold inputs. Records of inactive repos are excluded (`sync.md`).

### 7.2 Generic rules (every collection)

| ID | Rule |
|---|---|
| G1 | `value.$type` equals the record's collection NSID (after `NS` prefix mapping). |
| G2 | The value passes lexicon validation:<br>• required fields present;<br>• JSON types;<br>• formats (`did`, `cid`, `datetime`, `at-uri`, `uri`, `tid`);<br>• `maxLength` and `maxGraphemes`;<br>• integer `minimum` and `maximum`;<br>• array `maxLength`;<br>• blob `accept` and `maxSize`, checked against the declared `mimeType` and `size`.<br>knownValues are **not** enforced; unknown values fall back per §13.3. Unknown fields are ignored. Known union members are validated; unknown union members are left alone. |
| G3 | Blobs use the typed form `{"$type":"blob","ref":{"$link":<cid>},"mimeType":…,"size":…}`. The legacy untyped form is invalid. |
| G4 | Every reference field, including every array entry, holds a canonical at-uri (§2.3) in an allowed collection (the table in §2.3). A reference into an NS collection whose rkey does not match that collection's grammar also fails G4. The *entry* rules in §7.3 only discard entries that pass G4 but are out of scope. |
| G5 | The rkey is canonical (§3.2), including every listed equality. |
| G6 | The record's DAG-CBOR encoding is at most 1,000,000 bytes (`limits.md`). |
| G7 | Every `md` value whose `$type` is `NS.markdown` satisfies body rules 1 and 2 (§5.2). |
| G8 | Every `slug`-typed value, including `#react.react` and `#tagVote.tag`, matches `^[a-z0-9]+(-[a-z0-9]+)*$`. |
| G9 | Collections with a `forum` field (every collection except `forum`, `profile` and `revision`): `fk(value.forum)` equals the key's fk segment. This is part of G5, listed for clarity. A revision has no `forum` field; its key range comes from its subject (RV3). |
| G10 | The value's nesting depth (`limits.md` §1: the root object is depth 1) is at most 32 (`NESTING_MAX_RECORD`). A deeper value that could still be decoded within the response limits (`limits.md` §6) is stored as an invalid version, so it replaces older versions like any other; a response too deep to decode is rejected whole and changes nothing (`sync.md` §5.5). |

### 7.3 Per-collection rules

**forum**

| ID | Rule |
|---|---|
| FO1 | The rkey matches `SLUG`. |
| FO2 | *entry.* A `moderators` entry is ignored if its `did` equals the repo DID (the owner), if its `role` is unknown, or if its `did` already appeared in an earlier entry (the first entry for a DID wins). An unknown `role` raises the banner of §13.3 ("this forum uses rules newer than your client"), in any entry, including one that is also ignored for another reason. |
| FO3 | *entry.* A `discovery.indexers` entry is ignored unless its `url` is `https:` with a hostname and no explicit port. |
| FO4 | *entry.* A `policy` kv field with an unknown value takes its default, with a banner (§13.3). An unknown `membership` or `defaultTier` likewise takes its fallback (§13.3: `approval`, `personal`) with the banner. `social.md` §1.6 lists the ten knownValue fields of the declaration that raise the banner. Integers outside their range fail G2, which invalidates the declaration. The forum then cannot be opened, which fails closed. |

**membership**

| ID | Rule |
|---|---|
| ME1 | `rkey = fk(forum)`. |

**post**

| ID | Rule |
|---|---|
| PO1 | The rkey is `{fk}-{tid}` with `fk = fk(forum)`. |
| PO2 | The effective kind is `kind` if known, else `article`. |
| PO3 | If the effective kind ≠ `quickTake`, `title` is present and contains a non-`White_Space` character. |
| PO4 | If the effective kind = `quickTake`, `body` is present. *entry:* `title` is ignored. |
| PO5 | If the effective kind = `link`, `link` is present. *entry:* for other kinds, `link` is ignored. |
| PO6 | If `body` uses `textBlob`, `excerpt` is present. *entry:* if it does not, `excerpt` is ignored. |
| PO7 | *entry.* `tags` has set semantics: duplicates collapse. |
| PO8 | *entry.* Only the `com.atproto.label.defs#selfLabels` member of `labels` is read. Unknown label values are ignored. |

**comment**

| ID | Rule |
|---|---|
| CO1 | The rkey is `{fk}-{rootTid}-{tid}` with `fk = fk(forum)`. |
| CO2 | `root.uri` is an `NS.post` uri whose rkey is `{fk}-{T}` with the same fk, and `rootTid = T`. |
| CO3 | `parent`, if present, is an `NS.comment` uri. Where the comment is placed is decided by the thread rules in `social.md`, not by validity. |
| CO4 | `body` is inline (G7). |
| CO5 *(x)* | If a **valid** record with the root uri is in the fold input, its `value.forum` equals `comment.forum`. An invalid root is not compared: the fold treats it as a `Missing` root, so its comments stay in the thread under the placeholder (`social.md` §11, §20.1 items 28 and 51). A valid root in F's key range names F (G9), so CO5 fails only on a 60-bit fk collision, like the other forum checks. |
| CO6 | *entry.* `moderatorHat` is honoured only if the author is a current authority of `forum`. |

**revision**

| ID | Rule |
|---|---|
| RV1 | `did(subject.uri)` = the repo DID. |
| RV2 | `collection(subject.uri)` ∈ {post, comment, forum, sequence, modAction}. |
| RV3 | `rkey = P(subject) + "-" + TID`. |
| RV4 | `op` is known. `op = delete` only if the subject is a `modAction`. |
| RV5 | If `record` is present: it is an object, `record.$type` = `collection(subject.uri)`, and the CIDv1 (dag-cbor, sha256) of `DAG-CBOR(record)` equals `subject.cid`. |

**opinion**

| ID | Rule |
|---|---|
| OP1 | `collection(subject.uri)` ∈ {post, comment, wikiRevision, sequence}. |
| OP2 | `rkey = fk(forum) + "-" + stid(subject) + "-" + h16("opinion", subject.uri)`, and `fkOf(rkey(subject.uri)) = fk(forum)`. |
| OP3 | *entry.* An unknown `karma` or `agreement` value makes that axis absent. An unknown `#tagVote.value`, `#react.stance`, or a `#react.react` outside the forum's palette discards that entry. |
| OP4 | *entry.* `tags` is ignored unless the subject is a post. Several entries for one tag collapse if their values are equal; otherwise all of them are ignored. |
| OP5 | Whether an opinion *counts* is decided by `social.md` (opinion validity): the subject exists in the input with the same forum, the voter is not the author and not banned, nullified or restricted, the subject is not spam, and the deleted-subject rule applies. |

**modAction**

| ID | Rule |
|---|---|
| MA1 | `subject` matches exactly one form: a `did` (§2.3 DID grammar), a canonical at-uri (G4 rules apply), or `"tag:" + SLUG`. |
| MA2 | The rkey matches `^FK-H16$` and equals `fk(forum) + "-" + h16("modAction", scope_eff, subject, actionKey(action))`. A key of the reserved form `^FK-TID$` is not invalid but is **ignored** (no effect) in v1. Such a record is checked against G1–G10 and MA10–MA12 only: MA1 and MA3–MA7 are not checked, because the P2 actions that will use the key form may take subjects and values that v1 does not know (the reason MA7 skips MA3–MA6). |
| MA3 | An at-uri subject in a forum-scoped collection (everything except `forum` and `profile`) has `fkOf(rkey(subject)) = fk(forum)`. A `forum` subject is checked by MA4 and MA9 only. |
| MA4 | The subject form and collection are allowed for the action (table in §6.7). |
| MA5 | Positive assertions (§6.7) carry `subjectCid`. |
| MA6 | If `scope_eff = author`, the action is allowed in author scope (table in §6.7). Whether the subject is in the actor's own thread or post, and the author gates, are checked by the fold (`social.md`). |
| MA7 | An unknown `scope` value, an unknown action `$type`, or an unknown `#access.level`, `#tier.tier` or `#pin.scope` makes the record valid but **without effect**. It is shown as "unknown action" in the mod log. When MA7 applies, MA3–MA6 are not checked (the subject may name any collection, since a future action may target one); G1–G10, MA1, MA2 and MA10–MA12 still apply. |
| MA8 | *entry.* Unknown `#restrict.capabilities` items are ignored, and duplicates collapse. A `#restrict` with no known capability and no `nullifyVotes: true` asserts the empty set and `nullify = false`: it restricts nothing itself, but like any assertion it competes in resolution, so at a higher rank it lifts lower-rank restrictions of the same DID (`social.md` §4.4, §4.7). |
| MA9 | `#lock` whose subject is an `NS.forum` uri: reserved (P2), valid but without effect. *entry:* `allowed` entries outside the subject post's thread (comment rkey not `{fk}-{T}-…` with `T = threadTid(subject)`) are ignored. The filter reads the key only: an entry that names a comment not in the fold input is kept (`social.md` §4.4). |
| MA10 | `#tag.head`, if present, is an `NS.wikiRevision` uri with `fkOf = fk(forum)`. Whether `head` names a revision of the same slug is checked by the fold. |
| MA11 | `adopts`, if present, is an `NS.modAction` uri (G4). It is informational only. |
| MA12 | `#tier.notice` is inline (G7). |

**wikiRevision**

| ID | Rule |
|---|---|
| WR1 | The rkey is `{fk}-{tid}` with `fk = fk(forum)`. |
| WR2 | Every `parents[].uri` is an `NS.wikiRevision` uri with `fkOf = fk(forum)`. *entry:* duplicate parents collapse. |

**sequence**

| ID | Rule |
|---|---|
| SQ1 | The rkey is `{fk}-{tid}` with `fk = fk(forum)`. |
| SQ2 | Every `#chapter.posts[]` entry is an `NS.post` uri (G4). *entry:* entries with `fkOf ≠ fk(forum)` are not displayed. |
| SQ3 | `#chapter.body` is inline (G7). *entry:* `cover` is ignored unless its `mimeType` is `image/*`. |

**profile**

| ID | Rule |
|---|---|
| PF1 | The rkey is `self`. |
| PF2 | `bio` is inline (G7). |

### 7.4 Normative fetch caps (fold-input limits)

Clients that truncated at different points would fold different inputs. So the cap is part of the data model:

```
for each (repo, collection, fk):
    N = 100000  if collection = opinion
      =  20000  otherwise
    input(repo, collection, fk) = the first N records of the "{fk}-" key range in DESCENDING rkey byte order
```

- Excess records are excluded identically by every client, and the view marks that repo `[partial]`.
- The cap applies to the ranged, forum-scoped collections: post, comment, revision, opinion, modAction, wikiRevision and sequence.
- `membership` is a single record. `forum` and `profile` are not ranged.
- Byte caps per range (50 MiB, or 100 MiB for `opinion`) and per response are safety limits in `limits.md` §5. Hitting one marks the repo `[partial]` and makes no completeness claim.
- A range with N or more keys is reported as capped (`sync.md` §5.4); the fold input is the same either way.

### 7.5 Ordering and determinism

- Validity never depends on the order in which records arrive.
- Rules marked *(x)* are re-evaluated whenever the referenced record enters or leaves the input.
- Tie-breaks between records are by uri bytes, then CID bytes (`social.md`).

---

## 8. Write protocol (outbox)

### 8.1 Writer rules

| ID | Rule |
|---|---|
| W1 | Every record and union member carries `$type`. |
| W2 | `validate` is never sent (left unset). |
| W3 | Read-modify-write preserves unknown fields, including fields in nested objects and unknown union members. |
| W4 | Key-determining fields are immutable. Writers MUST NOT change:<br>• `forum` (any collection);<br>• `comment.root`;<br>• `opinion.subject.uri`;<br>• `revision.subject`;<br>• `modAction.scope`, `.subject` and the action `$type`;<br>• `#pin.scope`;<br>• `wikiRevision.slug`.<br>Such a change needs a new record. Writers also never change `comment.parent`, although the format permits it. |
| W5 | `createdAt` is the writer's clock at creation. It never changes. `updatedAt` is set on every update. |
| W6 | A TID is generated from the current clock in microseconds plus a random 10-bit clock id. TIDs are strictly increasing per account. A write's rkeys, including the TIDs of any revisions it creates, are fixed when its `PendingWrite` is persisted, and are never regenerated. |
| W7 | Writers emit only canonical references (§2.3). Handles are resolved to DIDs before writing. |
| W8 | Derived fields: `markdown.mentions` and `.links` (§5.4); `excerpt` when spilled (§5.3); `#textQuote.prefix`, `.suffix` and `.start` from P of the version the user saw. |
| W9 | Writes go only to the session's own repo. A `PendingWrite` stamped with a different account DID is refused. |
| W10 | Reserved names and key forms (§9) are never written. |
| W11 | Size and spill: §5.3 and §8.9. Blobs: §8.8. |

### 8.2 Outbox model

```
PendingWrite { id, account: did, seq: i64, intent, ops: [Op], base: Option<cid>, state, attempts, createdAt,
               notBefore: Option<ms>,     // seq: FIFO position per account; notBefore: end of the hold (held)
                                          // or of the back-off (throttled)
               needsLogin: bool }         // throttled because the session was refused (below)
state ∈ queued | held | sending | done | conflict | failed(reason) | throttled
```

**Refused session.** A write request refused with `auth_required` (the PDS answers 401, or the host has no usable session: expired, revoked, logged out) is not an attempt and is not retried: the write becomes `throttled` with `needsLogin` and no `notBefore`, the pump sends nothing else of that account (FIFO), and `Auth { did, Expired }` is emitted (`engine-api.md` §3.5, §3.8). The account's next login queues every such write again.

**Durability**
- A `PendingWrite` is persisted in the `state` store before any network call.
- After a crash, a write found in `sending` is resolved first by reading its targets:
  - if each target's current CID equals the CID of the intended value, computed locally, the write is `done`;
  - otherwise it is re-sent through the operation's own protocol.

**Serialisation**
- Writes are processed per account in FIFO order, with **at most one write in flight per account**.
- Different accounts run independently.

**Swap head.** Every `swapCommit` is the `cid` from a `com.atproto.sync.getLatestCommit` call made immediately before the write. It is never taken from the cache or from an earlier write's result.

**Holding opinions.** Opinion intents are held 4 s (`held`) so they can be undone or coalesced. Intents on the same subject are applied in order, as one write.

**Rate limits**
- On 429, back off and honour `Retry-After` and `RateLimit-*`.
- Track the write budget: 5,000 points per hour and 35,000 points per day, at 3 per create, 2 per update and 1 per delete. Every op in an `applyWrites` counts.
- A write that would exceed the budget waits in `throttled`.

**Results.** On success, the client applies the results to the cache optimistically (`sync.md`). If `applyWrites` returns no `commit`, it calls `getLatestCommit`. It then marks its own repo dirty.

### 8.3 Edit protocol

This protocol applies to post, comment, sequence and forum updates, and to modAction updates.

```
edit(target, base, newValue, summary?):          // base = CID the user edited; newValue preserves unknown fields
  rk = P(target) + "-" + tid_fixed               // W6
  repeat up to 3 times:
    head = getLatestCommit(self)                 // fresh
    cur  = getRecord(target)                     // fresh, own PDS
    if cur.cid = cid(newValue): return Done      // already applied (lost response)
    if cur.cid ≠ base:          return Conflict(cur)   // shown side by side
    rev = { $type: NS.revision, subject: {uri: target, cid: cur.cid}, op: "update",
            record: cur.value, summary?, createdAt: now }
    r = applyWrites(self, [ #create{NS.revision, rk, rev}, #update{coll(target), rkey(target), newValue} ],
                    swapCommit = head.cid)
    if ok(r): return Done(r)
    if r = InvalidSwap: sleep(uniform(200 ms, 800 ms)); continue
    else: handle per §8.2 (backoff on 429/5xx; otherwise failed)
  return Failed(TooManySwaps)
```

**`--no-history`** (posts, comments, sequences and the forum declaration only): `putRecord(target, newValue, swapRecord = base)`. A swap failure is a conflict.

**Retract** is an edit with `retracted: true`.

### 8.4 Operations

| Operation | Protocol |
|---|---|
| Create forum | `applyWrites[#create forum (rkey slug), #create membership (rkey fk)]`. The owner's own membership SHOULD be included. Fails if the slug exists (an HTTP 500 is ambiguous: re-read the declaration, below), and the CLI then asks for another slug. Writes the values in §6.1. |
| Edit forum (including `mods add`) | Edit protocol, with `P = fk(forum)`. |
| Remove a moderator (`mods rm`) | §8.6. |
| Join | Create-if-absent of `NS.membership` with rkey `fk` (below). If it already exists, re-read; an existing record means success. |
| Leave | `deleteRecord(NS.membership, fk, swapRecord = cid)`. |
| First content in a forum | `applyWrites[#create membership (only if a fresh getRecord shows it absent), #create post or comment]` with `swapCommit` = fresh head. If the batch fails (an HTTP 500 is ambiguous), re-read the membership and the head, and retry once without the membership if it now exists. |
| Create post, comment, sequence or wikiRevision | `createRecord(rkey)` with the rkey fixed per W6. The MVP wiki writes `parents` = [the current head] or [] (§6.8). |
| Edit content | Edit protocol (§8.3). |
| Delete content (post, comment, sequence, forum) | Delete protocol (§8.7): removes the target and purges its revisions. |
| Delete wikiRevision | `deleteRecord`. A wikiRevision has no revisions. |
| Opinion | §8.5. |
| Create modAction | Create-if-absent (below). If it already exists, re-read and continue as *update*. |
| Update modAction | Edit protocol (`op: update`). |
| Remove modAction (unhide, unban, unpin, unlock, unrestrict, …) | `applyWrites[#create revision{subject: {uri, cid: cur.cid}, op: "delete", record: cur.value}, #delete modAction]` with `swapCommit` = fresh head, retried as in §8.3. |
| `mod adopt` | §8.6. |
| Profile | No record: create-if-absent of `self`. Otherwise `putRecord(self, swapRecord = cid)`. Profiles have no revisions. |
| Redact history | `applyWrites[#delete revision …]`, chunked at ≤ 200 ops. |
| Blob | §8.8. |

**PDS semantics (ADR 0005, spike S12, reference PDS).**
- **Create-if-absent** is `putRecord{rkey, record, swapRecord: null}`. It succeeds only if the key is absent; if the key exists it fails with the typed `400 InvalidSwap`, which means "exists". A host that answers a null `swapRecord` with any other 400 may not support it: the client falls back to `createRecord(rkey)` for that write, whose failure on an existing key is a generic 500 that is resolved by `getRecord`. The host is remembered as not supporting null swaps (per host, `sync.md` §7.2) only when that `createRecord` then succeeds with the same value, and only after a 400 whose error name could mean "unsupported": a 400 named for media, size, auth, account status or rate limits (`InvalidMimeType`, `BlobTooLarge`, `PayloadTooLarge`, `AuthRequired`, `AuthMissing`, `ExpiredToken`, `InvalidToken`, `AccountTakedown`, `AccountDeactivated`, `RateLimitExceeded`) is handled as itself and never triggers the fallback. A successful null-swap put records the host as supporting it.
- `putRecord` and `deleteRecord` with `swapRecord = cid` give compare-and-swap; a mismatch is `400 InvalidSwap`.
- `InvalidSwap` (on `swapRecord` or `swapCommit`) is the only conflict signal. Its message format is unspecified and is never relied on.
- `applyWrites` is atomic and strict: a create on an existing key, or an update or delete on an absent key, fails the whole batch with HTTP 500 and writes nothing. Every HTTP 500 on a write is **ambiguous**: the client re-reads the targets (and the head) before deciding, and never resends blindly.
- `deleteRecord` of an absent key succeeds (`200 {}`). `putRecord` of an identical value returns no `commit` and does not advance the head; writers skip no-op updates by comparing CIDs before sending.

### 8.5 Opinion writes

```
opinion_write(subject, intents):                 // after the 4 s hold; intents coalesced in order
  key = fk(forum) + "-" + stid(subject) + "-" + h16("opinion", subject.uri)
  repeat up to 3 times:
    cur = getRecord(NS.opinion, key)             // fresh; may be absent
    new = apply(cur?.value, intents)             // pure function, below
    if equalIgnoringUpdatedAt(new, cur?.value): return Done
    if cur absent:      if empty(new): return Done; r = putRecord(key, new, swapRecord = null)   // create-if-absent (§8.4)
    elif empty(new):    r = deleteRecord(key, swapRecord = cur.cid)
    else:               r = putRecord(key, new, swapRecord = cur.cid)
    if ok(r): return Done
    if r is InvalidSwap (or, on a createRecord-fallback host, a 500 after which getRecord shows the key):
                        sleep(uniform(200 ms, 800 ms)); continue
  return Failed(TooManySwaps)

apply(v, intents):
  start from v (or {$type, subject, forum, createdAt: now}); keep unknown fields
  setKarma(x|clear), setAgreement(x|clear): set or remove that axis
  react(add, r, quote?, stance): replace or insert the entry with key (r, qkey(quote))
  react(remove, r, quote?):      remove the entry with key (r, qkey(quote))
  tag(set, slug, value | clear): replace, insert or remove the entry with that slug
  (a replaced entry keeps its array position; an inserted entry is appended; removal keeps the order of the rest)
  subject.cid = the subject's current CID as seen by the user; updatedAt = now if v existed
empty(v) ⇔ v has no field other than $type, subject, forum, createdAt, updatedAt,
           where an empty reacts or tags array counts as absent
```

### 8.6 Moderation writes

**`mod adopt <former>`.** Re-issues a former (or departing) moderator's forum-scope assertions under the acting authority.

```
mod_adopt(forum, former):
  originals = valid records (§7) in former's NS.modAction "{fk}-" range (fresh, complete listing)
              with scope_eff = forum, not lapsed (until > now)
  own       = fresh, complete listing of own NS.modAction "{fk}-" range
  for o in originals, rkey ascending:
     if the actor's role may not issue o.action (social.md table): report "cannot adopt"; continue
     v = copy of o.value (unknown fields kept) with adopts = {uri: o.uri, cid: o.cid}, createdAt = now, no updatedAt
     k = o.rkey                                  // same (scope, subject, actionKey) ⇒ same key
     mine = own[k]
     if mine absent:                 ops += [#create(k, v)]
     elif mine.adopts.uri = o.uri:   if mine ≠ v ignoring timestamps:
                                        ops += [#create revision{op: update, subject: mine, record: mine.value},
                                                #update(k, v with createdAt = mine.createdAt, updatedAt = now)]
     else:                           report "kept own assertion"; continue
  send ops in applyWrites batches of ≤ 200 ops (a revision and its update are never split),
  each with swapCommit = fresh head; on InvalidSwap or HTTP 500 re-list own range, recompute the remaining batches (≤ 3 tries)
```

**`mods rm <did>`** is run by the owner, since the declaration is in the owner's repo.
1. Run `mod adopt <did>` as the owner.
2. If, and only if, step 1 completed (reported skips are allowed), edit the declaration to remove the entry (edit protocol).

The CLI enforces this order. It MAY offer an explicit opt-out (for example for a rogue moderator) only after a confirmation that lists every assertion that will lapse.

**Lock.** Before writing `#lock{locked: true, allowed}` on a post:
1. The client computes `allowed` as the uris of the comments whose visibility in its fold view of the thread is `Visible` or `Collapsed` (`social.md`), sorted ascending by uri bytes.
2. If there are more than 5,000, the lock is **refused**, and the client tells the moderator to use hide instead.

### 8.7 Delete protocol (content)

```
delete(target):                                  // post | comment | sequence | forum
  revs = fresh listing of own NS.revision records with rkey matching exactly P(target) + "-" + TID
         and value.subject.uri = target           // the prefix alone is not enough (§3.2)
  chunk 1  = [#delete target] ++ first 199 of revs
  chunk i  = the next 200 of revs
  send the chunks in order with applyWrites (no swapCommit); on failure re-list, recompute the remaining chunks, retry with backoff
```

**Notes**
- The target is always deleted in the first chunk. A partial failure therefore leaves only orphaned revisions, and the persisted `PendingWrite` keeps retrying them.
- Deleting a forum declaration does not delete other repos' content. Clients treat a forum with no declaration as unavailable.
- ModActions are never deleted with this protocol (§8.4).

### 8.8 Blobs and MIME normalisation

**Upload**
- Blobs are uploaded only at publish time, after the media pipeline (strip, orient, downscale; `sync.md` and `threat-model.md`).
- The record embeds the blob object returned by `uploadBlob` **verbatim**. The locally computed CID is for preview only and MUST equal the returned `ref.$link`; if it does not, the write fails.
- On 413, the client downscales (images) or refuses.
- A blob that is not referenced is garbage-collected by the PDS after about an hour. So the write that references a blob MUST follow its upload within the same `PendingWrite`.

**MIME normalisation.** The declared `mimeType` sent to `uploadBlob` is normalised from the **sniffed** type, so that it falls within the requested blob scopes (§10.3):

| Sniffed content | Declared `mimeType` | Covered by |
|---|---|---|
| JPEG / PNG / WebP / GIF | `image/jpeg` / `image/png` / `image/webp` / `image/gif` | `blob:image/*` |
| ISO-BMFF (MP4) with major brand `M4A ` or `M4B ` (`.m4a`, also sniffed as `audio/x-m4a`) | `audio/mp4` | `blob:audio/*` |
| ISO-BMFF (MP4), any other brand | `video/mp4` | `blob:video/*` |
| WebM / Matroska | `video/webm` | `blob:video/*` |
| MP3 (ID3 or MPEG audio frame sync) | `audio/mpeg` | `blob:audio/*` |
| Ogg (`OggS`; also sniffed as `application/ogg`) | `audio/ogg` | `blob:audio/*` |
| PDF | `application/pdf` | `blob:application/pdf` |
| Spilled LFM text | `text/markdown` | `blob:text/*` |
| anything else | refused | – |

Readers compare declared and sniffed types through the same table (`sync.md`).

### 8.9 Size check

```
body_bytes = the exact UTF-8 JSON request body of the createRecord / putRecord / applyWrites call
1. spill every spillable md field whose source > 65536 bytes (§5.3)
2. while body_bytes > 900000 and an inline spillable field remains in the new value: spill the largest; recompute
3. if body_bytes > 900000: refuse with TooLarge (for an edit, offer --no-history)
```

---

## 9. Reserved names

These are not written or published until they are final. v1 readers ignore them.

| Name | Kind | Phase | Purpose |
|---|---|---|---|
| `report` | collection | P2 | sealed report: age v1 with X25519 anonymous recipients and padding, `ciphertext` ≤ 16 KiB, **no** forum, subject or recipient fields |
| `coauthor` | collection | P2 | CID-bound co-author acceptance (`karma/2`) |
| `subscription` | collection | P2 | public subscriptions |
| `turn` | collection | P8+ | dialogue message |
| `pollVote` | collection | P8+ | poll vote |
| `collection` | collection | P8+ | books |
| `karmaSnapshot` | collection | P7 | karma checkpoint |
| `observation` | collection | later | authority first-seen batches |
| `index`, `temp` | group names | – | never record names (§1.5) |
| `modAction` keys `{fk}-{tid}` | key form | P2 | list actions `#banList`, `#raid`, `#banHost` |
| `#lock` with a forum-uri subject | subject form | P2 | forum lock (read-only forum); cut from v1 |
| `#access.keep` | field | P2 | keep snapshots for bans |
| `#tag.aliasOf` | field | P2 | tag aliasing; cut from v1 |
| `post.coauthors` | field | P2 | co-authors |
| `comment.answer` | field | P2 | Q&A answers and accepted answers |
| `profile.moderationGuidelines` | field | P2 | author moderation style and guidelines |
| `forum.successor`, `forum.aliases` | field | P2 | forum succession |
| `forum.sensitive` | field | P2 | sensitive-forum flag |
| `forum.policy.labelers` | field | P2 | labelers |
| `post.event`, `post.poll` | field | P8+ | events (community calendar, same rkey) and polls |
| `ratelimit/1`, `karma/2`, `magic/2` | behaviour IDs | P2 | advisory rate limits; co-author credit; personalised magic |

**Explicit deferrals with no reserved name.** These can be added later as optional fields or new collections (§13.1). Their phases are set by `docs/plan.md` §6.5:
- recommendations (P8+);
- shareable drafts and comments on drafts (P8+);
- authorOnly tags (P2, with `karma/2`);
- the 10-karma removal of `nofollow` (Never: `rel` is fixed by the `lfm/1` HTML contract);
- `lockedToAccountsCreatedAfter` and `repliesBlockedUntil` (P2, with `ratelimit/1`);
- karma from votes on tag revisions (P2, with `karma/2`).

---

## 10. Permission sets, scopes and OAuth client metadata

### 10.1 `authFull`

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

### 10.2 `authInteract`

This set is meant for lightweight third-party clients. lessdugin's own clients request `authFull`.

```json
{"lexicon":1,"id":"online.7778777.lessdugin.authInteract","defs":{"main":{"type":"permission-set",
 "title":"lessdugin (interact)",
 "detail":"Join lessdugin forums, write, edit and delete your comments, and vote and react.",
 "permissions":[{"type":"permission","resource":"repo","collection":[
  "online.7778777.lessdugin.membership","online.7778777.lessdugin.comment",
  "online.7778777.lessdugin.revision","online.7778777.lessdugin.opinion"]}]}}}
```

**Rules for both sets**
- The JSON above omits the `description` strings that every lexicon document and def carries (§2.1); the files in `lexicons/` are otherwise identical to it.
- Permission sets cannot contain blob permissions, so blob scopes are requested directly (§10.3).
- A set may cover only its own group and its child groups.
- Authorization servers re-resolve sets on token refresh, so collections added later reach existing sessions.

### 10.3 Scope strings

| Use | Exact scope string |
|---|---|
| **Default** (all clients, release builds) | `atproto include:online.7778777.lessdugin.authFull blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf` |
| **Fallback** (chosen at PAR on `invalid_scope`, before the browser opens), the devnet, and every `.temp` build (with the `.temp` prefix) | `atproto repo?collection=online.7778777.lessdugin.forum&collection=online.7778777.lessdugin.membership&collection=online.7778777.lessdugin.post&collection=online.7778777.lessdugin.comment&collection=online.7778777.lessdugin.revision&collection=online.7778777.lessdugin.opinion&collection=online.7778777.lessdugin.modAction&collection=online.7778777.lessdugin.wikiRevision&collection=online.7778777.lessdugin.sequence&collection=online.7778777.lessdugin.profile blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf` |
| **Never** | `transition:generic`; `blob:*/*` |
| **P2 interop** (requested incrementally, only after the feature ships) | `repo:community.lexicon.calendar.event?action=create …`, `repo:site.standard.document …` |

**Rules**
- Every variant begins with `atproto`.
- Audio is covered by `blob:audio/*` only because MIME types are normalised (§8.8).
- Before writing to any non-NS collection, clients check the granted `scope` in the token response.
- Spike S8 records how an unresolvable `include:` fails at PAR, and S4 checks that the multi-collection `repo?collection=` form is accepted.

### 10.4 Client metadata

`SUPERSET` is the scope string in both files, with the `.temp` substitutions of §1.3 for development builds:

```
atproto include:online.7778777.lessdugin.authFull repo?collection=online.7778777.lessdugin.forum&collection=online.7778777.lessdugin.membership&collection=online.7778777.lessdugin.post&collection=online.7778777.lessdugin.comment&collection=online.7778777.lessdugin.revision&collection=online.7778777.lessdugin.opinion&collection=online.7778777.lessdugin.modAction&collection=online.7778777.lessdugin.wikiRevision&collection=online.7778777.lessdugin.sequence&collection=online.7778777.lessdugin.profile blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf
```

A `.temp` build's metadata omits the `include:` token, because `.temp` publishes no permission set.

**Web client** (`deploy/site/oauth/client-metadata.json`, served at `https://lessdugin.7778777.online/oauth/client-metadata.json`):

```json
{
  "client_id": "https://lessdugin.7778777.online/oauth/client-metadata.json",
  "client_name": "lessdugin",
  "client_uri": "https://lessdugin.7778777.online/",
  "application_type": "web",
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "redirect_uris": ["https://lessdugin.7778777.online/"],
  "scope": "atproto include:online.7778777.lessdugin.authFull repo?collection=online.7778777.lessdugin.forum&collection=online.7778777.lessdugin.membership&collection=online.7778777.lessdugin.post&collection=online.7778777.lessdugin.comment&collection=online.7778777.lessdugin.revision&collection=online.7778777.lessdugin.opinion&collection=online.7778777.lessdugin.modAction&collection=online.7778777.lessdugin.wikiRevision&collection=online.7778777.lessdugin.sequence&collection=online.7778777.lessdugin.profile blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf",
  "token_endpoint_auth_method": "none",
  "dpop_bound_access_tokens": true
}
```

**Native client, hosted metadata** (`deploy/site/oauth/cli-client-metadata.json`, served at `https://lessdugin.7778777.online/oauth/cli-client-metadata.json`):

```json
{
  "client_id": "https://lessdugin.7778777.online/oauth/cli-client-metadata.json",
  "client_name": "lessdugin CLI",
  "client_uri": "https://lessdugin.7778777.online/",
  "application_type": "native",
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "redirect_uris": ["http://127.0.0.1/callback"],
  "scope": "atproto include:online.7778777.lessdugin.authFull repo?collection=online.7778777.lessdugin.forum&collection=online.7778777.lessdugin.membership&collection=online.7778777.lessdugin.post&collection=online.7778777.lessdugin.comment&collection=online.7778777.lessdugin.revision&collection=online.7778777.lessdugin.opinion&collection=online.7778777.lessdugin.modAction&collection=online.7778777.lessdugin.wikiRevision&collection=online.7778777.lessdugin.sequence&collection=online.7778777.lessdugin.profile blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf",
  "token_endpoint_auth_method": "none",
  "dpop_bound_access_tokens": true
}
```

The port of a loopback redirect is ignored when matching. The CLI listens on an ephemeral port at `http://127.0.0.1:<port>/callback`.

**Native loopback client** (the default; no metadata document):
- `client_id` = `http://localhost?redirect_uri=` + pct(`http://127.0.0.1:<port>/callback`) + `&scope=` + pct(SUPERSET).
- `pct` is percent-encoding (RFC 3986, unreserved characters kept).
- The `client_id` has no port and an empty path.
- The hosted native metadata is used automatically after `invalid_client`, or with `--oauth-client hosted`.

**Self-hosting.** `xtask site --origin <url>` regenerates both files, substituting the origin in `client_id`, `client_uri` and `redirect_uris`. `client_uri` MUST have the same host as `client_id`.

### 10.5 Callback and login binding (normative here; the identity details are in `sync.md`)

**Callback**
- Every lessdugin client sends `response_mode=query` in its authorization request.
- Callback handlers accept both `?query` and `#fragment` responses.
- The SPA handles the callback **before** its hash router mounts. It then removes the parameters with `history.replaceState`.

**Login binding** (both checks MUST pass before a session is used):
1. Take the PDS from the DID document of the token's `sub`.
2. Fetch `<pds>/.well-known/oauth-protected-resource`.
3. Require its `authorization_servers` to contain the token's `iss`.
4. The authorization server is shown at first login and pinned (trust on first use).

---

## 11. Indexer query lexicons (`NS.index.*`)

### 11.1 Common rules

**Transport**
- Every method is a `query`: an unauthenticated `GET /xrpc/online.7778777.lessdugin.index.<method>`, with `Access-Control-Allow-Origin: *` and no custom request headers.
- Errors use XRPC form, `{"error": <name>, "message": …}`.
  - Every method may return `InvalidRequest` and HTTP 429 `RateLimitExceeded`. These are generic XRPC errors and are not declared in the lexicons.
  - Methods that take `forum` or `forums`, and `getThread` (for the post's forum, or a post the indexer does not know), may return `NotIndexed`. The lexicons declare it, and `CursorTooOld` for `getChanges`.
- Lists are `Cache-Control: max-age=5` with ETags. Record data is never `immutable`.

**Request parameters**
- Array parameters hold at most 25 at-uris or at most 50 DIDs.
- Actor parameters are DIDs only. Clients resolve handles locally.
- `cursor` is an opaque string of at most 512 bytes, except in `getChanges`.

**Response metadata.** Every JSON response carries:
- `asOf: #asOf`;
- `indexer: #indexerInfo`.

`getRecordProofs` returns a CAR and carries neither; each proof is self-describing through its commit `rev`.

**Trust.** Everything except proofs is a **hint**. Clients:
- use hints for discovery, ordering and placeholders;
- recompute displayed values locally;
- never display hint text attributed to a DID.

**Phases**
- **First pass (M6):** `describeIndexer`, `listForums`, `listParticipants`, `getRepoHeads`, `getChanges`, `listPosts`, `listRecentDiscussion`, `getThread`, `getRecordProofs`.
- **Later (Reserved (P2), `docs/plan.md` §6.1):** `searchPosts`, `listBacklinks`, `getActivity`, `getKarma`, `getTallies`, `getRecords`. Their lexicons are drafted now so that their shapes are reviewed with the rest; a v1 indexer does not serve them. "later" in the table below means this phase.

An indexer advertises the methods it serves in `describeIndexer.features`. Clients MUST call only advertised methods.

### 11.2 `index.defs`

```
#asOf        { seq!: count, timeUs!: count, lagMs!: count }
#indexerInfo { did!: did, coreVersion!: s(64b) }
#recordRef   { uri!: atu, cid!: cid, firstSeen: datetime, value: unknown /* HINT; present only when hydrate=true */ }
#tally       { subject!: atu, policy!: s(64b), score!: sint, agreement: sint, unweighted: sint, reacts: [#reactTally]≤64 }
#reactTally  { react!: slug, for!: count, against: count, quote: defs#textQuote }
#postView    { ref!: #recordRef, tally: #tally, state: kv{visible, hidden, pending, collapsed},
               tier: kv{personal, frontpage, curated}, tags: [slug]≤32, comments: count, lastCommentAt: datetime,
               effectiveAt!: datetime, rank: s(64b) }
#commentView { ref!: #recordRef, parent: atu, depth: int[0..1000], tally: #tally, state: kv{visible, hidden, pending, collapsed} }
#participant { did!: did, status!: kv{owner, admin, moderator, member, established, newcomer}, rev: tid, lastSeen: datetime }
#change      { seq!: count, uri!: atu, cid: cid, did!: did, rev!: tid, op!: kv{create, update, delete} }
#activity    { reason!: kv{reply, mention, quote, react, vote, modAction, newPost, newComment, pingback},
               ref!: #recordRef, subject: atu }
```

**Field notes**
- `#tally.policy` holds the behaviour IDs used, for example `"karma/1"`.
- `#postView.effectiveAt` is `t_rank` (`social.md`).
- `#postView.rank` is present only for `sort=magic`. It is the `hot` value printed as the shortest round-trip decimal of the binary64 value without an exponent (Rust `Display` for `f64`). It is informative only.
- An unknown kv value in any index response means the client ignores that entry.

### 11.3 Methods

| Method | Phase | Parameters | Output |
|---|---|---|---|
| `describeIndexer` | first | – | `{did!, version!: s(64b), coreVersion!: s(64b), policies!: [s(64b)]≤64, features!: [s(64b)]≤64 (method names), forums!: [#forumStatus]≤1000 (by uri; use listForums for more)}` |
| `listForums` | first | `q?: s(512b)`, `limit?: int[1..100] =50`, `cursor?` | `{forums!: [#forumSummary]≤100, cursor?}` |
| `listParticipants` | first | `forum!: atu`, `status?: kv{owner, admin, moderator, member, established, newcomer}`, `limit?: int[1..1000] =500`, `cursor?` | `{participants!: [#participant]≤1000, cursor?}` |
| `getRepoHeads` | first | `dids!: [did]≤50` | `{heads!: [#repoHead]≤50}` |
| `getChanges` | first | `forum?: atu`, `since!: count` (exclusive), `limit?: int[1..1000] =500` | `{changes!: [#change]≤1000 (seq ascending), cursor!: count}`; `cursor` is the last returned seq, or `since` when `changes` is empty, and is the next `since`; error `CursorTooOld` when `since` is older than the retained log (30 days) |
| `listPosts` | first | `forum!: atu`, `sort?: kv{magic, top, new, old, recentComments} =magic`, `tier?: kv{personal, frontpage, curated}` (minimum tier; absent = all visible), `tag?: slug`, `author?: did`, `kind?: kv{article, link, question, quickTake}`, `hydrate?: bool`, `limit?: int[1..100] =25`, `cursor?` | `{posts!: [#postView]≤100, cursor?, policy!: #policyIds, inputDigest!: s(64b) (hex sha256)}` |
| `listRecentDiscussion` | first | `forum!: atu`, `limit?: int[1..50] =20`, `cursor?` | `{items!: [#discussionItem]≤50, cursor?}` |
| `getThread` | first | `post!: atu`, `sort?: kv{magic, top, new, old, latestReply} =magic`, `hydrate?: bool` | `{post!: #postView, comments!: [#commentView]≤10000 (pre-order), truncated!: bool, inputDigest!: s(64b)}` |
| `getRecordProofs` | first | `uris!: [atu]≤25` | `application/vnd.ipld.car`: root = DAG-CBOR manifest `{v: 1, items: [{uri, cid \| null, commit}]}`, then deduplicated blocks (`proofs/1`, `sync.md`); size per `limits.md` |
| `searchPosts` | later | `q!: s(512b)` (≤ 16 terms), `forum?: atu`, `author?: did`, `limit?: int[1..50] =25`, `cursor?` | `{hits!: [#hit]≤50, cursor?}` |
| `listBacklinks` | later | `subject!: atu`, `collection!: nsid`, `path?: s(256b)`, `limit?: int[1..100] =50`, `cursor?` | `{records!: [#recordRef]≤100, cursor?}` |
| `getActivity` | later | `actor!: did`, `since?: datetime`, `forums?: [atu]≤25`, `limit?: int[1..100] =50`, `cursor?` | `{activity!: [#activity]≤100, cursor?}`; public data only; rate-limited per actor |
| `getKarma` | later | `forum!: atu`, `actors!: [did]≤50` | `{karma!: [#karmaEntry]≤50, policy!: s(64b), inputDigest!: s(64b)}` |
| `getTallies` | later | `subjects!: [atu]≤25` | `{tallies!: [#tally]≤25}` |
| `getRecords` | later | `uris!: [atu]≤25` | `{records!: [#recordRef with value]≤25}` (hints) |

`cursor` is `s(512b)` everywhere except `getChanges`. Every JSON output also has `asOf!` and `indexer!` (§11.1). Every top-level array of an output is required and may be empty. In this section, `#recordRef`, `#postView`, `#commentView`, `#participant`, `#change`, `#activity` and `#tally` name `index.defs` defs.

**Method-local defs** (each in its method's lexicon, for example `NS.index.getRepoHeads#repoHead`):

```
describeIndexer#forumStatus       { uri!: atu, lagMs: count, backfilled: bool }
listForums#forumSummary           { ref!: #recordRef, participants: count, posts: count }
getRepoHeads#repoHead             { did!: did, rev: tid, commit: cid, pds: url, checkedAt: datetime,
                                    status: kv{active, takendown, suspended, deleted, deactivated, desynchronized, throttled} }
listPosts#policyIds               { karma!: s(64b), ranking!: s(64b) }
listRecentDiscussion#discussionItem { post!: #postView, comments: [#commentView]≤3 }
searchPosts#hit                   { ref!: #recordRef, snippet: s(1000b) /* plain text */, highlights: [#highlight]≤16 }
searchPosts#highlight             { start!: int[0..1000], end!: int[0..1000] }   // half-open, code points in snippet
getKarma#karmaEntry               { did!: did, k1!: sint, kp!: sint, k!: sint }
```

**Semantics**
- `getRecordProofs` items are verified independently. A failed item is dropped and counted against the source. A `cid: null` item proves absence.
- `listPosts.tier` maps to the list filters as follows: `frontpage` gives tier ≥ frontpage (the SPA default), `curated` gives tier = curated, and `personal` or absent gives All.
- `inputDigest` is defined in `social.md` (fold contract). Clients compare it in `--audit`.
- `searchPosts` is backed by SQLite fts5 inside the indexer only. Every client (CLI, TUI, SPA) searches locally with the core's in-memory index, and uses `searchPosts` only for remote or global search (`--remote`), whose hits are hints.

### 11.4 Indexer discovery

- An indexer is a `did:web` whose DID document has the service `#lessdugin_index`, of type `LessduginIndex`, with an `https` `serviceEndpoint`.
- Clients also learn of indexers from `forum.discovery.indexers` (after one-time consent) and from user configuration.

---

## 12. Interop decisions

| Lexicon | Decision |
|---|---|
| `com.atproto.repo.strongRef`, `com.atproto.label.defs#selfLabels` | Reused. |
| `app.bsky.actor.profile` | Read-only fallback for display name and avatar, read from the PDS, never the AppView (§6.10). |
| `app.bsky.graph.block` | Optional local lens: one's own blocks may collapse content (`social.md`). Never written. |
| `site.standard.publication` / `.document` | No default dual-publishing. P2: opt-in syndication of one's own posts, same rkey, with `content` = our `NS.markdown` object. |
| `at.markpub.markdown` | Not adopted; our `markdown` is deliberately similar. Readers may add it to the `md` union later (non-breaking). |
| `community.lexicon.calendar.event` / `.rsvp`, `community.lexicon.location.*` | Events, dual-published with the same rkey: P8+. |
| `community.lexicon.bookmarks.bookmark` | P2: opt-in public bookmarks. Bookmarks are local by default. |
| `community.lexicon.interaction.like` | Not used (it would double-count votes). |
| Tangled, Frontpage, atbbs, atBB | Shape inspiration only; a possible importer later. |
| Atproto labelers | P2: `forum.policy.labelers` (reserved). |
| Private messages | Out of scope; we link to Bluesky chat. |

---

## 13. Evolution rules and unknown-value fallbacks

### 13.1 Allowed changes

- new optional fields;
- new knownValues;
- new refs in open unions;
- new defs;
- new collections;
- new indexer methods;
- new optional query parameters and output fields.

### 13.2 Frozen and breaking changes

**Frozen at first publication:**
- types;
- field names;
- required sets;
- every length, grapheme, array, integer and blob limit;
- accept lists;
- rkey types and the rkey grammars of §3;
- the core validity rules of §7 (they are part of `sk/1`, `fk/1` and the record semantics).

**Breaking changes** get a new NSID, `<name>V2`, for example `NS.postV2`.
- Clients read both versions.
- Writers switch after a transition period.
- Nobody ever dual-writes.

**Behaviour IDs**
- They never change meaning. A changed behaviour gets a new ID (`karma/2`), and the old one stays implementable from its vectors.
- Forums select behaviours through `policy` IDs.

**`.temp`** has no compatibility promise (§1.3).

### 13.3 Fallbacks for unknown values

| Unknown | Fallback |
|---|---|
| `post.kind` | `article`, so `title` is required (PO2, PO3) |
| `#moderator.role` | the entry is ignored: no authority, rank or capability |
| `forum.membership` | `approval` |
| `forum.defaultTier` | `personal` |
| `policy.moderation`, `karma.algorithm`, `ranking.algorithm`, `reactPalette` | the default ID, plus a banner: "this forum uses rules newer than your client" |
| `policy.voting`, `policy.tagCreation`, `display.newUsers` | that field's default, plus the same banner |
| `opinion.karma`, `opinion.agreement` | that axis is ignored |
| `#tagVote.value`, `#react.stance` | that entry is ignored |
| react id outside the palette | that entry is ignored |
| `modAction.scope`, the action `$type`, `#access.level`, `#tier.tier`, `#pin.scope` | the record has no effect; shown as "unknown action" (key = `$type` for an unknown action) |
| a `#restrict.capabilities` item | that item is ignored |
| `revision.op` | the revision is invalid (raw view only) |
| `markdown.profile` | rendered as `lfm/1` with a badge |
| an `md` union member | not rendered; "unsupported body format" |
| a `labels` union member, a self-label value | ignored |
| unknown fields anywhere | ignored on read; preserved on read-modify-write (W3) |
| kv values in indexer responses | that entry is ignored |

---

## 14. Decisions made while writing

These choices fill gaps or resolve ambiguities in the sources. Each takes the simplest safe option. They are open to review before M7.

1. **Agreement values.** The plan's table says "`agreement` (same four values)". This document keeps the synthesis names `agree | disagree | strongAgree | strongDisagree`, read as the same four *strengths*. They match the `karma/1` power function, the CLI verbs and ForumMagnum. The lexicon author must use the same names.
2. **ModAction field names.** The plan's abbreviations are expanded to the synthesis names: `#restrict.capabilities` (not `caps`), `#access.level`, `#pin.scope` with `pinned`, `#lock.locked` with `allowed`.
3. **`#tag.aliasOf`** is not in the v1 lexicon (cut from the first pass, per the plan's table). It is reserved for P2.
4. **`revision.record`** is optional in the lexicon. Writers always include it; a revision without it means "snapshot not available". If present, its CID MUST equal `subject.cid` (RV5). A mismatch makes the revision invalid rather than "unverified". Revisions of `profile` are not supported, following the synthesis union.
5. **Revision keys of a forum** use `P = fk(forumUri)`, so every revision sits in the forum's `{fk}-` range. Because keys nest, selection by prefix MUST also check the exact grammar and `subject.uri` (§3.2, §8.7).
6. **quickTake requires a `body`** (PO4). This prevents empty items. A "required" title must contain a non-space character (PO3). A `link` on a non-link post and an `excerpt` on an unspilled body are ignored.
7. **Reference strictness.** A non-canonical at-uri, or one in a disallowed collection, in any reference field invalidates the whole record (G4). That includes array entries in `#chapter.posts`, `profile.pinned` and `markdown.links`, which follows the synthesis rule "a reference that is not canonical makes the record invalid". Only did:plc and did:web (no port or path) are accepted.
8. **Entry-level tolerance** is used where the sources speak per axis or per entry: unknown axis and react values, tags on non-post subjects, conflicting duplicate `#tagVote`s (all dropped, like react contradictions), unknown restrict capabilities, `#lock.allowed` entries outside the thread.
9. **Forum `moderators`.** Duplicate DIDs: the first entry wins. Entries for the owner and entries with unknown roles are ignored (no authority, so they are not seeds either).
10. **`#pin` subjects.** Scope `forum` takes posts or sequences; scope `thread` takes comments. **Author-scope `#hide`** ignores `spam` and cannot keep (`hidden=false`).
11. **`mod adopt`** never overwrites the adopter's own unrelated assertion at the same key; it reports and skips it. It updates (with a revision) only records that already adopt the same original. It skips actions the adopter's role cannot issue. **`mods rm`** may be opted out of adoption, after a confirmation that lists what will lapse; this is for rogue moderators.
12. **`forum create`** writes the owner's membership in the same `applyWrites`, so the owner appears as a member and gets the badge.
13. **Opinion emptiness** ignores only `$type`, `subject`, `forum` and the timestamps. An opinion that holds only unknown (future) fields is kept, not deleted.
14. **MIME normalisation table** (§8.8). It is a concrete rule for the completeness fix: M4A becomes `audio/mp4`, Ogg becomes `audio/ogg`, WebM becomes `video/webm`. `blob:application/ogg` is not requested.
15. **`.temp` migration** cannot be a plain copy: fk and h16 hash at-uris that contain the collection NSID, so keys and strongRef CIDs change. `migrate-temp` is specified as a pure deterministic rewrite (§1.3). This corrects the completeness review's remark that "the rkey grammar is identical, so it only copies records".
16. **Client metadata scope** is the union of the default and fallback variants. P2 interop scopes are added to the hosted metadata when those features ship, since metadata is not frozen. `.temp` metadata omits `include:`.
17. **Indexer details the synthesis left open.** For each, this document takes the simplest choice that fits the synthesis:
    - **Moved to "later":** `getRecords`, which is in neither phase list.
    - **Error names:** `NotIndexed` and `RateLimitExceeded`.
    - **Parameters and paging:**
      - actor parameters are DIDs only;
      - `getChanges` uses an integer cursor with an exclusive `since`;
      - `listPosts.tier` is a minimum tier;
      - default page sizes are listed in §11.3.
    - **Output caps:**
      - `getThread` returns at most 10,000 comments;
      - `describeIndexer` lists at most 1,000 forums;
      - `getRepoHeads.status` uses the atproto `getRepoStatus` values.
    - **Response metadata:**
      - `rank` is formatted with Rust `f64` `Display`;
      - `getRecordProofs` carries no `asOf`, because proofs carry their own revs.
18. **Text blobs.** Readers accept any `text/*`, decode as UTF-8 and strip one BOM. Undecodable text shows a placeholder without invalidating the record. Spill measurement uses the exact request body.
19. **Precedence.** This document overrides the lexicon JSON until M7.
20. **Lexicon authoring** (made while writing `lexicons/`; this document was amended to match):
    - **Named defs.** Lexicon forbids inline objects in arrays and properties, so the indexer's inline output objects became method-local defs (§11.3). `searchPosts.highlights` changed from `[[start, end]]` pairs to `[#highlight{start, end}]`, because Lexicon has no arrays of arrays.
    - **Integer bounds.** The unbounded indexer integers got final bounds: `count` for counts and sequence numbers, `sint` for scores and karma (§2.1). The 2^53−1 limit keeps every value exact in JavaScript.
    - **Array limits.** Top-level output arrays are capped at the method's maximum page size. They are required and may be empty, so that clients never have to tell "absent" from "empty".
    - **`getChanges.cursor`** is required. When `changes` is empty it equals `since`.
    - **Errors.** Only `NotIndexed` and `CursorTooOld` are declared. `InvalidRequest` and `RateLimitExceeded` are generic XRPC errors. `getThread` also returns `NotIndexed`, for an unknown post or a post in a forum the indexer does not serve.
    - **Permission sets** carry a `description` next to `title` and `detail`.
    - **Lint.** `goat lex lint` warns `large-string` on `markdown.text` (100,000 bytes against goat's 20,480-byte threshold). The limit is deliberate, so the lint step ignores that rule (§1.4). CI and `xtask lex lint` must pass `-I large-string`.
21. **Key vectors** (§3.4). Error codes (`UnitSeparator`, `Undefined`, `InvalidTid`), the `raw` flag and the intermediate fields were added so that the Rust and TypeScript runners can share the files. `actionKey` of an unknown `$type` is the `$type` string, including for a foreign NSID. A `#pin` with an unknown `scope` hashes as `"pin:" + scope`. An unknown `scope` hashes as written. All three records then have no effect (MA7).

22. **Consistency review (amendments to this document).**
    - **Create-if-absent** (§8.4, §8.5, §8.6) now follows accepted ADR 0005: `putRecord{swapRecord: null}` with a typed `InvalidSwap`, a per-host `createRecord` fallback, and every HTTP 500 on a write treated as ambiguous and re-read (formerly README §1.4 K1). Content creates with fresh TID keys keep `createRecord`.
    - **MA8** now says that an empty `#restrict` competes in resolution like any assertion, so at a higher rank it lifts lower-rank restrictions, as `social.md` §4.2 and §4.7 require (formerly README §1.4 K2).
    - **G10** (nesting depth ≤ 32) makes the pre-parse limit of `limits.md` §6 a validity rule, so that every client treats a too-deep record identically: it is an invalid version, not a silently dropped or "unreadable" row. This resolves the open question shared by `limits.md`, `sync.md` and `threat-model.md`.
    - §7.4 states the opinion byte cap (100 MiB, from `limits.md`) and that a range reaching N keys is reported capped (`sync.md` §5.4).
    - §8.2 `PendingWrite` gains `seq` and `notBefore`, which the pump of `sync.md` §7.2 reads; §8.5 `apply` fixes the array order of inserted and replaced entries, so the written bytes are deterministic.
    - §9 gives the explicit ForumMagnum deferrals the phases of `docs/plan.md` §6.5; the nofollow gate is Never. §11.1 names the "later" indexer methods Reserved (P2).
    - `qkey` (§6.6) and the excerpt projection (§5.3) now point at their single definitions in `lfm-1.md`.

23. **Lexicon implementation review (M1).**
    - MA7 takes precedence over MA3–MA6 (§7.3).
    - Numbers: a JSON number with a fraction or exponent is never a valid integer, even when its value is integral (`123.0`, `1e3`). This deliberately diverges from the atproto interop fixture that accepts `123.0`; lessdugin never writes such numbers, and admission rejects them before the conversion (`sync.md` §5.5 step 2).
    - A `revision` whose `record` is a snapshot of a record already at depth 32 is itself at depth 33 and therefore invalid (G10). Writers refuse such edits; this cannot happen with records lessdugin writes.
    - FO2: when a DID appears several times in `moderators`, the first entry wins even if that entry is itself ignored (owner, unknown role).
    - Validity reports list at most 16 violations per rule ID; the rest are counted. The first failing rule and the set of failing rule IDs are always exact.

**Open questions** for the owner or the other spec authors:
- confirm item 1 (agreement value names);
- confirm item 6 (quickTake requires a body);
- confirm the opt-out in item 11;
- `goat lex validate` rejects every record whose union holds `com.atproto.label.defs#selfLabels` ("NSID syntax didn't validate via regex"), `app.bsky.feed.post` included. This is a goat bug. Spike S4 should record it, and no lessdugin check may rely on `goat lex validate` for `post`.
