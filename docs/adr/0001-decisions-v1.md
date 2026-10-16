<!-- SPDX-License-Identifier: CC0-1.0 -->
# ADR 0001 — Decisions for v1 (D1–D21) and resolved critiques

- Status: Accepted
- Date: 2026-10-16
- Sources: `docs/design/synthesis-v1.md` §0 and §10; `docs/design/completeness-review.md`; the approved plan. Critique IDs (F\*, H\*, M\*, X\*, C\*) refer to `docs/design/archive/critiques.md`.
- Amended by: ADR 0002 (S3/S10), ADR 0003 (S4/S6), ADR 0004 (S7/S9), ADR 0005 (S12, S4 writes), ADR 0006 (S5), where noted below.

## Context

lessdugin's design came from three research passes and a nine-agent design workflow: four architects (protocol, sync, social, architecture), three adversarial critics (security, protocol, product), a synthesizer, and a completeness critic. The synthesis reconciled the four drafts and the three critiques into 21 decisions (D1–D21). The completeness review then found blockers, inconsistencies and factual errors, and the approved plan folded its fixes in.

This ADR is the decision log. It records each decision as it now stands (with the completeness fixes and the spike ADRs applied), why it was taken, and where the spec defines it. It also keeps the synthesis's resolution of every critique finding, under its original ID. The normative text is in `docs/spec/`; where this log and the spec differ, the spec wins and this log is corrected by a later ADR.

### Principles

The rationales below cite these principles (synthesis §1.1).

| ID | Principle |
|---|---|
| P1 | The PDS is the database. Author PDSes, PLC/did:web and DNS are enough for every MVP feature. |
| P2 | Authoritative bytes come from exactly two places: the author's PDS (from the DID document, over TLS, CID recomputed) or a signed commit proof checked against the DID's `#atproto` key. |
| P3 | Hints never become content. They may discover, schedule, order placeholders and give approximate counts. |
| P4 | Determinism. `ForumView = fold(…)` is byte-identical on native, wasm and the indexer. |
| P5 | Validity by construction: deterministic keys, required fields, frozen limits. Invalid records appear only in the raw view. |
| P6 | Time is a claim. Honest content behaves the same on every client; dishonest timestamps only hurt their author. |
| P7 | Fail closed. Unknown is not absent; moderation never vanishes because a PDS is down. |
| P8 | Public means public. Votes, reacts, memberships and mod actions are public, and the UI says so. |
| P9 | One implementation of domain logic: the Rust core serves the CLI, TUI, SPA and indexer. |
| P10 | Decide the permanent things now (NSIDs, rkey grammars, required sets, limits, behaviour IDs); defer anything that can be added later. |

(These P-numbers are design principles. They are unrelated to the phases P2/P7/P8+, the dirty-queue priorities P0–P3 and the fold properties P1–P9; see `docs/spec/glossary.md` §2.)

---

## Decision

### D1. The PDS is the only database

- **Decision.** No MVP feature needs a lessdugin server. The indexer is an optional extension whose output is either a hint or a verifiable proof.
- **Rationale.** The user's core constraint: no lessdugin server, no extra database, no trusted website. Any client can then recompute any view (P1, P4), and no operator can censor or rewrite the forum.
- **Amended (completeness review, P3 "nobody runs anything server-side").** Only approval mode is complete without third-party services: in open mode with `sources = direct`, seeds never see newcomers, so the closure never grows. The `membership` lexicon default is therefore `approval`, the CLI always writes the mode explicitly, and open mode is labelled "relies on public relay infrastructure".
- **Spec.** `sync.md` §2; `data-model.md` §6.1; `social.md` §3.

### D2. Namespace

- **Decision.** One flat namespace `NS = online.7778777.lessdugin` (one DNS TXT record), plus `NS.index.*` for indexer queries (its own TXT record). Development and dogfood builds write `NS.temp = online.7778777.lessdugin.temp`, which is never published. The prefix is a build-time constant; lexicon files are authored with the final NSIDs.
- **Rationale.** A flat group needs one TXT record and avoids record/group name clashes (no `NS.forum.*` group, M3). The indexer API evolves separately from the records (X3). `.temp` lets the record shapes be dogfooded before they freeze (product critique §6, risk 1).
- **Amended.** ADR 0003 confirmed that the digit segment works in every tool on the path. `data-model.md` §1.3 corrects the completeness review: moving `.temp` data to `NS` is a deterministic rewrite (`xtask migrate-temp`), not a copy, because fk and h16 hash at-uris that contain the collection NSID.
- **Spec.** `data-model.md` §1.

### D3. Ten record collections

