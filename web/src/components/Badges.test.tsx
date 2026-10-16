// SPDX-License-Identifier: AGPL-3.0-or-later
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BADGE_GLYPHS } from "../lib/badges.ts";
import { Badges, badgeList } from "./Badges.tsx";

describe("Badges", () => {
  it("renders the bracketed labels in a fixed order", () => {
    render(
      <Badges
        badges={{
          verified: true,
          edited: true,
          stale: true,
          futureDated: true,
          identityUnverified: true,
        }}
        completeness={{ partial: true, reposComplete: 3, reposTotal: 5, degraded: false }}
      />,
    );
    const labels = [...document.querySelectorAll("[data-badge]")].map((b) =>
      b.textContent?.replace(/^[^[]*/, ""),
    );
    expect(labels).toEqual([
      "[verified]",
      "[edited]",
      "[stale]",
      "[partial 3/5]",
      "[future-dated]",
      "[identity unverified]",
    ]);
    expect(screen.getByText("[partial 3/5]")).toBeTruthy();
  });

  it("uses only glyphs from BADGE_GLYPHS", () => {
    const all = badgeList(
      {
        verified: true,
        edited: true,
        futureDated: true,
        identityUnverified: true,
        stale: true,
        hint: true,
        inconsistentTimestamps: true,
      },
      { partial: true, reposComplete: 1, reposTotal: 2, degraded: false },
    );
    for (const b of all) {
      if (b.glyph !== undefined) expect(BADGE_GLYPHS).toContain(b.glyph);
      expect(b.label).toMatch(/^\[[a-z0-9 /-]+\]$/);
    }
  });

  it("renders nothing for plain pds content", () => {
    const { container } = render(<Badges badges={{}} />);
    expect(container.innerHTML).toBe("");
  });

  it("shows [pending] for an own write in flight and [degraded] without moderation data", () => {
    render(
      <Badges
        pending
        completeness={{ partial: false, reposComplete: 2, reposTotal: 2, degraded: true }}
      />,
    );
    const labels = [...document.querySelectorAll("[data-badge]")].map((b) =>
      b.textContent?.replace(/^[^[]*/, ""),
    );
    expect(labels).toContain("[pending]");
    expect(labels).toContain("[degraded]");
    expect(labels).not.toContain("[partial 2/2]");
  });
});
