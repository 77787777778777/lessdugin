// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// Writing and editing a post: the edit warning comes from the engine's editPreflight
// (social.md §4.5; review finding robustness-6), a submission sent again after a refusal
// lists its attachments once (robustness-5), and a post whose body spilled into a blob is
// edited from its bodySource, or refused with the reason (robustness-2).

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { EditorView } from "@codemirror/view";
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView, Json, MediaRef, RenderedBody } from "@lessdugin/engine/types";
import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import * as wasm from "../../../../packages/engine/pkg/lessdugin_engine.js";
import { NS } from "../../../../packages/engine/src/auth-bridge.ts";
import { WorkerEngine, type WorkerEnv } from "../../../../packages/engine/src/worker-core.ts";
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

/** One render through the actual Rust engine compiled to WASM, not the mock renderer. */
async function renderWithRealEngine(
  markdown: string,
  media: MediaRef[] | Json[],
): Promise<RenderedBody> {
  const require = createRequire(import.meta.url);
  const wasmPath = require.resolve("../../../../packages/engine/pkg/lessdugin_engine_bg.wasm");
  wasm.initSync({ module: readFileSync(wasmPath) });
  const env: WorkerEnv = {
    fetch: (async () => {
      throw new TypeError("offline");
    }) as typeof fetch,
    locks: null,
    idb: null,
    channel: null,
    warn: () => {},
  };
  const worker = new WorkerEngine(wasm, env);
  await worker.init(
    { config: {}, namespace: NS, elect: false },
    {
      request: async () => {
        throw { kind: "authRequired", message: "pure render needs no session" };
      },
    },
    () => {},
  );
  try {
    return (await worker.call("render", [
      markdown,
      media,
      { html: true, text: true },
    ])) as RenderedBody;
  } finally {
    await worker.close();
  }
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

  it("renders a referenced retained medium once and submits one edited item", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0, spilledBodies: true });
    const rendered: RenderedBody[] = [];
    vi.spyOn(engine, "render").mockImplementation(async (markdown, media) => {
      const result = await renderWithRealEngine(markdown, media);
      rendered.push(result);
      return result;
    });
    const { view, uri } = await ownPost(engine);
    const current = await engine.raw(uri);
    const media = {
      id: "bafyreiea7pi4w34g3sju7r6yhhl4tv4zwgtq2g4w2h2g2g2g2g2g2g2g2g",
      cid: "bafyreiea7pi4w34g3sju7r6yhhl4tv4zwgtq2g4w2h2g2g2g2g2g2g2g2g",
      mime: "image/png",
      sizeBytes: 1234,
      alt: "Retained image",
      aspectRatio: { width: 4, height: 3 },
    };
    const originalRaw = engine.raw.bind(engine);
    const edit = vi.spyOn(engine, "edit");
    const originalSource = engine.bodySource.bind(engine);
    vi.spyOn(engine, "bodySource").mockImplementation(async (target) => {
      const source = await originalSource(target);
      return target === uri
        ? { ...source, markdown: `![Retained image](cid:${media.cid})` }
        : source;
    });
    let initialRaw = true;
    vi.spyOn(engine, "raw").mockImplementation(async (target) => {
      const result = await originalRaw(target);
      if (target !== uri || !initialRaw) return result;
      initialRaw = false;
      return {
        ...result,
        record: {
          ...(current.record as object),
          body: {
            ...(current.record as { body: object }).body,
            media: [
              {
                blob: { ref: { $link: media.cid }, mimeType: media.mime, size: media.sizeBytes },
                alt: media.alt,
                aspectRatio: media.aspectRatio,
              },
            ],
          },
        },
      };
    });
    const firstForm = renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    const alt = (await screen.findByLabelText("Alt text")) as HTMLInputElement;
    expect(alt.value).toBe("Retained image");
    const preview = await screen.findByRole("region", { name: "Preview" }, { timeout: 10_000 });
    await waitFor(() => expect(rendered.length).toBeGreaterThan(0));
    // The real renderer returns one media entry per matching ref (crates/markdown media_used),
    // so the kept ref plus the initial attachment must not both reach it.
    expect(rendered.at(-1)?.media.filter((item) => item.cid === media.cid)).toHaveLength(1);
    expect(preview.querySelectorAll("img[data-cid]")).toHaveLength(1);
    fireEvent.change(alt, { target: { value: "Edited retained image" } });
    await waitFor(() => expect(rendered.at(-1)?.media[0]?.alt).toBe("Edited retained image"));
    expect(rendered.at(-1)?.media.filter((item) => item.cid === media.cid)).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Save edit" }));
    await screen.findByText("Post updated");
    const saved = (await engine.raw(uri)).record as { body: { media: unknown[] } };
    expect(saved.body.media).toHaveLength(1);
    expect(saved).toMatchObject({ body: { media: [{ alt: "Edited retained image" }] } });
    const submitted = edit.mock.calls[0]?.[1];
    expect(submitted?.kind).toBe("post");
    if (submitted?.kind !== "post") throw new Error("post edit not submitted");
    const submittedMedia = submitted.post.body.media;
    expect(submittedMedia).toHaveLength(1);
    expect(submittedMedia?.[0]?.alt).toBe("Edited retained image");

    firstForm.unmount();
    renderWithApp(
      <ForumContext value={view}>
        <EditPost uri={uri} />
      </ForumContext>,
      { engine },
    );
    const remove = await screen.findByRole("button", {
      name: `Remove Existing media ${media.cid}`,
    });
    fireEvent.click(remove);
    fireEvent.click(screen.getByRole("button", { name: "Save edit" }));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(2));
    const removed = edit.mock.calls[1]?.[1];
    if (removed?.kind !== "post") throw new Error("post removal edit not submitted");
    expect(removed.post.body.media).toEqual([]);
  }, 30_000);

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
