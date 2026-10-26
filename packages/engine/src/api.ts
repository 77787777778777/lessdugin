// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The engine's public operations (docs/spec/engine-api.md §3: engine API v0, frozen at M3; only
// compatible changes from here on), one async method per operation, camelCased:
// `lessdugin-wasm` dispatches each name (`WasmEngine.call(op, argsJson)`, crates/wasm/src/ops.rs,
// whose native test checks this list). Every method resolves with a DTO from `./types.ts` or
// rejects with an `EngineError`. The operations of later milestones reject with code
// `unsupported` and `detail.milestone` (engine-api.md §3.9): `forumDiscover`, `tags`, `tag`,
// `sequences`, `sequence`, `user`, `search`, `wikiEdit`, the `seq*` writes (M5);
// `redactRevision`, `prepareMedia`, the engine's drafts, `settings`/`setSetting`,
// `exportLocal`/`importLocal`, `validateRecord`, `deriveRkey` (M4); `runVector` (tests).
//
// Two names differ from a literal camelCasing of §3, because JavaScript has no overloading:
// - `subscribe(cb)` is the event subscription of §3.8;
// - the local-state operation `subscribe(Subscription, on)` of §3.6 is `setSubscription`.

import type {
  AccountInfo,
  AccountView,
  ActionTicket,
  ActorView,
  AtUri,
  BodyDraft,
  BookmarkEntry,
  CommentDraft,
  CommentSort,
  Did,
  DiscussionList,
  Draft,
  DraftEntry,
  EditOpts,
  EventListener,
  ForumDraft,
  ForumSummary,
  ForumView,
  HistoryView,
  Href,
  HrefContext,
  Json,
  KarmaCadence,
  ListPostsQuery,
  MediaInput,
  MediaRef,
  MembersQuery,
  MemberView,
  ModActionIntent,
  ModLog,
  ModQueue,
  ModsChange,
  NotificationList,
  NotificationsQuery,
  OpinionIntent,
  PostDraft,
  PostList,
  PostView,
  PublicActionKind,
  RawView,
  Ref,
  RefContext,
  RenderedBody,
  RenderOpts,
  RkeyInput,
  SearchQuery,
  SearchResults,
  SequenceDraft,
  SequencePosition,
  SequenceSummary,
  SequenceView,
  SettingKey,
  Settings,
  Slug,
  Subscription,
  SubscriptionEntry,
  SyncOpts,
  SyncReport,
  TagSummary,
  TagView,
  ThreadOpts,
  ThreadView,
  UnreadCounts,
  Unsubscribe,
  UserView,
  ValidationReport,
  VerifyReport,
  WikiDraft,
  WriteTicket,
} from "./types.ts";

export interface EngineApi {
  // §3.1 Identity, accounts, refs ------------------------------------------------------------
  resolveActor(input: string): Promise<ActorView>;
  accounts(): Promise<AccountView[]>;
  /** Called by host-specific login code after a successful login. */
  addAccount(info: AccountInfo): Promise<AccountView>;
  removeAccount(did: Did, purge: boolean): Promise<void>;
  setActiveAccount(did: Did): Promise<void>;
  activeAccount(): Promise<AccountView | null>;
  /** at-uris, SPA URLs (`#/p/…`, `#/c/…`, `#/f/…`, `#/s/…`, `#/u/…`, `#/at/…`), `handle/rkey`,
   * a did or a handle. Also one of the pure functions of §3.7. */
  parseRef(input: string, ctx: RefContext): Promise<Ref>;

  // §3.2 Forums and membership ----------------------------------------------------------------
  /** `ref` is anything `parseRef` accepts that names a forum. */
  forumOpen(ref: string): Promise<ForumView>;
  forumDiscover(q: string): Promise<ForumSummary[]>;
  forumFollow(uri: AtUri, alias?: string): Promise<void>;
  forumUnfollow(uri: AtUri): Promise<void>;
  followed(): Promise<ForumSummary[]>;
  forumCreate(draft: ForumDraft): Promise<WriteTicket>;
  /** By the owner, through the edit protocol. The slug is fixed; an absent or empty
   * `moderators` keeps them (`modsSet` changes them). */
  forumUpdate(uri: AtUri, draft: ForumDraft): Promise<WriteTicket>;
  /** Membership create: applies in approval mode, joins in open mode (a public action:
   * `acknowledgePublicAction("join")` first). */
  join(uri: AtUri, note?: string): Promise<WriteTicket>;
  leave(uri: AtUri): Promise<WriteTicket>;
  /** Every DID with a valid membership record, by DID; `pending: true` keeps the applicants
   * not rejected on this device. */
  members(uri: AtUri, query: MembersQuery): Promise<MemberView[]>;
  /** One write per actor (a DID or handle); resolves with the first actor's ticket (the others
   * report through `write` events and `writes()`). */
  grant(uri: AtUri, dids: Did[]): Promise<WriteTicket>;
  /** Like `grant`: one write per actor, the first actor's ticket. */
  revoke(uri: AtUri, dids: Did[]): Promise<WriteTicket>;
  /** Approval mode, by an authority: grants each actor (a DID or handle) membership and drops
   * it from this device's rejected list; one ticket per actor. */
  approve(uri: AtUri, actors: string[]): Promise<WriteTicket[]>;
  /** Dismisses applicants on this device (nothing public is written for them) and removes the
   * active account's own grant of an actor, if it has one: the removals' tickets. */
  reject(uri: AtUri, actors: string[]): Promise<WriteTicket[]>;
  /** The applicants rejected on this device (`members` with `pending` leaves them out). */
  rejectedApplicants(uri: AtUri): Promise<Did[]>;
  /** By the owner. `remove` adopts the moderator's assertions first (notice `adoptedFirst`)
   * unless `skipAdopt`. */
  modsSet(uri: AtUri, change: ModsChange): Promise<ActionTicket>;

