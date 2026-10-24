// SPDX-License-Identifier: AGPL-3.0-or-later
// M3 wiring: viewer caps gate the action buttons, the score view shows the not-yet-counted
// and earlier-version notes, an uncounted own react is marked, and the shared Banners
// component renders each banner kind (social.md §5, §7, §8, §9.2; engine-api.md banners).

import type { ReactTally, ScoreView, ViewerCaps } from "@lessdugin/engine/types";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithApp } from "../test/render.tsx";
import { Banners } from "./Banners.tsx";
import { ReactBar } from "./Reacts.tsx";
import { VoteControls } from "./VoteControls.tsx";

/** A complete ViewerCaps with every cap on, for tests that flip one field. */
function caps(over: Partial<ViewerCaps> = {}): ViewerCaps {
  return {
    vote: true,
    strongVote: true,
    agree: true,
    weighted: true,
    react: true,
    reactCreate: true,
    reactSecond: true,
    reactAnti: true,
    post: true,
    comment: true,
    newcomer: false,
    applicant: false,
    moderate: false,
    curate: false,
    authorModPersonal: false,
    authorModFrontpage: false,
    ...over,
  };
}

const SCORE: ScoreView = { score: 5, unweighted: 0, earlierVersionVotes: 0 };

describe("VoteControls caps gating", () => {
  it("disables karma buttons when the fold says `vote` is false", async () => {
    renderWithApp(<VoteControls subject="at://x/y/z" score={SCORE} caps={caps({ vote: false })} />);
    const group = await screen.findByRole("group", { name: "Karma vote" });
    for (const b of within(group).getAllByRole("button")) {
      expect((b as HTMLButtonElement).disabled).toBe(true);
    }
    expect(
      (within(group).getByRole("button", { name: /^Upvote/ }) as HTMLElement).title,
    ).toBeTruthy();
  });

  it("disables the agreement axis with `agree` false but keeps karma enabled", async () => {
    renderWithApp(
      <VoteControls
        subject="at://x/y/z"
        score={SCORE}
        mode="twoAxis"
        caps={caps({ agree: false })}
      />,
    );
    for (const b of within(
      await screen.findByRole("group", { name: "Agreement vote" }),
    ).getAllByRole("button")) {
      expect((b as HTMLButtonElement).disabled).toBe(true);
    }
    for (const b of within(await screen.findByRole("group", { name: "Karma vote" })).getAllByRole(
      "button",
    )) {
      expect((b as HTMLButtonElement).disabled).toBe(false);
    }
  });

  it("never disables from an absent caps (signed out leads to sign-in instead)", async () => {
    renderWithApp(<VoteControls subject="at://x/y/z" score={SCORE} />);
    for (const b of within(await screen.findByRole("group", { name: "Karma vote" })).getAllByRole(
      "button",
    )) {
      expect((b as HTMLButtonElement).disabled).toBe(false);
    }
  });
});

describe("VoteControls score notes", () => {
  it("shows 'n votes not yet counted' from `unweighted`", async () => {
    renderWithApp(
      <VoteControls subject="at://x/y/z" size="post" score={{ ...SCORE, unweighted: 4 }} />,
    );
    expect(await screen.findByText("4 votes not yet counted")).toBeTruthy();
  });

  it("shows 'N of M votes were cast on an earlier version'", async () => {
    renderWithApp(
      <VoteControls
        subject="at://x/y/z"
        size="post"
        score={{ ...SCORE, earlierVersionVotes: 3, weightedVotes: 12 }}
      />,
    );
    expect(await screen.findByText(/3 of 12 votes were cast on an earlier version/)).toBeTruthy();
  });

  it("marks the viewer's own not-yet-counted vote", async () => {
    renderWithApp(
      <VoteControls
        subject="at://x/y/z"
        score={{ ...SCORE, mine: { karma: "up", notYetCounted: true } }}
      />,
    );
    expect((await screen.findByTestId("not-yet-counted")).textContent).toMatch(/will count once/);
  });
});

describe("ReactBar caps gating and uncounted reacts", () => {
  it("disables the add-react trigger when `reactCreate` is false", async () => {
    renderWithApp(
      <ReactBar
        subject="at://x/y/z"
        reacts={[]}
        getQuote={() => null}
        caps={caps({ reactCreate: false })}
      />,
    );
    const add = await screen.findByRole("button", { name: "Add react" });
    expect((add as HTMLButtonElement).disabled).toBe(true);
    expect(add.title).toBeTruthy();
  });

  it("leaves the add-react trigger enabled with absent caps", async () => {
    renderWithApp(<ReactBar subject="at://x/y/z" reacts={[]} getQuote={() => null} />);
    expect(
      ((await screen.findByRole("button", { name: "Add react" })) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("disables seconding and opposing by their own caps", async () => {
    const tally: ReactTally = {
      react: "heart",
      label: "Heart",
      glyph: "♥",
      for: 2,
      against: 1,
      creator: "did:plc:creator",
    };
    renderWithApp(
      <ReactBar
        subject="at://x/y/z"
        reacts={[tally]}
        getQuote={() => null}
        caps={caps({ reactSecond: false, reactAnti: false })}
      />,
    );
    const item = await screen.findByRole("listitem");
    for (const b of within(item).getAllByRole("button")) {
      expect((b as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it("marks the viewer's uncounted own react", async () => {
    const tally: ReactTally = {
      react: "heart",
      label: "Heart",
      glyph: "♥",
      for: 0,
      against: 0,
      mine: "for",
      uncounted: true,
    };
    renderWithApp(<ReactBar subject="at://x/y/z" reacts={[tally]} getQuote={() => null} />);
    expect(await screen.findByTestId("react-uncounted")).toBeTruthy();
    // The uncounted tally is still listed (it carries the viewer's stance).
    expect((await screen.findByRole("listitem")).textContent).toContain("Heart");
  });
});

describe("Banners", () => {
  it("renders nothing for absent or empty banners", async () => {
    const { container } = render(<Banners banners={[]} />);
    expect(container.textContent).toBe("");
    const none = render(<Banners banners={undefined} />);
    expect(none.container.textContent).toBe("");
  });

  it("renders each banner kind with its code and message", async () => {
    render(
      <Banners
        banners={[
          { code: "moderationUnavailable", message: "Moderation data is unavailable." },
          { code: "newerRules", message: "The rules changed." },
        ]}
      />,
    );
    expect(screen.getByText(/Moderation data is unavailable/)).toBeTruthy();
    expect(document.querySelector('[data-banner="moderationUnavailable"]')).toBeTruthy();
    expect(document.querySelector('[data-banner="newerRules"]')).toBeTruthy();
  });

  it("renders unknown codes with their message as-is", async () => {
    render(<Banners banners={[{ code: "futureThing", message: "Something new." }]} />);
    expect(screen.getByText("Something new.")).toBeTruthy();
    expect(document.querySelector('[data-banner="futureThing"]')).toBeTruthy();
  });
});
