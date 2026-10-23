// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The engine's public operations (docs/spec/engine-api.md §3), one async method per operation,
// camelCased: `lessdugin-wasm` dispatches each name (`WasmEngine.call(op, argsJson)`,
// crates/wasm/src/ops.rs, whose native test checks this list). Every method resolves with a DTO
// from `./types.ts` or rejects with an `EngineError`; operations the engine does not implement
// yet reject with code `unsupported`.
//
// Two names differ from a literal camelCasing of §3, because JavaScript has no overloading:
// - `subscribe(cb)` is the event subscription of §3.8;
// - the local-state operation `subscribe(Subscription, on)` of §3.6 is `setSubscription`.

import type {
  AccountInfo,
  AccountView,
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
  forumUpdate(uri: AtUri, draft: ForumDraft): Promise<WriteTicket>;
  join(uri: AtUri, note?: string): Promise<WriteTicket>;
  leave(uri: AtUri): Promise<WriteTicket>;
  members(uri: AtUri, query: MembersQuery): Promise<MemberView[]>;
  /** One write per actor; resolves with the first actor's ticket (the others report through
   * `write` events and `writes()`). */
  grant(uri: AtUri, dids: Did[]): Promise<WriteTicket>;
  /** Like `grant`: one write per actor, the first actor's ticket. */
  revoke(uri: AtUri, dids: Did[]): Promise<WriteTicket>;
  modsSet(uri: AtUri, change: ModsChange): Promise<WriteTicket>;

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
  notifications(query: NotificationsQuery): Promise<NotificationList>;
  modQueue(forum: AtUri): Promise<ModQueue>;
  modLog(forum: AtUri, cursor?: string): Promise<ModLog>;
  raw(uri: AtUri): Promise<RawView>;
  drafts(): Promise<DraftEntry[]>;
  bookmarks(): Promise<BookmarkEntry[]>;
  subscriptions(): Promise<SubscriptionEntry[]>;

  // §3.5 Writes -------------------------------------------------------------------------------
  publish(draft: PostDraft): Promise<WriteTicket>;
  comment(draft: CommentDraft): Promise<WriteTicket>;
  edit(uri: AtUri, draft: Draft, opts: EditOpts): Promise<WriteTicket>;
  delete(uri: AtUri): Promise<WriteTicket>;
  retract(uri: AtUri): Promise<WriteTicket>;
  /** Held for `OPINION_UNDO_WINDOW_MS` before it is sent; `undo` cancels it meanwhile. */
  opinion(subjectUri: AtUri, intent: OpinionIntent): Promise<WriteTicket>;
  modAction(intent: ModActionIntent): Promise<WriteTicket>;
  modAdopt(forum: AtUri, formerMod: Did): Promise<WriteTicket>;
  wikiEdit(forum: AtUri, slug: Slug, draft: WikiDraft): Promise<WriteTicket>;
  seqCreate(draft: SequenceDraft): Promise<WriteTicket>;
  seqEdit(uri: AtUri, draft: SequenceDraft): Promise<WriteTicket>;
  seqAdd(uri: AtUri, post: AtUri, at: SequencePosition): Promise<WriteTicket>;
  seqRemove(uri: AtUri, post: AtUri): Promise<WriteTicket>;
  seqMove(uri: AtUri, from: SequencePosition, to: SequencePosition): Promise<WriteTicket>;
  redactRevision(uri: AtUri): Promise<WriteTicket>;
  prepareMedia(input: MediaInput): Promise<MediaRef>;
  undo(writeId: string): Promise<WriteTicket>;
  /** Queues a `failed` (or conflicting) write again. */
  retryWrite(writeId: string): Promise<WriteTicket>;
  /** `null`: unknown or pruned. */
  writeStatus(writeId: string): Promise<WriteTicket | null>;
  /** Every write in the outbox, by account then sequence (pending badges, an outbox view). */
  writes(): Promise<WriteTicket[]>;

  // §3.6 Local state --------------------------------------------------------------------------
  markRead(postUri: AtUri): Promise<void>;
  /** Returns the draft id (pass it back in `Draft.id` to update the same draft). */
  saveDraft(draft: Draft): Promise<string>;
  deleteDraft(id: string): Promise<void>;
  bookmark(uri: AtUri, on: boolean): Promise<void>;
  /** §3.6 `subscribe(Subscription, on)`. */
  setSubscription(subscription: Subscription, on: boolean): Promise<void>;
  mute(did: Did, on: boolean): Promise<void>;
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
