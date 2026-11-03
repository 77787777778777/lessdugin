// SPDX-License-Identifier: AGPL-3.0-or-later
// A post opened from a sequence is read with that sequence as its context (engine-api.md §3.4
// `get_post(uri, sequence)`, social.md §14, review finding m5-acceptance-8): its next and
// previous links follow the sequence, and the sequence page offers "Continue reading" at the
// post last opened from it.
import { MockEngine } from "@lessdugin/engine/mock";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createQueryClient } from "../App.tsx";
import { parseAtUri } from "../lib/uri.ts";
import { renderWithApp } from "../test/render.tsx";
import { PostPage } from "./PostPage.tsx";
import { SequencePage } from "./SequencePage.tsx";

/** An engine with a second sequence, "Mine", listing the fixture sequence's three posts in
 * another order. */
async function withMine() {
  const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
  const forum = (await engine.followed())[0]?.uri ?? "";
  const theirs = (await engine.sequences(forum))[0]?.uri ?? "";
  const [map1, map2, confusion] = (await engine.sequence(theirs)).chapters
    .flatMap((c) => c.posts)
    .map((p) => p.uri);
  if (!map1 || !map2 || !confusion) throw new Error("the fixture sequence changed");
  const ticket = await engine.seqCreate({
    forum,
    title: "Mine",
    chapters: [{ posts: [confusion, map2, map1] }],
  });
  let mine = "";
  await waitFor(async () => {
    const s = (await engine.writes()).find((w) => w.id === ticket.id)?.state;
    if (s?.kind !== "done") throw new Error("not done yet");
    mine = s.uri;
  });
  return { engine, mine, theirs, map1, map2, confusion };
}

function renderPost(engine: MockEngine, uri: string, sequence?: string) {
  const p = parseAtUri(uri);
  const search = sequence !== undefined ? `?seq=${encodeURIComponent(sequence)}` : "";
  return renderWithApp(<PostPage />, {
    engine,
    route: { path: "/p/$author/$rkey", at: `/p/${p?.did}/${p?.rkey}${search}` },
  });
}

/** The `seq` search parameter of a link's target. */
function seqOf(link: HTMLElement): string | null {
  const href = link.getAttribute("href") ?? "";
  return new URLSearchParams(href.split("?")[1] ?? "").get("seq");
}

