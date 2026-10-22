<!-- SPDX-License-Identifier: CC0-1.0 -->
# lessdugin engine API and DTO schema (`dto/1`)

Status: normative for implementations in this repository; informative for other implementations
(they only need the data model, LFM/1, social and sync specs). Version: `dto/1`.

## 0. Purpose

One engine (`lessdugin-core::engine::Engine<H: Host>`) serves the CLI, the TUI, the SPA (through
`lessdugin-wasm` and `@lessdugin/engine`) and the optional indexer. This document fixes:

- the engine's public operations, grouped as in synthesis §6.5;
- the **DTO schema** returned by queries. The CLI's `--json` output *is* this schema, and the SPA
  receives the same objects (generated TypeScript types via `tsify`), so cross-client parity
  checks are byte comparisons of serialised DTOs (`docs/plan.md`, M2 acceptance);
- the event model used by every UI.

Normative details of the underlying behaviour live in `data-model.md` (records, writes),
`sync.md` (identity, discovery, sync, verification, store), `social.md` (the fold) and
`lfm-1.md` (rendering). This document only describes how they are exposed.

## 1. Conventions

- **Serialisation.** DTOs are `serde` structs/enums with `#[serde(rename_all = "camelCase")]`.
  Enums are internally tagged with `"kind"` (`#[serde(tag = "kind")]`) unless stated otherwise.
  Optional fields are omitted when `None` (`skip_serializing_if`). Maps are `BTreeMap` (sorted
  keys). Integers that may exceed 2^53 (none in `dto/1`) would be strings.
- **Times** are integer milliseconds since the Unix epoch (`…Ms` fields) — derived from the fold's
  µs values by floor division. `createdAt` strings are passed through as written only inside
  `raw` views.
- **Identifiers** are canonical strings: `did`, at-uri (`uri`), CID (`cid`).
- **Deterministic output.** For the same `(FoldInput, Query)` and the same local state, every
  query returns byte-identical JSON on every platform (`social.md` §1.3). Local-only fields
  (read marks, mutes, "mine") are computed from the local `state` store and are part of the
  parity scenario's injected state.
- **Versioning.** Every top-level response carries `"dto": 1`. Adding optional fields or enum
  variants is compatible; anything else bumps the version.
- **Errors.** Operations return `Result<T, EngineError>`; `EngineError` serialises as
  `{ "code": <string>, "message": <string>, "retryable": bool, "detail"?: object }` with codes:
  `auth_required`, `not_found`, `invalid_ref`, `invalid_input`, `conflict`, `rate_limited`,
  `network`, `blocked`, `too_large`, `partial` (results incomplete under the time/request budget;
  `detail` carries the partial result), `unsupported`, `internal`. CLI exit codes map:
  ok 0, other 1, usage 2, `auth_required` 3, `not_found` 4, `conflict` 5, `partial` 6.

## 2. Construction and configuration

```rust
Engine::new(host: H, config: EngineConfig) -> Engine<H>

EngineConfig {
  network: Network,                 // Production | Devnet { plc: Url, pds: Url } (handles resolve via the PDS)
  sources: Sources,                 // Direct | Assisted (default Assisted)
  verify: VerifyMode,               // Pds | Proof (default Pds)
  doh: Vec<Url>,                    // default [cloudflare-dns.com/dns-query, dns.google/resolve]
  plc: Url,                         // default https://plc.directory
  relays: Vec<Url>,                 // default bsky.network relays
  jetstream: Vec<Url>,              // default jetstream.us-east / us-west
  constellation: Option<Url>,       // default constellation.microcosm.blue (Assisted only)
  slingshot: Option<Url>,           // hint-only identity fallback
  indexers: Vec<IndexerConsent>,    // user-configured or consented forum indexers
  user_agent: String,
  platform: Platform,               // Native | Browser (concurrency limits, CORS rules)
  parity_now_ms: Option<i64>,       // test-only parity mode (§2.1); None in production
}
```

The engine keeps no global state; two engines over two hosts are independent.

### 2.1 Parity mode (test-only, devnet-only)

