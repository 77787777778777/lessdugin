// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// Writing and editing a post: the edit warning comes from the engine's editPreflight
// (social.md §4.5; review finding robustness-6), a submission sent again after a refusal
// lists its attachments once (robustness-5), and a post whose body spilled into a blob is
// refused with the reason, never read as empty (robustness-2).
import { EditorView } from "@codemirror/view";
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView, Json } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor } from "@testing-library/react";
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

  it("says why a post whose body spilled into a blob is not edited here (robustness-2)", async () => {
    const engine = new MockEngine();
    const { view, uri } = await ownPost(engine);
    const real = await engine.raw(uri);
    const record = real.record as { [k: string]: Json };
    const body: Json = {
      $type: "online.7778777.lessdugin.temp.markdown",
      textBlob: {
        $type: "blob",
        ref: { $link: "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy" },
        mimeType: "text/markdown",
        size: 70_000,
      },
    };
    vi.spyOn(engine, "raw").mockResolvedValue({ ...real, record: { ...record, body } });
    renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    await screen.findByText("This post cannot be edited");
    expect(screen.getByText(/stored in a blob this app cannot read yet/)).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Edit post" })).toBeNull();
  });
});
