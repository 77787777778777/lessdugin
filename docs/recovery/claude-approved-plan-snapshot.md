# lessdugin: a serverless markdown and media forum on atproto (plan)

## Context
- **Repo.** `77787777778777/lessdugin` is greenfield: it holds only an AGPL LICENSE. Work happens on branch `claude/exciting-keller-17oykr`. The `DS2SRandomizer` repo is unrelated.
- **Goal.** A ForumMagnum-class forum (posts, threaded comments, karma and agree/disagree voting, reacts, tags/wiki, sequences, moderation). All content lives in users' own atproto PDS repos.
- **Constraints.**
  - The core works with nothing but PDSes: no lessdugin server, no extra database, no trusted website.
  - It must be usable from the command line.
  - Optional, verifiable extensions add a richer experience.
  - Rust and TypeScript; CLI, TUI and SPA clients.
- **User decisions.**
  - NSID authority `lessdugin.7778777.online`, so the prefix is `online.7778777.lessdugin.*`.
  - The Rust core is shared with the SPA through wasm.
  - The SPA uses React and Vite.
  - The first pass is the MVP in all three clients plus the optional indexer.
  - Licences are split: applications are AGPL-3.0-or-later; lexicons, spec and test vectors are CC0-1.0; the `atp` and `lexicon` crates are MIT OR Apache-2.0.
  - Cadence: run M0 to M6 continuously, committing and pushing after each milestone, and stop before M7, which publishes lexicons and DNS.
- **Sources.** The design comes from three research passes and a 9-agent design workflow (4 architects, 3 adversarial critics, a synthesizer and a completeness critic).
  - Full reconciled design (~120 KB): `result.synthesis` inside `/tmp/claude-0/-home-user/ceecc645-bbf9-531d-a32e-64c609b00879/tasks/w7jforgnt.output`. Per-agent outputs are in `/root/.claude/projects/-home-user/ceecc645-bbf9-531d-a32e-64c609b00879/subagents/workflows/wf_8d0197cf-bfc/journal.jsonl`.
  - **Step 1 of M0 commits that synthesis into `docs/spec/`, with the completeness fixes below applied.** From then on the spec in the repo is the source of truth. This plan is its executive summary.

## Status and resumption (2026-10-16 17:25 UTC, after the usage-limit interruption)

**Done and pushed** (branch `claude/exciting-keller-17oykr`, head `0f07345`)
- M0 is complete:
  - docs/spec (data-model, lfm-1, social, sync, threat-model, limits, engine-api, glossary, README) and docs/plan.md;
  - ADRs 0000–0006;
  - 30 lexicons (goat lint and `@atproto/lexicon` clean) and key vectors (134);
  - scaffold, CI, devnet (boots in 0.9 s), and the spikes S3, S4, S5, S6, S7, S9, S10 and S12.
- `lessdugin-atp` is complete: 121 tests, reviewed and fixed.

**Implemented but missing their review/fix stage** (the usage limit killed those agents; the code is on disk and partly in WIP commits)

| Workstream | Finished stages | Missing |
|---|---|---|
| LFM/1 renderer (`crates/markdown`, 505 vectors and XSS/terminal/MathML corpora) | implement, review | **fix**: review found a CRITICAL math-core exponential-time hang on unmatched delimiters, output-amplification DoS, sanitiser-equality breaks, spoiler/real-host leaks; plus the ADR 0002 math follow-ups (K3) |
| SPA shell (`web/`, `packages/engine` types/api/mock; 62 unit + 7 Playwright tests) | implement, review | **fix**: 1 high, 23 medium and 7 low findings |
| M1 foundations (core identity/xrpc/verify/engine/accounts + testkit MemHost; native host + SqliteKv + config; auth + CLI) | three implementation agents | **review + fix** |
| `lessdugin-lexicon` (141 tests per namespace) | implement | **review + fix** |
| M2 engine | FakePds (`testkit::fake_pds`), record store + admission | **sync + discovery, outbox, engine wiring, review, fix** |

**Resumption steps** (in order)
1. **Free disk.** `target/` is 18 GB with 7.8 GB free: delete `target/debug/incremental` and stale wasm/release artifacts.
2. **Snapshot.** Run `cargo check --workspace`, then commit and push a WIP snapshot of the uncommitted work.
3. **Resume the five workflows with `Workflow({scriptPath, resumeFromRunId})`.** Completed agents replay from cache and only the failed stages run. Run them two or three at a time to limit CPU and disk pressure:
   - `lfm1-renderer` (wf_24f1a634-cb0);
   - `spa-shell` (wf_067a2591-44e);
   - `m1-foundations` (wf_f3eb3c6f-b02);
   - `m1-lexicon` (wf_b0def068-8b7);
   - `m2-engine` (wf_22bd7809-0e0).