The parity scenario (`docs/plan.md` §4.2) compares clients that synced the same records at
different times. Two inputs of the fold would otherwise differ between them: `now` (ordering,
time windows) and each version's first-seen time `obs` (`social.md` §1, §2: future-dating).
`EngineConfig.parity_now_ms = Some(t)` injects both:

- every view (`list_posts`, `get_post`, `get_thread`, `recent_discussion`, `history`,
  `members`, `forum_open`, …) is computed at `now = t`, whatever the host clock says;
- every admitted version's `obs`, and the declaration's, is taken to be `t` (the stored
  first-seen times are not changed);
- the handles of the actors a view shows are verified by the query itself (`sync.md` §3.3),
  instead of being queued for `live_tail`, so the handle state does not depend on whether a
  client ran `live_tail`.

Sync, the outbox, record `createdAt` values, caches, TTLs and back-offs keep using the host
clock. Choose `t` later than every record of the scenario (a record whose `createdAt` is more
than `FUTURE_SKEW_MS` after `t` would be flagged `futureDated`), e.g. the scenario's start plus
one day, and give every client the same `t`.

Parity mode is **test-only** and is never enabled in production:

- **Native (CLI, TUI):** `LESSDUGIN_PARITY_NOW_MS=<ms since the epoch>`, read by
  `lessdugin_native::config::parity_now_ms`, which refuses it unless the configuration is in
  devnet mode (`--dev`, a devnet-enabled build). The CLI prints a warning when it is on.
- **SPA:** only an e2e build (a build-time flag, like the demo flag that gates the mock engine)
  passes `parityNowMs` to the wasm engine's configuration; release builds do not contain the
  code path.

## 3. Operations

All operations are `async` methods on `Engine<H>`; futures are `MaybeSend`. "Long-lived"
operations return a future the host must drive (`tokio::spawn` / `spawn_local`) and that ends
when the engine is dropped or the corresponding stop method is called.

### 3.1 Identity, accounts, refs

| Operation | Result |
|---|---|
| `resolve_actor(input: &str)` | `ActorView` (handle verification state included) |
| `accounts()` | `Vec<AccountView>` |
| `add_account(AccountInfo { did, pds, auth: AuthKind })` | called by host-specific login code after a successful login; verifies the login binding (`sync.md` §3.6) |
| `remove_account(did, purge: bool)` | – |
| `set_active_account(did)` / `active_account()` | – |
| `parse_ref(input, RefContext) -> Ref` | accepts at-uris, SPA URLs (`#/p/…`, `#/c/…`, `#/f/…`, `#/s/…`, `#/u/…`, `#/at/…`), `handle/rkey` (post in the current forum), `did`, `handle`. Listing indices (`3`, `c12`) are resolved by the CLI before calling. |

### 3.2 Forums and membership

