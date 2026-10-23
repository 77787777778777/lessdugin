<!-- SPDX-License-Identifier: CC0-1.0 -->
# lessdugin engine API and DTO schema (`dto/1`)

Status: normative for implementations in this repository; informative for other implementations
(they only need the data model, LFM/1, social and sync specs). Version: **engine API v0
(`dto/1`), frozen at M3**: from M3 on, the operations, DTOs and events below change only
compatibly (§1 "Freeze"). The operations a later milestone implements are listed with it
(§3.9); until then they reject with `unsupported`.

## 0. Purpose

One engine (`lessdugin-core::engine::Engine<H: Host>`) serves the CLI, the TUI, the SPA (through
`lessdugin-wasm` and `@lessdugin/engine`) and the optional indexer. This document fixes:

- the engine's public operations, grouped as in synthesis §6.5;
- the **DTO schema** returned by queries. The CLI's `--json` output *is* this schema, and the SPA
  receives the same objects (TypeScript types checked against the declarations `tsify`
  generates from the Rust DTOs, §5), so cross-client parity checks are byte comparisons of
  serialised DTOs (`docs/plan.md`, M2 acceptance);
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
- **Request fields.** Fields of operation inputs (§4.4) that the engine defaults
  (`#[serde(default)]`: `SyncOpts.full`, `PostDraft.tags`, `ModActionValue.hide.spam`, …) may be
  left out by callers; the TypeScript types mark them optional like the omitted output fields.
- **Versioning.** Every top-level view DTO (§4: `PostList`, `ThreadView`, `ForumView`, …)
  carries `"dto": 1`. Operation results that are not views, such as a `WriteTicket`, a
  `Vec<…>` list (`accounts()`, `writes()`, `bookmarks()`, …) or a bare value, are serialised
  without it, exactly as `dto.rs` serialises them; the engine and the wasm binding (§5) pass
  them on unchanged, and the CLI's `--json` wraps every object it prints in its own
  `{ "dto": 1, … }` envelope (`crates/cli/src/out.rs`).
- **Freeze (engine API v0, M3).** After M3 only compatible changes are made within `dto/1`:
  new operations; implementing an operation of §3.9 (it stops rejecting with `unsupported`); new
  optional output fields; new enum variants and new event kinds or topics (clients ignore what
  they do not know); new optional request fields whose default keeps the old behaviour; new
  error `detail` keys. Anything else (removing or renaming an operation, a field or a variant,
  changing a type, making an optional field required, changing what an operation means) is a
  new major version: `dto/2`, engine API v1. The Rust DTOs (`crates/core/src/dto.rs`) are the
  schema; their TypeScript form `testdata/dto/dto-1.d.ts` is generated from them
  (`crates/wasm/tests/dto_decl.rs`, which fails when it is stale) and the hand-written
  `packages/engine/src/types.ts` must match it field for field (`types.parity.test.ts`).
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
| `add_account(AccountInfo { did, pds, auth: AuthKind, issuer?, intended?, confirmIssuerChange })` | called by host-specific login code after a successful login; verifies the login binding (`sync.md` §3.6) |
| `remove_account(did, purge: bool)` | – |
| `set_active_account(did)` / `active_account()` | – |
| `acting_as(did) -> Engine` | Rust API only (not in the wasm bindings): a clone of the engine whose `active_account()` is the recorded account `did` (`not_found` otherwise) without changing the stored active account; its queries and writes act as that account (CLI `--account`, writes to a record by its owner) |
| `parse_ref(input, RefContext { forum? }) -> Ref` | accepts at-uris, SPA URLs (`#/p/…`, `#/c/…`, `#/f/…`, `#/s/…`, `#/u/…`, `#/at/…`, also inside a full URL), `handle/rkey` (post in `ctx.forum`; without one `invalid_ref`), `did`, `@handle`/`handle`. Listing indices (`3`, `c12`) are resolved by the CLI before calling. `Ref = forum{uri} \| post{uri} \| comment{uri} \| sequence{uri} \| record{uri} \| actor{did}` (tagged by `kind`). |

### 3.2 Forums and membership

| Operation | Result / notes |
|---|---|
| `forum_open(ref)` | `ForumView` header (§4.3). Resolves the declaration (first open fetches a proof), schedules authority sync. |
| `forum_discover(q)` | `Vec<ForumSummary>` (relay + owners' listings + indexers; grouped by owner). M5 (§3.9) |
| `forum_follow(uri, alias?)`, `forum_unfollow(uri)`, `followed()` | local; `followed()` also seeds from the user's own `membership` records |
| `forum_create(ForumDraft)` | `WriteTicket`; writes the declaration and the owner's membership in one `applyWrites` |
| `forum_update(uri, ForumDraft)` | `WriteTicket`. By the owner, through the edit protocol (a revision of the old version; unknown fields and the policy are kept): name, description, about, membership mode, default tier, featured tags, report contact. The slug is fixed; an absent or empty `moderators` keeps them, a different list is refused (`invalid_input`: use `mods_set`, which adopts first) |
| `join(uri, note?)`, `leave(uri)` | `WriteTicket`: membership create / delete. Joining (applying, in approval mode) is a public action (`social.md` §17, `PublicActionKind::Join`) |
| `members(uri, MembersQuery { status?, pending? })` | `Vec<MemberView>`: every DID with a valid membership record (`member(d)`, `social.md` §3.1), by DID, with its role, grant, karma and moderation flags; `pending: true` keeps the applicants (approval mode) not rejected on this device; `status` keeps one role |
| `grant(uri, actors)`, `revoke(uri, actors)` | `Vec<WriteTicket>`, one per actor (a DID or handle): `#access{member}` by the active account / its removal |
| `approve(uri, actors)` | `Vec<WriteTicket>`: approval mode, by an authority (`blocked` otherwise): `grant`, and the actors leave this device's rejected list |
| `reject(uri, actors)` | `Vec<WriteTicket>`: dismisses applicants on this device (nothing public is written for them; they leave `members { pending }`) and removes the active account's own grant of an actor, if any (the removals' tickets) |
| `rejected_applicants(uri)` | `BTreeSet<did>` (a JSON array): the applicants rejected on this device |
| `mods_set(uri, ModsChange)` | `ActionTicket` (§3.5). By the owner, through the edit protocol of the declaration. `Add` adds the entry or replaces the DID's (a role change). `Remove` first enqueues `mod_adopt` of the moderator's assertions, then the declaration edit, sent only once the adoption is done (it fails unsent if the adoption failed); the ticket carries an `adoptedFirst` notice naming the adoption's write. `skipAdopt` lets the assertions lapse at once (clients confirm first, listing them) (`social.md` §4.10) |

