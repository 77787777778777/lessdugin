<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin specification

lessdugin is a markdown and media forum on the AT Protocol, modelled on ForumMagnum. Every forum, post, comment, vote, react, wiki edit, sequence and moderation action is an ordinary record in its author's own atproto repository. Every client computes the same forum view from those records with one deterministic fold. No lessdugin server is needed; an optional indexer only speeds things up.

This directory is the normative specification. It is written so that an independent implementation can interoperate with lessdugin's own clients without reading their code.

**Status:** draft for the development namespace `online.7778777.lessdugin.temp` (milestone M0). It is frozen at release 0.1 (M7), when the lexicons are published (§2).
**Key words.** MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED, MAY and OPTIONAL are used as in RFC 2119 and RFC 8174 when, and only when, they appear in capitals.

---

## 1. Documents

### 1.1 Index

| Document | Status | Defines | Behaviour IDs |
|---|---|---|---|
| `README.md` (this file) | normative | the document set, precedence, behaviour-ID rules, conformance levels, test vectors, licensing | – |
| `glossary.md` | normative for terms | every term, symbol and rule-ID family used in the spec, including overloaded names | – |
| `data-model.md` | normative | namespaces and publishing; every lexicon (records, shared definitions, permission sets, indexer queries); record keys; record validity (rules G\*, FO\*, ME\*, PO\*, CO\*, RV\*, OP\*, MA\*, WR\*, SQ\*, PF\*); normative fetch caps; the write protocol; reserved names; evolution rules and fallbacks | `fk/1`, `sk/1` |
| `lfm-1.md` | normative | LFM/1 markdown: parser profile, post-passes, URL policy, media references, math, the HTML and terminal contracts, the plain-text projection P, quote resolution, derived links and mentions | `lfm/1` |
| `social.md` | normative | the fold: input, `inputDigest`, layers, time, participation, moderation, opinion validity, karma, scores, gates, reacts, visibility, threads, ranking, tags and wiki, sequences, notifications | `mod/1`, `karma/1`, `magic/1`, `default/1` |
| `reacts-default-1.json` | normative data | the `default/1` react palette | `default/1` |
| `sync.md` | normative | trust levels and admission, identity, forum resolution and discovery, per-repo sync, live updates, the sync side of the outbox, proof verification, scheduling, the store, media display, the `Host` contract, the optional indexer | `proofs/1` |
| `limits.md` | normative | every byte, count, depth, time and budget limit, with its change class (N, L, W, S, P, O) and the rules for hitting a limit | – (its class-N numbers belong to the IDs that use them) |
| `threat-model.md` | normative | assets, trust boundaries, adversaries, inherent limits, measures (IDs `AREA-n`), the SPA CSP and headers, `lessdugin web`, residual risks | – |
| `engine-api.md` | normative for this repository's implementations; informative for others | the engine's operations, events and the DTO schema `dto/1` shared by the CLI's `--json`, the TUI, the SPA and the indexer | – (`dto/1` is a schema version, §3.4) |

Related material outside this directory:

| Path | Status |
|---|---|
| `lexicons/online/7778777/lessdugin/` | The lexicon JSON files, authored with the final NSIDs. Until M7, `data-model.md` wins over them; after publication, the published lexicons win (§1.3). |
| `testdata/vectors/` | Normative test vectors (§5). |
| `docs/adr/` | Decision records. `0001-decisions-v1.md` is the decision log (D1–D21) and the resolution of every design critique. Spike ADRs record measurements. |
| `docs/plan.md` | The implementation plan: milestones, acceptance criteria, deferred work. Normative for scope and order only. |
| `docs/design/` | Provenance: the reconciled design and the archived drafts and critiques. Informative only. |

A new spec document MUST be added to the index in the same change that creates it.

### 1.2 Reading order

For an independent implementation:
1. `glossary.md`, for vocabulary and the overloaded names.
2. `data-model.md` §1–§7: what records exist and when a record is valid.
3. `sync.md` §1–§5: where records may come from and how they are admitted.
4. `social.md`: how records become a forum view.
5. `lfm-1.md`: how bodies render.
6. `limits.md` and `threat-model.md`: the caps and the security measures every client must apply.
7. `data-model.md` §8 and `sync.md` §7: writing.
8. §4 below, to choose a conformance level.