| Operation | Result / notes |
|---|---|
| `forum_open(ref)` | `ForumView` header (§4.3). Resolves the declaration (first open fetches a proof), schedules authority sync. |
| `forum_discover(q)` | `Vec<ForumSummary>` (relay + owners' listings + indexers; grouped by owner) |
| `forum_follow(uri, alias?)`, `forum_unfollow(uri)`, `followed()` | local; `followed()` also seeds from the user's own `membership` records |
| `forum_create(ForumDraft)` | `WriteTicket`; writes the declaration and the owner's membership in one `applyWrites` |
| `forum_update(uri, ForumDraft)` | edit protocol |
| `join(uri, note?)`, `leave(uri)` | membership create / delete |
| `members(uri, MembersQuery { status?, pending? })` | `Vec<MemberView>` |
| `grant(uri, dids)`, `revoke(uri, dids)` | `#access{member}` / delete |
| `mods_set(uri, ModsChange)` | `mods rm` adopts first (`social.md` §4.10) |

### 3.3 Sync

| Operation | Notes |
|---|---|
| `sync_forum(uri, SyncOpts { budget_ms, max_requests, full })` | `SyncReport` |
| `sync_thread(post_uri, SyncOpts)` | P0 fetch of the thread's participants' ranges |
| `live_tail()` | long-lived: Jetstream / polling / indexer `getChanges`, feeds the dirty queue |
| `outbox_pump()` | long-lived: drains the outbox, one write in flight per account |
| `background_verify()` | long-lived; `verify = Proof` is Reserved (P2) (`sync.md` §2.3), so in v1 it is a no-op |

```
SyncReport { dto, forum, reposTotal, reposComplete, reposStale, reposUnavailable,
             requests, bytes, changed: u32, partial: bool, degraded: bool,
             errors: Vec<SyncIssue { did?, host?, code, message }> }
```

### 3.4 Queries (all return DTOs, §4)

| Operation | Result |
|---|---|
| `list_posts(ListPostsQuery { forum, sort, filter, tag?, kind?, author?, cursor?, limit })` | `PostList` |
| `recent_discussion(forum, cursor?, limit)` | `DiscussionList` |
| `get_post(uri)` | `PostView` |
| `get_thread(uri, ThreadOpts { sort, unreadOnly })` | `ThreadView` (comments in pre-order with depth) |
| `history(uri)` | `HistoryView` |
| `tags(forum)` / `tag(forum, slug)` | `Vec<TagSummary>` / `TagView` |
| `sequences(forum)` / `sequence(uri)` | `Vec<SequenceSummary>` / `SequenceView` |
| `user(forum?, actor)` | `UserView` |
| `search(SearchQuery { q, forum?, author?, limit })` | `SearchResults` |
| `notifications(NotificationsQuery { all, limit })` | `NotificationList` |
| `mod_queue(forum)` / `mod_log(forum, cursor?)` | `ModQueue` / `ModLog` |
| `raw(uri)` | `RawView` (record JSON, validity errors, every assertion incl. lapsed) |
| `drafts()` / `bookmarks()` / `subscriptions()` | local lists |

`sort`/`filter` values are exactly those of `social.md` §12 (`magic | top | new | old | recentComments`;
`frontpage | all | curated`; comment sorts `magic | top | new | old | latestReply`).

### 3.5 Writes

Writes go through the durable outbox (`data-model.md` §8). Each returns a `WriteTicket
{ id, state }` immediately after the operation is persisted; `state` progresses through
`Pending → Sending → Done { uri, cid } | Failed { error } | Undone` and is reported by
`Write` events.

| Operation | Notes |
|---|---|
| `publish(PostDraft)` | `PostDraft { forum, kind, title?, body: BodyDraft, link?, tags, labels }`; adds membership on first content (`data-model.md` §8) |
| `comment(CommentDraft)` | `{ root, parent?, body, quote? }` |
| `edit(uri, Draft, EditOpts { history: bool, summary? })` | edit protocol with revision snapshot |
| `delete(uri)` / `retract(uri)` | |
| `opinion(subject_uri, OpinionIntent)` | `Karma(v|clear) | Agreement(v|clear) | React{ id, stance, quote?, remove } | Tag{ slug, v|clear }`; 4 s undo window |
| `mod_action(ModActionIntent)` | per `data-model.md` §6.7 / §8.6 |
| `mod_adopt(forum, former_mod)` | |
| `wiki_edit(forum, slug, WikiDraft)` | |
| `seq_create`, `seq_edit`, `seq_add`, `seq_remove`, `seq_move` | |
| `redact_revision(uri)` | deletes revision records |
| `prepare_media(MediaInput) -> MediaRef` | sniff, strip, orient (native), CID; upload happens at publish |
| `undo(write_id)` | within the undo window |

`BodyDraft { markdown, media: Vec<MediaRef> }` — the engine performs spill-to-blob, mention
rewriting (`at://handle` → DID), derived `mentions`/`links`, and validation before persisting.

### 3.6 Local state

`mark_read(post_uri)`, `save_draft(Draft) -> draft_id`, `delete_draft(id)`, `bookmark(uri, on)`,
`subscribe(Subscription, on)`, `mute(did, on)`, `settings() / set_setting(key, value)`,
`export_local() -> Json`, `import_local(Json)`.

### 3.7 Pure functions (also exported to wasm)

`render(markdown, media, RenderOpts) -> RenderedBody`, `safe_href(raw, ctx)`, `parse_ref`,
`validate_record(collection, json)`, `derive_rkey(...)`, `verify(uri, deep) -> VerifyReport`
(network, but no state), `run_vector(kind, json) -> json` (tests only, feature `vectors`).

### 3.8 Events

`subscribe(cb: Fn(Event))`; events are delivered in order, never re-entrantly.

```
Event =
  | Changed { topics: Vec<Topic> }             // Topic = Forum(uri) | Thread(uri) | User(did)
                                               //       | Notifications | Outbox | Drafts | Accounts
  | SyncProgress { forum, done, total }
  | Write { id, state }
  | Auth { did, state: Ok | Expiring { inMs } | Expired }
  | Warning { kind, source?, message }         // e.g. equivocation, indexer contradiction, degraded
```

UIs re-query on `Changed` and never move the user's cursor on live updates (TUI banner
"N new — R"; SPA query invalidation).