### 3.3 Sync

| Operation | Notes |
|---|---|
| `sync_forum(uri, SyncOpts { budget_ms, max_requests, full })` | `SyncReport`. User-initiated: stale collection lists are refreshed (`sync.md` §5.2) and up to `REJECTED_RECHECK_BATCH` rejected candidates are confirmed again, cheapest first (`sync.md` §4.2.1) |
| `sync_thread(post_or_comment_uri, SyncOpts)` | P0 fetch of the thread's participants' ranges; opens an unknown forum from the record itself; `degraded` reflects the state after the sync |
| `live_tail()` | long-lived: Jetstream / polling / indexer `getChanges`, feeds the dirty queue; runs `live_step` rounds until `live_stop()` |
| `live_step() -> LiveRound` | one `live_tail` round (seeding, head polls, reconciles, up to `LIVE_TAIL_BATCH` queued syncs, handle verification, tombstone GC); for one-shot use (`tail --once`). `LiveRound { polled, reconciles, synced, changed: Vec<forum uri>, verified, gc, nextMs }` is a Rust value, not a DTO |
| `live_stop()` | sync; stops every running `live_tail` after its current round; a `live_tail` started later runs |
| `verify_handles(n) -> usize` | verifies the handles of up to `n` shown authors queued by views (`sync.md` §3.3); emits `Changed` when a handle state changed; `live_tail` calls it each round |
| `outbox_pump()` | long-lived: drains the outbox of every logged-in account, one write in flight per account (`sync.md` §7.1, across processes and tabs through `Host::lock`); picks up account changes; never returns an error; ends on `outbox_stop()` |
| `outbox_stop()` | sync; stops every running pump after its current round; a pump started later runs |
| `outbox_flush() -> PumpReport` | one-shot (CLI, tests): prunes finished writes older than `DONE_KEEP_MS`, then runs every account's outbox until nothing is pending, sleeping through holds and back-offs; an account whose writes wait for a new login (below) is skipped. `PumpReport { processed, done, failed, conflicts, throttled, needsLogin }` is a Rust value, not a DTO |
| `outbox_expedite() -> u32` | "send now" (`lessdugin outbox flush`): ends the back-off of every write that waits after an ambiguous or transient failure, so the next step tries it at once. The undo window, the write budget, a PDS rate limit and a refused session are not shortened. Returns how many writes it expedited |
| `background_verify()` | long-lived; `verify = Proof` is Reserved (P2) (`sync.md` §2.3), so in v1 it returns at once |
| `external_change(topics)` | sync; another engine over the same stores (another browser tab, `sync.md` §10.6) emitted `Changed { topics }`: drops the cached models those topics touch (a `Thread` or `User` topic drops every cached forum model). Emits nothing; the caller forwards the event to its own subscribers |

Clients start `outbox_pump()` and `live_tail()` once at start-up (before or after login) and stop them with `outbox_stop()` / `live_stop()`; one-shot CLI commands use `outbox_flush()` and `live_step()` instead.

```
SyncReport { dto, forum, reposTotal, reposComplete, reposStale, reposUnavailable,
             requests, bytes, changed: u32, partial: bool, degraded: bool,
             errors: Vec<SyncIssue { did?, host?, code, message }> }
```

### 3.4 Queries (all return DTOs, §4)

