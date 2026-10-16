<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin glossary

**Status:** normative for terms; draft for the `.temp` namespace (M0).
**Sources:** the glossary of `docs/design/synthesis-v1.md`, extended with every term the spec documents use (`docs/spec/README.md` §1.1).

**How to read it.** Each entry is a short definition plus the section that defines the term normatively. If an entry here and its defining section ever differ, the defining section wins and this glossary is corrected. Section references use the document name without `.md` where unambiguous: `data-model` §3.1 means `data-model.md` §3.1.

---

## 1. Notation and conventions

| Notation | Meaning | Defined in |
|---|---|---|
| MUST, SHOULD, MAY, … | Requirement levels as in RFC 2119 and RFC 8174, when in capitals | README |
| **Reserved (Pn)** | A name or behaviour that phase Pn (P2, P7, P8+) will define. v1 clients MUST NOT write it and MUST give it no effect when they read it. | README §2; `docs/plan.md` §6 |
| **Reserved (deferred)** | The same, for an item cut from the first pass without a phase. `docs/plan.md` schedules these in P2. | `threat-model` §0 |
| *tunable* | An operational constant that may change in any release without a new behaviour ID | `sync` §0.5 |
| *entry* (validity rule) | A rule that discards one field, axis or array entry instead of invalidating the record | `data-model` §7 |
| *(x)* (validity rule) | A rule that depends on another record in the fold input and is re-evaluated when that record enters or leaves | `data-model` §7, §7.5 |
| `!` | Required lexicon field | `data-model` §2.1 |
| `s(G/B)` | String with at most G graphemes and B bytes | `data-model` §2.1 |
| `kv{…}` | knownValues string (open enumeration), ≤ 64 bytes unless noted | `data-model` §2.1 |
| `[T]≤N` | Array of at most N items of type T | `data-model` §2.1 |
| `blob(accept; max)` | Blob with an accept list and a maximum size | `data-model` §2.1 |
| `md` | Open union containing `NS.markdown` | `data-model` §2.1, §5 |
| `slug` | ≤ 64-byte string matching `^[a-z0-9]+(-[a-z0-9]+)*$` | `data-model` §2.1 |
| `sref` | `com.atproto.repo.strongRef` `{uri, cid}`: a reference to one version | `data-model` §2.3 |
| `atu` | at-uri, ≤ 3,000 bytes: a reference to a record's identity, not a version | `data-model` §2.3 |
| `url` | `uri`-format string, ≤ 3,000 bytes | `data-model` §2.1 |
| `int[a..b]` | Integer between a and b inclusive | `data-model` §2.1 |
| `{fk}-`, `{fk}-{tid}`, `{fk}-{rootTid}-{tid}` | Record-key grammars; `{x}` is a substituted segment | `data-model` §3.2 |
| µs, ms | Integer microseconds (fold time) and milliseconds (host clock) since 1970-01-01T00:00:00Z | `social` §0.4; `sync` §0.5 |
| KB, MB / KiB, MiB | 1,000 and 1,000,000 bytes / 1,024 and 1,048,576 bytes | `limits` §1 |
| `author(x)`, `did(u)`, `coll(u)`, `rkey(u)` | The author DID of a record (or the DID itself for a DID subject); the components of a canonical at-uri | `social` §0.4 |
| `min_some(a, b)` | The minimum of the values that are present; none if neither is | `social` §0.4 |
| `X ∖ Y`, `\|X\|` | Set difference; cardinality | `social` §0.4 |

---

## 2. Overloaded names

Several short names mean different things in different documents. The context always decides; when writing, qualify them.