### 3.9 Planned operations (not specified yet)

Clients asked for these; they arrive with the milestone that implements the behaviour behind
them, as compatible additions (`docs/plan.md`):

| Operation / field | Milestone | Notes |
|---|---|---|
| `mutes() -> Vec<ActorView>` | M3 | the local mute list behind `mute(did, on)` (§3.6) |
| `mark_notifications_read(ids)`, or all | M3 | notification read state (`social.md` §16) |
| viewer's own tag votes and tag relevance on `PostView` | M5 | `applies(p, t)` (`social.md` §13.3); until then a client only knows the votes it made itself |
| `backlinks(uri)` ("linked from") | M5 | listed posts linking to an item, from rendered bodies (`social.md` §16 Pingbacks) |

## 4. DTO schema (`dto/1`)

### 4.1 Common types

```
ActorView     { did, handle?, handleState: verified|unverified|invalid, displayName?, avatar?: MediaView,
                shortDid }                       // handle present only if verified both ways
ItemState     = { kind: "visible" }
              | { kind: "collapsed", reason: "newUser"|"lowScore"|"muted" }
              | { kind: "hidden", reason: HiddenReason }
              | { kind: "tombstone", reason: HiddenReason }
HiddenReason  = { kind: "notParticipant" } | { kind: "moderationUnavailable" }
              | { kind: "removed", rank: u8, actor?: did, spam: bool, reason?: string }
              | { kind: "banned" } | { kind: "restricted" } | { kind: "authorModerated" }
              | { kind: "locked" } | { kind: "newUser" }
Badges        { verified?: true, edited?: true, futureDated?: true, identityUnverified?: true,
                stale?: true, hint?: true, pending?: true, inconsistentTimestamps?: true }   // sync.md §1.6
ScoreView     { score: i64, agreement?: i64 /* absent when policy.voting = karmaOnly, social.md §7 */,
                unweighted: u32, earlierVersionVotes: u32,
                mine?: { karma?: KarmaValue, agreement?: AgreementValue } }
ReactTally    { react, label, glyph, for: u32, against: u32, quote?: TextQuote, anchor?: Anchor,
                creator?: did, mine?: "for"|"against" }
Anchor        { start: u32, end: u32 } | { orphan: true }    // code-point offsets into P
RenderedBody  { html, text, toc: Vec<TocEntry>, wordCount, readMinutes, media: Vec<MediaView>,
                excerpt, notices: Vec<string>, rendererVersion }
MediaView     { index, cid, mime, alt?, aspectRatio?, durationMs?, name?, author: did, pds,
                autoload: bool, sizeBytes }
Completeness  { partial: bool, reposComplete, reposTotal, degraded: bool }
```

The TUI renders from `RenderedBody` plus a natively computed `StyledDoc` (same renderer, same
source); `StyledDoc` is not part of the JSON schema.

### 4.2 Posts and threads