4. **Follow-ups not in those scripts** (listed in the scratchpad `followups.md`). Run them as a small follow-up workflow after the LFM fix:
   - LFM math per ADR 0002 (pre-scan list, MathML normaliser with `mc-*` classes, `mathcolor`) together with the lfm-1 §7 and math/mathml vector updates;
   - core identity/doc using `DidDocument::atproto_with(policy)`;
   - strict JSON admission (already in the store brief; verify it);
   - reconcile `AccountView` between `dto.rs` and the SPA `types.ts`;
   - add the `lessdugin-worker` Trusted Types policy to `_headers`;
   - fold the SPA review's engine-integration requirements into the M2 clients brief.
5. **Gate M1.** Once the M1 and lexicon fixes land, run `cargo xtask ci` plus the `LESSDUGIN_E2E=1` devnet tests, then commit "M1 complete".
6. **M2 clients workflow**, after m2-engine's fix stage:
   - wasm bindings, JsHost and the real engine adapter in the SPA;
   - CLI read/ls/post/comment/sync/forum/join/edit/delete with drafts;
   - TUI screens with reply via `$EDITOR`;
   - the `lessdugin web` bridge;
   - the devnet parity e2e.
7. **M3 → M6** as in the milestone table below. Each milestone is one workflow (implement in parallel by crate, then review, then fix, then the CI gate), with a commit and push per milestone. Stop before M7.

**Operating rules learned**
- At most about 3 concurrent workflows: 4 CPUs, a shared target dir, and disk pressure.
- Commit a WIP snapshot whenever the workspace compiles.
- Agents must never edit crates owned by another live workstream.

## Core principles (normative)
1. **The PDS is the database.** Author PDSes plus PLC/did:web plus DNS-over-HTTPS (DoH) are enough for every MVP feature. The cache is disposable: deleting it costs only time.
2. **Trust levels.**
   - Displayed text, and anything that feeds moderation or karma, must come from one of two places:
     - the author's PDS from their DID document, over TLS, with the CID recomputed from JSON → DAG-CBOR (`pds` level);
     - a signed commit proof, `sync.getRecord` CAR, verified against the DID's `#atproto` key (`proven`).
   - A third-party proof counts as current only if its rev is at least the PDS `getLatestCommit` head. Otherwise it is `[stale]` and kept out of the fold.
   - The relay, Jetstream, Constellation, Slingshot and the indexer are **hints**. They may discover, schedule and show placeholders, but they never supply displayed text.
3. **Determinism.** `ForumView = fold(decl, records, obs, repoStatus, now, algorithmIds)` is a pure fold in the Rust core.
   - It is byte-identical across CLI, TUI, wasm and indexer.
   - Its behaviour is versioned by IDs that never change meaning: `mod/1 karma/1 magic/1 lfm/1 default/1 sk/1 fk/1 proofs/1`.
   - Integers only, apart from `hot()` via `libm`. Sorted maps; ties broken by uri, then cid.
4. **Fail closed.** If moderator repos are unavailable, the view degrades. Spam is never resurrected.
5. **Time is a claim.** `t_rank = max(causal lower bound, min(createdAt, TID time))`, clamped to first-seen when an item is future-dated by more than 5 minutes. Only ranking and display depend on time, so backdating can only hurt its author.
6. **Public means public.** Votes, reacts, memberships and mod actions are public records, and the UI says so on first use. Drafts, read marks, subscriptions, bookmarks and mutes stay local.
7. **Dependency tiers.**
   - **CORE:** PDS, PLC, DoH.
   - **CORE+:** relay `listReposByCollection`, Jetstream v2, Constellation, Slingshot.
   - **EXT:** lessdugin indexer.
   - Approval-mode forums are fully complete at CORE. Open mode needs CORE+ only to discover brand-new newcomers.

## Protocol (lexicons in `lexicons/online/7778777/lessdugin/`)
**Namespace**
- One flat group `NS = online.7778777.lessdugin`, with one DNS TXT record. A second group `NS.index` covers indexer queries.
- Development and dogfooding use the unpublished `NS.temp` namespace. It is a build constant and switches to `NS` in M7. `xtask migrate-temp` copies records across.

**Record keys**
- `fk = b32s(sha256("lessdugin-fk/1" ‖ forumUri))[..12]`.
- `h16 = b32s(H("lessdugin-sk/1", collection, parts…))[..16]`.
- A record whose rkey isn't the canonical one is invalid.

**Collections**