### 1.3 Precedence

When two texts disagree, the first applicable rule wins:
1. **After publication (M7)**, a published lexicon wins over any document on the shape of a record; the document is corrected. **Before M7**, `data-model.md` wins over the lexicon JSON files, which are corrected to match.
2. **Numbers:** `limits.md` wins over a value repeated elsewhere. The *meaning* of a limited rule is defined by the document that `limits.md` names for it.
3. **Topics:** each document wins on what its row in §1.1 says it defines. In particular `data-model.md` wins on record semantics, validity and the write protocol; `social.md` on every fold rule; `sync.md` on admission, sync, verification and storage; `lfm-1.md` on rendering; `threat-model.md` on security measures.
4. **Accepted ADRs** refine the spec only once folded into it. Where a document says it follows an ADR that another document has not yet absorbed, the ADR wins until the other document is amended (§1.4).
5. The spec wins over `docs/plan.md`, over ADR 0001's summaries, and over everything in `docs/design/`.
6. **Vectors and text** of a released behaviour ID: see §3.3.

### 1.4 Known open issues between documents

These are recorded here so that implementers do not have to discover them. Each is resolved by amending the named document before the milestone that freezes the affected vectors. Resolved entries keep their number.

| # | Issue | Interim rule |
|---|---|---|
| K1 | *Resolved:* `data-model.md` §8.4–§8.5 now specify create-if-absent as `putRecord{rkey, swapRecord: null}` (typed `InvalidSwap`), with `createRecord` only as a per-host fallback, as ADR 0005 and `sync.md` §7.2 do. | – |
| K2 | *Resolved:* `data-model.md` MA8 now agrees with `social.md` §4.2 and §4.7: an empty `#restrict` restricts nothing itself, but at a higher rank it lifts lower-rank restrictions. | – |
| K3 | *Resolved:* ADR 0002 won (precedence rule 4) and is folded into `lfm-1.md`: §7.1 disables `\label`, `\eqref` and `\ref`; §7.3 is the MathML normaliser, which maps math-core's closed set of `style` values to `mc-*` classes (styled by `crates/markdown/assets/mathml.css`, after math-core's `mathmlfixes.css`) and `color` to `mathcolor`; §8.2 allows them on MathML. `lfm1/math.json`, `xss/mathml.json` and `mathml/` were amended with it. `RENDERER_VERSION` bumps when this lands. | – |

---

## 2. Namespace and freezing

- `NS = online.7778777.lessdugin`, with NSID authority `lessdugin.7778777.online`. Indexer queries live under `NS.index.*`.
- Development, devnet, CI and dogfood builds read and write only `NS.temp = online.7778777.lessdugin.temp`, which is never published. Every document and every lexicon file uses the **final** NSIDs; a `.temp` build substitutes the prefix everywhere (`data-model.md` §1.3). `.temp` data carries no compatibility promise.
- **Frozen at publication (M7):** NSIDs, field types and names, required sets, every lexicon limit and accept list, rkey grammars, and every behaviour ID released with 0.1.
- **May change in any release:** anything `limits.md` classes W, S, P or O; values marked *tunable* in `sync.md`; presentation under `RENDERER_VERSION` (`lfm-1.md` §1.2); the engine API compatibly within `dto/1`.
- **Before M7:** any normative text MAY change, but only together with its vectors, and with an ADR when it reverses a recorded decision (`docs/plan.md` §2.2).
- **"Reserved (Pn)"** marks a name or behaviour that phase Pn will define, and **"Reserved (deferred)"** one cut from the first pass without a phase. In both cases v1 clients MUST NOT write it, and MUST give it no effect when they read it.

---

## 3. Behaviour IDs

### 3.1 The IDs

A behaviour ID names a deterministic behaviour whose outputs every conforming client reproduces exactly. Its form is `name/N`.

