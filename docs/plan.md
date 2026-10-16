<!-- SPDX-License-Identifier: CC0-1.0 -->

# lessdugin implementation plan (first pass, release 0.1)

**Status:** approved; execution started at M0. This is a living document: milestone status and spike results are updated in place, and a change of scope needs an ADR (`docs/adr/0000-template.md`).
**Sources:** the approved plan, `docs/design/synthesis-v1.md` §8 and §9, every fix and cut from `docs/design/completeness-review.md`, and the accepted spike ADRs 0002–0006. Where the approved plan and the synthesis disagree, the approved plan wins.
**Scope of this document.** It is normative for the *scope, order and acceptance* of the first pass. It is **not** normative for behaviour: `docs/spec/` is (`docs/spec/README.md`). Where this plan summarises behaviour and differs from the spec, the spec wins and this plan is corrected. The decisions behind the plan (D1–D21) are recorded in `docs/adr/0001-decisions-v1.md`.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

---

## 0. Vocabulary

| Term | Meaning |
|---|---|
| **M0–M7** | Milestones of the first pass, ending in release 0.1. M0–M6 run without the user. M7 needs the user and is never started automatically. |
| **P2, P7, P8+** | Later phases (§6). In the spec, **"Reserved (Pn)"** marks a name or behaviour that phase Pn will define. Until then v1 clients MUST NOT write it, and MUST give it no effect when they read it. |
| **Never** | Out of scope for the product (§6.4). |
| **S1–S12** | Spikes (§5). Each one is written up as an ADR. |
| **[auto]** | An acceptance criterion that runs in CI, or against the devnet or FakePds, without the user. It gates its milestone. |
| **[user]** | An acceptance criterion that needs the user: real accounts, a device, DNS or publishing. It is built and run against the devnet first, then recorded as **pending user run** (§8). It does not gate M0–M6. Every [user] item gates M7. |
| **[target]** | A measured number that is reported (in CI or in the milestone's ADR) but does not gate. |

"P0–P3" in `sync.md` are dirty-queue priorities, and "P1–P9" in `social.md` are fold properties. Neither is a phase. `docs/spec/glossary.md` §2 lists every overloaded name.

---

## 1. Goal, constraints and user decisions

**Goal.** A ForumMagnum-class forum: posts, threaded comments, karma and agreement voting, reacts, tags and wiki, sequences, and moderation. All content lives in its authors' own atproto PDS repos.

**Constraints**
- The core works with nothing but PDSes, PLC/did:web and DNS over HTTPS: no lessdugin server, no extra database, no trusted website.
- It is usable from the command line.
- Optional, verifiable extensions (the indexer, live updates) make the experience richer without changing what anyone sees.
- Rust and TypeScript. Three clients: CLI, TUI and SPA.

**User decisions**
- The NSID authority is `lessdugin.7778777.online`, so `NS = online.7778777.lessdugin`.
- The Rust core is shared with the SPA through wasm. The SPA uses React and Vite.
- The first pass is the MVP in all three clients, plus the optional indexer.
- Licences: applications AGPL-3.0-or-later; lexicons, spec and test vectors CC0-1.0; the `atp` and `lexicon` crates MIT OR Apache-2.0 (the owner still has to confirm this, D21).
- Cadence: run M0 to M6 continuously, committing and pushing after each milestone to the working branch. Stop before M7, which publishes lexicons and DNS records.

---

## 2. How milestones run

### 2.1 Execution

1. Each milestone runs as a workflow: parallel agents on independent crates or packages, then an adversarial review, then a fix pass, then `cargo xtask ci` and the e2e workflow, then commit and push.
2. Order: M0 → M1 → M2 → M3. M3 freezes engine API v0 (`dto/1`, `docs/spec/engine-api.md`). After M3, M4, M5 and M6 MAY overlap; each still ends with its own green gate and its own commit.
3. M7 starts only on the user's explicit instruction.

### 2.2 Definition of done (every milestone)

A milestone is done when all of the following hold:
1. Every [auto] criterion of the milestone passes.
2. CI is green: `ci.yml` and `e2e.yml` with every job that exists at that point (§4.3).
3. Every behaviour change lands in the same commit as its spec change and its re-blessed vectors (`cargo xtask vectors --bless`, with the diff reviewed). Reversing a decision recorded in ADR 0001 or in a spike ADR needs a new ADR.
4. Nothing writes the final `NS`. Every build writes `NS.temp` (`data-model.md` §1.3). Nothing writes to a production PDS, except the [user] smoke tests on throwaway accounts.
5. The milestone's [user] items are listed in §8 as "pending user run", and in the milestone's commit message.
6. After M3, the engine API changes only compatibly (`engine-api.md` §1: new optional fields and new enum variants).

### 2.3 Overview

| M | Name | Needs | Freezes or delivers | Needs the user |
|---|---|---|---|---|
| M0 | Spec, scaffold, spikes | – | spec frozen for `.temp`; draft lexicons; spike ADRs | no ([user] spikes recorded as pending) |
| M1 | Protocol foundations | M0 | atp, lexicon, identity, native host, auth | no ([user] S1) |
| M2 | Walking skeleton (text) | M1 | store, sync, outbox, three clients end to end | no ([user] smoke test) |
| M3 | Social core | M2 | `mod/1`, `karma/1`, `magic/1`, `default/1`; engine API v0 frozen | no |
| M4 | Rich content and media | M3 | full `lfm/1`; media pipeline | no |
| M5 | Organisation and discovery | M3 | tags, wiki, sequences, search, live updates, open-mode discovery | no ([user] production logins) |
| M6 | Optional indexer | M3 | `lessdugin-indexer` | no |
| M7 | Release 0.1 | M4, M5, M6 | published lexicons, DNS, site, binaries | **yes** |

---

## 3. Milestones

### M0 — Spec, scaffold, spikes

**Scope**
1. Commit the spec in `docs/spec/` (`README.md`, `data-model.md`, `lfm-1.md`, `social.md`, `sync.md`, `engine-api.md`, `threat-model.md`, `limits.md`, `glossary.md`, `reacts-default-1.json`) with every completeness fix applied. Commit this plan, ADR 0000 (template), ADR 0001 (decision log) and one ADR per spike.
2. Draft the lexicons with their **final** NSIDs in `lexicons/online/7778777/lessdugin/`: the ten records, `defs`, `markdown`, `authFull`, `authInteract` and `index/*`. Every limit is final. `membership` defaults to `approval`. `revision.record` has type `unknown` with a mandatory `$type`. The code maps the prefix to `NS.temp`.
3. Generate the key vectors `testdata/vectors/keys/*.json` for `fk/1` and `sk/1`. The draft sk/1 vectors are superseded and MUST NOT be used (`data-model.md` §3.4).
4. Workspace and toolchain: Rust 1.97 with `wasm32-unknown-unknown`; `wasm-bindgen-cli =0.2.129`; binaryen; the pnpm workspace; `[workspace.dependencies]` with exact `=` pins for the parsing and crypto crates and the pins decided in ADR 0002.
5. CI skeleton: `ci.yml` and `e2e.yml`. `release.yml` and `pages.yml` are prepared but used only at M7. `clippy.toml` bans clocks and sleeps in the wasm-safe crates; `deny.toml` holds the licence allowlist.
6. The devnet script `packages/e2e/devnet.mjs` (S5).
7. Every spike S1–S12 (§5). Spikes that need real accounts or devices are built and run against the devnet, and recorded as pending user run.

**Acceptance**
- [auto] Every spike has an ADR. Its status is Accepted, or Accepted with a "pending user run" section.
- [auto] `cargo xtask ci` is green.
- [auto] The devnet boots 3 accounts in under 30 s. (ADR 0006 measured 0.9 s.)
- [auto] The draft lexicons pass `goat lex lint`.
- [auto] The spec is reviewed and frozen for `.temp`. From then on, changes follow §2.2 item 3.

### M1 — Protocol foundations

**Scope**
- **`lessdugin-atp`:** syntax types (Did, Handle, Nsid, AtUri, Rkey, Tid, Datetime) plus the Jetstream `NsidPattern` type (ADR 0003); JSON ⇄ DAG-CBOR; CID with our own base32-lower code (ADR 0002); a CARv1 reader with caps; the MST proof walk; commit verification (k256 and p256, low-S); did:key; DID documents; XRPC request and response structs.
- **`lessdugin-lexicon`:** hand-written owned serde structs with flattened unknown fields; a data-driven validator over the embedded lexicon JSON; `fk/1` and `sk/1` derivation and key grammars; the record validity rules of `data-model.md` §7, including the required fields per post kind; the `NS` constant and the `.temp` mapping. There is no lexicon code generation, so module-name mangling for the digit segment is moot.
- **Identity:** DoH (Cloudflare, then Google), the `.well-known` fallback, PLC and did:web documents, Slingshot as a hint, bidirectional handle verification, PLC pacing, account status, devnet mode (ADR 0006).
- **XRPC subset** (about 16 endpoints).
- **`NativeHost`:** reqwest (rustls) behind an SSRF guard, `SqliteKv`, WebSocket, keyring sessions, `fd-lock`.
- **Auth:**
  - app passwords (by prompt, stdin or environment variable, never argv; main passwords refused);
  - OAuth loopback through `jacquard-oauth =0.12.2` behind the facade;
  - hosted native client metadata;
  - `--no-browser`;
  - refresh under an exclusive lock;
  - the login binding: take the PDS from the DID document of `sub`, fetch that PDS's `/.well-known/oauth-protected-resource`, and require its `authorization_servers` to contain the token's `iss` (`sync.md` §3.6);
  - a trust-on-first-use pin of the authorization server.
- `TermText`, `term_sanitize` and `safe_href`, shared with the M4 renderer.
- **CLI:** `login`, `logout`, `whoami`, `accounts`, `resolve`, `verify`, `doctor`.

**Acceptance**
- [auto] Interop vectors pass on native and wasm32. These are the vendored atproto-interop-tests (syntax, including `online.7778777.lessdugin.*` cases and the NSID and pattern vectors of ADR 0003; data model JSON ⇄ CBOR ⇄ CID; MST; signatures, with high-S rejected; did:key), plus the `keys` and `lexicon` vectors.
- [auto] `lessdugin verify at://<a real public bsky record>` reports `proven`, with the key source. Tampered fixtures fail: a changed block, a wrong DID, a bad or high-S signature, a future rev, unsorted keys, a wrong MST layer, depth overflow. (This needs read-only network access, no account.)
- [auto] Login works on the devnet: app password, and OAuth wherever the devnet's http issuer is accepted.
- [auto] Concurrent refresh is safe: parallel CLI processes refreshing one session never invalidate it.
- [auto] The login binding accepts the entryway layout (issuer `bsky.social`, PDS on `*.host.bsky.network`) and rejects an issuer missing from `authorization_servers`. This is a FakePds test.
- [user] S1: login on bsky.social through loopback, hosted-native metadata and `--no-browser`, each with one consent screen. Until M7 the scope is the enumerated fallback, because `include:` needs the published permission set.
- [target] S9: the fixed-IP check of per-host read budgets (ADR 0004).

### M2 — Walking skeleton (text, all three clients)

**Scope**
- **Stores:** `KvStore` for SQLite, IndexedDB and memory; the store invariants I1–I9 (`sync.md` §10.2).
- **Sync:** L1 forum-range sync with the cursor-range probe and the full-listing fallback (ADR 0004). The normative fetch caps: the first N keys of each `{fk}-` range in **descending** rkey order, with N = 100,000 for `opinion` and 20,000 otherwise; excess records are excluded and the repo is marked `[partial]`. Tombstones only after a complete listing or `RecordNotFound`.
- **Outbox:**
  - durable, and serialised per account with one write in flight;
  - every `swapCommit` comes from a fresh `getLatestCommit`, never from the cache;
  - edits are `applyWrites[create revision, update target]` with a CID-aware retry;
  - create-if-absent is `putRecord{swapRecord: null}`, and an HTTP 500 is treated as ambiguous and re-read, never resent blindly (ADR 0005);
  - a user's first content in a forum is batched with their membership record;
  - deletes put the target in the first chunk;
  - no-op updates are skipped.
- **Content:** `forum create` (which always writes the membership mode explicitly; the default is `approval`), join, post, comment, edit with revision, delete, retract; threads; basic LFM (no math, no media).
- **Followed forums** are seeded from the user's own membership records, so they follow the user across devices.
- **CLI:** `read` (anonymous), `ls`, `post`, `comment`, `edit`, `delete`, `sync`, `join`, `forum *`.
- **TUI:** read, and reply through `$EDITOR`.
- **Wasm and engine package:** worker, `JsHost`, AuthBridge (the write allowlist), leader election (Web Locks plus BroadcastChannel).
- **SPA:** anonymous read, login and comment. Login forces `response_mode=query` and handles the callback before the hash router mounts. The CSP is sent as a header (`lessdugin web` and `_headers`).
- **`lessdugin web`** with the bridge: `/bridge/session`, `/bridge/xrpc`, `/bridge/login`, `/bridge/kv`. `/bridge/fetch` is Reserved (P2).

**Acceptance**
- [auto] The devnet parity scenario, text subset (§4.2), is byte-identical across CLI `--json`, SPA and TUI.
- [auto] A 50-member FakePds forum cold-syncs natively in under 5 s.
- [auto] No CSP or Trusted Types violations: any `securitypolicyviolation` fails Playwright.
- [auto] CI reports the wasm size. It fails above 1.1 MB gzipped (target 900 KB) and warns above 2.5 MB raw (ADR 0002).
- [auto] Sync and outbox scenarios on `MemHost` + FakePds pass: idempotent sync; cursor-range fallback (opaque cursors, cursors that ignore the bound); rollback; deletes only after confirmation; the outbox survives `kill -9`; `InvalidSwap` retry versus a real conflict; the FakePds behaviours listed in ADR 0005.
- [user] A production smoke test on `.temp` with throwaway bsky.social accounts.
- [target] The rest of S9: a cold sync of 200 real repos (read-only, a small opt-in sample) and PLC throughput at 20 requests/s.

### M3 — Social core

**Scope**
- **Opinions:** a 4 s undo and coalescing window; merging across devices (swap failure → re-read → re-apply the intent per axis).
- **`karma/1`:** K1 → E → KP → K with the per-pair cap. Scores, agreement, and unweighted counts ("n votes not yet counted").
- **`magic/1`:** post sorts magic, top, new, old and recentComments; comment sorts magic, top, new, old and latestReply; list filters frontpage (default), all and curated; Recent Discussion. The magic cutoff affects only the magic sort.
- **`mod/1`, forum tier:**
  - ranks, with only current authorities counting, and safety-min at the highest rank;
  - CID-bound positive assertions;
  - a ban hides all of the user's in-forum content, and item-level keeps override it;
  - `#restrict` hides retroactively (rule 4b, `Hidden(Restricted)`);
  - post locks with allow-lists. A lock is refused above 5,000 comments (the moderator is told to hide instead). The forum-wide lock is Reserved (P2);
  - `tag:` subjects have rank 0;
  - fail closed (`degraded`).
- **`mod/1`, the rest:** the author tier; modAction updates and removals through the revision protocol; mod queue, mod log and raw view; `mod adopt`; `mods rm`, which runs `mod adopt` first.
- **Participation:** approval and open modes, the closure, newcomer collapse, the grants UI. Open mode is labelled "relies on public relay infrastructure (Bluesky relay, Jetstream, Constellation)".
- **Notifications and unread state,** computed from first-seen times, never from `createdAt`.
- **UI:** public-action interstitials; the "recorded; will count once you're established" message.
- **Reacts:** the `default/1` palette, gates, against-reacts, anchors shown as a list (the margin UI is P2).
- **Engine API v0 (`dto/1`) is frozen.**

**Acceptance**
- [auto] All social vectors (time, modfold, karma, reacts, rank, thread, fold) and the property tests P1–P9 (`social.md` §6.3, §19) pass, and native equals wasm.
- [auto] With a moderator's PDS offline, no hidden spam reappears.
- [auto] Every MVP action is reachable in all three clients (in the TUI at least through the `:` command line).
- [auto] The parity scenario, social subset (§4.2), is byte-identical, in approval and open variants.
- [auto] CLI tests: a lock is refused above 5,000 allowed comments; `mods rm` runs `mod adopt` first.

### M4 — Rich content and media

**Scope**
- **Full LFM/1:** math (math-core `=0.9.0` with the pre-scan and the MathML normaliser, ADR 0002), footnotes and sidenotes, table of contents, alerts, spoilers, details, figure captions, hover previews, verified mentions.
- **Media pipeline in core,** so the CLI and the SPA behave identically: sniff and allowlist; strip metadata by allowlist rebuild; physically apply orientation; downscale (try 5 MiB first, fit on 413); compute the raw CID; upload at publish time and embed the returned reference verbatim; warn when alt text is missing. Declared MIME types, audio included, are normalised before upload (`data-model.md` §8.8).
- **Streaming `Host::fetch_blob`:** in JS the bytes are streamed and hashed with `crypto.subtle.digest`; natively they are streamed to a file while hashing with sha256. Blob bytes never enter wasm linear memory.
- Display and autoload policy; attachments (download cards saved as `<cid>.<ext>`); history view and redaction; the SPA editor (toolbar, paste and drop, live preview, mention autocomplete); TUI images (ratatui-image); `export` (default `--mine`).
- **Not in M4:** thumbnails (P2), the math font subset (P2), Temml (only the documented contingency of ADR 0002).

**Acceptance**
- [auto] At least 150 LFM vectors, plus the XSS, mXSS, terminal-escape and MathML corpora.
- [auto] `DOMPurify(html) === html` and `ammonia(html) == html` for every render vector.
- [auto] A 10-minute fuzz run (CAR, CBOR, MST, lexicon validator, renderer) without panics.
- [auto] EXIF, GPS and MP4 `udta`/`meta` are removed, with orientation preserved. Automatic downscaling stays under 5 MiB.
- [auto] A blob at the hard limit (300 MiB, `limits.md` §5) served by FakePds is fetched and CID-checked by the SPA engine without its bytes entering wasm linear memory, and by the CLI without being held in memory.
- [auto] The parity scenario, media subset (§4.2), is byte-identical.

### M5 — Organisation and discovery

**Scope**
- **Tags:** relevance; the wiki with **linear history** (the page head is the latest revision by an eligible author, or the moderator's `#tag.head`; a deleted parent makes its child a flagged root); the moderator tag actions `#tag{removed, head}`. Forks, merges and `aliasOf` are Reserved (P2); the `parents[]` schema stays.
- **Sequences:** library listing, next and previous, "continue reading" (local).
- **Search:** every client searches locally with the core's in-memory index. fts5 is for the indexer only (P2).
- **Forum directory** (relay, owners' listings, indexer).
- **Live updates** through Jetstream (ADR 0003): the v2 endpoint by default and the v1 wire for large DID filters; `identity`, `account` and `sync` events for unknown DIDs are dropped at once; the browser sets its DID filter once the participants are known.
- **Open-mode discovery** (Constellation, Jetstream, relay).
- **Pingbacks,** computed from the rendered bodies of synced posts (indexer backlinks are P2 hints).
- **Scopes:** `include:` with the PAR-time fallback to the enumerated scope.
- **Hosted SPA configuration:** web and native client metadata; `_headers` with the CSP for Cloudflare Pages or Netlify; the GitHub Pages fallback with a `<meta>` CSP and a JS frame guard (a `<meta>` CSP cannot carry `frame-ancestors`).
- Mobile layout.

**Acceptance**
- [auto] The open-mode devnet scenario passes with the mini-Jetstream (both wires, with identity and account noise for unrelated DIDs) and the fake Constellation (short pages that still carry a cursor).
- [auto] A cached forum renders in under 1.5 s.
- [auto] axe accessibility score ≥ 90.
- [auto] The full parity scenario (§4.2) is byte-identical, in approval and open mode, with `sources` direct and assisted.
- [user] Login on bsky.social through CLI loopback, CLI hosted-native, the hosted SPA (a preview deployment on the user's static host) and `lessdugin web`, each with one consent screen (S1, S2).
- [user] S11: session and draft persistence on Safari and iOS.

### M6 — Optional indexer

**Scope**
- **Ingest:** the Jetstream wildcard from two endpoints, with a 10 s cursor rewind; a backfill every 6 h (relay `listReposByCollection` per NSID, then range listing); a daily reconcile; `--poll <pds>` mode for the devnet and air-gapped setups; per-host politeness; a `sync.getRecord` proof per live event, and lazily for backfilled records.
- **XRPC, first pass:** `describeIndexer`, `listForums`, `listParticipants`, `getRepoHeads`, `getChanges`, `listPosts`, `listRecentDiscussion`, `getThread`, `getRecordProofs`. Reserved (P2): `searchPosts` (fts5), `listBacklinks`, `getActivity`, `getKarma`, `getTallies`, `getRecords`.
- **Client integration:** assisted mode; cold start through proofs under the currency rule (a third-party proof whose rev is below the author PDS's head is `[stale]` and stays out of the fold); proof fetches on Jetstream events; the manual `--audit` (checks A1–A5, `sync.md` §15.4). Automatic audit sampling and demotion are P2.
- Docker image (distroless, GHCR) and operator documentation.

**Acceptance**
- [auto] On the devnet, the indexer's view converges within 5 s of a write, and a restart resumes with no gaps.
- [auto] Clients produce identical views with and without the indexer.
- [auto] With the indexer, the SPA cold-paints a 2,000-participant synthetic forum in under 2 s.
- [auto] `--audit` flags an injected omission and a forged proof.
- [target] `listPosts` p95 under 50 ms at 200 requests/s on 1M records.

### M7 — Release 0.1 (needs the user; never run automatically)

**Scope**
- A security review pass: threat model, fuzzing, corpora. A final review of every lexicon limit (limits are frozen at publication).
- Switch `NS` from `.temp` to `online.7778777.lessdugin`.
- **Publisher account:** did:plc, handle `lessdugin.7778777.online`, an offline rotation key. It publishes one `com.atproto.lexicon.schema` record per NSID, and the permission sets, after `goat lex lint`, `goat lex breaking` and `goat lex check-dns` pass.
- **DNS:** TXT `_lexicon.lessdugin.7778777.online` and `_lexicon.index.lessdugin.7778777.online`, both `did=<publisher DID>`. Domain hardening: registrar lock, multi-year auto-renew, DNSSEC, CAA.
- **Site:** deploy the static site, preferably to Cloudflare Pages or Netlify (which send the CSP as a header), with GitHub Pages as the fallback. Enable `release.yml` and `pages.yml`.
- **Guides:** user guide; forum-owner guide, including succession (use a dedicated did:plc forum account whose rotation keys are held by at least two admins); indexer self-hosting guide; a Level-0 implementer guide (informative; the normative definition is `docs/spec/README.md` §4).
- **`.temp` data** is orphaned at 0.1. `xtask migrate-temp` copies a user's own records with the deterministic rewrite of `data-model.md` §1.3: keys and strongRef CIDs change, because fk and h16 hash at-uris that contain the collection NSID.

**Acceptance (all [user])**
- `goat lex check-dns` passes.
- `include:` works on bsky.social (S8).
- On a fresh machine: install from a release, log in, post. The post appears on the hosted SPA and in `lessdugin web`, with matching build hashes.
- Every earlier "pending user run" item (§8) is closed, or explicitly waived by the user.

---

## 4. Verification

### 4.1 Test layers

| Layer | How | Milestone |
|---|---|---|
| Protocol | Vendored atproto-interop-tests; tampered-proof fixtures; jacquard-repo and `@atproto/repo` as oracles; cargo-fuzz on CAR, CBOR, MST, the validator and the renderer | M1, fuzz M4 |
| Shared vectors | `testdata/vectors/<kind>/*.json` run by the Rust runner (testkit) and the TS runner (vitest over the wasm `vectors` build): native equals wasm (`docs/spec/README.md` §5) | from M0 (keys) |
| Social | Vectors plus property tests P1–P9 (proptest) | M3 |
| Markdown and terminal | ≥ 150 LFM/1 vectors; XSS, mXSS and escape corpora; DOMPurify and ammonia equality | M2 (basic), M4 |
| Sync and engine | `MemHost` + FakePds scenarios (fault injection, cursor modes, the ADR 0005 behaviours); a 5,000-repo load simulation against the budgets of `sync.md` §14; a synthetic 1k-post / 50k-opinion forum lists in under 300 ms warm | M2, M3 |
| CLI and TUI | `assert_cmd` + `insta` against FakePds; ratatui `TestBackend` snapshots at 80×24 and 160×48; a resize fuzz test; the plain `--color never` output | M1–M5 |
| End to end | `packages/e2e/devnet.mjs` (`@atproto/dev-env` `TestNetworkNoAppView`) plus the testkit mini-Jetstream and fake Constellation, or the indexer in poll mode. Rust e2e runs with `LESSDUGIN_E2E=1`. Playwright (Chromium from `/opt/pw-browsers`) does real OAuth against the devnet, or uses an e2e-only app-password build if http issuers are rejected. Any `securitypolicyviolation` fails; axe runs. | M2+ |
| Production smoke | Throwaway bsky.social accounts with `.temp` NSIDs, and optionally one self-hosted PDS. [user] only. | M2+, M7 |

### 4.2 Parity scenario

With `now` and `obs` injected, CLI `--json`, the SPA's `getThread`, the TUI snapshot and (from M6) indexer-hydrated output MUST match byte for byte.

| Step | Actor and client | Action | Subset |
|---|---|---|---|
| 1 | alice, CLI | creates a forum | text (M2) |
| 2 | bob, SPA (Playwright) | joins and posts (with an image from M4) | text (M2); media (M4) |
| 3 | carol, TUI | replies | text (M2) |
| 4 | carol, TUI | votes, agrees, reacts | social (M3) |
| 5 | a moderator, CLI | hides a comment and pins the post | social (M3) |
| 6 | bob, SPA | edits the post, which creates a revision | text (M2) |
| 7 | carol, TUI | tags the post; relevance applies | full (M5) |

Variants: approval and open mode (M3), `sources` direct and assisted (M5), indexer on and off (M6).

### 4.3 CI

| Workflow | Contents | Added |
|---|---|---|
| `ci.yml` (pull requests and pushes) | **rust:** fmt; clippy native and wasm32 (atp, lexicon, markdown, core, wasm); tests; doctests; lexicon conformance test (every lexicon property mapped to a struct field); `cargo deny`; `cargo vet`. **wasm:** `wasm-bindgen-cli =0.2.129` and binaryen (pinned); `xtask wasm --release --budget`. **web:** `pnpm i --frozen-lockfile`, biome, one TypeScript type-check job, vitest, `vite build`, lockfile-lint, osv-scanner. **lexicons:** pinned goat `lex lint`, and `lex breaking` against the merge base once a tag exists; `@atproto/lexicon` parse. | skeleton M0; jobs as their tooling lands (wasm budget M2) |
| `e2e.yml` (pull requests and nightly) | devnet, Rust e2e, Playwright; traces uploaded on failure | M0 (devnet boot), M2+ |
| `release.yml` (tags) | Linux x86_64/aarch64 musl, macOS arm64/x86_64 and Windows binaries with the embedded SPA; `SHA256SUMS`; build provenance attestation; `cargo auditable`; indexer image on GHCR; SPA dist tarball and hash | prepared M0, used M7 |
| `pages.yml` | tagged dist plus site files, deployed through a protected environment | prepared M0, used M7 |

Every GitHub Action is pinned by SHA, workflows run with `permissions: contents: read`, and `pull_request_target` is never used.

---

## 5. Spikes S1–S12

| ID | Question | Real accounts or devices? | Status | Consumer |
|---|---|---|---|---|
| S1 | `jacquard-oauth` loopback, hosted-native metadata and `--no-browser` against bsky.social; signed XRPC through the facade | **yes**: a bsky.social account. The devnet part (http issuer) runs in M1. | **pending user run** (ADR to be written). Fallback if it fails: our own PAR, PKCE and DPoP client (about 1.5k lines). | M1 |
| S2 | The browser OAuth library: non-extractable IndexedDB keys; `signIn(did)`; query and fragment callbacks (`response_mode=query` forced); OAuth against an http devnet PDS; loopback fallback for `lessdugin web` | **yes** for production authorization servers; the devnet part runs in M2 | **pending user run** (ADR to be written). Fallback: `@atcute/oauth-browser-client` with a store that keeps keys non-extractable. | M2, M5 |
| S3 | Wasm size with and without math-core and k256/p256 | no | **done**, ADR 0002 (the budget is realistic; do not split `verify.wasm` yet) | M2 |
| S4 | The digit NSID through goat, `@atproto/lexicon`, the Jetstream wildcard, relay `listReposByCollection` and Constellation `source=`; `validate`-unset writes containing union-like values and nested blob refs; blobs referenced only by a revision surviving GC | no (read-only public services, and the devnet) | **done**, ADR 0003 (tooling, discovery) and ADR 0005 (writes, blobs). Remaining: `goat lex lint` of the full lexicon set with `revision.record: unknown` (M0 CI); audio MIME acceptance on a production PDS ([user], M4). | M0, M2 |
| S5 | The devnet in GitHub Actions | no | **done**, ADR 0006 | M0 |
| S6 | Jetstream wildcard plus rkey-prefix filtering | no | **done**, ADR 0003 | M5 |
| S7 | `listRecords` cursor-range behaviour on bsky.social and non-reference PDSes | no (read-only) | **done**, ADR 0004 (reference PDS, Bluesky production, rsky-pds) | M2 |
| S8 | Detecting `include:` failure at PAR | **yes**: a bsky.social account, and the published permission set for the success path (M7). The devnet shows only the failure path, because dev-env cannot resolve `include:`. | **pending user run** | M5, M7 |
| S9 | PDS read limits per host versus shared; PLC throughput at 20 requests/s | no account; needs a fixed egress IP | **partly done**, ADR 0004 (per host, statistically). Remaining: the fixed-IP check ([target], M1); a 200-repo cold sync and PLC throughput ([target], M2). | M1, M2 |
| S10 | math-core coverage and the cost of a Temml fallback | no | **done**, ADR 0002 (math-core `=0.9.0` with pre-scan and normaliser; Temml only as contingency) | M4 |
| S11 | Safari/iOS IndexedDB eviction and session persistence | **yes**: an Apple device and a real account | **pending user run** | M5 |
| S12 | `createRecord` with an rkey as create-if-absent; `putRecord` with `swapRecord` | no (devnet) | **done**, ADR 0005 (create-if-absent uses `putRecord{swapRecord: null}`). Remaining: non-reference PDSes, through the `doctor` compatibility probe against devnet-like accounts only. | M2, M3 |

S3 and S10 were run together. ADR numbers: 0000 template, 0001 decision log, 0002 onward spikes and later decisions, in order of acceptance.

---

## 6. Deferred work

### 6.1 P2

| Area | Item | Reserved names or notes |
|---|---|---|
| Content | Q&A answers and accepted answers | `comment.answer` |
| | Quick-take feed (rendering is MVP) | – |
| | Co-authors, with `karma/2` credit | `post.coauthors`, `coauthor` |
| | Side-comment and inline-react margin UI (the data is MVP) | – |
| | Thumbnails | – |
| Moderation | Forum-wide lock (read-only forum) | `#lock` on a forum uri; MA9 |
| | Bulk mod actions and keep snapshots | `#banList`, `#raid`, `#banHost` under `{fk}-{tid}` keys; `#access.keep` |
| | Author moderation styles and guidelines | `profile.moderationGuidelines` |
| | Rate limits (`ratelimit/1`, advisory because time is self-asserted) | – |
| | Reports: sealed (age v1, X25519 anonymous recipients) or a forum report service | `report` |
| | Equivocation recording and fork-evidence actions (F15) | – |
| | Forum succession | `forum.successor`, `forum.aliases` |
| Tags and wiki | Wiki forks, merges and aliases | `#tag.aliasOf`; `wikiRevision.parents[]` already exists |
| Ranking and lists | `topAdjusted`; personalised magic (`magic/2`); tag-filter weights | – |
| Privacy and social | "What's public about me" view; `sensitive` forum flag; public bookmarks and subscriptions | `subscription`; `community.lexicon.bookmarks.bookmark` |
| Interop | standard.site syndication; labelers | `policy.labelers` |
| ForumMagnum | authorOnly tags; `lockedToAccountsCreatedAfter` and `repliesBlockedUntil`; karma from votes on tag revisions | see §6.5 |
| Indexer | `searchPosts` (fts5), `getActivity` (offline and cross-forum notifications), `getKarma`, `listBacklinks`, `getTallies`, `getRecords`; automatic audit sampling and demotion | – |
| Sync and clients | Automatic native switch of the Jetstream DID filter above ~50 events/s; background `--verify proof` (`verify = proof`); `/bridge/fetch`; `verify-site`; UTS #39 confusable checks beyond owner and moderator names | – |
| Build | The math font subset (STIX Two Math, or a `math-core-fonts` subset, ADR 0002); a second TypeScript type-check job (both tsgo and tsc) | – |

### 6.2 P7

- Verified MST range sync (`sync.getRecord` + `getBlocks`) as the default; sync windows.
- Indexer range proofs (`getRepoRanges`) and set digests.
- `karmaSnapshot` checkpoints.
- PLC audit verification: identity level `audited`, a strict mode, proofs against historical keys.
- Media mirrors; Service Worker media streaming.
- A full native engine bridge.

### 6.3 P8 and later

- Dialogues (`turn`); events and RSVPs (community calendar records); polls (`pollVote`); books (`collection`).
- Encrypted cross-device state; push notifications.
- The private-ballot tally extension.
- RSS import and export; importers from other forum lexicons.
- ForumMagnum recommendations; shareable drafts and comments on drafts (§6.5).

### 6.4 Never

- LLM features.
- Real-time collaborative editing.
- The jargon glossary and the annual review.
- Private messages (we link to Bluesky chat).
- ForumMagnum's karma-gated removal of `nofollow` (§6.5).

### 6.5 ForumMagnum features outside the MVP

| Feature | Phase | Reason |
|---|---|---|
| Recommendations ("recommended", "from the archives") | P8+ | Needs personalisation and cross-forum signals; personalised ranking (`magic/2`) comes first in P2. |
| Shareable drafts and comments on drafts | P8+ | Drafts are local because repos are public. Sharing needs encrypted cross-device state. |
| authorOnly tags | P2 | Needs an optional wikiRevision field and a new behaviour ID: a client that ignored the field would compute different relevance. It ships with `karma/2`. |
| Removal of `nofollow` for authors with ≥ 10 karma | Never | The HTML contract fixes `rel="nofollow ugc noopener noreferrer"` (`lfm-1.md`). Making it depend on karma would make rendering depend on the fold and break the `(cid, RENDERER_VERSION)` cache. The hash-routed SPA is not indexed by search engines anyway. |
| `lockedToAccountsCreatedAfter`, `repliesBlockedUntil` | P2 | Time is self-asserted, so these are best-effort rules. Account age comes from the PLC genesis operation (`social.md` §2). They ship with `ratelimit/1`. |
| Karma from votes on tag revisions | P2 | Needs a new behaviour ID (`karma/2`, shared with co-author credit). Under `karma/1`, opinions on wikiRevisions are L5 inputs and never feed karma (`social.md` P4). |

---

## 7. Risks

| Risk | Spike | Mitigation | Status |
|---|---|---|---|
| PDS read limits are shared across Bluesky hosts, so cold sync takes minutes | S9 | Progressive paint, time budget, persistent cache, few collections, fk ranges, indexer thresholds, honest cost table | ADR 0004: per host (statistical); fixed-IP check pending |
| Cursor range listing unsupported on non-reference PDSes | S7 | Per-host probe and fallback to a full collection listing; P7 range sync | ADR 0004: works on the reference PDS and rsky-pds |
| `include:` resolution fails, blocking login | S8 (M14) | Fallback chosen at PAR; enumerated scopes on the devnet and `.temp` | pending user run |
| jacquard is alpha (0.13 is breaking) | S1 | Exact pin `=0.12.2`, facade, our own OAuth client as fallback | pending user run |
| Browser OAuth key storage | S2 | Library choice gated on non-extractable keys; strict CSP, Trusted Types, two sanitiser layers | pending user run |
| Wasm size and math-core coverage | S3, S10 | Feature gating; size hygiene rules; `verify.wasm` split rule; Temml contingency | ADR 0002: within budget; math-core accepted with known gaps |
| math-core is young and changes about monthly | S10 | Exact pin, cargo-vet entry, `mathml` vectors must pass unchanged on every bump | open |
| Constellation or Jetstream disappear; Jetstream v1 hosts retired | S4, S6 | Approval mode and the closure need neither; indexer; relay fallback; several v2 connections of ≤ 150 DIDs | open |
| Open mode silently depends on Bluesky infrastructure | – | `membership` defaults to `approval`; the CLI always writes the mode; open mode is labelled in the UI | fixed in spec |
| A forum is bound to its owner DID forever | – | Guide: a dedicated did:plc forum account with rotation keys held by ≥ 2 admins; `forum.successor` Reserved (P2) | guidance at M7 |
| Lexicon lock-in before learning | – | `.temp` dogfooding; limits reviewed at M0 and again at M7 | ongoing |
| `.temp` dogfood data is orphaned at 0.1 | – | `xtask migrate-temp` (deterministic rewrite, `data-model.md` §1.3) | M7 |
| Heavy voters exceed fetch caps, so clients could fold different inputs | – | Normative caps (first N keys descending; opinions 100k); `[partial]` | fixed in spec |
| Stale third-party proofs could resurrect deleted or edited content | – | Currency rule: a proof is current only if its rev ≥ the PDS head; otherwise `[stale]` and outside the fold. Cost: readers still contact changed authors' PDSes (`sync.md` §17, open question 2). | fixed in spec; cost measured in M6 |
| PDS behaviour differs on non-reference implementations (null `swapRecord`, `applyWrites` errors) | S12 | `doctor` compatibility probe; per-host `createRecord` fallback | open |
| Seeded karma slows bootstrap in new open forums | – | Configurable gates and `establishSeeds`; authorities bypass gates; grants | accepted |
| Public votes change social dynamics compared with ForumMagnum | – | Clear interstitials; private-ballot tally extension (P8+) | accepted |
| CID-bound curation and pins create friction on edits | – | Warning before editing; one-key re-assert in the mod queue | accepted |
| A ban hides past content (unlike ForumMagnum) | – | Item-level keep now; keep snapshots in P2 | accepted |
| Abuse and legal exposure for indexer operators (CSAM, takedowns) | – | No blob storage, blocklist, documented contact, upstream takedowns honoured | M6 docs |
| Safari storage eviction | S11 | PWA install, `navigator.storage.persist()`, draft download, re-login prompt | pending user run |
| PLC throughput for cold starts | S9 | Pacing, caching, badged Slingshot hints, indexer | target pending |
| Single-maintainer domain or publisher compromise | – | Domain hardening, offline rotation key, reproducible builds, `lessdugin web` as the zero-trust path | M7 |

---

## 8. User-dependent items

| # | Item | Needed for | State |
|---|---|---|---|
| U1 | DNS for `lessdugin.7778777.online`: the two `_lexicon` TXT records, site hosting records, DNSSEC, CAA, registrar lock, multi-year auto-renew | M7 | pending |
| U2 | Publisher account (did:plc, handle `lessdugin.7778777.online`) with an offline rotation key | M7 | pending |
| U3 | Static host account: Cloudflare Pages or Netlify preferred, GitHub Pages as the fallback | M5 preview, M7 | pending |
| U4 | Throwaway bsky.social test accounts (and optionally one self-hosted PDS) | S1, S2, S8, M2 smoke test, M5 logins, M7 | pending |
| U5 | An Apple device with Safari and iOS | S11 | pending |
| U6 | Confirmation of MIT OR Apache-2.0 for the `atp` and `lexicon` crates | M7 (D21) | pending |
| U7 | Review of the open questions in `data-model.md` §14, `social.md` §20.2, `sync.md` §17, `lfm-1.md` §14, `limits.md` §22, `threat-model.md` §12, `docs/spec/README.md` §7, ADR 0001 and §9 below; and a decision on the known issue K3 of `docs/spec/README.md` §1.4 (math normaliser, ADR 0002 against `lfm-1.md` §7) before M4 freezes the math vectors | M4 (K3), M7 (before freezing) | pending |
| U8 | Authorisation to publish. Publication is irreversible: published NSIDs, limits and behaviour IDs are frozen. | M7 | pending |

**Pending user runs** (updated as milestones finish): S1 (passed on the devnet, ADR 0007; pending: a bsky.social entryway login, the hosted client metadata, the `include:` scope with `final-ns`, the OS keyrings), S2, S8, S11; the M2 production smoke test; the M5 production logins; the S4 audio MIME check on a production PDS.

---

## 9. Decisions made while writing

1. **Three classes of acceptance criteria** ([auto], [user], [target]). Where the approved plan and synthesis §8 list different criteria for a milestone, the approved plan's criteria gate. Synthesis-only criteria that need real accounts became [user] (M1 and M5 bsky.social logins, M2 smoke test). The synthesis-only `listPosts` p95 criterion needs no user but is absent from the approved plan, so it became [target].
2. **`listRecentDiscussion` stays in the M6 first pass,** as in the approved plan, `data-model.md` §11.3 and `sync.md` §15.3, although the completeness review's "keep only" list omits it.
3. **Phases for the ForumMagnum features** the completeness review asked to place (§6.5): recommendations P8+, shareable drafts P8+, authorOnly tags P2, the nofollow gate Never, the account-age and reply-block locks P2, karma from tag revisions P2 via `karma/2`.
4. **Dialogues, events and polls are P8+,** following synthesis §8 and the approved plan, not synthesis §4.15 (which says P2 or P3).
5. **Every item cut by the completeness review is scheduled for P2,** matching `sync.md` §16. This includes the build items (math font subset, second type checker). `threat-model.md` labels the same items "Reserved (deferred)" because the review gave them no phase; this plan supplies the phase, and the meaning (not in v1, no effect when read) is the same.
6. **`migrate-temp` is a deterministic rewrite,** not a copy (`data-model.md` §1.3 corrects the completeness review).
7. **Testable acceptance criteria were added for completeness fixes** that had none: the login-binding test (M1), the streaming-blob test (M4) and the Jetstream identity/account noise fixture (M5).
8. **The parity scenario is split into milestone subsets** (§4.2), so M2 and M3 can be gated on it before media and tags exist.
9. **The Level-0 document of M7 is an informative implementer guide.** The normative definition of the conformance levels is in `docs/spec/README.md` §4.
10. **One TypeScript type-check job in CI** (TypeScript 7), because running both tsgo and tsc is deferred.
11. **A hosted SPA login at M5 uses a preview deployment** on the user's static host; the production site is deployed at M7.

**Open questions**
- Confirm the phases in item 3, especially recommendations (P8+) and the nofollow gate (Never).
- Confirm that `listPosts` p95 should not gate M6 (item 1).