- **Decision.** `forum`, `membership`, `post`, `comment`, `revision`, `opinion`, `modAction`, `wikiRevision`, `sequence`, `profile`. One `opinion` record per (voter, subject) holds the karma axis, the agreement axis, reacts and tag votes.
- **Rationale.** Few collections mean few `listRecords` calls per participant, which dominates cold-sync cost. One opinion record gives one write per toggle, atomic updates and one Constellation path (C1, X1).
- **Amended.** `membership` defaults to `approval` (see D1). `revision.record` has type `unknown` with a mandatory `$type`, validated in core, because a union of record types may trip lint and codegen. modAction updates and deletes write revisions, so `op: delete` exists only for modActions.
- **Spec.** `data-model.md` §6.

### D4. Forum-prefixed, deterministic record keys

- **Decision.** Every forum-scoped record key starts with the forum key `{fk}-`. Keys of records that track a subject are deterministic (`h16`). A record whose key is not the canonical one is invalid.
- **Rationale.** A forum is then one key range per collection in every repo, listable with the `listRecords` cursor trick (C3). A thread is one range. The comment key `{fk}-{rootTid}-{tid}` ties each comment to its root, so backdating cannot hide a comment from windowed sync (H7). A one-per-(actor, subject) record is one `getRecord` and cannot be duplicated (P5).
- **Amended.** ADR 0004 confirmed range listing on the reference PDS, Bluesky production and rsky-pds, and fixed the probe rules. The completeness review made the fetch caps normative: per collection, the first N keys of the `{fk}-` range in descending rkey order (N = 100,000 for `opinion`, 20,000 otherwise), so every client excludes the same excess.
- **Spec.** `data-model.md` §3, §7.4; `sync.md` §5.3–§5.4.

### D5. Every content record names its forum

- **Decision.** Every content record has a required `forum` field. There are no forum-less posts.
- **Rationale.** ForumMagnum's "personal blog" is a tier inside a forum (C16); real personal blogs belong to standard.site. Keys need the fk.
- **Amended (completeness review, P0.6).** Required fields per post kind: `title` is required unless the post is a `quickTake` (whose title is ignored); `link` is required for `kind = link`. `data-model.md` also requires a `body` on a `quickTake`.
- **Spec.** `data-model.md` §6.3, §7.3 (PO1–PO8).

### D6. Trust levels

- **Decision.** Content counts only if it was fetched from the author's PDS over TLS with the CID recomputed (`pds`), or arrived with a signed commit proof (`proven`). Everything else is a hint: it can schedule work and order placeholders, but never supplies displayed text and never feeds moderation or karma.
- **Rationale.** Otherwise any indexer could put words in anyone's mouth (F13). TLS from the PDS named in the DID document authenticates as well as a signature does while that PDS holds the key (X6), so proofs are needed only for third-party sources.
- **Amended (completeness review, P0.3).** A third-party proof counts as current only if its rev is at least the head returned by the author PDS's `getLatestCommit`. Otherwise it is `[stale]` and stays out of the fold input; head checks are batched per author. `sync.md` adds provisional display of such items.
- **Spec.** `sync.md` §1.

### D7. MVP sync

- **Decision.** Per-forum `listRecords` key ranges (the cursor trick, checked at runtime per host), with Jetstream as the "this repo changed" signal and a daily reconcile. Verified MST range sync comes in P7.
- **Rationale.** Range sync over the MST is a large build that the MVP does not need (C12). A repo's rev changes on every `app.bsky` write, so polling revs alone is useless (H5); a filtered Jetstream signal is not. Cursors are opaque per spec, so the trick needs a probe and a fallback (H6).
- **Amended.** ADR 0004: the two-part probe, the short-page rule, and per-host read budgets. ADR 0003: the v2 Jetstream endpoint by default, the v1 wire for large DID filters, `identity`/`account`/`sync` events for unknown DIDs dropped at once. Completeness review: normative fetch caps (D4).
- **Spec.** `sync.md` §5–§6.

### D8. One deterministic fold, versioned by behaviour IDs

