// SPDX-License-Identifier: AGPL-3.0-or-later
// The settings page writes each setting through the operation the engine keeps it with, and
// shows a refusal instead of dropping it (review finding m3-5).
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumIndexer, IndexerConsent } from "@lessdugin/engine/types";

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { SettingsPage } from "./SettingsPage.tsx";

describe("SettingsPage", () => {
  // the build-info card fetches the served build-info.json; there is no server here
  beforeEach(() => vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("offline"))));
  afterEach(() => vi.unstubAllGlobals());

  it("sets the karma cadence through setKarmaNotifications", async () => {
    const engine = new MockEngine();
    const cadence = vi.spyOn(engine, "setKarmaNotifications");
    renderWithApp(<SettingsPage />, { engine });
    fireEvent.click(await screen.findByRole("radio", { name: /Weekly/ }));
    await waitFor(() => expect(cadence).toHaveBeenCalledWith("weekly"));
    expect(await engine.karmaNotifications()).toBe("weekly");
    await waitFor(() =>
      expect((screen.getByRole("radio", { name: /Weekly/ }) as HTMLInputElement).checked).toBe(
        true,
      ),
    );
  });

  it("shows a refused setting instead of leaving the rejection unhandled", async () => {
    const engine = new MockEngine();
    vi.spyOn(engine, "setSetting").mockRejectedValue({
      code: "invalid_input",
      message: "verify = proof is not available in direct mode",
      retryable: false,
    });
    renderWithApp(<SettingsPage />, { engine });
    const radios = await screen.findAllByRole("radio");
    const unchecked = radios.find(
      (r) => (r as HTMLInputElement).name === "verify" && !(r as HTMLInputElement).checked,
    );
    if (!unchecked) throw new Error("no verify option to pick");
    fireEvent.click(unchecked);
    expect(await screen.findByText("verify = proof is not available in direct mode")).toBeTruthy();
  });

  it("keeps a manual indexer URL when saving its consent fails", async () => {
    const engine = new MockEngine();
    vi.spyOn(engine, "setSetting").mockRejectedValue(new Error("consent was not saved"));
    renderWithApp(<SettingsPage />, { engine });
    const input = (await screen.findByRole("textbox", { name: "Indexer URL" })) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "https://indexer.example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Consent and add" }));
    expect(await screen.findByText("consent was not saved")).toBeTruthy();
    expect(input.value).toBe("https://indexer.example.com");
    expect((await engine.settings()).indexers).toEqual([]);
  });

  it("keeps forum-declared consent and its revocation pending until persistence succeeds", async () => {
    const engine = new MockEngine();
    const forum = (await engine.followed())[0];
    if (!forum) throw new Error("mock forum is missing");
    const declared: ForumIndexer = {
      url: "https://forum-indexer.example.com",
      did: "did:web:forum-indexer.example.com",
      consented: false,
    };
    vi.spyOn(engine, "forumIndexers").mockResolvedValue([declared]);
    renderWithApp(<SettingsPage />, { engine });
    fireEvent.click(await screen.findByRole("button", { name: "Consent for this forum" }));
    await waitFor(async () => {
      const saved: IndexerConsent[] = (await engine.settings()).indexers;
      expect(saved.some((x) => x.url === declared.url && x.forum === forum.uri)).toBe(true);
    });
    expect(await screen.findByRole("button", { name: "Revoke consent" })).toBeTruthy();

    vi.spyOn(engine, "setSetting").mockRejectedValue(new Error("revoke was not saved"));
    for (const button of await screen.findAllByRole("button", { name: "Revoke consent" })) {
      fireEvent.click(button);
    }
    expect(await screen.findByText("revoke was not saved")).toBeTruthy();
    expect((await engine.settings()).indexers.some((x) => x.url === declared.url)).toBe(true);
  });
});
