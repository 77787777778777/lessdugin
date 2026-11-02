// SPDX-License-Identifier: AGPL-3.0-or-later
// The SPA creates and edits sequences (engine-api.md §3.5 `seq_*`, review finding
// m3m4-continuation-3). The author moves, removes and adds entries on the sequence page at the
// record's positions (the view leaves out what it cannot display), and the editor keeps the
// entries the view does not show.
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView, SequenceDraft, WriteTicket } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { parseAtUri } from "../lib/uri.ts";
import { renderWithApp } from "../test/render.tsx";
import { ForumNewSequencePage } from "./forum/ForumNewSequencePage.tsx";
import { ForumContext } from "./forum/forumContext.tsx";
import { EditSequence } from "./SequenceEditPage.tsx";
import { SequencePage } from "./SequencePage.tsx";

async function done(engine: MockEngine, ticket: WriteTicket): Promise<string> {
  let uri = "";
  await waitFor(async () => {
    const s = (await engine.writes()).find((w) => w.id === ticket.id)?.state;
    if (s?.kind !== "done") throw new Error("not done yet");
    uri = s.uri;
  });
  return uri;
}

/** An own sequence: chapter 1 lists three posts with an unavailable entry between the first two,
 * chapter 2 one post. */
async function ownSequence() {
  const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
  const forum = (await engine.followed())[0]?.uri ?? "";
  const posts = (await engine.listPosts({ forum, sort: "new", filter: "all", limit: 100 })).items;
  const [a, b, c, d, extra] = posts;
  if (!a || !b || !c || !d || !extra) throw new Error("not enough fixture posts");
  const me = (await engine.activeAccount())?.did ?? "";
  const gone = `at://${me}/online.7778777.lessdugin.post/${parseAtUri(a.uri)?.rkey}x`;
  const uri = await done(
    engine,
    await engine.seqCreate({
      forum,
      title: "Own reading list",
      chapters: [
        { title: "First", posts: [a.uri, gone, b.uri, c.uri] },
        { title: "Second", posts: [d.uri] },
      ],
    }),
  );
  return { engine, forum, uri, posts: { a, b, c, d, extra }, gone };
}

function renderSequence(engine: MockEngine, uri: string) {
  const p = parseAtUri(uri);
  return renderWithApp(<SequencePage />, {
    engine,
    route: { path: "/s/$author/$rkey", at: `/s/${p?.did}/${p?.rkey}` },
  });
}

/** A control's name for a post: the title (or excerpt) starts it, cut to its first words. */
const named = (verb: string, p: { title?: string; excerpt: string }, tail: string): RegExp => {
  const start = (p.title ?? p.excerpt).slice(0, 24).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^${verb} ${start}.*${tail}$`);
};

describe("SequencePage, the author's controls", () => {
  it("moves an entry past the one the view leaves out (seq_move at record positions)", async () => {
    const { engine, uri, posts } = await ownSequence();
    const move = vi.spyOn(engine, "seqMove");
    renderSequence(engine, uri);
    expect(await screen.findByRole("link", { name: "Edit sequence" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: named("Move", posts.b, " up") }));
    await waitFor(() =>
      expect(move).toHaveBeenCalledWith(uri, { chapter: 0, index: 2 }, { chapter: 0, index: 0 }),
    );
    await waitFor(async () =>
      expect((await engine.sequence(uri)).chapters[0]?.posts.map((p) => p.uri)).toEqual([
        posts.b.uri,
        posts.a.uri,
        posts.c.uri,
      ]),
    );
    // the last entry of a chapter moves down into the next one
    fireEvent.click(await screen.findByRole("button", { name: named("Move", posts.c, " down") }));
    await waitFor(() =>
      expect(move).toHaveBeenLastCalledWith(
        uri,
        { chapter: 0, index: 3 },
        { chapter: 1, index: 0 },
      ),
    );
  });

  it("removes an entry and adds a post at the end of a chapter", async () => {
    const { engine, uri, posts } = await ownSequence();
    const remove = vi.spyOn(engine, "seqRemove");
    const add = vi.spyOn(engine, "seqAdd");
    renderSequence(engine, uri);
    fireEvent.click(
      await screen.findByRole("button", { name: named("Remove", posts.a, " from the sequence") }),
    );
    await waitFor(() => expect(remove).toHaveBeenCalledWith(uri, posts.a.uri));
    const second = await screen.findByRole("region", { name: "Second" });
    fireEvent.change(within(second).getByRole("textbox", { name: "Add a post to Second" }), {
      target: { value: posts.extra.uri },
    });
    fireEvent.click(within(second).getByRole("button", { name: "Add" }));
    await waitFor(() =>
      expect(add).toHaveBeenCalledWith(uri, posts.extra.uri, { chapter: 1, index: 1 }),
    );
    await waitFor(async () =>
      expect((await engine.sequence(uri)).chapters[1]?.posts.map((p) => p.uri)).toEqual([
        posts.d.uri,
        posts.extra.uri,
      ]),
    );
  });

  it("shows no controls on someone else's sequence", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const theirs = (await engine.sequences(forum))[0]?.uri ?? "";
    renderSequence(engine, theirs);
    await screen.findAllByRole("article");
    expect(screen.queryByRole("link", { name: "Edit sequence" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Move / })).toBeNull();
    expect(screen.queryByRole("textbox", { name: /^Add a post/ })).toBeNull();
  });
});

describe("the sequence editor", () => {
  it("saves an edit from the record, keeping the entries the view does not show", async () => {
    const { engine, forum, uri, posts, gone } = await ownSequence();
    const edit = vi.spyOn(engine, "seqEdit");
    renderWithApp(<EditSequence uri={uri} />, { engine });
    const title = (await screen.findByRole("textbox", { name: "Title" })) as HTMLInputElement;
    expect(title.value).toBe("Own reading list");
    fireEvent.change(title, { target: { value: "Renamed list" } });
    fireEvent.click(screen.getByRole("button", { name: "Save sequence" }));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(1));
    expect(edit.mock.calls[0]?.[0]).toBe(uri);
    expect(edit.mock.calls[0]?.[1]).toEqual({
      forum,
      title: "Renamed list",
      chapters: [
        { title: "First", posts: [posts.a.uri, gone, posts.b.uri, posts.c.uri] },
        { title: "Second", posts: [posts.d.uri] },
      ],
    } satisfies SequenceDraft);
  });

  it("creates a sequence with a post added by its address", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view: ForumView = await engine.forumOpen(forum);
    const post = (await engine.listPosts({ forum, sort: "new", filter: "all", limit: 1 })).items[0];
    const create = vi.spyOn(engine, "seqCreate");
    renderWithApp(
      <ForumContext value={view}>
        <ForumNewSequencePage />
      </ForumContext>,
      { engine },
    );
    fireEvent.change(await screen.findByRole("textbox", { name: "Title" }), {
      target: { value: "A new list" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Add a post to chapter 1" }), {
      target: { value: post?.uri },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByRole("list", { name: "Posts of chapter 1" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Create sequence" }));
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith({
        forum,
        title: "A new list",
        chapters: [{ posts: [post?.uri] }],
      }),
    );
  });
});