| Operation | Result |
|---|---|
| `list_posts(ListPostsQuery { forum, sort, filter, tag?, kind?, author?, cursor?, limit })` | `PostList`: the listed posts (`Visible` or `Collapsed`, `social.md` §10.1) passing `filter`, `tag` (`applies(p, t)`, §13.3: a tag applies only once its wiki page exists), `kind` and `author` (a DID or handle), in `sort` order (§12.2) |
| `recent_discussion(forum, cursor?, limit)` | `DiscussionList`: the full fold's Recent Discussion (§12.5), each post with its `DISCUSSION_COMMENTS` newest `Visible` comments |
| `get_post(uri)` | `PostView` |
| `get_thread(uri, ThreadOpts { sort, unreadOnly })` | `ThreadView` (comments in pre-order with depth) |
| `history(uri)` | `HistoryView` |
| `tags(forum)` / `tag(forum, slug)` | `Vec<TagSummary>` / `TagView`. M5 (§3.9) |
| `sequences(forum)` / `sequence(uri)` | `Vec<SequenceSummary>` / `SequenceView`. M5 |
| `user(forum?, actor)` | `UserView`. M5 |
| `search(SearchQuery { q, forum?, author?, limit })` | `SearchResults`. M5 |
| `notifications(NotificationsQuery { all?, limit })` | `NotificationList` (`social.md` §16): the active account's notifications in its followed forums, newest first (then by id), unread only unless `all`, at most `limit`; `unread` counts every unread one (the badge). Empty without an active account. Computed lazily: it first brings the stored notifications up to date over the cached models (it does not sync); clients re-query on `Changed` with a `Notifications`, `Forum` or `Thread` topic. `atMs` is when the item was first seen (never its `createdAt`) or when the change was noticed |
| `unread_counts()` | `UnreadCounts`: the notification badge and, per followed forum (by uri), the listed posts first seen since its post list was last visited, the listed comments first seen since each visited thread was last viewed (threads never viewed count as new posts) and its unread notifications; the active account's own items never count. A forum's first count records the visit mark. Brings the notifications up to date first |
| `mod_queue(forum)` | `ModQueue` (`social.md` §4.12): applicants, top-rank unordered conflicts, stale positive assertions and, in open mode, the newest newcomer items. The forum's authorities only (`blocked` for anyone else) |
| `mod_log(forum, cursor?, limit)` | `ModLog` (`social.md` §4.12), `(time desc, uri asc)`, paged like `list_posts`. Public, except the `reason` of a `#hide` with `showReason: false`, shown to the forum's authorities only |
| `raw(uri)` | `RawView`: the stored record JSON, its validity violations, every modAction naming it (valid or not, each with its mod-log status), its state and its score. Open to every viewer: the raw view shows everything, hidden content included (`social.md` §10.3) |
| `bookmarks()` | `Vec<BookmarkEntry>`: newest first, each with the post's summary when its forum's model is cached |
| `bookmarked(uri)` | `bool` |
| `subscriptions()` | `Vec<SubscriptionEntry>`, oldest first |
| `mutes()` | `Vec<ActorView>`: the muted actors (the lens, `social.md` §10.3), by DID |
| `drafts()` | `Vec<DraftEntry>`: the engine's draft store. M4 (§3.9); until then the CLI keeps drafts as files and the SPA in browser storage |

`sort`/`filter` values are exactly those of `social.md` §12 (`magic | top | new | old | recentComments`;
`frontpage | all | curated`; comment sorts `magic | top | new | old | latestReply`).

**Defaults (M3).** `ListPostsQuery` defaults to `sort = magic`, `filter = frontpage`;
`ThreadOpts` to `sort = magic`. Every client opens a forum's post list with these defaults (the
CLI's `ls`, the TUI's forum screen, the SPA's `web/src/lib/listDefaults.ts`), so the three show
the same posts for the same forum; the parity scenario lists with explicit values.

**One fold per forum.** The engine folds a forum once per change of its cache (`social.md` §1,
the full fold with the empty query) and answers every query from that model: a sort, filter,
tag, kind, author or thread is re-ranked from the cached layers (`full::reorder`), which gives
the same orderings as a fold with that query. Refolds are single-flight per forum, throttled by
the last fold's cost (a forum whose fold took `c` ms is not refolded more often than every `c`
ms, at most 5 s apart, and its model is reused for up to `30 c` ms or `MODEL_MAX_AGE_MS`), and a
fold that rejects its input (a malformed `FoldInput`, §1.1) keeps the last good model and logs
the error. The model's `now` is the view time (`parity_now_ms` in parity mode), so the same
cache, local state and clock give byte-identical JSON on every engine.

