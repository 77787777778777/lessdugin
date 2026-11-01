// SPDX-License-Identifier: AGPL-3.0-or-later
// Removing a moderator adopts their assertions first; the engine's ticket says so, and says
// which it will not adopt, before anything is sent (engine-api.md §3.2 `mods_set`; review
// finding sec-8, SPA part). A declaration whose about spilled into a blob is edited from its
// bodySource, or refused while that source is unavailable (robustness-2).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView } from "@lessdugin/engine/types";
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

  it("loads a spilled about's source before showing the settings form (robustness-2)", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0, spilledBodies: true });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
    const expected = await engine.bodySource(forum);
    expect(expected.spilled).toBe(true);
    const source = vi.spyOn(engine, "bodySource");
    renderWithApp(
      <ForumContext value={view}>
        <ForumSettingsPage />
      </ForumContext>,
      { engine },
    );
    fireEvent.click(await screen.findByRole("button", { name: "Edit the forum" }));
    const about = (await screen.findByRole("textbox", { name: /^About/ })) as HTMLTextAreaElement;
    await waitFor(() => expect(about.value).toBe(expected.markdown));
    expect(about.disabled).toBe(false);
    expect(source).toHaveBeenCalledWith(forum);
    expect(screen.queryByTestId("about-unavailable")).toBeNull();
    expect(
      (screen.getByRole("button", { name: "Save the declaration" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("refuses to save a declaration whose spilled source is unavailable (robustness-2)", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0, spilledBodies: true });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
    vi.spyOn(engine, "bodySource").mockRejectedValue({
      code: "network",
      message: "The source is not available.",
      retryable: true,
    });
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
