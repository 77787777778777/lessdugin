// SPDX-License-Identifier: AGPL-3.0-or-later
// The settings page writes each setting through the operation the engine keeps it with, and
// shows a refusal instead of dropping it (review finding m3-5).
import { MockEngine } from "@lessdugin/engine/mock";
import { FORUM_URI } from "@lessdugin/engine/mock/fixture";
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
});