| Name | Meanings |
|---|---|
| **P** | (1) the plain-text projection `P(source)` of a body (`lfm-1` §10), with `P_x` its spoiler-free variant; (2) the set of **participants** P (`social` §3.1); (3) the revision-key prefix `{P}` = `rkey(subject)`, or `fk` for a forum subject (`data-model` §3.2). |
| **P1–P10** | Design principles (ADR 0001). |
| **P1–P9** | Fold properties checked by property tests (`social` §6.3). |
| **P0–P3** | Dirty-queue priorities, P0 highest (`sync` §6.1). |
| **P2, P7, P8+** | Later phases (`docs/plan.md` §6). |
| **L0–L7** | Fold layers (`social` §1.4). |
| **L1 (sync)** | The MVP sync level: per-forum `listRecords` ranges at trust level `pds` (`sync` §5). The verified range sync that would follow it is P7. Unrelated to fold layer L1. |
| **M0–M7** | Milestones (`docs/plan.md` §3). **M1–M14** are also protocol-critique findings (ADR 0001). |
| **S** | (1) the set of **seeds** (`social` §3.1); (2) S1–S12 are spikes (`docs/plan.md` §5); (3) limit class S, safety caps (`limits` §2). |
| **E** | The set of eligible (weighted) voters (`social` §6.2). |
| **K, K1, KP** | Karma stages (`social` §6). K1 is unrelated to the open issue K1 in README §1.4. |
| **head** | (1) a repo head `{cid, rev}` returned by `getLatestCommit` (`sync` §0.5); (2) the head of a wiki page, its current revision (`social` §13); (3) the field `#tag.head` of a modAction. |
| **tier** | (1) the post tier personal < frontpage < curated (`#tier`); (2) the moderation tier, forum or author (a modAction's `scope`); (3) the dependency tiers CORE, CORE+, EXT (`sync` §2.1). |
| **scope** | (1) a modAction's `scope`, forum or author; (2) a pin's scope, forum or thread; (3) an OAuth scope string (`data-model` §10.3); (4) a sync scope, `fk`, `GLOBAL` or `OWN` (`sync` §5.1). |
| **level** | (1) a content trust level, `hint`, `pds` or `proven` (`sync` §1.1); (2) an identity level, `hint`, `doc` or `audited` (`sync` §1.3); (3) a conformance level, Level 0 or Level 1 (README §4); (4) the field `#access.level`. |
| **revision** | (1) a `revision` record, the snapshot of an edited or deleted version; (2) a `wikiRevision` record, one version of a wiki page; (3) a repo revision, `rev`, a TID. |
| **tombstone** | (1) in the store, a marker that a record was confirmed deleted at a rev (`sync` §5.6, I4); (2) in a thread, the state `Tombstone(reason)` of a hidden node that has listed descendants (`social` §10.2). |
| **lock** | (1) the moderation action `#lock` on a post (`social` §4.8); (2) a process or tab lock: `fd-lock`, Web Locks (`sync` §10.6). |
| **rank** | (1) an authority's rank: owner 3, admin 2, moderator 1, others 0 (`social` §4.1); (2) `t_rank`, an item's effective time; (3) the `rank` field of an indexer `#postView`, the decimal form of `hot`. |
| **proof / property** | A *proof* is a signed-commit CAR verified by `proofs/1`; a *property* is a property-tested invariant (P1–P9). |

---

## 3. Rule and identifier families

| Family | Meaning | Defined in |
|---|---|---|
| D1–D21 | Design decisions | ADR 0001 |
| F\*, H\*, M\*, X\*, C\* | Findings of the security, protocol (high, medium, contradictions) and product critiques | ADR 0001; `docs/design/archive/critiques.md` |
| S1–S12 | Spikes | `docs/plan.md` §5 |
| U1–U8 | Items that need the user | `docs/plan.md` §8 |
| G1–G10 | Generic record-validity rules | `data-model` §7.2 |
| FO\*, ME\*, PO\*, CO\*, RV\*, OP\*, MA\*, WR\*, SQ\*, PF\* | Per-collection validity rules (forum, membership, post, comment, revision, opinion, modAction, wikiRevision, sequence, profile) | `data-model` §7.3 |
| W\* | Writer rules | `data-model` §8.1 |
| V1–V10 | Visibility rules, first match wins | `social` §10.1 |
| P1–P9 | Fold properties | `social` §6.3 |
| I1–I9 | Store invariants | `sync` §10.2 |
| A1–A5 | Indexer audit checks | `sync` §15.4 |
| R1–R6 | Rules when a limit is hit | `limits` §3 |
| N, L, W, S, P, O | Limit classes: normative behaviour, lexicon, writer, safety, presentation, operational | `limits` §2 |
| `AREA-n` (e.g. `TERM-3`) | Threat-model measure IDs | `threat-model` §0 |
| K1–K3 | Known issues between documents (resolved ones keep their number) | README §1.4 |

---

## 4. Terms

### A

| Term | Meaning | Defined in |
|---|---|---|
| A | The set of **authorities**: the owner plus every admin and moderator in the current declaration. | `social` §4.1 |
| `absent` | Fold input: the uris of opinion subjects whose absence the sync layer confirmed (tombstone, or a complete listing without them). Used by the deleted-subject rule. | `social` §1.1; `sync` §5.6 |
| access (`#access`) | Moderation action on a DID with `level` `banned` or `member`. `member` is a **grant**. | `data-model` §6.7; `social` §4 |
| adopt, `adopts` | `lessdugin mod adopt <did>` re-issues a former moderator's forum-scope assertions under the adopting authority; each copy's `adopts` points at the original and is informational only. | `social` §4.10; `data-model` §8.6 |
| admission | Accepting a record version into the record store at level `pds` or `proven`, after the CID check and validity-independent checks. | `sync` §5.5 |
| agreement | The second voting axis (`agree`, `disagree`, `strongAgree`, `strongDisagree`), summed with KP-based power over E. It never feeds karma. | `social` §7 |
| `algorithmIds` | The effective behaviour IDs (moderation, karma, ranking, react palette) in the fold input. | `social` §1.1, §1.6 |
| allow-list (lock) | `#lock.allowed`: at most 5,000 comment uris that stay visible on a locked post. | `social` §4.8 |
| approval mode | `forum.membership = approval` (the lexicon default): participants need a grant. Complete at CORE. | `social` §3.2 |
| assertion | What one modAction states about one subject and dimension. **Positive** assertions grant prominence or keep content and are CID-bound. | `social` §4.2, §4.5 |
| assisted | `sources = assisted` (default): CORE plus CORE+ and enabled indexers. | `sync` §2.3 |
| at-uri, canonical | `at://<did>/<nsid>/<rkey>` with no handle, query, fragment or trailing slash. A non-canonical reference invalidates the record (G4). | `data-model` §2.3 |
| audit (`--audit`) | The manual indexer cross-check, checks A1–A5. Automatic sampling is P2. | `sync` §15.4 |
| `audited` | Identity level: PLC audit log verified. Reserved (P7). | `sync` §1.3 |
| `authFull`, `authInteract` | The permission sets: all ten collections; membership, comment, revision and opinion. | `data-model` §10.1–§10.2 |
| AuthBridge | The SPA's main-thread gatekeeper (`auth-bridge.ts`) that holds the OAuth session and allows only `createRecord`, `putRecord`, `deleteRecord`, `applyWrites` and `uploadBlob` on the session's repo and NS collections, plus `getSession`. | `threat-model` §5.8 (AUTH-4); `engine-api` §5 |
| author tier, author scope | modActions with `scope = author`, written by a post author (rank 0) about their own threads, subject to gates. | `social` §4.9 |
| authority | A member of A. Authority comes only from the current declaration. | `social` §4.1 |
| authorization server, login binding | At login, the PDS from the DID document of the token's `sub` must list the token's `iss` in its `/.well-known/oauth-protected-resource` `authorization_servers`. | `sync` §3.6; `data-model` §10.5 |
| autoload policy | The conditions under which media loads without a click: author in S ∪ E, item visible, no sensitive self-label, trusted host. | `sync` §11.3 |

### B

| Term | Meaning | Defined in |
|---|---|---|
| B | The set of banned DIDs (resolved `access = banned`). | `social` §3.1 |
| badge | A status marker drawn only in the fixed badge column: `[verified]`, `[stale]`, `[partial k/n]`, `[partial]`, `[future-dated]`, `[identity unverified]`, `[edited]`, `[hint]`, `[pending]`. Plain `pds` content has none. | `sync` §1.6 |
| `BADGE_GLYPHS` | The 10 code points reserved for badges; stripped from user-supplied names and titles. | `lfm-1` §9.1 |
| ban | A resolved forum-tier `access = banned`: hides all of the user's in-forum content (V4) unless an item-level keep at ≥ the ban's rank applies. | `social` §4.6 |
| `b32s` | Base32 with alphabet `234567abcdefghijklmnopqrstuvwxyz`, MSB-first, no padding; its sort order is bit order. | `data-model` §3.1 |
| behaviour ID | A `name/N` identifier of a frozen deterministic behaviour: `fk/1`, `sk/1`, `lfm/1`, `mod/1`, `karma/1`, `magic/1`, `default/1`, `proofs/1`. Never changes meaning once released. | README §3 |
| blob | An atproto blob. Uploaded only with the write that references it; the returned blob object is embedded verbatim; fetched with `getBlob` and checked against its CID while streaming. | `data-model` §8.8; `sync` §11 |
| blob functions (`fetch_blob`) | The `Host` functions that stream a blob to storage while hashing it, so blob bytes never enter wasm memory. Called `BlobHandle` in the completeness review. | `sync` §12.2 |
| bridge | The `lessdugin web` endpoints `/bridge/session`, `/bridge/xrpc`, `/bridge/login`, `/bridge/kv` through which the SPA uses the CLI's session. `/bridge/fetch` is Reserved (P2). | `threat-model` §7 |

### C

| Term | Meaning | Defined in |
|---|---|---|
| C(F) | The **completeness set** of forum F: A ∪ G in approval mode; A ∪ S ∪ confirmed closure in open mode. | `sync` §4.5 |
| `cap(x)` | The restrict capability matching an item's collection: post, comment, sequence, wiki. | `social` §10.1 |
| capability (restrict) | One of `post`, `comment`, `vote`, `react`, `tag`, `wiki`, `sequence` in `#restrict.capabilities`. | `social` §4.7 |
| CAR | Content-addressable archive v1, the container of a proof. Parsed by our own capped parser. | `sync` §8.2 |
| CID | Content identifier: CIDv1, dag-cbor, sha256 for records; raw for blobs. Compared by the bytes of its canonical base32 string. | `social` §0.4; `sync` §5.5 |
| CID-bound | A positive assertion counts only while its `subjectCid` equals the subject's current CID. A stale one becomes a mod-queue item. | `social` §4.5 |
| `cid:` URL | A markdown media reference, resolved only against the body's own `media` list. | `lfm-1` §6 |
| closure | DIDs referenced by seeds' records in the forum (opinion subjects, comment roots and parents), each confirmed by its membership record. Found with CORE only. | `sync` §4.3 |
| Collapsed(reason) | A listed-but-collapsed state: `NewUser`, `LowScore` or (lens) `Muted`. | `social` §10 |
| comment | A threaded reply, rkey `{fk}-{rootTid}-{tid}`, with `root` and optional `parent` strongRefs. | `data-model` §6.4 |
| completeness | A view is complete when every repo in C(F) is `Fresh`; otherwise it shows `[partial k/n]`. | `sync` §4.5, I6 |
| conformance level | Level 0 (reader: posts, comments, moderator hides) or Level 1 (full fold). | README §4 |
| Constellation | A public backlink index (CORE+). A hint source for newcomers and counts. | `sync` §4.4 |
| CORE, CORE+, EXT | Dependency tiers: PDS, PLC/did:web and DoH; relay, Jetstream, Constellation, Slingshot; lessdugin indexers. | `sync` §2.1 |
| creator (react) | The supporter with the highest K (ties by DID) who is credited with a react; the others "seconded". | `social` §9.2 |
| curated | The highest post tier. Only the owner and admins may curate. Adds `curatedBonus` in magic. | `social` §4.3, §12 |
| currency rule | A third-party proof is current only if its rev ≥ a recently observed PDS head; otherwise it is `[stale]` and not a fold input. Proofs from the author's own PDS are always current. | `sync` §1.2 |

### D

| Term | Meaning | Defined in |
|---|---|---|
| DAG-CBOR | The canonical CBOR encoding of records, decoded strictly. | `sync` §8.3 |
| declaration (`decl`) | The `forum` record in the owner's repo. Only the current declaration confers authority and policy. | `data-model` §6.1; `social` §1.6 |
| deleted-subject rule | Votes on a confirmed-absent subject still count toward karma only if ≥ 2 distinct eligible voters cast them; never toward a score. | `social` §5 |
| degraded | Fold state when an authority repo is `Unavailable`: only content by S is shown, the rest `Hidden(ModerationUnavailable)`, with a banner. | `social` §4.11 |
| devnet | A local PLC + PDS from `@atproto/dev-env` for tests; clients reach it only in explicit devnet mode. | ADR 0006; `sync` §3.7 |
| dimension | A moderation axis: hidden, spam, access, restrict, nullify, tier, pin[scope], locked, tag.removed, tag.head. Each has a safety order. | `social` §4.2 |
| direct | `sources = direct`: CORE only. | `sync` §2.3 |
| dirty queue | The single coalesced queue (one item per DID) fed by every change source, with priorities P0–P3. | `sync` §6.1 |
| discovery | Finding forums (directory) and participants (seeds, closure, newcomers). | `sync` §4 |
| DoH | DNS over HTTPS (JSON API). The default way to resolve handles on every platform. | `sync` §3.1 |
| `dto/1` | The engine's DTO schema version, shared by CLI `--json`, the SPA and the indexer. Not a behaviour ID. | `engine-api` §1 |

### E

| Term | Meaning | Defined in |
|---|---|---|
| E | Eligible (weighted) voters: S plus established users. In approval mode E = S. | `social` §6.2 |
| `effective(x, d)` | The resolved value of dimension d for subject x after rank, safety-min and defaults. | `social` §4.4 |
| effective policy | The declaration's policy with lexicon defaults, `karma/1` gate defaults and unknown-value fallbacks applied. | `social` §1.6 |
| `[edited]` | Badge for an item that has a valid `op: update` revision, or an `updatedAt` later than its `createdAt`. | `social` §11; `sync` §1.6 |
| engine | `Engine<H: Host>`, the one implementation behind the CLI, TUI, SPA (wasm) and indexer. | `engine-api` |
| equivocation | A PDS serving different commits for the same rev. Recording it is P2. | `threat-model` §9 |
| `establish`, `establishSeeds` | Karma policy: the K1 threshold (default 1) and the number of distinct seed upvoters (default 2) needed to become established. | `social` §6.2 |
| established user | In open mode, a participant outside S with K1 ≥ `establish` and upvotes from ≥ `establishSeeds` distinct seeds. | `social` §6.2 |
| excerpt | The first ≤ 499 graphemes of P_x, cut at the last space, plus `…`; required in `post.excerpt` when the body is spilled. | `lfm-1` §10.4 |
| Excluded | The state of an invalid or out-of-scope record (V1). | `social` §10.1 |

### F

| Term | Meaning | Defined in |
|---|---|---|
| F | The forum's at-uri. | `social` §0.4 |
| fail closed | Unknown is not absent: missing moderation data degrades the view instead of resurrecting hidden content. | `social` §1.5, §4.11 |
| FakePds | The testkit's fake PDS with fault injection, cursor modes and proofs. | `docs/plan.md` §4 |
| fetch caps (normative) | Per (repo, collection, fk): only the first N keys of the `{fk}-` range in descending rkey order are fold input; N = 100,000 for `opinion`, 20,000 otherwise. Excess makes the repo `[partial]`. | `data-model` §7.4; `sync` §5.4 |
| `fk`, `fk/1` | The forum key: `b32s(H("lessdugin-fk/1", forumUri))[0..12]`, 60 bits. Prefixes every forum-scoped rkey. | `data-model` §3.1 |
| `fkOf(rkey)` | The fk segment of a forum-scoped rkey. | `data-model` §3.1 |
| fold | `ForumView = fold(FoldInput, Query)`: the pure, deterministic derivation of a forum view. | `social` §1 |
| FoldInput | `{decl, records, absent, obs, repoStatus, now, algorithmIds}`. | `social` §1.1 |
| followed forum | A forum the user follows locally; on a new device the list is seeded from the user's own membership records. | `sync` §4.8 |
| forum directory | The list of known forums from relays, owners' listings and indexers, grouped by owner. | `sync` §4.7 |
| forum tier, forum scope | modActions with `scope = forum`, written by authorities. | `social` §4 |
| ForumMagnum (FM) | The LessWrong / EA Forum codebase that lessdugin's features and vote power are modelled on. | ADR 0001 |
| ForumModel | The in-memory per-forum model of skeletons (references, opinion values, titles, times), rebuilt or updated from the store. | `sync` §13 |
| ForumView | The output of the fold. | `social` §1.1 |
| Fresh, Stale, Unavailable | `repoStatus`: listed completely at the latest known head; at an earlier rev; never completely. | `social` §1.1; `sync` §5.9 |
| frontpage | Post tier ≥ frontpage; the default list filter. | `social` §12 |
| future-dated | An item whose effective time exceeds its first-seen time by more than 5 min. Its time is clamped to first seen and it gets `[future-dated]`; it is not hidden. | `social` §2 |

### G

| Term | Meaning | Defined in |
|---|---|---|
| G | Granted DIDs: resolved `access = member`. | `social` §3.1 |
| gate | A karma threshold for an action (`reactSecond`, `reactCreate`, `reactAnti`, `authorModPersonal`, `authorModFrontpage`, `createTag`, `wikiEdit`, `sequence`), checked against current K. Authorities bypass every gate. | `social` §8 |
| genesis revision | A wikiRevision with no parents. | `social` §13 |
| grant | `#access{member}` written by an authority: an explicit human decision. Grants lapse with their author's authority. | `social` §3, §4.10 |

### H

| Term | Meaning | Defined in |
|---|---|---|
| `H(tag, p…)` | `sha256(UTF8(tag) ‖ (0x1F ‖ UTF8(NFC(p)))*)`; a part containing U+001F is rejected. | `data-model` §3.1 |
| `h16` | `b32s(H("lessdugin-sk/1", name, parts…))[0..16]`, an 80-bit subject hash; `name` is the short collection name. | `data-model` §3.1 |
| head observation | A `getLatestCommit` response from a repo's own PDS, recorded with its send time. Only these advance repo state. | `sync` §1.2, §5.9 |
| Hidden(reason) | A state that is not listed: `NotParticipant`, `ModerationUnavailable`, `Removed`, `Banned`, `Restricted`, `AuthorModerated`, `Locked`, `NewUser`. | `social` §10.1 |
| hint | Any non-authoritative source (relay, Jetstream, Constellation, Slingshot, indexer JSON). Hints may discover, schedule and show placeholders; they never supply displayed text and never feed the fold. | `sync` §1.4 |
| `[hint]` | Badge on a placeholder that exists only because of a hint. | `sync` §1.6 |
| `hot()` | `b / (ageHours + offsetHours)^(exponentMilli/1000)`, the only float computation, via `libm`. | `social` §12 |
| hover preview | A depth-1 preview of an `at://` link to one of our collections. | `lfm-1` §5 |

### I

| Term | Meaning | Defined in |
|---|---|---|
| identity level | `hint` (Slingshot, AppView, indexer), `doc` (PLC or did:web over TLS), `audited` (Reserved P7). | `sync` §1.3 |
| `[identity unverified]` | Badge for content whose author's identity is only a hint. | `sync` §1.6 |
| `include:` scope | An OAuth scope naming a published permission set (`include:NS.authFull`). Falls back at PAR time to enumerated `repo?collection=…` scopes. | `data-model` §10.3 |
| indexer | The optional `lessdugin-indexer` (EXT): the same engine plus `NS.index.*` XRPC, serving hints and proof CARs. | `sync` §15; `data-model` §11 |
| `inputDigest` | sha256 over the sorted fold-input lines, the declaration CID, the algorithm IDs and the policy parameters. Lets clients compare an indexer's input with their own. | `social` §1.2 |
| interstitial | The first-use notice that votes, reacts, memberships and mod actions are public. | `social` §17 |
| `InvalidSwap` | The PDS error for a failed `swapCommit` or `swapRecord`; the only conflict signal. | ADR 0005; `sync` §7.2 |

### J

| Term | Meaning | Defined in |
|---|---|---|
| Jetstream | Bluesky's JSON event stream (CORE+), used as a "this repo changed" signal. v2 endpoint by default; the v1 wire for DID filters above 150 DIDs. | `sync` §6.2; ADR 0003 |
| JsHost | The browser implementation of `Host` (with IndexedDB stores) in the engine's Web Worker. | `sync` §12 |

### K

| Term | Meaning | Defined in |
|---|---|---|
| K1 | Stage 1 karma: unweighted, per-pair-capped sums of seeds' votes. | `social` §6.2 |
| KP | Stage 2 karma: sums of E's votes weighted by K1-based power. It is the basis for scores. | `social` §6.2 |
| K | Displayed karma: sums of E's votes weighted by KP-based power. Gates use K. | `social` §6.2 |
| `karma/1` | Behaviour ID for opinion validity, karma, scores, gates, react aggregation, tags and wiki, and the sequence library. | `social` §0.3 |
| keep | A forum-tier `#hide{hidden: false}`: a CID-bound positive assertion that keeps an item visible despite a ban, restrict, lock or lower-rank hide. | `social` §4.5–§4.6 |
| KvStore | The storage trait (`get_many`, `scan`, atomic `write`), implemented by SQLite, IndexedDB and memory. | `sync` §12 |

### L

| Term | Meaning | Defined in |
|---|---|---|
| lapse | An assertion whose actor is no longer an authority has no forum-tier effect, at once. | `social` §4.10 |
| leader | The one background-sync process or tab per cache (`fd-lock` natively, a Web Lock in the browser). | `sync` §10.6 |
| lens (L7) | Viewer-local adjustments after the fold: mutes, optional Bluesky blocks, the raw view, "my view". It never changes fold outputs. | `social` §10.3 |
| `lessdugin web` | The CLI serving the SPA on `127.0.0.1` with the bridge: the zero-trust way to use the web UI. | `threat-model` §7 |
| LFM/1, `lfm/1` | lessdugin flavoured markdown, and its behaviour ID. | `lfm-1` |
| `libm` | The pure-Rust maths crate used for `hot()`, bit-identical on native and wasm. | `social` §1.3 |
| listed | State `Visible` or `Collapsed(*)`. Lists, feeds, search and notifications contain only listed items. | `social` §10.1 |
| `list_range`, cursor-range probe | Listing a `{fk}-` range with `listRecords(cursor = "{fk}.")` descending; a per-host probe decides whether the host honours it, else the full collection is listed. | `sync` §5.3; ADR 0004 |
| lock | `#lock{locked, allowed}` on a post: comments not in `allowed` and not by authorities are hidden (V7). Refused above 5,000 comments. The forum-wide lock is Reserved (P2). | `social` §4.8 |

### M

| Term | Meaning | Defined in |
|---|---|---|
| `magic/1` | Behaviour ID for time and ranking. The **magic** sort orders by pins, then `hot`; its `cutoffDays` window applies only to the magic sort. | `social` §2, §12 |
| `markdown` (`NS.markdown`) | The body object `{profile, text \| textBlob, media, mentions, links}`. | `data-model` §5 |
| media | `defs#media` entries of a body: the only scope for its `cid:` URLs. | `data-model` §4; `lfm-1` §6 |
| membership | The record (rkey `fk`) in a member's repo that says they joined. Also the forum's mode, `approval` or `open`. | `data-model` §6.2; `social` §3 |
| membership beacon | The membership record written in the same batch as a user's first content in a forum. | `data-model` §8.4 |
| mention | `[@x](at://<did>)`; displayed as `@handle` only if the handle verifies, otherwise `@short_did`. | `lfm-1` §5.4 |
| MIME normalisation | The declared MIME type sent to `uploadBlob` is derived from the sniffed type (for example M4A → `audio/mp4`, Ogg → `audio/ogg`). | `data-model` §8.8 |
| `mod/1` | Behaviour ID for participation, moderation, visibility and threads. | `social` §0.3 |
| mod log, mod queue, raw view | The list of every modAction and modAction revision; the actionable items for moderators; the unfiltered view with every assertion annotated. | `social` §4.12 |
| modAction | One record per (actor, scope, subject, action key) in the actor's repo. Updates and removals go through the revision protocol. | `data-model` §6.7, §8.4 |
| moderator hat | `comment.moderatorHat`, shown only if the author is a current authority. | `data-model` CO6 |
| MST | Merkle Search Tree: a repo's signed key-value structure; `proofs/1` walks it. | `sync` §8.5 |
| mute | A local lens setting: the author's listed items become `Collapsed(Muted)`. | `social` §10.3 |

### N

| Term | Meaning | Defined in |
|---|---|---|
| N | Nullified DIDs: resolved `#restrict.nullifyVotes = true`. Their votes are void and they leave S and E. | `social` §3.1, §4.7 |
| newcomer | In open mode, a participant outside S ∪ E; shown per `display.newUsers` (default collapsed, "new user"). | `social` §3.1, V8 |
| NativeHost | The native implementation of `Host` (reqwest, SQLite, keyring, WebSocket). | `sync` §12 |
| `NS`, `NS.temp`, `NS.index` | `online.7778777.lessdugin`; the unpublished development prefix `online.7778777.lessdugin.temp`; the indexer query group. | `data-model` §1 |
| `NsidPattern` | An NSID or `<nsid-prefix>.*`; used only for Jetstream filters. | ADR 0003 |

### O

| Term | Meaning | Defined in |
|---|---|---|
| `obs`, `t_obs` | The first-seen time (µs) of each admitted version, and of a record (the minimum over its versions). Used for the future clamp, notifications and unread state. | `social` §1.1, §2; `sync` §5.6 |
| open mode | `forum.membership = open`: anyone with a membership record participates. Newcomers are found only with CORE+. | `social` §3.2; `sync` §2.2 |
| opinion | One record per (voter, subject) holding the karma axis, the agreement axis, reacts and tag votes. Deleted when empty. | `data-model` §6.6 |
| opinion validity | The conditions under which an opinion counts (subject exists in the forum, voter not the author, not banned, nullified or restricted, subject not spam). | `social` §5 |
| Orphaned | Thread flag of a comment re-attached to the root after a cycle cut or depth overflow. | `social` §11 |
| `OURS`, `RANGED` | The ten record collections; the seven listed by `{fk}-` range. | `sync` §0.5 |
| outbox | The durable queue of `PendingWrite`s, pumped with one write in flight per account. | `data-model` §8.2; `sync` §7 |
| owner | The DID whose repo holds the declaration; rank 3. A forum is bound to it forever. | `social` §4.1 |

### P

| Term | Meaning | Defined in |
|---|---|---|
| P (participants) | Authorities, plus DIDs with a membership record (open) or with a membership record and a grant or ban (approval). | `social` §3.1 |
| P, P_x (projection) | The normalised plain text of a body, and the same with spoilers removed. Offsets are code points. | `lfm-1` §10 |
| `pairCap`, `pairsum` | The per-(voter, author) cap (default 25) and the capped summation used in each karma stage. | `social` §6 |
| PAR | Pushed authorization request. The scope fallback is chosen when PAR returns `invalid_scope`, before the browser opens. | `data-model` §10.3 |
| parity scenario | The cross-client test whose outputs must match byte for byte. | `docs/plan.md` §4.2 |
| `[partial k/n]`, `[partial]` | Badges: k of n completeness repos are complete; a repo hit a fetch or byte cap. | `sync` §1.6, §4.5 |
| `pds` (level) | Content fetched from the PDS in the author's DID document over TLS, with the CID recomputed. | `sync` §1.1 |
| PDS of D | The `serviceEndpoint` of the `#atproto_pds` service in D's current DID document. | `sync` §0.5 |
| `[pending]` | Badge for the viewer's own content or opinion whose write is not yet done. Pending writes never enter the fold. | `sync` §1.6, §7.4 |
| PendingWrite | A persisted outbox entry `{account, ops, intent, state}`. | `data-model` §8.2 |
| permission set | A published lexicon of type `permission-set` (`authFull`, `authInteract`) named by `include:`. | `data-model` §10 |
| pin | `#pin{scope: forum \| thread, pinned}`; CID-bound. Pinned items come first. | `social` §4, §12 |
| pingback | A notification computed from the `at://` links in the rendered bodies of synced posts. `markdown.links` is only a hint. | `social` §16; `data-model` §5.4 |
| PLC | The did:plc directory (CORE). Paced at ≤ 20 requests/s, 8 in flight. | `sync` §3.5 |
| policy | `forum.policy`: moderation, karma, ranking, voting system, react palette, gates, display, author moderation, tag creation. | `data-model` §6.1 |
| positive assertion | `pinned = true`, tier ≥ frontpage, or a keep (`hidden = false`). CID-bound. | `social` §4.5 |
| `power(k, v)`, `small(k)`, `strong(k)` | Vote power ported from ForumMagnum's `calculateVotePower`: small 1, or 2 at k ≥ 1,000; strong from a ladder 1 … 16 (2 at k ≥ 10 up to 16 at k ≥ 500,000). | `social` §6.1 |
| `proofs/1` | Behaviour ID for CAR parsing, commit verification (version 3, DID match, low-S k256/p256), the MST walk and the indexer proof-bundle manifest. | `sync` §8 |
| `proven@rev` | Content verified by a signed commit proof at a given rev. | `sync` §1.1 |
| provisional item | A `[stale]`-proof or identity-hint item shown without entering the fold. | `sync` §1.5 |
| publisher account | The did:plc account `lessdugin.7778777.online` that publishes the lexicons (M7). | `data-model` §1.4 |

### Q

| Term | Meaning | Defined in |
|---|---|---|
| `qkey` | The normalised, trimmed `exact` text of a quote (or `""`): the grouping key of react anchors. | `lfm-1` §11 |
| quick take | `post.kind = quickTake`: an untitled post (a title is ignored; a body is required). The feed is P2. | `data-model` PO3–PO4; `social` §15 |
| quote, quote resolution | `defs#textQuote {exact, prefix, suffix, start}`, resolved deterministically in P of the target's current version: one match, best context, nearest `start`, lowest offset; otherwise orphaned, or pending while the body is unavailable. | `lfm-1` §11 |

### R

| Term | Meaning | Defined in |
|---|---|---|
| react | `#react{react, stance, quote}` inside an opinion; ids from the forum's palette (`default/1`, 34 ids). Valid only if a supporter meets `reactCreate`. Reacts never affect karma. | `social` §9 |
| reconcile | The periodic full re-listing of every participant (spread over 24 h) and of authorities (hourly). | `sync` §6.8 |
| Recent Discussion | Posts ordered by their latest visible comment, each with its newest 3 visible comments. | `social` §12 |
| record store | The `cache` rows of admitted records. Holds only `pds` and `proven` versions (I5). | `sync` §10 |
| redaction | Deleting revision records, so their content and blobs are removed; shown as "history redacted". | `data-model` §8.4 |
| `RENDERER_VERSION` | The presentation version of the renderer, part of the render-cache key; not a behaviour ID. | `lfm-1` §1.2 |
| `repoStatus` | Per DID: `Fresh`, `Stale` or `Unavailable`. | `social` §1.1 |
| restrict | `#restrict{capabilities, nullifyVotes}`. Retroactively hides every in-forum item of a restricted kind by the user (rule 4b, V5); disables restricted axes. | `social` §4.7 |
| retract | `comment.retracted = true`: rendered struck through; still counts. | `social` §11 |
| rev | A repo revision (a TID), compared bytewise. | `sync` §0.5 |
| revision | A record in the editor's repo holding the verbatim prior value of an edited or deleted subject (`op: update`, or `op: delete` for modActions only). | `data-model` §6.5 |
| revision protocol | `applyWrites[create revision, update target]` with `swapCommit` from a fresh head and a CID-aware retry. | `data-model` §8.3 |
| rkey grammar | The canonical record-key form of each collection. A non-canonical key makes the record invalid. | `data-model` §3.2 |
| `rootTid`, `threadTid`, `stid` | The TID of a thread's root post; the function giving it for a post or comment uri; the subject segment of an opinion key (`threadTid`, or `~` for other subjects). | `data-model` §3.1–§3.2 |

### S

| Term | Meaning | Defined in |
|---|---|---|
| S | Seeds: `(A ∪ G) ∖ (B ∪ N)`. Always in E. | `social` §3.1 |
| safety-min | At the highest rank present, the safest value of a dimension wins (for example hidden over visible). | `social` §4.4 |
| `safe_href` | The URL sanitiser for every link and URL field. | `lfm-1` §5 |
| score | `self(i) + Σ power(KP[voter], karma)` over valid opinions by voters in E; no pair cap. | `social` §7 |
| `seedUp` | The number of distinct seeds whose net karma votes on a user's items are positive. | `social` §6.2 |
| sequence | An ordered list of chapters of post uris. Listed in the library if its author meets the `sequence` gate or it is forum-pinned. | `social` §14 |
| `short_did` | The placeholder display of a DID: `did:plc:` + 8 characters + `…`, or a did:web up to 40 characters. | `lfm-1` §5.4 |
| `sk/1` | Behaviour ID for `h16`, `threadTid`, `stid` and every rkey grammar. | `data-model` §3 |
| Slingshot | A public identity and record cache; a hint only. | `sync` §3 |
| spam | `#hide.spam`: opinions on a spam item do not count. | `social` §4.2, §5 |
| spill | Moving a body above 64 KiB (or a write body above 900,000 bytes) into `textBlob`. | `data-model` §5.3 |
| `[stale]`, stale proof | A third-party proof below the PDS head (not a fold input), or cached content after a failed sync. `stale-proof` also names a proof that fails only because the key rotated. | `sync` §1.2, §1.6, §8.7 |
| stance | `for` or `against` on a react. | `social` §9 |
| `StyledDoc`, `TermText`, `term_sanitize` | The terminal rendering: styled spans that can only be built by the sanitiser, which neutralises control, escape and bidi characters. | `lfm-1` §9 |
| subject, `subjectCid` | What an opinion or modAction is about; the CID a positive assertion is bound to. | `data-model` §6.6–§6.7 |
| succession | A forum cannot move to a new owner DID. The guide recommends a dedicated did:plc forum account with rotation keys held by ≥ 2 admins; `forum.successor` is Reserved (P2). | `social` §4.1 |
| supporters, opposers | The for-stance reactors with K ≥ `reactSecond`, and the against-stance reactors with K ≥ `reactAnti`. | `social` §9.2 |
| `swapCommit`, `swapRecord` | Compare-and-swap on the repo head (batch-level) and on one record. `swapRecord: null` means "must not exist". | `data-model` §8; ADR 0005 |
| sybil bound | Property P7: records from DIDs outside E (and not authorities) change no one else's K, scores or visibility, under the stated hypotheses. | `social` §6.3 |

### T

| Term | Meaning | Defined in |
|---|---|---|
| `t_claim`, `t_tid`, `t_eff`, `t_rank` | Claimed `createdAt`; the record's own TID time; their minimum, clamped to `t_obs` if more than 5 min in the future; the effective time `max(causal lower bound, t_eff)` used for ranking and display. | `social` §2 |
| tag | A (forum, slug) pair. It exists if its canonical wiki head exists and is not removed; it applies to a post if its relevance is > 0. | `social` §13 |
| `tag:` subject | A modAction subject `"tag:" + slug`; rank 0. | `social` §4.1 |
| tag relevance | `rel(post, slug)`: the author's own tag weight plus the power-weighted tag votes of E. | `social` §13 |
| TID | A timestamp identifier (13 base32-sortable characters). | atproto |
| tier (post) | `personal < frontpage < curated`; default `decl.defaultTier`. | `social` §4.2 |
| tombstone | See §2. | `sync` §5.6; `social` §10.2 |
| Trusted Types | The browser mechanism that forces every HTML sink through named policies (`lessdugin-md`, `dompurify`, `lessdugin-worker`). | `threat-model` §6 |
| trust level | `hint`, `pds`, `proven`. | `sync` §1.1 |

### U–Z

| Term | Meaning | Defined in |
|---|---|---|
| unknown is not absent | A record leaves the fold input only after a complete listing without it or a confirmed delete; unavailable repos contribute their cached records. | `social` §1.5 |
| `unweighted(i)` | The number of valid opinions with a karma value on an item from voters outside E: "n votes not yet counted". | `social` §7 |
| `until` | A modAction expiry; an assertion with `until ≤ now` has no effect. | `social` §4.4 |
| V1–V10 | The visibility rules, first match wins: Excluded, NotParticipant (and ModerationUnavailable), Removed, Banned, Restricted, AuthorModerated, Locked, NewUser, LowScore, Visible. | `social` §10.1 |
| vector | A test case `{name, input, expected}` under `testdata/vectors/<kind>/`. | README §5 |
| `[verified]` | Badge for `proven` content with a `doc`-level identity. | `sync` §1.6 |
| `verify` | The setting `pds` (default) or `proof` (Reserved P2), and the command `lessdugin verify <ref> [--deep]`. | `sync` §2.3, §8.8 |
| Visible | The default state when no other visibility rule matches. | `social` §10.1 |
| voting system | `policy.voting`: `karmaOnly`, `twoAxis` or `reacts` (default); disabled axes are stored but ignored. | `social` §7 |
| `wantedDids`, `dids=` | Jetstream DID filters (v1 `options_update`, v2 query parameter). | `sync` §6.2 |
| wiki, wikiRevision | A tag's page: immutable-by-convention revisions sharing (forum, slug). The MVP uses linear history; forks and merges are Reserved (P2). | `social` §13; `data-model` §6.8 |
| write budget | Per account: 5,000 points per hour and 35,000 per day; create 3, update 2, delete 1. | `sync` §7.3 |

---

## 5. Decisions made while writing

1. **The synthesis glossary entries are kept** (authority, seed, E, newcomer, fk, h16, t_rank, pds/proven/hint, closure, positive assertion, fold, NS), with their definitions aligned to the spec's final wording (for example S excludes banned and nullified DIDs).
2. **An overloaded-names section (§2)** was added because P, S, K1, L1, head, tier, scope, level, revision, tombstone, lock and rank each have several meanings across the documents. The glossary does not rename anything; it only disambiguates.
3. **Rule and identifier families (§3)** are listed so that a citation such as "PO3", "V5", "I4", "R1" or "TERM-3" can be traced without knowing which document defines it.
4. **"Reserved (deferred)"** from `threat-model.md` is listed beside "Reserved (Pn)"; `docs/plan.md` gives those items phase P2, and both mean "not in v1, no effect when read".
5. **Definitions are summaries.** Exact formulas (karma, `hot`, `inputDigest`, quote scoring) are given only where they fit one line; otherwise the entry points at the defining section.
