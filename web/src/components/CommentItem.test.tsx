// SPDX-License-Identifier: AGPL-3.0-or-later
// A comment's labels name its author by display name AND verified handle or short DID, never by
// the display name alone (threat-model ID-5, review finding sec-11).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ActorView, CommentNode } from "@lessdugin/engine/types";
import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { CommentItem } from "./CommentItem.tsx";

const impostor: ActorView = {
  did: "did:plc:abcdefghijklmnopqrstuvwx",
  shortDid: "did:plc:abcd…uvwx",
  handleState: "unverified",
  displayName: "@alice.bsky.social",
};

async function visibleComment(engine: MockEngine): Promise<{ node: CommentNode; post: string }> {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  for (const p of list.items) {
    const thread = await engine.getThread(p.uri, { sort: "magic", unreadOnly: false });
    const node = thread.comments.find((c) => c.state.kind === "visible" && !c.missing);
    if (node) return { node: { ...node, author: impostor, unread: false }, post: p.uri };
  }
  throw new Error("no visible comment");
}

const ctxFor = (post: string, forum: string) => ({
  postUri: post,
  forum,
  votingMode: "reacts" as const,
  locked: false,
  myDid: null,
  isAuthority: false,
  canCurate: false,
});

const WHO = "@alice.bsky.social (did:plc:abcd…uvwx)";

describe("CommentItem labels (sec-11)", () => {
  it("never name the author by display name alone", async () => {
    const engine = new MockEngine();
    const { node, post } = await visibleComment(engine);
    const forum = (await engine.followed())[0]?.uri ?? "";
    renderWithApp(
      <CommentItem
        row={{
          kind: "comment",
          node,
          indent: 1,
          collapsed: true,
          hiddenReplies: 0,
          authorOverflow: 3,
        }}
        index={0}
        ctx={ctxFor(post, forum)}
        focused={false}
        replying={false}
        onToggle={() => {}}
        onFocusRow={() => {}}
        onReply={() => {}}
        onShowAuthor={() => {}}
      />,
      { engine },
    );
    expect(await screen.findByRole("article", { name: `Comment by ${WHO}` })).toBeTruthy();
    expect(screen.getByRole("button", { name: `Show 3 more by ${WHO}` })).toBeTruthy();
  });

  it("names the author the same way in the reply composer", async () => {
    const engine = new MockEngine();
    const { node, post } = await visibleComment(engine);
    const forum = (await engine.followed())[0]?.uri ?? "";
    renderWithApp(
      <CommentItem
        row={{ kind: "comment", node, indent: 1, collapsed: false, hiddenReplies: 0 }}
        index={0}
        ctx={{ ...ctxFor(post, forum), myDid: (await engine.activeAccount())?.did ?? null }}
        focused={false}
        replying
        onToggle={() => {}}
        onFocusRow={() => {}}
        onReply={() => {}}
      />,
      { engine },
    );
    // the editable element lives in the composer's shadow root
    await waitFor(
      () => {
        const host = document.querySelector('[data-testid="composer-editor"]');
        const content = host?.shadowRoot?.querySelector(".cm-content");
        expect(content?.getAttribute("aria-label")).toBe(`Reply to ${WHO}`);
      },
      { timeout: 4000 },
    );
  });
});