  // §3.3 Sync ---------------------------------------------------------------------------------
  syncForum(uri: AtUri, opts: SyncOpts): Promise<SyncReport>;
  syncThread(postUri: AtUri, opts: SyncOpts): Promise<SyncReport>;
  /** Long-lived; resolves after `liveStop()`. In the browser the worker runs it in the leader
   * tab only. */
  liveTail(): Promise<void>;
  /** Stops every running `liveTail` after its current round; a later `liveTail` runs. */
  liveStop(): Promise<void>;
  /** Verifies the handles of up to `n` shown authors queued by views (`liveTail` does this each
   * round); resolves with how many were checked. */
  verifyHandles(n: number): Promise<number>;
  /** Long-lived; resolves after `outboxStop()`. In the browser the worker runs it in the leader
   * tab only. */
  outboxPump(): Promise<void>;
  /** Stops every running `outboxPump` after its current round; a later `outboxPump` runs. */
  outboxStop(): Promise<void>;
  /** "Send now": ends the back-off of every write waiting after an ambiguous or transient
   * failure; resolves with how many. */
  outboxExpedite(): Promise<number>;
  /** Long-lived; returns at once in v1 (`verify = proof` is Reserved). */
  backgroundVerify(): Promise<void>;

  // §3.4 Queries ------------------------------------------------------------------------------
  listPosts(query: ListPostsQuery): Promise<PostList>;
  recentDiscussion(
    forum: AtUri,
    cursor: string | undefined,
    limit: number,
  ): Promise<DiscussionList>;
  getPost(uri: AtUri): Promise<PostView>;
  getThread(uri: AtUri, opts: ThreadOpts): Promise<ThreadView>;
  history(uri: AtUri): Promise<HistoryView>;
  tags(forum: AtUri): Promise<TagSummary[]>;
  tag(forum: AtUri, slug: Slug): Promise<TagView>;
  sequences(forum: AtUri): Promise<SequenceSummary[]>;
  sequence(uri: AtUri): Promise<SequenceView>;
  user(forum: AtUri | undefined, actor: string): Promise<UserView>;
  search(query: SearchQuery): Promise<SearchResults>;
  /** The active account's notifications in its followed forums, newest first (unread only
   * unless `all`); `unread` counts every unread one. Re-query on `changed` topics
   * `notifications`, `forum` and `thread`. */
  notifications(query: NotificationsQuery): Promise<NotificationList>;
  /** The notification badge and every followed forum's unread posts, comments and
   * notifications. */
  unreadCounts(): Promise<UnreadCounts>;
  /** The forum's authorities only (`blocked` for anyone else). */
  modQueue(forum: AtUri): Promise<ModQueue>;
  /** Public; paged like `listPosts` (`limit` 0 or absent: the default page). */
  modLog(forum: AtUri, cursor?: string, limit?: number): Promise<ModLog>;
  raw(uri: AtUri): Promise<RawView>;
  drafts(): Promise<DraftEntry[]>;
  /** Newest first. */
  bookmarks(): Promise<BookmarkEntry[]>;
  bookmarked(uri: AtUri): Promise<boolean>;
  /** Oldest first. */
  subscriptions(): Promise<SubscriptionEntry[]>;
  /** The muted actors (the lens collapses their listed items), by DID. */
  mutes(): Promise<ActorView[]>;

