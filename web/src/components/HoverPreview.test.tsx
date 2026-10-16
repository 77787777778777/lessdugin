// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// (Previews render bodies through DOMPurify; see lib/sanitize.test.ts.)

import { MockEngine } from "@lessdugin/engine/mock";
import type { PostSummary, PostView } from "@lessdugin/engine/types";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { HoverPreview } from "./HoverPreview.tsx";

async function postTitled(engine: MockEngine, prefix: string): Promise<PostSummary> {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 100 });
  const post = list.items.find((p) => p.title?.startsWith(prefix));
  if (!post) throw new Error(`no post ${prefix}`);
  return post;
}

const rect = (): DOMRect =>
  ({
    x: 10,
    y: 10,
    left: 10,
    top: 10,
    right: 60,
    bottom: 30,
    width: 50,
    height: 20,
    toJSON: () => ({}),
  }) as DOMRect;

function preview(engine: MockEngine, raw: string) {
  return renderWithApp(
    <HoverPreview
      target={{ raw, rect: rect() }}
      onClose={() => {}}
      onPointerEnter={() => {}}
      onPointerLeave={() => {}}
    />,
    { engine },
  );
}

describe("HoverPreview (lfm-1.md §5.3)", () => {
  it("re-parses the raw target through the engine before using it (F5)", async () => {
    const engine = new MockEngine();
    const post = await postTitled(engine, "Welcome");
    const parseRef = vi.spyOn(engine, "parseRef");
    preview(engine, post.uri);
    const dialog = await screen.findByRole("dialog", { name: "Link preview" });
    await waitFor(() => expect(dialog.textContent).toContain("Welcome to LessDugin"));
    expect(parseRef).toHaveBeenCalledWith(post.uri, {});
  });

  it("shows a metadata card without the title or body for a post that is not listed", async () => {
    const engine = new MockEngine();
    const post = await postTitled(engine, "Welcome");
    const real = await engine.getPost(post.uri);
    const hidden: PostView = {
      ...real,
      post: {
        ...real.post,
        state: { kind: "hidden", reason: { kind: "removed", rank: 3, spam: false } },
      },
    };
    vi.spyOn(engine, "getPost").mockResolvedValue(hidden);
    preview(engine, post.uri);
    const dialog = await screen.findByRole("dialog", { name: "Link preview" });
    await waitFor(() => expect(dialog.textContent).toContain("This item is not shown."));
    expect(dialog.textContent).not.toContain("Welcome to LessDugin");
    expect(dialog.querySelector("[data-rendered-root]")).toBeNull();
    expect(within(dialog).getByRole("link", { name: "Open →" })).toBeTruthy();
  });

  it("opens no preview from inside a preview (depth 1)", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const engine = new MockEngine();
      const post = await postTitled(engine, "Welcome");
      preview(engine, post.uri);
      const dialog = await screen.findByRole("dialog", { name: "Link preview" });
      const link = await waitFor(() => {
        const a = dialog.querySelector("a.at-link");
        if (!(a instanceof HTMLElement)) throw new Error("no record link yet");
        return a;
      });
      fireEvent.mouseOver(link);
      fireEvent.focusIn(link);
      await act(async () => {
        vi.advanceTimersByTime(1000);
      });
      expect(screen.getAllByRole("dialog", { name: "Link preview" })).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
