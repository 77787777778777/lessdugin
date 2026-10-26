// SPDX-License-Identifier: AGPL-3.0-or-later
//
// TypeScript mirror of the engine's `dto/1` schema (docs/spec/engine-api.md §2–§4; engine API
// v0, frozen at M3: only compatible changes from here on).
//
// Every type with a Rust definition (`crates/core/src/dto.rs`, `EngineError`, `ErrorCode`,
// `AccountInfo`) mirrors it exactly: the same type names, field names (camelCase), optionality
// (`field?: T`, never `T | null`: a field the serialiser may omit, or the deserialiser may go
// without, as `#[serde(default)]` request fields), field types and variants (enums internally
// tagged with `"kind"`, variant names camelCased); integer milliseconds for every `…Ms` field,
// and `dto: 1` on every top-level view. `types.parity.test.ts` compares this file with
// `testdata/dto/dto-1.d.ts`, which the Rust types generate (crates/wasm/tests/dto_decl.rs), and
// fails on any drift. The few types without a Rust counterpart (aliases, `Href`, the inputs of
// operations of later milestones) are listed there with the reason.

// ---------------------------------------------------------------------------------------------
// Scalars
// ---------------------------------------------------------------------------------------------

/** `did:plc:…` or `did:web:…` (data-model.md §2.3). */
export type Did = string;
/** Canonical at-uri `at://DID/NSID/RKEY` (data-model.md §2.3). */
export type AtUri = string;
/** A CID string (base32, `b…`). */
export type Cid = string;
/** `[a-z0-9]+(-[a-z0-9]+)*`, at most 64 bytes (data-model.md §3.1). */
export type Slug = string;
/** Integer milliseconds since the Unix epoch. */
export type Ms = number;
/** Any JSON value (records in `raw` views, settings values, exports). */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** The schema version carried by every top-level response (`dto: 1`). */
export const DTO_VERSION = 1;
export type DtoVersion = number;

/** Opinion intents are held this long before they are sent, so they can be undone
 * (data-model.md §8.2 "Holding opinions"; engine-api.md §3.5 "4 s undo window"). */
export const OPINION_UNDO_WINDOW_MS = 4000;

// ---------------------------------------------------------------------------------------------
// Errors (engine-api.md §1)
// ---------------------------------------------------------------------------------------------

export type EngineErrorCode =
  | "auth_required"
  | "not_found"
  | "invalid_ref"
  | "invalid_input"
  | "conflict"
  | "rate_limited"
  | "network"
  | "blocked"
  | "too_large"
  | "partial"
  | "unsupported"
  | "internal";

export interface EngineError {
  code: EngineErrorCode;
  message: string;
  retryable: boolean;
  detail?: { [key: string]: Json };
}

// ---------------------------------------------------------------------------------------------
// View DTOs (engine-api.md §4). Mirrors `crates/core/src/dto.rs` one-to-one, type names included.
// ---------------------------------------------------------------------------------------------

// §4.1 Common types --------------------------------------------------------------------------

export type HandleState = "verified" | "unverified" | "invalid";

export interface ActorView {
  did: string;
  /** Present only if verified in both directions. */
  handle?: string;
  handleState: HandleState;
  /** The handle the DID document claims, present only while it is `unverified` (sync.md §3.1:
   * shown struck through next to `shortDid`). Untrusted text. */
  claimedHandle?: string;
  displayName?: string;
  avatar?: MediaView;
  shortDid: string;
}

export type CollapseReason = "newUser" | "lowScore" | "muted";

export type HiddenReason =
  | { kind: "notParticipant" }
  | { kind: "moderationUnavailable" }
  | { kind: "removed"; rank: number; actor?: string; spam: boolean; reason?: string }
  | { kind: "banned" }
  | { kind: "restricted" }
  | { kind: "authorModerated" }
  | { kind: "locked" }
  | { kind: "newUser" };

export type ItemState =
  | { kind: "visible" }
  | { kind: "collapsed"; reason: CollapseReason }
  | { kind: "hidden"; reason: HiddenReason }
  | { kind: "tombstone"; reason: HiddenReason };

/** Trust and status badges (fixed badge column); absent means false. */
export interface Badges {
  verified?: boolean;
  edited?: boolean;
  futureDated?: boolean;
  identityUnverified?: boolean;
  stale?: boolean;
  hint?: boolean;
  inconsistentTimestamps?: boolean;
  /** An own item with an outbox write that is not `done` (sync.md §1.6). */
  pending?: boolean;
}

export type KarmaValue = "up" | "down" | "strongUp" | "strongDown";
export type AgreementValue = "agree" | "disagree" | "strongAgree" | "strongDisagree";