**Paging.** `list_posts`, `recent_discussion` and `mod_log` page with an opaque `cursor` (the
next page's offset) and `limit` (0: `LIST_PAGE_DEFAULT`, at most `LIST_PAGE_MAX`).

**Viewer.** Views are computed for the active account (the viewer): its own opinions (`mine`),
react stances, read marks (`unread`, `unreadComments`), mutes (the lens: a muted author's listed
items are `collapsed/muted`, `social.md` §10.3) and `ForumView.viewer` capabilities. Hidden and
tombstoned items carry no author content (title, link, tags, labels, body, reacts) and a zero
score for viewers who are not the forum's authorities; `raw` shows everything.

### 3.5 Writes

Writes go through the durable outbox (`data-model.md` §8). Each returns a `WriteTicket
{ id, state }` immediately after the operation is persisted; `state` progresses through
`Pending → Sending → Done { uri, cid } | Failed { error } | Undone` and is reported by
`Write` events.

`Pending { notBeforeMs?, held?, throttled?, needsLogin?, error? }` says why a write waits:
`held` (the undo window), `throttled` (the write budget, a rate limit, or a back-off after an
ambiguous or transient failure), or `needsLogin`; `error` is the failure that caused a
`throttled` or `needsLogin` wait. **`needsLogin`:** the PDS or the host refused the account's
session (`auth_required`: expired, revoked, logged out). The write is not retried and does not
count as an attempt; nothing else of that account is sent (writes stay in order), the engine
emits `Auth { did, state: Expired }`, and the next login of the account (`add_account`,
`record_account`) queues the held writes again.

| Operation | Notes |
|---|---|
| `publish(PostDraft)` | `PostDraft { forum, kind, title?, body: BodyDraft, link?, tags?, labels? }`; adds membership on first content (`data-model.md` §8) |
| `comment(CommentDraft)` | `{ root, parent?, body, quote? }` |
| `edit(uri, Draft, EditOpts { history: bool, summary? })` | edit protocol with revision snapshot |
| `delete(uri)` / `retract(uri)` | |
| `opinion(subject_uri, OpinionIntent) -> ActionTicket` | the active account's opinion on a cached post, comment, wiki revision or sequence (`data-model.md` §8.5): `Karma { value? }`, `Agreement { value? }` (an absent value clears the axis), `React { id, stance, quote?, remove? }` (`quote.exact` only: the engine derives `prefix`, `suffix` and `start` from the item's current text), `Tag { slug, value? }`. Held for the 4 s undo window (`OPINION_UNDO_WINDOW_MS`, `Pending { held }`); an intent on a subject whose write is still held joins it (the ticket is that write's). When sent, the record is re-read and the intents re-applied per axis, so two devices' opinions merge; an opinion left empty is deleted; `subject.cid` is the cached version's. Refused before enqueueing when it could never count: `blocked` (banned, nullified, restricted, a gate of `ViewerCaps`), `invalid_input` (an own item, a forum without agreement or reacts, an unknown react, nothing to react against, an unknown value), `invalid_ref` (not an opinion subject), `not_found` (not cached). A vote that does not carry weight yet is recorded with a `notEstablished` notice ("recorded; will count once you're established"). A public action (§3.6) |
| `mod_action(ModActionIntent) -> ActionTicket` | `Assert { forum, scope, subject, action: ModActionValue, reason?, untilMs? }` creates the active account's record for (scope, subject, action key), or updates it through the edit protocol; `Remove { forum, scope, subject, actionKey }` deletes it through the removal protocol (unhide, unban, unpin, unlock, unrestrict, …) (`data-model.md` §6.7, §8.4, §8.6). `subject` is a DID or handle, an at-uri or `tag:<slug>`; a positive assertion is bound to the subject's cached CID. Refused before enqueueing when it would have no effect: `blocked` (forum scope needs an authority with the rank and role, `social.md` §4.3; author scope the §4.9 conditions), `invalid_input` (a subject of the wrong kind, an action author scope does not allow, a lock that would keep more than 5,000 comments: `detail.hint = "useHide"`, offer hiding the thread), `not_found` (a positive assertion on an uncached subject, or removing an assertion the account does not have). Bans and restrictions carry a `retroactive` notice. `lock.allowed` is computed by the engine. A public action (§3.6) |
| `mod_adopt(forum, former) -> WriteTicket` | by an authority: re-issues the former (or departing) moderator's (a DID or handle) non-lapsed forum-scope assertions with `adopts` pointing at each original (`social.md` §4.10); what could not be adopted is reported in the write's result |
| `wiki_edit(forum, slug, WikiDraft)` | M5 (§3.9) |
| `seq_create`, `seq_edit`, `seq_add`, `seq_remove`, `seq_move` | M5 |
| `redact_revision(uri)` | deletes revision records. M4 |
| `prepare_media(MediaInput) -> MediaRef` | sniff, strip, orient (native), CID; upload happens at publish. M4 |
| `undo(write_id)` | removes a write not sent yet (the undo window), or writes the inverse of a sent opinion (`sync.md` §7.4) |
| `retry_write(write_id)` | queues a `failed` (or conflicting) write again with its attempts reset; on a write that backs off after a failure, or waits for a login, it is "send now" (queued at once, attempts kept). A write held by the undo window, the write budget or a rate limit is refused (`invalid_input`). Returns its ticket |
| `write_status(write_id) -> Option<WriteTicket>` | `None`: unknown or pruned |
| `writes() -> Vec<WriteTicket>` | every write in the outbox, by account then `seq` (the CLI's `outbox` listing, the SPA's pending badges) |

`BodyDraft { markdown, media?: Vec<MediaRef> }` — the engine performs spill-to-blob, mention
rewriting (`at://handle` → DID), derived `mentions`/`links`, and validation before persisting.

**`ActionTicket { id, state, notices? }`** is a `WriteTicket` (same `id`, same `state`, the same
`Write` events) plus the notices clients show with the result: `ActionNotice { code, message }`
with `code` one of `notEstablished`, `retroactive`, `adoptedFirst` (stable) and `message`
English text. `opinion`, `mod_action` and `mods_set` return it.

### 3.6 Local state

Kept in the `state` store of this device, never written to a PDS. Every operation emits the
`Changed` topics named here.

| Operation | Notes |
|---|---|
| `mark_read(uri)` | a post: `lastVisited` becomes the greatest first-seen time among the post and its listed comments (never lower than before), and the active account's notifications about the thread are read (`Changed [Thread, Forum]`, plus `Notifications`). A forum: its post list was visited (the new-post count of `unread_counts` restarts) (`Changed [Forum]`) (`social.md` §16) |
| `mark_notifications_read(ids)` | the active account's notifications `ids` (unknown ids are ignored); `Changed [Notifications]` if one changed |
| `mark_all_notifications_read()` | every notification of the active account |
| `bookmark(uri, on)` | `Changed [Thread]` |
| `subscribe(Subscription, on)` | new posts in a forum, by a user or with a tag, new comments on a post, new entries of a sequence (`social.md` §16); `Changed [Notifications]`. In TypeScript `setSubscription`, since `subscribe` is the event subscription (§3.8) |
| `mute(actor, on)` | a DID or handle; the lens collapses the actor's listed items (`social.md` §10.3); `Changed` for the user and every cached forum |
| `acknowledged_public_actions() -> Vec<PublicActionKind>` | the "this is public" interstitials the active account has seen (`social.md` §17, `threat-model.md` PRIV-9), in `PublicActionKind` order (`vote`, `react`, `tagVote`, `join`, `modAction`); empty without an active account |
| `public_action_acknowledged(kind) -> bool` | |
| `acknowledge_public_action(kind)` | per account, shared by every client on this device; `auth_required` without an active account; `Changed [Accounts]` |
| `karma_notifications() -> KarmaCadence`, `set_karma_notifications(cadence)` | `realtime`, `daily` (the default) or `weekly` (`social.md` §16); `Changed [Notifications]` |
| `save_draft(Draft) -> draft_id`, `delete_draft(id)` | M4 (§3.9) |
| `settings()`, `set_setting(key, value)` | M4 |
| `export_local() -> Json`, `import_local(Json)` | M4 |

**Public actions.** Votes and agreement, reacts, tag votes, joining or applying, and mod actions
are public (`social.md` §17). Before the first action of a kind, every client shows the "this is
public" interstitial unless `public_action_acknowledged(kind)`, then calls
`acknowledge_public_action(kind)`; `OpinionIntent::public_kind()` names the kind of an intent.
The engine does not enforce it.

### 3.7 Pure functions (also exported to wasm)

`render(markdown, media, RenderOpts) -> RenderedBody` (media: M4), `safe_href(raw, ctx)`,
`parse_ref`, `verify(uri, deep) -> VerifyReport` (network, but no state);
`validate_record(collection, json)` and `derive_rkey(...)` (M4); `run_vector(kind, json) -> json`
(tests only).

### 3.8 Events

`subscribe(cb: Fn(Event))`; events are delivered in order, never re-entrantly.

```
Event =
  | Changed { topics: Vec<Topic> }             // Topic = Forum(uri) | Thread(uri) | User(did)
                                               //       | Notifications | Outbox | Drafts | Accounts
  | SyncProgress { forum, done, total }         // repos synced so far / planned so far
  | Write { id, state }                         // always followed by Changed [Outbox]
  | Auth { did, state: Ok | Expiring { inMs } | Expired }
  | Warning { kind, source?, message }         // e.g. equivocation, indexer contradiction, degraded
```

UIs re-query on `Changed` and never move the user's cursor on live updates (TUI banner
"N new — R"; SPA query invalidation).

