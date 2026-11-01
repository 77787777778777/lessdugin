// SPDX-License-Identifier: AGPL-3.0-or-later
// Removing a moderator adopts their assertions first; the engine's ticket says so, and says
// which it will not adopt, before anything is sent (engine-api.md §3.2 `mods_set`; review
// finding sec-8, SPA part). A declaration whose about spilled into a blob is not saved from
// here, since the form has no source to send and a draft without `about` removes it
// (robustness-2).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView, Json } from "@lessdugin/engine/types";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../../test/render.tsx";
import { ForumSettingsPage } from "./ForumSettingsPage.tsx";
import { ForumContext } from "./forumContext.tsx";

describe("ForumSettingsPage", () => {
  it("shows the engine's adoption notices when the owner removes a moderator", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
    const mod = view.moderators[0];
    if (mod === undefined) throw new Error("no moderator in the fixture");
    const modsSet = vi.spyOn(engine, "modsSet").mockResolvedValue({
      id: "w9",
      state: { kind: "done", uri: forum },
      notices: [
        { code: "adoptedFirst", message: "mod adopt runs first (write w8, 3 assertion(s))" },
        { code: "adoptedFirst", message: "not adopted, since they are not in effect: 1 lapsed" },
      ],
    });
    renderWithApp(
      <ForumContext value={view}>
        <ForumSettingsPage />
      </ForumContext>,
      { engine },
    );
    const row = (await screen.findAllByRole("button", { name: "Remove" }))[0] as HTMLElement;
    fireEvent.click(row);
    const ack = await screen.findByRole("dialog", { name: "Moderation actions are public" });
    fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
    await waitFor(() =>
      expect(modsSet).toHaveBeenCalledWith(forum, {
        kind: "remove",
        did: mod.actor.did,
        skipAdopt: false,
      }),
    );
    expect(await screen.findByText("mod adopt runs first (write w8, 3 assertion(s))")).toBeTruthy();
    expect(screen.getByText("not adopted, since they are not in effect: 1 lapsed")).toBeTruthy();
  });

  it("refuses to save a declaration whose about spilled into a blob (robustness-2)", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
    const real = await engine.raw(forum);
    const record = real.record as { [k: string]: Json };
    const spilled: Json = {
      $type: "online.7778777.lessdugin.temp.markdown",
      textBlob: {
        $type: "blob",
        ref: { $link: "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy" },
        mimeType: "text/markdown",
        size: 70_000,
      },
    };
    vi.spyOn(engine, "raw").mockResolvedValue({ ...real, record: { ...record, about: spilled } });
    const update = vi.spyOn(engine, "forumUpdate");
    renderWithApp(
      <ForumContext value={view}>
        <ForumSettingsPage />
      </ForumContext>,
      { engine },
    );
    fireEvent.click(await screen.findByRole("button", { name: "Edit the forum" }));
    const about = (await screen.findByRole("textbox", { name: /^About/ })) as HTMLTextAreaElement;
    expect(about.disabled).toBe(true);
    expect(screen.getByTestId("about-unavailable").textContent).toMatch(/stored in a blob/);
    const save = screen.getByRole("button", { name: "Save the declaration" }) as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    // a submit that gets past the button still sends nothing
    fireEvent.submit(save.closest("form") as HTMLFormElement);
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.some((a) => /stored in a blob/.test(a.textContent ?? ""))).toBe(true);
    expect(update).not.toHaveBeenCalled();
  });
});
