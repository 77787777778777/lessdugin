// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
// Before an author edits an item that carries positive assertions, the client warns that the
// edit suspends them (social.md §4.5; review finding sec-7).
import { MockEngine } from "@lessdugin/engine/mock";
import type { CommentNode, Json } from "@lessdugin/engine/types";
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

  it("warns that the edit suspends a moderator's keep", async () => {
    const engine = new MockEngine();
    const { node, post } = await ownComment(engine);
    const real = await engine.raw(node.uri);
    const keep: Json = {
      uri: "at://did:plc:moderatormoderatormoderat/online.7778777.lessdugin.temp.modAction/k",
      cid: "bafyrei",
      valid: true,
      status: "effective",
      record: {
        $type: "online.7778777.lessdugin.temp.modAction",
        subject: node.uri,
        action: { $type: "online.7778777.lessdugin.temp.modAction#hide", hidden: false },
      },
    };
    vi.spyOn(engine, "raw").mockResolvedValue({ ...real, assertions: [keep] });
    renderWithApp(<OwnCommentActions node={node} postUri={post} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await waitFor(() =>
      expect(screen.getByTestId("edit-suspends").textContent).toMatch(
        /suspends a moderator's keep/,
      ),
    );
  });

  it("says nothing when the comment carries no positive assertion", async () => {
    const engine = new MockEngine();
    const { node, post } = await ownComment(engine);
    const real = await engine.raw(node.uri);
    vi.spyOn(engine, "raw").mockResolvedValue({ ...real, assertions: [] });
    renderWithApp(<OwnCommentActions node={node} postUri={post} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    // the composer loads lazily
    await screen.findByRole("button", { name: "Save edit" }, { timeout: 10_000 });
    expect(screen.queryByTestId("edit-suspends")).toBeNull();
  });
});
