// SPDX-License-Identifier: AGPL-3.0-or-later
// AppShell is the one adoption-report presentation, including a direct modAdopt on the mod page.
import { MockEngine } from "@lessdugin/engine/mock";
import type { ForumView } from "@lessdugin/engine/types";
import { act, fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { writeStore } from "../engine/writes.ts";
import { ForumModPage } from "../pages/forum/ForumModPage.tsx";
import { ForumContext } from "../pages/forum/forumContext.tsx";
import { renderWithApp } from "../test/render.tsx";
import { AppShell } from "./AppShell.tsx";

describe("AppShell adoption presentation", () => {
  it("shows exactly one report for a direct adoption and retains a non-page report", async () => {
    const engine = new MockEngine({ holdMs: 0, sendMs: 0 });
    const forum = (await engine.followed())[0]?.uri ?? "";
    const view: ForumView = { ...(await engine.forumOpen(forum)), myRole: "owner" };
    const me = (await engine.activeAccount())?.did;
    if (!me) throw new Error("no active account");
    const lapsed = (await engine.modLog(forum)).entries.find((e) => e.lapsed);
    if (!lapsed) throw new Error("no lapsed entry");
    renderWithApp(
      <>
        <AppShell />
        <ForumContext value={view}>
          <ForumModPage />
        </ForumContext>
      </>,
      { engine },
    );
    fireEvent.change(await screen.findByRole("textbox", { name: /Former moderator/ }), {
      target: { value: lapsed.actor.did },
    });
    fireEvent.click(screen.getByRole("button", { name: "Adopt" }));
    const ack = await screen.findByRole("dialog", { name: "Moderation actions are public" });
    fireEvent.click(within(ack).getByRole("button", { name: "I understand, continue" }));
    const report = await screen.findByTestId("adoption-report");
    expect(report.textContent).toContain(`cannot adopt ${lapsed.uri}: not in effect`);
    expect(screen.getAllByTestId("adoption-report")).toHaveLength(1);
    expect(screen.getAllByText(/cannot adopt .*not in effect/)).toHaveLength(1);
    expect(screen.queryByRole("region", { name: "Not adopted" })).toBeNull();

    // A retained report from elsewhere still follows the direct page-originated one.
    act(() => {
      writeStore.seed([
        {
          id: `${me}/999999`,
          state: { kind: "done", uri: `${forum}/elsewhere`, report: ["Report from another tab"] },
        },
      ]);
    });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(await screen.findByText("Report from another tab")).toBeTruthy();
  });
});
