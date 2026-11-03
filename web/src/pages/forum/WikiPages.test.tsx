// SPDX-License-Identifier: AGPL-3.0-or-later
// The SPA writes wiki pages (engine-api.md §3.5 `wiki_edit`, review finding m3m4-continuation-3):
// an edit starts from the canonical revision and names it as `base` (decision O-2), a refusal
// because the head moved says so, a new page is written without a parent, and the history walks
// the revisions from the head (social.md §13.2).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView, RawView, WikiDraft } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../../test/render.tsx";
import { ForumContext, forumParams } from "./forumContext.tsx";
import {
  CARRIED_NOTE,
  HEAD_MOVED_MESSAGE,
  NEW_PAGE_NOTE,
  NOT_A_TAG_WARNING,
  WikiEditPage,
} from "./WikiEditPage.tsx";
import { WikiHistoryPage } from "./WikiHistoryPage.tsx";

const SLUG = "decision-theory";

async function setup(
  page: "edit" | "history",
  tag = SLUG,
  engine = new MockEngine({ holdMs: 0, sendMs: 0 }),
) {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const view: ForumView = await engine.forumOpen(forum);
  const { owner, slug } = forumParams(forum);
  const Page = page === "edit" ? WikiEditPage : WikiHistoryPage;
  const rendered = renderWithApp(
    <ForumContext value={view}>
      <Page />
    </ForumContext>,
    {
      engine,
      route: { path: `/f/$owner/$slug/t/$tag/${page}`, at: `/f/${owner}/${slug}/t/${tag}/${page}` },
    },
  );
  return { ...rendered, engine, forum };
}