- **`SyncProgress`.** `sync_forum` and `sync_thread` emit it as repos finish: `done` is the
  number of repos synced so far and `total` the number planned so far. `total` grows as each
  step of `sync.md` §4.5 plans its candidates, so the ratio can go down. The last event of a
  command always has `done == total`, also when the command fails; it is followed by
  `Changed [Forum (, Thread)]`. The counts are progress only; completeness is the report's
  `reposComplete` of `reposTotal`.
- **`Write`.** Every outbox state change emits `Write { id, state }`, then `Changed [Outbox]`.
  For an applied own write, the forum, thread and user topics come after `Write { done }`, so a
  re-query no longer shows the item as pending.
- **Accounts.** `add_account`, `record_account`, `set_active_account` and `remove_account` emit
  `Changed [Accounts]` (plus `Outbox` when a purge dropped writes).
- **`Auth`.** `Auth { did, state: Expired }` is emitted when the outbox finds the account's
  session refused (its writes then wait with `needsLogin`, §3.5). Clients show "session
  expired, log in again" (the CLI exits 3 with that hint). `Ok` and `Expiring` are Reserved.
- **Handles.** Handle verification (`verify_handles`, `live_tail`) emits `Changed` for the users
  whose handle state changed and for the cached forums.
- **Social writes and local state (M3).** `opinion`, `mod_action`, `mod_adopt`, `mods_set`,
  `approve` and `reject` report through `Write` events like every write (an opinion stays
  `pending { held }` for the undo window). The local-state operations emit the topics of §3.6.
  Notifications and unread counts are computed lazily: clients re-query `notifications` and
  `unread_counts` on `Changed` with a `Notifications`, `Forum` or `Thread` topic.

### 3.9 Not implemented in engine API v0, and the Rust-only API

**Unsupported operations.** These are part of the API (`EngineApi`, and the Rust API where it
exists) but not implemented yet: they reject with `unsupported` (`retryable: false`,
`detail.milestone`), and the milestone that brings the behaviour behind them implements them,
a compatible change (§1). Clients hide or stub what they need of them meanwhile.

| Operations | Milestone |
|---|---|
| `forum_discover` | M5 (the forum directory) |
| `tags`, `tag`, `wiki_edit` | M5 (tags and the wiki) |
| `sequences`, `sequence`, `seq_create`, `seq_edit`, `seq_add`, `seq_remove`, `seq_move` | M5 |
| `user` | M5 (user pages, with profiles) |
| `search` | M5 |
| `prepare_media`, `redact_revision`, `render` with media | M4 (the media pipeline; history and redaction) |
| `drafts`, `save_draft`, `delete_draft` | M4 (the editor; the CLI keeps drafts as files, the SPA in browser storage) |
| `settings`, `set_setting` | M4 (the media autoload and external-image policies; the karma cadence and the public-action acknowledgements already have their own operations, §3.6) |
| `export_local`, `import_local` | M4 (`export`) |
| `validate_record`, `derive_rkey` | M4 |
| `run_vector` | never in a release module (the social vectors run natively and in the wasm32 test harness) |

**Planned additions** (compatible, not in v0):

| Operation / field | Milestone | Notes |
|---|---|---|
| viewer's own tag votes and tag relevance on `PostView` | M5 | `applies(p, t)` (`social.md` §13.3); until then a client only knows the votes it made itself |
| `backlinks(uri)` ("linked from") | M5 | listed posts linking to an item, from rendered bodies (`social.md` §16 Pingbacks) |

**Rust API only** (native hosts and tests; not dispatched by the wasm binding): `acting_as`
(§3.1), the native login plumbing (`bind_login`, `record_account`, `refresh_account`,
`account`), identity helpers (`resolve_identity`, `resolve_identity_fresh`,
`invalidate_identity`), `read_mark(post)` (the read mark in µs behind `unread` and
`unreadComments`) and `muted()` (the DIDs behind `mutes()`). `live_step` and `outbox_flush`
are reachable from the worker as glue (§5).

## 4. DTO schema (`dto/1`)

### 4.1 Common types

