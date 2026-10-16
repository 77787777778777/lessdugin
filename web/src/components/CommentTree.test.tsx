// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
import { MockEngine } from "@lessdugin/engine/mock";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { CommentTree } from "./CommentTree.tsx";

const ctxFor = (post: { uri: string; forum: string }, myDid: string | null = null) => ({
  postUri: post.uri,
  forum: post.forum,
  votingMode: "reacts" as const,
  locked: false,
  myDid,
  isAuthority: false,
  canCurate: false,
});

const rowCount = (): number => document.querySelectorAll("[data-row-index]").length;

async function welcomeThread(engine: MockEngine) {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  const post = list.items.find((p) => p.title?.startsWith("Welcome"));
  if (!post) throw new Error("no welcome post");
  return { post, thread: await engine.getThread(post.uri, { sort: "magic", unreadOnly: false }) };
}

describe("CommentTree", () => {
  it("renders collapsed reasons and tombstones, and expands on demand", async () => {
    const engine = new MockEngine();
    const { post, thread } = await welcomeThread(engine);
    renderWithApp(
      <CommentTree
        comments={thread.comments}
        ctx={{
          postUri: post.uri,
          forum: post.forum,
          votingMode: "reacts",
          locked: false,
          myDid: null,
          isAuthority: false,
          canCurate: false,
        }}
      />,
      { engine },
    );
    const dan = await screen.findByRole("article", { name: /^Comment by Dan Newcomer/ });
    expect(within(dan).getByText(/Collapsed: from a new user/)).toBeTruthy();
    expect(screen.queryByText(/Welcome! Start with the moderation policy/)).toBeNull();
    fireEvent.click(within(dan).getByRole("button", { name: "Expand comment" }));
    await waitFor(() =>
      expect(screen.getByText(/Welcome! Start with the moderation policy/)).toBeTruthy(),
    );

    expect(screen.getByText("[Removed by a moderator: Off-topic advertising]")).toBeTruthy();
    expect(screen.getByText("[Hidden: the author is banned from this forum]")).toBeTruthy();
    expect(screen.getByText(/\[missing comment\]/)).toBeTruthy();
    expect(screen.getByRole("link", { name: /Continue this thread/ })).toBeTruthy();
    expect(screen.getAllByText("New").length).toBeGreaterThan(0);
  });

  it("does not hide a placeholder's replies on Enter (social.md §10.2)", async () => {
    const engine = new MockEngine();
    const { post, thread } = await welcomeThread(engine);
    renderWithApp(<CommentTree comments={thread.comments} ctx={ctxFor(post)} />, { engine });
    const removed = (await screen.findAllByRole("article", { name: "Removed comment" }))[0];
    if (!removed) throw new Error("no tombstone");
    const before = rowCount();
    removed.focus();
    fireEvent.keyDown(removed, { key: "Enter" });
    expect(rowCount()).toBe(before);
    expect(within(removed).queryByText(/hidden\./)).toBeNull();
  });

  it("keeps keyboard focus on the same comment when rows above it collapse", async () => {
    const engine = new MockEngine();
    const { post, thread } = await welcomeThread(engine);
    const cs = thread.comments;
    // X: a visible comment with replies; C: the next visible comment after X's subtree.
    const xi = cs.findIndex(
      (c, i) => c.state.kind === "visible" && !c.missing && (cs[i + 1]?.depth ?? 0) > c.depth,
    );
    const x = cs[xi];
    if (!x) throw new Error("no comment with replies");
    const c = cs.slice(xi + 1).find((n) => n.depth <= x.depth && n.state.kind === "visible");
    if (!c) throw new Error("no comment after the subtree");
    renderWithApp(<CommentTree comments={cs} ctx={ctxFor(post)} />, { engine });
    const article = (uri: string): HTMLElement => {
      const el = [...document.querySelectorAll("[data-comment-uri]")].find(
        (e) => e.getAttribute("data-comment-uri") === uri,
      );
      if (!(el instanceof HTMLElement)) throw new Error(`no row for ${uri}`);
      return el;
    };
    await waitFor(() => article(c.uri));
    const indexBefore = Number(article(c.uri).getAttribute("data-row-index"));
    article(c.uri).focus();
    await waitFor(() => expect(article(c.uri).className).toMatch(/focused/));
    fireEvent.click(within(article(x.uri)).getByRole("button", { name: "Collapse comment" }));
    await waitFor(() => {
      // The collapse hid rows above C: a focus tracked by row index would now be on another row.
      expect(Number(article(c.uri).getAttribute("data-row-index"))).toBeLessThan(indexBefore);
      const focused = [...document.querySelectorAll("[data-comment-uri]")].filter((e) =>
        /focused/.test(e.className),
      );
      expect(focused.map((e) => e.getAttribute("data-comment-uri"))).toEqual([c.uri]);
    });
  });

  it("offers sign-in instead of a reply box when signed out", async () => {
    const engine = new MockEngine({ signedOut: true });
    const { post, thread } = await welcomeThread(engine);
    const { router } = renderWithApp(
      <CommentTree comments={thread.comments} ctx={ctxFor(post)} />,
      { engine },
    );
    const buttons = await screen.findAllByRole("button", { name: "Sign in to reply" });
    expect(screen.queryByRole("button", { name: "Reply" })).toBeNull();
    fireEvent.click(buttons[0] as HTMLElement);
    await waitFor(() => expect(router.state.location.pathname).toBe("/login"));
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