```
PostSummary   { uri, cid, forum, author: ActorView, kind, title?, excerpt, tags: Vec<slug>,
                tier: personal|frontpage|curated, pinned: bool, locked: bool,
                createdAtMs, tRankMs, commentCount, lastCommentAtMs?, score: ScoreView,
                state: ItemState, badges: Badges, unreadComments?: u32, link?: ExternalLinkView,
                labels: Vec<string> }
PostList      { dto, forum, sort, filter, items: Vec<PostSummary>, cursor?, completeness }
PostView      { dto, post: PostSummary, body: RenderedBody, reacts: Vec<ReactTally>,
                history: { revisions: u32, redacted: bool }, sequences: Vec<SequenceNav> }
CommentNode   { uri, cid, root: uri /* the thread the engine placed it in */, parent?: uri,
                depth: u32, author: ActorView, body: RenderedBody,
                quote?: TextQuote, quoteAnchor?: Anchor, createdAtMs, tRankMs, score: ScoreView,
                reacts: Vec<ReactTally>, state: ItemState, badges: Badges, retracted: bool,
                moderatorHat: bool, version?: { revision: u32 } | { unavailable: true },
                orphaned: bool, missing: bool, unread: bool, pinned: bool }
ThreadView    { dto, post: PostView | { missing: uri }, comments: Vec<CommentNode>, sort,
                completeness }
DiscussionList{ dto, items: Vec<{ post: PostSummary, comments: Vec<CommentNode> }>, cursor?, completeness }
HistoryView   { dto, uri, versions: Vec<{ n, cid, createdAtMs, summary?, available: bool }> }
```

### 4.3 Forums, users, organisation

```
ForumSummary  { uri, owner: ActorView, name, description?, membership: open|approval,
                followed: bool, member: bool, alias?,
                reportContact? /* as written; shown only through safe_href (Field) */ }
ForumView     { dto, forum: ForumSummary, about?: RenderedBody, moderators: Vec<{ actor, role, title?, shown: bool }>,
                policy: EffectivePolicy, myRole: owner|admin|moderator|member|established|newcomer|none,
                myKarma?: i64, banners: Vec<Banner>, completeness }
UserView      { dto, actor: ActorView, forum?, karma?: { k1, kp, k }, status, bio?: RenderedBody,
                links, pinned: Vec<PostSummary>, recentPosts: Vec<PostSummary>, recentComments: Vec<CommentNode> }
MemberView    { actor, status, granted: bool, membershipNote?, joinedAtMs }
TagSummary    { slug, name, description?, postCount, unmergedEdits }
TagView       { dto, tag: TagSummary, page?: RenderedBody, head?: uri, posts: Vec<PostSummary> }
SequenceSummary { uri, author, title, chapters: u32, posts: u32, score: ScoreView, pinned: bool }
SequenceView  { dto, sequence: SequenceSummary, body?: RenderedBody, chapters: Vec<{ title?, body?, posts: Vec<PostSummary> }> }
SequenceNav   { sequence: uri, title, prev?: uri, next?: uri }
SearchResults { dto, hits: Vec<{ item: PostSummary | CommentNode, snippet: string, highlights: Vec<[u32,u32]> }> }
NotificationList { dto, items: Vec<{ id, reason, atMs, subject: uri, actor?: ActorView, read: bool, preview?: string }> }
                 // reason: reply|mention|quote|pingback|newComment|newPost|reacted|modAction|karmaChange (social.md §16)
ModQueue      { dto, items: Vec<{ kind, subject, reason, detail }> }
ModLog        { dto, entries: Vec<{ actor, action, subject, value, reason?, atMs, lapsed: bool, adoptedFrom? }> }
RawView       { dto, uri, cid?, root?: uri /* a comment's thread, as placed by the engine */,
                level: pds|proven|hint, record?: json, validity: Vec<string>,
                assertions: Vec<…>, state?: ItemState }
VerifyReport  { dto, uri, result: proven|absent|failed, rev?, commitCid?, keySource, checks: Vec<{ name, ok, detail? }> }
```

## 5. Wasm binding (`@lessdugin/engine`)

- `lessdugin-wasm` exposes `WasmEngine` with one method per operation above (camelCase), returning
  Promises of the DTOs; types are generated with `tsify` into `pkg/lessdugin_engine.d.ts`.
- The engine runs in a Web Worker (`packages/engine/src/worker.ts`); the main thread holds the
  OAuth session (`auth-bridge.ts`) and serves `Target::UserPds` requests through an allowlist
  (`threat-model.md`). `@lessdugin/engine/client` wraps the worker with Comlink and re-exports
  the DTO types.
- Events are forwarded with `postMessage`; the React app maps `Changed` topics to TanStack Query
  invalidations.