| ID | Defined in | Selected by | Covers | Vectors (`testdata/vectors/…`) |
|---|---|---|---|---|
| `fk/1` | `data-model.md` §3.1 | fixed (bound to the collection NSIDs) | the forum key `fk` | `keys/` |
| `sk/1` | `data-model.md` §3.1–§3.3 | fixed (bound to the collection NSIDs) | `h16`, `threadTid`, `stid`, every rkey grammar | `keys/` |
| `lfm/1` | `lfm-1.md` | `NS.markdown.profile` | markdown semantics: syntax, post-passes, URL policy, P, excerpts, word counts, quotes, derived links and mentions | `lfm1/` |
| `mod/1` | `social.md` §3, §4, §10, §11 | `forum.policy.moderation` | participation and seeds; moderation; visibility, the thread pass and the lens; threads | `modfold/`, `thread/` |
| `karma/1` | `social.md` §5–§8, §9.2, §13, §14 | `forum.policy.karma.algorithm` | opinion validity; vote power and karma; scores and agreement; gates; react aggregation; tags and wiki; the sequence library | `karma/`, `reacts/`, `tags/` |
| `magic/1` | `social.md` §2, §12 | `forum.policy.ranking.algorithm` | time (`t_rank`); ranking, sorts, filters, Recent Discussion | `time/`, `rank/` |
| `default/1` | `social.md` §9.1, `reacts-default-1.json` | `forum.policy.reactPalette` | the set of react ids | `reacts/` |
| `proofs/1` | `sync.md` §8 | fixed (commit version 3; bundle manifest `v: 1`) | CAR parsing, commit verification, the MST walk, the indexer proof-bundle manifest | `proofs/` |

The fold contract (`social.md` §1: purity, integer arithmetic, `libm` for `hot()` only, tie-breaks, `inputDigest`) binds every ID; its vectors are in `fold/`.

### 3.2 Rules

1. **A released behaviour ID never changes meaning.** Any change to a covered rule that could change any output for some input needs a new ID (for example `karma/2`), which is defined beside the old one. The old ID stays implementable from its text and its vectors.
2. **Editorial changes** that cannot change any output for any input (clarifications, examples, fixing a reference) MAY be made at any time.
3. **Selection.** The selectable IDs are chosen by the forum declaration. An absent field selects the lexicon default. An unknown ID falls back to the default (`mod/1`, `karma/1`, `magic/1`, `default/1`, or `lfm/1` for a markdown body), and the client shows the banner "this forum uses rules newer than your client" (`data-model.md` §13.3). The *effective* IDs are part of the fold input and of `inputDigest` (`social.md` §1.1–§1.2).
4. **Fixed IDs** (`fk/1`, `sk/1`, `proofs/1`) are not selectable. A successor of `fk/1` or `sk/1` needs new collection NSIDs (`data-model.md` §13.2); a successor of `proofs/1` needs a new repo commit format or bundle manifest version.
5. **Release.** IDs are released with 0.1 (M7). Before that they are drafts under `.temp` and follow §2's pre-M7 rule.
6. **Every limit of class N** in `limits.md` belongs to an ID. A different value is a new ID.
7. Clients SHOULD keep implementing every older ID that forums still select.

### 3.3 Vectors and text

- An implementation conforms to an ID only if it passes every normative vector of that ID (§5) and follows its text.
- If, after release, a vector of an ID is found to contradict that ID's text, **the vector wins for that ID**: it records what conforming clients compute. The text is corrected with a note, and a different behaviour needs a new ID.
- Expected outputs of a released ID's vectors MUST NOT be re-blessed. New vectors MAY be added.

### 3.4 Versions that are not behaviour IDs

| Version | Where | Change rule |
|---|---|---|
| `RENDERER_VERSION` (`u32`) | `lfm-1.md` §1.2 | Presentation only (HTML serialisation, classes, `StyledDoc` styling). It changes in any release that also re-blesses the presentation vectors and regenerates the sanitiser configurations. It is part of the render-cache key `(cid, RENDERER_VERSION)`. |
| `dto/1` | `engine-api.md` | The engine's DTO schema. Optional fields and enum variants may be added; anything else bumps it. |
| Lexicon evolution | `data-model.md` §13 | New optional fields, knownValues, union members, defs and collections are allowed. Breaking changes get a new NSID (`<name>V2`). |
| Limit classes W, S, P, O | `limits.md` §2 | As stated there. |
| Measure IDs `AREA-n` | `threat-model.md` §0 | An ID never changes meaning; a withdrawn measure keeps its ID. |
| Rule IDs (G1, PO3, V5, I4, A2, R1, …) | the defining document | Stable labels for citation. A rule's meaning changes only as its behaviour ID or freeze class allows. |

