// SPDX-License-Identifier: AGPL-3.0-or-later
// GENERATED from the Rust DTOs (crates/core/src/dto.rs, plus EngineError, ErrorCode and
// AccountInfo) by their tsify derives; do not edit. Regenerate with
//   LESSDUGIN_BLESS=1 cargo test -p lessdugin-wasm --features dto-decl --test dto_decl
// packages/engine/src/types.parity.test.ts compares packages/engine/src/types.ts with it.

export type Value = null | boolean | number | string | Value[] | { [key: string]: Value };

export type AccessLevel = "banned" | "member";

export interface AccountInfo {
    did: string;
    pds: string;
    auth: AuthKind;
    issuer?: string;
    intended?: string;
    confirmIssuerChange?: boolean;
}

export interface AccountView {
    did: string;
    handle?: string;
    pds: string;
    authKind: AuthKind;
    active: boolean;
}

export interface ActionNotice {
    code: NoticeCode;
    message: string;
}

export interface ActionTicket {
    id: string;
    state: WriteState;
    notices?: ActionNotice[];
}

export interface ActorView {
    did: string;
    handle?: string;
    handleState: HandleState;
    claimedHandle?: string;
    displayName?: string;
    avatar?: MediaView;
    shortDid: string;
}

export type AgreementValue = "agree" | "disagree" | "strongAgree" | "strongDisagree";

export type Anchor = { start: number; end: number } | { orphan: boolean };

export interface AspectRatio {
    width: number;
    height: number;
}

export type AuthKind = "oauth" | "appPassword";

export type AuthState = { kind: "ok" } | { kind: "expiring"; inMs: number } | { kind: "expired" };

export interface Badges {
    verified?: boolean;
    edited?: boolean;
    futureDated?: boolean;
    identityUnverified?: boolean;
    stale?: boolean;
    hint?: boolean;
    inconsistentTimestamps?: boolean;
    pending?: boolean;
}

export interface Banner {
    code: string;
    message: string;
}

export interface BodyDraft {
    markdown: string;
    media?: MediaRef[];
}

export interface BookmarkEntry {
    uri: string;
    savedAtMs: number;
    post?: PostSummary;
}

export type Capability = "post" | "comment" | "vote" | "react" | "tag" | "wiki" | "sequence";

export interface ChapterView {
    title?: string;
    body?: RenderedBody;
    posts: PostSummary[];
}

export type CollapseReason = "newUser" | "lowScore" | "muted";

export interface CommentDraft {
    root: string;
    parent?: string;
    body: BodyDraft;
    quote?: TextQuote;
}

