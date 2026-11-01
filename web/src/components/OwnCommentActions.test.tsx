// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// Before an author edits an item that carries positive assertions, the client warns that the
// edit suspends them, with what the engine's editPreflight names (social.md §4.5; review
// findings sec-7, robustness-6).
import { EditorView } from "@codemirror/view";
import { MockEngine } from "@lessdugin/engine/mock";
import type { CommentNode } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { OwnCommentActions } from "./OwnCommentActions.tsx";

async function ownComment(engine: MockEngine): Promise<{ node: CommentNode; post: string }> {
  const me = (await engine.activeAccount())?.did;
  const forum = (await engine.followed())[0]?.uri ?? "";
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  for (const p of list.items) {
    const t = await engine.getThread(p.uri, { sort: "top" });
    const node = t.comments.find((c) => c.author.did === me);
    if (node) return { node, post: p.uri };
  }
  throw new Error("no own comment in the fixture");
}

describe("OwnCommentActions edit", () => {
  // the edit composer focuses CodeMirror, which calls execCommand; jsdom has none
  beforeAll(() => {
    Object.defineProperty(document, "execCommand", { value: () => false, configurable: true });
  });

  it("warns with the engine's editPreflight notice (robustness-6)", async () => {
    const engine = new MockEngine();
    const { node, post } = await ownComment(engine);
    const preflight = vi.spyOn(engine, "editPreflight").mockResolvedValue([
      {
        code: "suspendsAssertions",
        message: "editing suspends a moderator's keep: they apply to the version a moderator saw",
      },
    ]);
    renderWithApp(<OwnCommentActions node={node} postUri={post} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await waitFor(() =>
      expect(screen.getByTestId("edit-suspends").textContent).toMatch(
        /^editing suspends a moderator's keep/,
      ),
    );
    expect(preflight).toHaveBeenCalledWith(node.uri);
  });

  it("loads a spilled comment's source before opening its normal editor (robustness-2)", async () => {
    const engine = new MockEngine({ spilledBodies: true });
    const { node, post } = await ownComment(engine);
    const expected = await engine.bodySource(node.uri);
    expect(expected.spilled).toBe(true);
    const source = vi.spyOn(engine, "bodySource");
    const { container } = renderWithApp(<OwnCommentActions node={node} postUri={post} />, {
      engine,
    });
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await screen.findByRole("button", { name: "Save edit" }, { timeout: 10_000 });
    await waitFor(() => {
      const host = container.querySelector('[data-testid="composer-editor"]');
      const content = host?.shadowRoot?.querySelector(".cm-content");
      const editor = content instanceof HTMLElement ? EditorView.findFromDOM(content) : null;
      expect(editor?.state.doc.toString()).toBe(expected.markdown);
    });
    expect(source).toHaveBeenCalledWith(node.uri);
  });

  it("keeps a spilled comment closed when its source is unavailable (robustness-2)", async () => {
    const engine = new MockEngine({ spilledBodies: true });
    const { node, post } = await ownComment(engine);
    const source = vi.spyOn(engine, "bodySource").mockRejectedValue({
      code: "network",
      message: "The source is not available.",
      retryable: true,
    });
    const preflight = vi.spyOn(engine, "editPreflight");
    renderWithApp(<OwnCommentActions node={node} postUri={post} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await screen.findByText("The source is not available.");
    expect(source).toHaveBeenCalledWith(node.uri);
    expect(preflight).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Save edit" })).toBeNull();
  });

  it("says nothing when the engine names nothing, or cannot answer", async () => {
    const engine = new MockEngine();
    const { node, post } = await ownComment(engine);
    // the mock's editPreflight answers [] for a cached item
    renderWithApp(<OwnCommentActions node={node} postUri={post} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    // the composer loads lazily
    await screen.findByRole("button", { name: "Save edit" }, { timeout: 10_000 });
    expect(screen.queryByTestId("edit-suspends")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    // a preflight that fails degrades to editing without the warning
    vi.spyOn(engine, "editPreflight").mockRejectedValue(new Error("offline"));
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await screen.findByRole("button", { name: "Save edit" }, { timeout: 10_000 });
    expect(screen.queryByTestId("edit-suspends")).toBeNull();
  });
});
