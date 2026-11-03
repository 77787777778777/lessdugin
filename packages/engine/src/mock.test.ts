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

  it("discovers forums and finds listed posts linking to an item", async () => {
    const e = new MockEngine();
    const all = await e.forumDiscover("");
    expect(all.length).toBeGreaterThan(0);
    expect(await e.forumDiscover(all[0]?.name.toUpperCase() ?? "missing")).toEqual([all[0]]);
    expect(await e.forumDiscover("no forum has this name")).toEqual([]);

    const posts = await e.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 100 });
    const target = posts.items.find((p) => p.title === "Moderation policy");
    expect(target).toBeDefined();
    const linked = await e.backlinks(target?.uri ?? "");
    expect(linked.map((p) => p.uri)).toContain(
      posts.items.find((p) => p.title?.startsWith("Welcome"))?.uri,
    );
    expect(linked.every((p) => p.state.kind !== "hidden")).toBe(true);
    await expect(e.backlinks("not an at-uri")).rejects.toMatchObject({ code: "invalid_ref" });
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

  it("reports the viewer's caps, and omits them signed out (M3, social.md §8)", async () => {
    const e = new MockEngine();
    const view = await e.forumOpen(FORUM_URI);
    expect(view.viewer).toBeDefined();
    // The fixture's active account is an admin of the fixture forum.
    expect(view.viewer?.moderate).toBe(true);
    expect(view.viewer?.rank).toBe(2);
    expect(view.viewer?.curate).toBe(true);
    expect(view.viewer?.vote).toBe(true);
    expect(view.viewer?.weighted).toBe(true);
    expect(view.viewer?.post).toBe(true);
    expect(view.viewer?.comment).toBe(true);
    expect(view.viewer?.newcomer).toBe(false);
    expect(view.viewer?.applicant).toBe(false);
    // Signed out (the fixture's only account removed): no `viewer` at all.
    const me = (await e.activeAccount())?.did ?? "";
    await e.removeAccount(me, false);
    expect((await e.forumOpen(FORUM_URI)).viewer).toBeUndefined();
  });

  it("flags member state and refuses the mod queue to non-authorities (M3)", async () => {
    const e = new MockEngine();
    const members = await e.members(FORUM_URI, {});
    const frank = members.find((m) => m.actor.handle === "frank.test");
    expect(frank?.banned).toBe(true);
    const dan = members.find((m) => m.actor.displayName?.includes("Dan") === true);
    expect(dan?.status).toBe("newcomer");
    // The authority sees the queue; a signed-out viewer gets `blocked`.
    expect((await e.modQueue(FORUM_URI)).items.length).toBeGreaterThan(0);
    const me = (await e.activeAccount())?.did ?? "";
    await e.removeAccount(me, false);
    await expect(e.modQueue(FORUM_URI)).rejects.toMatchObject({ code: "blocked" });
    // The public mod log stays readable, with status on every entry.
    const log = await e.modLog(FORUM_URI);
    expect(log.entries.length).toBeGreaterThan(0);
    for (const en of log.entries) expect(en.status).toBeDefined();
  });

  it("carries M3 score fields and own-opinion markers (social.md §5–§7)", async () => {
    const e = new MockEngine();
    const list = await e.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 100 });
    const withEarlier = list.items.find((p) => p.score.earlierVersionVotes > 0);
    expect(withEarlier).toBeDefined();
    expect(withEarlier?.score.weightedVotes).toBeGreaterThan(0);
    // The fixture's active account upvoted the "confusion" post.
    const confusion = list.items.find((p) => p.title?.includes("confusion") ?? false);
    const own = confusion !== undefined ? await e.getPost(confusion.uri) : null;
    expect(own?.post.score.mine?.karma).toBe("up");
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
    const me = (await e.activeAccount())?.did;
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

  it("lists a post's applied tags, sorted, not its declared ones (social.md §13.3)", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const all = await e.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 100 });
    // declared in the order rationality, practical; both have a page
    const declaredOutOfOrder = all.items.find((p) => p.title === "Noticing confusion");
    expect(declaredOutOfOrder?.tags).toEqual(["practical", "rationality"]);
    const ticket = await e.publish({
      forum: FORUM_URI,
      kind: "article",
      title: "Tagged without a page",
      body: { markdown: "Body", media: [] },
      tags: ["no-such-page", "meta", "meta"],
    });
    await vi.advanceTimersByTimeAsync(50);
    const state = (await e.writes()).find((w) => w.id === ticket.id)?.state;
    if (state?.kind !== "done") throw new Error("the publish did not finish");
    // a tag without a wiki page never applies; the record keeps what the author declared
    expect((await e.getPost(state.uri)).post.tags).toEqual(["meta"]);
    expect((await e.raw(state.uri)).record).toMatchObject({
      tags: ["no-such-page", "meta", "meta"],
    });
    const byMissing = await e.listPosts({
      forum: FORUM_URI,
      sort: "new",
      filter: "all",
      tag: "no-such-page",
      limit: 100,
    });
    expect(byMissing.items).toEqual([]);
    const byMeta = await e.listPosts({
      forum: FORUM_URI,
      sort: "new",
      filter: "all",
      tag: "meta",
      limit: 100,
    });
    expect(byMeta.items.map((p) => p.uri)).toContain(state.uri);
  });

  it("writes a wiki revision on its base, and refuses one whose head moved (O-2)", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const slug = "decision-theory";
    const head = (await e.tag(FORUM_URI, slug)).head ?? "";
    // a page's revisions are records: the head names the genesis as its parent
    const genesis = (await e.raw(head)).record as { parents: { uri: string }[] };
    expect(genesis.parents).toHaveLength(1);
    expect((await e.raw(genesis.parents[0]?.uri ?? "")).record).toMatchObject({
      slug,
      parents: [],
    });
    const draft = { name: "Decision theory", isTag: true, base: head };
    await e.wikiEdit(FORUM_URI, slug, draft);
    await vi.advanceTimersByTimeAsync(50);
    const next = (await e.tag(FORUM_URI, slug)).head ?? "";
    expect(next).not.toBe(head);
    expect((await e.raw(next)).record).toMatchObject({ parents: [{ uri: head }] });
    await expect(e.wikiEdit(FORUM_URI, slug, draft)).rejects.toMatchObject({
      code: "conflict",
      detail: { kind: "wikiHeadMoved", base: head, head: next },
    });
  });

  it("with wikiRace, meets another participant's revision on the first edit with a base", async () => {
    const e = new MockEngine({ sendMs: 10, wikiRace: true });
    const slug = "decision-theory";
    const head = (await e.tag(FORUM_URI, slug)).head ?? "";
    const events: EngineEvent[] = [];
    e.subscribe((ev) => events.push(ev));
    const draft = { name: "Decision theory", isTag: true, base: head };
    await expect(e.wikiEdit(FORUM_URI, slug, draft)).rejects.toMatchObject({
      code: "conflict",
      detail: { kind: "wikiHeadMoved", base: head },
    });
    const moved = (await e.tag(FORUM_URI, slug)).head ?? "";
    expect(moved).not.toBe(head);
    const race = (await e.raw(moved)).record as { parents: { uri: string }[]; summary: string };
    expect(race.parents.map((p) => p.uri)).toEqual([head]);
    expect(moved.split("/")[2]).not.toBe((await e.activeAccount())?.did);
    await vi.advanceTimersByTimeAsync(0);
    expect(events).toContainEqual({ kind: "changed", topics: [{ kind: "forum", uri: FORUM_URI }] });
    // only once: the edit redone on the newer head is written
    await e.wikiEdit(FORUM_URI, slug, { ...draft, base: moved });
    await vi.advanceTimersByTimeAsync(50);
    const next = (await e.tag(FORUM_URI, slug)).head ?? "";
    expect((await e.raw(next)).record).toMatchObject({ parents: [{ uri: moved }] });
  });

  // views.rs `tag_summary`: a page that exists but is not a tag (applicable = false) is no tag
  it("treats a page that is not a tag as no tag, and edits it on its head", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const slug = "decision-theory";
    const head = (await e.tag(FORUM_URI, slug)).head ?? "";
    const notTag = { name: "Glossary", isTag: false, base: head };
    const t = await e.wikiEdit(FORUM_URI, slug, notTag);
    await vi.advanceTimersByTimeAsync(50);
    const s = (await e.writes()).find((w) => w.id === t.id)?.state;
    if (s?.kind !== "done") throw new Error("not written");
    await expect(e.tag(FORUM_URI, slug)).rejects.toMatchObject({ code: "not_found" });
    expect((await e.tags(FORUM_URI)).map((x) => x.slug)).not.toContain(slug);
    // an edit without a base still names the page's head, as organisation.rs `wiki_edit` does
    await e.wikiEdit(FORUM_URI, slug, { name: "Decision theory", isTag: true });
    await vi.advanceTimersByTimeAsync(50);
    const back = (await e.tag(FORUM_URI, slug)).head ?? "";
    expect((await e.raw(back)).record).toMatchObject({ parents: [{ uri: s.uri }] });
  });

  it("bounds sequence positions before enqueueing, as the engine does", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const posts = (await e.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 3 }))
      .items;
    const [a, b] = posts;
    if (!a || !b) throw new Error("no posts");
    const t = await e.seqCreate({
      forum: FORUM_URI,
      title: "Mine",
      chapters: [{ posts: [a.uri] }],
    });
    await vi.advanceTimersByTimeAsync(50);
    const s = (await e.writes()).find((w) => w.id === t.id)?.state;
    if (s?.kind !== "done") throw new Error("not created");
    await expect(e.seqAdd(s.uri, b.uri, { chapter: 0, index: 2 })).rejects.toMatchObject({
      code: "invalid_input",
    });
    await expect(e.seqAdd(s.uri, s.uri, { chapter: 0, index: 0 })).rejects.toMatchObject({
      code: "invalid_input",
    });
    await expect(
      e.seqMove(s.uri, { chapter: 0, index: 0 }, { chapter: 0, index: 1 }),
    ).rejects.toMatchObject({ code: "invalid_input" });
    await e.seqAdd(s.uri, b.uri, { chapter: 0, index: 1 });
    await vi.advanceTimersByTimeAsync(50);
    await e.seqMove(s.uri, { chapter: 0, index: 1 }, { chapter: 0, index: 0 });
    await vi.advanceTimersByTimeAsync(50);
    expect((await e.sequence(s.uri)).chapters[0]?.posts.map((p) => p.uri)).toEqual([b.uri, a.uri]);
  });

  it("keeps the #restrict dimension an intent leaves out, as the engine merges (m3r-2)", async () => {
    const e = new MockEngine({ sendMs: 10 });
    const target = (await e.members(FORUM_URI, {})).find(
      (m) => m.banned !== true && m.nullified !== true && m.restricted === undefined,
    )?.actor.did;
    if (target === undefined) throw new Error("no unmoderated member");
    const restrict = (action: { capabilities?: ("post" | "comment")[]; nullifyVotes?: boolean }) =>
      e.modAction({
        kind: "assert",
        forum: FORUM_URI,
        scope: "forum",
        subject: target,
        action: { kind: "restrict", ...action },
      });
    const state = async () => {
      const m = (await e.members(FORUM_URI, {})).find((x) => x.actor.did === target);
      return { restricted: m?.restricted, nullified: m?.nullified === true };
    };
    await restrict({ capabilities: ["post", "comment"] });
    await vi.advanceTimersByTimeAsync(50);
    await restrict({ nullifyVotes: true });
    await vi.advanceTimersByTimeAsync(50);
    expect(await state()).toEqual({ restricted: ["post", "comment"], nullified: true });
    await restrict({ capabilities: ["comment"] });
    await vi.advanceTimersByTimeAsync(50);
    expect(await state()).toEqual({ restricted: ["comment"], nullified: true });
    await expect(restrict({})).rejects.toMatchObject({ code: "invalid_input" });
  });

  it("adopts before removing a moderator, says so, and reports what it did not adopt (sec-8)", async () => {
    const e = new MockEngine({ sendMs: 10 });
    // the fixture's former moderator, whose forum pin has lapsed
    const lapsed = (await e.modLog(FORUM_URI)).entries.find((x) => x.lapsed);
    if (lapsed === undefined) throw new Error("no lapsed entry");
    const adoption = await e.modAdopt(FORUM_URI, lapsed.actor.did);
    await vi.advanceTimersByTimeAsync(50);
    expect(adoption.id).toBe(`${(await e.activeAccount())?.did}/${adoption.id.split("/").pop()}`);
    expect((await e.writeStatus(adoption.id))?.state).toMatchObject({
      kind: "done",
      report: [`cannot adopt ${lapsed.uri}: not in effect`],
    });
    // a removal adopts first: the ticket's notice names the adoption's write
    const ticket = await e.forumCreate({
      slug: "mods",
      name: "Mods",
      membership: "open",
      defaultTier: "frontpage",
      featuredTags: [],
      moderators: [],
    });
    await vi.advanceTimersByTimeAsync(50);
    const uri = (await e.followed()).find((f) => f.name === "Mods")?.uri ?? "";
    expect(ticket.state.kind).toBe("pending");
    const did = lapsed.actor.did;
    await e.modsSet(uri, { kind: "add", did, role: "moderator" });
    await vi.advanceTimersByTimeAsync(50);
    const removal = await e.modsSet(uri, { kind: "remove", did });
    expect(removal.notices?.map((n) => n.code)).toEqual(["adoptedFirst"]);
    expect(removal.notices?.[0]?.message).toMatch(
      /^mod adopt runs first \(write did:[^/]+\/\d+, 0 /,
    );
    expect(
      (await e.modsSet(uri, { kind: "remove", did, skipAdopt: true })).notices,
    ).toBeUndefined();
  });
});

