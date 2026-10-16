// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
import { MockEngine } from "@lessdugin/engine/mock";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { CommentTree } from "./CommentTree.tsx";

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
});
