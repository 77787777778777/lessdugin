// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// Writing and editing a post: the edit warning comes from the engine's editPreflight
// (social.md §4.5; review finding robustness-6), a submission sent again after a refusal
// lists its attachments once (robustness-5), and a post whose body spilled into a blob is
// edited from its bodySource, or refused with the reason (robustness-2).
import { EditorView } from "@codemirror/view";
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView } from "@lessdugin/engine/types";
import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../../test/render.tsx";
import { EditPost, PostForm } from "./ForumNewPostPage.tsx";
import { ForumContext } from "./forumContext.tsx";

/** The CodeMirror view inside the composer's shadow root (as Composer.test.tsx finds it). */
async function editor(container: HTMLElement): Promise<EditorView> {
  return waitFor(
    () => {
      const host = container.querySelector('[data-testid="composer-editor"]');
      const content = host?.shadowRoot?.querySelector(".cm-content");
      const view = content instanceof HTMLElement ? EditorView.findFromDOM(content) : null;
      if (!view) throw new Error("editor not mounted");
      return view;
    },
    { timeout: 10_000 },
  );
}

async function ownPost(engine: MockEngine): Promise<{ view: ForumView; uri: string }> {
  const me = (await engine.activeAccount())?.did;
  const forum = (await engine.followed())[0]?.uri ?? "";
  const view = await engine.forumOpen(forum);
  const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
  const own = list.items.find((p) => p.author.did === me);
  if (!own) throw new Error("no own post in the fixture");
  return { view, uri: own.uri };
}

// the composer focuses CodeMirror, which calls execCommand; jsdom has none
beforeAll(() => {
  Object.defineProperty(document, "execCommand", { value: () => false, configurable: true });
});

describe("PostForm", () => {
  it("lists a prepared attachment once when a refused publish is sent again (robustness-5)", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view = await engine.forumOpen(forum);
    const refused = { code: "invalid_input", message: "The title is too long.", retryable: false };
    const publish = vi
      .spyOn(engine, "publish")
      .mockRejectedValueOnce(refused)
      .mockRejectedValueOnce(refused);
    const { container } = renderWithApp(
      <ForumContext value={view}>
        <PostForm initial={null} draftId={undefined} />
      </ForumContext>,
      { engine },
    );
    fireEvent.change(await screen.findByLabelText("Title"), { target: { value: "T" } });
    const cm = await editor(container);
    cm.dispatch({ changes: { from: 0, insert: "A body." } });
    const shot = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", {
      type: "image/png",
    });
    fireEvent.paste(cm.contentDOM, { clipboardData: { files: [shot], types: ["Files"] } });
    await waitFor(() =>
      expect(screen.getByTestId("attachment-list").textContent).toContain("ready"),
    );
    // refused before it is enqueued: the composer keeps the text and the attachment
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    await screen.findByRole("alert");
    await waitFor(() => expect(publish).toHaveBeenCalledTimes(1));
    expect(publish.mock.calls[0]?.[0].body.media).toHaveLength(1);
    // sent again: the same attachment, once
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(publish).toHaveBeenCalledTimes(2));
    const media = publish.mock.calls[1]?.[0].body.media ?? [];
    expect(media).toHaveLength(1);
    expect(media[0]?.cid).toBe(publish.mock.calls[0]?.[0].body.media?.[0]?.cid);
  });
});

describe("EditPost", () => {
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

  it("loads a spilled post's source, then uses the normal edit form (robustness-2)", async () => {
    const engine = new MockEngine({ spilledBodies: true });
    const { view, uri } = await ownPost(engine);
    const expected = await engine.bodySource(uri);
    expect(expected.spilled).toBe(true);
    const source = vi.spyOn(engine, "bodySource");
    const { container } = renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    await screen.findByRole("heading", { name: "Edit post" });
    const cm = await editor(container);
    expect(cm.state.doc.toString()).toBe(expected.markdown);
    expect(source).toHaveBeenCalledWith(uri);
    expect(screen.queryByText("This post cannot be edited")).toBeNull();
  });

  it("refuses a mixed revision instead of combining newer media with older source", async () => {
    vi.useRealTimers();
    const engine = new MockEngine({ spilledBodies: true });
    const { view, uri } = await ownPost(engine);
    const current = await engine.raw(uri);
    const originalRaw = engine.raw.bind(engine);
    const originalSource = engine.bodySource.bind(engine);
    const rawRead = vi.spyOn(engine, "raw");
    rawRead.mockImplementation(async (target) => {
      const result = await originalRaw(target);
      if (target !== uri || rawRead.mock.calls.length % 2 === 0) return result;
      // Odd calls are the page's outer raw reads; even ones belong to mock bodySource.
      return {
        ...result,
        cid: "record-before-source",
        record: { ...(current.record as object), title: "Wrong revision title" },
      };
    });
    let release: (() => void) | undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    let calls = 0;
    vi.spyOn(engine, "bodySource").mockImplementation(async (target) => {
      calls++;
      if (calls === 1) await held;
      return originalSource(target);
    });
    renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    await waitFor(() => expect(engine.bodySource).toHaveBeenCalledTimes(1));
    await act(async () => release?.());
    expect(await screen.findByText("This post cannot be edited")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Edit post" })).toBeNull();
    expect(screen.queryByDisplayValue("Wrong revision title")).toBeNull();
    await vi.waitFor(() => expect(engine.bodySource).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(engine.bodySource).toHaveBeenCalledTimes(2));
    // Each attempt calls the page's raw plus the mock's own coherent raw inside bodySource.
    expect(engine.raw).toHaveBeenCalledTimes(4);
  });

  it("refuses to edit a spilled post when its source is unavailable (robustness-2)", async () => {
    const engine = new MockEngine({ spilledBodies: true });
    const { view, uri } = await ownPost(engine);
    const source = vi.spyOn(engine, "bodySource").mockRejectedValue({
      code: "network",
      message: "The source is not available.",
      retryable: true,
    });
    renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    await screen.findByText("This post cannot be edited");
    expect(screen.getByText(/stored in a blob that could not be loaded/)).toBeTruthy();
    expect(source).toHaveBeenCalledWith(uri);
    expect(screen.queryByRole("heading", { name: "Edit post" })).toBeNull();
  });
});
