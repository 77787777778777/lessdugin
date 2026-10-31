// SPDX-License-Identifier: AGPL-3.0-or-later
// The open-mode label (threat-model L7, social.md §3.2; review findings m3-6, sec-6): the forum
// header carries it for an open forum even when the engine sends no banner, and the forum form
// says it when Open is chosen.
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BANNER_KIND, Banners, OPEN_MODE_LABEL, withOpenModeLabel } from "./Banners.tsx";
import { EMPTY_FORUM_DRAFT, ForumForm } from "./ForumForm.tsx";

describe("open-mode label", () => {
  it("is added for an open forum, once, and never for approval mode", () => {
    expect(withOpenModeLabel(undefined, "open")).toEqual([
      { code: "relayDependent", message: OPEN_MODE_LABEL },
    ]);
    const sent = [{ code: "relayDependent", message: "from the engine" }];
    expect(withOpenModeLabel(sent, "open")).toBe(sent);
    expect(withOpenModeLabel(undefined, "approval")).toBeUndefined();
  });

  it("renders with its hint", () => {
    render(<Banners banners={withOpenModeLabel([], "open")} />);
    const p = document.querySelector('[data-banner="relayDependent"]');
    expect(p?.textContent).toContain(
      "Open mode relies on Bluesky relay, Jetstream and Constellation",
    );
    expect(p?.textContent).toContain(BANNER_KIND.relayDependent?.hint ?? "?");
  });

  it("explains newer rules as unknown values, not earlier judgements (social.md §1.6)", () => {
    expect(BANNER_KIND.newerRules?.hint).not.toMatch(/earlier version/);
    expect(BANNER_KIND.newerRules?.hint).toMatch(/does not know/);
  });

  it("is shown in the forum form when Open is chosen", () => {
    render(
      <ForumForm
        initial={EMPTY_FORUM_DRAFT}
        creating
        submitLabel="Create"
        onSubmit={async () => true}
      />,
    );
    expect(screen.queryByTestId("open-mode-label")).toBeNull();
    fireEvent.click(screen.getByRole("radio", { name: "Open" }));
    expect(screen.getByTestId("open-mode-label").textContent).toContain(OPEN_MODE_LABEL);
  });
});