```
ActorView     { did, handle?, handleState: verified|unverified|invalid,
                claimedHandle? /* only while unverified; untrusted, shown struck through (sync.md §3.1) */,
                displayName?, avatar?: MediaView,
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
ScoreView     { score: i64 /* includes the implicit self-vote, social.md §7 */,
                agreement?: i64 /* absent when policy.voting = karmaOnly, social.md §7 */,
                unweighted: u32 /* "n votes not yet counted" */,
                earlierVersionVotes: u32, weightedVotes?: u32 /* N and M of "N of M votes were
                cast on an earlier version"; M omitted when 0 */,
                mine?: MyOpinion }
MyOpinion     { karma?: KarmaValue, agreement?: AgreementValue,
                pending?: true /* a write of it waits in the outbox */,
                notYetCounted?: true /* "recorded; will count once you're established" */ }
ReactTally    { react, label, glyph, for: u32 /* |supporters|, creator included */,
                against: u32 /* |opposers| */, quote?: TextQuote /* the creator's */,
                anchor?: Anchor, creator?: did, mine?: "for"|"against",
                uncounted?: true /* the viewer's own react, which forms no valid react */ }
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
PostList      { dto, forum, sort, filter, items: Vec<PostSummary>, cursor?, completeness,
                banners?: Vec<Banner> }
PostView      { dto, post: PostSummary, body: RenderedBody, reacts: Vec<ReactTally>,
                history: { revisions: u32, redacted: bool }, sequences: Vec<SequenceNav> }
CommentNode   { uri, cid, root: uri /* the thread the engine placed it in */, parent?: uri,
                depth: u32, author: ActorView, body: RenderedBody,
                quote?: TextQuote, quoteAnchor?: Anchor, createdAtMs, tRankMs, score: ScoreView,
                reacts: Vec<ReactTally>, state: ItemState, badges: Badges, retracted: bool,
                moderatorHat: bool, version?: { revision: u32 } | { unavailable: true },
                orphaned: bool, missing: bool, unread: bool, pinned: bool }
ThreadView    { dto, post: PostView | { missing: uri }, comments: Vec<CommentNode>, sort,
                completeness, banners?: Vec<Banner> }
DiscussionList{ dto, items: Vec<{ post: PostSummary, comments: Vec<CommentNode> }>, cursor?, completeness,
                banners?: Vec<Banner> }
Banner        { code: "moderationUnavailable" /* degraded, social.md §4.11 */ | "newerRules", message }
HistoryView   { dto, uri, versions: Vec<{ n, cid, createdAtMs, summary?, available: bool }> }
```

### 4.3 Forums, users, organisation

```
ForumSummary  { uri, owner: ActorView, name, description?, membership: open|approval,
                followed: bool, member: bool, alias?,
                reportContact? /* as written; shown only through safe_href (Field) */ }
ForumView     { dto, forum: ForumSummary, about?: RenderedBody, moderators: Vec<{ actor, role, title?, shown: bool }>,
                policy: EffectivePolicy, myRole: ForumRole, myKarma?: i64, banners: Vec<Banner>,
                completeness, viewer?: ViewerCaps /* absent without an active account */ }
ForumRole     = owner|admin|moderator      // A, by rank
              | member|established|newcomer // with a valid membership record: S, E ∖ S, P ∖ (S ∪ E)
              | none                       // (a grant without a membership record is "none")
ViewerCaps    { vote, strongVote, agree, weighted /* in E */, react, reactCreate, reactSecond,
                reactAnti, post, comment, newcomer, applicant, moderate, rank?: 1|2|3, curate,
                authorModPersonal, authorModFrontpage }   // all bool; gates as action enablement
                // (social.md §4.9, §5, §8): clients disable what would not count
UserView      { dto, actor: ActorView, forum?, karma?: { k1, kp, k }, status, bio?: RenderedBody,
                links, pinned: Vec<PostSummary>, recentPosts: Vec<PostSummary>, recentComments: Vec<CommentNode> }
MemberView    { actor, status: ForumRole, granted: bool, membershipNote?, joinedAtMs,
                karma?: i64 /* omitted when 0 */, applicant?: true, banned?: true,
                nullified?: true, restricted?: Vec<capability> }
TagSummary    { slug, name, description?, postCount, unmergedEdits }
TagView       { dto, tag: TagSummary, page?: RenderedBody, head?: uri, posts: Vec<PostSummary> }
SequenceSummary { uri, author, title, chapters: u32, posts: u32, score: ScoreView, pinned: bool }
SequenceView  { dto, sequence: SequenceSummary, body?: RenderedBody, chapters: Vec<{ title?, body?, posts: Vec<PostSummary> }> }
SequenceNav   { sequence: uri, title, prev?: uri, next?: uri }
SearchResults { dto, hits: Vec<{ item: PostSummary | CommentNode, snippet: string, highlights: Vec<[u32,u32]> }> }
NotificationList { dto, items: Vec<NotificationView>, unread: u32 /* every unread one: the badge */ }
NotificationView { id, reason: NotificationReason, atMs /* first seen, or when the change was
                 noticed; never createdAt */, subject: uri, forum?: uri, actor?: ActorView,
                 read: bool, preview?: string }
NotificationReason = reply|mention|quote|pingback|newComment|newPost|reacted|modAction|karmaChange
                                                                  // social.md §16
UnreadCounts  { dto, notifications: u32, forums: Vec<ForumUnread> /* by forum uri */ }
ForumUnread   { forum: uri, posts: u32, comments: u32, notifications: u32 }
BookmarkEntry { uri, savedAtMs, post?: PostSummary }
SubscriptionEntry { subscription: Subscription, sinceMs }
ModQueue      { dto, items: Vec<{ kind: applicant|conflict|stale|newcomer, subject, reason,
                detail?, uri? /* the stale modAction */, actor?: ActorView, atMs? }> }
ModLog        { dto, entries: Vec<ModLogEntry>, cursor? }
ModLogEntry   { uri /* modAction or revision */, actor: ActorView, action, subject, value,
                reason?, atMs, status: ModLogStatus, lapsed: bool, rank? /* the winner's, when
                overridden */, authorScope?: true, adoptedFrom?: uri }
ModLogStatus  = effective|overridden|lapsed|expired|stale|unauthorised|reservedUnknown
              | edited|removed                                   // social.md §4.12
RawView       { dto, uri, cid?, root?: uri /* a comment's thread, as placed by the engine */,
                level: pds|proven|hint, record?: json, validity: Vec<string>,
                assertions: Vec<{ uri, cid, valid, record, rule?, status?: ModLogStatus }>,
                state?: ItemState, score?: ScoreView }
VerifyReport  { dto, uri, result: proven|absent|failed, rev?, commitCid?, keySource, checks: Vec<{ name, ok, detail? }> }
```