describe("WikiEditPage", () => {
  it("edits from the canonical revision and names it as the base", async () => {
    const { engine, forum } = await setup("edit");
    const head = (await engine.tag(forum, SLUG)).head;
    expect(head).toBeDefined();
    const edit = vi.spyOn(engine, "wikiEdit");
    const name = (await screen.findByRole("textbox", { name: "Name" })) as HTMLInputElement;
    expect(name.value).toBe("Decision theory");
    const body = screen.getByRole("textbox", { name: /^Page/ }) as HTMLTextAreaElement;
    expect(body.value).toContain("## Classic problems");
    fireEvent.change(body, { target: { value: `${body.value}\n- Parfit's hitchhiker` } });
    fireEvent.change(screen.getByRole("textbox", { name: /Edit summary/ }), {
      target: { value: "Add a problem" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save page" }));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(1));
    expect(edit.mock.calls[0]?.[0]).toBe(forum);
    expect(edit.mock.calls[0]?.[1]).toBe(SLUG);
    expect(edit.mock.calls[0]?.[2]).toMatchObject({
      name: "Decision theory",
      description: "How agents should choose between actions.",
      isTag: true,
      parentTags: [],
      summary: "Add a problem",
      base: head,
    });
    expect(edit.mock.calls[0]?.[2].body?.markdown).toContain("Parfit's hitchhiker");
    // the new revision is the head, and its only parent is the one the edit started from
    await waitFor(async () => expect((await engine.tag(forum, SLUG)).head).not.toBe(head));
    const next = (await engine.tag(forum, SLUG)).head ?? "";
    expect((await engine.raw(next)).record).toMatchObject({ parents: [{ uri: head }] });
  });

  it("says when the head moved since the page was opened, with a reload", async () => {
    const { engine, forum } = await setup("edit");
    const head = (await engine.tag(forum, SLUG)).head;
    vi.spyOn(engine, "wikiEdit").mockRejectedValue({
      code: "conflict",
      message: "the page head moved",
      retryable: false,
      detail: { kind: "wikiHeadMoved", base: head ?? "", head: "at://did:plc:other/x/y" },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Save page" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(HEAD_MOVED_MESSAGE);
    expect(within(alert).getByRole("button", { name: "Reload the page" })).toBeTruthy();
  });

  // decision O-2 on the client side: the author's text is never lost to a newer head, and the
  // reload offers the newer version with their text beside it
  it("keeps the author's text when the save meets a newer head, and redoes it after a reload", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0, wikiRace: true });
    const { forum } = await setup("edit", SLUG, engine);
    const head = (await engine.tag(forum, SLUG)).head;
    const edit = vi.spyOn(engine, "wikiEdit");
    const body = (await screen.findByRole("textbox", { name: /^Page/ })) as HTMLTextAreaElement;
    const mine = `${body.value}\n- Parfit's hitchhiker`;
    fireEvent.change(body, { target: { value: mine } });
    fireEvent.click(screen.getByRole("button", { name: "Save page" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(HEAD_MOVED_MESSAGE);
    expect(edit.mock.calls[0]?.[2]).toMatchObject({ base: head });
    // the refusal keeps the text, even once the newer head reached the view
    const moved = (await engine.tag(forum, SLUG)).head;
    expect(moved).not.toBe(head);
    await waitFor(() => expect(screen.getAllByText(HEAD_MOVED_MESSAGE).length).toBeGreaterThan(0));
    expect((screen.getByRole("textbox", { name: /^Page/ }) as HTMLTextAreaElement).value).toBe(
      mine,
    );
    fireEvent.click(within(alert).getByRole("button", { name: "Reload the page" }));
    // the editor shows the newer version, the author's text beside it
    const kept = await screen.findByRole("region", { name: "Your unsaved edit" });
    expect(kept.textContent).toContain(CARRIED_NOTE);
    expect(
      (within(kept).getByRole("textbox", { name: "Your unsaved text" }) as HTMLTextAreaElement)
        .value,
    ).toBe(mine);
    const fresh = screen.getByRole("textbox", { name: /^Page/ }) as HTMLTextAreaElement;
    expect(fresh.value).toContain("An edit made elsewhere");
    expect(fresh.value).not.toContain("Parfit's hitchhiker");
    expect(screen.queryByRole("alert")).toBeNull();
    // redone on the newer head
    fireEvent.click(within(kept).getByRole("button", { name: "Use my text" }));
    expect((screen.getByRole("textbox", { name: /^Page/ }) as HTMLTextAreaElement).value).toBe(
      mine,
    );
    fireEvent.click(screen.getByRole("button", { name: "Save page" }));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(2));
    expect(edit.mock.calls[1]?.[2]).toMatchObject({ base: moved });
    await waitFor(async () => expect((await engine.tag(forum, SLUG)).head).not.toBe(moved));
    const next = (await engine.tag(forum, SLUG)).head ?? "";
    expect((await engine.raw(next)).record).toMatchObject({ parents: [{ uri: moved }] });
  });

  it("keeps the head it loaded while a newer one arrives, and says so", async () => {
    const { engine, forum } = await setup("edit");
    const head = (await engine.tag(forum, SLUG)).head ?? "";
    const body = (await screen.findByRole("textbox", { name: /^Page/ })) as HTMLTextAreaElement;
    fireEvent.change(body, { target: { value: "My own rewrite." } });
    // an edit made elsewhere becomes the head (a live update re-queries the page)
    await engine.wikiEdit(forum, SLUG, {
      name: "Decision theory",
      isTag: true,
      base: head,
      body: { markdown: "Someone else's version.", media: [] },
    });
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain(HEAD_MOVED_MESSAGE);
    expect((screen.getByRole("textbox", { name: /^Page/ }) as HTMLTextAreaElement).value).toBe(
      "My own rewrite.",
    );
    // a save still names the head the form started from, and is refused
    const edit = vi.spyOn(engine, "wikiEdit");
    fireEvent.click(screen.getByRole("button", { name: "Save page" }));
    expect((await screen.findByRole("alert")).textContent).toContain(HEAD_MOVED_MESSAGE);
    expect(edit.mock.calls[0]?.[2]).toMatchObject({ base: head });
  });

  it("writes a new page without a base", async () => {
    const { engine, forum } = await setup("edit", "new-topic");
    const edit = vi.spyOn(engine, "wikiEdit");
    fireEvent.change(await screen.findByRole("textbox", { name: "Name" }), {
      target: { value: "New topic" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: /^Page/ }), {
      target: { value: "What the new topic is." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create page" }));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(1));
    expect(edit).toHaveBeenCalledWith(forum, "new-topic", {
      name: "New topic",
      isTag: true,
      parentTags: [],
      body: { markdown: "What the new topic is.", media: [] },
    });
    await waitFor(async () =>
      expect((await engine.tag(forum, "new-topic")).tag.name).toBe("New topic"),
    );
  });
});

// the engine opens only pages that are tags (views.rs `tag_summary`): a page that is not a tag
// cannot be loaded, so a save there starts from an empty form and replaces its current text
describe("WikiEditPage and pages that are not tags", () => {
  it("warns before a page stops being a tag, then goes to the tags list", async () => {
    const { engine, forum, router } = await setup("edit");
    const head = (await engine.tag(forum, SLUG)).head;
    const edit = vi.spyOn(engine, "wikiEdit");
    const box = await screen.findByRole("checkbox", { name: /This page is a tag/ });
    expect(screen.queryByText(NOT_A_TAG_WARNING)).toBeNull();
    expect(screen.queryByText(NEW_PAGE_NOTE)).toBeNull();
    fireEvent.click(box);
    const warning = screen.getByText(NOT_A_TAG_WARNING);
    expect(box.getAttribute("aria-describedby")).toBe(warning.id);
    fireEvent.click(screen.getByRole("button", { name: "Save page" }));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(1));
    expect(edit.mock.calls[0]?.[2]).toMatchObject({ isTag: false, base: head });
    await waitFor(() =>
      expect(router.state.location.pathname).toMatch(/^\/f\/[^/]+\/[^/]+\/tags$/),
    );
    expect((await engine.tags(forum)).map((t) => t.slug)).not.toContain(SLUG);
  });

  it("says on an address with no tag page that a page that is not a tag is replaced", async () => {
    // a page that stopped being a tag opens as a new address, with the note
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const head = (await engine.tag(forum, SLUG)).head ?? "";
    const draft: WikiDraft = { name: "Glossary", isTag: false, base: head };
    await engine.wikiEdit(forum, SLUG, draft);
    await waitFor(async () =>
      expect((await engine.tags(forum)).map((t) => t.slug)).not.toContain(SLUG),
    );
    await setup("edit", SLUG, engine);
    expect(await screen.findByText(NEW_PAGE_NOTE)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Create page" })).toBeTruthy();
  });
});

describe("WikiHistoryPage", () => {
  it("lists the chain from the canonical revision and the unmerged edits", async () => {
    await setup("history");
    const list = await screen.findByRole("list", { name: "Revisions" });
    const items = within(list).getAllByRole("listitem");
    // carol's edit (current) of ada's genesis; dan's other edit is the unmerged one
    expect(items.map((li) => li.getAttribute("aria-label"))).toEqual(["Revision 2", "Revision 1"]);
    expect(items[0]?.textContent).toContain("(current)");
    expect(items[0]?.textContent).toContain("Classic problems and further reading");
    expect(items[1]?.textContent).toContain("New page");
    expect(screen.getByText(/1 unmerged edit:/)).toBeTruthy();
  });

  // social.md §13.2 `par`: a parent of another page gives no edge, so a longer chain there
  // never becomes this page's history
  it("leaves out a parent of another page, however deep its chain", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const head = (await engine.tag(forum, SLUG)).head ?? "";
    const collection = head.split("/")[3] ?? "";
    const did = head.split("/")[2] ?? "";
    const [x1, x2] = [`at://${did}/${collection}/3kxx1`, `at://${did}/${collection}/3kxx2`];
    const other = (uri: string, parents: string[]): RawView => ({
      dto: 1,
      uri,
      cid: `cid-${uri}`,
      level: "pds",
      record: {
        $type: collection,
        forum,
        slug: "other-page",
        name: "Another page",
        parents: parents.map((p) => ({ uri: p, cid: `cid-${p}` })),
        createdAt: "2026-01-01T00:00:00.000Z",
      },
      validity: [],
      assertions: [],
    });
    const raw = engine.raw.bind(engine);
    vi.spyOn(engine, "raw").mockImplementation(async (uri) => {
      if (uri === x1) return other(x1, [x2]);
      if (uri === x2) return other(x2, []);
      const view = await raw(uri);
      if (uri !== head) return view;
      const record = view.record as { parents: { uri: string; cid: string }[] };
      return {
        ...view,
        record: { ...record, parents: [...record.parents, { uri: x1, cid: `cid-${x1}` }] },
      };
    });
    const view: ForumView = await engine.forumOpen(forum);
    const { owner, slug } = forumParams(forum);
    renderWithApp(
      <ForumContext value={view}>
        <WikiHistoryPage />
      </ForumContext>,
      {
        engine,
        route: {
          path: "/f/$owner/$slug/t/$tag/history",
          at: `/f/${owner}/${slug}/t/${SLUG}/history`,
        },
      },
    );
    const list = await screen.findByRole("list", { name: "Revisions" });
    const items = within(list).getAllByRole("listitem");
    expect(items.map((li) => li.getAttribute("aria-label"))).toEqual(["Revision 2", "Revision 1"]);
    expect(list.textContent).not.toContain("Another page");
  });
});