| Collection | rkey | Key fields |
|---|---|---|
| `forum` (owner repo) | slug | `name`; `about: md`; `moderators[{did, role admin\|moderator}]`; `membership` approval\|open (lexicon default **approval**, and the CLI always writes it); `defaultTier`; `policy{moderation, karma{pairCap 25, establish 1, establishSeeds 2}, ranking{offsetHours 2, exponentMilli 1150, curatedBonus 10, cutoffDays 90}, voting karmaOnly\|twoAxis\|reacts, reactPalette, gates, display{collapseBelow -5, newUsers}, tagCreation}`; `discovery.indexers[]`; `reportContact` |
| `membership` | `{fk}` | `forum`, `note` |
| `post` | `{fk}-{tid}` | `forum!`; `kind` article\|link\|question\|quickTake; `title` (required unless quickTake); `body: md`; `link` (required for `kind=link`); `excerpt` (required if the body spills to a blob); `tags[]≤32`; `labels` (self-labels) |
| `comment` | `{fk}-{rootTid}-{tid}` | `root!`/`parent` strongRefs; `forum!`; `body!`; `quote: textQuote`; `retracted`; `moderatorHat` |
| `revision` | `{P}-{tid}` | `subject!` sref (the superseded version); `op` update\|delete; `record: unknown` (verbatim prior value, `$type` required); `summary` |
| `opinion` (one per voter and subject) | `{fk}-{stid}-{h16}` | `subject!` sref; `forum!`; `karma` up\|down\|strongUp\|strongDown; `agreement` (same four values); `reacts[{react, stance for\|against, quote}]≤32`; `tags[{tag, value}]≤32` |
| `modAction` (one per actor, subject and action) | `{fk}-{h16(scope, subject, actionKey)}` | `forum!`; `scope` forum\|author; `subject` (did \| at-uri \| `tag:slug`); `subjectCid` (required for positive assertions); `action` union of `#hide{hidden, spam, showReason}`, `#access{banned\|member}`, `#restrict{caps[], nullifyVotes}`, `#tier{personal\|frontpage\|curated}`, `#pin{forum\|thread}`, `#lock{allowed[]≤5000}` (posts only; forum locks reserved), `#tag{removed, head}`; `reason`; `until`; `adopts` |
| `wikiRevision` | `{fk}-{tid}` | `forum`, `slug`, `parents[]` (the MVP uses linear history), `name`, `body: md` (may spill) |
| `sequence` | `{fk}-{tid}` | `title`, `body`, `cover`, `chapters[{title, posts[at-uri]}]` |
| `profile` | `self` | `displayName`, `avatar`, `bio`, `links`, `pinned`. Falls back to `app.bsky.actor.profile` read directly from the PDS |

**Shared definitions**
- `NS.markdown {profile "lfm/1", text ≤100 KB | textBlob(text/*, ≤1 MB), media[defs#media]≤128, mentions[], links[]}`. Writers spill to `textBlob` above 64 KiB.
- `defs#media {blob, alt, aspectRatio, durationMs}`, `#textQuote {exact, prefix, suffix, start}`, `#externalLink`.
- Every limit is fixed at first publication.

**Reserved for later:** `report` (sealed with age to moderator keys), `turn` (dialogues), `coauthor`, `pollVote`, `collection`, `karmaSnapshot`, `observation`, `subscription`, `forum.successor`, `comment.answer`, `post.coauthors`, `post.event`.

**LFM/1 markdown** (spec at `docs/spec/lfm-1.md`)
- Parser: pulldown-cmark `=0.13.4` with tables, footnotes, strikethrough, tasklists, math and GFM alerts.
- Raw HTML becomes escaped text. Post-passes add autolinks, spoiler and details blocks, and figure captions.
- `cid:` images resolve only against the body's own `media`. `at://` links become internal routes with hover previews. `[@x](at://did)` is a mention.
- External images are never fetched automatically.
- Math is rendered to MathML by math-core, with Temml as fallback; spike S10 decides.
- There is a strict HTML allowlist contract and a canonical plain-text projection P, used for quote and react anchors, excerpts and word counts.

**Auth scopes**
- Default scope string: `atproto include:NS.authFull blob:image/* blob:video/* blob:audio/* blob:text/* blob:application/pdf` (audio MIME types are normalised before upload). `authInteract` covers membership, comment, revision and opinion.
- Fallback: enumerated `repo?collection=…` scopes, chosen when the pushed authorization request (PAR) returns `invalid_scope`.
- `transition:generic` is never requested.
- Client metadata JSONs for web and native clients live under `deploy/site/oauth/`.

**Interop**
- Reused: strongRef, selfLabels, and `app.bsky.actor.profile` (read-only fallback).
- Opt-in in P2: syndication to standard.site, community calendar and bookmarks, and labelers.
- `at.markpub.markdown` is not adopted; our `markdown` object is deliberately similar, so it can be added to the union later.

## Discovery, sync and verification (`docs/spec/sync.md`)
**Identity**
- Handles resolve via DoH TXT, then `.well-known`, everywhere. A handle is displayed only if it verifies in both directions.
- PLC and did:web supply DID documents. Slingshot is used only as a badged hint.
- At login, the token `iss` must be listed in the PDS's `/.well-known/oauth-protected-resource` `authorization_servers`.

**Forum open**
1. Fetch the declaration from the owner's PDS.
2. Sync the authorities: the owner and the moderators listed in the declaration.
3. Seeds S = authorities plus granted members.
4. In open mode, also sync:
   - the closure: DIDs referenced by seeds' records;
   - newcomers, found via Constellation backlinks, Jetstream membership events, the indexer, or the relay (while the ecosystem is small). Each newcomer is confirmed by fetching their membership record.