/** The viewer's own opinion on an item, as stored in the cache (their public opinion record). */
export interface MyOpinion {
  karma?: KarmaValue;
  agreement?: AgreementValue;
  /** A write of this opinion is in the outbox and not done (held for the undo window, or not
   * sent yet): the values above are the stored ones. */
  pending?: boolean;
  /** Recorded but carrying no weight (social.md §5–§7): "recorded; will count once you're
   * established". */
  notYetCounted?: boolean;
}

export interface ScoreView {
  score: number;
  /** Absent when the forum's `policy.voting` is `karmaOnly` (social.md §7). */
  agreement?: number;
  /** "n votes not yet counted": counting karma votes by voters outside E. */
  unweighted: number;
  /** N of "N of M votes were cast on an earlier version". */
  earlierVersionVotes: number;
  /** M of "N of M": the weighted karma votes (omitted when 0). */
  weightedVotes?: number;
  mine?: MyOpinion;
}

export type Stance = "for" | "against";

/** `defs#textQuote` (data-model.md §4). */
export interface TextQuote {
  exact: string;
  prefix?: string;
  suffix?: string;
  start?: number;
}

/** Untagged: code-point offsets into P, or an orphan marker (lfm-1.md §11). */
export type Anchor = { start: number; end: number } | { orphan: boolean };

export interface ReactTally {
  react: string;
  label: string;
  glyph: string;
  for: number;
  against: number;
  quote?: TextQuote;
  anchor?: Anchor;
  creator?: string;
  /** The viewer's stance, as their opinion record states it. */
  mine?: Stance;
  /** The viewer's own react, which forms no valid react (no creator passes `reactCreate`, or
   * the viewer's opinion does not count). */
  uncounted?: boolean;
}

export interface TocEntry {
  level: number;
  id: string;
  text: string;
}

export interface AspectRatio {
  width: number;
  height: number;
}

export interface MediaView {
  index: number;
  cid: string;
  mime: string;
  alt?: string;
  aspectRatio?: AspectRatio;
  durationMs?: number;
  name?: string;
  author: string;
  pds: string;
  autoload: boolean;
  sizeBytes: number;
}

export interface RenderedBody {
  /** LFM/1 HTML (lfm-1.md §8): root `<div class="lfm" dir="auto">`, allowlist §8.2 only. */
  html: string;
  /** The plain-text projection P (lfm-1.md §10). */
  text: string;
  toc: TocEntry[];
  wordCount: number;
  readMinutes: number;
  media: MediaView[];
  excerpt: string;
  notices: string[];
  rendererVersion: string;
}

export interface Completeness {
  partial: boolean;
  reposComplete: number;
  reposTotal: number;
  degraded: boolean;
}

// §4.2 Posts and threads ----------------------------------------------------------------------

export type PostKind = "article" | "link" | "question" | "quickTake";
export type Tier = "personal" | "frontpage" | "curated";
/** social.md §12.2 */
export type PostSort = "magic" | "top" | "new" | "old" | "recentComments";
/** social.md §12.3 */
export type PostFilter = "frontpage" | "all" | "curated";
/** social.md §12.4 */
export type CommentSort = "magic" | "top" | "new" | "old" | "latestReply";

export const POST_KINDS: readonly PostKind[] = ["article", "link", "question", "quickTake"];
export const POST_SORTS: readonly PostSort[] = ["magic", "top", "new", "old", "recentComments"];
export const POST_FILTERS: readonly PostFilter[] = ["frontpage", "all", "curated"];
export const COMMENT_SORTS: readonly CommentSort[] = ["magic", "top", "new", "old", "latestReply"];

/** A link post's external link. Show `uri` as a link only through `safeHref` (SafeLink). */
export interface ExternalLinkView {
  uri: string;
  host: string;
  title?: string;
  description?: string;
}

export interface PostSummary {
  uri: string;
  cid: string;
  forum: string;
  author: ActorView;
  kind: PostKind;
  title?: string;
  excerpt: string;
  tags: string[];
  tier: Tier;
  pinned: boolean;
  locked: boolean;
  createdAtMs: number;
  tRankMs: number;
  commentCount: number;
  lastCommentAtMs?: number;
  score: ScoreView;
  state: ItemState;
  badges: Badges;
  unreadComments?: number;
  link?: ExternalLinkView;
  labels: string[];
}