---

## 4. Conformance

### 4.1 Levels and profiles

| Name | Who | Summary |
|---|---|---|
| **Level 0 (reader)** | minimal readers, bots, archivers, alternative clients | Reads a forum's posts and comments and applies the moderators' hides. |
| **Level 1 (full fold)** | lessdugin's CLI, TUI, SPA and indexer | Computes the complete forum view, byte-identical to every other Level-1 client. |
| **Writer requirements** | any client that writes records, at either level | Writes only valid records, through the write protocol. |
| **Indexer requirements** | any service that serves `NS.index.*` | Level 1, plus the indexer rules. |

### 4.2 Level 0 (reader)

A Level-0 client displays a forum's posts and comments, with moderators' hides applied. It MUST:

1. **Admission** (`sync.md` §1, §5.5): display and fold only records admitted at level `pds` (fetched from the PDS in the author's DID document over TLS, with the CID recomputed from JSON → DAG-CBOR) or `proven` (verified by `proofs/1`, and current under `sync.md` §1.2). It MUST NOT display text from hints (relay, Jetstream, Constellation, Slingshot, indexer JSON). It MUST exclude records of inactive repos (`sync.md` §3.4).
2. **Identity** (`sync.md` §3): resolve DIDs through PLC or did:web, and display a handle only if it verifies in both directions; otherwise display `short_did` (`lfm-1.md` §5.4).
3. **Forum and authorities** (`sync.md` §4.2; `social.md` §4.1): read the declaration from the owner's PDS; take the authorities from it (rule FO2); use the effective policy (`social.md` §1.6).
4. **Discovery and sync** (`sync.md` §4–§5): find participants by any method of `sync.md` §4 (in approval mode the grants in the authorities' repos give the complete set); in every authority's repo and every participant repo it reads, list the `{fk}-` ranges of `post`, `comment` and `modAction` and fetch the membership record with rkey `fk`; apply the normative fetch caps (`data-model.md` §7.4); remove a record only after a complete listing without it or `RecordNotFound`. Authority repos come first; while one has never been listed, the view is `degraded` (item 6).
5. **Validity** (`data-model.md` §7): apply G1–G10 and the rules for `forum`, `membership`, `post`, `comment` and `modAction`, and ignore invalid records.
6. **Moderation** (`social.md` §3, §4): implement `mod/1` for forum-scope assertions of the dimensions `hidden` (with `spam` and `showReason`), `access`, `restrict` (including `nullifyVotes`) and `locked`, with every rule of `social.md` §4.3–§4.8 (authorisation by rank and role, current authorities only, `until`, CID-bound positive assertions, rank then safety-min, ban, retroactive restrict, post locks with allow-lists) and fail closed (`degraded`, §4.11).
7. **Visibility** (`social.md` §10.1–§10.2, §11): place threads by `social.md` §11, and evaluate rules V1, V2 (including `Hidden(ModerationUnavailable)`), V3, V4, V5 and V7 exactly; then:
   - V6 (author moderation) is not evaluated; author-scope modActions have no effect;
   - V8 is evaluated with S in place of S ∪ E: in open mode, every author not in S is treated as a newcomer (this collapses at least as much as a Level-1 client);
   - V9 (low score) is not evaluated;
   - the thread pass (tombstones) is applied.
8. **Rendering:** render bodies with `lfm/1`, or show the markdown source as escaped plain text. It MUST NOT interpret raw HTML, MUST NOT fetch external images automatically, and MUST NOT load blobs automatically unless it implements `sync.md` §11 (CID check and autoload policy). Terminal output MUST be sanitised as `lfm-1.md` §9.1 (`term_sanitize`) specifies.
9. **Disclosure:** indicate to the user that scores, karma, author moderation and newcomer establishment are not evaluated.

A Level-0 client MAY order items by `t_rank` (`social.md` §2) or by uri, and MAY list everything visible (the "all" filter). It MUST NOT present an ordering as `magic` or `top`, or a list as frontpage or curated, unless it implements `magic/1`, `karma/1` and `#tier`. It makes no claim of byte-identical output, and its `inputDigest`, if computed, is not comparable with Level 1.

Level-0 vectors: `keys/`, the `lexicon/` cases for the five collections above, `modfold/` cases without author scope or scores, `thread/`, and `time/` if it orders by `t_rank`.

### 4.3 Level 1 (full fold)

A Level-1 client MUST:
1. Meet every Level-0 requirement, with items 7 and 9 replaced by the full rules: every visibility rule V1–V10 and the lens (`social.md` §10.3).
2. Fetch every collection of `sync.md` §5.1 for its forums, including opinions (fetch cap 100,000), revisions, wikiRevisions and sequences.
3. Implement every behaviour ID that a forum it opens selects (in practice `mod/1`, `karma/1`, `magic/1`, `default/1`, `lfm/1`), plus `fk/1` and `sk/1`, and pass all their normative vectors and the `fold/` vectors, with byte-identical output.
4. Implement `proofs/1` if it admits any third-party proof, and apply the currency rule (`sync.md` §1.2).
5. Compute completeness and its badges as `sync.md` §4.5 and §1.6 specify.
6. Apply every MUST of `threat-model.md` §5 that concerns its platform (terminal, SPA, native).

### 4.4 Writer requirements (either level)

A client that writes records MUST:
1. Write only records that are valid under `data-model.md` §7, with canonical keys (`fk/1`, `sk/1`), `$type` set, and `validate` left unset (`data-model.md` §2.5).
2. Follow the write protocol of `data-model.md` §8 and `sync.md` §7: edits create a revision with the verbatim prior value under `swapCommit` taken from a fresh `getLatestCommit`; one write in flight per account; create-if-absent and conflict handling (`data-model.md` §8.4); deletes with the target in the first chunk; blobs uploaded with the write and embedded verbatim; spilling and size checks.
3. Never write a reserved name (`data-model.md` §9), and never write records of the other namespace prefix.
4. Write the membership record with a user's first content in a forum, and, when creating a forum, write its `membership` mode explicitly.

### 4.5 Indexer requirements

An indexer MUST be Level 1, MUST implement the methods it lists in `describeIndexer.features` exactly as `data-model.md` §11 and `sync.md` §15 define, MUST compute `inputDigest` as `social.md` §1.2 defines, and MUST serve record values only as hints and proofs only as `proofs/1` CARs.

### 4.6 Claims

A conformance claim names the level and every behaviour ID implemented, for example:

```
lessdugin Level 1 + writer: fk/1 sk/1 lfm/1 mod/1 karma/1 magic/1 default/1 proofs/1
lessdugin Level 0: fk/1 sk/1 mod/1 (forum scope)
```

A claim is valid only while the implementation passes every normative vector of the listed IDs.

---

## 5. Test vectors

### 5.1 Layout and format

- Vectors live in `testdata/vectors/<kind>/*.json`. Each file is a JSON object `{behaviour, kind, description?, vectors}`: `behaviour` names the behaviour ID (or contract) under test, `kind` the vector category within it, and `vectors` is an array of objects `{name, input, expected}`; `name` is unique within its file. The owning document defines the shapes of `input` and `expected` and MAY define further fields (for example `note`, or `raw` in `keys/`).
- Inputs inject every source of variation: `now`, `obs` and local state are explicit, and the fold never reads a clock.
- Floating-point results (only `hot`, `magic/1`) are written as the 16 lowercase hex digits of their binary64 bit pattern.
- `testdata/interop/` holds vendored atproto-interop-tests under their own licence. They test atproto conformance (syntax, data model, MST, signatures) and are not lessdugin vectors.

### 5.2 Kinds

| Kind | Owner | Class |
|---|---|---|
| `syntax`, `datamodel` | atproto, plus lessdugin cases (NSIDs, Jetstream patterns) | normative for the atproto layer |
| `keys` | `data-model.md` §3 | `fk/1`, `sk/1` |
| `lexicon` | `data-model.md` §7 | record validity; frozen with the lexicons |
| `limits` | `limits.md`, `data-model.md` §2.2 | N- and L-class limits |
| `lfm1` | `lfm-1.md` §13 | `lfm/1`; fields that `lfm-1.md` §1.2 assigns to presentation follow `RENDERER_VERSION` |
| `terminal`, `xss`, `mathml` | `lfm-1.md` §13 | presentation and security corpora; re-blessed only with a `RENDERER_VERSION` bump, and the security invariants (no ESC other than allowed SGR/OSC 8; `DOMPurify(html) === html`) are never relaxed |
| `media` | `data-model.md` §8.8, `sync.md` §11 | the MIME normalisation table is normative; stripping cases test the reference implementation |
| `time`, `rank` | `social.md` §2, §12 | `magic/1` |
| `modfold`, `thread` | `social.md` §4, §10, §11 | `mod/1` |
| `karma`, `reacts`, `tags` | `social.md` §5–§9, §13 | `karma/1` (`reacts` also `default/1`) |
| `fold` | `social.md` §1 | the fold contract (`inputDigest`, native = wasm) |
| `proofs` | `sync.md` §8 | `proofs/1` |

### 5.3 Runners and blessing

- The Rust runner (`lessdugin-testkit`) and the TypeScript runner (vitest, calling `runVector` in a `vectors`-feature wasm build) run every vector. Their results MUST be equal, which proves native = wasm.
- `cargo xtask vectors --bless` regenerates expected outputs. Every blessed diff is reviewed, and lands in the same commit as the spec change that explains it.
- Before M7, any expected output MAY change under that rule. After M7, the rules of §3.3 apply.
- Property tests (proptest; `social.md` §19) complement the vectors: permutation invariance, idempotence, layering, the pair and sybil bounds, incremental = batch, native = wasm.

---

## 6. Licensing

- **CC0-1.0:** this directory (`docs/spec/**`), the lexicons (`lexicons/**`) and the test vectors (`testdata/vectors/**`). The text is in `LICENSES/CC0-1.0.txt`. Anyone may implement, copy, modify and redistribute them, for any purpose, without attribution.
- Every markdown file in these paths starts with `<!-- SPDX-License-Identifier: CC0-1.0 -->`. Files that cannot carry a comment (JSON) are CC0-1.0 by their location, and this section is their notice.
- Contributions to these paths are accepted under CC0-1.0, with a DCO sign-off.
- `testdata/interop/` keeps its upstream licence (its own `LICENSE` file).
- Not covered here: the applications (AGPL-3.0-or-later) and the `lessdugin-atp` and `lessdugin-lexicon` crates (MIT OR Apache-2.0, pending the owner's confirmation; ADR 0001, D21). An implementation of this specification may use any licence.

---

## 7. Decisions made while writing

1. **Two levels plus two requirement sets.** The design names only Level 0 ("read posts and comments, apply mod hides"). Level 1 names what the reference clients already do. Writing and indexing are separate requirement sets, because a minimal reader may also write and a full reader may not.
2. **What Level 0 evaluates.** Level 0 implements forum-scope `mod/1` exactly (hide, ban, restrict, lock, participation, fail closed), because those are "mod hides" and need no karma. Author moderation (V6) and low-score collapse (V9) need `karma/1` gates and scores, so Level 0 skips them and must say so. Newcomer collapse (V8) uses S instead of S ∪ E, which never shows anything a Level-1 client would collapse. This is the simplest safe reading.
3. **Level 0 fetches only** posts, comments, modActions, memberships and the declaration. Opinions are not needed for any rule Level 0 evaluates.
4. **A vector wins over contradicting text after release** (§3.3), because deployed clients compute what the vectors say. Re-blessing a released vector is forbidden.
5. **Fixed IDs.** `fk/1`, `sk/1` and `proofs/1` are not selectable by a declaration; their successors need new NSIDs or a new proof format.
6. **Cross-document conflicts** (§1.4): K1 followed ADR 0005, as `sync.md` already did; K2 followed `social.md`, because it matches D10 and the undo rule of `data-model.md` §6.7. Both have since been folded into `data-model.md`. K3 follows precedence rule 4 until M4.
7. **Precedence** (§1.3) collects the rules each document states about itself into one ordered list; it adds no new preference except rule 4 (an ADR wins over a document that has not yet absorbed it).
8. **JSON files** carry no SPDX comment; their licence follows from their location and §6.

**Open questions**
- Confirm Level 0's treatment of author moderation (not evaluated, disclosed) rather than the alternative of applying author-scope hides without gates.
- *(Resolved at M4.)* K3 (math normaliser and pre-scan, ADR 0002 against `lfm-1.md` §7 and its vectors): ADR 0002 is folded into `lfm-1.md` §7 and §8.2 unchanged, so no new ADR was needed.
