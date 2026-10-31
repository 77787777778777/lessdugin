// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// Writing and editing a post: the edit warning comes from the engine's editPreflight
// (social.md §4.5; review finding robustness-6).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView } from "@lessdugin/engine/types";
import { screen } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../../test/render.tsx";
import { EditPost } from "./ForumNewPostPage.tsx";
import { ForumContext } from "./forumContext.tsx";

async function ownPost(engine: MockEngine): Promise<{ view: ForumView; uri: string }> {
  const me = (await engine.activeAccount())?.did;
  const forum = (await engine.followed())[0]?.uri ?? "";
  const view = await engine.forumOpen(forum);
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  const own = list.items.find((p) => p.author.did === me);
  if (!own) throw new Error("no own post in the fixture");
  return { view, uri: own.uri };
}

describe("EditPost", () => {
  // the composer focuses CodeMirror, which calls execCommand; jsdom has none
  beforeAll(() => {
    Object.defineProperty(document, "execCommand", { value: () => false, configurable: true });
  });

  it("warns with the engine's editPreflight notice (robustness-6)", async () => {
    const engine = new MockEngine();
    const { view, uri } = await ownPost(engine);
    const preflight = vi
      .spyOn(engine, "editPreflight")
      .mockResolvedValue([
        { code: "suspendsAssertions", message: "editing suspends its frontpage tier and its pin" },
      ]);
    renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    expect((await screen.findByTestId("edit-suspends")).textContent).toBe(
      "editing suspends its frontpage tier and its pin",
    );
    expect(preflight).toHaveBeenCalledWith(uri);
  });

  it("edits without a warning when the preflight fails", async () => {
    const engine = new MockEngine();
    const { view, uri } = await ownPost(engine);
    vi.spyOn(engine, "editPreflight").mockRejectedValue(new Error("offline"));
    renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    await screen.findByRole("heading", { name: "Edit post" });
    expect(screen.queryByTestId("edit-suspends")).toBeNull();
  });
});