export interface PostList {
  dto: DtoVersion;
  forum: string;
  sort: PostSort;
  filter: PostFilter;
  items: PostSummary[];
  cursor?: string;
  completeness: Completeness;
  /** `moderationUnavailable` (a degraded view, social.md §4.11), `newerRules`; omitted when
   * empty. */
  banners?: Banner[];
}

export interface HistorySummary {
  revisions: number;
  redacted: boolean;
}

export interface SequenceNav {
  sequence: string;
  title: string;
  prev?: string;
  next?: string;
}

export interface PostView {
  dto: DtoVersion;
  post: PostSummary;
  body: RenderedBody;
  reacts: ReactTally[];
  history: HistorySummary;
  sequences: SequenceNav[];
}

/** Untagged: `{ revision: n }` ("on revision n") or `{ unavailable }` ("on an unavailable
 * version"). */
export type VersionRef = { revision: number } | { unavailable: boolean };

export interface CommentNode {
  uri: string;
  cid: string;
  /** The thread's root post: the thread the engine placed the comment in (not the record's
   * own, author-controlled `root` field). */
  root: string;
  parent?: string;
  depth: number;
  author: ActorView;
  body: RenderedBody;
  quote?: TextQuote;
  quoteAnchor?: Anchor;
  createdAtMs: number;
  tRankMs: number;
  score: ScoreView;
  reacts: ReactTally[];
  state: ItemState;
  badges: Badges;
  retracted: boolean;
  moderatorHat: boolean;
  version?: VersionRef;
  orphaned: boolean;
  missing: boolean;
  unread: boolean;
  pinned: boolean;
}

/** Untagged: the post, or `{ missing: uri }` when the root is not available. */
export type ThreadRoot = PostView | { missing: string };

export interface ThreadView {
  dto: DtoVersion;
  post: ThreadRoot;
  /** Pre-order, with depth (the root's children have depth 1). */
  comments: CommentNode[];
  sort: CommentSort;
  completeness: Completeness;
  banners?: Banner[];
}

export interface DiscussionItem {
  post: PostSummary;
  comments: CommentNode[];
}

export interface DiscussionList {
  dto: DtoVersion;
  items: DiscussionItem[];
  cursor?: string;
  completeness: Completeness;
  banners?: Banner[];
}

export interface VersionEntry {
  n: number;
  cid: string;
  createdAtMs: number;
  summary?: string;
  available: boolean;
}

export interface HistoryView {
  dto: DtoVersion;
  uri: string;
  versions: VersionEntry[];
}

// §4.3 Forums, users, organisation ----------------------------------------------------------

export type MembershipMode = "open" | "approval";
export type ModRole = "admin" | "moderator";
/** The viewer's (or a member's) role or status in a forum. */
export type ForumRole =
  | "owner"
  | "admin"
  | "moderator"
  | "member"
  | "established"
  | "newcomer"
  | "none";

export interface ForumSummary {
  uri: string;
  owner: ActorView;
  name: string;
  description?: string;
  membership: MembershipMode;
  followed: boolean;
  member: boolean;
  alias?: string;
  /** The declaration's `reportContact` as written (`mailto:`, `https:` or `at://`): untrusted,
   * shown only through `safeHref` in `Field` context. */
  reportContact?: string;
}

export interface ModeratorView {
  actor: ActorView;
  role: ModRole;
  title?: string;
  /** Shown as a moderator only if they also hold a membership record. */
  shown: boolean;
}

export interface Banner {
  code: string;
  message: string;
}

/** The effective policy (social.md §1.6), as name/value pairs in the core's `PARAMS` order. */
export interface EffectivePolicy {
  moderation: string;
  karma: string;
  ranking: string;
  reactPalette: string;
  params: [string, string][];
}

export interface ForumView {
  dto: DtoVersion;
  forum: ForumSummary;
  about?: RenderedBody;
  moderators: ModeratorView[];
  policy: EffectivePolicy;
  myRole: ForumRole;
  myKarma?: number;
  banners: Banner[];
  completeness: Completeness;
  /** What the viewer may do (absent without an active account). */
  viewer?: ViewerCaps;
}

/** What the viewer (the active account) may do in a forum, from the fold (social.md §4.9, §5,
 * §8): clients enable or disable actions with it. Writers check gates as UX only; the fold
 * decides what counts. */
