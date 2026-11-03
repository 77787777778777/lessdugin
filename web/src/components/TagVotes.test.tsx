// SPDX-License-Identifier: AGPL-3.0-or-later
// Tag votes show the viewer's recorded vote from the view (`PostView.tagRelevance[].mine`,
// engine-api.md §4.2, review finding m5-acceptance-12), not one kept in client memory, with each
// tag's relevance and whether it applies (social.md §13.3).
import { MockEngine } from "@lessdugin/engine/mock";
import type { WriteTicket } from "@lessdugin/engine/types";
import { useQuery } from "@tanstack/react-query";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useEngine } from "../engine/context.tsx";
import { qk } from "../engine/queries.ts";
import { renderWithApp } from "../test/render.tsx";
import { TagVotes } from "./TagVotes.tsx";

const WELCOME = "Welcome to LessDugin: a tour of the forum";

/** The post page's wiring: the view read with `getPost`, refreshed by its topics. */
function Post({ uri }: { uri: string }) {
  const engine = useEngine();
  const q = useQuery({ queryKey: qk.post(uri), queryFn: () => engine.getPost(uri) });
  if (!q.data) return null;
  return (
    <TagVotes
      post={uri}
      forum={q.data.post.forum}
      applied={q.data.post.tags}
      relevance={q.data.tagRelevance}
      signedIn
    />
  );
}

async function welcome(engine: MockEngine): Promise<string> {
  const forum = (await engine.followed())[0]?.uri ?? "";
  const posts = await engine.listPosts({ forum, sort: "new", filter: "all", limit: 100 });
  return posts.items.find((p) => p.title === WELCOME)?.uri ?? "";
}

async function settled(engine: MockEngine, ticket: WriteTicket): Promise<void> {
  await waitFor(async () => expect((await engine.writeStatus(ticket.id))?.state.kind).toBe("done"));
}

function row(name: string): HTMLElement {
  const panel = screen.getByRole("region", { name: "Tag votes" });
  const item = within(panel)
    .getAllByRole("listitem")
    .find((li) => li.textContent?.startsWith(name));
  if (!item) throw new Error(`no tag row ${name}`);
  return item;
}

const pressed = (item: HTMLElement, name: RegExp): string | null =>
  within(item).getByRole("button", { name }).getAttribute("aria-pressed");

describe("TagVotes", () => {
  it("shows the vote the viewer recorded before the page opened, with relevance", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const uri = await welcome(engine);
    // recorded earlier (another page, another client): a strong vote presses its direction
    await settled(engine, await engine.opinion(uri, { kind: "tag", slug: "meta", value: "down" }));
    await settled(
      engine,
      await engine.opinion(uri, { kind: "tag", slug: "practical", value: "strongUp" }),
    );
    renderWithApp(<Post uri={uri} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Vote on tags" }));
    await waitFor(() => expect(row("Site meta")).toBeTruthy());
    expect(pressed(row("Site meta"), /^Does not apply$/)).toBe("true");
    expect(pressed(row("Site meta"), /^Applies$/)).toBe("false");
    // the author's declared tag, voted down by an authority: relevance 0, no longer applied
    expect(row("Site meta").textContent).toContain("relevance 0 · does not apply");
    expect(pressed(row("Practical"), /^Applies$/)).toBe("true");
    expect(row("Practical").textContent).toContain("relevance 3 · applies");
    expect(pressed(row("Rationality"), /^Applies$/)).toBe("false");
    expect(row("Rationality").textContent).toContain("relevance 1 · applies");
  });

  it("shows a vote in flight, then the recorded one; an undone vote goes back", async () => {
    const engine = new MockEngine({ holdMs: 200, sendMs: 0 });
    await engine.acknowledgePublicAction("tagVote");
    const uri = await welcome(engine);
    renderWithApp(<Post uri={uri} />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Vote on tags" }));
    await waitFor(() => expect(row("Rationality")).toBeTruthy());
    fireEvent.click(within(row("Rationality")).getByRole("button", { name: /^Applies$/ }));
    await waitFor(() => expect(pressed(row("Rationality"), /^Applies$/)).toBe("true"));
    // recorded: the view says so once the write is done
    await waitFor(async () =>
      expect(
        (await engine.getPost(uri)).tagRelevance?.find((t) => t.slug === "rationality")?.mine,
      ).toBe("up"),
    );
    await waitFor(() => expect(row("Rationality").textContent).toContain("relevance 2"));
    expect(pressed(row("Rationality"), /^Applies$/)).toBe("true");
    // a vote undone in its window: the button shows the recorded vote again
    fireEvent.click(within(row("Rationality")).getByRole("button", { name: /^Applies$/ }));
    await waitFor(() => expect(pressed(row("Rationality"), /^Applies$/)).toBe("false"));
    // the toast of this vote, the newest one
    const undo = await screen.findAllByRole("button", { name: "Undo" });
    fireEvent.click(undo[undo.length - 1] as HTMLElement);
    expect(await screen.findByText("Tag vote undone")).toBeTruthy();
    await waitFor(() => expect(pressed(row("Rationality"), /^Applies$/)).toBe("true"));
    expect(
      (await engine.getPost(uri)).tagRelevance?.find((t) => t.slug === "rationality")?.mine,
    ).toBe("up");
  });
});
