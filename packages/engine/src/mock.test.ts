// SPDX-License-Identifier: AGPL-3.0-or-later
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FORUM_URI } from "./mock/fixture.ts";
import { MockEngine, mockSafeHref } from "./mock.ts";
import { type EngineEvent, isMissingRoot, OPINION_UNDO_WINDOW_MS } from "./types.ts";

const LFM_ROOT = '<div class="lfm" dir="auto">';

describe("MockEngine fixture", () => {
  it("is deterministic", async () => {
    const a = new MockEngine();
    const b = new MockEngine();
    const q = { forum: FORUM_URI, sort: "magic", filter: "all", limit: 50 } as const;
    expect(JSON.stringify(await a.listPosts(q))).toBe(JSON.stringify(await b.listPosts(q)));
    const post = (await a.listPosts(q)).items[0];
    expect(post).toBeDefined();
    const t1 = await a.getThread(post?.uri ?? "", { sort: "magic", unreadOnly: false });
    const t2 = await b.getThread(post?.uri ?? "", { sort: "magic", unreadOnly: false });
    expect(JSON.stringify(t1)).toBe(JSON.stringify(t2));
  });

  it("lists posts of every kind with the frontpage filter excluding quick takes", async () => {
    const e = new MockEngine();
    const all = await e.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 100 });
    const kinds = new Set(all.items.map((p) => p.kind));
    expect(kinds).toEqual(new Set(["article", "link", "question", "quickTake"]));
    expect(all.items.length).toBeGreaterThanOrEqual(12);
    const front = await e.listPosts({
      forum: FORUM_URI,
      sort: "magic",
      filter: "frontpage",
      limit: 100,
    });
    expect(front.items.some((p) => p.kind === "quickTake")).toBe(false);
    expect(front.items[0]?.pinned).toBe(true);
    for (const p of all.items) expect(["visible", "collapsed"]).toContain(p.state.kind);
  });

  it("builds threads in pre-order with every display state", async () => {
    const e = new MockEngine();
    const list = await e.listPosts({ forum: FORUM_URI, sort: "top", filter: "all", limit: 100 });
    const welcome = list.items.find((p) => p.title?.startsWith("Welcome"));
    const t = await e.getThread(welcome?.uri ?? "", { sort: "magic", unreadOnly: false });
    expect(isMissingRoot(t.post)).toBe(false);
    const states = new Set(
      t.comments.map((c) =>
        c.state.kind === "collapsed" ? `collapsed:${c.state.reason}` : c.state.kind,
      ),
    );
    expect(states).toContain("collapsed:newUser");
    expect(states).toContain("collapsed:lowScore");
    expect(states).toContain("tombstone");
    expect(t.comments.some((c) => c.missing)).toBe(true);
    expect(t.comments.some((c) => c.orphaned)).toBe(true);
    expect(t.comments.some((c) => c.retracted)).toBe(true);
    expect(t.comments.some((c) => c.moderatorHat)).toBe(true);
    expect(t.comments.some((c) => c.unread)).toBe(true);
    expect(Math.max(...t.comments.map((c) => c.depth))).toBeGreaterThan(6);
    // pre-order: every comment's parent appears before it, one level up
    const seen = new Map<string, number>();
    for (const c of t.comments) {
      if (c.parent !== undefined) expect(seen.get(c.parent)).toBe(c.depth - 1);
      else expect(c.depth === 1 || c.missing === false).toBe(true);
      seen.set(c.uri, c.depth);
    }
    // the hidden comment without listed replies is not displayed at all
    expect(t.comments.some((c) => c.body.text.includes("you should not see this"))).toBe(false);
    expect(t.comments[0]?.pinned).toBe(true);
    for (const c of t.comments) expect(c.body.html.startsWith(LFM_ROOT)).toBe(true);
  });
});

