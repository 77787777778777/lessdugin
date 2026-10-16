// SPDX-License-Identifier: AGPL-3.0-or-later
import { MockEngine } from "@lessdugin/engine/mock";
import type { PostSummary } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { renderWithApp } from "../test/render.tsx";
import { displayedAxis, VoteControls } from "./VoteControls.tsx";

async function votablePost(engine: MockEngine): Promise<PostSummary> {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const me = (await engine.activeAccount())?.did;
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  const post = list.items.find((p) => p.author.did !== me && p.score.mine === undefined);
  if (!post) throw new Error("no votable post");
  return post;
}

/** VoteControls fed by a live engine query, as on the post page. */
function LiveVotes({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.post(uri), queryFn: () => engine.getPost(uri) });
  if (!q.data) return null;
  return <VoteControls subject={uri} score={q.data.post.score} size="post" />;
}

function karmaScore(): number {
  const group = screen.getByRole("group", { name: "Karma vote" });
  const text = within(group).getByText(/Karma score/).parentElement?.textContent ?? "";
  return Number(/(-?\d+)\s*$/.exec(text)?.[1]);
}

async function acknowledge(): Promise<void> {
  const dialog = await screen.findByRole("dialog", { name: "Votes are public" });
  fireEvent.click(within(dialog).getByRole("button", { name: "I understand, continue" }));
}

describe("displayedAxis", () => {
  const score = {
    score: 10,
    agreement: 2,
    unweighted: 0,
    earlierVersionVotes: 0,
    mine: { karma: "up" as const },
  };
  it("uses the server's vote without overrides", () => {
    expect(displayedAxis("karma", score, {})).toEqual({ mine: "up", total: 10 });
  });
  it("replaces the server's vote power with the optimistic one", () => {
    expect(displayedAxis("karma", score, { karma: "strongDown" })).toEqual({
      mine: "strongDown",
      total: 6,
    });
    expect(displayedAxis("karma", score, { karma: null })).toEqual({ mine: null, total: 9 });
  });
});

describe("VoteControls", () => {
  it("asks once that votes are public, votes optimistically, and undoes", async () => {
    const engine = new MockEngine({ holdMs: 400, sendMs: 10 });
    const undo = vi.spyOn(engine, "undo");
    const post = await votablePost(engine);
    renderWithApp(<LiveVotes uri={post.uri} />, { engine });
    const up = await screen.findByRole("button", { name: "Upvote" });
    const before = karmaScore();

    fireEvent.click(up);
    await acknowledge();
    await waitFor(() => expect(karmaScore()).toBe(before + 1));
    expect(screen.getByRole("button", { name: /^Upvote/ }).getAttribute("aria-pressed")).toBe(
      "true",
    );

    fireEvent.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(undo).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(karmaScore()).toBe(before));
    expect(await screen.findByText("Vote undone")).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Upvote/ }).getAttribute("aria-pressed")).toBe(
      "false",
    );
  });

  it("strong votes on shift-click and is confirmed by the engine's re-query", async () => {
    const engine = new MockEngine({ holdMs: 100, sendMs: 10 });
    const post = await votablePost(engine);
    renderWithApp(<LiveVotes uri={post.uri} />, { engine });
    const up = await screen.findByRole("button", { name: "Upvote" });
    const before = karmaScore();
    fireEvent.click(up, { shiftKey: true });
    await acknowledge();
    await waitFor(() => expect(karmaScore()).toBe(before + 3));
    expect(screen.getByRole("button", { name: /^Upvote/ }).getAttribute("aria-label")).toContain(
      "strong",
    );
    // After the hold the engine applies the vote; the re-queried score agrees.
    await waitFor(
      async () => expect((await engine.getPost(post.uri)).post.score.mine?.karma).toBe("strongUp"),
      {
        timeout: 2000,
      },
    );
    await waitFor(() => expect(karmaScore()).toBe(before + 3));
  });

  it("does not vote on the viewer's own items", async () => {
    const engine = new MockEngine();
    const opinion = vi.spyOn(engine, "opinion");
    renderWithApp(
      <VoteControls
        subject="at://did:plc:abcdefghijklmnopqrstuvwx/x/y"
        score={{ score: 1, agreement: 0, unweighted: 0, earlierVersionVotes: 0 }}
        ownItem
      />,
      { engine },
    );
    const up = await screen.findByRole("button", { name: "Upvote" });
    expect((up as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(up);
    expect(opinion).not.toHaveBeenCalled();
  });
});