### 4.4 Operation inputs and results

Optional request fields (`?`) may be left out; the default is named where it is not "none".

```
AccountInfo   { did, pds, auth: oauth|appPassword, issuer?, intended?, confirmIssuerChange? /* false */ }
AccountView   { did, handle?, pds, authKind: oauth|appPassword, active: bool }
Ref           = { kind: forum|post|comment|sequence|record, uri } | { kind: "actor", did }
RefContext    { forum? }
ForumDraft    { slug, name, description?, about?: BodyDraft, membership: open|approval,
                defaultTier: personal|frontpage, featuredTags?: Vec<slug>,
                moderators?: Vec<{ did, role: admin|moderator, title? }>, reportContact? }
MembersQuery  { status?: ForumRole, pending?: bool }
ModsChange    = { kind: "add", did, role: admin|moderator, title? }
              | { kind: "remove", did, skipAdopt? /* false */ }
SyncOpts      { budgetMs?, maxRequests?, full? /* false */ }
ListPostsQuery { forum, sort: PostSort, filter: PostFilter, tag?: slug, kind?: PostKind,
                 author? /* DID or handle */, cursor?, limit /* 0: LIST_PAGE_DEFAULT */ }
ThreadOpts    { sort: CommentSort, unreadOnly? /* false */ }
NotificationsQuery { all? /* false: unread only */, limit }
BodyDraft     { markdown, media?: Vec<MediaRef> }
MediaRef      { id, cid, mime, sizeBytes, alt?, aspectRatio?, durationMs?, name? }
PostDraft     { forum, kind: PostKind, title?, body: BodyDraft, link?: { uri, title?, description? },
                tags?: Vec<slug>, labels?: Vec<string> }
CommentDraft  { root, parent?, body: BodyDraft, quote?: TextQuote }
Draft         = { kind: "post", id?, post: PostDraft } | { kind: "comment", id?, comment: CommentDraft }
EditOpts      { history: bool, summary? }
OpinionIntent = { kind: "karma", value?: up|down|strongUp|strongDown }      // absent: clear
              | { kind: "agreement", value?: agree|disagree|strongAgree|strongDisagree }
              | { kind: "react", id /* palette id */, stance: for|against, quote?: TextQuote,
                  remove? /* false */ }
              | { kind: "tag", slug, value?: KarmaValue }
ModActionIntent = { kind: "assert", forum, scope: forum|author, subject /* DID or handle,
                    at-uri or tag:<slug> */, action: ModActionValue, reason?, untilMs? }
                | { kind: "remove", forum, scope: forum|author, subject, actionKey }
ModActionValue = { kind: "hide", hidden, spam? /* false */, showReason? /* true */ }
               | { kind: "access", level: banned|member }
               | { kind: "restrict", capabilities: Vec<post|comment|vote|react|tag|wiki|sequence>,
                   nullifyVotes? /* false */ }
               | { kind: "tier", tier: Tier, notice?: BodyDraft }
               | { kind: "pin", scope: forum|thread, pinned }
               | { kind: "lock", locked }
               | { kind: "tag", removed, head?: uri /* a wikiRevision */ }
               // actionKey: the kind, or "pin:forum" / "pin:thread" (data-model.md §3.1)
WriteTicket   { id, state: WriteState }
WriteState    = { kind: "pending", notBeforeMs?, held?: true, throttled?: true, needsLogin?: true,
                  error?: EngineError } | { kind: "sending" } | { kind: "done", uri, cid? }
              | { kind: "failed", error: EngineError } | { kind: "undone" }
ActionTicket  { id, state: WriteState, notices?: Vec<ActionNotice> }
ActionNotice  { code: notEstablished|retroactive|adoptedFirst, message }
Subscription  = { kind: "forum", uri } | { kind: "post", uri } | { kind: "user", did }
              | { kind: "tag", forum, slug } | { kind: "sequence", uri }
PublicActionKind = vote|react|tagVote|join|modAction
KarmaCadence  = realtime|daily|weekly
EngineError   { code, message, retryable: bool, detail?: object }      // §1
Event         = { kind: "changed", topics: Vec<Topic> }
              | { kind: "syncProgress", forum, done, total }
              | { kind: "write", id, state: WriteState }
              | { kind: "auth", did, state: { kind: ok } | { kind: expiring, inMs } | { kind: expired } }
              | { kind: "warning", warning: equivocation|indexerContradiction|degraded|renderFailure|other,
                  source?, message }
Topic         = { kind: forum|thread, uri } | { kind: "user", did }
              | { kind: notifications|outbox|drafts|accounts }
```

