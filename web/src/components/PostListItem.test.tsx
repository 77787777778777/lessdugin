// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// The viewer's own newcomer post (social.md §10.3): listed like any other post, with the
// "collapsed for other readers" label; everyone else sees it collapsed.

import { MockEngine } from "@lessdugin/engine/mock";
import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { PostListItem } from "./PostListItem.tsx";

async function newcomerPost(engine: MockEngine) {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const list = await engine.listPosts({ forum, sort: "new", filter: "all", limit: 100 });
  const post = list.items.find((p) => p.state.kind === "collapsed" && p.state.reason === "newUser");
  if (!post) throw new Error("no newcomer post");
  return post;
}

describe("PostListItem", () => {
  it("marks a newcomer's post collapsed for other readers", async () => {
    const engine = new MockEngine();
    const post = await newcomerPost(engine);
    renderWithApp(<PostListItem post={post} />, { engine });
    const item = await screen.findByRole("article");
    expect(within(item).getByText("Collapsed: from a new user")).toBeTruthy();
    expect(item.className).toMatch(/collapsed/);
  });

  it("shows the newcomer their own post expanded, label kept", async () => {
    const engine = new MockEngine();
    const post = await newcomerPost(engine);
    await engine.addAccount({ did: post.author.did, pds: "https://pds.test", auth: "oauth" });
    await engine.setActiveAccount(post.author.did);
    renderWithApp(<PostListItem post={post} />, { engine });
    const item = await screen.findByRole("article");
    expect(
      await within(item).findByText("Collapsed for other readers: from a new user"),
    ).toBeTruthy();
    expect(item.className).not.toMatch(/collapsed/);
  });
});