export interface ViewerCaps {
  /** Karma votes count (not banned or nullified; not restricted from `vote`). */
  vote: boolean;
  strongVote: boolean;
  /** Agreement votes count: `vote` and `policy.voting` ≠ `karmaOnly`. */
  agree: boolean;
  /** The viewer is an eligible voter (E); otherwise votes are "recorded; will count once
   * you're established". */
  weighted: boolean;
  /** Reacts count (`policy.voting = reacts`, not restricted, in E). */
  react: boolean;
  reactCreate: boolean;
  reactSecond: boolean;
  reactAnti: boolean;
  /** The viewer's posts are listed (V2–V5, V8). */
  post: boolean;
  comment: boolean;
  /** Open mode: the viewer's content is collapsed or hidden as a new user (V8). */
  newcomer: boolean;
  /** Approval mode: a membership record and no grant. */
  applicant: boolean;
  /** Forum-tier moderation (an authority). */
  moderate: boolean;
  /** 1 moderator, 2 admin, 3 owner. */
  rank?: number;
  /** `#tier{curated}`: an admin or the owner. */
  curate: boolean;
  authorModPersonal: boolean;
  authorModFrontpage: boolean;
}

export interface KarmaView {
  k1: number;
  kp: number;
  k: number;
}

/** A link on a profile. Show `uri` as a link only through `safeHref` (SafeLink). */
export interface LinkView {
  uri: string;
  host: string;
  label?: string;
}

export interface UserView {
  dto: DtoVersion;
  actor: ActorView;
  forum?: string;
  karma?: KarmaView;
  status: ForumRole;
  bio?: RenderedBody;
  links: LinkView[];
  pinned: PostSummary[];
  recentPosts: PostSummary[];
  recentComments: CommentNode[];
}

export interface MemberView {
  actor: ActorView;
  status: ForumRole;
  granted: boolean;
  membershipNote?: string;
  joinedAtMs: number;
  /** K (omitted when 0). */
  karma?: number;
  /** Approval mode: a membership without a grant. */
  applicant?: boolean;
  banned?: boolean;
  nullified?: boolean;
  /** Restricted capabilities (`vote`, `comment`, …). */
  restricted?: string[];
}

export interface TagSummary {
  slug: string;
  name: string;
  description?: string;
  postCount: number;
  unmergedEdits: number;
}

export interface TagView {
  dto: DtoVersion;
  tag: TagSummary;
  page?: RenderedBody;
  head?: string;
  posts: PostSummary[];
}

export interface SequenceSummary {
  uri: string;
  author: ActorView;
  title: string;
  chapters: number;
  posts: number;
  score: ScoreView;
  pinned: boolean;
}

export interface ChapterView {
  title?: string;
  body?: RenderedBody;
  posts: PostSummary[];
}

export interface SequenceView {
  dto: DtoVersion;
  sequence: SequenceSummary;
  body?: RenderedBody;
  chapters: ChapterView[];
}

/** Untagged `PostSummary | CommentNode` (see `isCommentNode`). */
export type SearchItem = PostSummary | CommentNode;

export interface SearchHit {
  item: SearchItem;
  /** Plain text. */
  snippet: string;
  /** Code-point ranges `[start, end)` into `snippet`. */
  highlights: [number, number][];
}

export interface SearchResults {
  dto: DtoVersion;
  hits: SearchHit[];
}

/** social.md §16 reasons, as `dto.rs` has them. */
export type NotificationReason =
  | "reply"
  | "mention"
  | "quote"
  | "pingback"
  | "newComment"
  | "newPost"
  | "reacted"
  | "modAction"
  | "karmaChange";

export interface NotificationView {
  id: string;
  reason: NotificationReason;
  /** When the item was first seen (never its `createdAt`), or when a change was noticed. */
  atMs: number;
  subject: string;
  /** The forum the notification belongs to. */
  forum?: AtUri;
  actor?: ActorView;
  read: boolean;
  /** Plain text. */
  preview?: string;
}

export interface NotificationList {
  dto: DtoVersion;
  items: NotificationView[];
  /** Every unread notification (the badge), not only those in `items`. */
  unread: number;
}

/** Unread state of one followed forum (social.md §16). */
export interface ForumUnread {
  forum: AtUri;
  posts: number;
  comments: number;
  notifications: number;
}

/** `unreadCounts()`: the notification badge and every followed forum's unread state. */
export interface UnreadCounts {
  dto: DtoVersion;
  notifications: number;
  forums: ForumUnread[];
}

export interface ModQueueItem {
  /** social.md §4.12: stale positive assertion, unordered conflict, applicant, newcomer item. */
  kind: string;
  /** A did, a canonical at-uri or `tag:slug`. */
  subject: string;
  reason: string;
  detail?: string;
  /** The modAction a `stale` item is about. */
  uri?: string;
  /** The applicant, or the newcomer item's author. */
  actor?: ActorView;
  /** When the applicant joined, or the newcomer item's `t_rank`. */
  atMs?: number;
}