export interface CommentNode {
    uri: string;
    cid: string;
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

export type CommentSort = "magic" | "top" | "new" | "old" | "latestReply";

export interface Completeness {
    partial: boolean;
    reposComplete: number;
    reposTotal: number;
    degraded: boolean;
}

export type DefaultTier = "personal" | "frontpage";

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

export type Draft = { kind: "post"; id?: string; post: PostDraft } | { kind: "comment"; id?: string; comment: CommentDraft };

export type DtoVersion = number;

export interface EditOpts {
    history: boolean;
    summary?: string;
}

export interface EffectivePolicy {
    moderation: string;
    karma: string;
    ranking: string;
    reactPalette: string;
    params: [string, string][];
}

export interface EngineError {
    code: ErrorCode;
    message: string;
    retryable: boolean;
    detail?: Value;
}

export type ErrorCode = "auth_required" | "not_found" | "invalid_ref" | "invalid_input" | "conflict" | "rate_limited" | "network" | "blocked" | "too_large" | "partial" | "unsupported" | "internal";

export type Event = { kind: "changed"; topics: Topic[] } | { kind: "syncProgress"; forum: string; done: number; total: number } | { kind: "write"; id: string; state: WriteState } | { kind: "auth"; did: string; state: AuthState } | { kind: "warning"; warning: WarningKind; source?: string; message: string };

export interface ExternalLinkDraft {
    uri: string;
    title?: string;
    description?: string;
}

export interface ExternalLinkView {
    uri: string;
    host: string;
    title?: string;
    description?: string;
}

export interface ForumDraft {
    slug: string;
    name: string;
    description?: string;
    about?: BodyDraft;
    membership: MembershipMode;
    defaultTier: DefaultTier;
    featuredTags?: string[];
    moderators?: ModeratorDraft[];
    reportContact?: string;
}

export type ForumRole = "owner" | "admin" | "moderator" | "member" | "established" | "newcomer" | "none";

export interface ForumSummary {
    uri: string;
    owner: ActorView;
    name: string;
    description?: string;
    membership: MembershipMode;
    followed: boolean;
    member: boolean;
    alias?: string;
    reportContact?: string;
}

export interface ForumUnread {
    forum: string;
    posts: number;
    comments: number;
    notifications: number;
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
    viewer?: ViewerCaps;
}

export type HandleState = "verified" | "unverified" | "invalid";

export type HiddenReason = { kind: "notParticipant" } | { kind: "moderationUnavailable" } | { kind: "removed"; rank: number; actor?: string; spam: boolean; reason?: string } | { kind: "banned" } | { kind: "restricted" } | { kind: "authorModerated" } | { kind: "locked" } | { kind: "newUser" };

export interface HistorySummary {
    revisions: number;
    redacted: boolean;
}

export interface HistoryView {
    dto: DtoVersion;
    uri: string;
    versions: VersionEntry[];
}

export interface IndexerConsent {
    url: string;
    did?: string;
    forum?: string;
    grantedAtMs: number;
}

export type ItemState = { kind: "visible" } | { kind: "collapsed"; reason: CollapseReason } | { kind: "hidden"; reason: HiddenReason } | { kind: "tombstone"; reason: HiddenReason };

export type KarmaCadence = "realtime" | "daily" | "weekly";

export type KarmaValue = "up" | "down" | "strongUp" | "strongDown";

export interface KarmaView {
    k1: number;
    kp: number;
    k: number;
}

export interface LinkView {
    uri: string;
    host: string;
    label?: string;
}

export interface ListPostsQuery {
    forum: string;
    sort: PostSort;
    filter: PostFilter;
    tag?: string;
    kind?: PostKind;
    author?: string;
    cursor?: string;
    limit: number;
}

export interface LoadedMedia {
    cid: string;
    size: number;
    url: string;
}

export interface MediaInput {
    declaredMime?: string;
    alt?: string;
    name?: string;
    aspectRatio?: AspectRatio;
}

export interface MediaRef {
    id: string;
    cid: string;
    mime: string;
    sizeBytes: number;
    alt?: string;
    aspectRatio?: AspectRatio;
    durationMs?: number;
    name?: string;
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

export interface MemberView {
    actor: ActorView;
    status: ForumRole;
    granted: boolean;
    membershipNote?: string;
    joinedAtMs: number;
    karma?: number;
    applicant?: boolean;
    banned?: boolean;
    nullified?: boolean;
    restricted?: string[];
}

export interface MembersQuery {
    status?: ForumRole;
    pending?: boolean;
}

export type MembershipMode = "open" | "approval";

export type ModActionIntent = { kind: "assert"; forum: string; scope: ModScope; subject: string; action: ModActionValue; reason?: string; untilMs?: number } | { kind: "remove"; forum: string; scope: ModScope; subject: string; actionKey: string };

export type ModActionValue = { kind: "hide"; hidden: boolean; spam?: boolean; showReason?: boolean } | { kind: "access"; level: AccessLevel } | { kind: "restrict"; capabilities: Capability[]; nullifyVotes?: boolean } | { kind: "tier"; tier: Tier; notice?: BodyDraft } | { kind: "pin"; scope: PinScope; pinned: boolean } | { kind: "lock"; locked: boolean } | { kind: "tag"; removed: boolean; head?: string };

export interface ModLog {
    dto: DtoVersion;
    entries: ModLogEntry[];
    cursor?: string;
}

export interface ModLogEntry {
    uri: string;
    actor: ActorView;
    action: string;
    subject: string;
    value: string;
    reason?: string;
    atMs: number;
    status: ModLogStatus;
    lapsed: boolean;
    rank?: number;
    authorScope?: boolean;
    adoptedFrom?: string;
}

export type ModLogStatus = "effective" | "overridden" | "lapsed" | "expired" | "stale" | "unauthorised" | "reservedUnknown" | "edited" | "removed";

export interface ModQueue {
    dto: DtoVersion;
    items: ModQueueItem[];
}

export interface ModQueueItem {
    kind: string;
    subject: string;
    reason: string;
    detail?: string;
    uri?: string;
    actor?: ActorView;
    atMs?: number;
}

export type ModRole = "admin" | "moderator";

export type ModScope = "forum" | "author";

export interface ModeratorDraft {
    did: string;
    role: ModRole;
    title?: string;
}

export interface ModeratorView {
    actor: ActorView;
    role: ModRole;
    title?: string;
    shown: boolean;
}

export type ModsChange = { kind: "add"; did: string; role: ModRole; title?: string } | { kind: "remove"; did: string; skipAdopt?: boolean };

export interface MyOpinion {
    karma?: KarmaValue;
    agreement?: AgreementValue;
    pending?: boolean;
    notYetCounted?: boolean;
}

export type NoticeCode = "notEstablished" | "retroactive" | "adoptedFirst";

export interface NotificationList {
    dto: DtoVersion;
    items: NotificationView[];
    unread: number;
}

export type NotificationReason = "reply" | "mention" | "quote" | "pingback" | "newComment" | "newPost" | "reacted" | "modAction" | "karmaChange";

export interface NotificationView {
    id: string;
    reason: NotificationReason;
    atMs: number;
    subject: string;
    forum?: string;
    actor?: ActorView;
    read: boolean;
    preview?: string;
}

export interface NotificationsQuery {
    all?: boolean;
    limit: number;
}

export type OpinionIntent = { kind: "karma"; value?: KarmaValue } | { kind: "agreement"; value?: AgreementValue } | { kind: "react"; id: string; stance: Stance; quote?: TextQuote; remove?: boolean } | { kind: "tag"; slug: string; value?: KarmaValue };

export type PinScope = "forum" | "thread";

export interface PostDraft {
    forum: string;
    kind: PostKind;
    title?: string;
    body: BodyDraft;
    link?: ExternalLinkDraft;
    tags?: string[];
    labels?: string[];
}

export type PostFilter = "frontpage" | "all" | "curated";

export type PostKind = "article" | "link" | "question" | "quickTake";

export interface PostList {
    dto: DtoVersion;
    forum: string;
    sort: PostSort;
    filter: PostFilter;
    items: PostSummary[];
    cursor?: string;
    completeness: Completeness;
    banners?: Banner[];
}

export type PostSort = "magic" | "top" | "new" | "old" | "recentComments";

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

export interface PostView {
    dto: DtoVersion;
    post: PostSummary;
    body: RenderedBody;
    reacts: ReactTally[];
    history: HistorySummary;
    sequences: SequenceNav[];
}

export type PublicActionKind = "vote" | "react" | "tagVote" | "join" | "modAction";

export interface RawView {
    dto: DtoVersion;
    uri: string;
    cid?: string;
    root?: string;
    level: TrustLevel;
    record?: Value;
    validity: string[];
    assertions: Value[];
    state?: ItemState;
    score?: ScoreView;
}

export interface ReactTally {
    react: string;
    label: string;
    glyph: string;
    for: number;
    against: number;
    quote?: TextQuote;
    anchor?: Anchor;
    creator?: string;
    mine?: Stance;
    uncounted?: boolean;
}

export type Ref = { kind: "forum"; uri: string } | { kind: "post"; uri: string } | { kind: "comment"; uri: string } | { kind: "sequence"; uri: string } | { kind: "record"; uri: string } | { kind: "actor"; did: string };

export interface RefContext {
    forum?: string;
}

export interface RenderedBody {
    html: string;
    text: string;
    toc: TocEntry[];
    wordCount: number;
    readMinutes: number;
    media: MediaView[];
    excerpt: string;
    notices: string[];
    rendererVersion: string;
}

export interface ScoreView {
    score: number;
    agreement?: number;
    unweighted: number;
    earlierVersionVotes: number;
    weightedVotes?: number;
    mine?: MyOpinion;
}

export interface SearchHit {
    item: SearchItem;
    snippet: string;
    highlights: [number, number][];
}

export type SearchItem = PostSummary | CommentNode;

export interface SearchResults {
    dto: DtoVersion;
    hits: SearchHit[];
}

export interface SequenceNav {
    sequence: string;
    title: string;
    prev?: string;
    next?: string;
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

export interface SequenceView {
    dto: DtoVersion;
    sequence: SequenceSummary;
    body?: RenderedBody;
    chapters: ChapterView[];
}

export interface Settings {
    sources: Sources;
    verify: VerifyMode;
    indexers: IndexerConsent[];
    externalImagesForums: string[];
    mediaAutoloadHosts: string[];
    karmaNotifications: KarmaCadence;
    acknowledgedPublicActions: PublicActionKind[];
}

export type Sources = "direct" | "assisted";

export type Stance = "for" | "against";

export type Subscription = { kind: "forum"; uri: string } | { kind: "post"; uri: string } | { kind: "user"; did: string } | { kind: "tag"; forum: string; slug: string } | { kind: "sequence"; uri: string };

export interface SubscriptionEntry {
    subscription: Subscription;
    sinceMs: number;
}

export interface SyncIssue {
    did?: string;
    host?: string;
    code: string;
    message: string;
}

export interface SyncOpts {
    budgetMs?: number;
    maxRequests?: number;
    full?: boolean;
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

export interface TextQuote {
    exact: string;
    prefix?: string;
    suffix?: string;
    start?: number;
}

export interface ThreadOpts {
    sort: CommentSort;
    unreadOnly?: boolean;
}

export type ThreadRoot = PostView | { missing: string };

export interface ThreadView {
    dto: DtoVersion;
    post: ThreadRoot;
    comments: CommentNode[];
    sort: CommentSort;
    completeness: Completeness;
    banners?: Banner[];
}

export type Tier = "personal" | "frontpage" | "curated";

export interface TocEntry {
    level: number;
    id: string;
    text: string;
}

export type Topic = { kind: "forum"; uri: string } | { kind: "thread"; uri: string } | { kind: "user"; did: string } | { kind: "notifications" } | { kind: "outbox" } | { kind: "drafts" } | { kind: "accounts" };

export type TrustLevel = "pds" | "proven" | "hint";

export interface UnreadCounts {
    dto: DtoVersion;
    notifications: number;
    forums: ForumUnread[];
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

export interface VerifyCheck {
    name: string;
    ok: boolean;
    detail?: string;
}

export type VerifyMode = "pds" | "proof";

export type VerifyOutcome = "proven" | "absent" | "failed";

export interface VerifyReport {
    dto: DtoVersion;
    uri: string;
    result: VerifyOutcome;
    rev?: string;
    commitCid?: string;
    keySource: string;
    checks: VerifyCheck[];
}

export interface VersionEntry {
    n: number;
    cid: string;
    createdAtMs: number;
    summary?: string;
    available: boolean;
}

export type VersionRef = { revision: number } | { unavailable: boolean };

export interface ViewerCaps {
    vote: boolean;
    strongVote: boolean;
    agree: boolean;
    weighted: boolean;
    react: boolean;
    reactCreate: boolean;
    reactSecond: boolean;
    reactAnti: boolean;
    post: boolean;
    comment: boolean;
    newcomer: boolean;
    applicant: boolean;
    moderate: boolean;
    rank?: number;
    curate: boolean;
    authorModPersonal: boolean;
    authorModFrontpage: boolean;
}

export type WarningKind = "equivocation" | "indexerContradiction" | "degraded" | "renderFailure" | "other";

export type WriteState = { kind: "pending"; notBeforeMs?: number; held?: boolean; throttled?: boolean; needsLogin?: boolean; error?: EngineError } | { kind: "sending" } | { kind: "done"; uri: string; cid?: string } | { kind: "failed"; error: EngineError } | { kind: "undone" };

export interface WriteTicket {
    id: string;
    state: WriteState;
}