## 5. Wasm binding (`@lessdugin/engine`)

- **Boundary.** `lessdugin-wasm` exposes one class, `WasmEngine`, built with wasm-bindgen
  (`--target web`, no tsify, no serde-wasm-bindgen): `new WasmEngine(configJson, host)`,
  `call(op, argsJson) → Promise<string>`, `subscribe(cb) → id`, `unsubscribe(id)`. `op` is an
  `EngineApi` method name (`packages/engine/src/api.ts`), `argsJson` its arguments as a JSON
  array, and the result is the DTO's dto/1 JSON text exactly as `dto.rs` serialises it (the
  bytes the CLI's `--json` prints); a failure rejects with an `EngineError` JSON string. Events
  reach `cb` as dto/1 JSON strings, in order, never re-entrantly. The TS types are the
  hand-written `types.ts`, checked by `types.parity.test.ts` against `testdata/dto/dto-1.d.ts`,
  which the Rust types generate through their tsify derives (§1 "Freeze"; the derives are a
  test-only feature, `lessdugin-wasm/dto-decl`, never in the module).
- **Coverage.** Every `EngineApi` method is either dispatched (`crates/wasm/src/ops.rs` `OPS`)
  or rejects with `unsupported` and `detail.milestone` (`UNSUPPORTED`, §3.9); a native test
  parses `api.ts` and fails when a method is in neither list, and `crates/wasm/tests/ops_social.rs`
  runs the M3 operations through the dispatcher with the JSON the client sends. Method names
  are the Rust names camelCased, except `setSubscription` (§3.6 `subscribe`). Worker glue
  operations outside `EngineApi`: `externalChange`, `outboxWake`, `outboxFlush`, `liveStep`.
  `grant`/`revoke` resolve with one ticket per actor in wasm; the client returns the first.
  `approve`/`reject` resolve with every ticket. The wasm side reads the arguments as a
  fixed-size tuple, so the client sends an omitted trailing optional argument as `null`
  (`client.ts` `OP_ARITY`, checked against `api.ts` by `client.test.ts`).
- **Configuration.** `configJson` is `EngineConfigInput` (`packages/engine/src/protocol.ts`,
  camelCase, unknown keys refused): `network` (`production` or `devnet { plc, pds }`), the
  sources, `verify`, the resolver lists and `parityNowMs` (§2.1). The worker accepts `devnet`
  only in development and e2e builds and `parityNowMs` only in e2e builds (`VITE_E2E=1`).
- **Host.** The worker implements the core `Host` in JS (`host.ts`) and passes it to the
  constructor: `Target::Anon` requests through `fetch` with the browser URL policy, streaming
  byte caps and `cache: "no-store"` (`fetch.ts`); `Target::UserPds` requests (writes and
  `getSession` only; the core reads public records, the account's own included, as
  `Target::Anon`) to the main thread's AuthBridge; `fetch_blob` streamed and hashed in JS with
  the CID check, stored in IndexedDB, never copied into wasm (`blobs.ts`); `lock` as the Web
  Lock `lessdugin-<name>` (`locks.ts`), where the leader's own `sync-leader` request is answered
  at once (it holds that Web Lock, which is not re-entrant), and in `lessdugin web` mode an
  `outbox:<did>` lock is the Web Lock and then the native lock through `POST /bridge/lock`
  (`bridge-lock.ts`, threat-model §7.4); `sleep`, `now_ms`, `fill_random` from the platform;
  WebSockets are `unsupported` until M5. The two `KvStore`s are IndexedDB databases
  (`<prefix>-cache` relaxed, `<prefix>-state` strict; `idb.ts`), or in `lessdugin web` mode
  the state store is the native one behind `POST /bridge/kv` (`bridge-kv.ts`). The IndexedDB
  store passes the core KV conformance suites in CI (`kv-conformance.test.ts`, with the test
  module of `cargo xtask wasm --kv-test`).
- **Threads.** The engine runs in a Web Worker (`worker.ts`; the main thread calls it through
  Comlink); the main thread holds the session and serves `Target::UserPds` requests over a
  dedicated `MessagePort` with a fixed message schema (`bridge-port.ts`: requests are rebuilt
  field by field, no main-thread object is exposed to the worker) through the AuthBridge
  allowlist
  (`auth-bridge.ts`, threat-model AUTH-4, AUTH-8), with a pluggable session: hosted OAuth
  (`oauthSession(did, fetchHandler)`) or the `lessdugin web` bridge
  (`bridgeSession({ did, token })`). `@lessdugin/engine/client` (`startEngine`) wraps the
  worker as an `EngineApi` and re-exports the DTO types.
- **Tabs.** One worker per tab; the Web Lock `lessdugin-sync-leader` elects the leader, the
  only worker that runs `outbox_pump()` and `live_tail()` (the client's `outboxPump()` and
  `liveTail()` resolve at once). Workers relay their `Changed` and `Write` events on the
  BroadcastChannel `lessdugin-changes`; a receiving worker calls `external_change(topics)`
  (§3.3) and forwards the event to its subscribers, and the leader wakes its pump on another
  tab's `Write { pending }` (`leader.ts`, `sync.md` §10.6). The React app maps `Changed`
  topics to TanStack Query invalidations.
- **Size.** `cargo xtask wasm [--release|--dev] [--budget] [--report]` builds
  `packages/engine/pkg` with wasm-opt (ADR 0002); `--budget` fails above 2.5 MB gzipped, and a
  release build warns above 1.5 MB gzipped or 4 MB raw (ADR 0008). Native-only dependencies (ammonia/html5ever) are refused
  in the wasm build, and `idna` uses the ASCII-only adapter.