export interface ModQueue {
  dto: DtoVersion;
  items: ModQueueItem[];
}

/** The status of a mod-log entry (social.md §4.12). */
export type ModLogStatus =
  | "effective"
  | "overridden"
  | "lapsed"
  | "expired"
  | "stale"
  | "unauthorised"
  | "reservedUnknown"
  | "edited"
  | "removed";

/** A mod-log entry: a modAction (any scope, any actor) or a revision that edited or removed one.
 * `reason` of a `#hide` with `showReason: false` is shown to the forum's authorities only. */
export interface ModLogEntry {
  /** The modAction or revision uri. */
  uri: string;
  actor: ActorView;
  action: string;
  subject: string;
  value: string;
  reason?: string;
  /** `updatedAt ?? createdAt` (0: a time before 1970). */
  atMs: number;
  status: ModLogStatus;
  lapsed: boolean;
  /** The winning rank of an `overridden` entry. */
  rank?: number;
  /** An author-scope assertion (social.md §4.9). */
  authorScope?: boolean;
  adoptedFrom?: string;
}

export interface ModLog {
  dto: DtoVersion;
  entries: ModLogEntry[];
  cursor?: string;
}

/** Content trust level (sync.md §1.2). */
export type TrustLevel = "pds" | "proven" | "hint";

export interface RawView {
  dto: DtoVersion;
  uri: string;
  cid?: string;
  /** For a comment: the root post of the thread the engine placed it in. */
  root?: string;
  level: TrustLevel;
  record?: Json;
  validity: string[];
  /** Every modAction naming the item, valid or not: `{uri, cid, valid, record, rule?,
   * status?}` (`status`: its mod-log status). */
  assertions: Json[];
  state?: ItemState;
  /** The fold's score (hidden items have scores too). */
  score?: ScoreView;
}

export type VerifyOutcome = "proven" | "absent" | "failed";

export interface VerifyCheck {
  name: string;
  ok: boolean;
  detail?: string;
}

export interface VerifyReport {
  dto: DtoVersion;
  uri: string;
  result: VerifyOutcome;
  rev?: string;
  commitCid?: string;
  keySource: string;
  checks: VerifyCheck[];
}

export interface SyncIssue {
  did?: string;
  host?: string;
  code: string;
  message: string;
}

export interface SyncReport {
  dto: DtoVersion;
  forum: string;
  reposTotal: number;
  reposComplete: number;
  reposStale: number;
  reposUnavailable: number;
  requests: number;
  bytes: number;
  changed: number;
  partial: boolean;
  degraded: boolean;
  errors: SyncIssue[];
}

// ---------------------------------------------------------------------------------------------
// URL policy (lfm-1.md §5.1): the result of the pure function `safeHref`
// ---------------------------------------------------------------------------------------------

export type HrefContext = "markdown" | "field";

/** `Href = External{url, host} | Mailto{url, host} | Record{uri} | Profile{did}
 * | ProfileHandle{handle} | Anchor{id}`, tagged with `kind` like every engine enum. */
export type Href =
  | { kind: "external"; url: string; host: string }
  | { kind: "mailto"; url: string; host: string }
  | { kind: "record"; uri: string }
  | { kind: "profile"; did: string }
  | { kind: "profileHandle"; handle: string }
  | { kind: "anchor"; id: string };

// ---------------------------------------------------------------------------------------------
// §3.1 Identity, accounts, refs
// ---------------------------------------------------------------------------------------------

/** How an account is authenticated (`dto.rs` `AuthKind`). */
export type AuthKind = "oauth" | "appPassword";

/** A logged-in account (`dto.rs` `AccountView`, engine-api.md §3.1 `accounts()`). */
export interface AccountView {
  did: string;
  /** The handle, if it was verified in both directions at login. */
  handle?: string;
  /** The PDS bound at login. */
  pds: string;
  authKind: AuthKind;
  active: boolean;
}

/** `add_account(AccountInfo { did, pds, auth: AuthKind, issuer?, intended?,
 * confirmIssuerChange })`: what the login code reports after obtaining a session. */
export interface AccountInfo {
  /** The session's DID (the token's `sub`). */
  did: Did;
  /** The PDS the session talks to. */
  pds: string;
  auth: AuthKind;
  /** The OAuth authorization server issuer (`iss`); required for OAuth. */
  issuer?: string;
  /** The DID the user asked to log in as (resolved from their input), if any. */
  intended?: Did;
  /** The user confirmed a changed authorization server (`sync.md` §3.6); default `false`. */
  confirmIssuerChange?: boolean;
}

