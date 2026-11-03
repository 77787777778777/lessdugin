// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum directory comes grouped by owner (engine-api.md §3.2 `forum_discover`, sync.md
// §4.7): the home page lists each owner's forums under a heading naming the owner.
import { MockEngine } from "@lessdugin/engine/mock";
import type { ActorView, ForumSummary } from "@lessdugin/engine/types";
import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { HomePage, ownerGroups } from "./HomePage.tsx";

const ADA: ActorView = {
  did: "did:plc:adaadaadaadaadaadaadaada",
  handle: "ada.test",
  handleState: "verified",
  shortDid: "did:plc:adaa…",
};
const BOB: ActorView = {
  did: "did:plc:bobbobbobbobbobbobbobbob",
  handle: "bob.test",
  handleState: "verified",
  shortDid: "did:plc:bobb…",
};

function forum(owner: ActorView, slug: string, name: string): ForumSummary {
  return {
    uri: `at://${owner.did}/online.7778777.lessdugin.temp.forum/${slug}`,
    owner,
    name,
    membership: "open",
    followed: false,
    member: false,
  };
}

describe("the forum directory", () => {
  it("groups runs of one owner's forums", () => {
    const a1 = forum(ADA, "a1", "Alpha");
    const a2 = forum(ADA, "a2", "Beta");
    const b1 = forum(BOB, "b1", "Gamma");
    expect(ownerGroups([a1, a2, b1])).toEqual([
      { owner: ADA, forums: [a1, a2] },
      { owner: BOB, forums: [b1] },
    ]);
    expect(ownerGroups([])).toEqual([]);
  });

  it("lists each owner's forums under a heading naming the owner", async () => {
    const engine = new MockEngine();
    vi.spyOn(engine, "forumDiscover").mockResolvedValue([
      forum(ADA, "a1", "Alpha"),
      forum(ADA, "a2", "Beta"),
      forum(BOB, "b1", "Lookalike Alpha"),
    ]);
    renderWithApp(<HomePage />, { engine });
    expect(await screen.findByRole("heading", { level: 2, name: "Discover" })).toBeTruthy();
    const owners = screen.getAllByRole("heading", { level: 3 });
    expect(owners.map((h) => h.getAttribute("data-owner"))).toEqual([ADA.did, BOB.did]);
    expect(owners[0]?.textContent).toMatch(/^Forums by .*ada\.test/);
    expect(owners[1]?.textContent).toMatch(/^Forums by .*bob\.test/);
    // each card follows its owner's heading, titled one level below it
    const ada = owners[0]?.parentElement as HTMLElement;
    const bob = owners[1]?.parentElement as HTMLElement;
    expect(
      within(ada)
        .getAllByRole("article")
        .map((a) => a.getAttribute("aria-label")),
    ).toEqual(["Alpha", "Beta"]);
    expect(
      within(bob)
        .getAllByRole("article")
        .map((a) => a.getAttribute("aria-label")),
    ).toEqual(["Lookalike Alpha"]);
    expect(within(bob).getByRole("heading", { level: 4, name: "Lookalike Alpha" })).toBeTruthy();
  });
});