- **Decision.** All derived state is a pure, deterministic fold in the Rust core. Native code, wasm and the indexer produce byte-identical output for the same inputs. Behaviour is versioned by IDs that never change meaning: `mod/1`, `karma/1`, `magic/1`, `lfm/1`, `default/1`, `sk/1`, `fk/1`, `proofs/1`.
- **Rationale.** Every client must compute the same view (P4), parity tests become byte comparisons, and an ID that never changes meaning keeps old clients correct for forums that select it.
- **Amended.** `proofs/1` is listed explicitly (the synthesis's D8 omitted it). The fold input gains `absent` (confirmed-absent opinion subjects) and excludes stale third-party proofs.
- **Spec.** `social.md` §1; `docs/spec/README.md` §3.

### D9. Seeded, staged `karma/1`

- **Decision.** K1 → E → KP → K, with a per-(voter, author) cap. Becoming an established voter in open mode takes upvotes from at least `establishSeeds` (default 2) distinct seeds. Vote power ports ForumMagnum's `calculateVotePower`.
- **Rationale.** Unseeded karma lets a sybil farm elect itself (F18, X7). A fixed-point iteration was rejected: its convergence and determinism are hard to guarantee and to explain. Each stage reads only earlier ones, so the fold is acyclic and linear.
- **Amended (completeness review, P1).** The remark "`establishSeeds` = 1 in approval mode" is deleted: in approval mode E = S, so the setting has no effect. The sybil theorem is restricted to K, scores and other users' visibility, because unweighted counts and newcomer collapse legitimately change.
- **Spec.** `social.md` §5–§8.

### D10. Moderation records state, not events

- **Decision.** One record per (actor, scope, subject, action). At the highest rank present, the safest value wins. Only authorities in the current declaration count. Positive assertions (pins, frontpage and curated tiers, keeps) are bound to a CID. The fold fails closed.
- **Rationale.** Per-action records make undo a delete and give one mod-log line per action (C6). Rank plus safety-minimum does not depend on self-asserted time, unlike "latest wins" (X1). CID binding stops bait-and-switch edits (F20). Failing closed means spam is never resurrected because a moderator's PDS is down (F22, P7).
- **Amended.**
  - `#restrict` hides retroactively: a restricted capability hides every in-forum item of that kind by the user as `Hidden(Restricted)` (rule 4b). Item-level keeps override it and `until` lapses it (completeness P0.1).
  - `tag:` subjects have rank 0.
  - modAction updates and deletes go through the revision protocol.
  - Removing a moderator lapses their assertions at once, so `mods rm` runs `mod adopt` first; the CLI enforces the order.
  - Create-if-absent uses `putRecord{swapRecord: null}` (ADR 0005).
- **Spec.** `social.md` §4; `data-model.md` §6.7, §8.6.

### D11. Bans and locks resist backdating

- **Decision.** A ban hides all of the user's in-forum content. A post lock carries an explicit allow-list of the comments that stay visible.
- **Rationale.** Time is self-asserted, so "content after the ban" cannot be defined (F21). The late-arrival rule from the social draft was rejected: it needs a live observer, which cold-syncing clients are not.
- **Amended (completeness review, P0.2).** The forum-wide lock is cut from v1 and `#lock` on a forum uri is Reserved (P2): an allow-list of ≤ 5,000 cannot list a whole forum, and the rank rule stops an admin from locking a subject owned by the owner. Owners archive a forum by switching it to approval mode. A post lock is refused above 5,000 comments, and the moderator is told to hide instead.
- **Spec.** `social.md` §4.6, §4.8.

### D12. Time (corrected)

- **Decision.**

  ```
  t_claim = createdAt (unparseable → none);  t_tid = time of the record's own TID segment
  t_eff   = min_some(t_claim, t_tid)
  if t_obs is known and t_eff > t_obs + 5 min:  t_eff = t_obs, and the item is flagged future-dated
  lower(r) = t_rank(parent, or root if no parent)   for a comment
           = decl.createdAt                         for a post, wikiRevision or sequence
  t_rank  = max(lower(r), t_eff)
  ```

  Only ranking and display depend on time.
- **Correction.** The synthesis's D12 read `t_rank = max(causal lower bound, min(createdAt, TID, firstSeen + 5 min))`. That puts an observer term into every item's time, so honest content would rank differently on different clients, contradicting synthesis §4.2 and P4. The completeness review keeps §4.2: the first-seen time only *clamps* items that claim to be more than 5 minutes in the future.
- **Rationale.** Honest content (claimed time ≤ first seen + 5 min) gets the same `t_rank` everywhere. Backdating only hurts its author, because the causal lower bound stops a reply from ranking before its parent. Future-dated items are badged, not hidden (X8): hiding them would create a scheduled-reveal channel and hide them from moderators too.
- **Spec.** `social.md` §2.

### D13. Edits

- **Decision.** An edit is `applyWrites[create revision (the verbatim prior value), update target]` with `swapCommit` and a CID-aware automatic retry. Authors can redact revisions.
- **Rationale.** `applyWrites` supports only a batch-level `swapCommit`, so any unrelated write to the repo (a Bluesky like, our own vote, another device) would look like a conflict without a CID-aware retry (H1, C10). Verbatim snapshots keep hash verification and quote anchoring (C9, X5). Redaction exists because public history otherwise defeats privacy edits (F30).
- **Amended.**
  - The outbox is serialised per account, with one write in flight, and every `swapCommit` comes from a fresh `getLatestCommit`, never the cache (completeness P0.7).
  - modAction updates and deletes use this protocol too (D10).
  - Deletes of more than 200 ops are chunked with the target in the first chunk, so a partial failure leaves only orphaned revisions.
  - ADR 0005: create-if-absent via `putRecord{swapRecord: null}`; `InvalidSwap` is the only conflict signal; an HTTP 500 on a write is ambiguous and re-read, never resent blindly; no-op updates are skipped.
- **Spec.** `data-model.md` §8; `sync.md` §7.

### D14. LFM/1 markdown

- **Decision.** CommonMark plus GFM tables, strikethrough, task lists, footnotes and alerts; math; spoiler and details blocks; figure captions. No raw HTML. Media is referenced with `cid:` URLs resolved only against the body's own `media`. External images are never fetched automatically.
- **Rationale.** These are the ForumMagnum features people use, and the profile freezes at release (product critique §3). No raw HTML removes the largest XSS surface. `cid:` scoping and no automatic external images stop tracking pixels (F5, F29).
- **Amended.** ADR 0002: math-core `=0.9.0` with a mandatory pre-scan (an extended list of disabled macros) and a MathML normaliser; Temml is a contingency only. `lfm-1.md` separates `lfm/1` semantics (frozen) from presentation (versioned by `RENDERER_VERSION`).
- **Spec.** `lfm-1.md`.

### D15. One renderer

- **Decision.** Rust produces HTML+MathML for the SPA and for export, and sanitised `TermText` styled lines for the TUI and CLI.
- **Rationale.** One implementation gives byte-identical output (P4, P9) and one sanitiser contract, checked by two independent sanitisers (DOMPurify in the SPA, ammonia natively). `TermText` closes terminal escape injection (F1).
- **Amended.** ADR 0002: ammonia stays native-only, as the test-time equality oracle (it would add 266 KB gz to wasm).
- **Spec.** `lfm-1.md` §8–§9.

### D16. Media

- **Decision.** Metadata is stripped by allowlist rebuild; images are physically rotated and downscaled; every fetch is checked against its CID; media loads automatically only under a privacy policy.
- **Rationale.** Uploads leak location and device data (F33); media on self-hosted PDSes acts as a tracking pixel and prefetching stores other people's content on readers' disks (F29).
- **Amended.**
  - Blobs stream through `Host::fetch_blob`: in JS, streamed and hashed with `crypto.subtle.digest`; natively, streamed to a file while hashing. Blob bytes never pass through wasm linear memory (completeness P0.4).
  - Declared MIME types, audio included, are normalised before upload, so `.ogg` and `.m4a` stay within `blob:audio/*`.
  - The blob object returned by `uploadBlob` is embedded verbatim (M5); ADR 0005 confirmed that nested refs keep blobs alive and that deleting the last reference collects them.
- **Spec.** `sync.md` §11; `data-model.md` §8.8.

### D17. Authentication

- **Decision.** OAuth everywhere: loopback or hosted-native metadata for the CLI, a web client for the SPA, and for `lessdugin web` the CLI holds the session. App passwords are accepted; main passwords are refused. `transition:generic` is never requested.
- **Rationale.** Narrow scopes and no main passwords contain the damage of any bug (F25). Every lessdugin client is a public client with sessions of at most about 14 days (M13), so headless use takes app passwords. `lessdugin web` is the path that trusts no website.
- **Amended.**
  - Login check (completeness review, P2): take the PDS from the DID document of `sub`, fetch that PDS's `/.well-known/oauth-protected-resource`, and require its `authorization_servers` to contain the token's `iss`. ("sub, PDS and issuer agree" fails for every bsky.social user, whose issuer is the entryway.)
  - The SPA forces `response_mode=query` and handles the callback before the hash router mounts.
  - The default scope is `include:NS.authFull` plus enumerated blob scopes, with the enumerated `repo?collection=…` fallback chosen at PAR time (S8).
- **Spec.** `data-model.md` §10; `sync.md` §3.6; `threat-model.md` §5.8.

### D18. Rust layout

- **Decision.** The wasm-safe crates `atp`, `lexicon`, `markdown` and `core` sit over a `Host` trait. jacquard is used only behind the native OAuth facade.
- **Rationale.** One core for every client (P9); jacquard is alpha, so it is confined and replaceable.
- **Amended.** Types are hand-written with a data-driven validator, so the digit-segment module mangling (M4) is moot. ADR 0002 adds size-hygiene rules for the wasm-safe crates.
- **Spec.** `engine-api.md`; `sync.md` §12.

### D19. SPA

- **Decision.** React, Vite and TypeScript. The wasm engine runs in a Web Worker. Hash routing. Strict CSP plus Trusted Types.
- **Rationale.** React and Vite are the user's choice. A worker keeps sync and the fold off the UI thread. Hash routes work on any static host without rewrites. CSP, Trusted Types and two sanitiser layers answer F5–F8.
- **Amended (completeness review, P2).** A `<meta>` CSP ignores `frame-ancestors`, and GitHub Pages cannot send headers, so the preferred hosts are Cloudflare Pages or Netlify (CSP as a header), with GitHub Pages as the fallback. `threat-model.md` adds a third Trusted Types policy for the worker URL.
- **Spec.** `threat-model.md` §6, §8.

### D20. Indexer

- **Decision.** The same engine plus `NS.index.*` XRPC. It returns uri + cid with hint values, and verifiable proof CARs.
- **Rationale.** Browser cold starts above about 300 participants need help, and the help must not need trust (P2, P3).
- **Amended (completeness review, cuts).** The first pass serves `describeIndexer`, `listForums`, `listParticipants`, `getRepoHeads`, `getChanges`, `listPosts`, `listRecentDiscussion`, `getThread` and `getRecordProofs`. Search (fts5), activity, karma, backlinks and automatic demotion are P2. Its proofs are subject to the currency rule (D6).
- **Spec.** `data-model.md` §11; `sync.md` §15.

### D21. Licences

- **Decision.** Applications are AGPL-3.0-or-later. Lexicons, spec and test vectors are CC0-1.0. The `atp` and `lexicon` crates are MIT OR Apache-2.0. No dependency we need is incompatible with the AGPL.
- **Rationale.** The AGPL keeps hosted forks of the SPA and indexer open (the AGPL §13 source link). CC0 lets anyone implement a compatible client without conditions. Permissive protocol crates encourage reuse in the wider atproto ecosystem.
- **Open.** The owner still has to confirm MIT OR Apache-2.0 for `atp` and `lexicon` (`docs/plan.md` §8, U6).
- **Spec.** `docs/spec/README.md` §6.

---

### Completeness-review fixes, by decision

| Fix | Decision | Spec |
|---|---|---|
| `#restrict` hides retroactively (rule 4b) | D10 | `social.md` §4.7, V5 |
| Forum-wide lock cut and reserved; lock refused above 5,000 comments | D11 | `social.md` §4.8; `data-model.md` MA9 |
| Stale third-party proofs excluded unless rev ≥ PDS head | D6, D20 | `sync.md` §1.2 |
| Streaming `Host::fetch_blob` | D16 | `sync.md` §12.2 |
| Normative fetch caps (first N keys descending; opinion 100k, others 20k) | D4, D7 | `data-model.md` §7.4; `sync.md` §5.4 |
| Title required unless quickTake; link required for `kind=link` | D5 | `data-model.md` PO3–PO5 |
| Outbox serialised per account, fresh `getLatestCommit` swap head | D13 | `sync.md` §7.1–§7.2 |
| D12 corrected to the clamp | D12 | `social.md` §2 |
| modAction updates and deletes via revisions | D10, D13 | `data-model.md` §8.3–§8.6; `social.md` §4.12 |
| `mods rm` adopts first | D10 | `social.md` §4.10 |
| `tag:` subjects rank 0 | D10 | `social.md` §4.1 |
| `establishSeeds` approval remark deleted; sybil theorem restricted | D9 | `social.md` §3.2, §6.3 |
| Clients search with the core in-memory index; fts5 only in the indexer | D20 | `data-model.md` §11.3; `sync.md` §2.2 |
| M4 module-name mangling dropped | D18 | – |
| Login issuer check via protected-resource metadata | D17 | `sync.md` §3.6; `data-model.md` §10.5 |
| Jetstream identity/account noise dropped; browser DID filter | D7 | `sync.md` §6.2–§6.4 |
| Deletes chunked with the target first | D13 | `data-model.md` §8.7; `sync.md` §7.2 |
| `revision.record` is `unknown` with a mandatory `$type` | D3 | `data-model.md` §6.5 |
| Audio MIME normalisation | D16 | `data-model.md` §8.8 |
| `response_mode=query` | D17 | `data-model.md` §10.5 |
| CSP as a header (Cloudflare Pages/Netlify preferred) | D19 | `threat-model.md` §6, §8 |
| `membership` defaults to approval; open mode labelled | D1, D3 | `data-model.md` §6.1; `sync.md` §2.2 |
| Forum succession guidance; `forum.successor` reserved | D1 | `social.md` §4.1; `data-model.md` §6.1 |
| Followed forums seeded from own membership records | D1 | `sync.md` §4.8 |
| Pingbacks computed from rendered links | D14 | `data-model.md` §5.4; `social.md` §16 |
| Explicit ForumMagnum deferrals | – | `docs/plan.md` §6.5 |
| `.temp` data orphaned; `migrate-temp` | D2 | `data-model.md` §1.3 |
| Magic cutoff affects only magic | D8 | `social.md` §12 |
| Wiki: linear history; deleted parent → flagged root; forks, merges, `aliasOf` cut | – (X12) | `social.md` §13 |
| Indexer reduced to the first-pass methods; other cuts | D20 | `docs/plan.md` §6.1 |

---

## Resolved critiques

The decision column is the synthesis's (§10). "Since:" notes record later amendments.

### Security critique (F\*)

| ID | Finding | Decision |
|---|---|---|
| F1 | Terminal escape injection wherever network text reaches a TTY | **Accept.** `TermText` everywhere, OSC 8 policy, JSON-on-TTY escaping, escape corpus. |
| F2 | Editor-template injection | **Accept.** |
| F3, F4 | Local-file exfiltration through draft image paths; attachment download and open | **Accept.** |
| F5 | SPA CSP and HTML contract gaps | **Accept:** `img-src 'self' blob:`, alignment classes, no inline SVG, `data-anchor`, ordinal footnote ids, `dir=auto`. |
| F6 | TeX and MathML | **Accept.** |
| F7 | URL fields outside markdown bypass the sanitiser | **Accept** (`safe_href` + `<SafeLink>`). |
| F8 | Poison-pill records cause permanent crash loops | **Accept** (iterative walkers, pre-parse limits, quarantine). |
| F9 | Display-name, link and forum impersonation; moderators listed without consent | **Accept**, with one change: the moderator role is *displayed* only if that moderator has a membership record (no separate acceptance record). `movedTo` is dropped from v1. |
| F10 | Native plain-DNS handle spoofing | **Accept:** DoH by default on native too, TOFU auth server, DID login. |
| F11 | Stale DID document plus a rotated key lets third parties forge `[verified]` | **Accept.** |
| F12 | – | (No finding has this number in the critique.) |
| F13 | Hint content is displayed by default | **Accept** with no exception in the MVP. Indexer content becomes displayable only through proofs. |
| F14 | Resource exhaustion from lying PDSes and huge repos | **Accept** (`limits.rs`). |
| F15 | Equivocation and cloaking | **Partial.** Record and compare commits; a warning in the MVP; evidence actions in P2. *Since:* the completeness review deferred equivocation recording itself to P2. |
| F16, F17 | Fan-out and reflection amplification; indexer specifics | **Accept.** |
| F18 | `karma/1` has no seeds | **Accept:** seeded `karma/1`, `establishSeeds=2`. Seeds include *granted* members, which are explicit human grants. |
| F19 | The automatic mod sweep turns discovered DIDs into seeds | **Accept:** no automatic sweep; explicit grants only. The per-moderator grant cap is **rejected** for v1: removing a moderator lapses every grant they made. |
| F20 | Bait-and-switch through edits | **Accept** (CID-bound positive assertions). |
| F21 | Backdating bypasses bans, locks and rate limits | **Accept:** a ban hides everything; locks carry allow-lists; rate limits are advisory and deferred. |
| F22 | Raid economics and fail-open on moderator outage | **Partial.** Fail closed accepted. Bulk actions deferred, with the rkey form reserved. `modMirrors` deferred. |
| F23 | React spam | **Accept** (one opinion, ≤ 32 reacts, dedupe). |
| F24 | Miscellaneous | **Accept:** canonical uris; deleted-subject rule with ≥ 2 E voters; wiki walk cap; JS `Map`. Window disagreement is moot because the MVP has no windows. |
| F25–F28 | Scope over-grant and main-password login; exportable browser DPoP keys; `lessdugin web` origin squatting; native token hygiene | **Accept** (blob scopes enumerated, so no `*/*`). |
| F29–F31 | Reader IP leaks and illegal-content prefetch via media; public revisions defeat redaction; deleted content persists | **Accept.** |
| F32 | Reports and moderation reasons | **Accept** for the P2 design; `showReason` rename accepted now. |
| F33 | Gaps in metadata stripping | **Accept** (box neutralisation instead of rewriting). |
| F34 | Public social-graph UX | **Accept** (interstitials in the MVP; the rest in P2). |
| F35, F36 | The hosted SPA and namespace control are a single point of failure; npm, cargo and Actions hygiene | **Accept.** *Since:* `verify-site` is deferred to P2 (completeness review cuts). |

### Protocol critique (H\*, M\*, low items, X\*)

| ID | Finding | Decision |
|---|---|---|
| H1 | Edits with `swapRecord` cannot work; `applyWrites` has no upsert | **Accept:** `swapCommit` with CID-aware retry; no upsert, so create-if-absent uses `createRecord`. *Since:* ADR 0005 replaced `createRecord` with `putRecord{swapRecord: null}`, whose failure is typed. |
| H2 | `blob:image/* blob:video/*` blocks text blobs, audio and attachments | **Accept the intent:** blob scopes are enumerated to cover every type we upload, and every variant begins with `atproto`. |
| H3 | "Immutable" revision and wiki rules can be broken by the authors of ancestor records | **Accept:** ancestry by uri; tag discussion anchors to (forum, slug) in P2; revision history is "verifiable, not complete". |
| H4 | Many fields have no limits, and limits can never be added later | **Accept:** every field has a final limit. |
| H5 | "Skip the repo if its rev is unchanged" fails, because `app.bsky` writes change the rev | **Accept:** Jetstream as the dirty signal; `getRepo?since` dropped except for export. |
| H6 | The newest-first fallback does not work with `{fk}-{tid}` keys | **Accept:** runtime probe plus fallback. |
| H7 | Windowing by author-chosen TIDs creates a moderation blind spot | **Accept:** comment key `{fk}-{rootTid}-{tid}`. |
| M1–M3 | `format=car` cannot be one method; GET array parameters too large; `forum.checkpoint` name collision | **Accept:** separate CAR method; array limits 25 and 50; no `forum.checkpoint`. |
| M4 | The digit segment needs module-name mangling in codegen | **Accept:** mangle the module name `_7778777`; tooling covered by S4. *Since:* moot, because types are hand-written (completeness review); ADR 0003 confirmed the tooling. |
| M5–M14 | M5 blob refs must be taken verbatim from `uploadBlob`; M6 proofs fail after key rotation; M7 the SPA cannot build link cards without a proxy; M8 the report-service scope needs `aud=*`; M9 handle DNS versus HTTPS disagreement; M10 a merged opinion record needs a swap on write; M11 OAuth callback handling and web client metadata; M12 the size budget can be exceeded; M13 OAuth sessions are short for every client; M14 publishing and testing facts that follow from permission-set rules | **Accept.** |
| Low items | Indexer service on a did:plc; naming-rule status; PLC limits undocumented; Jetstream identity/account events unfiltered; relay coverage; one NSID per `listReposByCollection`; optional `commit` output; `getBlocks` all-or-nothing; devnet CSP; interop dual-publish scope | **Accept all** (indexer on did:web, PLC pacing, optional commit output, `getBlocks` batches ≤ 50, dev CSP, checking granted scopes before interop writes). |
| X1–X18 | Contradictions between the four designs | **Accept the stated resolutions**, with these refinements: X2, the hash domain uses the short collection name and fk is 60 bits. X4, per-member `#access` grants plus mutual consent. X6, the MVP uses `pds` level plus proofs for indexer content and on demand, not mandatory proofs for moderation inputs (TLS from the author's PDS gives the same trust when the PDS holds the key). X8, future-dated items are badged, not hidden. X18, author moderation uses the same `modAction` collection with `scope=author`. |

The X items individually (informative; the outcome column applies the refinements above and later amendments):

| ID | Contradiction | Outcome |
|---|---|---|
| X1 | Record model | One `opinion` per (voter, subject); per-action `modAction` with rank and safety-minimum (D3, D10) |
| X2 | Four derived-rkey schemes | `{fk}-` locality prefix, domain-separated `h16` with the short collection name, 60-bit fk, key type `any` validated by core; no forum-less sentinel (D4, D5) |
| X3 | Indexer namespace | `NS.index.*`, own TXT record, separate CAR method (D2, D20) |
| X4 | Membership and roster | Own membership record plus per-member `#access` grants; roster pages possible later as compaction |
| X5 | Revision records | `{subject strongRef, op, record}` with `{P}-{tid}` keys; comments get revisions too (D13) |
| X6 | Verification and the sync primitive | `pds` level by default, proofs for third-party content and on demand; range sync in P7 (D6, D7) |
| X7 | Karma | Seeded staged `karma/1` (D9) |
| X8 | Time | `t_rank` with the future clamp; badged, not hidden (D12) |
| X9 | Vote values | knownValues strings |
| X10 | Forum declaration | One merged declaration with final limits |
| X11 | Comment fields | `root`, `parent`, `moderatorHat` |
| X12 | Wiki | DAG schema (`parents[] ≤ 8`) with the H3 fix. *Since:* the MVP uses linear history; forks, merges and `aliasOf` are P2. |
| X13 | Spill thresholds | Spill at 64 KiB, inline maximum 100,000 bytes |
| X14 | Scope chain | `include:` → enumerated `repo?collection=…`. The critique's last resort, `transition:generic`, is never requested (F25, D17). |
| X15 | Open-mode discovery | Constellation first, relay as fallback |
| X16 | Reports | age v1 with X25519 anonymous recipients, no forum or subject fields (P2) |
| X17 | Identity details | DoH-first handle resolution, handle cache 6 h and DID documents 24 h; the core re-checks the session's authorization-server binding (D17) |
| X18 | Author moderation storage | `modAction` with `scope=author` |

### Product critique (C\* and sections)

| ID | Finding | Decision |
|---|---|---|
| C1 | Interaction record granularity | **Accept** (`opinion`). |
| C2 | Derived rkey function | **Partial.** One domain-separated hash function plus the fk prefix. The `stid` segment is **kept**, contrary to the product critique, because rkeys are permanent and it enables P7 windowing at no MVP cost. |
| C3–C5 | Post/comment rkey type; roster; membership modes | **Accept.** |
| C6 | Mod record shape and conflict rules | **Accept** (per-action records with rank and safety-min). |
| C7–C10 | Karma; time; revision snapshot; edit concurrency | **Accept.** |
| C11 | Markdown profile, id prefix, math engine | **Accept:** `lfm/1`, `md-` id prefix, math engine decided by S10. |
| C12 | MVP sync and store | **Accept.** |
| C13 | Indexer API | **Accept**, adding `getRecordProofs` to the MVP. |
| C14 | Blob scope | **Accept** (enumerated blob scopes). |
| C15 | Feature phases | **Accept** (synthesis §8; now `docs/plan.md` §6). |
| C16–C18 | Posts with no forum; names and glossary; crates and codegen | **Accept.** |
| §2 cuts | Over-engineering to cut or defer | **Accept**, except that these stay: minimal author moderation (core ForumMagnum behaviour, cheap with a unified collection), `discovery.indexers`, `post.labels` (safety), and `excerpt` (only when the body is spilled). In-memory indices and hand-written types with a runtime validator: **accept**. |
| §3 must-haves | Missing ForumMagnum must-haves for the MVP | **Accept all.** |
| §4 CLI | CLI first-run friction and fixes | **Accept all.** |
| §5 | TUI and SPA gaps | **Accept all.** |
| §6 | Implementation risks | **Accept** `.temp` dogfooding. Multi-tab: **choose** Web Locks leader election plus per-repo locks rather than independent per-tab workers. |
| §7, §8 | MVP cut-line; phase order | **Accept**, merged into synthesis §8 (now `docs/plan.md`). |

### Earlier design drafts

| Draft position | Decision |
|---|---|
| Sync design: proofs and range sync by default in the MVP | **Defer to P7.** Keep its key layout, verification code, scheduler concepts and cost model. |
| Sync design: three profiles × three verification levels | **Reduce** to `sources` × `verify`. |
| Sync design: roster pages and mod sweep | **Reject for v1** (could later be added as compaction). |
| Architecture design: KV-only indices, own lexicon codegen, `transition:generic`, `blob:image/* blob:video/*` only, 16 KB revision blobs, comments limited to `editedAt` | **Reject.** |
| Protocol design: forum-less posts, sections, author-scope mod actions without `forum`, integer vote values, unseeded karma, latest-time-wins for tier and section | **Reject.** |
| Social design: the late-arrival rule | **Reject** (needs a live observer; replaced by allow-lists and ban-hides-all). |
| Social design: the remaining dimensions (`readonly`, `votesVoid`, `official`, `acceptedAnswer`) | **Defer.** |

---

## Consequences

- `docs/spec/` implements these decisions; each D-entry names its sections. A spec change that contradicts an entry here needs a new ADR that amends this one.
- Behaviour IDs named in D8 are frozen at release 0.1 (M7). Before that they change only together with their vectors (`docs/spec/README.md` §3).
- The phases and acceptance criteria that follow from these decisions are in `docs/plan.md`.
- Open: D21's crate licences (owner confirmation).

## Decisions made while writing

1. **Each decision is recorded as amended,** with the original synthesis wording kept only where it was corrected (D12) or extended (D8). The amendments come from the completeness review, the approved plan and ADRs 0002–0006.
2. **D12's correction** follows synthesis §4.2 exactly, as the completeness review directs, using the precise form of `social.md` §2.
3. **F12** does not exist in the security critique (its numbering skips from F11 to F13); the row is kept so the ID sequence is visibly complete.
4. **"Finding" titles** were added to the critique tables, taken from the critique headings, so the IDs are readable without the archive. The decision column is the synthesis's text, with later amendments marked "Since:".
5. **X14** is accepted except for its last resort: `transition:generic` is never requested, as D17 and F25 require. The synthesis's blanket "accept the stated resolutions" is read with that exception.
6. **X17's cache durations** are the synthesis's (handle 6 h, DID document 24 h), not the critique's suggestion.
7. **The principle list P1–P10** is included as context because the rationales cite it; it is copied from synthesis §1.1 without change.