describe("MockEngine body sources", () => {
  it("reads inline markdown, or the source behind a spilled record (engine-api.md §3.4)", async () => {
    const inline = new MockEngine();
    const post = (
      await inline.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 1 })
    ).items[0];
    if (post === undefined) throw new Error("no post in the fixture");
    const record = (await inline.raw(post.uri)).record as { body: { text: string } };
    await expect(inline.bodySource(post.uri)).resolves.toMatchObject({
      markdown: record.body.text,
      cid: (await inline.raw(post.uri)).cid,
      profile: "lfm/1",
      spilled: false,
    });

    const spilled = new MockEngine({ spilledBodies: true });
    const raw = (await spilled.raw(post.uri)).record as {
      body: { text?: string; textBlob?: unknown };
    };
    expect(raw.body.text).toBeUndefined();
    expect(raw.body.textBlob).toBeDefined();
    await expect(spilled.bodySource(post.uri)).resolves.toEqual({
      markdown: (await inline.bodySource(post.uri)).markdown,
      cid: (await spilled.raw(post.uri)).cid,
      profile: "lfm/1",
      spilled: true,
    });
  });

  it("does not call an absent forum about spilled", async () => {
    const e = new MockEngine({ sendMs: 1, spilledBodies: true });
    await e.forumCreate({
      slug: "without-about",
      name: "Without about",
      membership: "open",
      defaultTier: "frontpage",
      featuredTags: [],
      moderators: [],
    });
    await vi.waitFor(async () => {
      const uri = (await e.followed()).find((f) => f.name === "Without about")?.uri;
      expect(uri).toBeDefined();
    });
    const uri = (await e.followed()).find((f) => f.name === "Without about")?.uri ?? "";
    await expect(e.bodySource(uri)).resolves.toMatchObject({ markdown: "", spilled: false });
  });
});