describe("MockEngine writes", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  async function firstVotablePost(e: MockEngine) {
    const list = await e.listPosts({ forum: FORUM_URI, sort: "top", filter: "all", limit: 100 });
    const me = (await e.activeAccount())?.actor.did;
    const post = list.items.find((p) => p.author.did !== me && p.score.mine === undefined);
    if (!post) throw new Error("no votable post");
    return post;
  }

  it("holds opinions for the undo window, then applies them and announces the change", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const events: EngineEvent[] = [];
    e.subscribe((ev) => events.push(ev));
    const post = await firstVotablePost(e);
    const ticket = await e.opinion(post.uri, { kind: "karma", value: "up" });
    expect(ticket.state.kind).toBe("pending");
    await vi.advanceTimersByTimeAsync(OPINION_UNDO_WINDOW_MS - 100);
    expect((await e.getPost(post.uri)).post.score.score).toBe(post.score.score);
    await vi.advanceTimersByTimeAsync(200);
    const after = await e.getPost(post.uri);
    expect(after.post.score.score).toBe(post.score.score + 1);
    expect(after.post.score.mine?.karma).toBe("up");
    const states = events.filter((x) => x.kind === "write").map((x) => x.state.kind);
    expect(states).toEqual(["pending", "sending", "done"]);
    expect(
      events.some(
        (x) =>
          x.kind === "changed" && x.topics.some((t) => t.kind === "thread" && t.uri === post.uri),
      ),
    ).toBe(true);
  });

  it("undoes an opinion inside the window and refuses afterwards", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const post = await firstVotablePost(e);
    const ticket = await e.opinion(post.uri, { kind: "karma", value: "strongDown" });
    await vi.advanceTimersByTimeAsync(1000);
    expect((await e.undo(ticket.id)).state.kind).toBe("undone");
    await vi.advanceTimersByTimeAsync(OPINION_UNDO_WINDOW_MS * 2);
    expect((await e.getPost(post.uri)).post.score.score).toBe(post.score.score);
    const t2 = await e.opinion(post.uri, { kind: "karma", value: "up" });
    await vi.advanceTimersByTimeAsync(OPINION_UNDO_WINDOW_MS + 100);
    await expect(e.undo(t2.id)).rejects.toMatchObject({ code: "conflict" });
  });

  it("adds a reply to the thread and bumps the comment count", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const post = await firstVotablePost(e);
    const ticket = await e.comment({
      root: post.uri,
      body: { markdown: "A **new** reply", media: [] },
    });
    await vi.advanceTimersByTimeAsync(50);
    const t = await e.getThread(post.uri, { sort: "new", unreadOnly: false });
    const mine = t.comments.find((c) => c.body.text === "A new reply");
    expect(mine?.body.html).toContain("<strong>new</strong>");
    expect((await e.getPost(post.uri)).post.commentCount).toBe(post.commentCount + 1);
    expect(ticket.state.kind).toBe("pending");
  });
});

describe("mock safe_href", () => {
  it("accepts only the lfm-1 §5.1 shapes", () => {
    expect(mockSafeHref("https://Example.com/ä", "markdown")).toEqual({
      kind: "external",
      url: "https://example.com/%C3%A4",
      host: "example.com",
    });
    expect(mockSafeHref("javascript:alert(1)", "markdown")).toBeNull();
    expect(mockSafeHref("//evil.example", "markdown")).toBeNull();
    expect(mockSafeHref("https://user@evil.example", "markdown")).toBeNull();
    expect(mockSafeHref("https://evil.example\\@good", "markdown")).toBeNull();
    expect(mockSafeHref("#Intro", "markdown")).toEqual({ kind: "anchor", id: "intro" });
    expect(mockSafeHref("#intro", "field")).toBeNull();
    expect(mockSafeHref("at://alice.test", "markdown")).toBeNull();
    expect(mockSafeHref("at://alice.test", "field")).toEqual({
      kind: "profileHandle",
      handle: "alice.test",
    });
    expect(mockSafeHref("at://did:plc:abcdefghijklmnopqrstuvwx", "markdown")).toEqual({
      kind: "profile",
      did: "did:plc:abcdefghijklmnopqrstuvwx",
    });
  });
});