describe("a post opened from a sequence", () => {
  it("navigates that sequence and becomes its continue-reading position", async () => {
    const { engine, mine, map1, map2, confusion } = await withMine();
    const getPost = vi.spyOn(engine, "getPost");
    renderPost(engine, map2, mine);
    const nav = await screen.findByRole("navigation", { name: "Sequence: Mine" });
    expect(screen.queryByRole("navigation", { name: "Sequence: Map and Territory" })).toBeNull();
    const prev = within(nav).getByRole("link", { name: "← Previous" });
    const next = within(nav).getByRole("link", { name: "Next →" });
    // next and previous carry the sequence, so the next post follows it too
    expect(prev.getAttribute("href")).toContain(parseAtUri(confusion)?.rkey);
    expect(next.getAttribute("href")).toContain(parseAtUri(map1)?.rkey);
    expect(seqOf(prev)).toBe(mine);
    expect(seqOf(next)).toBe(mine);
    expect(getPost).toHaveBeenCalledWith(map2, mine);
    expect((await engine.sequence(mine)).continueReading?.post).toBe(map2);
  });

  // the read records the position, so it runs on every open, even while the app's query cache
  // still holds the view (App.tsx `createQueryClient`, staleTime 30 s)
  it("moves the position back when a post is opened again from the same sequence", async () => {
    const { engine, mine, map2, confusion } = await withMine();
    const getPost = vi.spyOn(engine, "getPost");
    const p = parseAtUri(confusion);
    const { router } = renderWithApp(<PostPage />, {
      engine,
      queryClient: createQueryClient(),
      route: {
        path: "/p/$author/$rkey",
        at: `/p/${p?.did}/${p?.rkey}?seq=${encodeURIComponent(mine)}`,
      },
    });
    const first = await screen.findByRole("navigation", { name: "Sequence: Mine" });
    await waitFor(async () =>
      expect((await engine.sequence(mine)).continueReading?.post).toBe(confusion),
    );
    fireEvent.click(within(first).getByRole("link", { name: "Next →" }));
    await waitFor(() => expect(router.state.location.pathname).toContain(parseAtUri(map2)?.rkey));
    await waitFor(async () =>
      expect((await engine.sequence(mine)).continueReading?.post).toBe(map2),
    );
    const second = await screen.findByRole("navigation", { name: "Sequence: Mine" });
    await waitFor(() =>
      expect(
        within(second).getByRole("link", { name: "← Previous" }).getAttribute("href"),
      ).toContain(p?.rkey),
    );
    fireEvent.click(within(second).getByRole("link", { name: "← Previous" }));
    await waitFor(() => expect(router.state.location.pathname).toContain(p?.rkey));
    await waitFor(() => expect(getPost).toHaveBeenCalledTimes(3));
    expect(getPost.mock.calls.map((c) => [c[0], c[1]])).toEqual([
      [confusion, mine],
      [map2, mine],
      [confusion, mine],
    ]);
    await waitFor(async () =>
      expect((await engine.sequence(mine)).continueReading?.post).toBe(confusion),
    );
  });

  it("without a sequence keeps the view's own navigation, whose links carry it", async () => {
    const { engine, mine, theirs, map2 } = await withMine();
    const getPost = vi.spyOn(engine, "getPost");
    renderPost(engine, map2);
    const nav = await screen.findByRole("navigation", { name: "Sequence: Map and Territory" });
    expect(seqOf(within(nav).getByRole("link", { name: "Next →" }))).toBe(theirs);
    expect(getPost).not.toHaveBeenCalled();
    expect((await engine.sequence(mine)).continueReading).toBeUndefined();
  });
});

describe("the sequence page", () => {
  it("opens its posts with the sequence as their context", async () => {
    const { engine, mine } = await withMine();
    const p = parseAtUri(mine);
    renderWithApp(<SequencePage />, {
      engine,
      route: { path: "/s/$author/$rkey", at: `/s/${p?.did}/${p?.rkey}` },
    });
    // the post rows, inside the page's own article
    const items = (await screen.findAllByRole("article")).filter(
      (a) => a.querySelector("article") === null,
    );
    expect(items).toHaveLength(3);
    for (const item of items) {
      for (const link of within(item)
        .getAllByRole("link")
        .filter((l) => seqOf(l) !== null)) {
        expect(seqOf(link)).toBe(mine);
      }
      expect(
        within(item)
          .getAllByRole("link")
          .some((l) => seqOf(l) === mine),
      ).toBe(true);
    }
    expect(screen.queryByTestId("continue-reading")).toBeNull();
  });

  it("offers continue reading at the post last opened from it", async () => {
    const { engine, mine, theirs, map2 } = await withMine();
    await engine.getPost(map2, mine);
    const p = parseAtUri(mine);
    renderWithApp(<SequencePage />, {
      engine,
      route: { path: "/s/$author/$rkey", at: `/s/${p?.did}/${p?.rkey}` },
    });
    const resume = await screen.findByTestId("continue-reading");
    const link = within(resume).getByRole("link", {
      name: "Continue reading: Map and Territory, part 2: updating",
    });
    expect(link.getAttribute("href")).toContain(parseAtUri(map2)?.rkey);
    expect(seqOf(link)).toBe(mine);
    // another sequence's position is its own
    renderWithApp(<SequencePage />, {
      engine,
      route: {
        path: "/s/$author/$rkey",
        at: `/s/${parseAtUri(theirs)?.did}/${parseAtUri(theirs)?.rkey}`,
      },
    });
    await waitFor(() => expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(2));
    expect(screen.getAllByTestId("continue-reading")).toHaveLength(1);
  });
});
