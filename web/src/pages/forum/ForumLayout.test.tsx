// SPDX-License-Identifier: AGPL-3.0-or-later
// The forum header's membership actions: a member leaves after confirming (engine-api.md §3.2
// `leave`, social.md §3 "Leaving"; review finding conformance-2).
import { MockEngine } from "@lessdugin/engine/mock";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { parseAtUri } from "../../lib/uri.ts";
import { renderWithApp } from "../../test/render.tsx";
import { ForumLayout } from "./ForumLayout.tsx";

async function openForum(engine: MockEngine) {
  const forum = (await engine.followed())[0];
  if (!forum) throw new Error("no forum in the fixture");
  const owner = forum.owner.handle ?? forum.owner.did;
  const slug = parseAtUri(forum.uri)?.rkey ?? "";
  renderWithApp(<ForumLayout />, {
    engine,
    route: { path: "/f/$owner/$slug", at: `/f/${owner}/${slug}` },
  });
  await screen.findByRole("heading", { level: 1, name: forum.name });
  return forum;
}

describe("ForumLayout membership", () => {
  it("lets a member leave after confirming, and reports the write", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const leave = vi.spyOn(engine, "leave");
    const forum = await openForum(engine);
    // the fixture's account is a member: Join is not offered, Leave is
    expect(screen.queryByRole("button", { name: "Join" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Leave" }));
    const dialog = await screen.findByRole("dialog", { name: "Leave this forum?" });
    expect(leave).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByText("Leave this forum?")).toBeNull());
    expect(leave).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Leave" }));
    const again = await screen.findByRole("dialog", { name: "Leave this forum?" });
    fireEvent.click(within(again).getByRole("button", { name: "Leave" }));
    await waitFor(() => expect(leave).toHaveBeenCalledWith(forum.uri));
    expect(await screen.findByText("Left the forum")).toBeTruthy();
  });

  it("offers Leave only to a signed-in member", async () => {
    const engine = new MockEngine({ signedOut: true });
    await openForum(engine);
    expect(screen.queryByRole("button", { name: "Leave" })).toBeNull();
    expect(screen.getByRole("link", { name: "Sign in to join" })).toBeTruthy();
  });
});
