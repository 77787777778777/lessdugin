// SPDX-License-Identifier: AGPL-3.0-or-later
// Until the engine has the `user` operation (M5), a profile shows the identity with subscribe
// and mute, so muting is reachable (review finding m3-11).
import { MockEngine } from "@lessdugin/engine/mock";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { actorIdent } from "../components/ActorName.tsx";
import { renderWithApp } from "../test/render.tsx";
import { UserProfile } from "./UserPage.tsx";

describe("UserProfile without the user op", () => {
  it("shows the identity and mutes from it", async () => {
    const engine = new MockEngine();
    const forum = (await engine.followed())[0]?.uri ?? "";
    const me = (await engine.activeAccount())?.did;
    const list = await engine.listPosts({ forum, sort: "top", filter: "all", limit: 50 });
    const other = list.items.find((p) => p.author.did !== me)?.author;
    if (!other) throw new Error("no other author");
    vi.spyOn(engine, "user").mockRejectedValue({
      code: "unsupported",
      message: "user: M5",
      retryable: false,
    });
    const mute = vi.spyOn(engine, "mute");
    renderWithApp(<UserProfile actor={other.did} />, { engine });
    expect(await screen.findByText(other.did)).toBeTruthy();
    expect(screen.getAllByText(actorIdent(other)).length).toBeGreaterThan(0);
    fireEvent.click(await screen.findByRole("button", { name: "Mute (local)" }));
    await waitFor(() => expect(mute).toHaveBeenCalledWith(other.did, true));
    expect(await screen.findByRole("button", { name: "Unmute" })).toBeTruthy();
  });
});
