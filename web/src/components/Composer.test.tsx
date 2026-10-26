// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// (CodeMirror in a shadow root, and the live preview through DOMPurify.)

import { EditorView } from "@codemirror/view";
import { MockEngine } from "@lessdugin/engine/mock";
import type { Draft } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { Composer } from "./Composer.tsx";

const ROOT = "at://did:plc:abcdefghijklmnopqrstuvwx/online.7778777.lessdugin.temp.post/3kabc";

function makeDraft(markdown: string, id: string | undefined): Draft {
  return {
    kind: "comment",
    ...(id !== undefined ? { id } : {}),
    comment: { root: ROOT, body: { markdown, media: [] } },
  };
}

/** The CodeMirror view inside the composer's shadow root. */
async function editor(container: HTMLElement): Promise<EditorView> {
  return waitFor(() => {
    const host = container.querySelector('[data-testid="composer-editor"]');
    const content = host?.shadowRoot?.querySelector(".cm-content");
    const view = content instanceof HTMLElement ? EditorView.findFromDOM(content) : null;
    if (!view) throw new Error("editor not mounted");
    return view;
  });
}

function type(view: EditorView, text: string): void {
  view.dispatch({ changes: { from: view.state.doc.length, insert: text } });
}

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve: (v: T) => void = () => {};
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe("Composer", () => {
  it("names the editor for assistive technology (the label cannot cross the shadow root)", async () => {
    const { container } = renderWithApp(
      <Composer label="Reply to Carol" onSubmit={async () => true} />,
    );
    const view = await editor(container);
    expect(view.contentDOM.getAttribute("aria-label")).toBe("Reply to Carol");
    expect(view.contentDOM.getAttribute("role")).toBe("textbox");
  });

  it("does not resurrect the draft when an autosave lands after publishing", async () => {
    const engine = new MockEngine();
    const before = await engine.drafts();
    const gate = deferred<void>();
    const realSave = engine.saveDraft.bind(engine);
    const save = vi.spyOn(engine, "saveDraft").mockImplementation(async (d) => {
      await gate.promise;
      return realSave(d);
    });
    const remove = vi.spyOn(engine, "deleteDraft");
    const onSubmit = vi.fn(async () => true);
    const { container } = renderWithApp(
      <Composer label="Comment" submitLabel="Publish" makeDraft={makeDraft} onSubmit={onSubmit} />,
      { engine },
    );
    type(await editor(container), "A comment that is published quickly.");
    // The autosave starts after its debounce and stays in flight.
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1), { timeout: 3000 });
    fireEvent.click(screen.getByRole("button", { name: "Publish" }));
    // the second argument is the composer's attachment rows (empty here, M4)
    await waitFor(() =>
      expect(onSubmit).toHaveBeenCalledWith("A comment that is published quickly.", []),
    );
    gate.resolve();
    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
    const id = await save.mock.results[0]?.value;
    expect(remove).toHaveBeenCalledWith(id);
    expect(await engine.drafts()).toEqual(before);
  });

  it("keeps the draft when the composer goes away without publishing", async () => {
    const engine = new MockEngine();
    const before = (await engine.drafts()).length;
    const remove = vi.spyOn(engine, "deleteDraft");
    const { container, unmount } = renderWithApp(
      <Composer label="Comment" makeDraft={makeDraft} onSubmit={async () => true} />,
      { engine },
    );
    type(await editor(container), "Unfinished thought");
    await waitFor(async () => expect(await engine.drafts()).toHaveLength(before + 1), {
      timeout: 3000,
    });
    unmount();
    expect(remove).not.toHaveBeenCalled();
    const kept = (await engine.drafts()).filter(
      (d) => d.draft.kind === "comment" && d.draft.comment.body.markdown === "Unfinished thought",
    );
    expect(kept).toHaveLength(1);
  });
});
