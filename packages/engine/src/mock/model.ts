// SPDX-License-Identifier: AGPL-3.0-or-later
// The mock engine's in-memory state. It stores roughly what the fold would produce (states,
// base scores, tallies) plus the viewer's own opinions, and derives DTOs from it on demand.

import type {
  ActorView,
  Anchor,
  AtUri,
  AuthKind,
  Badges,
  Banner,
  BookmarkEntry,
  Did,
  DraftEntry,
  ExternalLinkView,
  ForumRole,
  ForumSummary,
  ItemState,
  KarmaView,
  MembershipMode,
  ModeratorView,
  ModLogEntry,
  ModQueueItem,
  MyOpinion,
  NotificationReason,
  PostKind,
  RenderedBody,
  Settings,
  Slug,
  Stance,
  SubscriptionEntry,
  TextQuote,
  Tier,
  VersionEntry,
  VersionRef,
} from "../types.ts";

export interface ActorRec {
  view: ActorView;
  pds: string;
  bio?: string;
  links: { uri: string; label?: string }[];
  karma: KarmaView;
  status: ForumRole;
  joinedAtMs: number;
  granted: boolean;
  membershipNote?: string;
  pinned: AtUri[];
}

export interface BaseScore {
  score: number;
  agreement: number;
  unweighted: number;
  earlier: number;
}

export interface BaseReact {
  react: string;
  for: number;
  against: number;
  quote?: TextQuote;
  anchor?: Anchor;
  creator?: Did;
}

export interface PostRec {
  uri: AtUri;
  cid: string;
  forum: AtUri;
  author: Did;
  kind: PostKind;
  title?: string;
  markdown: string;
  body: RenderedBody;
  tags: Slug[];
  tier: Tier;
  pinned: boolean;
  locked: boolean;
  createdAtMs: number;
  tRankMs: number;
  base: BaseScore;
  reacts: BaseReact[];
  /** The fold state before the lens and the thread pass. */
  state: ItemState;
  badges: Badges;
  link?: ExternalLinkView;
  labels: string[];
  /** Earlier versions, oldest first (the current version is not included). */
  revisions: VersionEntry[];
  redacted: boolean;
}

export interface CommentRec {
  uri: AtUri;
  cid: string;
  forum: AtUri;
  root: AtUri;
  /** The record's parent pointer (may name a missing comment). */
  parent?: AtUri;
  author: Did;
  markdown: string;
  body: RenderedBody;
  quote?: TextQuote;
  quoteAnchor?: Anchor;
  createdAtMs: number;
  tRankMs: number;
  base: BaseScore;
  reacts: BaseReact[];
  state: ItemState;
  badges: Badges;
  retracted: boolean;
  moderatorHat: boolean;
  version?: VersionRef;
  /** Re-attached to the root by the cycle rule (social.md §11 step 3). */
  orphaned: boolean;
  pinned: boolean;
}

export interface MyReact {
  react: string;
  stance: Stance;
  quote?: TextQuote;
}

export interface MyOpinionRec extends MyOpinion {
  reacts: MyReact[];
}

export interface TagRec {
  slug: Slug;
  name: string;
  description?: string;
  page?: string;
  head?: AtUri;
  unmergedEdits: number;
}

export interface SequenceRec {
  uri: AtUri;
  author: Did;
  title: string;
  body?: string;
  chapters: { title?: string; body?: string; posts: AtUri[] }[];
  base: BaseScore;
  pinned: boolean;
  createdAtMs: number;
}

export interface NotificationRec {
  id: string;
  reason: NotificationReason;
  atMs: number;
  subject: AtUri;
  actor?: Did;
  read: boolean;
  preview?: string;
}

export interface ForumRec {
  summary: Omit<ForumSummary, "followed" | "member" | "owner"> & { owner: Did };
  about?: string;
  moderators: (Omit<ModeratorView, "actor"> & { did: Did })[];
  policy: PolicyRec;
  banners: Banner[];
  featuredTags: Slug[];
  defaultTier: "personal" | "frontpage";
  membership: MembershipMode;
}

/** The forum's policy with every default filled in (data-model.md §6.1 `#policy`). */
export interface PolicyRec {
  moderation: string;
  karma: { algorithm: string; pairCap: number; establish: number; establishSeeds: number };
  ranking: {
    algorithm: string;
    offsetHours: number;
    exponentMilli: number;
    curatedBonus: number;
    frontpageBonus: number;
    cutoffDays: number;
  };
  voting: "karmaOnly" | "twoAxis" | "reacts";
  reactPalette: string;
  gates: Record<string, number>;
  display: { collapseBelow: number; newUsers: "show" | "collapse" | "hide" };
  authorModeration: boolean;
  tagCreation: "open" | "moderators";
}

export interface AccountRec {
  did: Did;
  pds: string;
  auth: AuthKind;
}

export interface MockState {
  actors: Map<Did, ActorRec>;
  forums: Map<AtUri, ForumRec>;
  posts: Map<AtUri, PostRec>;
  comments: Map<AtUri, CommentRec>;
  tags: Map<AtUri, Map<Slug, TagRec>>;
  sequences: Map<AtUri, SequenceRec>;
  /** opinions[account][subject] */
  opinions: Map<Did, Map<AtUri, MyOpinionRec>>;
  notifications: Map<Did, NotificationRec[]>;
  modLog: Map<AtUri, ModLogEntryRec[]>;
  modQueue: Map<AtUri, ModQueueItem[]>;
  accounts: AccountRec[];
  active: Did | null;
  // local state
  followed: Map<AtUri, string | undefined>;
  lastVisited: Map<AtUri, number>;
  drafts: Map<string, DraftEntry>;
  bookmarks: Map<AtUri, BookmarkEntry>;
  subscriptions: SubscriptionEntry[];
  mutes: Set<Did>;
  settings: Settings;
  nextId: number;
}

export type ModLogEntryRec = Omit<ModLogEntry, "actor"> & { actor: Did };
