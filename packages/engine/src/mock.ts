// SPDX-License-Identifier: AGPL-3.0-or-later
//
// MockEngine: an in-memory implementation of `EngineApi` over a realistic, deterministic
// fixture (./mock/fixture.ts). The SPA is built and tested against it until the wasm engine
// implements the same operations. Writes mutate the in-memory state, report `WriteTicket`
// states through `write` events (opinions are held for the 4 s undo window), and announce
// `changed` topics so the UI re-queries. No Math.random and no wall clock: the mock has a
// logical clock that starts at the fixture's "now" and advances one second per write.

import type { EngineApi } from "./api.ts";
import { buildFixture, FIXTURE_NOW, FK, FORUM_URI } from "./mock/fixture.ts";
import type {
  ActorRec,
  BaseReact,
  BaseScore,
  CommentRec,
  ForumRec,
  MockState,
  MyOpinionRec,
  PolicyRec,
  PostRec,
  SequenceRec,
  TagRec,
} from "./mock/model.ts";
import { renderMarkdown } from "./mock/render.ts";
import { isDid, isHandle, safeHref } from "./mock/safe-href.ts";
import {
  atUri,
  b32hash,
  COLL,
  clone,
  didFor,
  fakeBlobCid,
  fakeCid,
  MOCK_RENDERER_VERSION,
  normalizeP,
  shortDid,
  splitAtUri,
  tid,
} from "./mock/util.ts";
import { paletteReact } from "./palette.ts";
import {
  type AccountInfo,
  type AccountView,
  type ActorView,
  type Anchor,
  type AtUri,
  type BookmarkEntry,
  type CommentDraft,
  type CommentNode,
  type Completeness,
  type Did,
  type DiscussionList,
  type Draft,
  type DraftEntry,
  DTO_VERSION,
  type EditOpts,
  type EffectivePolicy,
  type EngineError,
  type EngineErrorCode,
  type EngineEvent,
  type EventListener,
  type ForumDraft,
  type ForumRole,
  type ForumSummary,
  type ForumView,
  type HistoryView,
  type Href,
  type HrefContext,
  type ItemState,
  type Json,
  type ListPostsQuery,
  type MediaInput,
  type MediaRef,
  type MembersQuery,
  type MemberView,
  type ModActionIntent,
  type ModLog,
  type ModQueue,
  type ModsChange,
  type NotificationList,
  type NotificationsQuery,
  OPINION_UNDO_WINDOW_MS,
  type OpinionIntent,
  type PostDraft,
  type PostList,
  type PostSummary,
  type PostView,
  type RawView,
  type ReactTally,
  type Ref,
  type RefContext,
  type RenderedBody,
  type RenderOpts,
  type RkeyInput,
  type ScoreView,
  type SearchHit,
  type SearchQuery,
  type SearchResults,
  type SequenceDraft,
  type SequencePosition,
  type SequenceSummary,
  type SequenceView,
  type SettingKey,
  type Settings,
  type Slug,
  type Subscription,
  type SubscriptionEntry,
  type SyncOpts,
  type SyncReport,
  type TagSummary,
  type TagView,
  type TextQuote,
  type ThreadOpts,
  type ThreadView,
  type Topic,
  type Unsubscribe,
  type UserView,
  type ValidationReport,
  type VerifyReport,
  type WikiDraft,
  type WriteState,
  type WriteTicket,
} from "./types.ts";

export { FIXTURE_NOW } from "./mock/fixture.ts";
export { safeHref as mockSafeHref } from "./mock/safe-href.ts";

export interface MockEngineOptions {
  /** How long opinion intents are held before sending (default `OPINION_UNDO_WINDOW_MS`). */
  holdMs?: number;
  /** Simulated latency of every write once it is sent (default 250 ms). */
  sendMs?: number;
  /** Start without a signed-in account. */
  signedOut?: boolean;
}

type Timer = ReturnType<typeof setTimeout>;

interface WriteRec {
  id: string;
  state: WriteState;
  undoable: boolean;
  timer?: Timer;
  apply: () => { uri: AtUri; cid: string; topics: Topic[] };
}

const RENDER_NOTICE = "Rendered by the mock engine's minimal markdown renderer.";
const AUTHORITY: ReadonlySet<string> = new Set(["owner", "admin", "moderator"]);
const RANK: Record<string, number> = { owner: 3, admin: 2, moderator: 1 };

function err(code: EngineErrorCode, message: string, retryable = false): EngineError {
  return { code, message, retryable };
}

function karmaPower(v: string | undefined): number {
  switch (v) {
    case "up":
    case "agree":
      return 1;
    case "strongUp":
    case "strongAgree":
      return 3;
    case "down":
    case "disagree":
      return -1;
    case "strongDown":
    case "strongDisagree":
      return -3;
    default:
      return 0;
  }
}

function qkey(q: TextQuote | undefined): string {
  return q ? normalizeP(q.exact) : "";
}

function listed(s: ItemState): boolean {
  return s.kind === "visible" || s.kind === "collapsed";
}

function cpIndexOf(text: string, exact: string): Anchor {
  const i = text.indexOf(exact);
  if (i < 0) return { orphan: true };
  const start = Array.from(text.slice(0, i)).length;
  return { start, end: start + Array.from(exact).length };
}

function emptyBody(notice: string): RenderedBody {
  return {
    html: `<div class="lfm" dir="auto">\n<p class="notice" dir="auto">${notice}</p>\n</div>\n`,
    text: "",
    toc: [],
    wordCount: 0,
    readMinutes: 0,
    media: [],
    excerpt: "",
    notices: [],
    rendererVersion: MOCK_RENDERER_VERSION,
  };
}

/** The effective policy as the core reports it: algorithm ids plus name/value pairs. */
function effectivePolicy(p: PolicyRec): EffectivePolicy {
  const params: [string, string][] = [
    ["voting", p.voting],
    ["authorModeration", String(p.authorModeration)],
    ["tagCreation", p.tagCreation],
    ["karma.pairCap", String(p.karma.pairCap)],
    ["karma.establish", String(p.karma.establish)],
    ["karma.establishSeeds", String(p.karma.establishSeeds)],
    ["ranking.offsetHours", String(p.ranking.offsetHours)],
    ["ranking.exponentMilli", String(p.ranking.exponentMilli)],
    ["ranking.curatedBonus", String(p.ranking.curatedBonus)],
    ["ranking.frontpageBonus", String(p.ranking.frontpageBonus)],
    ["ranking.cutoffDays", String(p.ranking.cutoffDays)],
    ...Object.entries(p.gates).map(([k, v]): [string, string] => [`gates.${k}`, String(v)]),
    ["display.collapseBelow", String(p.display.collapseBelow)],
    ["display.newUsers", p.display.newUsers],
  ];
  return {
    moderation: p.moderation,
    karma: p.karma.algorithm,
    ranking: p.ranking.algorithm,
    reactPalette: p.reactPalette,
    params,
  };
}

/** social.md §12.1 `hot`, in milliseconds. */
function hot(
  b: number,
  tMs: number,
  now: number,
  offsetHours: number,
  exponentMilli: number,
): number {
  const age = Math.max(now - tMs, 0);
  const h = age / 3_600_000;
  return b / (h + offsetHours) ** (exponentMilli / 1000);
}

export class MockEngine implements EngineApi {
  private readonly s: MockState;
  private readonly holdMs: number;
  private readonly sendMs: number;
  private ticks = 0;
  private readonly listeners = new Set<EventListener>();
  private readonly queue: EngineEvent[] = [];
  private flushing = false;
  private readonly writes = new Map<string, WriteRec>();

  constructor(opts: MockEngineOptions = {}) {
    this.s = buildFixture().state;
    this.holdMs = opts.holdMs ?? OPINION_UNDO_WINDOW_MS;
    this.sendMs = opts.sendMs ?? 250;
    if (opts.signedOut) this.s.active = null;
  }

  // ------------------------------------------------------------------------------- plumbing

  /** The logical clock: the fixture's "now" plus one second per write. */
  private now(): number {
    return FIXTURE_NOW + this.ticks * 1000;
  }

  private tick(): number {
    this.ticks++;
    return this.now();
  }

  private nextId(prefix: string): string {
    return `${prefix}-${this.s.nextId++}`;
  }

  private emit(event: EngineEvent): void {
    this.queue.push(event);
    if (this.flushing) return;
    this.flushing = true;
    queueMicrotask(() => {
      while (this.queue.length > 0) {
        const e = this.queue.shift();
        if (!e) break;
        for (const l of [...this.listeners]) {
          try {
            l(e);
          } catch (error) {
            console.error("engine event listener failed", error);
          }
        }
      }
      this.flushing = false;
    });
  }

  private changed(...topics: Topic[]): void {
    this.emit({ kind: "changed", topics });
  }

  private me(): Did {
    if (!this.s.active) throw err("auth_required", "Sign in to do this.");
    return this.s.active;
  }

  private actorRec(did: Did): ActorRec {
    let a = this.s.actors.get(did);
    if (!a) {
      a = {
        view: { did, handleState: "unverified", shortDid: shortDid(did) },
        pds: "https://pds.example.test",
        links: [],
        karma: { k1: 0, kp: 0, k: 0 },
        status: "none",
        joinedAtMs: this.now(),
        granted: false,
        pinned: [],
      };
      this.s.actors.set(did, a);
    }
    return a;
  }

  private actor(did: Did): ActorView {
    return clone(this.actorRec(did).view);
  }

  private forumRec(uri: AtUri): ForumRec {
    const f = this.s.forums.get(uri);
    if (!f) throw err("not_found", `No forum ${uri}`);
    return f;
  }

  private myRole(forum: ForumRec): ForumRole {
    const me = this.s.active;
    if (!me) return "none";
    if (forum.summary.owner === me) return "owner";
    const mod = forum.moderators.find((m) => m.did === me);
    if (mod) return mod.role;
    const status = this.s.actors.get(me)?.status ?? "none";
    if (status === "member" || status === "established" || status === "newcomer") return status;
    return "none";
  }

  private isAuthorityDid(forumUri: AtUri, did: Did): boolean {
    const f = this.s.forums.get(forumUri);
    return f !== undefined && (f.summary.owner === did || f.moderators.some((m) => m.did === did));
  }

  private isAuthority(forumUri: AtUri): boolean {
    return AUTHORITY.has(this.myRole(this.forumRec(forumUri)));
  }

  private completeness(): Completeness {
    return { partial: true, reposComplete: 8, reposTotal: 9, degraded: false };
  }

  private myOpinion(subject: AtUri): MyOpinionRec | undefined {
    const me = this.s.active;
    if (!me) return undefined;
    return this.s.opinions.get(me)?.get(subject);
  }