**Per-repo sync**
- Compare `getLatestCommit` with the cached rev, then call `describeRepo`.
- List per-forum key ranges with a `listRecords` cursor trick: probe each host and fall back to a full listing if the trick doesn't work.
- Recompute the CID of every record and validate it.
- Create tombstones only after a complete listing, or after `RecordNotFound`.
- Check the head again before the atomic store write.
- Fetch caps are normative: the first N keys in descending order, with N = 20k per collection, or 100k for opinions. Beyond that the view shows `[partial]`.

**Live updates**
- Jetstream v2 subscribes with the wildcard `NS.*` and serves as a "this repo changed" signal, which triggers point fetches.
- Natively, identity and account events for unknown DIDs are dropped. In the browser, `wantedDids` is set once the participants are known.
- Polling fallback runs at 2 min, 10 min, 1 h or 12 h depending on the repo's class, plus a daily reconcile.
- All sources feed a single dirty queue with P0–P3 priorities.

**Outbox (durable, one write in flight per account)**
- Edits use `applyWrites[create revision, update]` with `swapCommit` taken from a fresh `getLatestCommit`, plus a CID-aware retry.
- Opinions use create-if-absent, `putRecord` with `swapRecord`, or delete when empty, behind a 4 s undo window.
- A user's first post in a forum is batched together with the membership record.
- Deletes remove the target in the first chunk.
- Blobs are uploaded at publish time, and the returned reference is embedded verbatim.
- 429 responses and the write budget are respected.

**Verification (`proofs/1`)**
- Our own CAR parser with caps.
- Commit: version 3, the DID matches, low-S ECDSA signature (k256 or p256).
- Walk the MST to `collection/rkey`, then check the record's CID.
- `lessdugin verify <ref> --deep` also checks strongRefs, revision CIDs and blob CIDs.

**Store**
- `cache` (disposable): records, repo state, identity, tombstones, blobs, cursors.
- `state` (durable): outbox, drafts, read marks, subscriptions, settings.
- Native uses SQLite; the browser uses IndexedDB, both behind a KV trait with tuple-encoded keys.
- One background-sync leader: `fd-lock` natively, a Web Lock in the browser.
- The ForumModel keeps per-forum skeletons in memory.

**Media**
- Fetched with `getBlob` from the author's PDS and checked against its CID via a streaming `Host::fetch_blob` (no blob bytes in wasm memory).
- Autoload policy: the author is in S ∪ E, the item is visible, no sensitive self-labels, and the host is trusted. Otherwise the user clicks through ("loads from X, which sees your IP").

**Scale**
- Up to ~300 participants, serverless everywhere.
- 300–3,000: native clients are fine; the forum should declare an indexer for SPA cold starts.
- Above 3,000: an indexer is needed for the browser.

## Social semantics (`docs/spec/social.md`, with vectors in `testdata/vectors/`)
**Moderation fold (`mod/1`)**
- Ranks: owner 3, admin 2, moderator 1, post author 0 (author scope only); `tag:` subjects rank 0.
- Only authorities in the current declaration count. Removing a moderator lapses their actions, so `mods rm` runs `mod adopt` first.
- At the highest rank present, the safest value wins.
- Positive assertions (pin, frontpage or curated tier, keep) are bound to a CID.
- A ban hides all of the user's in-forum content; an item-level keep overrides it.
- `#restrict` retroactively hides items of the restricted kind (`Hidden(Restricted)`).
- Locks use allow-lists of comment uris. Forum-wide locks are deferred.
- modAction updates and deletes go through the revision protocol.
- There is a mod log and a raw view.

**Karma (`karma/1`), seeded and staged**
- `K1 = pairsum(None, S)`.
- Established voters: `E = S ∪ {u : K1 ≥ establish ∧ upvoted by ≥ establishSeeds distinct seeds}`.
- `KP = pairsum(K1-power, E)`, then `K = pairsum(KP-power, E)`, with a per-(voter, author) cap.
- Vote power ports ForumMagnum's `calculateVotePower`:
  - small vote: 1, or 2 at ≥ 1000 karma;
  - strong vote ladder: 10 → 2, 100 → 3, … 500,000 → 16.