  // §3.5 Writes -------------------------------------------------------------------------------
  publish(draft: PostDraft): Promise<WriteTicket>;
  comment(draft: CommentDraft): Promise<WriteTicket>;
  edit(uri: AtUri, draft: Draft, opts: EditOpts): Promise<WriteTicket>;
  delete(uri: AtUri): Promise<WriteTicket>;
  retract(uri: AtUri): Promise<WriteTicket>;
  /** A public action (`acknowledgePublicAction(vote | react | tagVote)` first). Held for
   * `OPINION_UNDO_WINDOW_MS` before it is sent and merged with the other intents on the same
   * subject; `undo` cancels it meanwhile, or writes the inverse once sent. Show the ticket's
   * `notices` (`notEstablished`: "recorded; will count once you're established"). */
  opinion(subjectUri: AtUri, intent: OpinionIntent): Promise<ActionTicket>;
  /** A public action (`acknowledgePublicAction("modAction")` first): asserts or removes the
   * active account's modAction for (scope, subject, action key). A lock refused above 5,000
   * comments rejects with `detail.hint: "useHide"`. */
  modAction(intent: ModActionIntent): Promise<ActionTicket>;
  /** Re-issues the former moderator's (a DID or handle) non-lapsed assertions. */
  modAdopt(forum: AtUri, formerMod: string): Promise<WriteTicket>;
  wikiEdit(forum: AtUri, slug: Slug, draft: WikiDraft): Promise<WriteTicket>;
  seqCreate(draft: SequenceDraft): Promise<WriteTicket>;
  seqEdit(uri: AtUri, draft: SequenceDraft): Promise<WriteTicket>;
  seqAdd(uri: AtUri, post: AtUri, at: SequencePosition): Promise<WriteTicket>;
  seqRemove(uri: AtUri, post: AtUri): Promise<WriteTicket>;
  seqMove(uri: AtUri, from: SequencePosition, to: SequencePosition): Promise<WriteTicket>;
  redactRevision(uri: AtUri): Promise<WriteTicket>;
  /**
   * `prepare_media` (engine-api.md §3.6): sniff, strip metadata, apply EXIF orientation and
   * compute the CID. The bytes are a separate argument because a Blob or a file is not a DTO
   * field; the upload itself happens at publish time (data-model.md §8.8).
   */
  prepareMedia(bytes: Uint8Array, input?: MediaInput): Promise<MediaRef>;
  undo(writeId: string): Promise<WriteTicket>;
  /** Queues a `failed` (or conflicting) write again. */
  retryWrite(writeId: string): Promise<WriteTicket>;
  /** `null`: unknown or pruned. */
  writeStatus(writeId: string): Promise<WriteTicket | null>;
  /** Every write in the outbox, by account then sequence (pending badges, an outbox view). */
  writes(): Promise<WriteTicket[]>;

  // §3.6 Local state --------------------------------------------------------------------------
  /** A post (its thread was viewed: its comments and notifications are read) or a forum (its
   * post list was visited). */
  markRead(uri: AtUri): Promise<void>;
  markNotificationsRead(ids: string[]): Promise<void>;
  markAllNotificationsRead(): Promise<void>;
  /** Returns the draft id (pass it back in `Draft.id` to update the same draft). */
  saveDraft(draft: Draft): Promise<string>;
  deleteDraft(id: string): Promise<void>;
  bookmark(uri: AtUri, on: boolean): Promise<void>;
  /** §3.6 `subscribe(Subscription, on)`. */
  setSubscription(subscription: Subscription, on: boolean): Promise<void>;
  /** `actor`: a DID or handle. */
  mute(actor: string, on: boolean): Promise<void>;
  /** The "this is public" interstitials the active account has seen (social.md §17); empty
   * without an account. */
  acknowledgedPublicActions(): Promise<PublicActionKind[]>;
  publicActionAcknowledged(kind: PublicActionKind): Promise<boolean>;
  /** Records that the active account saw the interstitial of `kind` (`auth_required` without
   * an account). */
  acknowledgePublicAction(kind: PublicActionKind): Promise<void>;
  /** How often karma changes are notified (default `daily`). */
  karmaNotifications(): Promise<KarmaCadence>;
  setKarmaNotifications(cadence: KarmaCadence): Promise<void>;
  settings(): Promise<Settings>;
  setSetting<K extends SettingKey>(key: K, value: Settings[K]): Promise<void>;
  exportLocal(): Promise<Json>;
  importLocal(json: Json): Promise<void>;

  // §3.7 Pure functions -----------------------------------------------------------------------
  render(markdown: string, media: MediaRef[], opts: RenderOpts): Promise<RenderedBody>;
  safeHref(raw: string, ctx: HrefContext): Promise<Href | null>;
  validateRecord(collection: string, json: Json): Promise<ValidationReport>;
  deriveRkey(input: RkeyInput): Promise<string>;
  verify(uri: AtUri, deep: boolean): Promise<VerifyReport>;
  /** Tests only (feature `vectors`). */
  runVector(kind: string, json: Json): Promise<Json>;

  // §3.8 Events -------------------------------------------------------------------------------
  /** Events are delivered in order and never re-entrantly. */
  subscribe(listener: EventListener): Unsubscribe;
}

/** Convenience: the body of a markdown-only draft. */
export function bodyDraft(markdown: string): BodyDraft {
  return { markdown, media: [] };
}

/** The comment sort used when none is chosen. */
export const DEFAULT_COMMENT_SORT: CommentSort = "magic";