  private scoreView(subject: AtUri, base: BaseScore): ScoreView {
    const mine = this.myOpinion(subject);
    const view: ScoreView = {
      score: base.score + karmaPower(mine?.karma),
      agreement: base.agreement + karmaPower(mine?.agreement),
      unweighted: base.unweighted,
      earlierVersionVotes: base.earlier,
    };
    if (mine && (mine.karma !== undefined || mine.agreement !== undefined)) {
      view.mine = {};
      if (mine.karma !== undefined) view.mine.karma = mine.karma;
      if (mine.agreement !== undefined) view.mine.agreement = mine.agreement;
    }
    return view;
  }

  private reacts(subject: AtUri, base: BaseReact[], text: string): ReactTally[] {
    const out: ReactTally[] = base.map((b) => {
      const p = paletteReact(b.react);
      return { ...clone(b), label: p?.label ?? b.react, glyph: p?.glyph ?? "?" };
    });
    const mine = this.myOpinion(subject)?.reacts ?? [];
    for (const r of mine) {
      const k = qkey(r.quote);
      const t = out.find((x) => x.react === r.react && qkey(x.quote) === k);
      if (t) {
        if (r.stance === "for") t.for++;
        else t.against++;
        t.mine = r.stance;
      } else if (r.stance === "for") {
        const p = paletteReact(r.react);
        const tally: ReactTally = {
          react: r.react,
          label: p?.label ?? r.react,
          glyph: p?.glyph ?? "?",
          for: 1,
          against: 0,
          mine: "for",
        };
        if (this.s.active) tally.creator = this.s.active;
        if (r.quote) {
          tally.quote = clone(r.quote);
          tally.anchor = cpIndexOf(text, normalizeP(r.quote.exact));
        }
        out.push(tally);
      }
    }
    return out;
  }

  /** The lens (social.md §10.3): mutes collapse listed items. */
  private lens(author: Did, state: ItemState): ItemState {
    if (listed(state) && this.s.mutes.has(author)) return { kind: "collapsed", reason: "muted" };
    return clone(state);
  }

  private postSummary(p: PostRec): PostSummary {
    const thread = [...this.s.comments.values()].filter((c) => c.root === p.uri);
    const listedComments = thread.filter((c) => listed(c.state));
    const visibleTimes = thread.filter((c) => c.state.kind === "visible").map((c) => c.tRankMs);
    const summary: PostSummary = {
      uri: p.uri,
      cid: p.cid,
      forum: p.forum,
      author: this.actor(p.author),
      kind: p.kind,
      excerpt: p.body.excerpt,
      tags: [...p.tags],
      tier: p.tier,
      pinned: p.pinned,
      locked: p.locked,
      createdAtMs: p.createdAtMs,
      tRankMs: p.tRankMs,
      commentCount: listedComments.length,
      score: this.scoreView(p.uri, p.base),
      state: this.lens(p.author, p.state),
      badges: { ...p.badges },
      labels: [...p.labels],
    };
    if (p.kind !== "quickTake" && p.title !== undefined) summary.title = p.title;
    if (visibleTimes.length > 0) summary.lastCommentAtMs = Math.max(...visibleTimes);
    const lv = this.s.lastVisited.get(p.uri);
    if (lv !== undefined) {
      summary.unreadComments = listedComments.filter(
        (c) => c.tRankMs > lv && c.author !== this.s.active,
      ).length;
    }
    if (p.link) summary.link = clone(p.link);
    return summary;
  }

  private fkOf(uri: AtUri): string {
    return uri === FORUM_URI ? FK : b32hash(`lessdugin-fk/1:${uri}`, 12);
  }

  private postView(p: PostRec): PostView {
    const summary = this.postSummary(p);
    const hidden = p.state.kind === "hidden";
    const view: PostView = {
      dto: DTO_VERSION,
      post: summary,
      body: hidden ? emptyBody("[hidden]") : clone(p.body),
      reacts: hidden ? [] : this.reacts(p.uri, p.reacts, p.body.text),
      history: { revisions: p.revisions.length, redacted: p.redacted },
      sequences: [],
    };
    for (const seq of this.s.sequences.values()) {
      const entries = seq.chapters.flatMap((c) => c.posts).filter((u) => this.s.posts.has(u));
      const i = entries.indexOf(p.uri);
      if (i < 0) continue;
      const nav: PostView["sequences"][number] = { sequence: seq.uri, title: seq.title };
      const prev = entries[i - 1];
      const next = entries[i + 1];
      if (prev) nav.prev = prev;
      if (next) nav.next = next;
      view.sequences.push(nav);
    }
    return view;
  }

  private commentNode(
    c: CommentRec,
    depth: number,
    parent: AtUri | undefined,
    state: ItemState,
  ): CommentNode {
    const lv = this.s.lastVisited.get(c.root);
    const node: CommentNode = {
      uri: c.uri,
      cid: c.cid,
      depth,
      author: this.actor(c.author),
      body: state.kind === "tombstone" ? emptyBody("[removed]") : clone(c.body),
      createdAtMs: c.createdAtMs,
      tRankMs: c.tRankMs,
      score: this.scoreView(c.uri, c.base),
      reacts: state.kind === "tombstone" ? [] : this.reacts(c.uri, c.reacts, c.body.text),
      state,
      badges: { ...c.badges },
      retracted: c.retracted,
      moderatorHat: c.moderatorHat && this.isAuthorityDid(c.forum, c.author),
      orphaned: c.orphaned,
      missing: false,
      unread: lv !== undefined && c.tRankMs > lv && c.author !== this.s.active,
      pinned: c.pinned,
    };
    if (parent !== undefined) node.parent = parent;
    if (c.quote) node.quote = clone(c.quote);
    if (c.quoteAnchor) node.quoteAnchor = clone(c.quoteAnchor);
    if (c.version) node.version = clone(c.version);
    return node;
  }

  private missingNode(uri: AtUri, depth: number): CommentNode {
    const author = splitAtUri(uri)?.did ?? "did:plc:unknown";
    return {
      uri,
      cid: "",
      depth,
      author: this.actor(author),
      body: emptyBody("[missing]"),
      createdAtMs: 0,
      tRankMs: 0,
      score: { score: 0, agreement: 0, unweighted: 0, earlierVersionVotes: 0 },
      reacts: [],
      state: { kind: "visible" },
      badges: {},
      retracted: false,
      moderatorHat: false,
      orphaned: false,
      missing: true,
      unread: false,
      pinned: false,
    };
  }

  /** Thread placement (social.md §11), thread pass (§10.2) and comment sorts (§12.4). */
  private thread(rootUri: AtUri, sort: ThreadOpts["sort"]): CommentNode[] {
    const recs = [...this.s.comments.values()].filter((c) => c.root === rootUri);
    const byUri = new Map(recs.map((c) => [c.uri, c]));
    const children = new Map<string, string[]>(); // parent key → child uris ("" = root)
    const missing = new Set<string>();
    const add = (parent: string, child: string): void => {
      const list = children.get(parent) ?? [];
      list.push(child);
      children.set(parent, list);
    };
    for (const c of recs) {
      if (c.orphaned || c.parent === undefined) add("", c.uri);
      else if (byUri.has(c.parent)) add(c.parent, c.uri);
      else {
        if (!missing.has(c.parent)) {
          missing.add(c.parent);
          add("", c.parent);
        }
        add(c.parent, c.uri);
      }
    }
    const state = (uri: string): ItemState => {
      const c = byUri.get(uri);
      return c ? this.lens(c.author, c.state) : { kind: "visible" };
    };
    // listed descendants, bottom-up over a post-order walk
    const hasListed = new Map<string, boolean>();
    const order: string[] = [];
    const stack: string[] = [""];
    while (stack.length > 0) {
      const u = stack.pop() ?? "";
      order.push(u);
      for (const ch of children.get(u) ?? []) stack.push(ch);
    }
    for (let i = order.length - 1; i >= 0; i--) {
      const u = order[i] ?? "";
      const any = (children.get(u) ?? []).some(
        (ch) => (byUri.has(ch) && listed(state(ch))) || hasListed.get(ch) === true,
      );
      hasListed.set(u, any);
    }
    const now = this.now();
    const latest = (uri: string): number => {
      let best = byUri.get(uri)?.tRankMs ?? 0;
      const st: string[] = [...(children.get(uri) ?? [])];
      while (st.length > 0) {
        const u = st.pop() ?? "";
        const c = byUri.get(u);
        if (c && c.state.kind === "visible") best = Math.max(best, c.tRankMs);
        st.push(...(children.get(u) ?? []));
      }
      return best;
    };
    const cmp = (a: string, b: string): number => {
      const ca = byUri.get(a);
      const cb = byUri.get(b);
      if (!ca || !cb) {
        if (!ca && !cb) return a < b ? -1 : a > b ? 1 : 0;
        return ca ? -1 : 1; // missing placeholders after comments
      }
      if (ca.pinned !== cb.pinned) return ca.pinned ? -1 : 1;
      const sa = this.scoreView(a, ca.base).score;
      const sb = this.scoreView(b, cb.base).score;
      let d = 0;
      switch (sort) {
        case "top":
          d = sb - sa || cb.tRankMs - ca.tRankMs;
          break;
        case "new":
          d = cb.tRankMs - ca.tRankMs;
          break;
        case "old":
          d = ca.tRankMs - cb.tRankMs;
          break;
        case "latestReply":
          d = latest(b) - latest(a);
          break;
        default: {
          const pa = sa > 0;
          const pb = sb > 0;
          if (pa !== pb) d = pa ? -1 : 1;
          else if (pa) d = hot(sb, cb.tRankMs, now, 2, 1150) - hot(sa, ca.tRankMs, now, 2, 1150);
          else d = sb - sa || cb.tRankMs - ca.tRankMs;
        }
      }
      if (d !== 0) return d < 0 ? -1 : 1;
      return a < b ? -1 : a > b ? 1 : 0;
    };
    const out: CommentNode[] = [];
    const walk: { uri: string; depth: number; parent?: string }[] = [];
    const pushChildren = (parent: string, depth: number): void => {
      const kids = [...(children.get(parent) ?? [])].sort(cmp);
      for (let i = kids.length - 1; i >= 0; i--) {
        const k = kids[i] ?? "";
        walk.push(parent === "" ? { uri: k, depth } : { uri: k, depth, parent });
      }
    };
    pushChildren("", 1);
    while (walk.length > 0) {
      const w = walk.pop();
      if (!w) break;
      const c = byUri.get(w.uri);
      if (!c) {
        if (hasListed.get(w.uri)) {
          out.push(this.missingNode(w.uri, w.depth));
          pushChildren(w.uri, w.depth + 1);
        }
        continue;
      }
      const st = state(w.uri);
      if (listed(st)) {
        out.push(this.commentNode(c, w.depth, w.parent, st));
      } else if (st.kind === "hidden" && hasListed.get(w.uri)) {
        out.push(this.commentNode(c, w.depth, w.parent, { kind: "tombstone", reason: st.reason }));
      } else {
        continue;
      }
      pushChildren(w.uri, w.depth + 1);
    }
    return out;
  }