- Scores: `self(i) + Σ power(KP[voter])` over voters in E. Agreement never feeds karma. Votes not yet counted are shown as "n votes not yet counted".
- Property tests: order independence, idempotence, layering, the sybil bound (restricted to K, scores and others' visibility), and incremental fold equal to batch fold.

**Gates.** Defaults for react second/create/anti are 5/10/20 karma, author moderation of personal/frontpage posts 50/2000, create tag 10, wiki edit 5, sequences 1000. Approval-mode presets are lower. Authorities bypass all gates.

**Reacts**
- `default/1` palette of 34 ids. Stance is for or against; anchors resolve deterministically against P.
- Validity follows the gates above. Reacts never affect karma.

**Ranking (`magic/1`)**
- `hot = b / (ageHours + 2)^1.15`, where `b = score + curatedBonus`.
- Pinned items come first. Magic includes only items within `cutoffDays`; top, new and old include everything.
- Post sorts: magic, top, new, old, recentComments. Comment sorts: magic, top, new, old, latestReply. Plus a Recent Discussion view.
- List filters: frontpage (default), all, curated.

**Visibility.** The first matching rule wins:
1. Excluded.
2. Not a participant.
3. Removed.
4. Banned.
5. Restricted.
6. Author-moderated.
7. Locked.
8. New user (collapse).
9. Low score (collapse).
10. Visible.

Hidden items that have visible children become tombstones. Cycles in parent chains are cut deterministically.

**Tags and wiki.** A tag is (forum, slug). The MVP page head is the latest revision by an eligible author, or the moderator's `#tag.head`. Relevance = the author's own tags plus weighted tag votes.

**Sequences.** Library listing is gated or mod-pinned. Next/previous navigation; "continue reading" is local.

**Notifications.** Computed locally from first-seen times, never from `createdAt`. Reasons: reply, mention, quote, new comment or post in subscriptions, reacts, mod actions affecting me, karma changes.

**In the MVP but minimal:** question badge, quick-take rendering, link posts, and comment quotes listed (margin UI later).

## Architecture
```
lessdugin/
  Cargo.toml (workspace) rust-toolchain.toml (1.97 + wasm32) deny.toml clippy.toml (bans SystemTime::now etc. in wasm-safe crates)
  package.json pnpm-workspace.yaml biome.json tsconfig.base.json
  lexicons/online/7778777/lessdugin/{...}.json (CC0)    testdata/vectors/<kind>/*.json (CC0)   testdata/interop/ (vendored)
  crates/
    atp/        (MIT/Apache, wasm-safe) syntax types, JSON⇄DAG-CBOR, CID, CAR reader, MST proof walk, commit verify, did:key, XRPC structs
    lexicon/    (MIT/Apache, wasm-safe) hand-written serde structs (+flatten extra) + data-driven validator from embedded lexicon JSON; rkey derivation; NS const
    markdown/   (wasm-safe) LFM/1 → HTML+MathML, StyledDoc<TermText> for terminals, P projection, ToC/excerpt/anchors, term_sanitize, safe_href
    core/       (wasm-safe) Host/KvStore traits, identity, discovery, sync, Jetstream parsing, outbox, verify orchestration, social fold, ForumModel, search index, notifications, media (sniff/strip/CID), limits, DTOs, Engine<H>
    native/     NativeHost: reqwest(rustls)+SSRF guard, SqliteKv (rusqlite bundled), tokio-tungstenite, keyring sessions, app-password + OAuth facade (jacquard-oauth =0.12.2), image re-encode
    tui/        ratatui 0.30.2 + crossterm + ratatui-image 11.1 (no chafa) + tui-textarea; Elm-style State/Action/Effect
    cli/        bin `lessdugin` (clap 4): command tree, drafts, `tui`, `web` (axum + rust-embed SPA + local bridge)
    wasm/       wasm-bindgen + tsify bindings, JsHost, WasmEngine (size budget ~900 KB gz, CI fails >1.1 MB)
    indexer/    bin `lessdugin-indexer` (axum, same engine + SQLite side tables + fts5)
    testkit/    MemHost, FakePds (fault injection, cursor modes, proofs), mini-Jetstream, fake Constellation, vector runner, synthetic forums
  xtask/        wasm, web, dist, site, devnet, e2e, vectors [--bless], lex lint|breaking|publish|check-dns, migrate-temp, ci
  packages/engine/  worker.ts (wasm in Web Worker, idb stores, fetch, WebSocket, DoH; Comlink), client.ts, auth-bridge.ts, leader.ts
  packages/e2e/     devnet.mjs (@atproto/dev-env 0.6.20 TestNetworkNoAppView + mini-Jetstream + fake Constellation), Playwright
  web/              React 19 + Vite + TS, TanStack Router (hash history) + Query + Virtual, CodeMirror 6 markdown editor, Radix, DOMPurify, Trusted Types
  docs/spec/{data-model,lfm-1,social,sync,threat-model,limits,glossary}.md  docs/adr/  docs/guide/  docs/plan.md
  deploy/site/{_headers, oauth/*.json}  deploy/indexer/Dockerfile  .github/workflows/{ci,e2e,release,pages}.yml
```
- **Host trait.** `http(HttpRequest{target: Anon(url) | UserPds{account, path}}) -> HttpResponse` (byte caps counted while streaming), `fetch_blob`, `websocket`, `sleep`, `now_ms`, `fill_random`, and `KvStore{get_many, scan, write(batch)}`. The core never spawns tasks. Futures are `MaybeSend`: `Send` natively, not in wasm. Natively `tokio::spawn`; in wasm `spawn_local`.
- **One engine API** serves CLI, TUI, wasm and indexer: forums, sync, queries, writes, local state, pure render/verify, plus events (`Changed`, `SyncProgress`, `Write`, `Auth`). The CLI's `--json` output and the SPA DTOs share one versioned schema, so parity checks are byte diffs.
- **SPA auth**
  - Hosted mode: `@atproto/oauth-client-browser` 0.5.9 (non-extractable keys; spike S2), with `@atcute/oauth-browser-client` as fallback. It uses `response_mode=query`, and the callback is handled before the hash router mounts.
  - The main-thread AuthBridge allowlists the write methods (`createRecord`, `putRecord`, `deleteRecord`, `applyWrites`, `uploadBlob`) on NS collections only.
- **Native auth**, in this order of preference:
  1. OAuth loopback (`http://localhost?redirect_uri=http://127.0.0.1:<port>/callback`).
  2. Hosted native client metadata.
  3. `--no-browser`: paste back the redirect URL.
  4. `--app-password`: taken by prompt, stdin or environment variable, never argv; main passwords are refused.
  - Sessions live in the OS keyring. Refresh runs under an `fd-lock`. Sessions last about 14 days, with a re-login warning.
- **Hosting modes**
  - **Static site:** prefer Cloudflare Pages or Netlify so the CSP is sent as a header; GitHub Pages is the fallback. Strict CSP plus Trusted Types; reproducible build hash.
  - **`lessdugin web`** (the zero-trust path): 127.0.0.1 only with a Host-header check. A per-launch capability travels in the URL fragment. The CLI keeps the session and the SPA writes through `/bridge/xrpc`; drafts go through `/bridge/kv`. The browser holds no secrets.

## Clients
- **CLI** (`lessdugin`). Reading never needs login. Bare `lessdugin` on a TTY opens the TUI.
  - Commands:
    - Auth and identity: `login`, `logout`, `whoami`, `accounts`, `resolve`, `doctor`, `verify`.
    - Forums: `forum discover|ls|follow|use|show|create|edit`, `join`, `leave`, `members`, `approve`, `mods add|rm`.
    - Reading: `ls`, `activity`, `read`.
    - Writing: `post`, `comment`, `edit`, `delete`, `retract`, `history`.
    - Opinions: `vote`, `agree`, `react`, `tag`.
    - Organisation and discovery: `tags`, `seq`, `bookmark`, `subscribe`, `notifications`, `search`, `user`, `profile edit`.
    - Moderation: `mod hide|keep|ban|approve|restrict|frontpage|curate|pin|lock|tag-*|queue|log|adopt`.
    - Data and tooling: `draft *`, `media add|get|open`, `sync`, `export`, `tui`, `web`, `config`, `completions`.
  - Refs can be `3` (from the last listing), `c12` (a comment label), an at-uri, an SPA URL or `handle/rkey`.
  - Drafts are `$DATA/drafts/<id>.md` files with YAML frontmatter, edited in `$EDITOR`. Local image paths are stripped of metadata, uploaded at publish time and rewritten to `cid:`. Publishing asks for confirmation.
  - `--json` gives stable output. All terminal output goes through `TermText`, which sanitises escape sequences. Exit codes 0–6.
- **TUI.** Screens: Home, Forum (sort and filter), Thread (collapsible, unread jumping), Tags, Sequences, User, Notifications, Search, Mod queue and log, Drafts. Overlays: help, a `:` command line that accepts any CLI subcommand, the react palette and login.
  - Vim-style keys; opinion keys `u/U d/D a/A z/Z 0 x`. An inline composer escalates to `$EDITOR`.
  - Images via ratatui-image. A "N new — R" banner announces live updates and never moves the cursor.
- **SPA**
  - Hash routes: `#/`, `#/f/:owner/:slug[/activity|new|tags|t/:tag|seqs|mod|members|settings]`, `#/p/:author/:rkey`, `#/c/…`, `#/s/…`, `#/u/:actor`, `#/at/*`, `#/notifications`, `#/drafts`, `#/bookmarks`, `#/search`, `#/settings`, `#/login`.
  - Rendered HTML is inserted through Trusted Types plus DOMPurify configured with the renderer's exact allowlist.
  - Delegated handlers cover hover previews, CID-verified media as object URLs, anchors, and footnotes shown as sidenotes.
  - The editor has a toolbar, live preview, paste or drop of images, and mention autocomplete.
  - Mobile layout; verification and pending-write badges.

## Optional indexer (M6)
- **Ingest:** Jetstream `NS.*` from two endpoints with a cursor rewind. Backfill every 6 h via relay `listReposByCollection`, then range listing. `sync.getRecord` proofs are fetched per live event and cached. A `--poll <pds>` mode serves the devnet. Polite per-host limits.
- **XRPC `NS.index.*`:** all unauthenticated GET with CORS `*` and `asOf`.
  - First: `describeIndexer`, `listForums`, `listParticipants`, `getRepoHeads`, `getChanges`, `listPosts`, `listRecentDiscussion`, `getThread`, `getRecordProofs` (CAR).
  - Then: `searchPosts` (fts5), `listBacklinks`, `getActivity`, `getKarma`, `getTallies`.
- **Clients** treat it as a discovery, ordering and proof source and recompute displayed values locally. `--audit` cross-checks heads, changes and `inputDigest`. The indexer ships as a Docker image.

## Fixes from the completeness review, folded into the spec
- `#restrict` retroactively hides; forum-wide lock is cut; stale proofs are excluded; streaming `fetch_blob`; normative fetch caps; required fields per post kind; the outbox is serialised with a fresh swap head.
- D12 corrected to the §4.2 clamp; modAction edits produce revisions; `mods rm` adopts first; `tag:` subjects rank 0.
- Clients search with the core in-memory index; fts5 is for the indexer only.
- The login issuer check uses protected-resource metadata; Jetstream identity and account noise is handled; `revision.record` is `unknown`; audio MIME types are normalised; `response_mode=query`; CSP is sent as a header.
- `membership` defaults to approval and the CLI always writes it explicitly. Open mode is labelled as relying on public relay infrastructure.
- Forum-succession guidance (a dedicated did:plc forum account with ≥ 2 admins holding rotation keys).
- Followed forums are seeded from the user's own membership records; pingbacks are computed from rendered links.
- Explicit deferrals: recommendations, shareable drafts, authorOnly tags, the nofollow gate, `lockedToAccountsCreatedAfter`, karma from tag revisions.
- Wiki MVP uses linear history; a deleted parent makes the child a flagged root.

## Milestones (each ends with the checks green, then commit and push to `claude/exciting-keller-17oykr`)
Execution: each milestone runs as a workflow, with parallel agents on independent crates or packages, then an adversarial review and fix pass, then `cargo xtask ci` and e2e. After M3 freezes engine API v0, M4, M5 and M6 can overlap.

| M | Scope | Acceptance criteria |
|---|---|---|
| **M0 Spec, scaffold, spikes** | Commit the spec, ADRs and draft `.temp` lexicons (`goat lex lint`). Generate key vectors. Workspace, toolchain (`rustup target add wasm32-unknown-unknown`, wasm-bindgen-cli pinned, binaryen), CI skeleton, devnet script. Spikes runnable here: S3/S10 wasm size and math; S4 (lexicon tooling with the digit NSID; Jetstream wildcard; relay and Constellation queries, read-only); S5 devnet in CI; S6; S7/S9 (`listRecords` cursor behaviour and read limits on public bsky repos, read-only); S12 against the devnet. Spikes that need real accounts (S1 OAuth on bsky.social, S2 browser OAuth on production, S8, S11 Safari) are built and run against the devnet and recorded as "pending user run". | ADR per spike; `cargo xtask ci` green; devnet boots 3 accounts in < 30 s |
| **M1 Protocol foundations** | atp, lexicon plus validator, keys, identity (DoH/PLC/did:web), XRPC subset, NativeHost, auth (app password, OAuth loopback, hosted, no-browser, refresh lock), TermText. CLI: `login`, `logout`, `whoami`, `accounts`, `resolve`, `verify`, `doctor`. | Interop vectors pass. `verify` proves a real public bsky record and tampered fixtures fail. Devnet login works. Concurrent refresh is safe. |
| **M2 Walking skeleton** (text) | KvStores (SQLite, IndexedDB, memory); range sync; outbox; forum create/join/post/comment/edit/delete/retract; threads; basic LFM. CLI `read`, `ls`, `post`, `comment`, `sync`. TUI read and reply. Wasm worker, JsHost, AuthBridge, leader election. SPA read, login and comment. `lessdugin web` with the bridge. | Devnet parity scenario byte-identical across CLI, SPA and TUI. 50-member FakePds cold sync in < 5 s. No CSP violations. Wasm size reported. |
| **M3 Social core** | Opinions (undo, merge), `karma/1`, scores, agreement, `magic/1` sorts, Recent Discussion, `mod/1` (author tier, CID-bound, ban, lock, restrict, fail closed), approval and open modes, closure, newcomer collapse, mod queue/log/adopt, grants, notifications and unread, public-action interstitials, reacts with gates. Engine API v0 frozen. | All social vectors and property tests pass. With a mod PDS offline, spam stays hidden. Every action reachable in all three clients. Clean parity diffs. |
| **M4 Rich content and media** | Full LFM/1 (math, footnotes/sidenotes, ToC, alerts, spoilers, captions, hover previews, verified mentions); media pipeline (strip, orient, downscale, CID); autoload policy; history view and redaction; SPA editor; TUI images; `export`. | ≥ 150 LFM vectors plus XSS, mXSS and terminal-escape corpora. DOMPurify and ammonia leave output unchanged. 10-minute fuzz run without panics. EXIF, GPS and MP4 `udta` removed. |
| **M5 Organisation and discovery** | Tags, relevance and linear wiki; sequences; local search; forum directory; live Jetstream; open-mode discovery; `include:` scope with PAR fallback; hosted-SPA configuration (client metadata, `_headers`); mobile layout. | Open-mode devnet scenario passes with mini-Jetstream and fake Constellation. Cached forum renders in < 1.5 s. axe accessibility ≥ 90. |
| **M6 Optional indexer** | Ingest, backfill, poll mode; the XRPC list above; client assisted mode, cold start via proofs, `--audit`; Docker image; operator docs. | Devnet view converges within 5 s of a write. Identical views with and without the indexer. SPA cold-paints a 2k-participant synthetic forum in < 2 s. `--audit` catches an injected omission and a forged proof. |
| **M7 Release 0.1** (**needs the user; not run automatically**) | Security review; switch `.temp` to `NS`; publish lexicons and permission sets from a publisher account; DNS TXT `_lexicon.lessdugin.7778777.online` and `_lexicon.index.…`; deploy the site; release pipeline; guides. | `goat lex check-dns` passes; `include:` works on bsky.social; fresh-machine install → login → post appears in the hosted SPA and in `lessdugin web`. |

**Deferred**
- **P2:** Q&A answers; quick-take feed; co-authors (`karma/2`); side-comment and inline-react margin UI; moderation styles and guidelines; `ratelimit/1`; sealed reports; bulk mod actions; public bookmarks; standard.site syndication; labelers; thumbnails; forum lock; wiki forks and merges; personalised magic and tag filters.
- **P7:** verified MST range sync; PLC audit; `karmaSnapshot`.
- **P8+:** dialogues, events, polls, books, encrypted cross-device state, push notifications, private-ballot tally, RSS import/export.
- **Never:** LLM features, real-time collaborative editing, DMs (link to Bluesky chat instead).

## Verification (how we prove it works)
- **Unit tests and shared vectors.** `testdata/vectors/{keys,lexicon,lfm1,terminal,xss,karma,modfold,rank,reacts,tags,time,thread,proofs,limits}` run in Rust (testkit) and in TS via a wasm `vectors` build (vitest). Native and wasm results must be equal.
- **Property tests** (proptest): fold permutation invariance, idempotence, layering, sybil bound, incremental equal to batch, native equal to wasm.
- **Protocol tests.** Vendored atproto-interop-tests (syntax, data model, MST, signatures). jacquard-repo and `@atproto/repo` serve as test oracles for proofs. cargo-fuzz targets on CAR, CBOR, MST, the validator and the renderer.
- **CLI and TUI tests.** `assert_cmd` + `insta` against FakePds; ratatui `TestBackend` snapshots at 80×24 and 160×48.
- **End-to-end.** `pnpm --filter e2e devnet` starts `@atproto/dev-env` `TestNetworkNoAppView` (local PLC and PDS). The parity scenario:
  1. alice creates a forum (CLI).
  2. bob joins and posts with an image (SPA via Playwright, Chromium at `/opt/pw-browsers`).
  3. carol replies, votes, reacts and tags (TUI).
  4. A moderator hides and pins (CLI).
  5. Edits produce revisions.
  - With `now` and `obs` injected, CLI `--json`, SPA `getThread`, the TUI snapshot and indexer-hydrated output must match byte for byte.
  - Variants: approval and open mode, indexer on and off. Any `securitypolicyviolation` fails the test.
- **CI.** `ci.yml` runs fmt, clippy (native and wasm32), nextest, `cargo deny`, the wasm budget, biome, typecheck, vitest, the vite build and `goat lex lint`/`breaking`. `e2e.yml` runs the devnet tests and Playwright. `release.yml` and `pages.yml` are prepared but used only at M7.
- **Production smoke tests** on throwaway bsky.social accounts with `.temp` NSIDs need credentials from the user. They are listed as a manual step, alongside the spikes that need real accounts.

## Risks and user-dependent items
- **Library and protocol churn.** jacquard is alpha: pin it exactly, keep it behind a facade, and fall back to our own OAuth client. Wasm size: feature-gate, lazy-load `verify.wasm`. Constellation and Jetstream are best-effort: approval mode and the closure don't need them.
- **Rate limits.** If the PDS read limit is shared across Bluesky hosts, cold syncs slow down (S9). Mitigations: progressive paint, persistent cache, and indexer thresholds.
- **Social design limits.** Public votes change the social dynamics compared with ForumMagnum, so the UI explains it. Bans hide past content; an item-level keep mitigates this.
- **Needs the user.** DNS records for `lessdugin.7778777.online`; a publisher account with an offline rotation key; a static host account (Cloudflare Pages or Netlify preferred); test accounts for production smoke tests and the real-account spikes; and publishing at M7.