export type AuthState = { kind: "ok" } | { kind: "expiring"; inMs: Ms } | { kind: "expired" };

/** The result of `parse_ref`. */
export type Ref =
  | { kind: "forum"; uri: AtUri }
  | { kind: "post"; uri: AtUri }
  | { kind: "comment"; uri: AtUri }
  | { kind: "sequence"; uri: AtUri }
  | { kind: "record"; uri: AtUri }
  | { kind: "actor"; did: Did };

export interface RefContext {
  /** The current forum, for `handle/rkey` post references. */
  forum?: AtUri;
}

// ---------------------------------------------------------------------------------------------
// §3.2 Forums and membership — request types (derived)
// ---------------------------------------------------------------------------------------------

export interface ModeratorDraft {
  did: Did;
  role: ModRole;
  title?: string;
}

/** `forum.defaultTier` values a declaration may write. */
export type DefaultTier = "personal" | "frontpage";

export interface ForumDraft {
  /** The forum rkey (`forum_create` only; fixed afterwards). */
  slug: Slug;
  name: string;
  description?: string;
  about?: BodyDraft;
  membership: MembershipMode;
  defaultTier: DefaultTier;
  /** Default none. */
  featuredTags?: Slug[];
  /** Default none (`forum_update`: keep them; change them with `modsSet`). */
  moderators?: ModeratorDraft[];
  reportContact?: string;
}

export interface MembersQuery {
  status?: ForumRole;
  pending?: boolean;
}

export type ModsChange =
  | { kind: "add"; did: Did; role: ModRole; title?: string }
  /** `skipAdopt` (default false): let the moderator's assertions lapse at once instead of
   * adopting them first; clients offer it only after a confirmation. */
  | { kind: "remove"; did: Did; skipAdopt?: boolean };

// ---------------------------------------------------------------------------------------------
// §3.3 Sync
// ---------------------------------------------------------------------------------------------

export interface SyncOpts {
  budgetMs?: number;
  maxRequests?: number;
  /** Default false. */
  full?: boolean;
}

// ---------------------------------------------------------------------------------------------
// §3.4 Queries — request types
// ---------------------------------------------------------------------------------------------

export interface ListPostsQuery {
  forum: AtUri;
  sort: PostSort;
  filter: PostFilter;
  tag?: Slug;
  kind?: PostKind;
  author?: Did;
  cursor?: string;
  limit: number;
}

export interface ThreadOpts {
  sort: CommentSort;
  /** Default false. */
  unreadOnly?: boolean;
}

export interface SearchQuery {
  q: string;
  forum?: AtUri;
  author?: Did;
  limit: number;
}

export interface NotificationsQuery {
  /** `false` (the default): unread only; `true`: everything. */
  all?: boolean;
  limit: number;
}

// ---------------------------------------------------------------------------------------------
// §3.5 Writes
// ---------------------------------------------------------------------------------------------

/** The result of `prepare_media` (derived): a local, not yet uploaded blob. */
export interface MediaRef {
  id: string;
  cid: Cid;
  mime: string;
  sizeBytes: number;
  alt?: string;
  aspectRatio?: AspectRatio;
  durationMs?: number;
  name?: string;
}

// What `prepare_media` takes: the display hints and the DECLARED type. The bytes are a
// separate argument (a `Uint8Array` or a file), because neither belongs in a DTO that is also
// exported to Rust (engine-api.md §3.6; the Rust type is `MediaInput`).
export interface MediaInput {
  /** The declared mimeType; the sniffed one wins when they disagree (data-model.md §8.8). */
  declaredMime?: string;
  alt?: string;
  name?: string;
  aspectRatio?: AspectRatio;
}

export interface BodyDraft {
  markdown: string;
  /** Default none. */
  media?: MediaRef[];
}

export interface ExternalLinkDraft {
  uri: string;
  title?: string;
  description?: string;
}

export interface PostDraft {
  forum: AtUri;
  kind: PostKind;
  title?: string;
  body: BodyDraft;
  link?: ExternalLinkDraft;
  /** Default none. */
  tags?: Slug[];
  /** Default none. */
  labels?: string[];
}

export interface CommentDraft {
  root: AtUri;
  parent?: AtUri;
  body: BodyDraft;
  quote?: TextQuote;
}

export interface SequenceChapterDraft {
  title?: string;
  body?: BodyDraft;
  posts: AtUri[];
}

export interface SequenceDraft {
  forum: AtUri;
  title: string;
  body?: BodyDraft;
  chapters: SequenceChapterDraft[];
}