describe("MockEngine local state and declarations", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("binds each body source to the same record CID as raw", async () => {
    const e = new MockEngine();
    const list = await e.listPosts({ forum: FORUM_URI, sort: "new", filter: "all", limit: 100 });
    const post = list.items.find((p) => p.commentCount > 0);
    if (!post) throw new Error("no post with comments");
    const thread = await e.getThread(post.uri, { sort: "new", unreadOnly: false });
    const comment = thread.comments.find((c) => c.body.text.length > 0);
    if (!comment) throw new Error("no comment with an inline body");
    for (const uri of [FORUM_URI, post.uri, comment.uri]) {
      const [raw, source] = await Promise.all([e.raw(uri), e.bodySource(uri)]);
      expect(source.cid).toBe(raw.cid);
      expect(source.profile).toBe("lfm/1");
    }
  });

  it("preserves media, card, label and quote fields through spilled edits", async () => {
    for (const spilledBodies of [false, true]) {
      const e = new MockEngine({ holdMs: 0, sendMs: 0, spilledBodies });
      const forum = FORUM_URI;
      const me = (await e.activeAccount())?.did;
      if (!me) throw new Error("no active account");
      const media = {
        id: "bafyreiea7pi4w34g3sju7r6yhhl4tv4zwgtq2g4w2h2g2g2g2g2g2g2g2g",
        cid: "bafyreiea7pi4w34g3sju7r6yhhl4tv4zwgtq2g4w2h2g2g2g2g2g2g2g",
        mime: "image/png",
        sizeBytes: 184211,
        alt: "Preserved diagram",
        aspectRatio: { width: 1600, height: 900 },
      };
      const link = {
        uri: "https://example.test/paper",
        title: "Card title",
        description: "Card description",
      };
      const published = await e.publish({
        forum,
        kind: "link",
        title: "Preserved card",
        body: { markdown: "Original source", media: [media] },
        link,
        labels: ["preserved-label"],
      });
      await vi.advanceTimersByTimeAsync(10);
      const post = (await e.writeStatus(published.id))?.state;
      if (post?.kind !== "done") throw new Error("post did not finish");
      await e.edit(
        post.uri,
        {
          kind: "post",
          post: {
            forum,
            kind: "link",
            title: "Preserved card",
            body: { markdown: "Edited source", media: [media] },
            link,
            labels: ["preserved-label"],
          },
        },
        { history: true },
      );
      await vi.advanceTimersByTimeAsync(10);
      expect(await e.raw(post.uri)).toMatchObject({
        record: {
          link,
          labels: ["preserved-label"],
          body: { media: [{ alt: "Preserved diagram", aspectRatio: media.aspectRatio }] },
        },
      });
      expect((await e.bodySource(post.uri)).markdown).toBe("Edited source");
      const reply = await e.comment({
        root: post.uri,
        body: { markdown: "Comment source", media: [media] },
        quote: { exact: "Original", prefix: "", suffix: " source", start: 0 },
      });
      await vi.advanceTimersByTimeAsync(10);
      const comment = (await e.writeStatus(reply.id))?.state;
      if (comment?.kind !== "done") throw new Error("comment did not finish");
      await e.edit(
        comment.uri,
        {
          kind: "comment",
          comment: {
            root: post.uri,
            body: { markdown: "Edited comment", media: [media] },
            quote: { exact: "Original", prefix: "", suffix: " source", start: 0 },
          },
        },
        { history: true },
      );
      await vi.advanceTimersByTimeAsync(10);
      expect(await e.raw(comment.uri)).toMatchObject({
        record: {
          quote: { exact: "Original", start: 0 },
          body: { media: [{ alt: "Preserved diagram", aspectRatio: media.aspectRatio }] },
        },
      });
    }
  });

  it("never gives a new draft the id of an existing one", async () => {
    const e = new MockEngine();
    const before = await e.drafts();
    expect(before.length).toBeGreaterThan(0);
    const id = await e.saveDraft({
      kind: "comment",
      comment: { root: "at://x", body: { markdown: "new", media: [] } },
    });
    expect(before.map((d) => d.id)).not.toContain(id);
    expect(await e.drafts()).toHaveLength(before.length + 1);
  });

  it("imports an export back, with or without the scope (engine-api.md §3.6)", async () => {
    const e = new MockEngine();
    const exported = await e.exportLocal();
    const report = await e.importLocal(exported, "everything");
    expect(report.imported).toBeGreaterThan(0);
    expect(await e.importLocal(exported)).toEqual(report);
    await expect(e.importLocal({ format: "other" }, "mine")).rejects.toMatchObject({
      code: "invalid_input",
    });
  });

  it("round-trips a forum declaration through forumCreate, raw and forumUpdate", async () => {
    vi.useFakeTimers();
    try {
      const e = new MockEngine({ sendMs: 1 });
      const ticket = await e.forumCreate({
        slug: "new-forum",
        name: "New forum",
        description: "Tagline",
        about: { markdown: "Rules", media: [] },
        membership: "open",
        defaultTier: "frontpage",
        featuredTags: ["ai"],
        moderators: [],
        reportContact: "mailto:mods@example.com",
      });
      await vi.advanceTimersByTimeAsync(10);
      const uri = (await e.followed()).find((f) => f.name === "New forum")?.uri ?? "";
      expect(uri).toMatch(/\/new-forum$/);
      expect(ticket.state.kind).toBe("pending");
      const raw = await e.raw(uri);
      expect(raw.record).toMatchObject({
        name: "New forum",
        description: "Tagline",
        about: { text: "Rules" },
        membership: "open",
        defaultTier: "frontpage",
        featuredTags: ["ai"],
        reportContact: "mailto:mods@example.com",
      });
      await e.forumUpdate(uri, {
        slug: "new-forum",
        name: "Renamed",
        membership: "approval",
        defaultTier: "personal",
        featuredTags: [],
        moderators: [],
      });
      await vi.advanceTimersByTimeAsync(10);
      const afterView = await e.raw(uri);
      const after = afterView.record as Record<string, unknown>;
      expect(afterView.cid).not.toBe(raw.cid);
      expect(afterView.cid).toBe((await e.bodySource(uri)).cid);
      expect(after.name).toBe("Renamed");
      expect(after.about).toBeUndefined();
      expect(after.reportContact).toBeUndefined();
      expect((await e.forumOpen(uri)).forum.membership).toBe("approval");
    } finally {
      vi.useRealTimers();
    }
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