  private postRec(uri: AtUri): PostRec {
    const p = this.s.posts.get(uri);
    if (!p) throw err("not_found", `No post ${uri}`);
    return p;
  }

  private resolveAuthority(authority: string): Did {
    const a = decodeURIComponent(authority).replace(/^@/, "");
    if (isDid(a)) return a;
    const lower = a.toLowerCase();
    for (const rec of this.s.actors.values()) {
      if (rec.view.handle === lower) return rec.view.did;
    }
    throw err("not_found", `Could not resolve ${a}`);
  }

  private refForUri(did: Did, collection: string, rkey: string): Ref {
    const uri = atUri(did, collection, rkey);
    switch (collection) {
      case COLL.forum:
        return { kind: "forum", uri };
      case COLL.post:
        return { kind: "post", uri };
      case COLL.comment:
        return { kind: "comment", uri };
      case COLL.sequence:
        return { kind: "sequence", uri };
      default:
        return { kind: "record", uri };
    }
  }

  // ------------------------------------------------------------------------------- writes

  private write(
    undoable: boolean,
    apply: () => { uri: AtUri; cid: string; topics: Topic[] },
  ): WriteTicket {
    const id = this.nextId("w");
    const rec: WriteRec = { id, state: { kind: "pending" }, undoable, apply };
    this.writes.set(id, rec);
    this.emit({ kind: "write", id, state: { kind: "pending" } });
    this.changed({ kind: "outbox" });
    rec.timer = setTimeout(() => this.send(id), undoable ? this.holdMs : 0);
    return { id, state: { kind: "pending" } };
  }

  private send(id: string): void {
    const rec = this.writes.get(id);
    if (rec?.state.kind !== "pending") return;
    rec.state = { kind: "sending" };
    this.emit({ kind: "write", id, state: rec.state });
    rec.timer = setTimeout(() => {
      try {
        const r = rec.apply();
        rec.state = { kind: "done", uri: r.uri, cid: r.cid };
        this.emit({ kind: "write", id, state: clone(rec.state) });
        this.changed(...r.topics, { kind: "outbox" });
      } catch (e) {
        const error =
          typeof e === "object" && e !== null && "code" in e
            ? (e as EngineError)
            : err("internal", String(e));
        rec.state = { kind: "failed", error };
        this.emit({ kind: "write", id, state: clone(rec.state) });
        this.changed({ kind: "outbox" });
      }
    }, this.sendMs);
  }

  private threadTopics(root: AtUri): Topic[] {
    const p = this.s.posts.get(root);
    const forum = p ? p.forum : FORUM_URI;
    const topics: Topic[] = [
      { kind: "thread", uri: root },
      { kind: "forum", uri: forum },
    ];
    if (this.s.active) topics.push({ kind: "user", did: this.s.active });
    return topics;
  }

  private subjectInfo(uri: AtUri): { author: Did; root: AtUri; forum: AtUri; text: string } {
    const p = this.s.posts.get(uri);
    if (p) return { author: p.author, root: p.uri, forum: p.forum, text: p.body.text };
    const c = this.s.comments.get(uri);
    if (c) return { author: c.author, root: c.root, forum: c.forum, text: c.body.text };
    const seq = this.s.sequences.get(uri);
    if (seq) return { author: seq.author, root: seq.uri, forum: FORUM_URI, text: seq.body ?? "" };
    throw err("not_found", `No subject ${uri}`);
  }

  // =============================================================================== §3.1

  async resolveActor(input: string): Promise<ActorView> {
    const s = input.trim().replace(/^@/, "");
    if (isDid(s)) {
      if (!this.s.actors.has(s)) throw err("not_found", `Unknown identity ${s}`);
      return this.actor(s);
    }
    if (!isHandle(s)) throw err("invalid_input", `"${input}" is neither a handle nor a DID.`);
    const handle = s.toLowerCase();
    for (const a of this.s.actors.values()) {
      if (a.view.handle === handle) return clone(a.view);
    }
    // The mock "resolves" any other well-formed handle to a deterministic new identity.
    const did = didFor(`handle:${handle}`);
    const rec = this.actorRec(did);
    rec.view = { did, handle, handleState: "verified", shortDid: shortDid(did) };
    return clone(rec.view);
  }

  private accountView(did: Did): AccountView {
    const a = this.s.accounts.find((x) => x.did === did);
    if (!a) throw err("not_found", `No account ${did}`);
    const view: AccountView = { did, pds: a.pds, authKind: a.auth, active: this.s.active === did };
    const actor = this.s.actors.get(did)?.view;
    if (actor?.handle !== undefined) view.handle = actor.handle;
    return view;
  }

  async accounts(): Promise<AccountView[]> {
    return this.s.accounts.map((a) => this.accountView(a.did));
  }

  async addAccount(info: AccountInfo): Promise<AccountView> {
    this.actorRec(info.did);
    if (!this.s.accounts.some((a) => a.did === info.did)) {
      this.s.accounts.push({ did: info.did, pds: info.pds, auth: info.auth });
    }
    if (!this.s.active) this.s.active = info.did;
    this.changed({ kind: "accounts" });
    this.emit({ kind: "auth", did: info.did, state: { kind: "ok" } });
    return this.accountView(info.did);
  }

  async removeAccount(did: Did, purge: boolean): Promise<void> {
    this.s.accounts = this.s.accounts.filter((a) => a.did !== did);
    if (this.s.active === did) this.s.active = this.s.accounts[0]?.did ?? null;
    if (purge) this.s.opinions.delete(did);
    this.changed(
      { kind: "accounts" },
      { kind: "forum", uri: FORUM_URI },
      { kind: "notifications" },
    );
  }

  async setActiveAccount(did: Did): Promise<void> {
    if (!this.s.accounts.some((a) => a.did === did)) throw err("not_found", `No account ${did}`);
    this.s.active = did;
    this.changed(
      { kind: "accounts" },
      { kind: "forum", uri: FORUM_URI },
      { kind: "notifications" },
    );
  }

  async activeAccount(): Promise<AccountView | null> {
    return this.s.active ? this.accountView(this.s.active) : null;
  }

  async parseRef(input: string, ctx: RefContext): Promise<Ref> {
    const raw = input.trim();
    const route = /^#?\/(p|c|s|f|u|at)\/(.+)$/.exec(raw.replace(/[?].*$/, ""));
    if (route) {
      const kind = route[1];
      const parts = (route[2] ?? "").split("/").map((p) => decodeURIComponent(p));
      const [a, b] = parts;
      if (kind === "at") return this.parseRef(`at://${parts.join("/")}`, ctx);
      if (kind === "u" && a) return { kind: "actor", did: this.resolveAuthority(a) };
      if (!a || !b) throw err("invalid_ref", `Incomplete reference ${input}`);
      const did = this.resolveAuthority(a);
      if (kind === "f") return { kind: "forum", uri: atUri(did, COLL.forum, b) };
      if (kind === "p") return { kind: "post", uri: atUri(did, COLL.post, b) };
      if (kind === "c") return { kind: "comment", uri: atUri(did, COLL.comment, b) };
      return { kind: "sequence", uri: atUri(did, COLL.sequence, b) };
    }
    if (raw.startsWith("at://")) {
      const segs = raw.slice(5).split("/");
      const [a, coll, rkey] = segs;
      if (!a) throw err("invalid_ref", `Invalid at-uri ${input}`);
      const did = this.resolveAuthority(a);
      if (segs.length === 1) return { kind: "actor", did };
      if (segs.length !== 3 || !coll || !rkey) throw err("invalid_ref", `Invalid at-uri ${input}`);
      return this.refForUri(did, coll, rkey);
    }
    const bare = raw.replace(/^@/, "");
    if (isDid(bare)) return { kind: "actor", did: bare };
    const slash = /^([^/]+)\/([^/]+)$/.exec(bare);
    if (slash?.[1] && slash[2]) {
      if (!ctx.forum) throw err("invalid_ref", "A handle/rkey reference needs a current forum.");
      return { kind: "post", uri: atUri(this.resolveAuthority(slash[1]), COLL.post, slash[2]) };
    }
    if (isHandle(bare)) return { kind: "actor", did: this.resolveAuthority(bare) };
    throw err("invalid_ref", `Not a reference: ${input}`);
  }

  // =============================================================================== §3.2

  private forumSummary(f: ForumRec): ForumSummary {
    const me = this.s.active;
    const status = me ? (this.s.actors.get(me)?.status ?? "none") : "none";
    const summary: ForumSummary = {
      uri: f.summary.uri,
      owner: this.actor(f.summary.owner),
      name: f.summary.name,
      membership: f.membership,
      followed: this.s.followed.has(f.summary.uri),
      member: me !== null && status !== "none",
    };
    if (f.summary.description !== undefined) summary.description = f.summary.description;
    const alias = this.s.followed.get(f.summary.uri);
    if (alias !== undefined) summary.alias = alias;
    return summary;
  }

  async forumOpen(ref: string): Promise<ForumView> {
    const parsed =
      ref.startsWith("at://") && ref.includes(`/${COLL.forum}/`)
        ? null
        : await this.parseRef(ref, {});
    const uri = parsed ? (parsed.kind === "forum" ? parsed.uri : null) : ref;
    if (!uri) throw err("invalid_ref", `${ref} is not a forum`);
    const f = this.forumRec(uri);
    const view: ForumView = {
      dto: DTO_VERSION,
      forum: this.forumSummary(f),
      moderators: f.moderators.map((m) => {
        const mod: ForumView["moderators"][number] = {
          actor: this.actor(m.did),
          role: m.role,
          shown: m.shown,
        };
        if (m.title !== undefined) mod.title = m.title;
        return mod;
      }),
      policy: effectivePolicy(f.policy),
      myRole: this.myRole(f),
      banners: clone(f.banners),
      completeness: this.completeness(),
    };
    if (f.about !== undefined) view.about = renderMarkdown(f.about).body;
    const me = this.s.active;
    if (me) view.myKarma = this.s.actors.get(me)?.karma.k ?? 0;
    return view;
  }

  async forumDiscover(q: string): Promise<ForumSummary[]> {
    const needle = q.trim().toLowerCase();
    return [...this.s.forums.values()]
      .filter((f) => needle === "" || f.summary.name.toLowerCase().includes(needle))
      .map((f) => this.forumSummary(f));
  }

  async forumFollow(uri: AtUri, alias?: string): Promise<void> {
    this.forumRec(uri);
    this.s.followed.set(uri, alias);
    this.changed({ kind: "forum", uri });
  }

  async forumUnfollow(uri: AtUri): Promise<void> {
    this.s.followed.delete(uri);
    this.changed({ kind: "forum", uri });
  }

  async followed(): Promise<ForumSummary[]> {
    return [...this.s.followed.keys()]
      .map((u) => this.s.forums.get(u))
      .filter((f): f is ForumRec => f !== undefined)
      .map((f) => this.forumSummary(f));
  }