export interface WikiDraft {
  name: string;
  description?: string;
  body?: BodyDraft;
  isTag: boolean;
  parentTags: Slug[];
  summary?: string;
}

/** A draft for `edit` and for the local draft store, tagged by what is drafted (sequence
 * drafts arrive with sequences, M5). */
export type Draft =
  | { kind: "post"; id?: string; post: PostDraft }
  | { kind: "comment"; id?: string; comment: CommentDraft };

export interface EditOpts {
  history: boolean;
  summary?: string;
}

/** `Karma(v|clear) | Agreement(v|clear) | React{…} | Tag{slug, v|clear}`; an absent `value`
 * means clear. */
export type TagVoteValue = "up" | "down" | "strongUp" | "strongDown";

export type OpinionIntent =
  | { kind: "karma"; value?: KarmaValue }
  | { kind: "agreement"; value?: AgreementValue }
  | { kind: "react"; id: string; stance: Stance; quote?: TextQuote; remove?: boolean }
  | { kind: "tag"; slug: Slug; value?: TagVoteValue };

/** Restrictable capabilities (data-model.md §6.7 `#restrict`). */
export type Capability = "post" | "comment" | "vote" | "react" | "tag" | "wiki" | "sequence";

export type AccessLevel = "banned" | "member";

export type PinScope = "forum" | "thread";

/** A modAction `action` as a client asks for it (data-model.md §6.7): the complete desired state
 * of its action key. Defaults: `spam` false, `showReason` true, `nullifyVotes` false. */
export type ModActionValue =
  | { kind: "hide"; hidden: boolean; spam?: boolean; showReason?: boolean }
  | { kind: "access"; level: AccessLevel }
  | { kind: "restrict"; capabilities: Capability[]; nullifyVotes?: boolean }
  | { kind: "tier"; tier: Tier; notice?: BodyDraft }
  | { kind: "pin"; scope: PinScope; pinned: boolean }
  | { kind: "lock"; locked: boolean }
  | { kind: "tag"; removed: boolean; head?: AtUri };

export type ModScope = "forum" | "author";

/** Derived from data-model.md §6.7, §8.4 and §8.6. `assert` creates or updates the actor's
 * record for (scope, subject, action key); `remove` deletes it through the removal protocol. */
export type ModActionIntent =
  | {
      kind: "assert";
      forum: AtUri;
      scope: ModScope;
      subject: string;
      action: ModActionValue;
      reason?: string;
      untilMs?: Ms;
    }
  | { kind: "remove"; forum: AtUri; scope: ModScope; subject: string; actionKey: string };

/** `Pending → Sending → Done { uri, cid } | Failed { error } | Undone` (engine-api.md §3.5).
 * Opinion writes stay `pending` for the undo window. */
export type WriteState =
  /** `held`: in the undo window; `throttled`: waiting for the write budget, a rate limit or a
   * back-off; `needsLogin`: the account's session was refused, the write waits for the next
   * login of the account; `notBeforeMs`: until when; `error`: the failure that caused a
   * `throttled` or `needsLogin` wait. */
  | {
      kind: "pending";
      notBeforeMs?: Ms;
      held?: boolean;
      throttled?: boolean;
      needsLogin?: boolean;
      error?: EngineError;
    }
  | { kind: "sending" }
  /** `cid` is absent for deletes. */
  | { kind: "done"; uri: AtUri; cid?: Cid }
  | { kind: "failed"; error: EngineError }
  | { kind: "undone" };

export interface WriteTicket {
  id: string;
  state: WriteState;
}

export type NoticeCode = "notEstablished" | "retroactive" | "adoptedFirst";

/** A notice to show with an action's result, e.g. "recorded; will count once you're
 * established". */
export interface ActionNotice {
  code: NoticeCode;
  message: string;
}

/** What `opinion`, `modAction` and `modsSet` resolve with: a `WriteTicket` plus notices. */
export interface ActionTicket {
  id: string;
  state: WriteState;
  notices?: ActionNotice[];
}

/** Arguments of `derive_rkey(...)` (derived from data-model.md §3.2). */
export type RkeyInput =
  | { kind: "membership"; forum: AtUri }
  | { kind: "post"; forum: AtUri; tid: string }
  | { kind: "comment"; root: AtUri; tid: string }
  | { kind: "opinion"; forum: AtUri; subject: AtUri }
  | { kind: "modAction"; forum: AtUri; scope: ModScope; subject: string; actionKey: string }
  | { kind: "revision"; subject: AtUri; tid: string };

export interface SequencePosition {
  chapter: number;
  index: number;
}

// ---------------------------------------------------------------------------------------------
// §3.6 Local state (derived shapes)
// ---------------------------------------------------------------------------------------------

export type Subscription =
  | { kind: "forum"; uri: AtUri }
  | { kind: "post"; uri: AtUri }
  | { kind: "user"; did: Did }
  | { kind: "tag"; forum: AtUri; slug: Slug }
  | { kind: "sequence"; uri: AtUri };

export interface SubscriptionEntry {
  subscription: Subscription;
  sinceMs: Ms;
}

/** A media blob fetched for display (`engine-api.md` §3.6 `loadMedia`). `url` is an object URL
 * in the browser; `cid` is already verified by the host while the bytes streamed
 * (threat-model: blob bytes never enter wasm linear memory). */
export interface LoadedMedia {
  cid: string;
  size: number;
  url: string;
}

export interface DraftEntry {
  id: string;
  savedAtMs: Ms;
  draft: Draft;
}

export interface BookmarkEntry {
  uri: AtUri;
  savedAtMs: Ms;
  post?: PostSummary;
}

/** A consented indexer endpoint (`EngineConfig.indexers`). */
export interface IndexerConsent {
  url: string;
  did?: Did;
  /** The forum that declared it, when it came from a declaration. */
  forum?: AtUri;
  grantedAtMs: Ms;
}

/** Local settings (synthesis §1.4, §3.9, §6.12). */
/** Where content comes from (synthesis-v1.md 1.4). */
export type Sources = "direct" | "assisted";

/** How much is verified (sync.md 2.3). */
export type VerifyMode = "pds" | "proof";

export interface Settings {
  sources: Sources;
  verify: VerifyMode;
  indexers: IndexerConsent[];
  /** Forums for which external images may be fetched (lfm-1.md §6.4; off by default). */
  externalImagesForums: AtUri[];
  /** Hosts whose media may load automatically. */
  mediaAutoloadHosts: string[];
  karmaNotifications: KarmaCadence;
  /** Public-action interstitials already acknowledged (social.md §17). */
  acknowledgedPublicActions: PublicActionKind[];
}

export type SettingKey = keyof Settings;

export type KarmaCadence = "realtime" | "daily" | "weekly";

export type PublicActionKind = "vote" | "react" | "tagVote" | "join" | "modAction";

// ---------------------------------------------------------------------------------------------
// §3.7 Pure functions
// ---------------------------------------------------------------------------------------------

export interface RenderOpts {
  html: boolean;
  text: boolean;
  styled: boolean;
}

export interface ValidationReport {
  valid: boolean;
  /** Failing rule IDs (data-model.md §7), first failure first. */
  errors: string[];
}

// ---------------------------------------------------------------------------------------------
// §3.8 Events
// ---------------------------------------------------------------------------------------------

export type Topic =
  | { kind: "forum"; uri: AtUri }
  | { kind: "thread"; uri: AtUri }
  | { kind: "user"; did: Did }
  | { kind: "notifications" }
  | { kind: "outbox" }
  | { kind: "drafts" }
  | { kind: "accounts" };

export type WarningKind =
  | "equivocation"
  | "indexerContradiction"
  | "degraded"
  | "renderFailure"
  | "other";

export type EngineEvent =
  | { kind: "changed"; topics: Topic[] }
  | { kind: "syncProgress"; forum: AtUri; done: number; total: number }
  | { kind: "write"; id: string; state: WriteState }
  | { kind: "auth"; did: Did; state: AuthState }
  /** `Warning { kind, source?, message }`: the warning's own kind travels as `warning`
   * because `kind` is the event tag. */
  | { kind: "warning"; warning: WarningKind; source?: string; message: string };

export type EventListener = (event: EngineEvent) => void;
export type Unsubscribe = () => void;

// ---------------------------------------------------------------------------------------------
// Helpers for untagged unions
// ---------------------------------------------------------------------------------------------

export function isCommentNode(item: SearchItem): item is CommentNode {
  return "depth" in item;
}

export function isMissingRoot(root: ThreadRoot): root is { missing: AtUri } {
  return "missing" in root;
}

/** `{ orphan: true }`. The Rust DTO's `orphan` is a bool, so `{ orphan: false }` can be
 * serialised too; it is not an orphan (and carries no range). */
export function isOrphanAnchor(anchor: Anchor): anchor is { orphan: true } {
  return "orphan" in anchor && anchor.orphan === true;
}

export function isEngineError(e: unknown): e is EngineError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as { code?: unknown }).code === "string" &&
    typeof (e as { message?: unknown }).message === "string" &&
    typeof (e as { retryable?: unknown }).retryable === "boolean"
  );
}