  async forumCreate(draft: ForumDraft): Promise<WriteTicket> {
    const me = this.me();
    if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(draft.slug)) throw err("invalid_input", "Invalid slug.");
    const uri = atUri(me, COLL.forum, draft.slug);
    if (this.s.forums.has(uri)) throw err("conflict", "A forum with this slug already exists.");
    return this.write(false, () => {
      const f: ForumRec = {
        summary: { uri, owner: me, name: draft.name, membership: draft.membership },
        moderators: draft.moderators.map((m) => ({ ...m, shown: true })),
        policy: clone(this.forumRec(FORUM_URI).policy),
        banners: [],
        featuredTags: [...draft.featuredTags],
        defaultTier: draft.defaultTier,
        membership: draft.membership,
      };
      if (draft.description !== undefined) f.summary.description = draft.description;
      if (draft.about) f.about = draft.about.markdown;
      this.s.forums.set(uri, f);
      this.s.followed.set(uri, undefined);
      this.s.tags.set(uri, new Map());
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: [{ kind: "forum", uri }] };
    });
  }

  async forumUpdate(uri: AtUri, draft: ForumDraft): Promise<WriteTicket> {
    const f = this.forumRec(uri);
    if (f.summary.owner !== this.me())
      throw err("blocked", "Only the owner can edit the declaration.");
    return this.write(false, () => {
      f.summary.name = draft.name;
      if (draft.description !== undefined) f.summary.description = draft.description;
      else delete f.summary.description;
      if (draft.about) f.about = draft.about.markdown;
      f.membership = draft.membership;
      f.summary.membership = draft.membership;
      f.defaultTier = draft.defaultTier;
      f.featuredTags = [...draft.featuredTags];
      f.moderators = draft.moderators.map((m) => ({ ...m, shown: true }));
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: [{ kind: "forum", uri }] };
    });
  }

  async join(uri: AtUri, note?: string): Promise<WriteTicket> {
    const me = this.me();
    const f = this.forumRec(uri);
    return this.write(false, () => {
      const a = this.actorRec(me);
      if (a.status === "none" && f.membership === "open") a.status = "newcomer";
      if (note !== undefined) a.membershipNote = note;
      a.joinedAtMs = this.tick();
      this.s.followed.set(uri, this.s.followed.get(uri));
      const rkey = this.fkOf(uri);
      return {
        uri: atUri(me, COLL.membership, rkey),
        cid: fakeCid(`membership:${me}:${uri}`),
        topics: [
          { kind: "forum", uri },
          { kind: "user", did: me },
        ],
      };
    });
  }

  async leave(uri: AtUri): Promise<WriteTicket> {
    const me = this.me();
    this.forumRec(uri);
    return this.write(false, () => {
      const a = this.actorRec(me);
      if (!AUTHORITY.has(a.status)) a.status = "none";
      this.tick();
      return {
        uri: atUri(me, COLL.membership, this.fkOf(uri)),
        cid: "",
        topics: [
          { kind: "forum", uri },
          { kind: "user", did: me },
        ],
      };
    });
  }

  async members(uri: AtUri, query: MembersQuery): Promise<MemberView[]> {
    this.forumRec(uri);
    return [...this.s.actors.values()]
      .filter((a) => a.status !== "none")
      .filter((a) => query.status === undefined || a.status === query.status)
      .filter((a) => !query.pending || a.status === "newcomer")
      .sort((x, y) => x.joinedAtMs - y.joinedAtMs || (x.view.did < y.view.did ? -1 : 1))
      .map((a) => {
        const m: MemberView = {
          actor: clone(a.view),
          status: a.status,
          granted: a.granted,
          joinedAtMs: a.joinedAtMs,
        };
        if (a.membershipNote !== undefined) m.membershipNote = a.membershipNote;
        return m;
      });
  }

  private grantWrite(uri: AtUri, dids: Did[], on: boolean): Promise<WriteTicket> {
    if (!this.isAuthority(uri)) throw err("blocked", "Only moderators can grant membership.");
    return Promise.resolve(
      this.write(false, () => {
        for (const d of dids) {
          const a = this.actorRec(d);
          a.granted = on;
          if (on && a.status === "newcomer") a.status = "member";
        }
        this.tick();
        return {
          uri: atUri(this.me(), COLL.modAction, `${this.fkOf(uri)}-${b32hash(dids.join(","), 16)}`),
          cid: fakeCid(`grant:${dids.join(",")}:${on}`),
          topics: [{ kind: "forum", uri }],
        };
      }),
    );
  }

  async grant(uri: AtUri, dids: Did[]): Promise<WriteTicket> {
    return this.grantWrite(uri, dids, true);
  }

  async revoke(uri: AtUri, dids: Did[]): Promise<WriteTicket> {
    return this.grantWrite(uri, dids, false);
  }

  async modsSet(uri: AtUri, change: ModsChange): Promise<WriteTicket> {
    const f = this.forumRec(uri);
    if (f.summary.owner !== this.me())
      throw err("blocked", "Only the owner can change moderators.");
    return this.write(false, () => {
      if (change.kind === "add") {
        f.moderators = f.moderators.filter((m) => m.did !== change.did);
        const m: ForumRec["moderators"][number] = {
          did: change.did,
          role: change.role,
          shown: true,
        };
        if (change.title !== undefined) m.title = change.title;
        f.moderators.push(m);
        this.actorRec(change.did).status = change.role;
      } else {
        f.moderators = f.moderators.filter((m) => m.did !== change.did);
        this.actorRec(change.did).status = "member";
      }
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: [{ kind: "forum", uri }] };
    });
  }

  // =============================================================================== §3.3

  private syncReport(forum: AtUri): SyncReport {
    return {
      dto: DTO_VERSION,
      forum,
      reposTotal: 9,
      reposComplete: 8,
      reposStale: 1,
      reposUnavailable: 0,
      requests: 27,
      bytes: 482_113,
      changed: 0,
      partial: true,
      degraded: false,
      errors: [
        {
          did: "did:web:heidi.example.com",
          host: "pds.heidi.example.com",
          code: "network",
          message: "PDS timed out; using cached records (stale).",
        },
      ],
    };
  }

  async syncForum(uri: AtUri, _opts: SyncOpts): Promise<SyncReport> {
    this.forumRec(uri);
    for (let done = 0; done <= 9; done += 3)
      this.emit({ kind: "syncProgress", forum: uri, done, total: 9 });
    return this.syncReport(uri);
  }

  async syncThread(postUri: AtUri, _opts: SyncOpts): Promise<SyncReport> {
    const p = this.postRec(postUri);
    return this.syncReport(p.forum);
  }

  async liveTail(): Promise<void> {}

  async outboxPump(): Promise<void> {}

  async backgroundVerify(): Promise<void> {}

  // =============================================================================== §3.4

  private listedPosts(forum: AtUri): PostRec[] {
    return [...this.s.posts.values()].filter(
      (p) => p.forum === forum && listed(this.lens(p.author, p.state)),
    );
  }

  async listPosts(q: ListPostsQuery): Promise<PostList> {
    const f = this.forumRec(q.forum);
    const { ranking } = f.policy;
    const now = this.now();
    let items = this.listedPosts(q.forum).filter((p) => {
      if (q.filter === "frontpage" && (p.tier === "personal" || p.kind === "quickTake"))
        return false;
      if (q.filter === "curated" && (p.tier !== "curated" || p.kind === "quickTake")) return false;
      if (q.tag !== undefined && !p.tags.includes(q.tag)) return false;
      if (q.kind !== undefined && p.kind !== q.kind) return false;
      if (q.author !== undefined && p.author !== q.author) return false;
      return true;
    });
    const summaries = new Map(items.map((p) => [p.uri, this.postSummary(p)]));
    const scoreOf = (p: PostRec): number => summaries.get(p.uri)?.score.score ?? 0;
    const bonus = (p: PostRec): number =>
      scoreOf(p) +
      (p.tier === "curated" ? ranking.curatedBonus : 0) +
      (p.tier !== "personal" ? ranking.frontpageBonus : 0);
    const activity = (p: PostRec): number =>
      Math.max(p.tRankMs, summaries.get(p.uri)?.lastCommentAtMs ?? 0);
    if (q.sort === "magic") {
      items = items.filter((p) => p.pinned || now - p.tRankMs <= ranking.cutoffDays * 86_400_000);
    }
    items.sort((a, b) => {
      let d = 0;
      switch (q.sort) {
        case "top":
          d = scoreOf(b) - scoreOf(a) || b.tRankMs - a.tRankMs;
          break;
        case "new":
          d = b.tRankMs - a.tRankMs;
          break;
        case "old":
          d = a.tRankMs - b.tRankMs;
          break;
        case "recentComments":
          d = activity(b) - activity(a);
          break;
        default: {
          if (a.pinned !== b.pinned) {
            d = a.pinned ? -1 : 1;
            break;
          }
          const ba = bonus(a);
          const bb = bonus(b);
          if (ba > 0 !== bb > 0) d = ba > 0 ? -1 : 1;
          else if (ba > 0) {
            d =
              hot(bb, b.tRankMs, now, ranking.offsetHours, ranking.exponentMilli) -
              hot(ba, a.tRankMs, now, ranking.offsetHours, ranking.exponentMilli);
          } else d = bb - ba || b.tRankMs - a.tRankMs;
        }
      }
      if (d !== 0) return d < 0 ? -1 : 1;
      return a.uri < b.uri ? -1 : a.uri > b.uri ? 1 : 0;
    });
    const offset = q.cursor !== undefined ? Number(q.cursor) || 0 : 0;
    const page = items.slice(offset, offset + q.limit);
    const list: PostList = {
      dto: DTO_VERSION,
      forum: q.forum,
      sort: q.sort,
      filter: q.filter,
      items: page.map((p) => summaries.get(p.uri) ?? this.postSummary(p)),
      completeness: this.completeness(),
    };
    if (offset + q.limit < items.length) list.cursor = String(offset + q.limit);
    return list;
  }

  async recentDiscussion(
    forum: AtUri,
    cursor: string | undefined,
    limit: number,
  ): Promise<DiscussionList> {
    this.forumRec(forum);
    const entries = this.listedPosts(forum)
      .map((p) => {
        const visible = this.thread(p.uri, "new").filter(
          (c) => !c.missing && c.state.kind === "visible",
        );
        return { p, visible };
      })
      .filter((e) => e.visible.length > 0)
      .sort((a, b) => {
        const ta = Math.max(...a.visible.map((c) => c.tRankMs));
        const tb = Math.max(...b.visible.map((c) => c.tRankMs));
        return tb - ta || (a.p.uri < b.p.uri ? -1 : 1);
      });
    const offset = cursor !== undefined ? Number(cursor) || 0 : 0;
    const page = entries.slice(offset, offset + limit);
    const out: DiscussionList = {
      dto: DTO_VERSION,
      items: page.map((e) => ({
        post: this.postSummary(e.p),
        comments: [...e.visible]
          .sort((a, b) => b.tRankMs - a.tRankMs || (a.uri < b.uri ? -1 : 1))
          .slice(0, 3),
      })),
      completeness: this.completeness(),
    };
    if (offset + limit < entries.length) out.cursor = String(offset + limit);
    return out;
  }

  async getPost(uri: AtUri): Promise<PostView> {
    return this.postView(this.postRec(uri));
  }

  async getThread(uri: AtUri, opts: ThreadOpts): Promise<ThreadView> {
    const p = this.s.posts.get(uri);
    let comments = this.thread(uri, opts.sort);
    if (opts.unreadOnly) {
      const keep = new Set<string>();
      const byUri = new Map(comments.map((c) => [c.uri, c]));
      for (const c of comments) {
        if (!c.unread) continue;
        let cur: CommentNode | undefined = c;
        while (cur && !keep.has(cur.uri)) {
          keep.add(cur.uri);
          cur = cur.parent ? byUri.get(cur.parent) : undefined;
        }
      }
      comments = comments.filter((c) => keep.has(c.uri));
    }
    if (!p && comments.length === 0) throw err("not_found", `No post ${uri}`);
    return {
      dto: DTO_VERSION,
      post: p ? this.postView(p) : { missing: uri },
      comments,
      sort: opts.sort,
      completeness: this.completeness(),
    };
  }

  async history(uri: AtUri): Promise<HistoryView> {
    const p = this.s.posts.get(uri);
    if (p) {
      const versions: HistoryView["versions"] = clone(p.revisions);
      versions.push({
        n: p.revisions.length + 1,
        cid: p.cid,
        createdAtMs:
          p.revisions.length > 0 ? p.tRankMs + (p.revisions.length + 1) * 3_600_000 : p.createdAtMs,
        available: true,
      });
      return { dto: DTO_VERSION, uri, versions };
    }
    const c = this.s.comments.get(uri);
    if (c) {
      return {
        dto: DTO_VERSION,
        uri,
        versions: [{ n: 1, cid: c.cid, createdAtMs: c.createdAtMs, available: true }],
      };
    }
    throw err("not_found", `No record ${uri}`);
  }

  private tagMap(forum: AtUri): Map<Slug, TagRec> {
    const m = this.s.tags.get(forum);
    if (!m) throw err("not_found", `No forum ${forum}`);
    return m;
  }

  private tagSummary(forum: AtUri, t: TagRec): TagSummary {
    const s: TagSummary = {
      slug: t.slug,
      name: t.name,
      postCount: this.listedPosts(forum).filter((p) => p.tags.includes(t.slug)).length,
      unmergedEdits: t.unmergedEdits,
    };
    if (t.description !== undefined) s.description = t.description;
    return s;
  }

  async tags(forum: AtUri): Promise<TagSummary[]> {
    return [...this.tagMap(forum).values()]
      .map((t) => this.tagSummary(forum, t))
      .sort((a, b) => b.postCount - a.postCount || (a.slug < b.slug ? -1 : 1));
  }

  async tag(forum: AtUri, slug: Slug): Promise<TagView> {
    const t = this.tagMap(forum).get(slug);
    if (!t) throw err("not_found", `No tag ${slug}`);
    const posts = this.listedPosts(forum)
      .filter((p) => p.tags.includes(slug))
      .map((p) => this.postSummary(p))
      .sort((a, b) => b.score.score - a.score.score || (a.uri < b.uri ? -1 : 1));
    const view: TagView = { dto: DTO_VERSION, tag: this.tagSummary(forum, t), posts };
    if (t.page !== undefined) view.page = renderMarkdown(t.page).body;
    if (t.head !== undefined) view.head = t.head;
    return view;
  }

  private sequenceSummary(seq: SequenceRec): SequenceSummary {
    return {
      uri: seq.uri,
      author: this.actor(seq.author),
      title: seq.title,
      chapters: seq.chapters.length,
      posts: seq.chapters.reduce((n, c) => n + c.posts.length, 0),
      score: this.scoreView(seq.uri, seq.base),
      pinned: seq.pinned,
    };
  }

  async sequences(forum: AtUri): Promise<SequenceSummary[]> {
    this.forumRec(forum);
    return [...this.s.sequences.values()]
      .map((s) => this.sequenceSummary(s))
      .sort(
        (a, b) =>
          Number(b.pinned) - Number(a.pinned) ||
          b.score.score - a.score.score ||
          (a.uri < b.uri ? -1 : 1),
      );
  }

  async sequence(uri: AtUri): Promise<SequenceView> {
    const seq = this.s.sequences.get(uri);
    if (!seq) throw err("not_found", `No sequence ${uri}`);
    const view: SequenceView = {
      dto: DTO_VERSION,
      sequence: this.sequenceSummary(seq),
      chapters: seq.chapters.map((c) => {
        const ch: SequenceView["chapters"][number] = {
          posts: c.posts
            .map((u) => this.s.posts.get(u))
            .filter((p): p is PostRec => p !== undefined && listed(p.state))
            .map((p) => this.postSummary(p)),
        };
        if (c.title !== undefined) ch.title = c.title;
        if (c.body !== undefined) ch.body = renderMarkdown(c.body).body;
        return ch;
      }),
    };
    if (seq.body !== undefined) view.body = renderMarkdown(seq.body).body;
    return view;
  }

  async user(forum: AtUri | undefined, actor: string): Promise<UserView> {
    const did = this.resolveAuthority(actor);
    const a = this.s.actors.get(did);
    if (!a) throw err("not_found", `No user ${actor}`);
    const posts = [...this.s.posts.values()]
      .filter((p) => p.author === did && listed(p.state))
      .sort((x, y) => y.tRankMs - x.tRankMs);
    const comments = [...this.s.comments.values()]
      .filter((c) => c.author === did && listed(c.state))
      .sort((x, y) => y.tRankMs - x.tRankMs)
      .slice(0, 10)
      .map((c) => this.commentNode(c, 1, c.parent, this.lens(c.author, c.state)));
    const view: UserView = {
      dto: DTO_VERSION,
      actor: clone(a.view),
      status: a.status,
      links: a.links.map((l) => {
        const href = safeHref(l.uri, "field");
        const host = href?.kind === "external" || href?.kind === "mailto" ? href.host : "";
        const out: UserView["links"][number] = { uri: l.uri, host };
        if (l.label !== undefined) out.label = l.label;
        return out;
      }),
      pinned: a.pinned
        .map((u) => this.s.posts.get(u))
        .filter((p): p is PostRec => p !== undefined)
        .map((p) => this.postSummary(p)),
      recentPosts: posts.slice(0, 10).map((p) => this.postSummary(p)),
      recentComments: comments,
    };
    if (forum !== undefined) {
      view.forum = forum;
      view.karma = clone(a.karma);
    }
    if (a.bio !== undefined) view.bio = renderMarkdown(a.bio).body;
    return view;
  }

  async search(q: SearchQuery): Promise<SearchResults> {
    const needle = normalizeP(q.q).toLowerCase();
    if (needle === "") return { dto: DTO_VERSION, hits: [] };
    const hits: SearchHit[] = [];
    const snippetOf = (text: string): Omit<SearchHit, "item"> | null => {
      const lower = text.toLowerCase();
      const i = lower.indexOf(needle);
      if (i < 0) return null;
      const cps = Array.from(text);
      const start = Math.max(0, Array.from(text.slice(0, i)).length - 60);
      const end = Math.min(cps.length, start + 160);
      const snippet =
        (start > 0 ? "…" : "") + cps.slice(start, end).join("") + (end < cps.length ? "…" : "");
      const highlights: [number, number][] = [];
      const sl = snippet.toLowerCase();
      let from = 0;
      for (;;) {
        const j = sl.indexOf(needle, from);
        if (j < 0) break;
        const s0 = Array.from(snippet.slice(0, j)).length;
        highlights.push([s0, s0 + Array.from(needle).length]);
        from = j + needle.length;
      }
      return { snippet, highlights };
    };
    for (const p of this.s.posts.values()) {
      if (!listed(p.state)) continue;
      if (q.author !== undefined && p.author !== q.author) continue;
      const hit = snippetOf(
        `${p.kind === "quickTake" ? "" : (p.title ?? "")} ${p.body.text}`.trim(),
      );
      if (hit) hits.push({ ...hit, item: this.postSummary(p) });
    }
    for (const c of this.s.comments.values()) {
      if (c.state.kind !== "visible") continue;
      if (q.author !== undefined && c.author !== q.author) continue;
      const hit = snippetOf(c.body.text);
      if (hit) hits.push({ ...hit, item: this.commentNode(c, 1, c.parent, { kind: "visible" }) });
    }
    return { dto: DTO_VERSION, hits: hits.slice(0, q.limit) };
  }

  async notifications(q: NotificationsQuery): Promise<NotificationList> {
    const me = this.s.active;
    if (!me) return { dto: DTO_VERSION, items: [] };
    const items = (this.s.notifications.get(me) ?? [])
      .filter((n) => q.all || !n.read)
      .sort((a, b) => b.atMs - a.atMs || (a.id < b.id ? -1 : 1))
      .slice(0, q.limit)
      .map((n) => {
        const item: NotificationList["items"][number] = {
          id: n.id,
          reason: n.reason,
          atMs: n.atMs,
          subject: n.subject,
          read: n.read,
        };
        if (n.actor !== undefined) item.actor = this.actor(n.actor);
        if (n.preview !== undefined) item.preview = n.preview;
        return item;
      });
    return { dto: DTO_VERSION, items };
  }

  async modQueue(forum: AtUri): Promise<ModQueue> {
    this.forumRec(forum);
    return { dto: DTO_VERSION, items: clone(this.s.modQueue.get(forum) ?? []) };
  }

  async modLog(forum: AtUri, _cursor?: string): Promise<ModLog> {
    this.forumRec(forum);
    const entries = [...(this.s.modLog.get(forum) ?? [])]
      .sort((a, b) => b.atMs - a.atMs || (a.subject < b.subject ? -1 : 1))
      .map((e) => ({ ...clone(e), actor: this.actor(e.actor) }));
    return { dto: DTO_VERSION, entries };
  }

  async raw(uri: AtUri): Promise<RawView> {
    const p = this.s.posts.get(uri);
    const c = this.s.comments.get(uri);
    const seq = this.s.sequences.get(uri);
    const f = this.s.forums.get(uri);
    const md = (text: string): Json => ({ $type: COLL.markdown, text, profile: "lfm/1" });
    let record: Json | undefined;
    let cid: string | undefined;
    let state: ItemState | undefined;
    if (p) {
      const r: { [k: string]: Json } = {
        $type: COLL.post,
        forum: p.forum,
        kind: p.kind,
        body: md(p.markdown),
        tags: p.tags,
        createdAt: new Date(p.createdAtMs).toISOString(),
      };
      if (p.title !== undefined) r.title = p.title;
      if (p.link) r.link = { uri: p.link.uri, ...(p.link.title ? { title: p.link.title } : {}) };
      record = r;
      cid = p.cid;
      state = clone(p.state);
    } else if (c) {
      const root = this.s.posts.get(c.root);
      const r: { [k: string]: Json } = {
        $type: COLL.comment,
        forum: c.forum,
        root: { uri: c.root, cid: root?.cid ?? fakeCid(c.root) },
        body: md(c.markdown),
        createdAt: new Date(c.createdAtMs).toISOString(),
      };
      if (c.parent !== undefined) {
        r.parent = { uri: c.parent, cid: this.s.comments.get(c.parent)?.cid ?? fakeCid(c.parent) };
      }
      if (c.retracted) r.retracted = true;
      if (c.moderatorHat) r.moderatorHat = true;
      if (c.quote) r.quote = clone(c.quote) as unknown as Json;
      record = r;
      cid = c.cid;
      state = clone(c.state);
    } else if (seq) {
      record = {
        $type: COLL.sequence,
        forum: FORUM_URI,
        title: seq.title,
        chapters: seq.chapters.map((ch) => ({
          ...(ch.title ? { title: ch.title } : {}),
          posts: ch.posts,
        })),
        createdAt: new Date(seq.createdAtMs).toISOString(),
      };
      cid = fakeCid(seq.uri);
    } else if (f) {
      record = {
        $type: COLL.forum,
        name: f.summary.name,
        membership: f.membership,
        defaultTier: f.defaultTier,
        featuredTags: f.featuredTags,
        moderators: f.moderators.map((m) => ({ did: m.did, role: m.role })),
      };
      cid = fakeCid(uri);
    } else {
      throw err("not_found", `No record ${uri}`);
    }
    const view: RawView = {
      dto: DTO_VERSION,
      uri,
      level: "pds",
      record,
      validity: [],
      assertions: (this.s.modLog.get(FORUM_URI) ?? [])
        .filter((e) => e.subject === uri)
        .map((e) => ({
          uri: atUri(e.actor, COLL.modAction, `${FK}-${b32hash(`${e.action}:${e.subject}`, 16)}`),
          actor: e.actor,
          action: e.action,
          value: e.value,
          status: e.lapsed ? "lapsed" : "effective",
        })),
    };
    if (cid !== undefined) view.cid = cid;
    if (state !== undefined) view.state = state;
    return view;
  }

  async drafts(): Promise<DraftEntry[]> {
    return [...this.s.drafts.values()]
      .sort((a, b) => b.savedAtMs - a.savedAtMs || (a.id < b.id ? -1 : 1))
      .map((d) => clone(d));
  }

  async bookmarks(): Promise<BookmarkEntry[]> {
    return [...this.s.bookmarks.values()]
      .sort((a, b) => b.savedAtMs - a.savedAtMs)
      .map((b) => {
        const p = this.s.posts.get(b.uri);
        return p ? { ...clone(b), post: this.postSummary(p) } : clone(b);
      });
  }

  async subscriptions(): Promise<SubscriptionEntry[]> {
    return clone(this.s.subscriptions);
  }

  // =============================================================================== §3.5

  async publish(draft: PostDraft): Promise<WriteTicket> {
    const me = this.me();
    const f = this.forumRec(draft.forum);
    const title = draft.title?.trim() ?? "";
    if (draft.kind !== "quickTake" && title === "")
      throw err("invalid_input", "A title is required (PO3).");
    if (draft.kind === "quickTake" && draft.body.markdown.trim() === "") {
      throw err("invalid_input", "A quick take needs a body (PO4).");
    }
    if (draft.kind === "link" && !draft.link?.uri)
      throw err("invalid_input", "A link post needs a link (PO5).");
    return this.write(false, () => {
      const t = this.tick();
      const uri = atUri(me, COLL.post, `${this.fkOf(draft.forum)}-${tid(t, this.s.nextId % 1024)}`);
      const rec: PostRec = {
        uri,
        cid: fakeCid(`${uri}#v0`),
        forum: draft.forum,
        author: me,
        kind: draft.kind,
        markdown: draft.body.markdown,
        body: renderMarkdown(draft.body.markdown).body,
        tags: [...draft.tags],
        tier: f.defaultTier,
        pinned: false,
        locked: false,
        createdAtMs: t,
        tRankMs: t,
        base: { score: 1, agreement: 0, unweighted: 0, earlier: 0 },
        reacts: [],
        state: { kind: "visible" },
        badges: {},
        labels: [...draft.labels],
        revisions: [],
        redacted: false,
      };
      if (draft.kind !== "quickTake") rec.title = title;
      if (draft.link) {
        const href = safeHref(draft.link.uri, "field");
        const link: NonNullable<PostRec["link"]> = {
          uri: draft.link.uri,
          host: href?.kind === "external" ? href.host : "",
        };
        if (draft.link.title !== undefined) link.title = draft.link.title;
        if (draft.link.description !== undefined) link.description = draft.link.description;
        rec.link = link;
      }
      this.s.posts.set(uri, rec);
      const a = this.actorRec(me);
      if (a.status === "none") a.status = "newcomer";
      return {
        uri,
        cid: rec.cid,
        topics: [
          { kind: "forum", uri: draft.forum },
          { kind: "user", did: me },
        ],
      };
    });
  }

  async comment(draft: CommentDraft): Promise<WriteTicket> {
    const me = this.me();
    const root = this.postRec(draft.root);
    const forum = root.forum;
    if (root.locked && !this.isAuthority(forum)) {
      throw err("blocked", "This post is locked; new comments would be hidden.");
    }
    if (draft.parent !== undefined && !this.s.comments.has(draft.parent)) {
      throw err("invalid_ref", "The parent comment does not exist.");
    }
    if (draft.body.markdown.trim() === "") throw err("invalid_input", "A comment needs a body.");
    return this.write(false, () => {
      const t = this.tick();
      const rootTid = root.uri.split("/").pop()?.slice(13) ?? "";
      const uri = atUri(
        me,
        COLL.comment,
        `${this.fkOf(forum)}-${rootTid}-${tid(t, this.s.nextId % 1024)}`,
      );
      const rec: CommentRec = {
        uri,
        cid: fakeCid(uri),
        forum,
        root: root.uri,
        author: me,
        markdown: draft.body.markdown,
        body: renderMarkdown(draft.body.markdown).body,
        createdAtMs: t,
        tRankMs: t,
        base: { score: 1, agreement: 0, unweighted: 0, earlier: 0 },
        reacts: [],
        state: { kind: "visible" },
        badges: {},
        retracted: false,
        moderatorHat: false,
        orphaned: false,
        pinned: false,
      };
      if (draft.parent !== undefined) rec.parent = draft.parent;
      if (draft.quote) {
        rec.quote = clone(draft.quote);
        const target = draft.parent ? this.s.comments.get(draft.parent)?.body.text : root.body.text;
        rec.quoteAnchor = cpIndexOf(target ?? "", normalizeP(draft.quote.exact));
      }
      this.s.comments.set(uri, rec);
      const a = this.actorRec(me);
      if (a.status === "none") a.status = "newcomer";
      return { uri, cid: rec.cid, topics: this.threadTopics(root.uri) };
    });
  }

  async edit(uri: AtUri, draft: Draft, opts: EditOpts): Promise<WriteTicket> {
    const me = this.me();
    const p = this.s.posts.get(uri);
    const c = this.s.comments.get(uri);
    if (!p && !c) throw err("not_found", `No record ${uri}`);
    if ((p?.author ?? c?.author) !== me)
      throw err("blocked", "You can only edit your own records.");
    return this.write(false, () => {
      const t = this.tick();
      if (p && draft.kind === "post") {
        if (opts.history) {
          const v: PostRec["revisions"][number] = {
            n: p.revisions.length + 1,
            cid: p.cid,
            createdAtMs: t,
            available: true,
          };
          if (opts.summary !== undefined) v.summary = opts.summary;
          p.revisions.push(v);
        }
        p.markdown = draft.post.body.markdown;
        p.body = renderMarkdown(p.markdown).body;
        if (draft.post.title !== undefined && p.kind !== "quickTake") p.title = draft.post.title;
        p.tags = [...draft.post.tags];
        p.cid = fakeCid(`${uri}#t${t}`);
        p.badges = { ...p.badges, edited: true };
        return { uri, cid: p.cid, topics: this.threadTopics(uri) };
      }
      if (c && draft.kind === "comment") {
        c.markdown = draft.comment.body.markdown;
        c.body = renderMarkdown(c.markdown).body;
        // The edit replaces the whole record: the quote is whatever the draft carries.
        if (draft.comment.quote) {
          c.quote = clone(draft.comment.quote);
          const target = c.parent
            ? this.s.comments.get(c.parent)?.body.text
            : this.s.posts.get(c.root)?.body.text;
          c.quoteAnchor = cpIndexOf(target ?? "", normalizeP(draft.comment.quote.exact));
        } else {
          delete c.quote;
          delete c.quoteAnchor;
        }
        c.cid = fakeCid(`${uri}#t${t}`);
        c.badges = { ...c.badges, edited: true };
        return { uri, cid: c.cid, topics: this.threadTopics(c.root) };
      }
      throw err("invalid_input", "The draft kind does not match the record.");
    });
  }

  async delete(uri: AtUri): Promise<WriteTicket> {
    const me = this.me();
    const p = this.s.posts.get(uri);
    const c = this.s.comments.get(uri);
    if (!p && !c) throw err("not_found", `No record ${uri}`);
    if ((p?.author ?? c?.author) !== me)
      throw err("blocked", "You can only delete your own records.");
    return this.write(false, () => {
      this.tick();
      const root = p ? p.uri : (c?.root ?? "");
      const topics = this.threadTopics(root);
      this.s.posts.delete(uri);
      this.s.comments.delete(uri);
      return { uri, cid: "", topics };
    });
  }

  async retract(uri: AtUri): Promise<WriteTicket> {
    const me = this.me();
    const c = this.s.comments.get(uri);
    if (!c) throw err("not_found", `No comment ${uri}`);
    if (c.author !== me) throw err("blocked", "You can only retract your own comments.");
    return this.write(false, () => {
      c.retracted = true;
      c.badges = { ...c.badges, edited: true };
      c.cid = fakeCid(`${uri}#t${this.tick()}`);
      return { uri, cid: c.cid, topics: this.threadTopics(c.root) };
    });
  }

  async opinion(subjectUri: AtUri, intent: OpinionIntent): Promise<WriteTicket> {
    const me = this.me();
    const subject = this.subjectInfo(subjectUri);
    if (subject.author === me)
      throw err("invalid_input", "You cannot vote or react on your own items (O3).");
    if (intent.kind === "react" && !paletteReact(intent.id)) {
      throw err("invalid_input", `Unknown react ${intent.id}`);
    }
    if (intent.kind === "tag" && !this.s.posts.has(subjectUri)) {
      throw err("invalid_input", "Tag votes apply to posts only.");
    }
    const intentCopy = clone(intent);
    return this.write(true, () => {
      this.tick();
      let mine = this.s.opinions.get(me);
      if (!mine) {
        mine = new Map();
        this.s.opinions.set(me, mine);
      }
      const op: MyOpinionRec = mine.get(subjectUri) ?? { reacts: [] };
      switch (intentCopy.kind) {
        case "karma":
          if (intentCopy.value === undefined) delete op.karma;
          else op.karma = intentCopy.value;
          break;
        case "agreement":
          if (intentCopy.value === undefined) delete op.agreement;
          else op.agreement = intentCopy.value;
          break;
        case "react": {
          const k = qkey(intentCopy.quote);
          op.reacts = op.reacts.filter((r) => !(r.react === intentCopy.id && qkey(r.quote) === k));
          if (!intentCopy.remove) {
            const r: MyOpinionRec["reacts"][number] = {
              react: intentCopy.id,
              stance: intentCopy.stance,
            };
            if (intentCopy.quote) r.quote = intentCopy.quote;
            op.reacts.push(r);
          }
          break;
        }
        case "tag":
          break;
      }
      mine.set(subjectUri, op);
      const p = splitAtUri(subjectUri);
      const stid =
        p?.collection === COLL.post
          ? p.rkey.slice(13)
          : p?.collection === COLL.comment
            ? p.rkey.slice(13, 26)
            : "~";
      const uri = atUri(me, COLL.opinion, `${FK}-${stid}-${b32hash(`opinion:${subjectUri}`, 16)}`);
      return {
        uri,
        cid: fakeCid(`${uri}@${JSON.stringify(op)}`),
        topics: this.threadTopics(subject.root),
      };
    });
  }

  async modAction(intent: ModActionIntent): Promise<WriteTicket> {
    const me = this.me();
    const f = this.forumRec(intent.forum);
    const role = this.myRole(f);
    if (intent.scope === "forum" && !AUTHORITY.has(role)) {
      throw err("blocked", "Only moderators can take forum-scope moderation actions.");
    }
    if (
      intent.kind === "assert" &&
      intent.action.kind === "tier" &&
      intent.action.tier === "curated" &&
      role === "moderator"
    ) {
      throw err("blocked", "Only admins and the owner can curate.");
    }
    const rank = RANK[role] ?? 0;
    const copy = clone(intent);
    return this.write(false, () => {
      const t = this.tick();
      const subject = copy.subject;
      const post = this.s.posts.get(subject);
      const comment = this.s.comments.get(subject);
      const item = post ?? comment;
      let actionKey: string;
      let value: string;
      if (copy.kind === "assert") {
        const a = copy.action;
        actionKey = a.kind === "pin" ? `pin:${a.scope}` : a.kind;
        switch (a.kind) {
          case "hide":
            if (!item) throw err("not_found", `No item ${subject}`);
            if (a.hidden) {
              const reason: {
                kind: "removed";
                rank: number;
                actor?: Did;
                spam: boolean;
                reason?: string;
              } = {
                kind: "removed",
                rank,
                actor: me,
                spam: a.spam,
              };
              if (copy.reason !== undefined && a.showReason) reason.reason = copy.reason;
              item.state = { kind: "hidden", reason };
              value = a.spam ? "hidden (spam)" : "hidden";
            } else {
              item.state = { kind: "visible" };
              value = "keep";
            }
            break;
          case "access": {
            const target = this.actorRec(subject);
            if (!AUTHORITY.has(target.status)) target.status = "member";
            for (const x of [...this.s.posts.values(), ...this.s.comments.values()]) {
              if (x.author !== subject) continue;
              if (a.level === "banned" && listed(x.state))
                x.state = { kind: "hidden", reason: { kind: "banned" } };
              if (
                a.level === "member" &&
                x.state.kind === "hidden" &&
                x.state.reason.kind === "banned"
              ) {
                x.state = { kind: "visible" };
              }
            }
            value = a.level;
            break;
          }
          case "restrict": {
            for (const x of [...this.s.posts.values(), ...this.s.comments.values()]) {
              if (x.author !== subject || !listed(x.state)) continue;
              const cap = this.s.posts.has(x.uri) ? "post" : "comment";
              if (a.capabilities.includes(cap))
                x.state = { kind: "hidden", reason: { kind: "restricted" } };
            }
            value = a.capabilities.join(",") || "none";
            break;
          }
          case "tier":
            if (!post) throw err("invalid_input", "Tier applies to posts.");
            post.tier = a.tier;
            value = a.tier;
            break;
          case "pin":
            if (a.scope === "forum") {
              const seq = this.s.sequences.get(subject);
              if (post) post.pinned = a.pinned;
              else if (seq) seq.pinned = a.pinned;
              else throw err("invalid_input", "Forum pins apply to posts and sequences.");
            } else {
              if (!comment) throw err("invalid_input", "Thread pins apply to comments.");
              comment.pinned = a.pinned;
            }
            value = a.pinned ? "pinned" : "unpinned";
            break;
          case "lock":
            if (!post) throw err("invalid_input", "Locks apply to posts.");
            post.locked = a.locked;
            value = a.locked ? "locked" : "unlocked";
            break;
          case "tag": {
            const slug = subject.replace(/^tag:/, "");
            if (a.removed) this.tagMap(copy.forum).delete(slug);
            value = a.removed ? "removed" : "head";
            break;
          }
        }
      } else {
        actionKey = copy.actionKey;
        value = "removed";
        if (item && copy.actionKey === "hide") item.state = { kind: "visible" };
        if (post && copy.actionKey === "pin:forum") post.pinned = false;
        if (comment && copy.actionKey === "pin:thread") comment.pinned = false;
        if (post && copy.actionKey === "lock") post.locked = false;
        if (copy.actionKey === "access") {
          for (const x of [...this.s.posts.values(), ...this.s.comments.values()]) {
            if (
              x.author === subject &&
              x.state.kind === "hidden" &&
              x.state.reason.kind === "banned"
            ) {
              x.state = { kind: "visible" };
            }
          }
        }
      }
      const log = this.s.modLog.get(copy.forum) ?? [];
      const entry: (typeof log)[number] = {
        actor: me,
        action: actionKey,
        subject,
        value,
        atMs: t,
        lapsed: false,
      };
      if (copy.kind === "assert" && copy.reason !== undefined) entry.reason = copy.reason;
      log.push(entry);
      this.s.modLog.set(copy.forum, log);
      const uri = atUri(
        me,
        COLL.modAction,
        `${this.fkOf(copy.forum)}-${b32hash(`modAction:${copy.scope}:${subject}:${actionKey}`, 16)}`,
      );
      const topics: Topic[] = [{ kind: "forum", uri: copy.forum }];
      if (post) topics.push({ kind: "thread", uri: post.uri });
      if (comment) topics.push({ kind: "thread", uri: comment.root });
      if (!item && isDid(subject)) {
        topics.push({ kind: "user", did: subject });
        for (const p of this.s.posts.values()) topics.push({ kind: "thread", uri: p.uri });
      }
      return { uri, cid: fakeCid(`${uri}@${t}`), topics };
    });
  }

  async modAdopt(forum: AtUri, formerMod: Did): Promise<WriteTicket> {
    if (!this.isAuthority(forum)) throw err("blocked", "Only authorities can adopt assertions.");
    return this.write(false, () => {
      const t = this.tick();
      for (const e of this.s.modLog.get(forum) ?? []) {
        if (e.actor === formerMod) e.lapsed = true;
      }
      return {
        uri: atUri(
          this.me(),
          COLL.modAction,
          `${this.fkOf(forum)}-${b32hash(`adopt:${formerMod}`, 16)}`,
        ),
        cid: fakeCid(`adopt:${formerMod}@${t}`),
        topics: [{ kind: "forum", uri: forum }],
      };
    });
  }

  async wikiEdit(forum: AtUri, slug: Slug, draft: WikiDraft): Promise<WriteTicket> {
    const me = this.me();
    const tags = this.tagMap(forum);
    return this.write(false, () => {
      const t = this.tick();
      const uri = atUri(
        me,
        COLL.wikiRevision,
        `${this.fkOf(forum)}-${tid(t, this.s.nextId % 1024)}`,
      );
      const prev = tags.get(slug);
      const rec: TagRec = {
        slug,
        name: draft.name,
        unmergedEdits: prev?.unmergedEdits ?? 0,
        head: uri,
      };
      if (draft.description !== undefined) rec.description = draft.description;
      if (draft.body) rec.page = draft.body.markdown;
      tags.set(slug, rec);
      return { uri, cid: fakeCid(uri), topics: [{ kind: "forum", uri: forum }] };
    });
  }

  private seqRec(uri: AtUri): SequenceRec {
    const s = this.s.sequences.get(uri);
    if (!s) throw err("not_found", `No sequence ${uri}`);
    if (s.author !== this.me()) throw err("blocked", "You can only edit your own sequences.");
    return s;
  }

  private seqTopics(): Topic[] {
    return [{ kind: "forum", uri: FORUM_URI }];
  }

  async seqCreate(draft: SequenceDraft): Promise<WriteTicket> {
    const me = this.me();
    this.forumRec(draft.forum);
    return this.write(false, () => {
      const t = this.tick();
      const uri = atUri(
        me,
        COLL.sequence,
        `${this.fkOf(draft.forum)}-${tid(t, this.s.nextId % 1024)}`,
      );
      const rec: SequenceRec = {
        uri,
        author: me,
        title: draft.title,
        chapters: draft.chapters.map((c) => {
          const ch: SequenceRec["chapters"][number] = { posts: [...c.posts] };
          if (c.title !== undefined) ch.title = c.title;
          if (c.body) ch.body = c.body.markdown;
          return ch;
        }),
        base: { score: 0, agreement: 0, unweighted: 0, earlier: 0 },
        pinned: false,
        createdAtMs: t,
      };
      if (draft.body) rec.body = draft.body.markdown;
      this.s.sequences.set(uri, rec);
      return { uri, cid: fakeCid(uri), topics: this.seqTopics() };
    });
  }

  async seqEdit(uri: AtUri, draft: SequenceDraft): Promise<WriteTicket> {
    const s = this.seqRec(uri);
    return this.write(false, () => {
      s.title = draft.title;
      if (draft.body) s.body = draft.body.markdown;
      s.chapters = draft.chapters.map((c) => {
        const ch: SequenceRec["chapters"][number] = { posts: [...c.posts] };
        if (c.title !== undefined) ch.title = c.title;
        if (c.body) ch.body = c.body.markdown;
        return ch;
      });
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: this.seqTopics() };
    });
  }

  async seqAdd(uri: AtUri, post: AtUri, at: SequencePosition): Promise<WriteTicket> {
    const s = this.seqRec(uri);
    return this.write(false, () => {
      const ch = s.chapters[at.chapter];
      if (!ch) throw err("invalid_input", "No such chapter.");
      ch.posts.splice(at.index, 0, post);
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: this.seqTopics() };
    });
  }

  async seqRemove(uri: AtUri, post: AtUri): Promise<WriteTicket> {
    const s = this.seqRec(uri);
    return this.write(false, () => {
      for (const ch of s.chapters) ch.posts = ch.posts.filter((p) => p !== post);
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: this.seqTopics() };
    });
  }

  async seqMove(uri: AtUri, from: SequencePosition, to: SequencePosition): Promise<WriteTicket> {
    const s = this.seqRec(uri);
    return this.write(false, () => {
      const src = s.chapters[from.chapter];
      const dst = s.chapters[to.chapter];
      const [moved] = src ? src.posts.splice(from.index, 1) : [];
      if (!dst || moved === undefined) throw err("invalid_input", "No such position.");
      dst.posts.splice(to.index, 0, moved);
      return { uri, cid: fakeCid(`${uri}@${this.tick()}`), topics: this.seqTopics() };
    });
  }

  async redactRevision(uri: AtUri): Promise<WriteTicket> {
    const p = this.postRec(uri);
    if (p.author !== this.me()) throw err("blocked", "You can only redact your own history.");
    return this.write(false, () => {
      p.revisions = p.revisions.map((r) => ({ ...r, available: false }));
      p.redacted = true;
      return { uri, cid: p.cid, topics: this.threadTopics(uri) };
    });
  }

  async prepareMedia(input: MediaInput): Promise<MediaRef> {
    const b = input.bytes;
    let mime = input.mime ?? "application/octet-stream";
    if (b[0] === 0x89 && b[1] === 0x50) mime = "image/png";
    else if (b[0] === 0xff && b[1] === 0xd8) mime = "image/jpeg";
    let digest = `${b.length}:`;
    for (let i = 0; i < b.length; i += Math.max(1, Math.floor(b.length / 64))) digest += b[i];
    const ref: MediaRef = {
      id: this.nextId("media"),
      cid: fakeBlobCid(digest),
      mime,
      sizeBytes: b.length,
    };
    if (input.alt !== undefined) ref.alt = input.alt;
    if (input.name !== undefined) ref.name = input.name;
    return ref;
  }

  async undo(writeId: string): Promise<WriteTicket> {
    const rec = this.writes.get(writeId);
    if (!rec) throw err("not_found", `No write ${writeId}`);
    if (!rec.undoable || rec.state.kind !== "pending") {
      throw err("conflict", "The undo window for this write has passed.");
    }
    if (rec.timer !== undefined) clearTimeout(rec.timer);
    rec.state = { kind: "undone" };
    this.emit({ kind: "write", id: writeId, state: { kind: "undone" } });
    this.changed({ kind: "outbox" });
    return { id: writeId, state: { kind: "undone" } };
  }

  // =============================================================================== §3.6

  async markRead(postUri: AtUri): Promise<void> {
    const times = [...this.s.comments.values()]
      .filter((c) => c.root === postUri)
      .map((c) => c.tRankMs);
    this.s.lastVisited.set(postUri, Math.max(this.now(), ...times));
    const me = this.s.active;
    let touched = false;
    if (me) {
      for (const n of this.s.notifications.get(me) ?? []) {
        const c = this.s.comments.get(n.subject);
        if (!n.read && (n.subject === postUri || c?.root === postUri)) {
          n.read = true;
          touched = true;
        }
      }
    }
    const topics: Topic[] = [
      { kind: "thread", uri: postUri },
      { kind: "forum", uri: FORUM_URI },
    ];
    if (touched) topics.push({ kind: "notifications" });
    this.changed(...topics);
  }

  async saveDraft(draft: Draft): Promise<string> {
    const id = draft.id ?? this.nextId("draft");
    const entry: DraftEntry = { id, savedAtMs: this.tick(), draft: { ...clone(draft), id } };
    this.s.drafts.set(id, entry);
    this.changed({ kind: "drafts" });
    return id;
  }

  async deleteDraft(id: string): Promise<void> {
    this.s.drafts.delete(id);
    this.changed({ kind: "drafts" });
  }

  async bookmark(uri: AtUri, on: boolean): Promise<void> {
    if (on) this.s.bookmarks.set(uri, { uri, savedAtMs: this.tick() });
    else this.s.bookmarks.delete(uri);
    this.changed({ kind: "thread", uri });
  }

  async setSubscription(subscription: Subscription, on: boolean): Promise<void> {
    const key = JSON.stringify(subscription);
    this.s.subscriptions = this.s.subscriptions.filter(
      (x) => JSON.stringify(x.subscription) !== key,
    );
    if (on) this.s.subscriptions.push({ subscription: clone(subscription), sinceMs: this.tick() });
    this.changed({ kind: "notifications" });
  }

  async mute(did: Did, on: boolean): Promise<void> {
    if (on) this.s.mutes.add(did);
    else this.s.mutes.delete(did);
    const topics: Topic[] = [
      { kind: "forum", uri: FORUM_URI },
      { kind: "user", did },
    ];
    for (const p of this.s.posts.values()) topics.push({ kind: "thread", uri: p.uri });
    this.changed(...topics);
  }

  async settings(): Promise<Settings> {
    return clone(this.s.settings);
  }

  async setSetting<K extends SettingKey>(key: K, value: Settings[K]): Promise<void> {
    this.s.settings[key] = clone(value);
  }

  async exportLocal(): Promise<Json> {
    return {
      format: "lessdugin-local/1",
      followed: [...this.s.followed].map(([uri, alias]) =>
        alias === undefined ? { uri } : { uri, alias },
      ),
      lastVisited: Object.fromEntries(this.s.lastVisited),
      drafts: clone([...this.s.drafts.values()]) as unknown as Json,
      bookmarks: clone([...this.s.bookmarks.values()]) as unknown as Json,
      subscriptions: clone(this.s.subscriptions) as unknown as Json,
      mutes: [...this.s.mutes],
      settings: clone(this.s.settings) as unknown as Json,
    };
  }

  async importLocal(json: Json): Promise<void> {
    if (
      typeof json !== "object" ||
      json === null ||
      Array.isArray(json) ||
      json.format !== "lessdugin-local/1"
    ) {
      throw err("invalid_input", "Not a lessdugin local-state export.");
    }
    const j = json as unknown as {
      followed?: { uri: string; alias?: string }[];
      lastVisited?: Record<string, number>;
      drafts?: DraftEntry[];
      bookmarks?: BookmarkEntry[];
      subscriptions?: SubscriptionEntry[];
      mutes?: string[];
      settings?: Settings;
    };
    if (j.followed) this.s.followed = new Map(j.followed.map((f) => [f.uri, f.alias]));
    if (j.lastVisited) this.s.lastVisited = new Map(Object.entries(j.lastVisited));
    if (j.drafts) this.s.drafts = new Map(j.drafts.map((d) => [d.id, d]));
    if (j.bookmarks) this.s.bookmarks = new Map(j.bookmarks.map((b) => [b.uri, b]));
    if (j.subscriptions) this.s.subscriptions = j.subscriptions;
    if (j.mutes) this.s.mutes = new Set(j.mutes);
    if (j.settings) this.s.settings = { ...this.s.settings, ...j.settings };
    this.changed({ kind: "drafts" }, { kind: "notifications" }, { kind: "forum", uri: FORUM_URI });
  }

  // =============================================================================== §3.7

  async render(markdown: string, media: MediaRef[], _opts: RenderOpts): Promise<RenderedBody> {
    const body = renderMarkdown(markdown, [RENDER_NOTICE]).body;
    const me = this.s.active ?? "did:plc:unknown";
    body.media = media.map((m, index) => {
      const v: RenderedBody["media"][number] = {
        index,
        cid: m.cid,
        mime: m.mime,
        author: me,
        pds: "https://pds.example.test",
        autoload: false,
        sizeBytes: m.sizeBytes,
      };
      if (m.alt !== undefined) v.alt = m.alt;
      if (m.name !== undefined) v.name = m.name;
      return v;
    });
    return body;
  }

  async safeHref(raw: string, ctx: HrefContext): Promise<Href | null> {
    return safeHref(raw, ctx);
  }

  async validateRecord(collection: string, json: Json): Promise<ValidationReport> {
    if (typeof json !== "object" || json === null || Array.isArray(json)) {
      return { valid: false, errors: ["G1"] };
    }
    if (json.$type !== collection) return { valid: false, errors: ["G1"] };
    return { valid: true, errors: [] };
  }

  async deriveRkey(input: RkeyInput): Promise<string> {
    switch (input.kind) {
      case "membership":
        return this.fkOf(input.forum);
      case "post":
        return `${this.fkOf(input.forum)}-${input.tid}`;
      case "comment": {
        const rkey = splitAtUri(input.root)?.rkey;
        if (!rkey) throw err("invalid_input", "Invalid root.");
        return `${rkey}-${input.tid}`;
      }
      case "opinion": {
        const p = splitAtUri(input.subject);
        const stid =
          p?.collection === COLL.post
            ? p.rkey.slice(13)
            : p?.collection === COLL.comment
              ? p.rkey.slice(13, 26)
              : "~";
        return `${this.fkOf(input.forum)}-${stid}-${b32hash(`opinion:${input.subject}`, 16)}`;
      }
      case "modAction":
        return `${this.fkOf(input.forum)}-${b32hash(`modAction:${input.scope}:${input.subject}:${input.actionKey}`, 16)}`;
      case "revision": {
        const p = splitAtUri(input.subject);
        if (!p) throw err("invalid_input", "Invalid subject.");
        return `${p.collection === COLL.forum ? this.fkOf(input.subject) : p.rkey}-${input.tid}`;
      }
    }
  }

  async verify(uri: AtUri, _deep: boolean): Promise<VerifyReport> {
    const exists = this.s.posts.has(uri) || this.s.comments.has(uri) || this.s.forums.has(uri);
    const rev = tid(this.now(), 1);
    const report: VerifyReport = {
      dto: DTO_VERSION,
      uri,
      result: exists ? "proven" : "absent",
      keySource: "plc (mock)",
      checks: [
        { name: "identity document", ok: true, detail: "resolved from the mock directory" },
        { name: "commit signature", ok: exists },
        { name: "MST path", ok: exists },
        { name: "record CID", ok: exists },
      ],
    };
    if (exists) {
      report.rev = rev;
      report.commitCid = fakeCid(`commit:${uri}`);
    }
    return report;
  }

  async runVector(kind: string, _json: Json): Promise<Json> {
    throw err("unsupported", `The mock engine has no ${kind} vectors.`);
  }

  // =============================================================================== §3.8

  subscribe(listener: EventListener): Unsubscribe {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}

/** Creates a mock engine with the default fixture. */
export function createMockEngine(opts: MockEngineOptions = {}): MockEngine {
  return new MockEngine(opts);
}
